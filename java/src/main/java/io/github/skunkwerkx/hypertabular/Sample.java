package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.channels.ClosedByInterruptException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.function.Consumer;
import java.util.function.Supplier;

/** A sample, in the shape the core takes it: each column's spans into text. */
final class Sample {
    /** Bit 1 of a numeric param: a format with {@code ,} grouping digits in threes. Western. */
    private static final int COMMA_GROUPS = 1 << 1;
    /** Bit 2 of a numeric param: a format with {@code ,} as the decimal separator. Western. */
    private static final int COMMA_DECIMAL = 1 << 2;

    private static final NumFormat COMMA_DECIMAL_FORMAT = new NumFormat(',', '.', NumFormat.STYLE_ALL);

    /** Past this a Java array cannot go: the most text or table entries a sample holds. */
    private static final long ARRAY_MAX = Integer.MAX_VALUE - 8;

    private static final ValueLayout.OfInt INT = ValueLayout.JAVA_INT;

    private final int width;
    private final int limit;
    private final boolean sheet;
    /** The spans' offsets and lengths, row after row. */
    private int[] offsets;

    private int[] lengths;
    /** A byte for each span of a sheet's: not 0 for a typed cell. */
    private byte[] typed;

    private byte[] text = new byte[4096];
    private int length;
    private int rows;
    private boolean truncated;

    private Sample(int width, int limit, boolean sheet) {
        this.width = width;
        this.limit = limit;
        this.sheet = sheet;
        int cells = Math.max(width, 1);
        offsets = new int[cells];
        lengths = new int[cells];
        typed = sheet ? new byte[cells] : null;
    }

    /**
     * Reads a source of {@code width} columns through the text door until the sample is whole and
     * a row past it, or the end, says whether it is the source's last. A failure among the sampled
     * rows is the caller's, and a failure just past them only says there is more.
     *
     * <p>A sheet's sample binds a second, UUID, door to every column, for what it says of a cell:
     * no door but a text door takes a typed cell's kind, so there it is malformed with no detail,
     * where a text cell that is not a UUID has one. That is the read's word for which cells were
     * typed, and it is what keeps a typed number out of the question of what {@code ,} and
     * {@code .} mean.
     */
    static Sample of(Supplier<Batch> source, Consumer<List<Column>> bind, int width, int limit, boolean sheet) {
        if (width > Columns.MAX_ORDINAL + 1) {
            throw new IllegalArgumentException(
                    "A suggestion cannot plan for " + width + " columns; an ordinal must be below 2^20 (1,048,576).");
        }
        Sample sample = new Sample(width, limit, sheet);
        if (width == 0) {
            return sample;
        }
        List<Column> plan = new ArrayList<>(sheet ? 2 * width : width);
        for (int ordinal = 0; ordinal < width; ordinal++) {
            plan.add(Column.text(ordinal));
        }
        if (sheet) {
            for (int ordinal = 0; ordinal < width; ordinal++) {
                plan.add(Column.uuid(ordinal));
            }
        }
        bind.accept(plan);
        while (!sample.truncated) {
            Batch batch;
            try {
                batch = source.get();
            } catch (TabularException | UncheckedIOException failure) {
                // A thread interrupt is the caller asking to stop, not a failure of the source.
                if (sample.rows != limit || interrupted(failure)) {
                    throw failure;
                }
                sample.truncated = true;
                break;
            }
            if (batch == null) {
                break;
            }
            sample.take(batch);
        }
        return sample;
    }

    /**
     * Whether a read failed because the thread was interrupted: a file's channel closes on an
     * interrupt, and the read says so with a {@link ClosedByInterruptException} under it.
     */
    private static boolean interrupted(RuntimeException failure) {
        for (Throwable cause = failure.getCause(); cause != null; cause = cause.getCause()) {
            if (cause instanceof ClosedByInterruptException) {
                return true;
            }
        }
        return false;
    }

    /**
     * Takes what the sample still wants of a batch. A blank, a token and padding are all a blank
     * span. A refused cell keeps its bytes, for the core's own text door to refuse.
     */
    private void take(Batch batch) {
        for (int row = 0; row < batch.rows(); row++) {
            if (rows == limit) {
                truncated = true;
                return;
            }
            // A span holds a 32-bit offset, and an array stops before that. A sample that far
            // along stops, and says it did.
            if ((rows + 1L) * width > ARRAY_MAX) {
                truncated = true;
                return;
            }
            int start = length;
            boolean fits = true;
            int at = roomForRow();
            for (int column = 0; column < width && fits; column++) {
                fits = append(cell(batch, column, row), at + column);
            }
            if (!fits) {
                length = start;
                truncated = true;
                return;
            }
            if (sheet) {
                for (int column = 0; column < width; column++) {
                    int second = width + column;
                    boolean mark = batch.verdict(second, row).reason() == CastFailure.MALFORMED
                            && batch.detail(second, row) == null;
                    typed[at + column] = (byte) (mark ? 1 : 0);
                }
            }
            rows++;
        }
    }

    private static MemorySegment cell(Batch batch, int column, int row) {
        CellVerdict verdict = batch.verdict(column, row);
        if (verdict.isOk()) {
            return batch.text(column, row);
        }
        return verdict.reason() != CastFailure.EMPTY ? batch.raw(column, row) : null;
    }

    /** Makes room for one more row of spans, and says where it starts. */
    private int roomForRow() {
        int at = rows * width;
        if (at + width > offsets.length) {
            int cells = (int) Math.min(Math.max((long) offsets.length * 2, (long) at + width), ARRAY_MAX);
            offsets = Arrays.copyOf(offsets, cells);
            lengths = Arrays.copyOf(lengths, cells);
            if (typed != null) {
                typed = Arrays.copyOf(typed, cells);
            }
        }
        return at;
    }

    private boolean append(MemorySegment bytes, int at) {
        int size = bytes == null ? 0 : (int) bytes.byteSize();
        long end = (long) length + size;
        if (end > ARRAY_MAX) {
            return false;
        }
        if (end > text.length) {
            text = Arrays.copyOf(text, (int) Math.min(Math.max(end, (long) text.length * 2), ARRAY_MAX));
        }
        if (size > 0) {
            MemorySegment.copy(bytes, ValueLayout.JAVA_BYTE, 0, text, length, size);
        }
        offsets[at] = length;
        lengths[at] = size;
        length = (int) end;
        return true;
    }

    /** Asks the core, and builds the plan and the report from its answer. */
    Suggestion judge(Header header, SuggestOptions options) {
        try (Arena scope = Arena.ofConfined()) {
            long cellCount = (long) width * rows;
            MemorySegment input = scope.allocate(Math.max(length, 1));
            MemorySegment.copy(text, 0, input, ValueLayout.JAVA_BYTE, 0, length);
            MemorySegment cells = scope.allocate(Math.max(cellCount, 1) * Abi.SPAN_BYTES, 4);
            MemorySegment marks = sheet ? scope.allocate(Math.max(cellCount, 1)) : MemorySegment.NULL;
            // Column after column, as the core takes them.
            long entry = 0;
            for (int column = 0; column < width; column++) {
                for (int row = 0; row < rows; row++) {
                    int from = row * width + column;
                    cells.set(INT, entry * Abi.SPAN_BYTES, offsets[from]);
                    cells.set(INT, entry * Abi.SPAN_BYTES + 4, lengths[from]);
                    if (sheet) {
                        marks.set(ValueLayout.JAVA_BYTE, entry, typed[from]);
                    }
                    entry++;
                }
            }
            NumFormat[] declared = new NumFormat[width];
            // Zeroed: a size of 0 is no format.
            MemorySegment formats = scope.allocate(Math.max(width, 1) * NativeValues.FORMAT_BYTES, 4);
            for (int ordinal = 0; ordinal < width; ordinal++) {
                declared[ordinal] = options.declaredFormat(ordinal);
                if (declared[ordinal] != null) {
                    NativeValues.writeFormat(declared[ordinal], formats, ordinal * NativeValues.FORMAT_BYTES);
                }
            }
            MemorySegment found = scope.allocate(Math.max(width, 1) * Abi.SUGGESTION_BYTES, 4);
            int code = Native.suggest(input, length, MemorySegment.NULL, 0, cells, marks, rows, width, formats, found);
            if (code != Abi.OK) {
                throw new IllegalStateException(Workbook.CONTRACT_VIOLATION);
            }
            Column[] plan = new Column[width];
            SuggestedColumn[] report = new SuggestedColumn[width];
            for (int ordinal = 0; ordinal < width; ordinal++) {
                long at = ordinal * Abi.SUGGESTION_BYTES;
                report[ordinal] = settled(
                        reportOf(
                                ordinal,
                                found.get(INT, at + Abi.SUGGESTION_DOOR),
                                found.get(INT, at + Abi.SUGGESTION_PARAM),
                                found.get(INT, at + Abi.SUGGESTION_STATUS),
                                found.get(INT, at + Abi.SUGGESTION_MATCHED),
                                found.get(INT, at + Abi.SUGGESTION_SAMPLED),
                                header,
                                declared[ordinal]),
                        options.declaredOrder(ordinal));
                plan[ordinal] = planOf(report[ordinal]);
            }
            return new Suggestion(List.of(plan), List.of(report), truncated);
        }
    }

    /**
     * What the core said of a column, in this binding's terms. Every door the core has is taken
     * here, not only those it suggests today: a door it does not name, or a param its door does
     * not declare, reads as text.
     */
    static SuggestedColumn reportOf(
            int ordinal,
            int doorCode,
            int foundParam,
            int statusCode,
            int matched,
            int sampled,
            Header header,
            NumFormat declared) {
        SuggestStatus status = SuggestStatus.of(statusCode);
        String name = header != null && ordinal < header.size() ? header.get(ordinal) : null;
        Door door = Door.ofCode(doorCode);
        int param = foundParam;
        List<Candidate> candidates = List.of();
        if (status == SuggestStatus.AMBIGUOUS) {
            // An ambiguous param is a set of candidates, bit n for code n.
            candidates = candidatesOf(door, foundParam);
            param = Integer.numberOfTrailingZeros(foundParam);
        }
        if (door != null && door.isNumeric()) {
            param = 0;
        }
        if (!declares(door, param)) {
            door = Door.TEXT;
            param = 0;
        } else if (!takesParam(door)) {
            param = 0;
        }
        NumFormat format = null;
        if (status == SuggestStatus.RESOLVED && door.isNumeric()) {
            if (declared != null) {
                format = declared;
            } else {
                format = foundParam == COMMA_DECIMAL ? COMMA_DECIMAL_FORMAT : NumFormat.INVARIANT;
            }
        }
        return new SuggestedColumn(ordinal, name, door, param, format, candidates, status, matched, sampled);
    }

    /** Whether a door is one, and {@code param} is what it declares. */
    private static boolean declares(Door door, int param) {
        if (door == null) {
            return false;
        }
        return switch (door) {
            case UNIX -> UnixPrecision.fromCode(param).isPresent();
            case DATE_ORDERED, DATETIME -> DateOrder.fromCode(param).isPresent();
            case EXCEL_SERIAL -> ExcelEpoch.fromCode(param).isPresent();
            default -> true;
        };
    }

    private static boolean takesParam(Door door) {
        return switch (door) {
            case UNIX, DATE_ORDERED, DATETIME, EXCEL_SERIAL -> true;
            default -> false;
        };
    }

    /**
     * Answers a date the sample left open with the order declared for its column, if that is one
     * of the candidates.
     */
    private static SuggestedColumn settled(SuggestedColumn report, DateOrder order) {
        if (order == null || report.status() != SuggestStatus.AMBIGUOUS) {
            return report;
        }
        for (Candidate candidate : report.candidates()) {
            if (candidate instanceof Candidate.Order date && date.order() == order) {
                return new SuggestedColumn(
                        report.ordinal(),
                        report.name(),
                        Door.DATE_ORDERED,
                        order.code(),
                        null,
                        List.of(),
                        SuggestStatus.RESOLVED,
                        report.matched(),
                        report.sampled());
            }
        }
        return report;
    }

    /** What an ambiguous column's param offers. */
    private static List<Candidate> candidatesOf(Door door, int param) {
        List<Candidate> candidates = new ArrayList<>();
        if (door == Door.DATE_ORDERED || door == Door.DATETIME) {
            for (DateOrder order :
                    List.of(DateOrder.YEAR_MONTH_DAY, DateOrder.MONTH_DAY_YEAR, DateOrder.DAY_MONTH_YEAR)) {
                if ((param & (1 << order.code())) != 0) {
                    candidates.add(new Candidate.Order(order));
                }
            }
            return List.copyOf(candidates);
        }
        if ((param & COMMA_GROUPS) != 0) {
            candidates.add(new Candidate.Notation(NumFormat.INVARIANT));
        }
        if ((param & COMMA_DECIMAL) != 0) {
            candidates.add(new Candidate.Notation(COMMA_DECIMAL_FORMAT));
        }
        return List.copyOf(candidates);
    }

    /**
     * The column a report plans: only a resolved column reads through its door, a numeric one in
     * the format the report names.
     */
    static Column planOf(SuggestedColumn report) {
        return report.status() == SuggestStatus.RESOLVED
                ? Column.planned(
                        report.ordinal(),
                        report.door(),
                        report.param(),
                        report.format() != null ? report.format() : NumFormat.INVARIANT)
                : Column.text(report.ordinal());
    }
}
