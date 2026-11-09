package io.github.skunkwerkx.hypertabular;

/** What a record with fewer cells than the width is. */
public enum ShortRows {
    /** A {@link TabularFailure#COLUMN_COUNT} failure: the default. */
    REJECT(0),
    /** A row whose missing cells are empty, at {@link Origin#PADDED}. */
    PAD(1);

    private final int code;

    ShortRows(int code) {
        this.code = code;
    }

    /** The core's number for it. */
    int code() {
        return code;
    }
}
