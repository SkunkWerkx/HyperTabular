package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import java.io.ByteArrayInputStream;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.NoSuchElementException;
import java.util.function.Supplier;
import org.junit.jupiter.api.Test;

/**
 * Replays {@code corpus/workbook.json} — the contract every binding replays, and the one the
 * Rust binding replays — through this binding: each package opened from a byte array, read
 * from a stream and mapped from its path, each sheet read by index and (where the name finds
 * it) by name, opened with its plan and header-first with the plan bound after, in batches of
 * one row, of two, and of more than any sheet has.
 */
final class WorkbookCorpusTest {
    private static final int[] BATCH_ROWS = {1, 2, 1024};

    private static SheetOptions optionsOf(JsonObject settings, int batchRows) {
        SheetOptions options = new SheetOptions(
                settings.get("has_header").getAsBoolean(),
                settings.get("skip_empty_rows").getAsBoolean(),
                batchRows);
        if (settings.has("skip")) {
            options = options.withSkip(settings.get("skip").getAsInt());
        }
        if (settings.has("header_row")) {
            options = options.withHeaderRow(settings.get("header_row").getAsInt());
        }
        if (settings.has("range")) {
            options = options.withRange(Range.parse(settings.get("range").getAsString()));
        }
        if (settings.has("tokens")) {
            options = options.withTokens(CorpusTest.tokensOf(settings.getAsJsonArray("tokens")));
        }
        return options;
    }

    @Test
    void theWorkbookCorpus() throws Exception {
        replay();
    }

    /**
     * The corpus again with every buffer starting at one element and never asked to be large
     * up front: each read stops wherever the core runs out of room and resumes in a grown
     * buffer that has to have kept what the old one held. A grow that dropped it would read
     * garbage, and the corpus would say so.
     */
    @Test
    void theWorkbookCorpusWithBuffersThatStartWithNoRoom() throws Exception {
        Workbook.stingy = true;
        Batch.stingyChars = true;
        java.util.Arrays.fill(Workbook.grown, 0);
        Batch.charsGrown = 0;
        try {
            replay();
        } finally {
            Workbook.stingy = false;
            Batch.stingyChars = false;
        }
        // The char arena, too, started with room for one cell and was replaced every time a
        // cell did not fit, every cell's chars still its string.
        assertTrue(Batch.charsGrown > 1_000, "chars grown " + Batch.charsGrown);
        // Not once per call: many times, mid-part, for each of the three.
        System.out.println("grown: " + java.util.Arrays.toString(Workbook.grown));
        for (long times : Workbook.grown) {
            assertTrue(times > 1_000, java.util.Arrays.toString(Workbook.grown));
        }
    }

    private static void replay() throws Exception {
        JsonArray corpus = CorpusTest.load("workbook.json");
        assertTrue(corpus.size() >= 398, "the corpus has " + corpus.size() + " cases");
        assertEquals(List.of(), CorpusKeys.unknownWorkbook(corpus));
        int cells = 0;
        for (JsonElement element : corpus) {
            JsonObject vector = element.getAsJsonObject();
            String name = vector.get("name").getAsString();
            Path path = CorpusTest.CORPUS_DIRECTORY.resolve(vector.get("file").getAsString());
            byte[] bytes = Files.readAllBytes(path);
            Map<String, Supplier<Workbook>> openings = Map.of(
                    "memory",
                    () -> Workbook.of(bytes),
                    "path",
                    () -> Workbook.open(path),
                    "stream",
                    () -> Workbook.of(new ByteArrayInputStream(bytes)));

            // A package the core refuses: every way of opening it gives the one failure.
            if (!vector.has("sheet")) {
                for (Map.Entry<String, Supplier<Workbook>> opening : openings.entrySet()) {
                    CorpusTest.assertFailure(
                            name + " (" + opening.getKey() + ")",
                            assertThrows(
                                    TabularException.class,
                                    () -> opening.getValue().get().close(),
                                    name),
                            vector);
                }
                continue;
            }

            List<Column> plan = new ArrayList<>();
            for (JsonElement entry : vector.getAsJsonArray("plan")) {
                plan.add(CorpusTest.columnOf(entry.getAsJsonObject()));
            }
            int index = vector.get("sheet").getAsInt();
            JsonArray sheets = vector.getAsJsonArray("sheets");
            String sheetName = sheets.get(index).getAsJsonObject().get("name").getAsString();
            JsonObject settings = vector.getAsJsonObject("options");
            JsonArray rows = vector.getAsJsonArray("rows");
            JsonArray numbers = vector.getAsJsonArray("numbers");
            cells += rows.size() * plan.size();

            for (Map.Entry<String, Supplier<Workbook>> opening : openings.entrySet()) {
                try (Workbook book = opening.getValue().get()) {
                    assertEquals(
                            switch (vector.get("format").getAsString()) {
                                case "xlsx" -> WorkbookFormat.XLSX;
                                case "numbers" -> WorkbookFormat.NUMBERS;
                                default -> WorkbookFormat.ODS;
                            },
                            book.format(),
                            name);
                    assertEquals(ExcelEpoch.values()[vector.get("epoch").getAsInt() - 1], book.dateSystem(), name);
                    List<String> listed = new ArrayList<>();
                    List<String> expected = new ArrayList<>();
                    for (SheetInfo sheet : book.sheets()) {
                        listed.add(sheet.name() + (sheet.hidden() ? " (hidden)" : ""));
                    }
                    for (JsonElement sheet : sheets) {
                        JsonObject entry = sheet.getAsJsonObject();
                        expected.add(entry.get("name").getAsString()
                                + (entry.get("hidden").getAsBoolean() ? " (hidden)" : ""));
                    }
                    assertEquals(expected, listed, name);
                    boolean byName =
                            book.sheets().stream().map(SheetInfo::name).toList().indexOf(sheetName) == index;

                    for (int batchRows : BATCH_ROWS) {
                        SheetOptions options = optionsOf(settings, batchRows);
                        for (boolean named : new boolean[] {false, true}) {
                            if (named && !byName) {
                                continue;
                            }
                            for (boolean headerFirst : new boolean[] {false, true}) {
                                String label = name + ": " + opening.getKey() + ", " + batchRows + " rows a batch"
                                        + (named ? ", by name" : "") + (headerFirst ? ", header first" : "");
                                try (Sheet sheet = headerFirst
                                        ? (named ? book.sheet(sheetName, options) : book.sheet(index, options))
                                        : (named
                                                ? book.sheet(sheetName, options, plan)
                                                : book.sheet(index, options, plan))) {
                                    replay(label, sheet, batchRows, plan, rows, numbers, vector);
                                }
                            }
                        }
                    }
                }
            }
        }
        assertTrue(cells >= 12_000, cells + " cells");
    }

    private static void replay(
            String label,
            Sheet sheet,
            int batchRows,
            List<Column> plan,
            JsonArray rows,
            JsonArray numbers,
            JsonObject vector) {
        JsonElement header = vector.get("header");
        if (header.isJsonNull()) {
            assertNull(sheet.header(), label);
        } else {
            List<String> names = new ArrayList<>();
            header.getAsJsonArray().forEach(entry -> names.add(entry.getAsString()));
            assertEquals(names, sheet.header(), label);
        }
        if (!sheet.isBound()) {
            CorpusTest.bindHeaderFirst(
                    label, sheet.header(), sheet.plan(), sheet.isBound(), sheet::read, sheet::bind, plan);
        }
        assertEquals(plan, sheet.plan(), label);

        int seen = 0;
        List<String> problems = new ArrayList<>();
        TabularException failure = null;
        try {
            for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                assertTrue(batch.rows() >= 1 && batch.rows() <= batchRows, label);
                CorpusTest.assertRows(label, batch);
                CorpusTest.appendProblems(label, problems, batch, seen);
                for (int row = 0; row < batch.rows(); row++, seen++) {
                    assertTrue(seen < rows.size(), label + ": more rows than the corpus lists");
                    assertEquals(numbers.get(seen).getAsInt(), batch.line(row), label + ", row " + seen);
                    JsonArray cells = rows.get(seen).getAsJsonArray();
                    assertEquals(plan.size(), cells.size(), label + ", row " + seen);
                    for (int column = 0; column < plan.size(); column++) {
                        CorpusTest.assertCell(
                                label + ", row " + seen + ", column " + column,
                                batch,
                                column,
                                row,
                                cells.get(column).getAsJsonObject());
                    }
                }
            }
        } catch (TabularException e) {
            failure = e;
            // A failed sheet stays failed: the same failure, again.
            assertSame(e, assertThrows(TabularException.class, sheet::read), label);
        }
        assertEquals(rows.size(), seen, label);
        CorpusTest.assertFailure(label, failure, vector);
        CorpusTest.assertProblems(label, problems, vector, true);
    }

    /**
     * Every case read with notes off, once, from memory: refused cells keep their verdicts and lose
     * what they say of why.
     */
    @Test
    void theWorkbookCorpusWithNotesOff() throws Exception {
        for (JsonElement element : CorpusTest.load("workbook.json")) {
            JsonObject vector = element.getAsJsonObject();
            if (!vector.has("sheet")) {
                continue;
            }
            String name = vector.get("name").getAsString();
            List<Column> plan = new ArrayList<>();
            for (JsonElement entry : vector.getAsJsonArray("plan")) {
                plan.add(CorpusTest.columnOf(entry.getAsJsonObject()));
            }
            JsonArray rows = vector.getAsJsonArray("rows");
            Path path = CorpusTest.CORPUS_DIRECTORY.resolve(vector.get("file").getAsString());
            SheetOptions options = optionsOf(vector.getAsJsonObject("options"), DelimitedReader.DEFAULT_BATCH_ROWS)
                    .withNotes(false);
            try (Workbook book = Workbook.of(Files.readAllBytes(path));
                    Sheet sheet = book.sheet(vector.get("sheet").getAsInt(), options, plan)) {
                int seen = 0;
                List<String> problems = new ArrayList<>();
                TabularException failure = null;
                try {
                    for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                        CorpusTest.appendProblems(name, problems, batch, seen);
                        for (int row = 0; row < batch.rows(); row++, seen++) {
                            JsonArray cells = rows.get(seen).getAsJsonArray();
                            for (int column = 0; column < plan.size(); column++) {
                                CorpusTest.assertCell(
                                        name + ", row " + seen + ", column " + column,
                                        batch,
                                        column,
                                        row,
                                        cells.get(column).getAsJsonObject(),
                                        false);
                            }
                        }
                    }
                } catch (TabularException e) {
                    failure = e;
                }
                assertEquals(rows.size(), seen, name);
                CorpusTest.assertFailure(name, failure, vector);
                CorpusTest.assertProblems(name, problems, vector, false);
            }
        }
    }

    @Test
    void everyWorkbookCorpusKeyIsRead() throws Exception {
        CorpusKeys.aJunkKeyIsSeenEverywhere("workbook.json", CorpusKeys::unknownWorkbook);
    }

    /** Every case that says what to suggest, from its own sheet under its own options. */
    @Test
    void everySheetSuggestionIsWhatTheCorpusSays() throws Exception {
        int seen = 0;
        for (JsonElement element : CorpusTest.load("workbook.json")) {
            JsonObject vector = element.getAsJsonObject();
            if (!vector.has("suggest")) {
                continue;
            }
            seen++;
            String name = vector.get("name").getAsString();
            JsonObject suggest = vector.getAsJsonObject("suggest");
            Path path = CorpusTest.CORPUS_DIRECTORY.resolve(vector.get("file").getAsString());
            SheetOptions options = optionsOf(vector.getAsJsonObject("options"), DelimitedReader.DEFAULT_BATCH_ROWS);
            try (Workbook book = Workbook.of(Files.readAllBytes(path));
                    Sheet sheet = book.sheet(vector.get("sheet").getAsInt(), options)) {
                CorpusTest.assertSuggestion(name, sheet.suggest(CorpusTest.suggestOptionsOf(suggest)), suggest);
            }
        }
        assertTrue(seen >= 15, seen + " cases say what to suggest");
    }

    @Test
    void whatIsNotThereIsAnExceptionOfItsOwn() {
        Path basic = CorpusTest.CORPUS_DIRECTORY.resolve("workbook").resolve("basic.xlsx");
        List<Column> plan = List.of(Column.text(0));
        try (Workbook book = Workbook.open(basic)) {
            assertThrows(NoSuchElementException.class, () -> book.sheet("No such sheet", SheetOptions.DEFAULT, plan));
            assertThrows(IndexOutOfBoundsException.class, () -> book.sheet(99, SheetOptions.DEFAULT, plan));
            assertThrows(
                    IllegalArgumentException.class, () -> book.sheet(0, SheetOptions.DEFAULT.withBatchRows(0), plan));

            // Two sheets read at once, each with its own buffers, over one workbook.
            Sheet first = book.sheet(0, SheetOptions.DEFAULT, plan);
            Sheet second = book.sheet(0, SheetOptions.DEFAULT.withHeader(false), plan);
            int a = first.read().rows();
            int b = second.read().rows();
            assertEquals(a + 1, b, "the header is one row more");

            book.close();
            assertThrows(IllegalStateException.class, first::read);
            assertThrows(IllegalStateException.class, () -> book.sheet(0, SheetOptions.DEFAULT, plan));
            first.close();
            second.close();
        }
        UncheckedIOException missing = assertThrows(
                UncheckedIOException.class, () -> Workbook.open(basic.resolveSibling("no-such-file.xlsx")));
        assertTrue(missing.getCause() instanceof NoSuchFileException);
        TabularException notAZip = assertThrows(
                TabularException.class, () -> Workbook.of("not a zip at all".getBytes(StandardCharsets.UTF_8)));
        assertEquals(TabularFailure.NOT_A_ZIP, notAZip.failure());
    }

    /** A stream that says whether it was closed, and can be made to fail part-way. */
    private static final class Watched extends java.io.InputStream {
        private final ByteArrayInputStream bytes;
        private final int failAt;
        private int read;
        boolean closed;

        Watched(byte[] bytes, int failAt) {
            this.bytes = new ByteArrayInputStream(bytes);
            this.failAt = failAt;
        }

        @Override
        public int read() throws java.io.IOException {
            byte[] one = new byte[1];
            return read(one, 0, 1) < 0 ? -1 : one[0] & 0xFF;
        }

        @Override
        public int read(byte[] into, int offset, int length) throws java.io.IOException {
            if (read >= failAt) {
                throw new java.io.IOException("the stream broke");
            }
            // A short read at a time, so the workbook's memory has to grow to hold it.
            int got = bytes.read(into, offset, Math.min(length, 1000));
            read += Math.max(got, 0);
            return got;
        }

        @Override
        public void close() {
            closed = true;
        }
    }

    @Test
    void aWorkbookReadFromAStreamOwnsWhatItRead() throws Exception {
        Path basic = CorpusTest.CORPUS_DIRECTORY.resolve("workbook").resolve("basic.xlsx");
        byte[] bytes = Files.readAllBytes(basic);
        Watched stream = new Watched(bytes, Integer.MAX_VALUE);
        List<String> header;
        int rows = 0;
        try (Workbook book = Workbook.of(stream)) {
            // Read to the end and closed before the workbook was handed out.
            assertTrue(stream.closed);
            assertEquals(WorkbookFormat.XLSX, book.format());
            try (Sheet sheet = book.sheet(0, SheetOptions.DEFAULT)) {
                header = new ArrayList<>(sheet.header());
                sheet.bind(List.of(Column.text(0)));
                for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                    rows += batch.rows();
                }
            }
        }
        try (Workbook book = Workbook.of(bytes);
                Sheet sheet = book.sheet(0, SheetOptions.DEFAULT, List.of(Column.text(0)))) {
            assertEquals(header, sheet.header());
            int expected = 0;
            for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                expected += batch.rows();
            }
            assertEquals(expected, rows);
        }

        // A stream that fails is closed, and the failure is the caller's to see.
        Watched broken = new Watched(bytes, 3000);
        assertThrows(UncheckedIOException.class, () -> Workbook.of(broken));
        assertTrue(broken.closed);
        // A stream that is not a workbook is closed too, its failure the core's.
        Watched notAZip = new Watched("not a zip".getBytes(StandardCharsets.UTF_8), Integer.MAX_VALUE);
        assertEquals(
                TabularFailure.NOT_A_ZIP,
                assertThrows(TabularException.class, () -> Workbook.of(notAZip)).failure());
        assertTrue(notAZip.closed);
        // And an empty one is an empty container.
        assertThrows(TabularException.class, () -> Workbook.of(new ByteArrayInputStream(new byte[0])));
    }

    @Test
    void aSheetStartedWithoutAPlanIsBoundOnce() {
        Path basic = CorpusTest.CORPUS_DIRECTORY.resolve("workbook").resolve("basic.xlsx");
        try (Workbook book = Workbook.open(basic)) {
            String first = book.sheets().get(0).name();
            try (Sheet sheet = book.sheet(first, SheetOptions.DEFAULT)) {
                assertFalse(sheet.isBound());
                assertEquals(List.of(), sheet.plan());
                assertThrows(IllegalStateException.class, sheet::read);
                assertThrows(IllegalStateException.class, sheet::read);
                Header header = sheet.header();
                assertEquals(0, header.ordinal(header.get(0)));
                assertThrows(NoSuchElementException.class, () -> header.ordinal("No such column"));
                assertThrows(NullPointerException.class, () -> sheet.bind(null));
                assertFalse(sheet.isBound());
                List<Column> plan = List.of(Column.text(0));
                sheet.bind(plan);
                assertTrue(sheet.isBound());
                assertEquals(plan, sheet.plan());
                assertThrows(IllegalStateException.class, () -> sheet.bind(plan));
                List<Integer> lines = new ArrayList<>();
                sheet.forEachRow(row -> lines.add(row.line()));
                assertFalse(lines.isEmpty());
                assertEquals(2, lines.get(0), "the header is row 1");
                assertNull(sheet.read());
            }
            assertThrows(NoSuchElementException.class, () -> book.sheet("No such sheet", SheetOptions.DEFAULT));
            assertThrows(IndexOutOfBoundsException.class, () -> book.sheet(99, SheetOptions.DEFAULT));
            Sheet closed = book.sheet(0, SheetOptions.DEFAULT);
            closed.close();
            assertThrows(IllegalStateException.class, () -> closed.bind(List.of(Column.text(0))));
        }
    }
}
