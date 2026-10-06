package io.github.skunkwerkx.hypertabular;

/**
 * How a sheet is read.
 *
 * @param hasHeader whether the first row delivered is the header rather than data
 * @param skipEmptyRows whether rows with no cell in them — and the rows a gap in the row
 *     numbers stands for — are left out rather than delivered as rows of empty cells
 * @param batchRows the most rows a batch holds
 */
public record SheetOptions(boolean hasHeader, boolean skipEmptyRows, int batchRows) {
    /** A header, empty rows skipped, {@value DelimitedReader#DEFAULT_BATCH_ROWS} rows a batch. */
    public static final SheetOptions DEFAULT = new SheetOptions(true, true, DelimitedReader.DEFAULT_BATCH_ROWS);

    /**
     * Checks the batch size.
     *
     * @throws IllegalArgumentException if {@code batchRows} is not positive
     */
    public SheetOptions {
        if (batchRows <= 0) {
            throw new IllegalArgumentException("batchRows must be positive; got " + batchRows);
        }
    }

    /**
     * The same options with or without a header row.
     *
     * @param hasHeader whether the first row is the header
     * @return the options
     */
    public SheetOptions withHeader(boolean hasHeader) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows);
    }

    /**
     * The same options skipping empty rows or delivering them.
     *
     * @param skipEmptyRows whether empty rows are left out
     * @return the options
     */
    public SheetOptions withEmptyRowsSkipped(boolean skipEmptyRows) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows);
    }

    /**
     * The same options with another batch size.
     *
     * @param batchRows the most rows a batch holds
     * @return the options
     */
    public SheetOptions withBatchRows(int batchRows) {
        return new SheetOptions(hasHeader, skipEmptyRows, batchRows);
    }
}
