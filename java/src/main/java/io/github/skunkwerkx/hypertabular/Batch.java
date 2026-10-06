package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.Success;
import io.github.skunkwerkx.hypercast.Verdict;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.math.BigDecimal;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.Instant;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.LocalTime;
import java.util.List;
import java.util.Objects;
import java.util.UUID;

/**
 * The rows of one read, column-major: for each column of the plan, the values its door made
 * and a verdict beside each. One type serves delimited text and workbooks alike; the two
 * differ only in where a cell's text lies, which the batch knows and its accessors hide.
 *
 * <p>A batch is a view of its reader's own native buffers, nothing copied to make it: a
 * column of integers is the array the core wrote, and a text cell is the input itself (or the
 * workbook's shared strings) wherever its bytes could stay where they were. It is valid until
 * the reader is asked for the next batch — the reader hands out the same instance each time —
 * and every segment it hands out is valid as long, and refused by the runtime once the reader
 * is closed. Not thread-safe, and confined to the reader's thread.
 */
public final class Batch {
    private static final ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;
    private static final ValueLayout.OfShort SHORT = ValueLayout.JAVA_SHORT;
    private static final ValueLayout.OfInt INT = ValueLayout.JAVA_INT;
    private static final ValueLayout.OfLong LONG = ValueLayout.JAVA_LONG;

    private final Columns columns;
    private final boolean workbook;
    private int rows;
    private MemorySegment cells;
    private int perRow;
    /** Where an unflagged span's bytes are: the input, or the shared strings. Handed out read-only. */
    private MemorySegment base;
    /** Where the spans' offsets count from in {@code base}. */
    private long baseStart;
    /** Where a flagged span's bytes are. Handed out read-only. */
    private MemorySegment arena;
    /** Where a raw quoted cell is unescaped to; made on first use, closed with the reader. */
    private Block scratch;

    Batch(Columns columns, boolean workbook) {
        this.columns = columns;
        this.workbook = workbook;
    }

    /** Points the batch at what the core just wrote. */
    Batch fill(int rows, MemorySegment cells, int perRow, MemorySegment base, long baseStart, MemorySegment arena) {
        this.rows = rows;
        this.cells = cells;
        this.perRow = perRow;
        this.base = base;
        this.baseStart = baseStart;
        this.arena = arena;
        return this;
    }

    /** Ends the batch: the reader is about to reuse what it points into. */
    void clear() {
        rows = 0;
    }

    /** Releases the scratch a raw read made, with the reader. */
    void close() {
        rows = 0;
        if (scratch != null) {
            scratch.close();
            scratch = null;
        }
    }

    /**
     * How many rows the batch holds. Never zero while the batch is current; zero once the
     * reader has moved on.
     *
     * @return the row count
     */
    public int rows() {
        return rows;
    }

    /**
     * The plan the batch was read through: column {@code i} of the batch is
     * {@code columns().get(i)}.
     *
     * @return the plan, unmodifiable
     */
    public List<Column> columns() {
        return columns.planList;
    }

    /**
     * Where row {@code row} came from: for delimited text the 1-based line its record starts
     * on, for a sheet its 1-based row number.
     *
     * @param row row of the batch
     * @return the line or row number
     */
    public int line(int row) {
        Objects.checkIndex(row, rows);
        long entry = ((long) row * perRow + perRow - 1) * Native.SPAN_BYTES;
        return cells.get(INT, entry + (workbook ? 0 : 4));
    }

    private Door door(int column) {
        return columns.plan[column].door();
    }

    /** The reason code of the cell at ({@code column}, {@code row}): {@code 0} when it cast. */
    private int code(int column, int row) {
        Objects.checkIndex(row, rows);
        return columns.verdicts[column].get(INT, row * Native.VERDICT_BYTES + 8);
    }

    private static CastFailure reason(int code) {
        return switch (code) {
            case 1 -> CastFailure.EMPTY;
            case 2 -> CastFailure.MALFORMED;
            case 3 -> CastFailure.OUT_OF_RANGE;
            default -> throw new IllegalStateException("libhypertabular returned unknown verdict code " + code);
        };
    }

    private <T> Fault<T> fault(int column, int row) {
        MemorySegment verdict = columns.verdicts[column];
        long at = row * Native.VERDICT_BYTES;
        return new Fault<>(reason(verdict.get(INT, at + 8)), verdict.get(INT, at), verdict.get(INT, at + 4));
    }

    /**
     * Whether the cell at ({@code column}, {@code row}) cast, whatever its door — the
     * allocation-free question.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return {@code true} when the cell cast
     */
    public boolean isOk(int column, int row) {
        return code(column, row) == 0;
    }

    /**
     * The verdict of the cell at ({@code column}, {@code row}), whatever its door: whether it
     * cast, and if not, why and where in the cell's own text ({@link #raw}).
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the cell's verdict
     */
    public CellVerdict verdict(int column, int row) {
        int code = code(column, row);
        if (code == 0) {
            return new CellVerdict(null, 0, 0);
        }
        MemorySegment verdict = columns.verdicts[column];
        long at = row * Native.VERDICT_BYTES;
        return new CellVerdict(reason(code), verdict.get(INT, at), verdict.get(INT, at + 4));
    }

    /**
     * A column's verdicts, exactly as the core wrote them: one {@link CellVerdict#LAYOUT}
     * entry per row. Read-only.
     *
     * @param column index into the plan
     * @return the column's verdict array
     */
    public MemorySegment verdicts(int column) {
        return columns.verdicts[column].asSlice(0, rows * Native.VERDICT_BYTES).asReadOnly();
    }

    /**
     * A whole column as the core wrote it, one value per row — for the doors whose value is a
     * primitive: {@link Door#BOOL} as one byte ({@code 0} or {@code 1}), the integer and
     * floating-point doors as their own width, read with the matching {@link ValueLayout}
     * ({@code values.getAtIndex(ValueLayout.JAVA_INT, row)} for {@link Door#I32} and
     * {@link Door#U32}, and so on). The value of a row whose verdict is not ok is zero.
     * Columns of the other doors are read a cell at a time, with {@link #get}. Read-only.
     *
     * @param column index into the plan
     * @return the column's value array
     * @throws IllegalStateException if the column's door does not write a primitive
     */
    public MemorySegment values(int column) {
        Door door = door(column);
        if (!door.isPrimitive()) {
            throw new IllegalStateException(
                    "Column " + column + " is cast through " + door + "; it has no array of primitives.");
        }
        return columns.values[column]
                .asSlice(0, (long) rows * door.valueBytes())
                .asReadOnly();
    }

    /** Whether {@code type} is the Java type a door's value is presented as. */
    private static boolean reads(Class<?> type, Door door) {
        return switch (door) {
            case BOOL -> type == Boolean.class;
            case I8 -> type == Byte.class;
            case I16 -> type == Short.class;
            case I32, U8, U16 -> type == Integer.class;
            case I64, U32, U64 -> type == Long.class;
            case F32 -> type == Float.class;
            case F64 -> type == Double.class;
            case DECIMAL -> type == BigDecimal.class;
            case UUID -> type == UUID.class;
            case TIMESTAMP, UNIX, EXCEL_SERIAL -> type == Instant.class;
            case DATE, DATE_ORDERED -> type == LocalDate.class;
            case DATETIME -> type == LocalDateTime.class;
            case TIME -> type == LocalTime.class;
            case DURATION -> type == Duration.class;
            case TEXT -> type == String.class;
        };
    }

    /**
     * The cell at ({@code column}, {@code row}) as HyperCast judged it: the value, or the
     * fault. {@code type} is the Java type the column's door is presented as:
     *
     * <table class="striped">
     * <caption>Doors and their types</caption>
     * <thead><tr><th scope="col">Type</th><th scope="col">Doors</th></tr></thead>
     * <tbody>
     * <tr><td>{@link Boolean}</td><td>{@link Door#BOOL}</td></tr>
     * <tr><td>{@link Byte}, {@link Short}</td><td>{@link Door#I8}, {@link Door#I16}</td></tr>
     * <tr><td>{@link Integer}</td><td>{@link Door#I32}; {@link Door#U8} and {@link Door#U16}, widened</td></tr>
     * <tr><td>{@link Long}</td><td>{@link Door#I64}; {@link Door#U32}, widened; {@link Door#U64}, its bits</td></tr>
     * <tr><td>{@link Float}, {@link Double}</td><td>{@link Door#F32}, {@link Door#F64}</td></tr>
     * <tr><td>{@link BigDecimal}</td><td>{@link Door#DECIMAL}, exact</td></tr>
     * <tr><td>{@link UUID}</td><td>{@link Door#UUID}</td></tr>
     * <tr><td>{@link Instant}</td><td>{@link Door#TIMESTAMP}, {@link Door#UNIX}, {@link Door#EXCEL_SERIAL}</td></tr>
     * <tr><td>{@link LocalDate}</td><td>{@link Door#DATE}, {@link Door#DATE_ORDERED}</td></tr>
     * <tr><td>{@link LocalDateTime}</td><td>{@link Door#DATETIME}</td></tr>
     * <tr><td>{@link LocalTime}</td><td>{@link Door#TIME}</td></tr>
     * <tr><td>{@link Duration}</td><td>{@link Door#DURATION}</td></tr>
     * <tr><td>{@link String}</td><td>{@link Door#TEXT}</td></tr>
     * </tbody>
     * </table>
     *
     * <p>The temporal types keep all nine fractional digits the core parsed.
     *
     * @param <T> the value's type
     * @param column index into the plan
     * @param row row of the batch
     * @param type the value's type, which must be the one the column's door is presented as
     * @return the value, or the fault
     * @throws IllegalStateException if the column's door is not presented as {@code type}
     * @throws IndexOutOfBoundsException if the row is not in the batch
     */
    public <T> Verdict<T> get(int column, int row, Class<T> type) {
        Door door = door(column);
        if (!reads(type, door)) {
            throw new IllegalStateException("Column " + column + " is cast through " + door
                    + ", which is not read as a " + type.getSimpleName() + ".");
        }
        if (code(column, row) != 0) {
            return fault(column, row);
        }
        MemorySegment value = columns.values[column];
        Object cell = switch (door) {
            case BOOL -> value.get(BYTE, row) != 0;
            case I8 -> value.get(BYTE, row);
            case I16 -> value.get(SHORT, row * 2L);
            case I32 -> value.get(INT, row * 4L);
            case U8 -> Byte.toUnsignedInt(value.get(BYTE, row));
            case U16 -> Short.toUnsignedInt(value.get(SHORT, row * 2L));
            case I64, U64 -> value.get(LONG, row * 8L);
            case U32 -> Integer.toUnsignedLong(value.get(INT, row * 4L));
            case F32 -> value.get(ValueLayout.JAVA_FLOAT, row * 4L);
            case F64 -> value.get(ValueLayout.JAVA_DOUBLE, row * 8L);
            case DECIMAL -> decimal(value, row * 16L);
            // RFC 9562 byte order is exactly UUID's msb/lsb decomposition: two big-endian longs.
            case UUID ->
                new UUID(
                        value.get(Native.Downcalls.BIG_ENDIAN_LONG, row * 16L),
                        value.get(Native.Downcalls.BIG_ENDIAN_LONG, row * 16L + 8));
            case TIMESTAMP, UNIX, EXCEL_SERIAL ->
                Instant.ofEpochSecond(value.get(LONG, row * 16L), value.get(INT, row * 16L + 8));
            case DATE, DATE_ORDERED -> day(value, row * 4L);
            case DATETIME ->
                LocalDateTime.of(day(value, row * 16L), LocalTime.ofNanoOfDay(value.get(LONG, row * 16L + 8)));
            case TIME -> LocalTime.ofNanoOfDay(value.get(LONG, row * 8L));
            // Duration.ofSeconds normalizes the core's same-signed nanos adjustment correctly.
            case DURATION -> Duration.ofSeconds(value.get(LONG, row * 16L), value.get(INT, row * 16L + 8));
            case TEXT -> string(column, row);
        };
        return new Success<>(type.cast(cell));
    }

    // {u64 lo, u32 hi, u8 scale, u8 negative}. BigInteger takes its magnitude big-endian, so
    // the two words are laid out high word first; the core never hands back a negative zero,
    // so the signum needs no zero check.
    private static BigDecimal decimal(MemorySegment value, long at) {
        byte[] magnitude = new byte[12];
        ByteBuffer.wrap(magnitude).putInt(value.get(INT, at + 8)).putLong(value.get(LONG, at));
        int signum = value.get(BYTE, at + 13) != 0 ? -1 : 1;
        return new BigDecimal(new BigInteger(signum, magnitude), value.get(BYTE, at + 12));
    }

    private static LocalDate day(MemorySegment value, long at) {
        return LocalDate.of(
                Short.toUnsignedInt(value.get(SHORT, at)), value.get(BYTE, at + 2), value.get(BYTE, at + 3));
    }

    /** The bytes a span names, read-only: the arena's when it is flagged, the base's when not. */
    private MemorySegment bytes(int offset, int span) {
        MemorySegment bytes =
                span < 0 ? arena.asSlice(offset, span & Native.SPAN_LENGTH) : base.asSlice(baseStart + offset, span);
        return bytes.asReadOnly();
    }

    private static String decode(MemorySegment from) {
        return new String(from.toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8);
    }

    private void requireText(int column) {
        if (door(column) != Door.TEXT) {
            throw new IllegalStateException("Column " + column + " is cast through " + door(column) + ", not TEXT.");
        }
    }

    /**
     * The cell of a {@link Door#TEXT} column: its bytes, untrimmed — quotes resolved for
     * delimited text, and a typed workbook cell said the canonical way ({@code 42},
     * {@code true}, {@code 2024-01-31T10:30:00}, {@code PT1H30M}) — or {@code null} for a
     * cell with no bytes at all, which is the one way text fails. A read-only view.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the cell's UTF-8 bytes, or {@code null} for an empty cell
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}
     */
    public MemorySegment text(int column, int row) {
        requireText(column);
        if (code(column, row) != 0) {
            return null;
        }
        MemorySegment value = columns.values[column];
        return bytes(value.get(INT, row * 8L), value.get(INT, row * 8L + 4));
    }

    /**
     * The cell of a {@link Door#TEXT} column as a string, or {@code null} for an empty cell.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the cell's text, or {@code null} for an empty cell
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}
     */
    public String string(int column, int row) {
        MemorySegment text = text(column, row);
        return text == null ? null : decode(text);
    }

    /**
     * The text the cell at ({@code column}, {@code row}) was cast from, whatever its door and
     * whatever its verdict — what a fault's span indexes, and what to show for a value that
     * did not cast. For a workbook this is a text cell's own text, and what a typed cell was
     * said as when it failed its door (or went through the text door); a typed cell that cast
     * has none, and neither has an empty cell. A read-only view, valid until the next call to
     * {@code raw} or {@link #rawString}, or the next read.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the cell's UTF-8 bytes
     */
    public MemorySegment raw(int column, int row) {
        Objects.checkIndex(row, rows);
        Objects.checkIndex(column, columns.plan.length);
        int at = workbook ? column : columns.plan[column].ordinal();
        long entry = ((long) row * perRow + at) * Native.SPAN_BYTES;
        int offset = cells.get(INT, entry);
        int span = cells.get(INT, entry + 4);
        if (workbook || span >= 0) {
            return bytes(offset, span);
        }
        // A quoted cell with an escaped quote in it: unescaped, as the core cast it.
        int length = span & Native.SPAN_LENGTH;
        if (scratch == null) {
            scratch = new Block(Math.max(length, 256), 1);
        } else if (scratch.bytes() < length) {
            scratch.resize(length, 0);
        }
        long written =
                Native.unescape(base.asSlice(baseStart + offset, length), length, scratch.segment, scratch.bytes());
        return scratch.readOnly.asSlice(0, written);
    }

    /**
     * {@link #raw} as a string: the text the cell was cast from, for a diagnostic. A fault's
     * span is in bytes of {@link #raw}, as the core reports it; on text with a character
     * outside ASCII it is not a {@code substring} range of this string.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the cell's text
     */
    public String rawString(int column, int row) {
        return decode(raw(column, row));
    }

    @Override
    public String toString() {
        return "Batch[rows=" + rows + ", columns=" + columns.plan.length + "]";
    }
}
