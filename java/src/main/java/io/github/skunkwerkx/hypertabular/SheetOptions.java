package io.github.skunkwerkx.hypertabular;

import java.util.Objects;

/**
 * How a sheet is read.
 *
 * @param hasHeader whether the first row delivered is the header rather than data
 * @param skipEmptyRows whether rows with no cell in them — and the rows a gap in the row
 *     numbers stands for — are left out rather than delivered as rows of empty cells; a row
 *     holding only formulas with no cached value is empty here too, so turn this off to keep
 *     every row
 * @param batchRows the most rows a batch holds; opening a sheet refuses {@code 0} and a batch
 *     asking for more than the core allows
 * @param skip the rows of the sheet to pass over at the start of the range, empty ones included;
 *     their cells are not judged, so a bad cell there does not matter
 * @param headerRow the rows to read and discard after those, as {@code skipEmptyRows} delivers
 *     them, so with it on an empty row does not count
 * @param range the part of the sheet to read
 * @param tokens what stands for no value in every column without tokens of its own: text cells
 *     alone can be one
 * @param notes whether a batch keeps what {@link Batch#origin}, {@link Batch#detail} and
 *     {@link Batch#faults()} read
 */
public record SheetOptions(
        boolean hasHeader,
        boolean skipEmptyRows,
        int batchRows,
        int skip,
        int headerRow,
        Range range,
        Tokens tokens,
        boolean notes) {
    /** A header, empty rows skipped, {@value DelimitedReader#DEFAULT_BATCH_ROWS} rows a batch. */
    public static final SheetOptions DEFAULT = new SheetOptions(true, true, DelimitedReader.DEFAULT_BATCH_ROWS);

    /**
     * Checks the declarations.
     *
     * @throws IllegalArgumentException if {@code skip} or {@code headerRow} is negative
     */
    public SheetOptions {
        if (skip < 0) {
            throw new IllegalArgumentException("skip must not be negative; got " + skip);
        }
        if (headerRow < 0) {
            throw new IllegalArgumentException("headerRow must not be negative; got " + headerRow);
        }
        Objects.requireNonNull(range, "range");
        Objects.requireNonNull(tokens, "tokens");
    }

    /**
     * Options of these three, with everything else as {@link #DEFAULT} has it.
     *
     * @param hasHeader whether the first row is the header
     * @param skipEmptyRows whether empty rows are left out
     * @param batchRows the most rows a batch holds
     */
    public SheetOptions(boolean hasHeader, boolean skipEmptyRows, int batchRows) {
        this(hasHeader, skipEmptyRows, batchRows, 0, 0, Range.ALL, Tokens.NONE, true);
    }

    /**
     * The same options with or without a header row.
     *
     * @param hasHeader whether the first row is the header
     * @return the options
     */
    public SheetOptions withHeader(boolean hasHeader) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows, skip, headerRow, range, tokens, notes);
    }

    /**
     * The same options skipping empty rows or delivering them.
     *
     * @param skipEmptyRows whether empty rows are left out
     * @return the options
     */
    public SheetOptions withEmptyRowsSkipped(boolean skipEmptyRows) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows, skip, headerRow, range, tokens, notes);
    }

    /**
     * The same options with another batch size.
     *
     * @param batchRows the most rows a batch holds
     * @return the options
     */
    public SheetOptions withBatchRows(int batchRows) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows, skip, headerRow, range, tokens, notes);
    }

    /**
     * The same options passing over rows first.
     *
     * @param skip the rows to pass over
     * @return the options
     */
    public SheetOptions withSkip(int skip) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows, skip, headerRow, range, tokens, notes);
    }

    /**
     * The same options discarding rows ahead of the header.
     *
     * @param headerRow the rows to discard
     * @return the options
     */
    public SheetOptions withHeaderRow(int headerRow) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows, skip, headerRow, range, tokens, notes);
    }

    /**
     * The same options reading part of the sheet.
     *
     * @param range the part to read
     * @return the options
     */
    public SheetOptions withRange(Range range) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows, skip, headerRow, range, tokens, notes);
    }

    /**
     * The same options with another list of what stands for no value.
     *
     * @param tokens the tokens
     * @return the options
     */
    public SheetOptions withTokens(Tokens tokens) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows, skip, headerRow, range, tokens, notes);
    }

    /**
     * The same options with another list of what stands for no value.
     *
     * @param tokens the tokens, matched as their exact UTF-8 bytes
     * @return the options
     */
    public SheetOptions withTokens(String... tokens) {
        return withTokens(Tokens.of(tokens));
    }

    /**
     * The same options keeping what a batch says of why, or not.
     *
     * @param notes whether a batch keeps it
     * @return the options
     */
    public SheetOptions withNotes(boolean notes) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows, skip, headerRow, range, tokens, notes);
    }

    /** What a plan's ordinal is short of the sheet's own column. */
    int firstColumn() {
        return Math.max(range.firstColumn(), 1) - 1;
    }
}
