package io.github.skunkwerkx.hypertabular;

/**
 * What a delimited file's bytes are, declared and never sniffed. Anything but {@link #UTF_8} is
 * decoded ahead of the read, so that everything a batch says, its spans included, is about the
 * decoded text. A unit that has no character is a {@link TabularFailure#ENCODING} failure, after
 * every intact row before it, whose {@link TabularException#byteOffset()} alone counts the
 * original bytes: nothing is ever replaced. A workbook declares its own encoding, so this is
 * delimited text's alone.
 */
public enum Encoding {
    /** Read without a copy: the default. A cell that is not valid is that cell's malformed verdict. */
    UTF_8(0),
    /**
     * As WHATWG has it: the five bytes the code page leaves undefined are their C1 controls.
     * {@code EF BB BF} is three characters here, not a byte-order mark.
     */
    WINDOWS_1252(1),
    /** ISO-8859-1: every byte the code point of its value. */
    LATIN_1(2),
    /**
     * UTF-16, little-endian, with or without a byte-order mark. A lone surrogate, or half a code
     * unit at the end of the input, is the failure.
     */
    UTF_16LE(3);

    private final int code;

    Encoding(int code) {
        this.code = code;
    }

    /** The core's number for it. */
    int code() {
        return code;
    }
}
