package io.github.skunkwerkx.hypertabular;

/** One sheet of a workbook, as its listing names it. */
public final class SheetInfo {
    private final String name;
    private final boolean hidden;
    private final byte[] part;
    private final int index;

    SheetInfo(String name, boolean hidden, byte[] part, int index) {
        this.name = name;
        this.hidden = hidden;
        this.part = part;
        this.index = index;
    }

    /**
     * The sheet's name. (Bytes that are not UTF-8 are replaced.)
     *
     * @return the name
     */
    public String name() {
        return name;
    }

    /**
     * Whether the workbook hides the sheet. A hidden sheet reads like any other.
     *
     * @return {@code true} for a hidden sheet
     */
    public boolean hidden() {
        return hidden;
    }

    /** The XLSX part that holds the sheet: what the core is given to position on it. */
    byte[] part() {
        return part;
    }

    /** The sheet's place among an ODS document's tables. */
    int index() {
        return index;
    }

    @Override
    public String toString() {
        return hidden ? name + " (hidden)" : name;
    }
}
