package io.github.skunkwerkx.hypertabular;

import java.util.Objects;

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
 * @param skip the physical lines to pass over after a byte-order mark, before anything is read
 *     of them: a quote in one means nothing
 * @param headerRow the records to discard after those, ahead of the header if there is one; a
 *     blank line this dialect skips is not a record
 * @param shortRows what a record with fewer cells than the width is
 * @param longRows what a record with more cells than the width is
 * @param encoding what the bytes are; anything but UTF-8 is decoded ahead of the read
 * @param tokens what stands for no value in every column without tokens of its own
 * @param notes whether a batch keeps what {@link Batch#origin}, {@link Batch#detail} and
 *     {@link Batch#faults()} read; off, a read costs nothing for them and they say nothing
 */
public record Dialect(
        char separator,
        boolean quoting,
        boolean hasHeader,
        boolean skipBlankLines,
        int skip,
        int headerRow,
        ShortRows shortRows,
        LongRows longRows,
        Encoding encoding,
        Tokens tokens,
        boolean notes) {
    /** Comma-separated, quoted, with a header, blank lines skipped. */
    public static final Dialect CSV = new Dialect(',');

    /** Tab-separated, otherwise as {@link #CSV}. */
    public static final Dialect TSV = new Dialect('\t');

    /** Pipe-separated, otherwise as {@link #CSV}. */
    public static final Dialect PSV = new Dialect('|');

    /**
     * Checks the declarations.
     *
     * @throws IllegalArgumentException if {@code skip} or {@code headerRow} is negative
     */
    public Dialect {
        if (skip < 0) {
            throw new IllegalArgumentException("skip must not be negative; got " + skip);
        }
        if (headerRow < 0) {
            throw new IllegalArgumentException("headerRow must not be negative; got " + headerRow);
        }
        Objects.requireNonNull(shortRows, "shortRows");
        Objects.requireNonNull(longRows, "longRows");
        Objects.requireNonNull(encoding, "encoding");
        Objects.requireNonNull(tokens, "tokens");
    }

    /**
     * A dialect of these four, with everything else as {@link #CSV} has it: no line skipped, no
     * record discarded, a row of the wrong width refused, UTF-8, no tokens and notes kept.
     *
     * @param separator the single-byte separator
     * @param quoting whether {@code "} quotes cells
     * @param hasHeader whether the first record is a header
     * @param skipBlankLines whether a completely empty line is skipped
     */
    public Dialect(char separator, boolean quoting, boolean hasHeader, boolean skipBlankLines) {
        this(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                0,
                0,
                ShortRows.REJECT,
                LongRows.REJECT,
                Encoding.UTF_8,
                Tokens.NONE,
                true);
    }

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
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect with quoting on or off.
     *
     * @param quoting whether {@code "} quotes cells
     * @return the changed dialect
     */
    public Dialect withQuoting(boolean quoting) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect with or without a header record.
     *
     * @param hasHeader whether the first record is a header
     * @return the changed dialect
     */
    public Dialect withHeader(boolean hasHeader) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect skipping blank lines or reading them as one-cell rows.
     *
     * @param skipBlankLines whether a completely empty line is skipped
     * @return the changed dialect
     */
    public Dialect withSkipBlankLines(boolean skipBlankLines) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect passing over physical lines first.
     *
     * @param skip the lines to pass over
     * @return the changed dialect
     */
    public Dialect withSkip(int skip) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect discarding records ahead of the header.
     *
     * @param headerRow the records to discard
     * @return the changed dialect
     */
    public Dialect withHeaderRow(int headerRow) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect with another answer to a short record.
     *
     * @param shortRows what a record with fewer cells than the width is
     * @return the changed dialect
     */
    public Dialect withShortRows(ShortRows shortRows) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect with another answer to a long record.
     *
     * @param longRows what a record with more cells than the width is
     * @return the changed dialect
     */
    public Dialect withLongRows(LongRows longRows) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect reading another encoding.
     *
     * @param encoding what the bytes are
     * @return the changed dialect
     */
    public Dialect withEncoding(Encoding encoding) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect with another list of what stands for no value.
     *
     * @param tokens the tokens
     * @return the changed dialect
     */
    public Dialect withTokens(Tokens tokens) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }

    /**
     * This dialect with another list of what stands for no value.
     *
     * @param tokens the tokens, matched as their exact UTF-8 bytes
     * @return the changed dialect
     */
    public Dialect withTokens(String... tokens) {
        return withTokens(Tokens.of(tokens));
    }

    /**
     * This dialect keeping what a batch says of why, or not.
     *
     * @param notes whether a batch keeps it
     * @return the changed dialect
     */
    public Dialect withNotes(boolean notes) {
        return new Dialect(
                separator,
                quoting,
                hasHeader,
                skipBlankLines,
                skip,
                headerRow,
                shortRows,
                longRows,
                encoding,
                tokens,
                notes);
    }
}
