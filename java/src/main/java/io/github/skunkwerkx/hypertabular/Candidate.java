package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.NumFormat;

/** A declaration that would settle a {@link SuggestStatus#AMBIGUOUS} column. */
public sealed interface Candidate {
    /**
     * The order a date's fields come in.
     *
     * @param order the order
     */
    record Order(DateOrder order) implements Candidate {}

    /**
     * The notation a number is in. Declare it with {@link SuggestOptions#withColumnFormat} and
     * ask again: the door can differ under each, as {@code 1,234} is an {@code int} under one and
     * a {@code double} under the other.
     *
     * @param format the notation
     */
    record Notation(NumFormat format) implements Candidate {}
}
