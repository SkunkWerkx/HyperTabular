package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.NoSuchElementException;
import java.util.function.Supplier;
import org.junit.jupiter.api.Test;

/**
 * Replays {@code corpus/workbook.json} — the contract every binding replays, and the one the
 * Rust binding replays — through this binding: each package opened from a byte array and
 * mapped from its path, each sheet read by index and (where the name finds it) by name, in
 * batches of one row, of two, and of more than any sheet has.
 */
final class WorkbookCorpusTest {
    private static final int[] BATCH_ROWS = {1, 2, 1024};

    private static void assertFailure(String label, TabularException actual, JsonObject vector) {
        if (!vector.has("failure") || vector.get("failure").isJsonNull()) {
            assertNull(actual, label);
            return;
        }
        assertNotNull(actual, label);
        JsonObject expected = vector.getAsJsonObject("failure");
        // The corpus names a failure as the core does: the constant's name, in snake case.
        assertEquals(expected.get("kind").getAsString(), actual.failure().name().toLowerCase(Locale.ROOT), label);
        assertEquals(expected.get("record").getAsLong(), actual.record(), label);
        assertEquals(expected.get("line").getAsInt(), actual.line(), label);
        assertEquals(expected.get("byte").getAsLong(), actual.byteOffset(), label);
        if (expected.has("expected")) {
            assertEquals(expected.get("expected").getAsInt(), actual.expected(), label);
            assertEquals(expected.get("found").getAsInt(), actual.found(), label);
        }
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
        java.util.Arrays.fill(Workbook.grown, 0);
        try {
            replay();
        } finally {
            Workbook.stingy = false;
        }
        // Not once per call: many times, mid-part, for each of the three.
        System.out.println("grown: " + java.util.Arrays.toString(Workbook.grown));
        for (long times : Workbook.grown) {
            assertTrue(times > 1_000, java.util.Arrays.toString(Workbook.grown));
        }
    }

    private static void replay() throws Exception {
        JsonArray corpus = CorpusTest.load("workbook.json");
        assertTrue(corpus.size() >= 80, "the corpus has " + corpus.size() + " cases");
        int cells = 0;
        for (JsonElement element : corpus) {
            JsonObject vector = element.getAsJsonObject();
            String name = vector.get("name").getAsString();
            Path path = CorpusTest.CORPUS_DIRECTORY.resolve(vector.get("file").getAsString());
            byte[] bytes = Files.readAllBytes(path);
            Map<String, Supplier<Workbook>> openings =
                    Map.of("memory", () -> Workbook.of(bytes), "path", () -> Workbook.open(path));

            // A package the core refuses: every way of opening it gives the one failure.
            if (!vector.has("sheet")) {
                for (Map.Entry<String, Supplier<Workbook>> opening : openings.entrySet()) {
                    assertFailure(
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
                            vector.get("format").getAsString().equals("xlsx")
                                    ? WorkbookFormat.XLSX
                                    : WorkbookFormat.ODS,
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
                        SheetOptions options = new SheetOptions(
                                settings.get("has_header").getAsBoolean(),
                                settings.get("skip_empty_rows").getAsBoolean(),
                                batchRows);
                        for (boolean named : new boolean[] {false, true}) {
                            if (named && !byName) {
                                continue;
                            }
                            String label = name + ": " + opening.getKey() + ", " + batchRows + " rows a batch"
                                    + (named ? ", by name" : "");
                            try (Sheet sheet =
                                    named ? book.sheet(sheetName, options, plan) : book.sheet(index, options, plan)) {
                                replay(label, sheet, batchRows, plan, rows, numbers, vector);
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

        int seen = 0;
        TabularException failure = null;
        try {
            for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                assertTrue(batch.rows() >= 1 && batch.rows() <= batchRows, label);
                for (int row = 0; row < batch.rows(); row++, seen++) {
                    assertTrue(seen < rows.size(), label + ": more rows than the corpus lists");
                    assertEquals(numbers.get(seen).getAsInt(), batch.line(row), label + ", row " + seen);
                    JsonArray cells = rows.get(seen).getAsJsonArray();
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
        assertFailure(label, failure, vector);
    }

    @Test
    void whatIsNotThereIsAnExceptionOfItsOwn() {
        Path basic = CorpusTest.CORPUS_DIRECTORY.resolve("workbook").resolve("basic.xlsx");
        List<Column> plan = List.of(Column.text(0));
        try (Workbook book = Workbook.open(basic)) {
            assertThrows(NoSuchElementException.class, () -> book.sheet("No such sheet", SheetOptions.DEFAULT, plan));
            assertThrows(IndexOutOfBoundsException.class, () -> book.sheet(99, SheetOptions.DEFAULT, plan));
            assertThrows(IllegalArgumentException.class, () -> SheetOptions.DEFAULT.withBatchRows(0));

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
}
