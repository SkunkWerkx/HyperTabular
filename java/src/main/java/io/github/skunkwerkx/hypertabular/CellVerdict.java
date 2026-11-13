package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.Keyword;
import java.lang.foreign.MemoryLayout;
import java.lang.foreign.StructLayout;
import java.lang.foreign.ValueLayout;

/**
 * One cell's verdict whatever its door: whether it cast, and if not, HyperCast's reason and
 * the offending span within the cell's own text; or, for a cell that cast and broke a constraint its
 * column declares, the keyword it broke and the judged text's span. What {@link Batch#verdict} reads
 * out of a column's verdict array, for code that wants the verdict without the value.
 *
 * <p>{@link #LAYOUT} is the entry the core writes, for code that scans a whole column's
 * verdicts in place through {@link Batch#verdicts}.
 *
 * @param reason why the cell did not cast, or {@code null} when it did
 * @param offset byte offset of the offending span within the cell's text; {@code 0} for a
 *     cell that cast
 * @param length byte length of the offending span; {@code 0} for a cell that cast
 * @param keyword for a cell that cast and broke a constraint, the keyword it broke; {@code null}
 *     otherwise
 */
public record CellVerdict(CastFailure reason, int offset, int length, Keyword keyword) {
    /**
     * One entry of a column's verdict array as the core writes it: {@code offset} and
     * {@code length} of the offending span, then {@code reason} — HyperCast's
     * {@link CastFailure#code()}, with {@code 0} for a cell that cast and {@code 4} for one that
     * cast and broke a constraint (the column's checks name the keyword). Twelve bytes, three
     * native-order {@code int}s.
     */
    public static final StructLayout LAYOUT = MemoryLayout.structLayout(
            ValueLayout.JAVA_INT.withName("offset"),
            ValueLayout.JAVA_INT.withName("length"),
            ValueLayout.JAVA_INT.withName("reason"));

    /**
     * A verdict that names no keyword: a fault, or a cell that cast.
     *
     * @param reason why the cell did not cast, or {@code null} when it did
     * @param offset byte offset of the offending span within the cell's text
     * @param length byte length of the offending span
     */
    public CellVerdict(CastFailure reason, int offset, int length) {
        this(reason, offset, length, null);
    }

    /**
     * Whether the cell cast, and broke no constraint.
     *
     * @return {@code true} when the cell cast and broke no constraint
     */
    public boolean isOk() {
        return reason == null && keyword == null;
    }

    /**
     * Whether the cell cast and broke a constraint its column declares: not a fault.
     *
     * @return {@code true} for a violation
     */
    public boolean isViolation() {
        return keyword != null;
    }

    /**
     * The verdict as HyperCast's {@link Fault}. Only for a cell that did not cast.
     *
     * @param <T> the value type of the verdict the fault is to inhabit
     * @return the fault
     * @throws IllegalStateException if the cell cast, whether or not it broke a constraint; there is
     *     no fault
     */
    public <T> Fault<T> toFault() {
        if (reason == null) {
            throw new IllegalStateException("The cell cast; it has no fault.");
        }
        return new Fault<>(reason, offset, length);
    }
}
