package io.github.skunkwerkx.hypertabular;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Objects;

/**
 * A forward-only read of one sheet of a {@link Workbook}, a batch at a time, through a plan —
 * the same {@link Batch} delimited text is read into.
 *
 * <p>A sheet has its own buffers and its own copy of the workbook's read state, so several can
 * be read at once; each reads from the workbook, which has to outlive it. A typed workbook cell
 * is converted directly by its door — a stored {@code 42.0} never passes through text to
 * become an {@code int} — and a text cell goes through the door as delimited text would.
 *
 * <p>Not thread-safe, and confined to the thread that opened the workbook.
 */
public final class Sheet implements AutoCloseable {
    private final Workbook book;
    private final SheetOptions options;
    private final Arena arena;
    private final Columns columns;
    private final Batch batch;
    private final MemorySegment state;
    private final Workbook.Scratch scratch;
    /** Cell-table entries to a row: one per plan column, and one more. */
    private final int perRow;

    private List<String> header;
    /** A failure met with rows before it: those went out first, and this is next. */
    private TabularException pending;

    private TabularException failure;
    private boolean closed;

    Sheet(Workbook book, SheetInfo info, SheetOptions options, List<Column> plan) {
        this.book = book;
        this.options = Objects.requireNonNull(options, "options");
        Column[] checked = Columns.checked(plan, options.batchRows());
        this.arena = Arena.ofConfined();
        Workbook.Scratch made = null;
        try {
            columns = new Columns(checked, options.batchRows(), arena);
            batch = new Batch(columns, true);
            perRow = checked.length + 1;
            // A copy of the opened state, which the core allows, so that this sheet's read is its own.
            state = arena.allocate(book.state().byteSize(), 8);
            state.copyFrom(book.state());
            made = Workbook.stingy
                    ? new Workbook.Scratch(arena, 1, 1, 1, columns.width)
                    : new Workbook.Scratch(
                            arena,
                            Native.WINDOW_MIN,
                            4096,
                            Math.multiplyExact((long) options.batchRows(), perRow),
                            columns.width);
            scratch = made;

            MemorySegment part = arena.allocate(Math.max(info.part().length, 1)).asSlice(0, info.part().length);
            MemorySegment.copy(info.part(), 0, part, ValueLayout.JAVA_BYTE, 0, info.part().length);
            Workbook.settle(
                    Native.sheet(
                            state,
                            book.container(),
                            part,
                            info.index(),
                            options.hasHeader(),
                            options.skipEmptyRows(),
                            scratch.filled),
                    scratch.filled);
            if (options.hasHeader()) {
                header = readHeader();
            }
        } catch (RuntimeException | Error thrown) {
            if (made != null) {
                made.close();
            }
            arena.close();
            throw thrown;
        }
    }

    private List<String> readHeader() {
        Workbook.Scratch.Answer answer = scratch.drive(book, state, Native.Book.HEADER);
        MemorySegment filled = Workbook.settle(answer.code(), answer.filled());
        int count = (int) filled.get(ValueLayout.JAVA_LONG, Native.FILLED_ROWS);
        String[] names = new String[count];
        MemorySegment cells = scratch.cells.segment;
        for (int index = 0; index < count; index++) {
            int offset = cells.get(ValueLayout.JAVA_INT, index * Native.SPAN_BYTES);
            int span = cells.get(ValueLayout.JAVA_INT, index * Native.SPAN_BYTES + 4);
            MemorySegment bytes = span < 0
                    ? scratch.arena.segment.asSlice(offset, span & Native.SPAN_LENGTH)
                    : book.strings().asSlice(offset, span);
            names[index] = new String(bytes.toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8);
        }
        return List.of(names);
    }

    /**
     * The options the sheet is read with.
     *
     * @return the options
     */
    public SheetOptions options() {
        return options;
    }

    /**
     * The plan the sheet is read through: column {@code i} of every batch is
     * {@code plan().get(i)}.
     *
     * @return the plan, unmodifiable
     */
    public List<Column> plan() {
        return columns.planList;
    }

    /**
     * The header row's names — a typed cell said the way the text door says it — or
     * {@code null} if the sheet was opened without a header. A header with no names is a
     * sheet with no rows.
     *
     * @return the header's names, unmodifiable, or {@code null}
     */
    public List<String> header() {
        return header;
    }

    /**
     * Reads the next batch: up to the sheet's batch size of rows, or {@code null} when the
     * sheet has no more. The batch is valid until the next call.
     *
     * @return the batch, or {@code null}
     * @throws TabularException if the sheet is structurally broken — raised after every
     *     intact row before the break has been delivered, and again, the same exception, on
     *     every later call
     * @throws IllegalStateException if the sheet or its workbook is closed
     */
    public Batch read() {
        if (closed) {
            throw new IllegalStateException("The sheet is closed.");
        }
        book.requireOpen();
        batch.clear();
        if (pending != null) {
            failure = pending;
            pending = null;
        }
        if (failure != null) {
            throw failure;
        }
        Workbook.Scratch.Answer answer = scratch.drive(book, state, null, columns);
        MemorySegment filled = answer.filled();
        int rows = (int) filled.get(ValueLayout.JAVA_LONG, Native.FILLED_ROWS);
        if (answer.code() != Native.OK) {
            if (answer.code() != Native.ERR_STRUCTURE) {
                throw new IllegalStateException(Workbook.CONTRACT_VIOLATION);
            }
            TabularException broken = TabularException.from(filled, Native.FAILURE_CODE);
            if (rows == 0) {
                throw failure = broken;
            }
            pending = broken;
        }
        if (rows == 0) {
            return null;
        }
        return batch.fill(rows, scratch.cells.segment, perRow, book.strings(), 0, scratch.arena.segment);
    }

    /** Releases the sheet's native memory. Closing twice is harmless. */
    @Override
    public void close() {
        if (closed) {
            return;
        }
        closed = true;
        batch.close();
        scratch.close();
        arena.close();
    }
}
