package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.Grouping;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;

/**
 * One Indian column beside a Western one in one read, and the retired policy word: which a
 * {@link Column} here cannot hold, so that a plan has no door to bring it in by, and which the
 * core refuses at the one door left, {@link Native}'s fills, on whichever path this JDK takes.
 */
final class GroupingTest {
    private static final byte[] BYTES =
            "indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n".getBytes(StandardCharsets.UTF_8);
    private static final NumFormat INDIAN = NumFormat.INVARIANT.withGrouping(Grouping.INDIAN);

    private static byte[] utf8(String text) {
        return text.getBytes(StandardCharsets.UTF_8);
    }

    private static Path workbook(String file) {
        return CorpusTest.CORPUS_DIRECTORY.resolve("workbook").resolve(file);
    }

    private static int i32(Batch batch, int column, int row) {
        return batch.values(column).getAtIndex(ValueLayout.JAVA_INT, row);
    }

    @Test
    void thePoliciesAreTheWordsTheySay() {
        assertEquals(95, NumFormat.INVARIANT.flags());
        assertEquals(607, INDIAN.flags());
        assertEquals(95, new NumFormat('.', ',', NumFormat.STYLE_ALL).flags());
    }

    /** Every way there is of putting the retired bit in a format that a column could then hold. */
    @Test
    void aColumnCannotHoldAStaleWord() throws Exception {
        assertThrows(IllegalArgumentException.class, () -> Grouping.of(1 << 8));
        assertThrows(IllegalArgumentException.class, () -> Grouping.of(3 << 8));
        assertThrows(IllegalArgumentException.class, () -> new NumFormat('.', ',', 351));
        assertThrows(IllegalArgumentException.class, () -> new NumFormat('.', ',', 863));
        assertThrows(IllegalArgumentException.class, () -> NumFormat.INVARIANT.withStyles(351));
        // A record's fields are not for reflection to write, accessible or not.
        Field styles = NumFormat.class.getDeclaredField("styles");
        styles.setAccessible(true);
        assertThrows(IllegalAccessException.class, () -> styles.setInt(NumFormat.INVARIANT, 351));
        assertEquals(95, NumFormat.INVARIANT.flags());
        // The control: the same call with a word that means something makes a format.
        assertEquals(607, new NumFormat('.', ',', NumFormat.STYLE_ALL, Grouping.of(2 << 8)).flags());
    }

    @Test
    void anIndianColumnReadsBesideAWesternOne() {
        // Twice: named, and with no policy said, which is Western.
        for (NumFormat western : List.of(NumFormat.INVARIANT, new NumFormat('.', ',', NumFormat.STYLE_ALL))) {
            try (DelimitedReader reader =
                    DelimitedReader.of(BYTES, Dialect.CSV, List.of(Column.i32(0, INDIAN), Column.i32(1, western)))) {
                Batch batch = reader.read();
                assertEquals(2, batch.rows());
                assertTrue(batch.isOk(0, 0));
                assertEquals(123456, i32(batch, 0, 0));
                assertEquals(CastFailure.MALFORMED, batch.verdict(0, 1).reason());
                assertEquals(CastFailure.MALFORMED, batch.verdict(1, 0).reason());
                assertTrue(batch.isOk(1, 1));
                assertEquals(1234567, i32(batch, 1, 1));
            }
        }
    }

    @Test
    void aColumnWithNoFormatIsWestern() {
        try (DelimitedReader reader = DelimitedReader.of(BYTES, Dialect.CSV, List.of(Column.i32(0)))) {
            Batch batch = reader.read();
            assertEquals(CastFailure.MALFORMED, batch.verdict(0, 0).reason());
            assertEquals(1234567, i32(batch, 0, 1));
            assertEquals(95, Column.i32(0).format().flags());
        }
    }

    @ParameterizedTest
    @ValueSource(strings = {"grouping.xlsx", "grouping.ods", "grouping.numbers"})
    void anIndianSheetColumnReadsBesideAWesternOne(String file) {
        try (Workbook book = Workbook.open(workbook(file));
                Sheet sheet = book.sheet(
                        0, SheetOptions.DEFAULT, List.of(Column.i32(2, INDIAN), Column.i32(2, NumFormat.INVARIANT)))) {
            Batch batch = sheet.read();
            assertEquals(3, batch.rows());
            assertTrue(batch.isOk(0, 0));
            assertEquals(123456, i32(batch, 0, 0));
            assertEquals(CastFailure.MALFORMED, batch.verdict(0, 1).reason());
            assertEquals(1234, i32(batch, 0, 2));
            assertEquals(CastFailure.MALFORMED, batch.verdict(1, 0).reason());
            assertTrue(batch.isOk(1, 1));
            assertEquals(1234567, i32(batch, 1, 1));
            assertEquals(1234, i32(batch, 1, 2));
        }
    }

    /** One column of a size-tagged plan, told {@code flags}, past everything a {@link Column} checks. */
    private static MemorySegment[] handBuilt(Arena arena, int flags) {
        // 95's bytes, with only the flags word changed.
        MemorySegment spec = arena.allocate(Abi.SPEC_WITH_BYTES, 8);
        spec.set(ValueLayout.JAVA_INT, 0, (int) Abi.SPEC_WITH_BYTES);
        spec.set(ValueLayout.JAVA_INT, Abi.SPEC_WITH_DOOR, Door.I32.code());
        NativeValues.writeFormat(NumFormat.INVARIANT, spec, Abi.SPEC_WITH_FORMAT);
        spec.set(ValueLayout.JAVA_INT, Abi.SPEC_WITH_FORMAT + 4, flags);
        MemorySegment column = arena.allocate(Abi.BUFFER_WITH_BYTES, 8);
        column.set(ValueLayout.JAVA_INT, 0, (int) Abi.BUFFER_WITH_BYTES);
        MemorySegment values = arena.allocate(64 * 4, 8);
        column.set(ValueLayout.ADDRESS, Abi.BUFFER_WITH_VALUES, values);
        column.set(ValueLayout.ADDRESS, Abi.BUFFER_WITH_VERDICTS, arena.allocate(64 * Abi.VERDICT_BYTES, 4));
        return new MemorySegment[] {spec, column, values};
    }

    /**
     * Told the retired word past a {@link Column}, a delimited fill is a contract error before any
     * cell; told Western or Indian, it reads.
     */
    @ParameterizedTest
    @ValueSource(ints = {351, 863, 95, 607})
    void aStaleWordAtTheDelimitedDoorIsTheCoresContractError(int flags) {
        boolean stale = flags == 351 || flags == 863;
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment state = arena.allocate(Native.stateSize(), 8);
            MemorySegment options = arena.allocate(Abi.DELIMITED_OPTIONS_BYTES, 4);
            options.set(ValueLayout.JAVA_INT, 0, (int) Abi.DELIMITED_OPTIONS_BYTES);
            options.set(ValueLayout.JAVA_BYTE, Abi.DELIMITED_OPTIONS_SEPARATOR, (byte) ',');
            options.set(ValueLayout.JAVA_BYTE, Abi.DELIMITED_OPTIONS_QUOTING, (byte) 1);
            options.set(ValueLayout.JAVA_BYTE, Abi.DELIMITED_OPTIONS_SKIP_BLANK_LINES, (byte) 1);
            assertEquals(Abi.OK, Native.initWith(state, options));

            MemorySegment[] plan = handBuilt(arena, flags);
            MemorySegment buffers = arena.allocate(Abi.BUFFERS_WITH_BYTES, 8);
            buffers.set(ValueLayout.JAVA_INT, 0, (int) Abi.BUFFERS_WITH_BYTES);
            buffers.set(ValueLayout.ADDRESS, Abi.BUFFERS_WITH_FIELDS + Abi.BUFFERS_ARENA, arena.allocate(4096));
            buffers.set(ValueLayout.JAVA_LONG, Abi.BUFFERS_WITH_FIELDS + Abi.BUFFERS_ARENA + 8, 4096);
            buffers.set(ValueLayout.ADDRESS, Abi.BUFFERS_WITH_FIELDS + Abi.BUFFERS_CELLS, arena.allocate(64 * 8, 4));
            buffers.set(ValueLayout.JAVA_LONG, Abi.BUFFERS_WITH_FIELDS + Abi.BUFFERS_CELLS + 8, 64);

            byte[] text = utf8("\"1,234\"\n");
            MemorySegment input = arena.allocate(text.length);
            MemorySegment.copy(text, 0, input, ValueLayout.JAVA_BYTE, 0, text.length);
            MemorySegment filled = arena.allocate(Abi.FILLED_BYTES, 8);
            int code = Native.fillWith(state, input, text.length, true, plan[0], plan[1], 1, 64, buffers, filled);
            assertEquals(stale ? Abi.ERR_CONTRACT : Abi.OK, code);
            if (!stale) {
                assertEquals(1, filled.get(ValueLayout.JAVA_LONG, Abi.FILLED_ROWS));
                assertEquals(1234, plan[2].get(ValueLayout.JAVA_INT, 0));
            }
        }
    }

    /** The same at the sheet's fill. */
    @ParameterizedTest
    @ValueSource(ints = {351, 863, 95, 607})
    void aStaleWordAtTheSheetDoorIsTheCoresContractError(int flags) {
        boolean stale = flags == 351 || flags == 863;
        try (Workbook book = Workbook.open(workbook("basic.xlsx"));
                Arena arena = Arena.ofConfined()) {
            SheetInfo info = book.sheets().get(0);
            MemorySegment state = arena.allocate(book.state().byteSize(), 8);
            state.copyFrom(book.state());
            MemorySegment part = Columns.held(info.part(), arena).asSlice(0, info.part().length);
            MemorySegment options = arena.allocate(Abi.SHEET_OPTIONS_BYTES, 4);
            options.set(ValueLayout.JAVA_INT, 0, (int) Abi.SHEET_OPTIONS_BYTES);
            options.set(ValueLayout.JAVA_INT, Abi.SHEET_OPTIONS_HAS_HEADER, 0);
            MemorySegment filled = arena.allocate(Abi.FILLED_BYTES, 8);
            assertEquals(Abi.OK, Native.sheetWith(state, book.container(), part, info.index(), options, filled));

            MemorySegment[] plan = handBuilt(arena, flags);
            Workbook.Scratch scratch = new Workbook.Scratch(arena, 1 << 20, 1 << 20, 4096, 64);
            try {
                MemorySegment buffers = arena.allocate(Abi.BUFFERS_WITH_BYTES, 8);
                MemorySegment.copy(scratch.buffers(book), 0, buffers, Abi.BUFFERS_WITH_FIELDS, Abi.BUFFERS_BYTES);
                buffers.set(ValueLayout.JAVA_INT, 0, (int) Abi.BUFFERS_WITH_BYTES);
                int code = Native.bookFillWith(state, book.container(), plan[0], plan[1], 1, 16, buffers, filled);
                assertEquals(stale ? Abi.ERR_CONTRACT : Abi.OK, code);
                if (!stale) {
                    assertTrue(filled.get(ValueLayout.JAVA_LONG, Abi.FILLED_ROWS) > 0);
                }
            } finally {
                scratch.close();
            }
        }
    }

    /** And at a suggestion's, past everything {@link SuggestOptions} checks. */
    @ParameterizedTest
    @ValueSource(ints = {351, 863, 95, 607})
    void aStaleWordAtTheSuggestDoorIsTheCoresContractError(int flags) {
        boolean stale = flags == 351 || flags == 863;
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment input = arena.allocate(1);
            input.set(ValueLayout.JAVA_BYTE, 0, (byte) '1');
            MemorySegment cells = arena.allocate(Abi.SPAN_BYTES, 4);
            cells.set(ValueLayout.JAVA_INT, 4, 1);
            MemorySegment formats = arena.allocate(NativeValues.FORMAT_BYTES, 4);
            NativeValues.writeFormat(NumFormat.INVARIANT, formats, 0);
            formats.set(ValueLayout.JAVA_INT, 4, flags);
            MemorySegment found = arena.allocate(Abi.SUGGESTION_BYTES, 4);
            int code = Native.suggest(input, 1, MemorySegment.NULL, 0, cells, MemorySegment.NULL, 1, 1, formats, found);
            assertEquals(stale ? Abi.ERR_CONTRACT : Abi.OK, code);
        }
    }

    /**
     * A plan's formats can't be stale, so a suggest has nothing to refuse first: {@link
     * #aColumnCannotHoldAStaleWord} is that refusal, at the constructor. Each declaration here goes
     * as far as its source does, and the source's own word is what ends it.
     */
    @Test
    void aDeclaredFormatGoesAsFarAsTheSource(@TempDir Path directory) throws IOException {
        Dialect headerless = Dialect.CSV.withHeader(false);
        for (NumFormat format : List.of(NumFormat.INVARIANT, INDIAN)) {
            SuggestOptions every = SuggestOptions.DEFAULT.withFormat(format);
            SuggestOptions past = SuggestOptions.DEFAULT.withColumnFormat(99, format);
            SuggestOptions replaced = every.withColumnFormat(0, INDIAN);
            assertEquals(
                    1,
                    DelimitedReader.suggest(utf8("a\n1\n"), Dialect.CSV, every)
                            .plan()
                            .size());
            assertEquals(
                    0,
                    DelimitedReader.suggest(new byte[0], Dialect.CSV, every)
                            .plan()
                            .size());
            assertEquals(
                    0,
                    DelimitedReader.suggest(new byte[0], headerless, every)
                            .plan()
                            .size());
            assertEquals(
                    1,
                    DelimitedReader.suggest(utf8("a\n1\n"), Dialect.CSV, past)
                            .plan()
                            .size());
            assertEquals(
                    1,
                    DelimitedReader.suggest(utf8("a\n1\n"), Dialect.CSV, replaced)
                            .plan()
                            .size());
            assertThrows(TabularException.class, () -> DelimitedReader.suggest(utf8("a\n\"1\n"), Dialect.CSV, every));
            assertThrows(TabularException.class, () -> DelimitedReader.suggest(utf8("\"1\n"), headerless, every));
            Path missing = directory.resolve("no-such-file.csv");
            assertThrows(UncheckedIOException.class, () -> DelimitedReader.suggest(missing, Dialect.CSV, every));
            try (Workbook book = Workbook.open(workbook("grouping.xlsx"));
                    Sheet sheet = book.sheet(0, SheetOptions.DEFAULT)) {
                assertFalse(sheet.suggest(every).plan().isEmpty());
                assertFalse(sheet.suggest(past).plan().isEmpty());
                assertFalse(sheet.suggest(replaced).plan().isEmpty());
            }
            try (Workbook broken = Workbook.open(workbook("broken.xlsx"));
                    Sheet part = broken.sheet(0, SheetOptions.DEFAULT)) {
                assertThrows(TabularException.class, () -> part.suggest(every));
            }
        }
        // The stream and path doors read what the memory door does.
        Path file = directory.resolve("grouping.csv");
        Files.write(file, BYTES);
        List<Column> plan = List.of(Column.i32(0, INDIAN));
        try (DelimitedReader stream = DelimitedReader.of(new ByteArrayInputStream(BYTES), Dialect.CSV, plan);
                DelimitedReader path = DelimitedReader.open(file, Dialect.CSV, plan)) {
            assertEquals(123456, i32(stream.read(), 0, 0));
            assertEquals(123456, i32(path.read(), 0, 0));
        }
    }

    /** What the delimiter rule says stays: nothing says 1,23,456 is Indian. Declared, it is a number. */
    @Test
    void anIndianShapeHasNoCultureUntilOneIsDeclared() {
        SuggestedColumn alone = ask("a\n\"1,23,456\"\n", SuggestOptions.DEFAULT);
        assertEquals(Door.TEXT, alone.door());
        assertEquals(SuggestStatus.RESOLVED, alone.status());
        SuggestedColumn beside = ask("a\n\"1,23,456\"\n5\n", SuggestOptions.DEFAULT);
        assertEquals(Door.TEXT, beside.door());
        assertEquals(SuggestStatus.UNRESOLVED, beside.status());
        assertEquals(
                95, ask("a\n\"1,234,567\"\n", SuggestOptions.DEFAULT).format().flags());

        SuggestOptions declared = SuggestOptions.DEFAULT.withColumnFormat(0, INDIAN);
        SuggestedColumn read = ask("a\n\"1,23,456\"\n", declared);
        assertEquals(Door.I32, read.door());
        assertEquals(SuggestStatus.RESOLVED, read.status());
        assertEquals(INDIAN, read.format());
        assertEquals(607, read.format().flags());
        SuggestedColumn western = ask("a\n\"1,234,567\"\n", declared);
        assertEquals(Door.TEXT, western.door());
        assertNull(western.format());
    }

    private static SuggestedColumn ask(String text, SuggestOptions options) {
        return DelimitedReader.suggest(utf8(text), Dialect.CSV, options)
                .report()
                .get(0);
    }
}
