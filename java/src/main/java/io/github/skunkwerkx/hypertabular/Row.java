package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.Verdict;
import java.lang.foreign.MemorySegment;
import java.nio.CharBuffer;

/**
 * One row of a {@link Batch}: the batch's accessors with the row already chosen, for code
 * that thinks in records — "for each row, build an object from these columns".
 *
 * {@snippet :
 * for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
 *     for (Row row : batch) {
 *         orders.add(new Order(row.get(0, Integer.class), row.string(1)));
 *     }
 * }
 * }
 *
 * <p>A row is a view, not a copy: a batch and an index, nothing more. It reads the batch's
 * column-major storage exactly as the batch's own accessors do, and is valid exactly as long
 * — until the reader is asked for the next batch. Do not keep one past that.
 */
public final class Row {
    private final Batch batch;
    private final int index;

    Row(Batch batch, int index) {
        this.batch = batch;
        this.index = index;
    }

    /**
     * The row's place in its batch, from 0.
     *
     * @return the row's index
     */
    public int index() {
        return index;
    }

    /**
     * Where the row came from: {@link Batch#line} of this row.
     *
     * @return the line or row number
     */
    public int line() {
        return batch.line(index);
    }

    /**
     * The cell in {@code column}, as HyperCast judged it: {@link Batch#get} of this row.
     *
     * @param <T> the value's type
     * @param column index into the plan
     * @param type the value's type, which must be the one the column's door is presented as
     * @return the value, or the fault
     * @throws IllegalStateException if the column's door is not presented as {@code type}
     */
    public <T> Verdict<T> get(int column, Class<T> type) {
        return batch.get(column, index, type);
    }

    /**
     * Whether the cell in {@code column} cast: {@link Batch#isOk} of this row.
     *
     * @param column index into the plan
     * @return {@code true} when the cell cast
     */
    public boolean isOk(int column) {
        return batch.isOk(column, index);
    }

    /**
     * The verdict of the cell in {@code column}: {@link Batch#verdict} of this row.
     *
     * @param column index into the plan
     * @return the cell's verdict
     */
    public CellVerdict verdict(int column) {
        return batch.verdict(column, index);
    }

    /**
     * The bytes of the text cell in {@code column}: {@link Batch#text} of this row.
     *
     * @param column index into the plan
     * @return the cell's UTF-8 bytes, or {@code null} for an empty cell
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}
     */
    public MemorySegment text(int column) {
        return batch.text(column, index);
    }

    /**
     * The text cell in {@code column} as a string: {@link Batch#string} of this row.
     *
     * @param column index into the plan
     * @return the cell's text, or {@code null} for an empty cell
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}
     */
    public String string(int column) {
        return batch.string(column, index);
    }

    /**
     * The text cell in {@code column} as UTF-16, without a string: {@link Batch#chars} of
     * this row.
     *
     * @param column index into the plan
     * @return the cell's text, or {@code null} for an empty cell
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}
     */
    public CharBuffer chars(int column) {
        return batch.chars(column, index);
    }

    /**
     * The text cell in {@code column} decoded into the caller's array: {@link Batch#getChars}
     * of this row.
     *
     * @param column index into the plan
     * @param destination where the text is written
     * @param offset where in {@code destination} it starts
     * @return the chars written, or {@code -1} if they do not fit
     * @throws IllegalStateException if the column is not read through {@link Door#TEXT}
     */
    public int getChars(int column, char[] destination, int offset) {
        return batch.getChars(column, index, destination, offset);
    }

    /**
     * The text the cell in {@code column} was cast from: {@link Batch#raw} of this row.
     *
     * @param column index into the plan
     * @return the cell's UTF-8 bytes
     */
    public MemorySegment raw(int column) {
        return batch.raw(column, index);
    }

    /**
     * {@link #raw} as a string: {@link Batch#rawString} of this row.
     *
     * @param column index into the plan
     * @return the cell's text
     */
    public String rawString(int column) {
        return batch.rawString(column, index);
    }

    @Override
    public String toString() {
        return "Row[index=" + index + ", columns=" + batch.columns().size() + "]";
    }
}
