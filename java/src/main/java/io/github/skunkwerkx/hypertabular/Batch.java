package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.Checked;
import io.github.skunkwerkx.hypercast.CheckedVerdict;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.FaultDetail;
import io.github.skunkwerkx.hypercast.Keyword;
import io.github.skunkwerkx.hypercast.Success;
import io.github.skunkwerkx.hypercast.Verdict;
import io.github.skunkwerkx.hypercast.Violation;
import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.math.BigDecimal;
import java.nio.ByteBuffer;
import java.nio.CharBuffer;
import java.nio.charset.CharsetDecoder;
import java.nio.charset.CoderResult;
import java.nio.charset.CodingErrorAction;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.Instant;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.LocalTime;
import java.util.ConcurrentModificationException;
import java.util.Iterator;
import java.util.List;
import java.util.NoSuchElementException;
import java.util.Objects;
import java.util.OptionalInt;
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
 *
 * <p>A batch is also its rows, in order — {@code for (Row row : batch)} — each a {@link Row}
 * view with the batch's accessors, valid as long as the batch is.
 */
public final class Batch implements Iterable<Row> {
    private static final ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;
    private static final ValueLayout.OfShort SHORT = ValueLayout.JAVA_SHORT;
    private static final ValueLayout.OfInt INT = ValueLayout.JAVA_INT;
    private static final ValueLayout.OfLong LONG = ValueLayout.JAVA_LONG;

    private final Columns columns;
    private final boolean workbook;
    /** What a plan's ordinal is short of the sheet's own column: a range's start. */
    private final int firstColumn;

    private int rows;
    /** Moves whenever the batch is over, so that a list asked of it never answers for the next. */
    private long generation;
    /** The width a long row is held to, as the read last knew it. */
    private int expected;
    /** Where {@link #faults()} is answered: the cursor, the count, then the problems. Made on first use. */
    private Block listed;

    private MemorySegment cells;
    private long perRow;
    /** Where an unflagged span's bytes are: the input, or the shared strings. Handed out read-only. */
    private MemorySegment base;
    /** Where the spans' offsets count from in {@code base}. */
    private long baseStart;
    /** Where a flagged span's bytes are. Handed out read-only. */
    private MemorySegment arena;
    /** Where a raw quoted cell is unescaped to; made on first use, closed with the reader. */
    private Block scratch;
    /**
     * Where {@link #chars} decodes to: made on first use, filled from the front, and started
     * over at each batch. Replaced, never resized, when it runs out, so that a view handed out
     * earlier keeps the old array, unchanged.
     */
    private char[] chars;
    /** {@link #chars}, read-only, for the views it hands out. */
    private CharBuffer charsView;

    private int charsUsed;
    /** Chars decoded for this batch, in every arena it has had: what the next arena is sized by. */
    private long charsDecoded;
    /** What a cell that is not UTF-8 is decoded by, replacing as {@link #string} does; made on first use. */
    private CharsetDecoder decoder;

    /** For the tests: the char arena starts with room for one cell and grows only to what a cell needs. */
    static boolean stingyChars;

    /** For the tests: how many times a char arena was made or replaced. */
    static long charsGrown;

    Batch(Columns columns, boolean workbook, int firstColumn) {
        this.columns = columns;
        this.workbook = workbook;
        this.firstColumn = firstColumn;
    }

    /** Points the batch at what the core just wrote. */
    Batch fill(
            int rows,
            MemorySegment cells,
            long perRow,
            MemorySegment base,
            long baseStart,
            MemorySegment arena,
            int expected) {
        generation++;
        this.expected = expected;
        this.rows = rows;
        this.cells = cells;
        this.perRow = perRow;
        this.base = base;
        this.baseStart = baseStart;
        this.arena = arena;
        charsUsed = 0;
        charsDecoded = 0;
        return this;
    }

    /** Ends the batch: the reader is about to reuse what it points into. */
    void clear() {
        generation++;
        rows = 0;
    }

    /** Releases the scratch a raw read made, with the reader. */
    void close() {
        generation++;
        rows = 0;
        if (scratch != null) {
            scratch.close();
            scratch = null;
        }
        if (listed != null) {
            listed.close();
            listed = null;
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
     * Row {@code row} of the batch, as a {@link Row} view.
     *
     * @param row row of the batch
     * @return the row, valid as long as the batch is
     * @throws IndexOutOfBoundsException if the row is not in the batch
     */
    public Row row(int row) {
        return new Row(this, Objects.checkIndex(row, rows));
    }

    /**
     * The batch's rows, in order: what {@code for (Row row : batch)} walks. Each row is a view
     * of this batch, valid until the next read, as the batch is. Going on with the iterator after
     * the next read throws {@link ConcurrentModificationException}, and never answers for the
     * next batch.
     *
     * @return an iterator over the rows
     */
    @Override
    public Iterator<Row> iterator() {
        long walkedFor = generation;
        return new Iterator<>() {
            private int next;

            @Override
            public boolean hasNext() {
                stillCurrent(walkedFor);
                return next < rows;
            }

            @Override
            public Row next() {
                stillCurrent(walkedFor);
                if (next >= rows) {
                    throw new NoSuchElementException();
                }
                return new Row(Batch.this, next++);
            }
        };
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
        long entry = ((long) row * perRow + perRow - 1) * Abi.SPAN_BYTES;
        return cells.get(INT, entry + (workbook ? 0 : 4));
    }

    private Door door(int column) {
        return columns.plan[column].door();
    }

    /** The reason code of the cell at ({@code column}, {@code row}): {@code 0} when it cast. */
    private int code(int column, int row) {
        Objects.checkIndex(row, rows);
        return columns.verdicts[column].get(INT, row * Abi.VERDICT_BYTES + 8);
    }

    private <T> Fault<T> fault(int column, int row) {
        MemorySegment verdict = columns.verdicts[column];
        long at = row * Abi.VERDICT_BYTES;
        return NativeValues.fault(verdict.get(INT, at + 8), verdict.get(INT, at), verdict.get(INT, at + 4));
    }

    /**
     * Whether the cell at ({@code column}, {@code row}) cast and broke no constraint, whatever its
     * door — the allocation-free question.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return {@code true} when the cell cast and broke no constraint
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
        if (code == Abi.VERDICT_VIOLATION) {
            MemorySegment verdict = columns.verdicts[column];
            long at = row * Abi.VERDICT_BYTES;
            return new CellVerdict(null, verdict.get(INT, at), verdict.get(INT, at + 4), keyword(column, row));
        }
        Fault<Object> fault = fault(column, row);
        return new CellVerdict(fault.reason(), fault.offset(), fault.length());
    }

    /** The word a note holds at {@code at}: 0 with notes off. */
    private int note(int column, int row, long at) {
        Objects.checkIndex(row, rows);
        Objects.checkIndex(column, columns.plan.length);
        return columns.notes == null
                ? 0
                : Short.toUnsignedInt(columns.notes[column].get(SHORT, row * Abi.NOTE_BYTES + at));
    }

    /**
     * What failed in the cell at ({@code column}, {@code row}): finer than its verdict's reason,
     * for people diagnosing rejected data.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the detail, one this binding does not name keeping its code; {@code null} for a cell
     *     that cast, a cell nothing more is known of, and with notes off
     *     ({@link Dialect#notes()}, {@link SheetOptions#notes()})
     */
    public FaultDetail detail(int column, int row) {
        int code = note(column, row, 0);
        return code == 0 ? null : FaultDetail.of(code);
    }

    /**
     * Where the verdict of the cell at ({@code column}, {@code row}) came from. With notes off
     * ({@link Dialect#notes()}, {@link SheetOptions#notes()}) every cell says {@link Origin#CELL}:
     * a token's or a padded cell's verdict is still empty, and nothing was kept of why.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return where it came from
     */
    public Origin origin(int column, int row) {
        return Origin.of(note(column, row, 2));
    }

    /**
     * Every cell of the batch that went wrong, and every row longer than the width under
     * {@link LongRows#REPORT}, in row order and within a row in plan order. An empty cell is not
     * on the list unless it is a formula with no cached value, and a token or padding is not: both
     * are what was declared. With notes off every cell's detail is {@code null} and its origin
     * {@link Origin#CELL}, so no uncached formula is listed.
     *
     * <p>The list belongs to this batch: going on with it after the next read throws
     * {@link ConcurrentModificationException}, and never answers for the next batch.
     *
     * @return the list, walked from the start each time it is asked for an iterator
     */
    public Iterable<Problem> faults() {
        long listedFor = generation;
        return () -> new Faults(listedFor);
    }

    private void stillCurrent(long listedFor) {
        if (listedFor != generation) {
            throw new ConcurrentModificationException("The batch this walks is over: the reader has read on.");
        }
    }

    /** A walk of {@link #faults()}, a page at a time, each page on the heap before it is walked. */
    private final class Faults implements Iterator<Problem> {
        private static final int HELD = 64;
        private static final long PROBLEMS = 16;

        private final long listedFor;
        private final Problem[] held = new Problem[HELD];
        private long cursor;
        private int count;
        private int next;
        private boolean finished;

        Faults(long listedFor) {
            this.listedFor = listedFor;
        }

        @Override
        public boolean hasNext() {
            stillCurrent(listedFor);
            if (next == count && !finished) {
                page();
            }
            return next < count;
        }

        @Override
        public Problem next() {
            if (!hasNext()) {
                throw new NoSuchElementException();
            }
            return held[next++];
        }

        private void page() {
            if (listed == null) {
                listed = new Block(PROBLEMS + HELD * Abi.PROBLEM_BYTES, 8);
            }
            MemorySegment memory = listed.segment;
            memory.set(LONG, 0, cursor);
            boolean report = columns.rowWidths.address() != 0 && !workbook;
            int code = Native.faults(
                    columns.buffers,
                    columns.plan.length,
                    rows,
                    report ? columns.rowWidths : MemorySegment.NULL,
                    expected,
                    (report ? LongRows.REPORT : LongRows.REJECT).code(),
                    memory,
                    memory.asSlice(PROBLEMS),
                    HELD,
                    memory.asSlice(8));
            if (code != Abi.OK) {
                throw new IllegalStateException(Workbook.CONTRACT_VIOLATION);
            }
            cursor = memory.get(LONG, 0);
            count = (int) memory.get(LONG, 8);
            next = 0;
            finished = count < HELD;
            for (int index = 0; index < count; index++) {
                held[index] = problem(memory, PROBLEMS + index * Abi.PROBLEM_BYTES);
            }
        }
    }

    private Problem problem(MemorySegment raw, long at) {
        int row = raw.get(INT, at);
        int line = line(row);
        int planColumn = raw.get(INT, at + Abi.PROBLEM_PLAN_COLUMN);
        int expectedWidth = raw.get(INT, at + Abi.PROBLEM_EXPECTED);
        int found = raw.get(INT, at + Abi.PROBLEM_FOUND);
        if (planColumn == Abi.PROBLEM_ROW_LEVEL) {
            return new Problem(
                    row,
                    line,
                    OptionalInt.empty(),
                    OptionalInt.empty(),
                    null,
                    null,
                    Origin.CELL,
                    0,
                    0,
                    expectedWidth,
                    found);
        }
        int offset = raw.get(INT, at + Abi.PROBLEM_OFFSET);
        int length = raw.get(INT, at + Abi.PROBLEM_LENGTH);
        int detail = Short.toUnsignedInt(raw.get(SHORT, at + Abi.PROBLEM_DETAIL));
        int code = raw.get(INT, at + Abi.PROBLEM_REASON);
        // A violation names its keyword and has no reason.
        boolean broke = code == Abi.VERDICT_VIOLATION;
        CastFailure reason =
                broke ? null : NativeValues.<Object>fault(code, offset, length).reason();
        return new Problem(
                row,
                line,
                OptionalInt.of(planColumn),
                OptionalInt.of(columns.plan[planColumn].ordinal() + firstColumn),
                reason,
                detail == 0 ? null : FaultDetail.of(detail),
                Origin.of(Short.toUnsignedInt(raw.get(SHORT, at + Abi.PROBLEM_ORIGIN))),
                offset,
                length,
                expectedWidth,
                found,
                broke ? new Keyword(raw.get(INT, at + Abi.PROBLEM_KEYWORD)) : null);
    }

    /**
     * A column's verdicts, exactly as the core wrote them: one {@link CellVerdict#LAYOUT}
     * entry per row. Read-only. A reason of 0 is a value. On a column that declares
     * constraints, a reason of 4 is a value that broke one, which {@link #getChecked} names.
     * Any other is the cell's fault, its span in the cell's text.
     *
     * @param column index into the plan
     * @return the column's verdict array
     */
    public MemorySegment verdicts(int column) {
        return columns.verdicts[column].asSlice(0, rows * Abi.VERDICT_BYTES).asReadOnly();
    }

    /**
     * A whole column as the core wrote it, one value per row, nothing copied — for every door
     * but {@link Door#TEXT}. {@link Door#BOOL} is one byte ({@code 0} or {@code 1}), and the
     * integer and floating-point doors are their own width, read with the matching
     * {@link ValueLayout} ({@code values.getAtIndex(ValueLayout.JAVA_INT, row)} for
     * {@link Door#I32} and {@link Door#U32}, and so on). Every other door's value is the layout
     * HyperCast's {@link NativeValues} reads, whose {@code *_BYTES} constant is the stride:
     * {@code NativeValues.decimal(values, row * NativeValues.DECIMAL_BYTES)} is what
     * {@link #get} makes of a {@link Door#DECIMAL} cell, and likewise {@code uuid},
     * {@code instant}, {@code date}, {@code civil}, {@code time} and {@code duration}. The value
     * of a row whose verdict is a fault is zero; a violation's is the value as cast. A text column is read a cell at a time, with
     * {@link #text}. Read-only.
     *
     * @param column index into the plan
     * @return the column's value array
     * @throws IllegalStateException if the column is read through {@link Door#TEXT}
     */
    public MemorySegment values(int column) {
        Door door = door(column);
        if (door == Door.TEXT) {
            throw new IllegalStateException("Column " + column + " is cast through TEXT; it has no array of values.");
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
     * @throws IllegalStateException if the column's door is not presented as {@code type}, or the
     *     column declares constraints, whose cells {@link #getChecked} reads
     * @throws IndexOutOfBoundsException if the row is not in the batch
     */
    public <T> Verdict<T> get(int column, int row, Class<T> type) {
        unchecked(column);
        Door door = door(column);
        if (!reads(type, door)) {
            throw new IllegalStateException("Column " + column + " is cast through " + door
                    + ", which is not read as a " + type.getSimpleName() + ".");
        }
        if (code(column, row) != 0) {
            return fault(column, row);
        }
        return new Success<>(type.cast(cell(column, row, door)));
    }

    /**
     * Refuses a column that declares constraints: a violation is no fault, so a plain getter would
     * read it as passed.
     */
    private void unchecked(int column) {
        if (columns.plan[column].isChecked()) {
            throw new IllegalStateException("Column " + column + " declares constraints: read it with getChecked.");
        }
    }

    /** The keyword the cell at ({@code column}, {@code row}) broke, a violation's. */
    private Keyword keyword(int column, int row) {
        return new Keyword(columns.checks[column].get(INT, row * Abi.CHECK_BYTES));
    }

    /**
     * The cell at ({@code column}, {@code row}) as HyperCast cast and judged it: the value, flagged
     * when {@code multipleOf}'s rounding adjusted it and naming the {@code enum} entry it matched, or
     * the rejection, a {@link Fault} as {@link #get} gives it or a {@link Violation} naming the keyword
     * broken and carrying the value as cast. {@code type} is the type {@link #get} presents the door
     * as, for the numeric doors and {@link Door#TEXT}, where an {@code enum} match reads as the entry
     * as declared, the caller's own string. A column that declares no constraints reads too: every
     * cell that cast passes, unadjusted.
     *
     * @param <T> the value's type
     * @param column index into the plan
     * @param row row of the batch
     * @param type the value's type, which must be the one the column's door is presented as
     * @return the value, or the rejection
     * @throws IllegalStateException if the column's door has no checked door presented as {@code type}
     * @throws IndexOutOfBoundsException if the row is not in the batch
     */
    public <T> CheckedVerdict<T> getChecked(int column, int row, Class<T> type) {
        Door door = door(column);
        boolean checkable = switch (door) {
            case I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, DECIMAL, TEXT -> true;
            default -> false;
        };
        if (!checkable || !reads(type, door)) {
            throw new IllegalStateException("Column " + column + " is cast through " + door
                    + ", which has no checked door read as a " + type.getSimpleName() + ".");
        }
        int code = code(column, row);
        if (code != 0 && code != Abi.VERDICT_VIOLATION) {
            return fault(column, row);
        }
        T value = type.cast(cell(column, row, door));
        MemorySegment checks = columns.checks[column];
        if (checks == null) {
            return new Checked<>(value, false, OptionalInt.empty());
        }
        if (code == Abi.VERDICT_VIOLATION) {
            MemorySegment verdict = columns.verdicts[column];
            long at = row * Abi.VERDICT_BYTES;
            return new Violation<>(keyword(column, row), value, verdict.get(INT, at), verdict.get(INT, at + 4));
        }
        long at = row * Abi.CHECK_BYTES;
        boolean adjusted = checks.get(INT, at + Abi.CHECK_ADJUSTED) != 0;
        int index = checks.get(INT, at + Abi.CHECK_ENUM_INDEX);
        if (index == Abi.CHECK_NO_ENUM_INDEX) {
            return new Checked<>(value, adjusted, OptionalInt.empty());
        }
        if (door == Door.TEXT) {
            // The entry as declared, the caller's own string.
            value = type.cast(columns.plan[column]
                    .constraints()
                    .enumValues()
                    .orElseThrow()
                    .get(index));
        }
        return new Checked<>(value, adjusted, OptionalInt.of(index));
    }

    /** The value the core wrote for the cell at ({@code column}, {@code row}), as {@link #get} presents it. */
    private Object cell(int column, int row, Door door) {
        MemorySegment value = columns.values[column];
        return switch (door) {
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
            case DECIMAL -> NativeValues.decimal(value, row * NativeValues.DECIMAL_BYTES);
            case UUID -> NativeValues.uuid(value, row * NativeValues.UUID_BYTES);
            case TIMESTAMP, UNIX, EXCEL_SERIAL -> NativeValues.instant(value, row * NativeValues.TIMESTAMP_BYTES);
            case DATE, DATE_ORDERED -> NativeValues.date(value, row * NativeValues.DATE_BYTES);
            case DATETIME -> NativeValues.civil(value, row * NativeValues.CIVIL_BYTES);
            case TIME -> NativeValues.time(value, row * NativeValues.TIME_BYTES);
            case DURATION -> NativeValues.duration(value, row * NativeValues.DURATION_BYTES);
            case TEXT -> decode(bytes(value.get(INT, row * 8L), value.get(INT, row * 8L + 4)));
        };
    }

    /** The bytes a span names, read-only: the arena's when it is flagged, the base's when not. */
    private MemorySegment bytes(int offset, int span) {
        MemorySegment bytes =
                span < 0 ? arena.asSlice(offset, span & Abi.SPAN_LENGTH) : base.asSlice(baseStart + offset, span);
        return bytes.asReadOnly();
    }

    private static String decode(MemorySegment from) {
        return new String(from.toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8);
    }

    private void requireText(int column) {
        unchecked(column);
        if (door(column) != Door.TEXT) {
            throw new IllegalStateException("Column " + column + " is cast through " + door(column) + ", not TEXT.");
        }
    }

    /**
     * The cell of a {@link Door#TEXT} column: its bytes, untrimmed — quotes resolved for
     * delimited text, and a typed workbook cell said the canonical way ({@code 42},
     * {@code true}, {@code 2024-01-31T10:30:00}, {@code PT1H30M}) — or {@code null} for a
     * cell with no bytes at all, or one whose bytes are not UTF-8, the two ways text fails. A
     * read-only view.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the cell's UTF-8 bytes, or {@code null} for an empty cell or one that is not UTF-8
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}, or declares
     *     constraints, whose cells {@link #getChecked} reads
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
     * The cell of a {@link Door#TEXT} column as a string, or {@code null} for an empty cell or
     * one that is not UTF-8, which {@link #verdict} tells apart and {@link #rawString} still reads.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the cell's text, or {@code null} for an empty cell or one that is not UTF-8
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}, or declares
     *     constraints, whose cells {@link #getChecked} reads
     */
    public String string(int column, int row) {
        MemorySegment text = text(column, row);
        return text == null ? null : decode(text);
    }

    /**
     * The cell of a {@link Door#TEXT} column as UTF-16, without making a string: its bytes
     * decoded into a char array the batch keeps, and handed out as a read-only view of it — position zero, limit the
     * cell's length. Valid until the next read, as everything a batch gives out is: the array
     * is only ever appended to while the batch is current, so a view handed out earlier is
     * never written over, and it starts over with the next batch, so after the first few
     * batches it is large enough and nothing but the view itself is allocated.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @return the cell's text, or {@code null} for a cell {@link #string} says it of
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}, or declares
     *     constraints, whose cells {@link #getChecked} reads
     */
    public CharBuffer chars(int column, int row) {
        requireText(column);
        if (code(column, row) != 0) {
            return null;
        }
        MemorySegment value = columns.values[column];
        int offset = value.get(INT, row * 8L);
        int span = value.get(INT, row * 8L + 4);
        int length = span & Abi.SPAN_LENGTH;
        // UTF-16 never takes more units than UTF-8 takes bytes, a replacement included.
        if (chars == null || chars.length - charsUsed < length) {
            // A view handed out earlier keeps the old array alive and unchanged; the new one
            // starts empty, with room for twice what this batch has decoded so far, so that
            // the next batch like it fits in one.
            long room = stingyChars ? Math.max(length, 1) : Math.max((charsDecoded + length) * 2, 1024);
            chars = new char[(int) Math.min(room, Integer.MAX_VALUE - 8)];
            charsView = CharBuffer.wrap(chars).asReadOnlyBuffer();
            charsUsed = 0;
            charsGrown++;
        }
        int written = span < 0
                ? decode(arena, offset, length, chars, charsUsed, length)
                : decode(base, baseStart + offset, length, chars, charsUsed, length);
        CharBuffer view = charsView.slice(charsUsed, written);
        charsUsed += written;
        charsDecoded += written;
        return view;
    }

    /**
     * The cell of a {@link Door#TEXT} column decoded as UTF-16 into {@code destination} from
     * {@code offset}. Nothing is allocated. An empty cell, and one that is not UTF-8, writes
     * nothing and returns {@code 0}; any other has at least one char.
     *
     * @param column index into the plan
     * @param row row of the batch
     * @param destination where the text is written
     * @param offset where in {@code destination} it starts
     * @return the chars written, or {@code -1} if they do not fit between {@code offset} and
     *     the end of {@code destination} — in which case what is there is unspecified
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}, or declares
     *     constraints, whose cells {@link #getChecked} reads
     * @throws IndexOutOfBoundsException if {@code offset} is not within {@code destination}
     */
    public int getChars(int column, int row, char[] destination, int offset) {
        requireText(column);
        Objects.checkFromToIndex(offset, destination.length, destination.length);
        if (code(column, row) != 0) {
            return 0;
        }
        MemorySegment value = columns.values[column];
        int at = value.get(INT, row * 8L);
        int span = value.get(INT, row * 8L + 4);
        int length = span & Abi.SPAN_LENGTH;
        int room = destination.length - offset;
        return span < 0
                ? decode(arena, at, length, destination, offset, room)
                : decode(base, baseStart + at, length, destination, offset, room);
    }

    /**
     * Decodes {@code length} bytes of UTF-8 from {@code from} at {@code at} into {@code to} at
     * {@code offset}, writing no more than {@code room} chars. Returns how many it wrote, or
     * {@code -1} if they do not fit. A text cell that reaches here has passed the door, so it is
     * decoded here, a byte at a time; should anything be wrong in it all the same, it is handed
     * whole to the JDK's own decoder.
     */
    private int decode(MemorySegment from, long at, int length, char[] to, int offset, int room) {
        int written = 0;
        int index = 0;
        while (index < length) {
            int lead = from.get(BYTE, at + index) & 0xFF;
            if (lead < 0x80) {
                if (written == room) {
                    return -1;
                }
                to[offset + written++] = (char) lead;
                index++;
                continue;
            }
            int extra;
            int point;
            int least;
            if (lead >= 0xC2 && lead <= 0xDF) {
                extra = 1;
                point = lead & 0x1F;
                least = 0x80;
            } else if (lead >= 0xE0 && lead <= 0xEF) {
                extra = 2;
                point = lead & 0x0F;
                least = 0x800;
            } else if (lead >= 0xF0 && lead <= 0xF4) {
                extra = 3;
                point = lead & 0x07;
                least = 0x10000;
            } else {
                return decodeReplacing(from, at, length, to, offset, room);
            }
            if (index + extra >= length) {
                // Cut off by the end of the cell.
                return decodeReplacing(from, at, length, to, offset, room);
            }
            for (int next = 1; next <= extra; next++) {
                int trail = from.get(BYTE, at + index + next) & 0xFF;
                if ((trail & 0xC0) != 0x80) {
                    return decodeReplacing(from, at, length, to, offset, room);
                }
                point = (point << 6) | (trail & 0x3F);
            }
            // Overlong, past the last code point, or a surrogate's own encoding.
            if (point < least || point > Character.MAX_CODE_POINT || (point >= 0xD800 && point <= 0xDFFF)) {
                return decodeReplacing(from, at, length, to, offset, room);
            }
            if (point < 0x10000) {
                if (written == room) {
                    return -1;
                }
                to[offset + written++] = (char) point;
            } else {
                if (room - written < 2) {
                    return -1;
                }
                to[offset + written++] = Character.highSurrogate(point);
                to[offset + written++] = Character.lowSurrogate(point);
            }
            index += extra + 1;
        }
        return written;
    }

    /** {@link #decode} of text that is not well-formed UTF-8: the JDK's decoder, replacing. */
    private int decodeReplacing(MemorySegment from, long at, int length, char[] to, int offset, int room) {
        if (decoder == null) {
            decoder = StandardCharsets.UTF_8
                    .newDecoder()
                    .onMalformedInput(CodingErrorAction.REPLACE)
                    .onUnmappableCharacter(CodingErrorAction.REPLACE);
        }
        ByteBuffer in = from.asSlice(at, length).asByteBuffer();
        CharBuffer out = CharBuffer.wrap(to, offset, room);
        decoder.reset();
        CoderResult result = decoder.decode(in, out, true);
        if (result.isOverflow() || decoder.flush(out).isOverflow()) {
            return -1;
        }
        return out.position() - offset;
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
        long entry = ((long) row * perRow + at) * Abi.SPAN_BYTES;
        int offset = cells.get(INT, entry);
        int span = cells.get(INT, entry + 4);
        if (workbook || span >= 0) {
            return bytes(offset, span);
        }
        // A quoted cell with an escaped quote in it: unescaped, as the core cast it.
        int length = span & Abi.SPAN_LENGTH;
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
