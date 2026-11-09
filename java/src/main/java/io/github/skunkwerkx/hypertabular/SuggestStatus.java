package io.github.skunkwerkx.hypertabular;

/** How far a sample settles a column. */
public enum SuggestStatus {
    /** One door and one declaration take every cell, and the plan reads the column through them. */
    RESOLVED,
    /**
     * Every cell is read, but the sample cannot say which declaration it is written in:
     * {@link SuggestedColumn#candidates()} are the choices. The plan reads the column as text
     * until the caller chooses.
     */
    AMBIGUOUS,
    /**
     * There is nothing to go on, or a cell no door took beside cells one did, or a cell that is
     * not text at all. The plan reads the column as text.
     */
    UNRESOLVED;

    /** The status a core's code names: a code this binding does not know settles nothing. */
    static SuggestStatus of(int code) {
        return switch (code) {
            case 1 -> RESOLVED;
            case 2 -> AMBIGUOUS;
            default -> UNRESOLVED;
        };
    }
}
