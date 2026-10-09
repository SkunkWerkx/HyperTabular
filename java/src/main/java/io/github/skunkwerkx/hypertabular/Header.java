package io.github.skunkwerkx.hypertabular;

import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.charset.StandardCharsets;
import java.util.AbstractList;
import java.util.Arrays;
import java.util.NoSuchElementException;
import java.util.Objects;
import java.util.OptionalInt;
import java.util.RandomAccess;

/**
 * A header record's names, in source order — the unmodifiable list of strings it always was,
 * and the way from a column's name to the ordinal a {@link Column} is built with.
 *
 * {@snippet :
 * try (DelimitedReader reader = DelimitedReader.open(Path.of("orders.csv"), Dialect.CSV)) {
 *     Header header = reader.header();
 *     reader.bind(List.of(Column.i32(header.ordinal("id")), Column.text(header.ordinal("name"))));
 *     // read as ever
 * }
 * }
 *
 * <p>A name is matched exactly: case-sensitive and untrimmed, as the Rust core's
 * {@code Header::ordinal} matches it. When two columns share a name, the first is the one
 * found.
 *
 * <p>The names are decoded from UTF-8 once, when the header is read (bytes that are not UTF-8
 * are replaced, as {@link String#String(byte[], java.nio.charset.Charset)} replaces them), and
 * the bytes they were decoded from are kept, so that {@link #ordinal(MemorySegment)} matches
 * what the source held rather than what it decoded to.
 *
 * <p>A header is a value of its own, in the Java heap: it outlives the reader that read it.
 */
public final class Header extends AbstractList<String> implements RandomAccess {
    /** A header with no names: the header of an input that had no record. */
    static final Header EMPTY = new Header(new String[0], new byte[0], new int[0]);

    private final String[] names;
    /** Every name's bytes, end to end; name {@code i} ends at {@code ends[i]}. */
    private final byte[] utf8;

    private final int[] ends;

    private Header(String[] names, byte[] utf8, int[] ends) {
        this.names = names;
        this.utf8 = utf8;
        this.ends = ends;
    }

    /** Collects a header's names as the core hands them out, then makes the header. */
    static final class Builder {
        private final String[] names;
        private final int[] ends;
        private byte[] utf8 = new byte[64];
        private int count;
        private int length;

        Builder(int count) {
            names = new String[count];
            ends = new int[count];
        }

        /** Adds the next name's bytes. */
        void add(MemorySegment name) {
            byte[] bytes = name.toArray(ValueLayout.JAVA_BYTE);
            if (utf8.length - length < bytes.length) {
                utf8 = Arrays.copyOf(utf8, Math.max(utf8.length * 2, length + bytes.length));
            }
            System.arraycopy(bytes, 0, utf8, length, bytes.length);
            names[count] = new String(bytes, StandardCharsets.UTF_8);
            length += bytes.length;
            ends[count++] = length;
        }

        /** The header of every name added. */
        Header build() {
            return count == 0 ? EMPTY : new Header(names, Arrays.copyOf(utf8, length), ends);
        }
    }

    /**
     * The name of the column at {@code ordinal}.
     *
     * @param ordinal the column's place in the record, from 0
     * @return its name
     * @throws IndexOutOfBoundsException if the header has no column there
     */
    @Override
    public String get(int ordinal) {
        return names[Objects.checkIndex(ordinal, names.length)];
    }

    /**
     * How many names the header has.
     *
     * @return the count
     */
    @Override
    public int size() {
        return names.length;
    }

    /**
     * The bytes the name at {@code ordinal} was decoded from, as a read-only view.
     *
     * @param ordinal the column's place in the record, from 0
     * @return the name's UTF-8 bytes
     * @throws IndexOutOfBoundsException if the header has no column there
     */
    public MemorySegment utf8(int ordinal) {
        Objects.checkIndex(ordinal, names.length);
        int start = ordinal == 0 ? 0 : ends[ordinal - 1];
        return MemorySegment.ofArray(utf8).asSlice(start, ends[ordinal] - start).asReadOnly();
    }

    /**
     * The ordinal of the first column named exactly {@code name}.
     *
     * @param name the column's name
     * @return its ordinal
     * @throws NoSuchElementException if no column has that name; the message names it
     */
    public int ordinal(String name) {
        int found = indexOf(Objects.requireNonNull(name, "name"));
        if (found < 0) {
            throw missing(name);
        }
        return found;
    }

    /**
     * The ordinal of the first column named exactly {@code name}, if there is one.
     *
     * @param name the column's name
     * @return its ordinal, or empty if no column has that name
     */
    public OptionalInt findOrdinal(String name) {
        int found = indexOf(Objects.requireNonNull(name, "name"));
        return found < 0 ? OptionalInt.empty() : OptionalInt.of(found);
    }

    /**
     * The ordinal of the first column whose name is exactly the bytes {@code utf8Name} — the
     * Rust core's {@code Header::ordinal}, byte for byte.
     *
     * @param utf8Name the column's name, as UTF-8
     * @return its ordinal
     * @throws NoSuchElementException if no column has that name; the message names it
     */
    public int ordinal(MemorySegment utf8Name) {
        int found = indexOf(utf8Name);
        if (found < 0) {
            throw missing(new String(utf8Name.toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8));
        }
        return found;
    }

    /**
     * The ordinal of the first column whose name is exactly the bytes {@code utf8Name}, if
     * there is one.
     *
     * @param utf8Name the column's name, as UTF-8
     * @return its ordinal, or empty if no column has that name
     */
    public OptionalInt findOrdinal(MemorySegment utf8Name) {
        int found = indexOf(utf8Name);
        return found < 0 ? OptionalInt.empty() : OptionalInt.of(found);
    }

    /**
     * {@link #ordinal(MemorySegment)} of a byte array.
     *
     * @param utf8Name the column's name, as UTF-8
     * @return its ordinal
     * @throws NoSuchElementException if no column has that name; the message names it
     */
    public int ordinal(byte[] utf8Name) {
        return ordinal(MemorySegment.ofArray(Objects.requireNonNull(utf8Name, "utf8Name")));
    }

    /**
     * {@link #findOrdinal(MemorySegment)} of a byte array.
     *
     * @param utf8Name the column's name, as UTF-8
     * @return its ordinal, or empty if no column has that name
     */
    public OptionalInt findOrdinal(byte[] utf8Name) {
        return findOrdinal(MemorySegment.ofArray(Objects.requireNonNull(utf8Name, "utf8Name")));
    }

    /**
     * The ordinal of the first column named {@code name}, or {@code -1}: {@link java.util.List#indexOf},
     * which for a header is exactly {@link #findOrdinal(String)}.
     *
     * @param name the name looked for
     * @return its first ordinal, or {@code -1}
     */
    @Override
    public int indexOf(Object name) {
        for (int index = 0; index < names.length; index++) {
            if (names[index].equals(name)) {
                return index;
            }
        }
        return -1;
    }

    private int indexOf(MemorySegment utf8Name) {
        Objects.requireNonNull(utf8Name, "utf8Name");
        MemorySegment all = MemorySegment.ofArray(utf8);
        int start = 0;
        for (int index = 0; index < ends.length; index++) {
            int end = ends[index];
            if (end - start == utf8Name.byteSize()
                    && all.asSlice(start, end - start).mismatch(utf8Name) == -1) {
                return index;
            }
            start = end;
        }
        return -1;
    }

    private static NoSuchElementException missing(String name) {
        return new NoSuchElementException("The header has no column named \"" + name + "\".");
    }
}
