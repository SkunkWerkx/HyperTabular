package io.github.skunkwerkx.hypertabular;

/** What a record with more cells than the width is. */
public enum LongRows {
    /** A {@link TabularFailure#COLUMN_COUNT} failure: the default. */
    REJECT(0),
    /** Cut to the width, and {@link Batch#faults()} lists it with the width it had. */
    REPORT(1),
    /** Cut to the width, and nothing says so. */
    TRUNCATE(2);

    private final int code;

    LongRows(int code) {
        this.code = code;
    }

    /** The core's number for it. */
    int code() {
        return code;
    }
}
