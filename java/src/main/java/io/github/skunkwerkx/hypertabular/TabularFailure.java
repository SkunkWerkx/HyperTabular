package io.github.skunkwerkx.hypertabular;

/** Why an input could not be read as rows at all. */
public enum TabularFailure {
    /** The input ended inside a quoted cell. */
    UNCLOSED_QUOTE,
    /** A record's cell count disagrees with the first record's. */
    COLUMN_COUNT,
    /** A single record is larger than {@link DelimitedReader#MAX_ROW_BYTES}. */
    ROW_TOO_LONG
}
