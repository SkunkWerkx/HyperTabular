package io.github.skunkwerkx.hypertabular;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.charset.StandardCharsets;
import java.util.Objects;
import java.util.Optional;

/**
 * The part of a sheet a read keeps: its corners, 1-based like the {@code A5:F200} they are
 * written as, and included, a 0 meaning no limit on that side; {@link #ALL} is all of the sheet.
 * The first column is ordinal 0 of a plan, while {@link Batch#line} and
 * {@link Problem#sourceColumn()} stay the sheet's own. A range that runs past the sheet's end
 * reads what is there, and adds no rows of its own.
 *
 * @param firstRow the first row, or 0 for no limit
 * @param firstColumn the first column, {@code A} being 1, or 0 for no limit
 * @param lastRow the last row, included, or 0 for no limit
 * @param lastColumn the last column, included, or 0 for no limit
 */
public record Range(int firstRow, int firstColumn, int lastRow, int lastColumn) {
    /** All of the sheet. */
    public static final Range ALL = new Range(0, 0, 0, 0);

    /**
     * Checks the corners.
     *
     * @throws IllegalArgumentException if a number is negative, or a last comes before its first
     */
    public Range {
        if (firstRow < 0
                || firstColumn < 0
                || lastRow < 0
                || lastColumn < 0
                || (firstRow != 0 && lastRow != 0 && firstRow > lastRow)
                || (firstColumn != 0 && lastColumn != 0 && firstColumn > lastColumn)) {
            throw new IllegalArgumentException("Rows " + firstRow + " to " + lastRow + ", columns " + firstColumn
                    + " to " + lastColumn + " is not a range.");
        }
    }

    /**
     * Reads {@code A5:F200}: both corners, in capitals, the second no less than the first in
     * either direction. The core reads it, so every binding agrees on what one is.
     *
     * @param text the range
     * @return the range
     * @throws IllegalArgumentException if it is not one: {@code A5} alone, {@code a5:f200} or
     *     {@code F200:A5}
     */
    public static Range parse(String text) {
        return tryParse(text).orElseThrow(() -> new IllegalArgumentException("\"" + text + "\" is not a range."));
    }

    /**
     * Reads {@code A5:F200} as {@link #parse} does.
     *
     * @param text the range
     * @return the range, or empty if it is not one
     */
    public static Optional<Range> tryParse(String text) {
        byte[] utf8 = Objects.requireNonNull(text, "text").getBytes(StandardCharsets.UTF_8);
        try (Arena scope = Arena.ofConfined()) {
            MemorySegment bytes = scope.allocate(Math.max(utf8.length, 1));
            MemorySegment.copy(utf8, 0, bytes, ValueLayout.JAVA_BYTE, 0, utf8.length);
            MemorySegment raw = scope.allocate(Abi.RANGE_BYTES, 4);
            if (Native.rangeParse(bytes, utf8.length, raw) != Abi.OK) {
                return Optional.empty();
            }
            return Optional.of(new Range(
                    raw.get(ValueLayout.JAVA_INT, 0),
                    raw.get(ValueLayout.JAVA_INT, 4),
                    raw.get(ValueLayout.JAVA_INT, 8),
                    raw.get(ValueLayout.JAVA_INT, 12)));
        }
    }
}
