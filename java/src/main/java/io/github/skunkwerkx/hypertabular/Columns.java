package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.NumFormat;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.charset.StandardCharsets;
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
    final int width;

    final MemorySegment specs;
    final MemorySegment buffers;
    final MemorySegment[] values;
    final MemorySegment[] verdicts;

    /** Checks a plan before anything native is allocated for it. */
    static Column[] checked(List<Column> plan, int batchRows) {
        Column[] columns = Objects.requireNonNull(plan, "plan").toArray(Column[]::new);
        if (batchRows <= 0) {
            throw new IllegalArgumentException("batchRows must be positive; got " + batchRows);
        }
        for (int index = 0; index < columns.length; index++) {
            Column column = Objects.requireNonNull(columns[index], "plan column " + index);
            if (column.ordinal() > Integer.MAX_VALUE - 2) {
                throw new IllegalArgumentException("A plan ordinal of " + column.ordinal() + " is out of range.");
            }
        }
        return columns;
    }

    Columns(Column[] plan, int batchRows, Arena arena) {
        this.plan = plan;
        this.planList = List.of(plan);
        this.batchRows = batchRows;
        int count = plan.length;
        int widest = -1;
        specs = arena.allocate(Math.max(count, 1) * Native.SPEC_BYTES, 4);
        buffers = arena.allocate(Math.max(count, 1) * Native.BUFFER_BYTES, 8);
        values = new MemorySegment[count];
        verdicts = new MemorySegment[count];
        for (int index = 0; index < count; index++) {
            Column column = plan[index];
            widest = Math.max(widest, column.ordinal());
            long spec = index * Native.SPEC_BYTES;
            specs.set(ValueLayout.JAVA_INT, spec + Native.SPEC_ORDINAL, column.ordinal());
            specs.set(
                    ValueLayout.JAVA_INT, spec + Native.SPEC_DOOR, column.door().code());
            specs.set(ValueLayout.JAVA_INT, spec + Native.SPEC_PARAM, column.declared());
            NumFormat format = column.format();
            byte[] symbol = format.currencySymbol().getBytes(StandardCharsets.UTF_8);
            specs.set(ValueLayout.JAVA_INT, spec + Native.SPEC_DECIMAL_SEP, format.decimalSeparator());
            specs.set(ValueLayout.JAVA_INT, spec + Native.SPEC_GROUP_SEP, format.groupSeparator());
            specs.set(ValueLayout.JAVA_INT, spec + Native.SPEC_FLAGS, format.styles());
            specs.set(ValueLayout.JAVA_INT, spec + Native.SPEC_CURRENCY_LEN, symbol.length);
            // NumFormat's own constructor holds the symbol to the 16 bytes there are.
            MemorySegment.copy(symbol, 0, specs, ValueLayout.JAVA_BYTE, spec + Native.SPEC_CURRENCY, symbol.length);

            // 8-aligned whatever the door, so every value reads through an aligned layout.
            values[index] = arena.allocate((long) batchRows * column.door().valueBytes(), 8);
            verdicts[index] = arena.allocate(batchRows * Native.VERDICT_BYTES, 4);
            long entry = index * Native.BUFFER_BYTES;
            buffers.set(ValueLayout.ADDRESS, entry, values[index]);
            buffers.set(ValueLayout.ADDRESS, entry + Native.BUFFER_VERDICTS, verdicts[index]);
        }
        this.width = widest + 1;
    }
}
