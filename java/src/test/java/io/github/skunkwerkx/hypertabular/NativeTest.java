package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicReference;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.ValueSource;

/**
 * What both paths check of a segment before its address crosses: an FFM downcall refuses a
 * heap segment, a closed one and another thread's, and the JNI path refuses each the same way,
 * with the same exception, before the core is called. And what no read reaches: the 1.0 exports
 * the ABI promises and no read path takes, the shapes the core reads, and Mono's limit of twelve
 * integer arguments.
 */
class NativeTest {
    /** A delimited state and a CSV dialect, ready for {@link Native#init}. */
    private static MemorySegment[] block(Arena arena) {
        MemorySegment state = arena.allocate(Native.stateSize(), 8);
        MemorySegment dialect = arena.allocate(Abi.DIALECT_BYTES);
        dialect.set(ValueLayout.JAVA_BYTE, 0, (byte) ',');
        dialect.set(ValueLayout.JAVA_BYTE, 1, (byte) 1);
        return new MemorySegment[] {state, dialect};
    }

    @Test
    void aSegmentOfThisThreadsOpenArenaCrosses() {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment[] block = block(arena);
            assertEquals(Abi.OK, Native.init(block[0], block[1]));
        }
    }

    @Test
    void aHeapSegmentIsRefused() {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment[] block = block(arena);
            MemorySegment heap = MemorySegment.ofArray(new byte[(int) Abi.DIALECT_BYTES]);
            assertThrows(IllegalArgumentException.class, () -> Native.init(block[0], heap));
        }
    }

    @Test
    void aClosedSegmentIsRefused() {
        MemorySegment[] block;
        try (Arena arena = Arena.ofConfined()) {
            block = block(arena);
        }
        MemorySegment[] closed = block;
        assertThrows(IllegalStateException.class, () -> Native.init(closed[0], closed[1]));
    }

    @Test
    void anotherThreadsSegmentIsRefused() throws InterruptedException {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment[] block = block(arena);
            AtomicReference<Throwable> thrown = new AtomicReference<>();
            Thread other = new Thread(() -> {
                try {
                    Native.init(block[0], block[1]);
                } catch (Throwable t) {
                    thrown.set(t);
                }
            });
            other.start();
            other.join();
            assertEquals(WrongThreadException.class, thrown.get().getClass(), String.valueOf(thrown.get()));
        }
    }

    private static final int CAPACITY = 64;

    /** A whole input read through the 1.0 calls, in columns of 8-byte values. */
    private record Fill10(MemorySegment filled, MemorySegment[] values, MemorySegment[] verdicts) {}

    private static MemorySegment bytes(Arena arena, byte[] text) {
        MemorySegment copy = arena.allocate(Math.max(text.length, 1));
        MemorySegment.copy(text, 0, copy, ValueLayout.JAVA_BYTE, 0, text.length);
        return copy.asSlice(0, text.length);
    }

    private static MemorySegment specs10(Arena arena, int[] doors, int[] flags) {
        MemorySegment specs = arena.allocate(doors.length * Abi.SPEC_BYTES, 4);
        for (int index = 0; index < doors.length; index++) {
            long spec = index * Abi.SPEC_BYTES;
            specs.set(ValueLayout.JAVA_INT, spec + Abi.SPEC_ORDINAL, index);
            specs.set(ValueLayout.JAVA_INT, spec + Abi.SPEC_DOOR, doors[index]);
            NativeValues.writeFormat(NumFormat.INVARIANT, specs, spec + Abi.SPEC_FORMAT);
            specs.set(ValueLayout.JAVA_INT, spec + Abi.SPEC_FORMAT + 4, flags[index]);
        }
        return specs;
    }

    private static MemorySegment columns10(Arena arena, int count, MemorySegment[] values, MemorySegment[] verdicts) {
        MemorySegment columns = arena.allocate(count * Abi.BUFFER_BYTES, 8);
        for (int index = 0; index < count; index++) {
            values[index] = arena.allocate(CAPACITY * 16L, 8);
            verdicts[index] = arena.allocate(CAPACITY * Abi.VERDICT_BYTES, 4);
            columns.set(ValueLayout.ADDRESS, index * Abi.BUFFER_BYTES, values[index]);
            columns.set(ValueLayout.ADDRESS, index * Abi.BUFFER_BYTES + Abi.BUFFER_VERDICTS, verdicts[index]);
        }
        return columns;
    }

    private static Fill10 fill10(Arena arena, byte[] text, char separator, int[] doors, int[] flags) {
        MemorySegment state = arena.allocate(Native.stateSize(), 8);
        MemorySegment dialect = arena.allocate(Abi.DIALECT_BYTES);
        dialect.set(ValueLayout.JAVA_BYTE, 0, (byte) separator);
        dialect.set(ValueLayout.JAVA_BYTE, 1, (byte) 1);
        dialect.set(ValueLayout.JAVA_BYTE, 2, (byte) 1);
        assertEquals(Abi.OK, Native.init(state, dialect));
        MemorySegment buffers = arena.allocate(Abi.BUFFERS_BYTES, 8);
        buffers.set(ValueLayout.ADDRESS, Abi.BUFFERS_ARENA, arena.allocate(4096));
        buffers.set(ValueLayout.JAVA_LONG, Abi.BUFFERS_ARENA + 8, 4096);
        buffers.set(ValueLayout.ADDRESS, Abi.BUFFERS_CELLS, arena.allocate(CAPACITY * Abi.SPAN_BYTES * 4, 4));
        buffers.set(ValueLayout.JAVA_LONG, Abi.BUFFERS_CELLS + 8, CAPACITY * 4L);
        MemorySegment[] values = new MemorySegment[doors.length];
        MemorySegment[] verdicts = new MemorySegment[doors.length];
        MemorySegment columns = columns10(arena, doors.length, values, verdicts);
        MemorySegment filled = arena.allocate(Abi.FILLED_BYTES, 8);
        assertEquals(
                Abi.OK,
                Native.fill(
                        state,
                        bytes(arena, text),
                        text.length,
                        true,
                        specs10(arena, doors, flags),
                        columns,
                        doors.length,
                        CAPACITY,
                        buffers,
                        filled));
        return new Fill10(filled, values, verdicts);
    }

    /** A policy is the one thing 1.0 cannot be told: its fill reads the low 8 flag bits, so every format is Western there, and a read of a Western format says the same of every cell. */
    @Test
    void theDelimited10ExportsReadWhatAReadDoes() {
        byte[] input = "1;a b;2.5\n12x;c;x\n1,23,456;d;4\n".getBytes(StandardCharsets.UTF_8);
        Dialect dialect = new Dialect(';', true, false, true).withNotes(false);
        try (Arena arena = Arena.ofConfined();
                DelimitedReader reader =
                        DelimitedReader.of(input, dialect, List.of(Column.i32(0), Column.text(1), Column.f64(2)))) {
            Fill10 fill = fill10(
                    arena, input, ';', new int[] {Door.I32.code(), Door.TEXT.code(), Door.F64.code()}, new int[] {
                        95, 95, 95
                    });
            assertEquals(3, fill.filled().get(ValueLayout.JAVA_LONG, Abi.FILLED_ROWS));
            Batch batch = reader.read();
            assertEquals(3, batch.rows());
            assertTrue(batch.isOk(0, 0));
            assertEquals(CastFailure.MALFORMED, batch.verdict(0, 2).reason());
            for (int column = 0; column < 3; column++) {
                assertEquals(
                        -1,
                        fill.verdicts()[column]
                                .asSlice(0, 3 * Abi.VERDICT_BYTES)
                                .mismatch(batch.verdicts(column)),
                        "column " + column);
            }
            assertEquals(-1, fill.values()[0].asSlice(0, 12).mismatch(batch.values(0)));
            assertEquals(-1, fill.values()[2].asSlice(0, 24).mismatch(batch.values(2)));
            for (int row = 0; row < 3; row++) {
                int offset = fill.values()[1].get(ValueLayout.JAVA_INT, row * 8L);
                int length = fill.values()[1].get(ValueLayout.JAVA_INT, row * 8L + 4);
                assertEquals(new String(input, offset, length, StandardCharsets.UTF_8), batch.string(1, row));
            }
        }
    }

    /** Every word, the retired one and Indian's included, reads Western through 1.0: 1,234,567 taken and 1,23,456 refused. */
    @ParameterizedTest
    @ValueSource(ints = {351, 607, 863})
    void aDelimited10FillReadsEveryWordWestern(int flags) {
        byte[] input = "\"1,234,567\"\n\"1,23,456\"\n".getBytes(StandardCharsets.UTF_8);
        try (Arena arena = Arena.ofConfined()) {
            Fill10 fill = fill10(arena, input, ',', new int[] {Door.I32.code()}, new int[] {flags});
            assertEquals(2, fill.filled().get(ValueLayout.JAVA_LONG, Abi.FILLED_ROWS));
            MemorySegment verdicts = fill.verdicts()[0];
            assertEquals(0, verdicts.get(ValueLayout.JAVA_INT, 8));
            assertEquals(1234567, fill.values()[0].get(ValueLayout.JAVA_INT, 0));
            assertEquals(CastFailure.MALFORMED.code(), verdicts.get(ValueLayout.JAVA_INT, Abi.VERDICT_BYTES + 8));
        }
    }

    @Test
    void theWorkbook10ExportsReadWhatAReadDoes() {
        try (Workbook book = Workbook.open(
                        CorpusTest.CORPUS_DIRECTORY.resolve("workbook").resolve("basic.xlsx"));
                Arena arena = Arena.ofConfined()) {
            SheetInfo info = book.sheets().get(0);
            MemorySegment state = arena.allocate(book.state().byteSize(), 8);
            state.copyFrom(book.state());
            MemorySegment filled = arena.allocate(Abi.FILLED_BYTES, 8);
            assertEquals(
                    Abi.OK,
                    Native.sheet(state, book.container(), bytes(arena, info.part()), info.index(), true, true, filled));
            Workbook.Scratch scratch = new Workbook.Scratch(arena, 1 << 20, 1 << 20, 4096, 64);
            try {
                MemorySegment buffers = scratch.buffers(book);
                assertEquals(Abi.OK, Native.book(Abi.Book.HEADER, state, book.container(), buffers, filled));
                int[] doors = {Door.TEXT.code(), Door.TEXT.code()};
                MemorySegment[] values = new MemorySegment[2];
                MemorySegment[] verdicts = new MemorySegment[2];
                MemorySegment columns = columns10(arena, 2, values, verdicts);
                assertEquals(
                        Abi.OK,
                        Native.bookFill(
                                state,
                                book.container(),
                                specs10(arena, doors, new int[] {0, 0}),
                                columns,
                                2,
                                16,
                                buffers,
                                filled));
                try (Sheet sheet = book.sheet(
                        0,
                        new SheetOptions(true, true, 16).withNotes(false),
                        List.of(Column.text(0), Column.text(1)))) {
                    Batch batch = sheet.read();
                    long rows = filled.get(ValueLayout.JAVA_LONG, Abi.FILLED_ROWS);
                    assertTrue(rows > 0);
                    assertEquals(batch.rows(), rows);
                    for (int column = 0; column < 2; column++) {
                        assertEquals(
                                -1,
                                verdicts[column]
                                        .asSlice(0, rows * Abi.VERDICT_BYTES)
                                        .mismatch(batch.verdicts(column)),
                                "column " + column);
                        for (int row = 0; row < rows; row++) {
                            MemorySegment text = batch.text(column, row);
                            if (text == null) {
                                continue;
                            }
                            int offset = values[column].get(ValueLayout.JAVA_INT, row * 8L);
                            int span = values[column].get(ValueLayout.JAVA_INT, row * 8L + 4);
                            MemorySegment said = span < 0
                                    ? scratch.arena.segment.asSlice(offset, span & Abi.SPAN_LENGTH)
                                    : book.strings().asSlice(offset, span);
                            assertEquals(-1, said.mismatch(text), "column " + column + ", row " + row);
                        }
                    }
                }
            } finally {
                scratch.close();
            }
        }
    }

    /** What the header says of each shape's size, held against what this binding lays out, by the header's name for it. */
    @Test
    void everyShapeIsTheSizeTheHeaderSays() throws Exception {
        Path path = CorpusTest.CORPUS_DIRECTORY.resolveSibling("include").resolve("hypertabular.h");
        Assumptions.assumeTrue(
                Files.exists(path),
                "include/ isn't in this tree: the Alpine run copies java/ and corpus/ only, and every other leg"
                        + " holds the sizes to the header.");
        String header = Files.readString(path);
        Object[][] sizes = {
            {"RawDelimitedOptions", Abi.DELIMITED_OPTIONS_BYTES},
            {"RawDialect", Abi.DIALECT_BYTES},
            {"RawTranscoded", Abi.TRANSCODED_BYTES},
            {"Span", Abi.SPAN_BYTES},
            {"Filled", Abi.FILLED_BYTES},
            {"ColumnSpec", Abi.SPEC_BYTES},
            {"CellNote", Abi.NOTE_BYTES},
            {"RawProblem", Abi.PROBLEM_BYTES},
            {"Slot", Abi.SLOT_BYTES},
            {"Opened", Abi.OPENED_BYTES},
            {"RawNumFormat", NativeValues.FORMAT_BYTES},
            {"RawDateOptions", NativeValues.DATE_OPTIONS_BYTES},
            {"RawSheetOptions", Abi.SHEET_OPTIONS_BYTES},
            {"RawRange", Abi.RANGE_BYTES},
            {"RawSuggestion", Abi.SUGGESTION_BYTES},
            {"CellVerdict", Abi.VERDICT_BYTES},
            {"RawConstraints", NativeValues.CONSTRAINTS_BYTES},
            {"RawCheck", Abi.CHECK_BYTES},
        };
        for (Object[] size : sizes) {
            Matcher said = Pattern.compile("#define HyperTabular" + size[0] + "_SIZE (\\d+)")
                    .matcher(header);
            assertTrue(said.find(), String.valueOf(size[0]));
            assertEquals(Long.parseLong(said.group(1)), size[1], String.valueOf(size[0]));
        }
        // Those that hold pointers are a sizeof in the header: the core's asserts say what they come to.
        long word = ValueLayout.ADDRESS.byteSize();
        assertEquals(120 + 7 * word, Abi.SPEC_WITH_BYTES);
        assertEquals(5 * word, Abi.BUFFER_WITH_BYTES);
        assertEquals(18 * word, Abi.BUFFERS_WITH_BYTES);
        assertEquals(120, Abi.SPEC_WITH_MONTHS);
        assertEquals(120 + 2 * word, Abi.SPEC_WITH_TOKENS);
        assertEquals(120 + 4 * word, Abi.SPEC_WITH_CONSTRAINTS);
        assertEquals(120 + 5 * word, Abi.SPEC_WITH_ENUM);
        assertEquals(3 * word, Abi.BUFFER_WITH_NOTES);
        assertEquals(4 * word, Abi.BUFFER_WITH_CHECKS);
        assertEquals(15 * word, Abi.BUFFERS_WITH_TOKENS);
        assertEquals(17 * word, Abi.BUFFERS_WITH_ROW_WIDTHS);
        assertEquals(32, Abi.SHEET_OPTIONS_LAST_COLUMN);
        assertEquals(32, Abi.PROBLEM_KEYWORD);
    }

    /**
     * Mono's interpreter passes no more than twelve integer arguments, and the ABI is the same for every
     * binding. Every door of this path's {@link Native} is counted, and the biggest is what it should
     * be, so that the count can't pass on nothing.
     */
    @Test
    void noExportTakesMoreThanTwelveArguments() {
        int most = 0;
        for (Method method : Native.class.getDeclaredMethods()) {
            if (Modifier.isStatic(method.getModifiers())) {
                assertTrue(method.getParameterCount() <= 12, method.getName());
                most = Math.max(most, method.getParameterCount());
            }
        }
        assertEquals(10, most, Arrays.toString(Native.class.getDeclaredMethods()));
    }
}
