package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.Success;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.math.BigDecimal;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.LocalTime;
import java.util.ArrayList;
import java.util.List;
import java.util.UUID;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Replays the shared conformance corpus ({@code corpus/delimited.json} at the repository
 * root) — the same file the Rust and C# bindings replay — through this binding: from a byte
 * array, from native memory read in place and a window at a time, from a stream read
 * through buffers too small for a record, and from a file, in batches of one row, of two
 * and of many. How the input is cut up is the binding's business and must not change the
 * answer.
 */
final class CorpusTest {
    private static final int[] BATCH_ROWS = {1, 2, 1024};
    private static final int[] BUFFER_BYTES = {1, 5, 64, DelimitedReader.DEFAULT_BUFFER_BYTES};

    /** The {@code corpus} directory at the repository root. */
    static final Path CORPUS_DIRECTORY = corpusDirectory();

    private static final JsonArray CORPUS = load("delimited.json");

    private static Path corpusDirectory() {
        for (Path dir = Path.of("").toAbsolutePath(); dir != null; dir = dir.getParent()) {
            if (Files.isRegularFile(dir.resolve("corpus").resolve("delimited.json"))) {
                return dir.resolve("corpus");
            }
        }
        throw new IllegalStateException("corpus/ not found above " + Path.of("").toAbsolutePath());
    }

    /** One of the corpus's JSON files, parsed. */
    static JsonArray load(String file) {
        try {
            return JsonParser.parseString(Files.readString(CORPUS_DIRECTORY.resolve(file)))
                    .getAsJsonArray();
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    /** How one replay gets its reader: the case's bytes, cut up one particular way. */
    @FunctionalInterface
    private interface Opening {
        DelimitedReader open(Dialect dialect, List<Column> plan);
    }

    private static NumFormat formatOf(JsonObject entry) {
        if (!entry.has("format")) {
            return NumFormat.INVARIANT;
        }
        JsonObject format = entry.getAsJsonObject("format");
        return new NumFormat(
                format.get("decimal_sep").getAsString().charAt(0),
                format.get("group_sep").getAsString().charAt(0),
                format.get("flags").getAsInt(),
                format.has("currency") ? format.get("currency").getAsString() : "");
    }

    // The corpus numbers what a door declares as HyperCast numbers it: from one, in order.
    private static <E extends Enum<E>> E declared(E[] constants, JsonObject entry, String key) {
        return constants[entry.get(key).getAsInt() - 1];
    }

    static Column columnOf(JsonObject entry) {
        int ordinal = entry.get("ordinal").getAsInt();
        NumFormat format = formatOf(entry);
        String door = entry.get("door").getAsString();
        return switch (door) {
            case "bool" -> Column.bool(ordinal);
            case "i8" -> Column.i8(ordinal, format);
            case "i16" -> Column.i16(ordinal, format);
            case "i32" -> Column.i32(ordinal, format);
            case "i64" -> Column.i64(ordinal, format);
            case "u8" -> Column.u8(ordinal, format);
            case "u16" -> Column.u16(ordinal, format);
            case "u32" -> Column.u32(ordinal, format);
            case "u64" -> Column.u64(ordinal, format);
            case "f32" -> Column.f32(ordinal, format);
            case "f64" -> Column.f64(ordinal, format);
            case "decimal" -> Column.decimal(ordinal, format);
            case "uuid" -> Column.uuid(ordinal);
            case "timestamp" -> Column.timestamp(ordinal);
            case "unix" -> Column.unix(ordinal, declared(UnixPrecision.values(), entry, "precision"));
            case "excel_serial" -> Column.excelSerial(ordinal, declared(ExcelEpoch.values(), entry, "epoch"));
            case "date" -> Column.date(ordinal);
            case "date_ordered" -> Column.date(ordinal, declared(DateOrder.values(), entry, "order"));
            case "datetime" -> Column.dateTime(ordinal, declared(DateOrder.values(), entry, "order"));
            case "time" -> Column.time(ordinal);
            case "duration" -> Column.duration(ordinal);
            case "text" -> Column.text(ordinal);
            default -> throw new IllegalStateException("unknown door " + door);
        };
    }

    /** The Java type a door's value is presented as: what {@link Batch#get} is asked for. */
    private static Class<?> typeOf(Door door) {
        return switch (door) {
            case BOOL -> Boolean.class;
            case I8 -> Byte.class;
            case I16 -> Short.class;
            case I32, U8, U16 -> Integer.class;
            case I64, U32, U64 -> Long.class;
            case F32 -> Float.class;
            case F64 -> Double.class;
            case DECIMAL -> BigDecimal.class;
            case UUID -> UUID.class;
            case TIMESTAMP, UNIX, EXCEL_SERIAL -> Instant.class;
            case DATE, DATE_ORDERED -> LocalDate.class;
            case DATETIME -> LocalDateTime.class;
            case TIME -> LocalTime.class;
            case DURATION -> Duration.class;
            case TEXT -> String.class;
        };
    }

    private static LocalDate dayOf(JsonObject cell) {
        return LocalDate.of(
                cell.get("year").getAsInt(),
                cell.get("month").getAsInt(),
                cell.get("day").getAsInt());
    }

    /** What the corpus says a cell that cast is worth, as the Java type its door presents. */
    private static Object valueOf(JsonObject cell, Door door) {
        return switch (door) {
            case BOOL -> cell.get("value").getAsBoolean();
            case I8 -> cell.get("value").getAsByte();
            case I16 -> cell.get("value").getAsShort();
            case I32, U8, U16 -> cell.get("value").getAsInt();
            case I64, U32 -> cell.get("value").getAsLong();
            // u64 arrives as the long with the same bits.
            case U64 -> cell.get("value").getAsBigInteger().longValue();
            case F32 -> (float) cell.get("value").getAsDouble();
            case F64 -> cell.get("value").getAsDouble();
            // The raw triple is the contract; BigDecimal is built from exactly that.
            case DECIMAL -> {
                BigInteger magnitude = new BigInteger(cell.get("magnitude").getAsString());
                yield new BigDecimal(
                        cell.get("negative").getAsBoolean() ? magnitude.negate() : magnitude,
                        cell.get("scale").getAsInt());
            }
            case UUID -> {
                String hex = cell.get("value").getAsString();
                yield new UUID(
                        Long.parseUnsignedLong(hex.substring(0, 16), 16),
                        Long.parseUnsignedLong(hex.substring(16), 16));
            }
            case TIMESTAMP, UNIX, EXCEL_SERIAL ->
                Instant.ofEpochSecond(
                        cell.get("seconds").getAsLong(), cell.get("nanos").getAsInt());
            case DATE, DATE_ORDERED -> dayOf(cell);
            case DATETIME ->
                LocalDateTime.of(
                        dayOf(cell),
                        LocalTime.ofNanoOfDay(cell.get("nanos_of_day").getAsLong()));
            case TIME -> LocalTime.ofNanoOfDay(cell.get("nanos").getAsLong());
            case DURATION ->
                Duration.ofSeconds(
                        cell.get("seconds").getAsLong(), cell.get("nanos").getAsInt());
            case TEXT -> cell.get("text").getAsString();
        };
    }

    private static String utf8(MemorySegment bytes) {
        return new String(bytes.toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8);
    }

    /** Holds one cell of the batch in hand to what the corpus says of it. */
    static void assertCell(String label, Batch batch, int column, int row, JsonObject expected) {
        CellVerdict verdict = batch.verdict(column, row);
        Door door = batch.columns().get(column).door();
        String expect = expected.get("expect").getAsString();
        // The verdict array, read in place, says what the per-cell verdict says.
        MemorySegment entry = batch.verdicts(column).asSlice(row * CellVerdict.LAYOUT.byteSize());
        assertEquals(verdict.isOk() ? 0 : verdict.reason().code(), entry.get(ValueLayout.JAVA_INT, 8), label);
        assertEquals(verdict.offset(), entry.get(ValueLayout.JAVA_INT, 0), label);
        assertEquals(verdict.length(), entry.get(ValueLayout.JAVA_INT, 4), label);
        assertEquals(verdict.isOk(), batch.isOk(column, row), label);

        if (!expect.equals("ok")) {
            assertFalse(verdict.isOk(), label);
            assertEquals(
                    switch (expect) {
                        case "empty" -> CastFailure.EMPTY;
                        case "malformed" -> CastFailure.MALFORMED;
                        case "out_of_range" -> CastFailure.OUT_OF_RANGE;
                        default -> throw new IllegalStateException("unknown expectation " + expect);
                    },
                    verdict.reason(),
                    label);
            if (expected.has("fault")) {
                JsonArray span = expected.getAsJsonArray("fault");
                assertEquals(span.get(0).getAsInt(), verdict.offset(), label);
                assertEquals(span.get(1).getAsInt(), verdict.length(), label);
                // The cell's own text is still to hand, for the diagnostic a fault deserves.
                String raw = expected.get("raw").getAsString();
                assertEquals(raw, batch.rawString(column, row), label);
                assertEquals(raw, utf8(batch.raw(column, row)), label);
            } else {
                // An empty cell: the corpus states no span and no text, and the text there
                // was — none, or only what the door trims — is still there to be asked for.
                assertEquals(batch.rawString(column, row), utf8(batch.raw(column, row)), label);
            }
            if (door == Door.TEXT) {
                assertNull(batch.text(column, row), label);
                assertNull(batch.string(column, row), label);
            }
            // The typed getter's own fault, as HyperCast's union.
            switch (batch.get(column, row, typeOf(door))) {
                case Success<?> success -> fail(label + ": expected a fault, got " + success);
                case Fault<?> fault -> assertEquals(verdict.toFault(), fault, label);
            }
            return;
        }

        assertTrue(verdict.isOk(), label);
        assertNull(verdict.reason(), label);
        Object value = valueOf(expected, door);
        if (door == Door.TEXT) {
            MemorySegment text = batch.text(column, row);
            assertNotNull(text, label);
            assertEquals(value, utf8(text), label);
            assertEquals(value, batch.string(column, row), label);
            // A text cell's raw text is its text: unescaped, as the core cast it.
            assertEquals(value, batch.rawString(column, row), label);
            assertEquals(new Success<>(value), batch.get(column, row, String.class), label);
            return;
        }
        switch (batch.get(column, row, typeOf(door))) {
            case Success<?> success -> assertEquals(value, success.value(), label);
            case Fault<?> fault -> fail(label + ": expected " + value + ", got " + fault);
        }
        if (door == Door.DECIMAL) {
            // And the value the corpus spells out is the same number.
            BigDecimal spelled = new BigDecimal(expected.get("value").getAsString());
            assertEquals(0, spelled.compareTo((BigDecimal) value), label);
        }
    }

    private static void assertFailure(String label, TabularException actual, JsonObject vector) {
        if (!vector.has("failure")) {
            assertNull(actual, label);
            return;
        }
        assertNotNull(actual, label);
        JsonObject expected = vector.getAsJsonObject("failure");
        String kind = expected.get("kind").getAsString();
        assertEquals(
                kind.equals("column_count") ? TabularFailure.COLUMN_COUNT : TabularFailure.UNCLOSED_QUOTE,
                actual.failure(),
                label);
        assertEquals(expected.get("record").getAsLong(), actual.record(), label);
        assertEquals(expected.get("line").getAsInt(), actual.line(), label);
        assertEquals(expected.get("byte").getAsLong(), actual.byteOffset(), label);
        if (kind.equals("column_count")) {
            assertEquals(expected.get("expected").getAsInt(), actual.expected(), label);
            assertEquals(expected.get("found").getAsInt(), actual.found(), label);
        }
    }

    private static void replay(String label, JsonObject vector, Opening opening) {
        JsonObject settings = vector.getAsJsonObject("dialect");
        Dialect dialect = new Dialect(
                settings.get("separator").getAsString().charAt(0),
                settings.get("quoting").getAsBoolean(),
                settings.get("has_header").getAsBoolean(),
                settings.get("skip_blank_lines").getAsBoolean());
        List<Column> plan = new ArrayList<>();
        for (JsonElement entry : vector.getAsJsonArray("plan")) {
            plan.add(columnOf(entry.getAsJsonObject()));
        }
        JsonArray rows = vector.getAsJsonArray("rows");

        try (DelimitedReader reader = opening.open(dialect, plan)) {
            JsonElement header = vector.get("header");
            if (header.isJsonNull()) {
                assertNull(reader.header(), label);
            } else {
                List<String> names = new ArrayList<>();
                header.getAsJsonArray().forEach(name -> names.add(name.getAsString()));
                assertEquals(names, reader.header(), label);
            }

            int seen = 0;
            TabularException failure = null;
            try {
                for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                    for (int row = 0; row < batch.rows(); row++, seen++) {
                        assertTrue(seen < rows.size(), label + ": more rows than the corpus lists");
                        JsonArray cells = rows.get(seen).getAsJsonArray();
                        for (int column = 0; column < plan.size(); column++) {
                            assertCell(
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
                // A structure failure is final: the same one, again.
                assertSame(e, assertThrows(TabularException.class, reader::read), label);
            }
            assertEquals(rows.size(), seen, label);
            assertFailure(label, failure, vector);
        }
    }

    private interface Mode {
        void replay(String name, JsonObject vector, byte[] input, int batchRows);
    }

    private static void everyCase(Mode mode) {
        assertTrue(CORPUS.size() >= 30, "the corpus has " + CORPUS.size() + " cases");
        for (JsonElement element : CORPUS) {
            JsonObject vector = element.getAsJsonObject();
            String name = vector.get("name").getAsString();
            byte[] input = vector.get("input").getAsString().getBytes(StandardCharsets.UTF_8);
            for (int batchRows : BATCH_ROWS) {
                mode.replay(name, vector, input, batchRows);
            }
        }
    }

    @Test
    void fromAByteArray() {
        everyCase((name, vector, input, batchRows) -> replay(
                name + " (byte[], " + batchRows + " rows a batch)",
                vector,
                (dialect, plan) -> DelimitedReader.of(input, dialect, plan, batchRows)));
    }

    @Test
    void fromNativeMemoryInPlace() {
        everyCase((name, vector, input, batchRows) -> {
            try (Arena arena = Arena.ofConfined()) {
                MemorySegment text = arena.allocateFrom(ValueLayout.JAVA_BYTE, input);
                replay(
                        name + " (native segment, " + batchRows + " rows a batch)",
                        vector,
                        (dialect, plan) -> DelimitedReader.of(text, dialect, plan, batchRows));
                // And a heap segment, which is copied in as a byte array is.
                replay(
                        name + " (heap segment, " + batchRows + " rows a batch)",
                        vector,
                        (dialect, plan) -> DelimitedReader.of(MemorySegment.ofArray(input), dialect, plan, batchRows));
            }
        });
    }

    /** The length of the longest record in {@code input}, its line break included. */
    private static int longestRecord(byte[] input, boolean quoting) {
        int longest = 0;
        int begun = 0;
        boolean quoted = false;
        for (int at = 0; at < input.length; at++) {
            if (input[at] == '"' && quoting) {
                quoted = !quoted;
            } else if (input[at] == '\n' && !quoted) {
                longest = Math.max(longest, at + 1 - begun);
                begun = at + 1;
            }
        }
        return Math.max(longest, input.length - begun);
    }

    @Test
    void fromMemoryAWindowAtATime() {
        // What memory past the ABI's 2 GiB a call is read as: a window that ends wherever
        // it ends, the next one starting where the core stopped. A record has to fit a
        // window, so each case's windows are no smaller than its longest record (and the
        // byte after it, which says a line break is over).
        everyCase((name, vector, input, batchRows) -> {
            boolean quoting = vector.getAsJsonObject("dialect").get("quoting").getAsBoolean();
            int smallest = longestRecord(input, quoting) + 1;
            try (Arena arena = Arena.ofConfined()) {
                MemorySegment text = arena.allocateFrom(ValueLayout.JAVA_BYTE, input);
                for (int windowBytes : new int[] {Math.max(smallest, 4), smallest + 7}) {
                    replay(
                            name + " (windows of " + windowBytes + " bytes, " + batchRows + " rows a batch)",
                            vector,
                            (dialect, plan) -> DelimitedReader.windowed(text, dialect, plan, batchRows, windowBytes));
                }
            }
        });
    }

    @Test
    void fromAStreamThroughBuffersOfEverySize() {
        everyCase((name, vector, input, batchRows) -> {
            for (int bufferBytes : BUFFER_BYTES) {
                replay(
                        name + " (stream through " + bufferBytes + " bytes, " + batchRows + " rows a batch)",
                        vector,
                        (dialect, plan) -> DelimitedReader.of(
                                new ByteArrayInputStream(input), dialect, plan, batchRows, bufferBytes));
            }
        });
    }

    @Test
    void fromAFile(@TempDir Path directory) throws IOException {
        Path file = directory.resolve("case.csv");
        for (JsonElement element : CORPUS) {
            JsonObject vector = element.getAsJsonObject();
            String name = vector.get("name").getAsString();
            byte[] input = vector.get("input").getAsString().getBytes(StandardCharsets.UTF_8);
            Files.write(file, input);
            assertArrayEquals(input, Files.readAllBytes(file));
            for (int batchRows : BATCH_ROWS) {
                for (int bufferBytes : new int[] {3, DelimitedReader.DEFAULT_BUFFER_BYTES}) {
                    replay(
                            name + " (file through " + bufferBytes + " bytes, " + batchRows + " rows a batch)",
                            vector,
                            (dialect, plan) -> DelimitedReader.open(file, dialect, plan, batchRows, bufferBytes));
                }
            }
        }
    }
}
