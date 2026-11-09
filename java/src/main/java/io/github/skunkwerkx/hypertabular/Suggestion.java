package io.github.skunkwerkx.hypertabular;

import java.util.List;

/**
 * What a suggest found: a plan to change or to hand to a read, and why. It is never applied.
 *
 * @param plan a column for each source column, in order; only a resolved column reads through its
 *     door, and the rest read as text, which refuses nothing and loses nothing
 * @param report a report for each source column, in the plan's order
 * @param truncated whether the source has rows past the sample; a source with exactly the
 *     sample's rows has not
 */
public record Suggestion(List<Column> plan, List<SuggestedColumn> report, boolean truncated) {}
