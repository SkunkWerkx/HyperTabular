package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.ExcelEpoch;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.channels.FileChannel;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.List;
import java.util.NoSuchElementException;
import java.util.Objects;

/**
 * A workbook — XLSX or ODS — opened for reading through a plan into the batches delimited
 * text is read into.
 *
 * <p>A workbook holds its container's bytes and what every sheet reads its cells against: the
 * shared strings and the number-format kind of each cell format, loaded once when it is
 * opened. {@link #sheet(int, SheetOptions, List)} starts a forward-only read of one sheet —
 * or {@link #sheet(int, SheetOptions)}, which reads its header first and leaves the plan to
 * {@link Sheet#bind};
 * several can be open at once, each with its own buffers, all reading from the one workbook,
 * which has to outlive them.
 *
 * <p>The core owns no memory, as for delimited text: every buffer it works in is this
 * binding's native memory, and when a call says one is too small it is grown — keeping what
 * it held, since a workbook part is a compressed stream the read cannot back up through — and
 * the call made again.
 *
 * {@snippet :
 * try (Workbook book = Workbook.open(Path.of("orders.xlsx"));
 *         Sheet sheet = book.sheet("Orders", SheetOptions.DEFAULT)) {
 *     Header header = sheet.header();
 *     sheet.bind(List.of(Column.i64(header.ordinal("Id")), Column.decimal(header.ordinal("Total"))));
 *     for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
 *         // the same Batch delimited text is read into
 *     }
 * }
 * }
 *
 * <p>Not thread-safe, and confined: the memory is a confined {@link Arena}'s, so a workbook
 * and its sheets belong to the thread that opened it.
 */
public final class Workbook implements AutoCloseable {
    /**
     * For the tests: every buffer a workbook call works in starts with room for one element
     * and no shared-strings bound is asked for, so every call that can stop and resume does —
     * the grow-and-keep path exercised mid-part.
     */
    static boolean stingy;

    /** For the tests: how many times the window, the arena and the cell table were grown. */
    static final long[] grown = new long[3];

    static final String CONTRACT_VIOLATION =
            "libhypertabular reported a contract violation — a binding bug, please report it";

    private final Arena arena;
    private final MemorySegment container;
    /** The memory a stream was read into, when the workbook was opened from one: closed with it. */
    private final Block read;
    /** The state as opening left it: the template every sheet's own state is copied from. */
    private MemorySegment state;

    private WorkbookFormat format;
    private ExcelEpoch dateSystem;
    private List<SheetInfo> sheets;
    private MemorySegment strings;
    private MemorySegment table;
    private MemorySegment kinds;
    private boolean closed;

    /**
     * Opens the file at {@code path}, mapped into memory rather than read: the container is
     * read in place, and nothing of it is copied.
     *
     * @param path the workbook
     * @return the workbook, its sheets listed and its shared strings and styles loaded
     * @throws TabularException if the file is not a workbook this reader can read
     * @throws UncheckedIOException if the file cannot be opened or mapped
     */
    public static Workbook open(Path path) {
        Objects.requireNonNull(path, "path");
        Arena arena = Arena.ofConfined();
        try (FileChannel channel = FileChannel.open(path, StandardOpenOption.READ)) {
            MemorySegment mapped = channel.map(FileChannel.MapMode.READ_ONLY, 0, channel.size(), arena);
            return new Workbook(arena, mapped);
        } catch (IOException e) {
            arena.close();
            throw new UncheckedIOException(e);
        } catch (RuntimeException | Error failure) {
            arena.close();
            throw failure;
        }
    }

    /**
     * Opens a workbook held in a byte array, copied once into native memory.
     *
     * @param bytes the workbook's bytes
     * @return the workbook
     * @throws TabularException if the bytes are not a workbook this reader can read
     */
    public static Workbook of(byte[] bytes) {
        Objects.requireNonNull(bytes, "bytes");
        Arena arena = Arena.ofConfined();
        try {
            MemorySegment copy = arena.allocate(Math.max(bytes.length, 1));
            MemorySegment.copy(bytes, 0, copy, ValueLayout.JAVA_BYTE, 0, bytes.length);
            return new Workbook(arena, copy.asSlice(0, bytes.length));
        } catch (RuntimeException | Error failure) {
            arena.close();
            throw failure;
        }
    }

    /**
     * Opens a workbook held in a memory segment. A native segment is read in place, and must
     * stay alive and unchanged for as long as the workbook is open; a heap segment is copied
     * once.
     *
     * @param bytes the workbook's bytes
     * @return the workbook
     * @throws TabularException if the bytes are not a workbook this reader can read
     */
    public static Workbook of(MemorySegment bytes) {
        Objects.requireNonNull(bytes, "bytes");
        Arena arena = Arena.ofConfined();
        try {
            MemorySegment container = bytes;
            if (!bytes.isNative()) {
                container = arena.allocate(Math.max(bytes.byteSize(), 1)).asSlice(0, bytes.byteSize());
                container.copyFrom(bytes);
            }
            return new Workbook(arena, container);
        } catch (RuntimeException | Error failure) {
            arena.close();
            throw failure;
        }
    }

    /**
     * Opens a workbook from a stream, read to its end into native memory the workbook owns.
     * The stream is the workbook's to close, as a stream handed to
     * {@link DelimitedReader#of(InputStream, Dialect, List)} is the reader's: it is closed as
     * soon as it has been read, and when this method throws. The format is told from the
     * bytes, as for every other way of opening one.
     *
     * @param container the workbook's bytes
     * @return the workbook
     * @throws TabularException if the bytes are not a workbook this reader can read
     * @throws UncheckedIOException if the stream fails, or fails to close
     */
    public static Workbook of(InputStream container) {
        Objects.requireNonNull(container, "container");
        Block bytes = null;
        long length = 0;
        try (container) {
            // What the stream says it has is only a hint for the first allocation; the
            // memory doubles, keeping what it holds, whenever the stream has more.
            bytes = new Block(Math.max(container.available(), 64 * 1024) + 1L, 8);
            byte[] transfer = new byte[64 * 1024];
            for (int got = container.read(transfer); got >= 0; got = container.read(transfer)) {
                if (bytes.bytes() - length < got) {
                    bytes.resize(Math.max(bytes.bytes() * 2, length + got), length);
                }
                MemorySegment.copy(transfer, 0, bytes.segment, ValueLayout.JAVA_BYTE, length, got);
                length += got;
            }
        } catch (IOException e) {
            if (bytes != null) {
                bytes.close();
            }
            throw new UncheckedIOException(e);
        } catch (RuntimeException | Error failure) {
            if (bytes != null) {
                bytes.close();
            }
            throw failure;
        }
        Arena arena = Arena.ofConfined();
        try {
            return new Workbook(arena, bytes.segment.asSlice(0, length), bytes);
        } catch (RuntimeException | Error failure) {
            arena.close();
            bytes.close();
            throw failure;
        }
    }

    private Workbook(Arena arena, MemorySegment container) {
        this(arena, container, null);
    }

    private Workbook(Arena arena, MemorySegment container, Block read) {
        this.arena = arena;
        this.container = container;
        this.read = read;
        state = arena.allocate(Native.workbookStateSize(), 8);
        Scratch scratch = stingy ? new Scratch(arena, 1, 1, 1, 0) : new Scratch(arena, Native.WINDOW_MIN, 1024, 64, 0);
        try {
            MemorySegment opened = open(scratch);
            format = opened.get(ValueLayout.JAVA_INT, Native.OPENED_FORMAT) == 1
                    ? WorkbookFormat.XLSX
                    : WorkbookFormat.ODS;
            dateSystem = ExcelEpoch.fromCode(opened.get(ValueLayout.JAVA_INT, Native.OPENED_EPOCH))
                    .orElseThrow(() -> new IllegalStateException(CONTRACT_VIOLATION));

            MemorySegment filled = settle(scratch.drive(this, state, Native.Book.SHEETS));
            int count = (int) filled.get(ValueLayout.JAVA_LONG, Native.FILLED_ROWS);
            SheetInfo[] listed = new SheetInfo[count];
            for (int index = 0; index < count; index++) {
                long at = index * 3L * Native.SPAN_BYTES;
                MemorySegment cells = scratch.cells.segment;
                byte[] name = scratch.arenaBytes(
                        cells.get(ValueLayout.JAVA_INT, at), cells.get(ValueLayout.JAVA_INT, at + 4));
                byte[] part = scratch.arenaBytes(
                        cells.get(ValueLayout.JAVA_INT, at + 8), cells.get(ValueLayout.JAVA_INT, at + 12));
                int flags = cells.get(ValueLayout.JAVA_INT, at + 16);
                listed[index] = new SheetInfo(
                        new String(name, StandardCharsets.UTF_8),
                        (flags & 1) != 0,
                        part,
                        cells.get(ValueLayout.JAVA_INT, at + 20));
            }
            sheets = List.of(listed);

            // The shared strings take no more room than their part inflates to; asking for it
            // once saves growing into it.
            long bound = Math.min(opened.get(ValueLayout.JAVA_LONG, Native.OPENED_STRINGS_BYTES), 1L << 28);
            if (!stingy && scratch.arena.bytes() < bound) {
                scratch.arena.resize(bound, scratch.arena.bytes());
            }
            filled = settle(scratch.drive(this, state, Native.Book.STRINGS));
            strings = copy(scratch.arena.segment, filled.get(ValueLayout.JAVA_LONG, Native.FILLED_ARENA_USED), 1);
            table = copy(
                    scratch.cells.segment,
                    filled.get(ValueLayout.JAVA_LONG, Native.FILLED_ROWS) * Native.SPAN_BYTES,
                    4);

            filled = settle(scratch.drive(this, state, Native.Book.STYLES));
            kinds = copy(scratch.arena.segment, filled.get(ValueLayout.JAVA_LONG, Native.FILLED_ROWS), 1);
        } finally {
            scratch.close();
        }
    }

    private MemorySegment copy(MemorySegment from, long bytes, long alignment) {
        MemorySegment kept = arena.allocate(Math.max(bytes, 1), alignment).asSlice(0, bytes);
        kept.copyFrom(from.asSlice(0, bytes));
        return kept;
    }

    /** Opening starts over when it is refused, and reports through its own shape. */
    private MemorySegment open(Scratch scratch) {
        MemorySegment opened = arena.allocate(Native.OPENED_BYTES, 8);
        while (true) {
            int code = Native.book(Native.Book.OPEN, state, container, scratch.buffers(null), opened);
            long needed = opened.get(ValueLayout.JAVA_LONG, Native.OPENED_NEEDED);
            switch (code) {
                case Native.OK -> {
                    return opened;
                }
                case Native.ERR_WINDOW -> scratch.window.resize(Math.max(needed, scratch.window.bytes() + 1), 0);
                case Native.ERR_ARENA -> scratch.arena.resize(Math.max(needed, scratch.arena.bytes() + 1), 0);
                case Native.ERR_STRUCTURE -> throw TabularException.from(opened, 32);
                default -> throw new IllegalStateException(CONTRACT_VIOLATION);
            }
        }
    }

    /** What a call's answer means: done, or a structural failure; anything else is this binding's bug. */
    static MemorySegment settle(int code, MemorySegment filled) {
        return switch (code) {
            case Native.OK -> filled;
            case Native.ERR_STRUCTURE -> throw TabularException.from(filled, Native.FAILURE_CODE);
            default -> throw new IllegalStateException(CONTRACT_VIOLATION);
        };
    }

    private static MemorySegment settle(Scratch.Answer answer) {
        return settle(answer.code(), answer.filled());
    }

    /**
     * Which kind of workbook this is.
     *
     * @return the format
     */
    public WorkbookFormat format() {
        return format;
    }

    /**
     * The date system the workbook's serials count in: what a date-formatted number is read
     * by.
     *
     * @return the date system
     */
    public ExcelEpoch dateSystem() {
        return dateSystem;
    }

    /**
     * The workbook's sheets, in its own order. Sheets that hold no cells (chart sheets, macro
     * sheets) are not among them.
     *
     * @return the sheets, unmodifiable
     */
    public List<SheetInfo> sheets() {
        return sheets;
    }

    /**
     * Starts a read of the sheet at {@code index} of {@link #sheets()} through {@code plan}.
     *
     * @param index the sheet's place in {@link #sheets()}
     * @param options how the sheet is read
     * @param plan the output columns, in output order
     * @return the sheet, its header already read when the options declare one
     * @throws IndexOutOfBoundsException if the workbook has no sheet at that index
     * @throws TabularException if the sheet, or its header row, is structurally broken
     */
    public Sheet sheet(int index, SheetOptions options, List<Column> plan) {
        Objects.requireNonNull(plan, "plan");
        requireOpen();
        return new Sheet(this, sheets.get(Objects.checkIndex(index, sheets.size())), options, plan);
    }

    /**
     * Starts a read of the sheet at {@code index} of {@link #sheets()} and reads its header,
     * leaving the plan to {@link Sheet#bind} — so that the plan can be made from the
     * header's names.
     *
     * @param index the sheet's place in {@link #sheets()}
     * @param options how the sheet is read
     * @return the sheet, its header already read when the options declare one
     * @throws IndexOutOfBoundsException if the workbook has no sheet at that index
     * @throws TabularException if the sheet, or its header row, is structurally broken
     */
    public Sheet sheet(int index, SheetOptions options) {
        requireOpen();
        return new Sheet(this, sheets.get(Objects.checkIndex(index, sheets.size())), options, null);
    }

    /**
     * Starts a read of the first sheet named {@code name} through {@code plan}.
     *
     * @param name the sheet's name
     * @param options how the sheet is read
     * @param plan the output columns, in output order
     * @return the sheet, its header already read when the options declare one
     * @throws NoSuchElementException if the workbook has no sheet by that name
     * @throws TabularException if the sheet, or its header row, is structurally broken
     */
    public Sheet sheet(String name, SheetOptions options, List<Column> plan) {
        Objects.requireNonNull(plan, "plan");
        return new Sheet(this, named(name), options, plan);
    }

    /**
     * Starts a read of the first sheet named {@code name} and reads its header, leaving the
     * plan to {@link Sheet#bind}.
     *
     * @param name the sheet's name
     * @param options how the sheet is read
     * @return the sheet, its header already read when the options declare one
     * @throws NoSuchElementException if the workbook has no sheet by that name
     * @throws TabularException if the sheet, or its header row, is structurally broken
     */
    public Sheet sheet(String name, SheetOptions options) {
        return new Sheet(this, named(name), options, null);
    }

    private SheetInfo named(String name) {
        requireOpen();
        Objects.requireNonNull(name, "name");
        for (SheetInfo sheet : sheets) {
            if (sheet.name().equals(name)) {
                return sheet;
            }
        }
        throw new NoSuchElementException("The workbook has no sheet named \"" + name + "\".");
    }

    void requireOpen() {
        if (closed) {
            throw new IllegalStateException("The workbook is closed.");
        }
    }

    MemorySegment container() {
        return container;
    }

    MemorySegment state() {
        return state;
    }

    MemorySegment strings() {
        return strings;
    }

    /** Writes the workbook's tables into a buffers block, for a call that reads a sheet. */
    void tables(MemorySegment buffers) {
        buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_STRINGS, strings);
        buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_STRINGS + 8, strings.byteSize());
        buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_TABLE, table);
        buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_TABLE + 8, table.byteSize() / Native.SPAN_BYTES);
        buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_KINDS, kinds);
        buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_KINDS + 8, kinds.byteSize());
    }

    /**
     * Releases the workbook's native memory, the container included when it was copied or
     * mapped. The workbook's sheets are over with it; closing twice is harmless.
     */
    @Override
    public void close() {
        if (closed) {
            return;
        }
        closed = true;
        arena.close();
        if (read != null) {
            read.close();
        }
    }

    /** The buffers a call to the core may ask to have grown: one set while the workbook opens, one for each sheet. */
    static final class Scratch {
        record Answer(int code, MemorySegment filled) {}

        final Block window;
        final Block arena;
        final Block cells;
        /** The row's slots: where the core assembles a row, and keeps the header row's cells. */
        final Block row;

        final MemorySegment buffers;
        final MemorySegment filled;
        /** How many slots the core is told {@link #row} has. */
        long rowSlots;

        Scratch(Arena owner, long window, long arena, long cells, long rowSlots) {
            this.window = new Block(window, 8);
            this.arena = new Block(arena, 8);
            this.cells = new Block(cells * Native.SPAN_BYTES, 4);
            this.row = new Block(Math.max(rowSlots, 1) * Native.SLOT_BYTES, 8);
            this.buffers = owner.allocate(Native.BUFFERS_BYTES, 8);
            this.filled = owner.allocate(Native.FILLED_BYTES, 8);
            this.rowSlots = rowSlots;
        }

        /** Entries the cell table has room for. */
        long cellEntries() {
            return cells.bytes() / Native.SPAN_BYTES;
        }

        /** Grows the row's slots to {@code slots}, keeping what they held: never shrinks them. */
        void growRow(long slots) {
            if (rowSlots >= slots) {
                return;
            }
            row.resize(Math.multiplyExact(slots, Native.SLOT_BYTES), rowSlots * Native.SLOT_BYTES);
            rowSlots = slots;
        }

        byte[] arenaBytes(int offset, int span) {
            return arena.segment.asSlice(offset, span & Native.SPAN_LENGTH).toArray(ValueLayout.JAVA_BYTE);
        }

        /** This scratch as the core takes it, with the workbook's tables when a sheet is being read. */
        MemorySegment buffers(Workbook book) {
            buffers.fill((byte) 0);
            buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_WINDOW, window.segment);
            buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_WINDOW + 8, window.bytes());
            buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_ARENA, arena.segment);
            buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_ARENA + 8, arena.bytes());
            buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_CELLS, cells.segment);
            buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_CELLS + 8, cells.bytes() / Native.SPAN_BYTES);
            buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_ROW, row.segment);
            buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_ROW + 8, rowSlots);
            if (book != null) {
                book.tables(buffers);
            }
            return buffers;
        }

        /**
         * Makes the call until it stops asking for room, growing the buffer it names each time
         * — with what the buffer held kept, which is what lets the core go on from where it
         * stopped. Returns the code it ended on and what it reported.
         */
        Answer drive(Workbook book, MemorySegment state, Native.Book call) {
            return drive(book, state, call, null, false);
        }

        /**
         * {@link #drive(Workbook, MemorySegment, Native.Book)}, for a fill when {@code columns}
         * is given. {@code rowFollowsCells} grows the row's slots with the cell table, keeping
         * what they held — for the header of a sheet with no plan, whose slots are to hold
         * every cell the header row has, since a row the sheet repeats (an ODS
         * {@code number-rows-repeated}) is delivered again from them.
         */
        Answer drive(Workbook book, MemorySegment state, Native.Book call, Columns columns, boolean rowFollowsCells) {
            boolean tables = call == Native.Book.HEADER || columns != null;
            while (true) {
                MemorySegment handed = buffers(tables ? book : null);
                int code = columns == null
                        ? Native.book(call, state, book.container, handed, filled)
                        : Native.bookFill(
                                state,
                                book.container,
                                columns.specs,
                                columns.buffers,
                                columns.plan.length,
                                columns.batchRows,
                                handed,
                                filled);
                long needed = filled.get(ValueLayout.JAVA_LONG, Native.FILLED_NEEDED);
                switch (code) {
                    case Native.ERR_WINDOW -> {
                        window.resize(Math.max(needed, window.bytes() + 1), window.bytes());
                        grown[0]++;
                    }
                    case Native.ERR_ARENA -> {
                        arena.resize(Math.max(needed, arena.bytes() + 1), arena.bytes());
                        grown[1]++;
                    }
                    case Native.ERR_CELLS -> {
                        cells.resize(
                                Math.max(needed * Native.SPAN_BYTES, cells.bytes() + Native.SPAN_BYTES), cells.bytes());
                        grown[2]++;
                        if (rowFollowsCells) {
                            growRow(cellEntries());
                        }
                    }
                    default -> {
                        return new Answer(code, filled);
                    }
                }
            }
        }

        void close() {
            window.close();
            arena.close();
            cells.close();
            row.close();
        }
    }
}
