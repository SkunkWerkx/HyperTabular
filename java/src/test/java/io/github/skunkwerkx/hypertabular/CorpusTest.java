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
import io.github.skunkwerkx.hypercast.Checked;
import io.github.skunkwerkx.hypercast.CheckedVerdict;
import io.github.skunkwerkx.hypercast.Constraints;
import io.github.skunkwerkx.hypercast.DateOptions;
import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.FaultDetail;
import io.github.skunkwerkx.hypercast.Grouping;
import io.github.skunkwerkx.hypercast.MonthNames;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.Rounding;
import io.github.skunkwerkx.hypercast.Success;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import io.github.skunkwerkx.hypercast.Violation;
import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.math.BigDecimal;
import java.math.BigInteger;
import java.nio.CharBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.LocalTime;
import java.util.ArrayList;
import java.util.HexFormat;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;
import java.util.UUID;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Replays the shared conformance corpus ({@code corpus/delimited.json} at the repository
 * root) — the same file the Rust and C# bindings replay — through this binding: from a byte
 * array, from native memory read in place and a window at a time, from a stream read
 * through buffers too small for a record, and from a file, in batches of one row, of two
 * and of many, with notes on and off. How the input is cut up is the binding's business and
 * must not change the answer.
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

    /** A corpus entry's format, or {@code null} where it has none. */
    private static NumFormat declaredFormat(JsonObject entry) {
        if (!entry.has("format")) {
            return null;
        }
        JsonObject format = entry.getAsJsonObject("format");
        // A corpus flags value mixes leniences and grouping, which HyperCast's type keeps apart.
        int flags = format.get("flags").getAsInt();
        NumFormat declared = new NumFormat(
                format.get("decimal_sep").getAsString(),
                format.get("group_sep").getAsString(),
                flags & 0xFF,
                Grouping.of(flags & ~0xFF));
        return format.has("currency")
                ? declared.withCurrencySymbol(format.get("currency").getAsString())
                : declared;
    }

    private static NumFormat formatOf(JsonObject entry) {
        NumFormat declared = declaredFormat(entry);
        return declared != null ? declared : NumFormat.INVARIANT;
    }

    private static String[] stringsOf(JsonArray list) {
        String[] strings = new String[list.size()];
        for (int index = 0; index < strings.length; index++) {
            strings[index] = list.get(index).getAsString();
        }
        return strings;
    }

    private static Column withDateOptions(Column column, JsonObject options) {
        int flags = 0;
        if (options.has("flags")) {
            for (JsonElement flag : options.getAsJsonArray("flags")) {
                flags |= switch (flag.getAsString()) {
                    case "compact_date" -> DateOptions.COMPACT_DATE;
                    case "space_separator" -> DateOptions.SPACE_SEPARATOR;
                    case "hour_offset" -> DateOptions.HOUR_OFFSET;
                    default -> throw new IllegalStateException("unknown date flag " + flag);
                };
            }
        }
        DateOptions declared = new DateOptions(
                flags, options.has("pivot") ? options.get("pivot").getAsInt() : 0);
        if (!options.has("months")) {
            return column.withDateOptions(declared);
        }
        MonthNames.Builder months = MonthNames.builder();
        for (JsonElement entry : options.getAsJsonArray("months")) {
            JsonArray pair = entry.getAsJsonArray();
            months.add(pair.get(0).getAsInt(), pair.get(1).getAsString());
        }
        return column.withDateOptions(declared, months.build());
    }

    // The corpus numbers what a door declares as HyperCast numbers it: from one, in order.
    private static <E extends Enum<E>> E declared(E[] constants, JsonObject entry, String key) {
        return constants[entry.get(key).getAsInt() - 1];
    }

    static Column columnOf(JsonObject entry) {
        Column column =
                entry.has("constraints") ? checkedOf(entry, entry.getAsJsonObject("constraints")) : doorOf(entry);
        if (entry.has("tokens")) {
            column = column.withTokens(stringsOf(entry.getAsJsonArray("tokens")));
        }
        if (entry.has("options")) {
            column = withDateOptions(column, entry.getAsJsonObject("options"));
        }
        return column;
    }

    private static Column doorOf(JsonObject entry) {
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

    /** A plan entry that declares constraints, through the factory of its door. */
    private static Column checkedOf(JsonObject entry, JsonObject declared) {
        int ordinal = entry.get("ordinal").getAsInt();
        NumFormat format = formatOf(entry);
        String door = entry.get("door").getAsString();
        return switch (door) {
            case "i8" -> Column.i8(ordinal, format, numeric(Constraints.i8(), declared));
            case "i16" -> Column.i16(ordinal, format, numeric(Constraints.i16(), declared));
            case "i32" -> Column.i32(ordinal, format, numeric(Constraints.i32(), declared));
            case "i64" -> Column.i64(ordinal, format, numeric(Constraints.i64(), declared));
            case "u8" -> Column.u8(ordinal, format, numeric(Constraints.u8(), declared));
            case "u16" -> Column.u16(ordinal, format, numeric(Constraints.u16(), declared));
            case "u32" -> Column.u32(ordinal, format, numeric(Constraints.u32(), declared));
            case "u64" -> Column.u64(ordinal, format, numeric(Constraints.u64(), declared));
            case "f32" -> Column.f32(ordinal, format, numeric(Constraints.f32(), declared));
            case "f64" -> Column.f64(ordinal, format, numeric(Constraints.f64(), declared));
            case "decimal" -> Column.decimal(ordinal, format, numeric(Constraints.decimal(), declared));
            case "text" -> {
                Constraints.Builder<String> text = Constraints.text();
                if (declared.has("minLength")) {
                    text.minLength(declared.get("minLength").getAsInt());
                }
                if (declared.has("maxLength")) {
                    text.maxLength(declared.get("maxLength").getAsInt());
                }
                if (declared.has("trim")) {
                    text.trim(declared.get("trim").getAsBoolean());
                }
                if (declared.has("caseSensitive")) {
                    text.caseSensitive(declared.get("caseSensitive").getAsBoolean());
                }
                if (declared.has("enum")) {
                    text.enumOf(stringsOf(declared.getAsJsonArray("enum")));
                }
                yield Column.text(ordinal, text.build());
            }
            default -> throw new IllegalStateException("no checked door " + door);
        };
    }

    /** A numeric declaration in the corpus's shape: every bound, step and entry a decimal's text. */
    private static <T> Constraints<T> numeric(Constraints.Builder<T> builder, JsonObject declared) {
        if (declared.has("minimum")) {
            builder.minimum(declared.get("minimum").getAsString());
        }
        if (declared.has("exclusiveMinimum")) {
            builder.exclusiveMinimum(declared.get("exclusiveMinimum").getAsString());
        }
        if (declared.has("maximum")) {
            builder.maximum(declared.get("maximum").getAsString());
        }
        if (declared.has("exclusiveMaximum")) {
            builder.exclusiveMaximum(declared.get("exclusiveMaximum").getAsString());
        }
        if (declared.has("multipleOf")) {
            builder.multipleOf(declared.get("multipleOf").getAsString());
        }
        if (declared.has("rounding")) {
            builder.rounding(
                    Rounding.valueOf(declared.get("rounding").getAsString().toUpperCase(Locale.ROOT)));
        }
        if (declared.has("enum")) {
            builder.enumOf(stringsOf(declared.getAsJsonArray("enum")));
        }
        return builder.build();
    }

    /** The Java type a door's value is presented as: what {@link Batch#get} is asked for. */
    static Class<?> typeOf(Door door) {
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

    static String utf8(MemorySegment bytes) {
        return new String(bytes.toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8);
    }

    /** Holds one cell of the batch in hand to what the corpus says of it, with notes kept. */
    static void assertCell(String label, Batch batch, int column, int row, JsonObject expected) {
        assertCell(label, batch, column, row, expected, true);
    }

    /** Holds one cell of the batch in hand to what the corpus says of it. */
    static void assertCell(String label, Batch batch, int column, int row, JsonObject expected, boolean notes) {
        CellVerdict verdict = batch.verdict(column, row);
        Door door = batch.columns().get(column).door();
        String expect = expected.get("expect").getAsString();
        // The note beside the verdict: with notes off there is none, and it says so.
        FaultDetail detail = batch.detail(column, row);
        assertEquals(
                notes && expected.has("detail") ? expected.get("detail").getAsString() : null,
                detail == null ? null : detail.name(),
                label);
        assertEquals(
                notes && expected.has("origin") ? expected.get("origin").getAsString() : "cell",
                batch.origin(column, row).name().toLowerCase(Locale.ROOT),
                label);
        // The verdict array, read in place, says what the per-cell verdict says.
        MemorySegment entry = batch.verdicts(column).asSlice(row * CellVerdict.LAYOUT.byteSize());
        assertEquals(
                verdict.isOk()
                        ? 0
                        : verdict.isViolation() ? 4 : verdict.reason().code(),
                entry.get(ValueLayout.JAVA_INT, 8),
                label);
        assertEquals(verdict.offset(), entry.get(ValueLayout.JAVA_INT, 0), label);
        assertEquals(verdict.length(), entry.get(ValueLayout.JAVA_INT, 4), label);
        assertEquals(verdict.isOk(), batch.isOk(column, row), label);
        assertEquals(expect.equals("violation"), verdict.isViolation(), label);
        if (batch.columns().get(column).isChecked()) {
            assertChecked(label, batch, column, row, expected, verdict);
            return;
        }

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
                if (expected.has("raw_hex")) {
                    assertEquals(
                            expected.get("raw_hex").getAsString(),
                            HexFormat.of().formatHex(batch.raw(column, row).toArray(ValueLayout.JAVA_BYTE)),
                            label);
                } else {
                    String raw = expected.get("raw").getAsString();
                    assertEquals(raw, batch.rawString(column, row), label);
                    assertEquals(raw, utf8(batch.raw(column, row)), label);
                }
            } else {
                // An empty cell: the corpus states no span and no text, and the text there
                // was — none, or only what the door trims — is still there to be asked for.
                assertEquals(batch.rawString(column, row), utf8(batch.raw(column, row)), label);
            }
            if (door == Door.TEXT) {
                assertNull(batch.text(column, row), label);
                assertNull(batch.string(column, row), label);
                // An empty cell has no chars either, and writes none.
                assertNull(batch.chars(column, row), label);
                assertEquals(0, batch.getChars(column, row, new char[0], 0), label);
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
            assertChars(label, batch, column, row, (String) value);
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

    /**
     * Holds a constrained column's cell to what the corpus says of it, through the checked getter:
     * a value with its {@code adjusted} flag and {@code enum} match, a fault, or a violation with its
     * keyword and the value as cast.
     */
    private static void assertChecked(
            String label, Batch batch, int column, int row, JsonObject expected, CellVerdict verdict) {
        Door door = batch.columns().get(column).door();
        String expect = expected.get("expect").getAsString();
        // The plain getters refuse the column: a violation is no fault, and they would read it as passed.
        assertThrows(IllegalStateException.class, () -> batch.get(column, row, typeOf(door)), label);
        if (door == Door.TEXT) {
            assertThrows(IllegalStateException.class, () -> batch.string(column, row), label);
        }
        CheckedVerdict<?> judged = batch.getChecked(column, row, typeOf(door));
        Object value;
        switch (judged) {
            case Checked<?> passed -> {
                assertEquals("ok", expect, label);
                assertEquals(expected.get("adjusted").getAsBoolean(), passed.adjusted(), label);
                assertEquals(
                        expected.has("enum_index") ? expected.get("enum_index").getAsInt() : -1,
                        passed.enumIndex().orElse(-1),
                        label);
                value = passed.value();
            }
            case Violation<?> broken -> {
                assertEquals("violation", expect, label);
                assertEquals(
                        expected.get("keyword").getAsString(), broken.keyword().name(), label);
                JsonArray span = expected.getAsJsonArray("fault");
                assertEquals(span.get(0).getAsInt(), broken.offset(), label);
                assertEquals(span.get(1).getAsInt(), broken.length(), label);
                assertEquals(broken.offset(), verdict.offset(), label);
                assertEquals(broken.length(), verdict.length(), label);
                assertEquals(broken.keyword(), verdict.keyword(), label);
                assertEquals(expected.get("raw").getAsString(), batch.rawString(column, row), label);
                value = broken.value();
            }
            case Fault<?> fault -> {
                assertEquals(verdict.toFault(), fault, label);
                assertEquals(
                        switch (expect) {
                            case "empty" -> CastFailure.EMPTY;
                            case "malformed" -> CastFailure.MALFORMED;
                            case "out_of_range" -> CastFailure.OUT_OF_RANGE;
                            default -> throw new IllegalStateException("unknown expectation " + expect);
                        },
                        fault.reason(),
                        label);
                if (expected.has("fault")) {
                    JsonArray span = expected.getAsJsonArray("fault");
                    assertEquals(span.get(0).getAsInt(), fault.offset(), label);
                    assertEquals(span.get(1).getAsInt(), fault.length(), label);
                    if (expected.has("raw_hex")) {
                        assertEquals(
                                expected.get("raw_hex").getAsString(),
                                HexFormat.of().formatHex(batch.raw(column, row).toArray(ValueLayout.JAVA_BYTE)),
                                label);
                    } else {
                        assertEquals(expected.get("raw").getAsString(), batch.rawString(column, row), label);
                    }
                }
                return;
            }
        }
        switch (door) {
            case F32 ->
                assertEquals(
                        expected.get("bits").getAsString(),
                        String.format("0x%08x", Float.floatToRawIntBits((Float) value)),
                        label);
            case F64 ->
                assertEquals(
                        expected.get("bits").getAsString(),
                        String.format("0x%016x", Double.doubleToRawLongBits((Double) value)),
                        label);
            default -> assertEquals(valueOf(expected, door), value, label);
        }
    }

    /**
     * Holds a text cell's chars — into the batch's own arena, and into the caller's array, at
     * an offset, too small by one, and exactly large enough — to its string.
     */
    static void assertChars(String label, Batch batch, int column, int row, String text) {
        assertEquals(text, batch.chars(column, row).toString(), label);
        char[] into = new char[text.length() + 3];
        assertEquals(text.length(), batch.getChars(column, row, into, 2), label);
        assertEquals(text, new String(into, 2, text.length()), label);
        if (!text.isEmpty()) {
            assertEquals(-1, batch.getChars(column, row, new char[text.length() + 1], 2), label);
        }
        assertEquals(text.length(), batch.getChars(column, row, new char[text.length()], 0), label);
    }

    /** Holds every {@link Row} of a batch, met by {@code for}, to the batch's own accessors. */
    static void assertRows(String label, Batch batch) {
        int index = 0;
        for (Row row : batch) {
            assertEquals(index, row.index(), label);
            assertEquals(batch.line(index), row.line(), label);
            assertEquals(batch.row(index).index(), index, label);
            for (int column = 0; column < batch.columns().size(); column++) {
                Door door = batch.columns().get(column).door();
                assertEquals(batch.verdict(column, index), row.verdict(column), label);
                assertEquals(batch.isOk(column, index), row.isOk(column), label);
                assertEquals(batch.detail(column, index), row.detail(column), label);
                assertEquals(batch.origin(column, index), row.origin(column), label);
                assertEquals(batch.rawString(column, index), utf8(row.raw(column)), label);
                assertEquals(batch.rawString(column, index), row.rawString(column), label);
                if (batch.columns().get(column).isChecked()) {
                    assertEquals(
                            batch.getChecked(column, index, typeOf(door)), row.getChecked(column, typeOf(door)), label);
                    continue;
                }
                assertEquals(batch.get(column, index, typeOf(door)), row.get(column, typeOf(door)), label);
                if (door == Door.TEXT) {
                    MemorySegment text = row.text(column);
                    assertEquals(batch.string(column, index), text == null ? null : utf8(text), label);
                    assertEquals(batch.string(column, index), row.string(column), label);
                    CharBuffer chars = row.chars(column);
                    assertEquals(batch.string(column, index), chars == null ? null : chars.toString(), label);
                }
            }
            index++;
        }
        assertEquals(batch.rows(), index, label);
    }

    /**
     * Holds a reader opened header-first to what a reader with no plan promises — the header's
     * names found first-match and refused strict where two columns share one, by string and by
     * bytes; no plan; no read, and not for good — and binds {@code plan}, which binds once.
     */
    static void bindHeaderFirst(
            String label,
            Header header,
            List<Column> unbound,
            boolean isBound,
            Runnable read,
            java.util.function.Consumer<List<Column>> bind,
            List<Column> plan) {
        assertFalse(isBound, label);
        assertTrue(unbound.isEmpty(), label);
        assertThrows(IllegalStateException.class, read::run, label);
        assertThrows(IllegalStateException.class, read::run, label);
        if (header != null) {
            for (int ordinal = 0; ordinal < header.size(); ordinal++) {
                String name = header.get(ordinal);
                int found = header.findOrdinal(name).getAsInt();
                assertTrue(found <= ordinal, label);
                assertEquals(header.get(found), name, label);
                assertEquals(found, header.indexOf(name), label);
                MemorySegment bytes = header.utf8(ordinal);
                int byBytes = header.findOrdinal(bytes).getAsInt();
                assertTrue(byBytes <= ordinal, label);
                assertEquals(utf8(header.utf8(byBytes)), utf8(bytes), label);
                assertEquals(
                        byBytes,
                        header.findOrdinal(bytes.toArray(ValueLayout.JAVA_BYTE)).getAsInt(),
                        label);
                // The strict door answers only for a name one column has.
                int second = -1;
                for (int later = found + 1; later < header.size() && second < 0; later++) {
                    second = header.get(later).equals(name) ? later : -1;
                }
                if (second < 0) {
                    assertEquals(found, header.ordinal(name), label);
                } else {
                    AmbiguousColumnException two =
                            assertThrows(AmbiguousColumnException.class, () -> header.ordinal(name), label);
                    assertEquals(List.of(name, found, second), List.of(two.name(), two.first(), two.second()), label);
                }
            }
        }
        bind.accept(plan);
        assertThrows(IllegalStateException.class, () -> bind.accept(plan), label);
    }

    /** Holds a failure, whole, to what the corpus says of it: every field for every kind. */
    static void assertFailure(String label, TabularException actual, JsonObject vector) {
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
        assertEquals(expected.has("expected") ? expected.get("expected").getAsInt() : 0, actual.expected(), label);
        assertEquals(expected.has("found") ? expected.get("found").getAsInt() : 0, actual.found(), label);
    }

    /** A JSON object with its keys in order, for comparing what a read listed to what the corpus says. */
    private static String canonical(JsonObject entry) {
        Map<String, JsonElement> sorted = new TreeMap<>();
        entry.entrySet().forEach(pair -> sorted.put(pair.getKey(), pair.getValue()));
        return sorted.toString();
    }

    /** The problems a batch lists, in the corpus's shape, its rows counted across the read. */
    static void appendProblems(String label, List<String> problems, Batch batch, int base) {
        for (Problem problem : batch.faults()) {
            assertEquals(batch.line(problem.row()), problem.line(), label);
            JsonObject entry = new JsonObject();
            entry.addProperty("row", base + problem.row());
            entry.addProperty("line", problem.line());
            if (problem.planColumn().isPresent()) {
                entry.addProperty("plan_column", problem.planColumn().getAsInt());
                entry.addProperty("source_column", problem.sourceColumn().getAsInt());
                // A violation names its keyword and has no reason; anything else has a reason and no keyword.
                assertEquals(problem.reason() == null, problem.keyword() != null, label);
                entry.addProperty(
                        "reason",
                        problem.reason() == null
                                ? "violation"
                                : problem.reason().name().toLowerCase(Locale.ROOT));
                if (problem.keyword() != null) {
                    entry.addProperty("keyword", problem.keyword().name());
                }
                if (problem.reason() != CastFailure.EMPTY) {
                    JsonArray span = new JsonArray();
                    span.add(problem.offset());
                    span.add(problem.length());
                    entry.add("fault", span);
                } else {
                    assertEquals(0, problem.offset(), label);
                    assertEquals(0, problem.length(), label);
                }
                assertEquals(0, problem.expected(), label);
                assertEquals(0, problem.found(), label);
            } else {
                assertTrue(problem.sourceColumn().isEmpty(), label);
                assertNull(problem.reason(), label);
                assertNull(problem.detail(), label);
                assertEquals(0, problem.offset(), label);
                assertEquals(0, problem.length(), label);
                entry.addProperty("expected", problem.expected());
                entry.addProperty("found", problem.found());
            }
            if (problem.detail() != null) {
                entry.addProperty("detail", problem.detail().name());
            }
            if (problem.origin() != Origin.CELL) {
                entry.addProperty("origin", problem.origin().name().toLowerCase(Locale.ROOT));
            }
            problems.add(canonical(entry));
        }
    }

    /**
     * Holds what the faults listed to what the corpus says. With notes off the list has no detail
     * or origin to give, and no formula with no cached value.
     */
    static void assertProblems(String label, List<String> found, JsonObject vector, boolean notes) {
        List<String> expected = new ArrayList<>();
        for (JsonElement element : vector.getAsJsonArray("problems")) {
            JsonObject problem = element.getAsJsonObject().deepCopy();
            if (!notes) {
                if (problem.has("origin") && problem.get("origin").getAsString().equals("uncached")) {
                    continue;
                }
                problem.remove("detail");
                problem.remove("origin");
            }
            expected.add(canonical(problem));
        }
        assertEquals(expected, found, label);
    }

    /** The bytes of a case's input. */
    static byte[] inputOf(JsonObject vector) {
        return vector.has("input_hex")
                ? HexFormat.of().parseHex(vector.get("input_hex").getAsString())
                : vector.get("input").getAsString().getBytes(StandardCharsets.UTF_8);
    }

    static Tokens tokensOf(JsonArray tokens) {
        return Tokens.of(stringsOf(tokens));
    }

    private static Dialect dialectOf(JsonObject vector) {
        JsonObject settings = vector.getAsJsonObject("dialect");
        Dialect dialect = new Dialect(
                settings.get("separator").getAsString().charAt(0),
                settings.get("quoting").getAsBoolean(),
                settings.get("has_header").getAsBoolean(),
                settings.get("skip_blank_lines").getAsBoolean());
        if (settings.has("skip")) {
            dialect = dialect.withSkip(settings.get("skip").getAsInt());
        }
        if (settings.has("header_row")) {
            dialect = dialect.withHeaderRow(settings.get("header_row").getAsInt());
        }
        if (settings.has("short_rows")) {
            dialect = dialect.withShortRows(
                    ShortRows.valueOf(settings.get("short_rows").getAsString().toUpperCase(Locale.ROOT)));
        }
        if (settings.has("long_rows")) {
            dialect = dialect.withLongRows(
                    LongRows.valueOf(settings.get("long_rows").getAsString().toUpperCase(Locale.ROOT)));
        }
        if (settings.has("tokens")) {
            dialect = dialect.withTokens(tokensOf(settings.getAsJsonArray("tokens")));
        }
        if (settings.has("encoding")) {
            dialect = dialect.withEncoding(
                    switch (settings.get("encoding").getAsString()) {
                        case "utf-8" -> Encoding.UTF_8;
                        case "windows-1252" -> Encoding.WINDOWS_1252;
                        case "latin-1" -> Encoding.LATIN_1;
                        case "utf-16le" -> Encoding.UTF_16LE;
                        default -> throw new IllegalStateException("unknown encoding " + settings.get("encoding"));
                    });
        }
        return dialect;
    }

    private static void replay(String label, JsonObject vector, Opening opening, int batchRows) {
        replay(label, vector, opening, batchRows, true);
    }

    private static void replay(String label, JsonObject vector, Opening opening, int batchRows, boolean notes) {
        Dialect dialect = dialectOf(vector).withNotes(notes);
        List<Column> plan = new ArrayList<>();
        for (JsonElement entry : vector.getAsJsonArray("plan")) {
            plan.add(columnOf(entry.getAsJsonObject()));
        }
        JsonArray rows = vector.getAsJsonArray("rows");

        // A failure in the header is the opening's: no reader, and no rows before it.
        DelimitedReader opened;
        try {
            opened = opening.open(dialect, plan);
        } catch (TabularException e) {
            assertEquals(0, rows.size(), label);
            assertFailure(label, e, vector);
            assertProblems(label, List.of(), vector, notes);
            return;
        }
        try (DelimitedReader reader = opened) {
            JsonElement header = vector.get("header");
            if (header.isJsonNull()) {
                assertNull(reader.header(), label);
            } else {
                List<String> names = new ArrayList<>();
                header.getAsJsonArray().forEach(name -> names.add(name.getAsString()));
                assertEquals(names, reader.header(), label);
                if (!names.isEmpty()) {
                    assertEquals(names.size(), reader.columnCount().getAsInt(), label);
                }
            }
            if (!reader.isBound()) {
                bindHeaderFirst(
                        label,
                        reader.header(),
                        reader.plan(),
                        reader.isBound(),
                        reader::read,
                        resolved -> reader.bind(resolved, batchRows),
                        plan);
            }
            assertEquals(plan, reader.plan(), label);

            int seen = 0;
            List<String> problems = new ArrayList<>();
            TabularException failure = null;
            try {
                for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                    assertTrue(batch.rows() >= 1 && batch.rows() <= batchRows, label);
                    assertRows(label, batch);
                    appendProblems(label, problems, batch, seen);
                    for (int row = 0; row < batch.rows(); row++, seen++) {
                        assertTrue(seen < rows.size(), label + ": more rows than the corpus lists");
                        JsonArray cells = rows.get(seen).getAsJsonArray();
                        // Every row as wide as the plan, whatever its record held.
                        assertEquals(plan.size(), cells.size(), label);
                        for (int column = 0; column < plan.size(); column++) {
                            assertCell(
                                    label + ", row " + seen + ", column " + column,
                                    batch,
                                    column,
                                    row,
                                    cells.get(column).getAsJsonObject(),
                                    notes);
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
            assertProblems(label, problems, vector, notes);
        }
    }

    private interface Mode {
        void replay(String name, JsonObject vector, byte[] input, int batchRows);
    }

    private static void everyCase(Mode mode) {
        assertTrue(CORPUS.size() >= 123, "the corpus has " + CORPUS.size() + " cases");
        assertEquals(List.of(), CorpusKeys.unknownDelimited(CORPUS));
        for (JsonElement element : CORPUS) {
            JsonObject vector = element.getAsJsonObject();
            String name = vector.get("name").getAsString();
            byte[] input = inputOf(vector);
            for (int batchRows : BATCH_ROWS) {
                mode.replay(name, vector, input, batchRows);
            }
        }
    }

    @Test
    void fromAByteArray() {
        everyCase((name, vector, input, batchRows) -> {
            replay(
                    name + " (byte[], " + batchRows + " rows a batch)",
                    vector,
                    (dialect, plan) -> DelimitedReader.of(input, dialect, plan, batchRows),
                    batchRows);
            replay(
                    name + " (byte[], header first, " + batchRows + " rows a batch)",
                    vector,
                    (dialect, plan) -> DelimitedReader.of(input, dialect),
                    batchRows);
            replay(
                    name + " (byte[], notes off, " + batchRows + " rows a batch)",
                    vector,
                    (dialect, plan) -> DelimitedReader.of(input, dialect, plan, batchRows),
                    batchRows,
                    false);
        });
    }

    @Test
    void fromNativeMemoryInPlace() {
        everyCase((name, vector, input, batchRows) -> {
            try (Arena arena = Arena.ofConfined()) {
                MemorySegment text = nativeCopy(arena, input);
                replay(
                        name + " (native segment, " + batchRows + " rows a batch)",
                        vector,
                        (dialect, plan) -> DelimitedReader.of(text, dialect, plan, batchRows),
                        batchRows);
                replay(
                        name + " (native segment, header first, " + batchRows + " rows a batch)",
                        vector,
                        (dialect, plan) -> DelimitedReader.of(text, dialect),
                        batchRows);
                // And a heap segment, which is copied in as a byte array is.
                replay(
                        name + " (heap segment, " + batchRows + " rows a batch)",
                        vector,
                        (dialect, plan) -> DelimitedReader.of(MemorySegment.ofArray(input), dialect, plan, batchRows),
                        batchRows);
                replay(
                        name + " (heap segment, header first, " + batchRows + " rows a batch)",
                        vector,
                        (dialect, plan) -> DelimitedReader.of(MemorySegment.ofArray(input), dialect),
                        batchRows);
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
            // Windows are the original memory's, and a declared encoding reads a buffer instead.
            if (vector.has("input_hex") || vector.getAsJsonObject("dialect").has("encoding")) {
                return;
            }
            boolean quoting = vector.getAsJsonObject("dialect").get("quoting").getAsBoolean();
            int smallest = longestRecord(input, quoting) + 1;
            try (Arena arena = Arena.ofConfined()) {
                MemorySegment text = nativeCopy(arena, input);
                for (int windowBytes : new int[] {Math.max(smallest, 4), smallest + 7}) {
                    replay(
                            name + " (windows of " + windowBytes + " bytes, " + batchRows + " rows a batch)",
                            vector,
                            (dialect, plan) -> DelimitedReader.windowed(text, dialect, plan, batchRows, windowBytes),
                            batchRows);
                    replay(
                            name + " (windows of " + windowBytes + " bytes, header first, " + batchRows
                                    + " rows a batch)",
                            vector,
                            (dialect, plan) -> DelimitedReader.windowed(text, dialect, null, 0, windowBytes),
                            batchRows);
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
                                new ByteArrayInputStream(input), dialect, plan, batchRows, bufferBytes),
                        batchRows);
                replay(
                        name + " (stream through " + bufferBytes + " bytes, header first, " + batchRows
                                + " rows a batch)",
                        vector,
                        (dialect, plan) -> DelimitedReader.of(new ByteArrayInputStream(input), dialect, bufferBytes),
                        batchRows);
            }
        });
    }

    @Test
    void fromAFile(@TempDir Path directory) throws IOException {
        Path file = directory.resolve("case.csv");
        for (JsonElement element : CORPUS) {
            JsonObject vector = element.getAsJsonObject();
            String name = vector.get("name").getAsString();
            byte[] input = inputOf(vector);
            Files.write(file, input);
            assertArrayEquals(input, Files.readAllBytes(file));
            for (int batchRows : BATCH_ROWS) {
                for (int bufferBytes : new int[] {3, DelimitedReader.DEFAULT_BUFFER_BYTES}) {
                    replay(
                            name + " (file through " + bufferBytes + " bytes, " + batchRows + " rows a batch)",
                            vector,
                            (dialect, plan) -> DelimitedReader.open(file, dialect, plan, batchRows, bufferBytes),
                            batchRows);
                    replay(
                            name + " (file through " + bufferBytes + " bytes, header first, " + batchRows
                                    + " rows a batch)",
                            vector,
                            (dialect, plan) -> DelimitedReader.open(file, dialect, bufferBytes),
                            batchRows);
                }
            }
        }
    }

    @Test
    void everyDelimitedCorpusKeyIsRead() throws IOException {
        CorpusKeys.aJunkKeyIsSeenEverywhere("delimited.json", CorpusKeys::unknownDelimited);
    }

    /** Holds a suggestion to what a corpus case says of it, its param put back together from the report. */
    static void assertSuggestion(String label, Suggestion found, JsonObject expected) {
        assertEquals(expected.get("truncated").getAsBoolean(), found.truncated(), label);
        JsonArray columns = expected.getAsJsonArray("columns");
        assertEquals(columns.size(), found.report().size(), label);
        assertEquals(columns.size(), found.plan().size(), label);
        NumFormat declared = declaredFormat(expected);
        for (int index = 0; index < columns.size(); index++) {
            SuggestedColumn report = found.report().get(index);
            JsonObject want = columns.get(index).getAsJsonObject();
            String where = label + ", column " + index;
            int param = want.get("param").getAsInt();
            int got = report.param();
            if (!report.candidates().isEmpty()) {
                got = 0;
                for (Candidate candidate : report.candidates()) {
                    got |= switch (candidate) {
                        case Candidate.Notation notation ->
                            notation.format().decimalSeparator().equals(".") ? 1 << 1 : 1 << 2;
                        case Candidate.Order order -> 1 << order.order().code();
                    };
                }
            }
            if (report.status() == SuggestStatus.RESOLVED && report.door().isNumeric()) {
                // The plan is in the one notation the param left, and comma groups is also what's
                // used with no separator to prove it. The export's param is held in its own replay.
                NumFormat planned = declared != null
                        ? declared
                        : param == 1 << 2 ? new NumFormat(',', '.', NumFormat.STYLE_ALL) : NumFormat.INVARIANT;
                param = 0;
                assertEquals(planned, report.format(), where);
                assertEquals(planned, found.plan().get(index).format(), where);
            } else {
                assertNull(report.format(), where);
            }
            assertEquals(want.get("door").getAsString(), doorName(report.door()), where);
            assertEquals(param, got, where);
            assertEquals(
                    want.get("status").getAsString(), report.status().name().toLowerCase(Locale.ROOT), where);
            assertEquals(want.get("matched").getAsInt(), report.matched(), where);
            assertEquals(want.get("sampled").getAsInt(), report.sampled(), where);
            assertEquals(index, report.ordinal(), where);
            assertEquals(index, found.plan().get(index).ordinal(), where);
            assertEquals(
                    report.status() == SuggestStatus.RESOLVED ? report.door() : Door.TEXT,
                    found.plan().get(index).door(),
                    where);
        }
    }

    /** A door's name in the corpus. */
    static String doorName(Door door) {
        return door.name().toLowerCase(Locale.ROOT);
    }

    /** What the corpus declares for a case's suggestion. */
    static SuggestOptions suggestOptionsOf(JsonObject suggest) {
        SuggestOptions options = SuggestOptions.DEFAULT;
        if (suggest.has("sample_rows")) {
            options = options.withSampleRows(suggest.get("sample_rows").getAsInt());
        }
        NumFormat format = declaredFormat(suggest);
        return format == null ? options : options.withFormat(format);
    }

    /**
     * Every case that says what to suggest: from memory and from a file; through the export, over
     * the cells the case's own rows say; and a plan that reads back without refusing what it
     * resolved.
     */
    @Test
    void everySuggestionIsWhatTheCorpusSays(@TempDir Path directory) throws IOException {
        int seen = 0;
        for (JsonElement element : CORPUS) {
            JsonObject vector = element.getAsJsonObject();
            if (!vector.has("suggest")) {
                continue;
            }
            seen++;
            JsonObject suggest = vector.getAsJsonObject("suggest");
            String name = vector.get("name").getAsString();
            byte[] input = inputOf(vector);
            Dialect dialect = dialectOf(vector);
            SuggestOptions options = suggestOptionsOf(suggest);
            Suggestion found = DelimitedReader.suggest(input, dialect, options);
            assertSuggestion(name + " (memory)", found, suggest);
            Path file = directory.resolve("suggest-" + seen + ".txt");
            Files.write(file, input);
            assertSuggestion(name + " (file)", DelimitedReader.suggest(file, dialect, options), suggest);
            assertTheExportSays(name, vector, suggest, options);

            // What's resolved takes every cell it saw, and refuses none.
            int sample = suggest.has("sample_rows") ? suggest.get("sample_rows").getAsInt() : Integer.MAX_VALUE;
            try (DelimitedReader reader = DelimitedReader.of(input, dialect, found.plan())) {
                int read = 0;
                for (Batch batch = read < sample ? reader.read() : null;
                        batch != null;
                        batch = read < sample ? reader.read() : null) {
                    for (Problem problem : batch.faults()) {
                        if (problem.row() + read < sample
                                && problem.planColumn().isPresent()) {
                            SuggestedColumn report =
                                    found.report().get(problem.planColumn().getAsInt());
                            assertFalse(
                                    report.status() == SuggestStatus.RESOLVED && report.door() != Door.TEXT,
                                    name + ": read back refused " + problem);
                        }
                    }
                    read += batch.rows();
                }
            }
        }
        assertTrue(seen >= 19, seen + " cases say what to suggest");
    }

    /**
     * The export, over a case's own rows, on whichever path this JDK takes: a cell with no text in
     * the case is blank here, as a read's empty verdict is.
     */
    private static void assertTheExportSays(String name, JsonObject vector, JsonObject suggest, SuggestOptions options)
            throws IOException {
        JsonArray rows = vector.getAsJsonArray("rows");
        int sample = suggest.has("sample_rows") ? suggest.get("sample_rows").getAsInt() : rows.size();
        int width = vector.getAsJsonArray("plan").size();
        java.io.ByteArrayOutputStream text = new java.io.ByteArrayOutputStream();
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment cells = arena.allocate(Math.max((long) width * sample, 1) * Abi.SPAN_BYTES, 4);
            long entry = 0;
            for (int column = 0; column < width; column++) {
                for (int row = 0; row < sample; row++) {
                    JsonObject cell = rows.get(row).getAsJsonArray().get(column).getAsJsonObject();
                    byte[] bytes = cell.has("raw_hex")
                            ? HexFormat.of().parseHex(cell.get("raw_hex").getAsString())
                            : cell.has("text")
                                    ? cell.get("text").getAsString().getBytes(StandardCharsets.UTF_8)
                                    : new byte[0];
                    cells.set(ValueLayout.JAVA_INT, entry * Abi.SPAN_BYTES, text.size());
                    cells.set(ValueLayout.JAVA_INT, entry * Abi.SPAN_BYTES + 4, bytes.length);
                    text.write(bytes);
                    entry++;
                }
            }
            MemorySegment input = nativeCopy(arena, text.toByteArray());
            MemorySegment formats = arena.allocate(Math.max(width, 1) * NativeValues.FORMAT_BYTES, 4);
            for (int column = 0; column < width; column++) {
                NumFormat format = options.declaredFormat(column);
                if (format != null) {
                    NativeValues.writeFormat(format, formats, column * NativeValues.FORMAT_BYTES);
                }
            }
            MemorySegment found = arena.allocate(Math.max(width, 1) * Abi.SUGGESTION_BYTES, 4);
            assertEquals(
                    Abi.OK,
                    Native.suggest(
                            input,
                            input.byteSize(),
                            MemorySegment.NULL,
                            0,
                            cells,
                            MemorySegment.NULL,
                            sample,
                            width,
                            formats,
                            found),
                    name);
            JsonArray columns = suggest.getAsJsonArray("columns");
            for (int column = 0; column < width; column++) {
                JsonObject want = columns.get(column).getAsJsonObject();
                long at = column * Abi.SUGGESTION_BYTES;
                String where = name + ": the export, column " + column;
                assertEquals(
                        want.get("door").getAsString(),
                        doorName(Door.ofCode(found.get(ValueLayout.JAVA_INT, at + Abi.SUGGESTION_DOOR))),
                        where);
                assertEquals(
                        want.get("param").getAsInt(),
                        found.get(ValueLayout.JAVA_INT, at + Abi.SUGGESTION_PARAM),
                        where);
                assertEquals(
                        want.get("status").getAsString(),
                        SuggestStatus.of(found.get(ValueLayout.JAVA_INT, at + Abi.SUGGESTION_STATUS))
                                .name()
                                .toLowerCase(Locale.ROOT),
                        where);
                assertEquals(
                        want.get("matched").getAsInt(),
                        found.get(ValueLayout.JAVA_INT, at + Abi.SUGGESTION_MATCHED),
                        where);
                assertEquals(
                        want.get("sampled").getAsInt(),
                        found.get(ValueLayout.JAVA_INT, at + Abi.SUGGESTION_SAMPLED),
                        where);
            }
        }
    }

    /** Every case the corpus has: a suggestion fails where a read of the case fails, and nowhere else. */
    @Test
    void aSuggestionFailsOnlyWhereAReadOfTheCaseFails() {
        int planned = 0;
        int failed = 0;
        for (JsonElement element : CORPUS) {
            JsonObject vector = element.getAsJsonObject();
            try {
                DelimitedReader.suggest(inputOf(vector), dialectOf(vector));
                planned++;
            } catch (TabularException e) {
                failed++;
                assertTrue(vector.has("failure"), vector.get("name") + ": " + e.getMessage());
            }
        }
        assertTrue(planned > 80, planned + " planned");
        assertTrue(failed > 0, failed + " failed");
    }

    // By hand rather than Arena.allocateFrom, which JDK 21 lacks: testFloor runs this too.
    private static MemorySegment nativeCopy(Arena arena, byte[] bytes) {
        MemorySegment copy = arena.allocate(bytes.length);
        MemorySegment.copy(bytes, 0, copy, ValueLayout.JAVA_BYTE, 0, bytes.length);
        return copy;
    }
}
