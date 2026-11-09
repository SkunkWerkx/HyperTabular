package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.DateOptions;
import io.github.skunkwerkx.hypercast.MonthNames;
import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.util.List;
import java.util.Objects;

/**
 * The columns a reader owns: the plan, the plan as the core takes it, and for each column a
 * value array, a verdict array and, unless the read turned them off, a note array sized for one batch — all native memory in the reader's
 * arena, allocated once and reused for every batch.
 */
final class Columns {
    final Column[] plan;
    final List<Column> planList;
    final int batchRows;
    /** The widest ordinal the plan reads, plus one: the row slots a sheet needs. */
    final long width;

    final MemorySegment specs;
    final MemorySegment buffers;
    final MemorySegment[] values;
    final MemorySegment[] verdicts;
    /** Each column's notes: {@code null} when the read keeps none. */
    final MemorySegment[] notes;
    /** How many cells each row had: {@link MemorySegment#NULL} when the read does not ask. */
    final MemorySegment rowWidths;

    /**
     * The widest ordinal a plan may name: below 2^20. No workbook format has that many columns
     * (XLSX and ODS stop at 16,384, Numbers at 1,000), and at the cap one row of cell table is
     * at most 8 MiB, so a headerless plan cannot ask for one of 16 GiB.
     */
    static final int MAX_ORDINAL = (1 << 20) - 1;

    /**
     * Checks a plan before anything native is allocated for it: every column present, and its
     * ordinal below 2^20.
     */
    static Column[] checked(List<Column> plan) {
        Column[] columns = Objects.requireNonNull(plan, "plan").toArray(Column[]::new);
        for (int index = 0; index < columns.length; index++) {
            Objects.requireNonNull(columns[index], "plan column " + index);
            int ordinal = columns[index].ordinal();
            if (ordinal > MAX_ORDINAL) {
                throw new IllegalArgumentException("Plan column " + index + " reads source column " + ordinal
                        + "; an ordinal must be below 2^20 (1,048,576).");
            }
        }
        return columns;
    }

    /** The widest ordinal {@code plan} reads, plus one. */
    static long reach(Column[] plan) {
        long width = 0;
        for (Column column : plan) {
            width = Math.max(width, column.ordinal() + 1L);
        }
        return width;
    }

    /**
     * The core's answer on a read's sizes, ahead of everything the read allocates: the core decides
     * how much a batch may ask for, and says what size to start an input buffer at.
     *
     * @return the size to start the input buffer at
     * @throws IllegalArgumentException for a size the core refuses, naming the bytes and the limit
     *     the core gave
     */
    static int checkSizes(MemorySegment specs, long columnCount, int batchRows, int bufferBytes, int flags) {
        // The core takes an unsigned count, which a negative one isn't.
        if (batchRows < 0) {
            throw new IllegalArgumentException("batchRows must not be negative; got " + batchRows);
        }
        if (bufferBytes < 0) {
            throw new IllegalArgumentException("bufferBytes must not be negative; got " + bufferBytes);
        }
        try (Arena scope = Arena.ofConfined()) {
            MemorySegment found = scope.allocate(Abi.BATCH_CHECK_BYTES, 8);
            int code = Native.batchCheck(specs, columnCount, batchRows, bufferBytes, flags, found);
            if (code == Abi.OK) {
                return (int) found.get(ValueLayout.JAVA_LONG, Abi.BATCH_CHECK_BUFFER_BYTES);
            }
            if (code != Abi.ERR_BATCH) {
                throw new IllegalArgumentException("The plan cannot be honoured.");
            }
            int refused = found.get(ValueLayout.JAVA_INT, Abi.BATCH_CHECK_REFUSED);
            if (refused == Abi.REFUSED_BATCH_ROWS_ZERO) {
                throw new IllegalArgumentException("batchRows must be at least 1");
            }
            if (refused == Abi.REFUSED_BUFFER_BYTES_ZERO) {
                throw new IllegalArgumentException("bufferBytes must be at least 1");
            }
            if (refused == Abi.REFUSED_ROW_TOO_WIDE) {
                throw new IllegalArgumentException("A row of "
                        + Long.toUnsignedString(found.get(ValueLayout.JAVA_LONG, Abi.BATCH_CHECK_WIDTH))
                        + " columns asks for "
                        + Long.toUnsignedString(found.get(ValueLayout.JAVA_LONG, Abi.BATCH_CHECK_NEEDED))
                        + " bytes, past the limit of "
                        + Long.toUnsignedString(found.get(ValueLayout.JAVA_LONG, Abi.BATCH_CHECK_LIMIT)));
            }
            throw new IllegalArgumentException("batchRows asks for "
                    + Long.toUnsignedString(found.get(ValueLayout.JAVA_LONG, Abi.BATCH_CHECK_NEEDED))
                    + " bytes, past the limit of "
                    + Long.toUnsignedString(found.get(ValueLayout.JAVA_LONG, Abi.BATCH_CHECK_LIMIT)));
        }
    }

    /**
     * The rows of a sampler's batch for a sampler wanting {@code wanted} rows: the core's answer,
     * however many batches that takes. Only a row past the limit is refused, in its words.
     */
    static int sampleRows(List<Column> plan, int wanted, int flags) {
        try (Arena scope = Arena.ofConfined()) {
            int count = plan.size();
            MemorySegment specs = scope.allocate(Math.max(count, 1) * Abi.SPEC_WITH_BYTES, 8);
            for (int index = 0; index < count; index++) {
                Column column = plan.get(index);
                long spec = index * Abi.SPEC_WITH_BYTES;
                specs.set(ValueLayout.JAVA_INT, spec, (int) Abi.SPEC_WITH_BYTES);
                specs.set(ValueLayout.JAVA_INT, spec + Abi.SPEC_WITH_ORDINAL, column.ordinal());
                specs.set(
                        ValueLayout.JAVA_INT,
                        spec + Abi.SPEC_WITH_DOOR,
                        column.door().code());
                specs.set(ValueLayout.JAVA_INT, spec + Abi.SPEC_WITH_PARAM, column.declared());
            }
            MemorySegment found = scope.allocate(Abi.BATCH_CHECK_BYTES, 8);
            Native.batchCheck(specs, count, wanted, 1, flags, found);
            long rows = found.get(ValueLayout.JAVA_LONG, Abi.BATCH_CHECK_SAMPLE_ROWS);
            if (rows == 0) {
                checkSizes(specs, count, wanted, 1, flags);
            }
            return (int) rows;
        }
    }

    Columns(Column[] plan, int batchRows, Arena arena, boolean keepNotes, boolean keepRowWidths, boolean sheet) {
        this.plan = plan;
        this.planList = List.of(plan);
        this.batchRows = batchRows;
        int count = plan.length;
        specs = arena.allocate(Math.max(count, 1) * Abi.SPEC_WITH_BYTES, 8);
        buffers = arena.allocate(Math.max(count, 1) * Abi.BUFFER_WITH_BYTES, 8);
        values = new MemorySegment[count];
        verdicts = new MemorySegment[count];
        notes = keepNotes ? new MemorySegment[count] : null;
        for (int index = 0; index < count; index++) {
            Column column = plan[index];
            long spec = index * Abi.SPEC_WITH_BYTES;
            specs.set(ValueLayout.JAVA_INT, spec, (int) Abi.SPEC_WITH_BYTES);
            specs.set(ValueLayout.JAVA_INT, spec + Abi.SPEC_WITH_ORDINAL, column.ordinal());
            specs.set(
                    ValueLayout.JAVA_INT,
                    spec + Abi.SPEC_WITH_DOOR,
                    column.door().code());
            specs.set(ValueLayout.JAVA_INT, spec + Abi.SPEC_WITH_PARAM, column.declared());
            NativeValues.writeFormat(column.format(), specs, spec + Abi.SPEC_WITH_FORMAT);
            // All of it zeroed by the arena: no date options, no months, tokens inherited.
            DateOptions date = column.dateOptions();
            MonthNames months = column.months();
            if (date != null || months != null) {
                DateOptions written = date != null ? date : DateOptions.NONE;
                NativeValues.writeDateOptions(written.flags(), written.pivot(), specs, spec + Abi.SPEC_WITH_DATE);
            }
            if (months != null) {
                byte[] packed = months.packed();
                specs.set(ValueLayout.ADDRESS, spec + Abi.SPEC_WITH_MONTHS, held(packed, arena));
                specs.set(ValueLayout.JAVA_LONG, spec + Abi.SPEC_WITH_MONTHS + 8, packed.length);
            }
            // An empty list has an address: null would inherit the dialect's or the sheet's.
            if (column.tokens() != null) {
                byte[] packed = column.tokens().packed();
                specs.set(ValueLayout.ADDRESS, spec + Abi.SPEC_WITH_TOKENS, held(packed, arena));
                specs.set(ValueLayout.JAVA_LONG, spec + Abi.SPEC_WITH_TOKENS + 8, packed.length);
            }
        }
        checkSizes(
                specs,
                count,
                batchRows,
                1,
                (keepNotes ? Abi.BATCH_NOTES : 0)
                        | (keepRowWidths ? Abi.BATCH_WIDTHS : 0)
                        | (sheet ? Abi.BATCH_SHEET : 0));
        rowWidths = keepRowWidths ? arena.allocate(batchRows * 4L, 4) : MemorySegment.NULL;
        for (int index = 0; index < count; index++) {
            Column column = plan[index];
            // 8-aligned whatever the door, so every value reads through an aligned layout.
            values[index] = arena.allocate((long) batchRows * column.door().valueBytes(), 8);
            verdicts[index] = arena.allocate(batchRows * Abi.VERDICT_BYTES, 4);
            long entry = index * Abi.BUFFER_WITH_BYTES;
            buffers.set(ValueLayout.JAVA_INT, entry, (int) Abi.BUFFER_WITH_BYTES);
            buffers.set(ValueLayout.ADDRESS, entry + Abi.BUFFER_WITH_VALUES, values[index]);
            buffers.set(ValueLayout.ADDRESS, entry + Abi.BUFFER_WITH_VERDICTS, verdicts[index]);
            if (notes != null) {
                notes[index] = arena.allocate(batchRows * Abi.NOTE_BYTES, 4);
                buffers.set(ValueLayout.ADDRESS, entry + Abi.BUFFER_WITH_NOTES, notes[index]);
            }
        }
        this.width = reach(plan);
    }

    /** A copy of {@code list} in native memory, at an address that is never null. */
    static MemorySegment held(byte[] list, Arena arena) {
        MemorySegment copy = arena.allocate(Math.max(list.length, 1));
        MemorySegment.copy(list, 0, copy, ValueLayout.JAVA_BYTE, 0, list.length);
        return copy;
    }
}
