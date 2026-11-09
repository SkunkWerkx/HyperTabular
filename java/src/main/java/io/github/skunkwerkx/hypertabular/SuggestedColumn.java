package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.NumFormat;
import java.util.List;

/**
 * What a sample says of one source column.
 *
 * @param ordinal the zero-based source column, as a {@link Column} names it
 * @param name the header's name for it, or {@code null} if the source has no header
 * @param door the door found: {@link Door#TEXT} when unresolved, and the first candidate's when
 *     ambiguous
 * @param param what {@code door} declares, as the core numbers it: a date's order, the first
 *     candidate's when ambiguous, a Unix precision or an Excel epoch; 0 for another door
 * @param format the notation the plan reads a resolved numeric column in: the one declared for
 *     it, or else the one candidate the sample left, comma decimal where only that reads the
 *     column, and otherwise {@code ,} grouping in threes, whether or not the sample held a
 *     {@code ,}. Both are Western, so a later {@code 1,23,456} is refused. {@code null} for any
 *     other column
 * @param candidates what to choose between: empty unless ambiguous
 * @param status how far the sample settles it
 * @param matched the cells {@code door} took: resolved or ambiguous, that is all of
 *     {@code sampled}; unresolved, the most that any one door but the text door took
 * @param sampled the cells that are not blank
 */
public record SuggestedColumn(
        int ordinal,
        String name,
        Door door,
        int param,
        NumFormat format,
        List<Candidate> candidates,
        SuggestStatus status,
        int matched,
        int sampled) {}
