package io.github.skunkwerkx.hypertabular;

/** Where a cell's verdict came from: {@link Batch#origin} and {@link Problem#origin()}. */
public enum Origin {
    /** The cell itself was judged. */
    CELL,
    /** The cell's exact bytes are a declared token ({@link Tokens}), so it is empty. */
    TOKEN,
    /** The record stopped short of this column and {@link ShortRows#PAD} filled it in. */
    PADDED,
    /** A workbook's formula with no cached value, so it is empty. */
    UNCACHED;

    private static final Origin[] VALUES = values();

    /** The origin a core's code names: a code this binding does not know is the cell's. */
    static Origin of(int code) {
        return code >= 0 && code < VALUES.length ? VALUES[code] : CELL;
    }
}
