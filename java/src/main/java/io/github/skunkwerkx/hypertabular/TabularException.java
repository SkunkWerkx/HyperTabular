package io.github.skunkwerkx.hypertabular;

/**
 * A structural failure: the input is not rows of cells — a record of the wrong width, input
 * that ends inside a quoted cell, a workbook whose container or parts cannot be read. Never a
 * cell's verdict: a value that does not cast is a
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

    /** The failure the core wrote at {@code at} of {@code out}, a {@code Failure} as the ABI lays it out. */
    static TabularException from(java.lang.foreign.MemorySegment out, long at) {
        return new TabularException(
                TabularFailure.of(out.get(java.lang.foreign.ValueLayout.JAVA_INT, at)),
                out.get(java.lang.foreign.ValueLayout.JAVA_LONG, at + 8),
                out.get(java.lang.foreign.ValueLayout.JAVA_INT, at + 4),
                out.get(java.lang.foreign.ValueLayout.JAVA_LONG, at + 16),
                out.get(java.lang.foreign.ValueLayout.JAVA_INT, at + 24),
                out.get(java.lang.foreign.ValueLayout.JAVA_INT, at + 28));
    }

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
     * Delimited text: the zero-based index of the offending record — the header and skipped
     * blank lines included. A workbook: the part being read ({@code 1} {@code _rels/.rels},
     * {@code 2} the workbook, {@code 3} its relationships, {@code 4} the styles, {@code 5} the
     * shared strings, {@code 6} a worksheet, {@code 7} {@code content.xml}, {@code 8}
     * {@code mimetype}, {@code 9} the manifest), or {@code 0} for the container itself.
     *
     * @return the record's index
     */
    public long record() {
        return record;
    }

    /**
     * Delimited text: the one-based line the offending record starts on. A workbook: the
     * sheet row being read, where there is one.
     *
     * @return the line number
     */
    public int line() {
        return line;
    }

    /**
     * Delimited text: the absolute byte offset of the offending record's start — a
     * byte-order mark and everything before the record counted. A workbook: how far into the
     * part's inflated bytes the read had got.
     *
     * @return the byte offset
     */
    public long byteOffset() {
        return byteOffset;
    }

    /**
     * What was expected, for the kinds that say: the first record's cell count for
     * {@link TabularFailure#COLUMN_COUNT}, the shared-string table's length for
     * {@link TabularFailure#SHARED_STRING}; {@code 0} otherwise.
     *
     * @return the expected cell count
     */
    public int expected() {
        return expected;
    }

    /**
     * What was found, for the kinds that say: this record's cell count, the compression
     * method, the shared-string index, or which XML construct the part ends inside;
     * {@code 0} otherwise.
     *
     * @return the cell count found
     */
    public int found() {
        return found;
    }

    private static String describe(
            TabularFailure failure, long record, int line, long byteOffset, int expected, int found) {
        return switch (failure) {
            case COLUMN_COUNT ->
                "Record " + record + " (line " + line + ", byte " + byteOffset + ") has " + found
                        + " cells; the first record had " + expected + ".";
            case UNCLOSED_QUOTE ->
                "The input ended inside a quoted cell in record " + record + " (line " + line + ", byte " + byteOffset
                        + ").";
            case ROW_TOO_LONG ->
                "Record " + record + " (line " + line + ", byte " + byteOffset + ") exceeds the "
                        + DelimitedReader.MAX_ROW_BYTES + "-byte row ceiling.";
            case NOT_A_ZIP -> "The workbook is not a zip file.";
            case CONTAINER -> "The workbook's zip structure is broken.";
            case ENCRYPTED -> "The workbook is encrypted.";
            case METHOD ->
                "Part " + record + " of the workbook is compressed by method " + found
                        + ", which is neither stored nor deflate.";
            case MISSING_PART -> "Part " + record + ", which the workbook cannot be read without, is missing.";
            case XML -> "Part " + record + " of the workbook ends inside an XML construct (byte " + byteOffset + ").";
            case DEFLATE ->
                "Part " + record + " of the workbook is not a whole deflate stream (byte " + byteOffset + ").";
            case NOT_A_WORKBOOK -> "The zip is neither an XLSX nor an ODS workbook.";
            case SHARED_STRING -> "Row " + line + " names shared string " + found + "; the table has " + expected + ".";
            case TOO_LARGE -> "The workbook holds more text than a batch can address.";
        };
    }
}
