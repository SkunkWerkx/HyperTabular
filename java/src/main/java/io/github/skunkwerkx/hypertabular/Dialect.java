package io.github.skunkwerkx.hypertabular;

/**
 * How the text is delimited, declared by the caller. Nothing is sniffed: the separator is
 * stated, quoting is stated, the header is stated — the same stance HyperCast's
 * {@code NumFormat} takes for numeric notation.
 *
 * @param separator the single-byte separator: tab, or any printable ASCII character except
 *     {@code "}
 * @param quoting whether {@code "} quotes cells (RFC 4180, {@code ""} for a literal quote);
 *     off, a quote is an ordinary byte
 * @param hasHeader whether the first record is a header, exposed through
 *     {@link DelimitedReader#header()} and never delivered as a row
 * @param skipBlankLines whether a completely empty line is skipped rather than read as a
 *     one-cell row
 */
public record Dialect(char separator, boolean quoting, boolean hasHeader, boolean skipBlankLines) {
    /** Comma-separated, quoted, with a header, blank lines skipped. */
    public static final Dialect CSV = new Dialect(',');

    /** Tab-separated, otherwise as {@link #CSV}. */
    public static final Dialect TSV = new Dialect('\t');

    /** Pipe-separated, otherwise as {@link #CSV}. */
    public static final Dialect PSV = new Dialect('|');

    /**
     * A dialect with the given separator: quoted, with a header, blank lines skipped.
     *
     * @param separator the single-byte separator
     */
    public Dialect(char separator) {
        this(separator, true, true, true);
    }

    /**
     * This dialect with another separator.
     *
     * @param separator the single-byte separator
     * @return the changed dialect
     */
    public Dialect withSeparator(char separator) {
        return new Dialect(separator, quoting, hasHeader, skipBlankLines);
    }

    /**
     * This dialect with quoting on or off.
     *
     * @param quoting whether {@code "} quotes cells
     * @return the changed dialect
     */
    public Dialect withQuoting(boolean quoting) {
        return new Dialect(separator, quoting, hasHeader, skipBlankLines);
    }

    /**
     * This dialect with or without a header record.
     *
     * @param hasHeader whether the first record is a header
     * @return the changed dialect
     */
    public Dialect withHeader(boolean hasHeader) {
        return new Dialect(separator, quoting, hasHeader, skipBlankLines);
    }

    /**
     * This dialect skipping blank lines or reading them as one-cell rows.
     *
     * @param skipBlankLines whether a completely empty line is skipped
     * @return the changed dialect
     */
    public Dialect withSkipBlankLines(boolean skipBlankLines) {
        return new Dialect(separator, quoting, hasHeader, skipBlankLines);
    }
}
