package io.github.skunkwerkx.hypertabular;

/** Why an input could not be read as rows at all. {@link #code()} is the core's failure code. */
public enum TabularFailure {
    /** The input ended inside a quoted cell. */
    UNCLOSED_QUOTE(1),
    /** A record's cell count disagrees with the first record's. */
    COLUMN_COUNT(2),
    /** A single record is larger than {@link DelimitedReader#MAX_ROW_BYTES}. */
    ROW_TOO_LONG(3),
    /** The workbook's container is not a zip file. */
    NOT_A_ZIP(16),
    /** The zip's own structure is broken. */
    CONTAINER(17),
    /** A part of the workbook, or the whole document, is encrypted. */
    ENCRYPTED(18),
    /** A part is compressed by a method that is neither stored nor deflate; {@link TabularException#found()} is the method. */
    METHOD(19),
    /** A part the workbook cannot be read without is missing. */
    MISSING_PART(20),
    /** A part's XML ends inside a construct. */
    XML(21),
    /** A part's bytes are not a deflate stream, or stop before the stream does. */
    DEFLATE(22),
    /** The container is a zip but not an XLSX, ODS or Numbers workbook. */
    NOT_A_WORKBOOK(23),
    /** A cell names a shared string that is not in the table. */
    SHARED_STRING(24),
    /** More text than can be addressed: over 4 GiB in a batch or in the shared strings, or 2 GiB in a cell. */
    TOO_LARGE(25),
    /**
     * A Numbers part is not what the IWA packaging says: {@code found} is 1 for a broken chunk or Snappy block, 2 for a
     * malformed message, 3 for an object a reference names that is not there ({@code expected} is its identifier), 4
     * for cell storage of a version the core does not read ({@code expected} is the version).
     */
    IWA(26);

    private final int code;

    TabularFailure(int code) {
        this.code = code;
    }

    /**
     * The core's code for the failure.
     *
     * @return the code
     */
    public int code() {
        return code;
    }

    /** The failure a code names — the reading the Rust binding gives an unknown one, too. */
    static TabularFailure of(int code) {
        for (TabularFailure failure : values()) {
            if (failure.code == code) {
                return failure;
            }
        }
        return CONTAINER;
    }
}
