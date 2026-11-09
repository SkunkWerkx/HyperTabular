package io.github.skunkwerkx.hypertabular;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.file.Path;
import java.util.List;
import java.util.Objects;
import java.util.function.Consumer;

/**
 * A forward-only read of one sheet of a {@link Workbook}, a batch at a time, through a plan —
 * the same {@link Batch} delimited text is read into.
 *
 * <p>A sheet has its own buffers and its own copy of the workbook's read state, so several can
 * be read at once; each reads from the workbook, which has to outlive it. A typed workbook cell
 * is converted directly by its door — a stored {@code 42.0} never passes through text to
 * become an {@code int} — and a text cell goes through the door as delimited text would. A
 * sheet started without a plan has read its header, and is given its plan by {@link #bind}.
 *
 * <p>Not thread-safe, and confined to the thread that opened the workbook.
 */
public final class Sheet implements AutoCloseable {
    private final Workbook book;
    private final SheetInfo info;
    private SheetOptions options;
    private final Arena arena;
    /** The plan's columns and the batch they are read into: {@code null} until a plan is bound. */
    private Columns columns;

    private Batch batch;
    private final MemorySegment state;
    private final Workbook.Scratch scratch;
    /** Cell-table entries to a row: one per plan column, and one more. */
    private int perRow;

    private Header header;
    /** A failure met with rows before it: those went out first, and this is next. */
    private TabularException pending;

    private TabularException failure;
    private boolean closed;

    /** Starts a read of a sheet, through {@code plan}, or with the plan left to {@link #bind} when it is {@code null}. */
    Sheet(Workbook book, SheetInfo info, SheetOptions options, List<Column> plan) {
        this.book = book;
        this.info = info;
        this.options = Objects.requireNonNull(options, "options");
        Column[] checked = plan == null ? null : Columns.checked(plan);
        Columns.checkSizes(
                MemorySegment.NULL,
                0,
                options.batchRows(),
                1,
                Abi.BATCH_SHEET | (options.notes() ? Abi.BATCH_NOTES : 0));
        this.arena = Arena.ofConfined();
        Workbook.Scratch made = null;
        try {
            if (checked != null) {
                Columns bound = new Columns(checked, options.batchRows(), arena, options.notes(), false, true);
                made = Workbook.stingy
                        ? new Workbook.Scratch(arena, 1, 1, 1, bound.width)
                        : new Workbook.Scratch(
                                arena,
                                Abi.WINDOW_MIN,
                                4096,
                                Math.multiplyExact((long) options.batchRows(), checked.length + 1),
                                bound.width);
                attach(bound);
            } else {
                // The header row's cells are kept in the row's slots as well as named in the
                // cell table — the slots are what a row the sheet repeats (an ODS
                // number-rows-repeated) is delivered again from. A plan says how many slots it
                // reads; without one there is a slot for every entry the cell table has room
                // for, and the slots grow with it (Scratch.drive) while the header is read.
                made = Workbook.stingy
                        ? new Workbook.Scratch(arena, 1, 1, 1, 1)
                        : new Workbook.Scratch(arena, Abi.WINDOW_MIN, 4096, 64, 64);
            }
            scratch = made;
            byte[] tokens = options.tokens().packed();
            if (tokens.length > 0) {
                scratch.tokens = Columns.held(tokens, arena);
                scratch.tokensBytes = tokens.length;
            }
            // A copy of the opened state, which the core allows, so that this sheet's read is its own.
            state = arena.allocate(book.state().byteSize(), 8);
            state.copyFrom(book.state());

            MemorySegment part = arena.allocate(Math.max(info.part().length, 1)).asSlice(0, info.part().length);
            MemorySegment.copy(info.part(), 0, part, ValueLayout.JAVA_BYTE, 0, info.part().length);
            MemorySegment raw = arena.allocate(Abi.SHEET_OPTIONS_BYTES, 4);
            raw.set(ValueLayout.JAVA_INT, 0, (int) Abi.SHEET_OPTIONS_BYTES);
            raw.set(ValueLayout.JAVA_INT, Abi.SHEET_OPTIONS_HAS_HEADER, options.hasHeader() ? 1 : 0);
            raw.set(ValueLayout.JAVA_INT, Abi.SHEET_OPTIONS_SKIP_EMPTY_ROWS, options.skipEmptyRows() ? 1 : 0);
            raw.set(ValueLayout.JAVA_INT, Abi.SHEET_OPTIONS_SKIP, options.skip());
            raw.set(ValueLayout.JAVA_INT, Abi.SHEET_OPTIONS_HEADER_ROW, options.headerRow());
            raw.set(
                    ValueLayout.JAVA_INT,
                    Abi.SHEET_OPTIONS_FIRST_ROW,
                    options.range().firstRow());
            raw.set(
                    ValueLayout.JAVA_INT,
                    Abi.SHEET_OPTIONS_FIRST_COLUMN,
                    options.range().firstColumn());
            raw.set(
                    ValueLayout.JAVA_INT,
                    Abi.SHEET_OPTIONS_LAST_ROW,
                    options.range().lastRow());
            raw.set(
                    ValueLayout.JAVA_INT,
                    Abi.SHEET_OPTIONS_LAST_COLUMN,
                    options.range().lastColumn());
            Workbook.settle(
                    Native.sheetWith(state, book.container(), part, info.index(), raw, scratch.filled), scratch.filled);
            if (options.hasHeader()) {
                header = readHeader();
            }
        } catch (RuntimeException | Error thrown) {
            if (batch != null) {
                batch.close();
            }
            if (made != null) {
                made.close();
            }
            arena.close();
            throw thrown;
        }
    }

    private void attach(Columns bound) {
        perRow = bound.plan.length + 1;
        batch = new Batch(bound, true, options.firstColumn());
        columns = bound;
    }

    private Header readHeader() {
        Workbook.Scratch.Answer answer = scratch.drive(book, state, Abi.Book.HEADER, null, columns == null);
        MemorySegment filled = Workbook.settle(answer.code(), answer.filled());
        int count = (int) filled.get(ValueLayout.JAVA_LONG, Abi.FILLED_ROWS);
        Header.Builder names = new Header.Builder(count);
        MemorySegment cells = scratch.cells.segment;
        for (int index = 0; index < count; index++) {
            int offset = cells.get(ValueLayout.JAVA_INT, index * Abi.SPAN_BYTES);
            int span = cells.get(ValueLayout.JAVA_INT, index * Abi.SPAN_BYTES + 4);
            names.add(
                    span < 0
                            ? scratch.arena.segment.asSlice(offset, span & Abi.SPAN_LENGTH)
                            : book.strings().asSlice(offset, span));
        }
        return names.build();
    }

    /**
     * Declares the plan a sheet started without one reads through — once, before the first
     * read, and usually after {@link #header()} has said where each column is. The column
     * arrays and the batch are sized here, by the plan and the options' batch size.
     *
     * @param plan the output columns, in output order
     * @throws IllegalStateException if the sheet already has a plan, or is closed
     * @throws IllegalArgumentException if a column cannot be honoured; the sheet is left
     *     without a plan
     */
    public void bind(List<Column> plan) {
        bindPlan(plan, false);
    }

    private void bindPlan(List<Column> plan, boolean rowWidths) {
        if (closed) {
            throw new IllegalStateException("The sheet is closed.");
        }
        if (columns != null) {
            throw new IllegalStateException("The sheet already has a plan; a plan is bound once.");
        }
        Column[] checked = Columns.checked(plan);
        Columns bound = new Columns(checked, options.batchRows(), arena, options.notes(), rowWidths, true);
        // Grown, never replaced: the cell table only to save the first fill a call (and not
        // for the tests' stingy pass, whose point is that it grows), the row's slots because
        // what they hold is a header row the sheet may yet repeat.
        long cells = Math.multiplyExact((long) options.batchRows(), checked.length + 1);
        if (!Workbook.stingy && scratch.cellEntries() < cells) {
            scratch.cells.resize(Math.multiplyExact(cells, Abi.SPAN_BYTES), scratch.cells.bytes());
        }
        scratch.growRow(bound.width);
        attach(bound);
    }

    /**
     * Whether a plan has been bound: always, for a sheet started with one.
     *
     * @return {@code true} once the sheet has a plan
     */
    public boolean isBound() {
        return columns != null;
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
     * {@code plan().get(i)}. Empty until one is bound.
     *
     * @return the plan, unmodifiable
     */
    public List<Column> plan() {
        return columns == null ? List.of() : columns.planList;
    }

    /**
     * The header row's names — a typed cell said the way the text door says it — or
     * {@code null} if the sheet was opened without a header. A header with no names is a
     * sheet with no rows.
     *
     * @return the header's names, unmodifiable, or {@code null}
     */
    public Header header() {
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
     * @throws IllegalStateException if the sheet or its workbook is closed, or no plan has
     *     been bound — not final: bind one and read
     */
    public Batch read() {
        if (closed) {
            throw new IllegalStateException("The sheet is closed.");
        }
        book.requireOpen();
        if (columns == null) {
            throw new IllegalStateException("The sheet has no plan yet: bind one before reading.");
        }
        batch.clear();
        if (pending != null) {
            failure = pending;
            pending = null;
        }
        if (failure != null) {
            throw failure;
        }
        Workbook.Scratch.Answer answer = scratch.drive(book, state, null, columns, false);
        MemorySegment filled = answer.filled();
        int rows = (int) filled.get(ValueLayout.JAVA_LONG, Abi.FILLED_ROWS);
        if (answer.code() != Abi.OK) {
            if (answer.code() != Abi.ERR_STRUCTURE) {
                throw new IllegalStateException(Workbook.CONTRACT_VIOLATION);
            }
            TabularException broken = TabularException.from(filled, Abi.FAILURE_CODE);
            if (rows == 0) {
                throw failure = broken;
            }
            pending = broken;
        }
        if (rows == 0) {
            return null;
        }
        return batch.fill(rows, scratch.cells.segment, perRow, book.strings(), 0, scratch.arena.segment, 0);
    }

    /**
     * Reads every row left, batch by batch, handing each to {@code action} — the row view of a
     * sheet, whose batches are over when the next is read. Each {@link Row} is a view of the
     * current batch, valid only for its own call: do not keep one. A failure, the sheet's or
     * {@code action}'s, ends the loop and is thrown from here.
     *
     * @param action what is done with each row
     * @throws TabularException if the sheet is structurally broken, after every intact row
     *     before the break has been handed over
     * @throws IllegalStateException as {@link #read()} throws it
     */
    public void forEachRow(Consumer<? super Row> action) {
        Objects.requireNonNull(action, "action");
        for (Batch next = read(); next != null; next = read()) {
            for (Row row : next) {
                action.accept(row);
            }
        }
    }

    /**
     * Reads a sample of this sheet from its start, with {@link SuggestOptions#DEFAULT}.
     *
     * @return the suggestion
     * @throws TabularException if the sampled rows are structurally broken
     * @see #suggest(SuggestOptions)
     */
    public Suggestion suggest() {
        return suggest(SuggestOptions.DEFAULT);
    }

    /**
     * Reads a sample of this sheet through the text door and says what its columns look like, as
     * {@link DelimitedReader#suggest(Path, Dialect, SuggestOptions)} does. It is a read of its own,
     * from the sheet's start with the sheet's options, and this one's own read is left as it was.
     * Its plan's ordinals count from the range's first column, as any plan's do.
     *
     * <p>A typed number is judged by its value and has no notation, whichever is declared: a column
     * of them is a number under any format, and its plan reads every one, since a typed cell reads
     * the same under them all. A text cell holding {@code 1.234} keeps its question, in a column of
     * its own or beside typed numbers. A typed date, date-time, time or duration cell reads as its
     * canonical text, which the suggester does not take as a date: such a column of a sheet is
     * text, resolved, for a plan to change by hand.
     *
     * @param options how the sample is taken and read
     * @return the suggestion
     * @throws TabularException if the sampled rows are structurally broken: there is no knowing
     *     what is past them, and a read would stop there too. A failure past them is not met, or
     *     only says the source was truncated
     * @throws IllegalStateException if the sheet or its workbook is closed
     */
    public Suggestion suggest(SuggestOptions options) {
        Objects.requireNonNull(options, "options");
        if (closed) {
            throw new IllegalStateException("The sheet is closed.");
        }
        book.requireOpen();
        int limit = options.rowsWithin(this.options.batchRows());
        int width = -1;
        if (!this.options.hasHeader()) {
            try (Sheet first =
                    new Sheet(book, info, this.options.withBatchRows(1).withNotes(false), null)) {
                width = first.firstRowWidth();
            }
        }
        // Notes on, for what the sample's second door says of a cell.
        try (Sheet sheet = new Sheet(book, info, this.options.withBatchRows(1).withNotes(true), null)) {
            int columns = width < 0 ? sheet.header.size() : width;
            // One row more than the sample, so that a row past it costs no second batch to find, in
            // as many batches as the core says that takes.
            Sample sample = Sample.of(
                    sheet::read,
                    plan -> {
                        sheet.options = sheet.options.withBatchRows(
                                Columns.sampleRows(plan, limit + 1, Abi.BATCH_SHEET | Abi.BATCH_NOTES));
                        sheet.bind(plan);
                    },
                    columns,
                    limit,
                    true);
            return sample.judge(sheet.header, options);
        }
    }

    /** How many cells the sheet's next row has: 0 for no row. */
    private int firstRowWidth() {
        bindPlan(List.of(Column.text(0)), true);
        return read() == null ? 0 : columns.rowWidths.get(ValueLayout.JAVA_INT, 0);
    }

    /** Releases the sheet's native memory. Closing twice is harmless. */
    @Override
    public void close() {
        if (closed) {
            return;
        }
        closed = true;
        if (batch != null) {
            batch.close();
        }
        scratch.close();
        arena.close();
    }
}
