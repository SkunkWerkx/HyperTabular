package io.github.skunkwerkx.hypertabular;

/**
 * A structural failure: the input is not rows of cells — a record of the wrong width, input
 * that ends inside a quoted cell. Never a cell's verdict: a value that does not cast is a
 * {@code Fault} in its column, and the read goes on. A structure failure ends the input,
 * after every intact row before it has been delivered.
 */
public final class TabularException extends RuntimeException {
    private static final long serialVersionUID = 1L;

    /** What is wrong. */
    private final TabularFailure failure;
    /** Zero-based index of the offending record. */
    private final long record;
    /** One-based line the offending record starts on. */
    private final int line;
    /** Absolute byte offset of the offending record's start. */
    private final long byteOffset;
    /** Cells in the first record. */
    private final int expected;
    /** Cells in this record. */
    private final int found;

    TabularException(TabularFailure failure, long record, int line, long byteOffset, int expected, int found) {
        super(describe(failure, record, line, byteOffset, expected, found));
        this.failure = failure;
        this.record = record;
        this.line = line;
        this.byteOffset = byteOffset;
        this.expected = expected;
        this.found = found;
    }

    /**
     * What is wrong.
     *
     * @return the kind of failure
     */
    public TabularFailure failure() {
        return failure;
    }

    /**
     * Zero-based index of the offending record — the header and skipped blank lines
     * included.
     *
     * @return the record's index
     */
    public long record() {
        return record;
    }

    /**
     * One-based line the offending record starts on.
     *
     * @return the line number
     */
    public int line() {
        return line;
    }

    /**
     * Absolute byte offset of the offending record's start — a byte-order mark and
     * everything before the record counted.
     *
     * @return the byte offset
     */
    public long byteOffset() {
        return byteOffset;
    }

    /**
     * Cells in the first record, for {@link TabularFailure#COLUMN_COUNT}; {@code 0}
     * otherwise.
     *
     * @return the expected cell count
     */
    public int expected() {
        return expected;
    }

    /**
     * Cells in this record, for {@link TabularFailure#COLUMN_COUNT}; {@code 0} otherwise.
     *
     * @return the cell count found
     */
    public int found() {
        return found;
    }

    private static String describe(
            TabularFailure failure, long record, int line, long byteOffset, int expected, int found) {
        return switch (failure) {
            case COLUMN_COUNT -> "Record " + record + " (line " + line + ", byte " + byteOffset + ") has "
                    + found + " cells; the first record had " + expected + ".";
            case UNCLOSED_QUOTE -> "The input ended inside a quoted cell in record " + record
                    + " (line " + line + ", byte " + byteOffset + ").";
            case ROW_TOO_LONG -> "Record " + record + " (line " + line + ", byte " + byteOffset
                    + ") exceeds the " + DelimitedReader.MAX_ROW_BYTES + "-byte row ceiling.";
        };
    }
}
