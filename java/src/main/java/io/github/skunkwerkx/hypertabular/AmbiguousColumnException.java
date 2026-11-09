package io.github.skunkwerkx.hypertabular;

import java.util.NoSuchElementException;

/**
 * What {@link Header#ordinal(String)} says of a name two columns of the header share: it will not
 * pick one for you. Say which by position, or ask {@link Header#findOrdinal(String)}, which keeps
 * the first and says so.
 */
public final class AmbiguousColumnException extends NoSuchElementException {
    private static final long serialVersionUID = 1L;

    /** The name two columns share. */
    private final String name;
    /** The first column with the name. */
    private final int first;
    /** The second column with the name. */
    private final int second;

    AmbiguousColumnException(String name, int first, int second) {
        super("The header names two columns \"" + name + "\", at positions " + first + " and " + second
                + ": say which by position.");
        this.name = name;
        this.first = first;
        this.second = second;
    }

    /**
     * The name two columns share.
     *
     * @return the name
     */
    public String name() {
        return name;
    }

    /**
     * The first column with the name.
     *
     * @return its ordinal
     */
    public int first() {
        return first;
    }

    /**
     * The second column with the name.
     *
     * @return its ordinal
     */
    public int second() {
        return second;
    }
}
