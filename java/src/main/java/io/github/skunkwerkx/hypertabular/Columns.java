package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.util.List;
import java.util.Objects;

/**
 * The columns a reader owns: the plan, the plan as the core takes it, and for each column a
 * value array and a verdict array sized for one batch — all native memory in the reader's
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
    static Column[] checked(List<Column> plan, int batchRows) {
        Column[] columns = Objects.requireNonNull(plan, "plan").toArray(Column[]::new);
        if (batchRows <= 0) {
            throw new IllegalArgumentException("batchRows must be positive; got " + batchRows);
        }
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

    Columns(Column[] plan, int batchRows, Arena arena) {
        this.plan = plan;
        this.planList = List.of(plan);
        this.batchRows = batchRows;
        int count = plan.length;
        specs = arena.allocate(Math.max(count, 1) * Native.SPEC_BYTES, 4);
        buffers = arena.allocate(Math.max(count, 1) * Native.BUFFER_BYTES, 8);
        values = new MemorySegment[count];
        verdicts = new MemorySegment[count];
        for (int index = 0; index < count; index++) {
            Column column = plan[index];
            long spec = index * Native.SPEC_BYTES;
            specs.set(ValueLayout.JAVA_INT, spec + Native.SPEC_ORDINAL, column.ordinal());
            specs.set(
                    ValueLayout.JAVA_INT, spec + Native.SPEC_DOOR, column.door().code());
            specs.set(ValueLayout.JAVA_INT, spec + Native.SPEC_PARAM, column.declared());
            NativeValues.writeFormat(column.format(), specs, spec + Native.SPEC_FORMAT);

            // 8-aligned whatever the door, so every value reads through an aligned layout.
            values[index] = arena.allocate((long) batchRows * column.door().valueBytes(), 8);
            verdicts[index] = arena.allocate(batchRows * Native.VERDICT_BYTES, 4);
            long entry = index * Native.BUFFER_BYTES;
            buffers.set(ValueLayout.ADDRESS, entry, values[index]);
            buffers.set(ValueLayout.ADDRESS, entry + Native.BUFFER_VERDICTS, verdicts[index]);
        }
        this.width = reach(plan);
    }
}
