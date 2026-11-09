package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.NumFormat;
import java.util.HashMap;
import java.util.Map;
import java.util.Objects;

/**
 * How a sample is taken and read: what {@link DelimitedReader#suggest} and {@link Sheet#suggest}
 * are told. Immutable. {@link #DEFAULT} takes 1,000 rows with nothing declared.
 */
public final class SuggestOptions {
    /** 1,000 rows, nothing declared. */
    public static final SuggestOptions DEFAULT = new SuggestOptions(1000, null, Map.of(), null, Map.of());

    private final int sampleRows;
    private final NumFormat format;
    private final Map<Integer, NumFormat> columnFormats;
    private final DateOrder dateOrder;
    private final Map<Integer, DateOrder> columnDateOrders;

    private SuggestOptions(
            int sampleRows,
            NumFormat format,
            Map<Integer, NumFormat> columnFormats,
            DateOrder dateOrder,
            Map<Integer, DateOrder> columnDateOrders) {
        this.sampleRows = sampleRows;
        this.format = format;
        this.columnFormats = columnFormats;
        this.dateOrder = dateOrder;
        this.columnDateOrders = columnDateOrders;
    }

    /**
     * These options with another sample size. It stops at {@link DelimitedReader#DEFAULT_BATCH_ROWS} of delimited
     * text, and at a sheet's own batch size, read in as many batches as the core says fit: a sample is
     * never refused for the size of its batch.
     *
     * @param sampleRows the most rows the sample holds, and always at least one: 0 is 1
     * @return the options
     * @throws IllegalArgumentException if {@code sampleRows} is negative
     */
    public SuggestOptions withSampleRows(int sampleRows) {
        if (sampleRows < 0) {
            throw new IllegalArgumentException("sampleRows must not be negative; got " + sampleRows);
        }
        return new SuggestOptions(sampleRows, format, columnFormats, dateOrder, columnDateOrders);
    }

    /**
     * These options reading every column's numbers in this notation, which settles what {@code ,}
     * means in them.
     *
     * @param format the notation
     * @return the options
     */
    public SuggestOptions withFormat(NumFormat format) {
        Objects.requireNonNull(format, "format");
        return new SuggestOptions(sampleRows, format, columnFormats, dateOrder, columnDateOrders);
    }

    /**
     * These options reading the numbers of a source column in a notation of its own, ahead of
     * {@link #withFormat}.
     *
     * @param ordinal the source column
     * @param format the notation
     * @return the options
     * @throws IllegalArgumentException if {@code ordinal} is negative
     */
    public SuggestOptions withColumnFormat(int ordinal, NumFormat format) {
        requireOrdinal(ordinal);
        Objects.requireNonNull(format, "format");
        Map<Integer, NumFormat> formats = new HashMap<>(columnFormats);
        formats.put(ordinal, format);
        return new SuggestOptions(sampleRows, this.format, Map.copyOf(formats), dateOrder, columnDateOrders);
    }

    /**
     * These options reading every column's dates in this order where the sample leaves the order
     * open, as {@code 01/02/2026} does. A column whose cells rule that order out keeps the order
     * they leave: it answers what the sample cannot, and does not overrule what it can.
     *
     * @param order the order
     * @return the options
     */
    public SuggestOptions withDateOrder(DateOrder order) {
        Objects.requireNonNull(order, "order");
        return new SuggestOptions(sampleRows, format, columnFormats, order, columnDateOrders);
    }

    /**
     * These options reading the dates of a source column in an order of its own, ahead of
     * {@link #withDateOrder}, and as that does.
     *
     * @param ordinal the source column
     * @param order the order
     * @return the options
     * @throws IllegalArgumentException if {@code ordinal} is negative
     */
    public SuggestOptions withColumnDateOrder(int ordinal, DateOrder order) {
        requireOrdinal(ordinal);
        Objects.requireNonNull(order, "order");
        Map<Integer, DateOrder> orders = new HashMap<>(columnDateOrders);
        orders.put(ordinal, order);
        return new SuggestOptions(sampleRows, format, columnFormats, dateOrder, Map.copyOf(orders));
    }

    /**
     * The most rows the sample holds.
     *
     * @return the rows; 0 takes one
     */
    public int sampleRows() {
        return sampleRows;
    }

    /** The notation declared for a source column, or {@code null}. */
    NumFormat declaredFormat(int ordinal) {
        NumFormat own = columnFormats.get(ordinal);
        return own != null ? own : format;
    }

    /** The order declared for a source column, or {@code null}. */
    DateOrder declaredOrder(int ordinal) {
        DateOrder own = columnDateOrders.get(ordinal);
        return own != null ? own : dateOrder;
    }

    private static void requireOrdinal(int ordinal) {
        if (ordinal < 0) {
            throw new IllegalArgumentException("A column's ordinal must not be negative; got " + ordinal);
        }
    }

    /** How many rows to take, where a batch is {@code batchRows}. */
    int rowsWithin(int batchRows) {
        return Math.min(Math.max(sampleRows, 1), Math.max(batchRows, 1));
    }
}
