package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.Verdict;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.List;
import java.util.Objects;
import java.util.OptionalInt;
import java.util.function.Consumer;

/**
 * Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
 * typed columns, every cell a HyperCast {@link Verdict}.
 *
 * <p>The native core ({@code libhypertabular}) owns no memory and reads no files. It is
 * handed a chunk of input and the buffers to fill, casts each plan column in one native
 * loop, and says how many rows it wrote and how many bytes it is finished with. Everything
 * else is here, which is the point of the design: this class allocates the input buffer,
 * one value array and one verdict array per column, the table that locates each cell, and
 * the arena for the rare escaped cell — once, as native memory, reused for every batch —
 * and puts what the core did not consume back in front of it. The native boundary is
 * crossed once per batch, not once per cell.
 *
 * <p>{@link #read()} hands out a {@link Batch} that is a view of those buffers, valid until
 * the next {@link #read()}. A value that does not cast is that cell's verdict, and the read
 * goes on. Input that is not rows of cells at all — a record of the wrong width, a quote never
 * closed — is a {@link TabularException}, raised after every intact row before it has been
 * delivered.
 *
 * {@snippet :
 * List<Column> plan = List.of(Column.i32(0), Column.text(1), Column.f64(2));
 * try (DelimitedReader reader = DelimitedReader.open(Path.of("orders.csv"), Dialect.CSV, plan)) {
 *     for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
 *         for (int row = 0; row < batch.rows(); row++) {
 *             String line = switch (batch.get(2, row, Double.class)) {
 *                 case Success<Double> score -> batch.string(1, row) + ": " + score.value();
 *                 case Fault<Double> fault -> fault.reason() + " in \"" + batch.rawString(2, row) + "\"";
 *             };
 *         }
 *     }
 * }
 * }
 *
 * <p>Every factory has a form without a plan, which reads the header and leaves the plan to
 * {@link #bind(List, int)}, so that it can be made from the header's names:
 *
 * {@snippet :
 * try (DelimitedReader reader = DelimitedReader.open(Path.of("orders.csv"), Dialect.CSV)) {
 *     Header header = reader.header();
 *     reader.bind(List.of(Column.i32(header.ordinal("id")), Column.f64(header.ordinal("score"))));
 *     reader.forEachRow(row -> System.out.println(row.line() + ": " + row.get(1, Double.class)));
 * }
 * }
 *
 * <p>A failure of the underlying stream or file is an {@link UncheckedIOException}, from the
 * factory or from {@link #read()}.
 *
 * <p>Not thread-safe, and confined: the buffers are a confined {@link Arena}'s, so a reader
 * belongs to the thread that built it, and the runtime refuses a call from any other.
 */
public final class DelimitedReader implements AutoCloseable {
    /** The row ceiling: a single record larger than this is a {@link TabularFailure#ROW_TOO_LONG}. */
    public static final int MAX_ROW_BYTES = 1 << 30;

    /** The initial read buffer for a stream or a file. */
    public static final int DEFAULT_BUFFER_BYTES = 256 * 1024;

    /** Rows per batch unless told otherwise. */
    public static final int DEFAULT_BATCH_ROWS = 4096;

    private static final String CONTRACT_VIOLATION =
            "libhypertabular reported a contract violation — a binding bug, please report it";

    // One call's input is at most this: a span's offset and length are 31 bits.
    private static final long MAX_WINDOW_BYTES = Integer.MAX_VALUE;

    private static final ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;
    private static final ValueLayout.OfShort SHORT = ValueLayout.JAVA_SHORT;
    private static final ValueLayout.OfInt INT = ValueLayout.JAVA_INT;
    private static final ValueLayout.OfLong LONG = ValueLayout.JAVA_LONG;

    /** Where a stream's or a file's bytes come from, read straight into native memory. */
    private interface Source {
        /** Reads at least one byte into {@code target} at {@code at}, or returns -1 at the end. */
        int read(MemorySegment target, long at, long room) throws IOException;

        void close() throws IOException;
    }

    /** A file: the channel reads into the native buffer itself, with no copy between. */
    private static final class FileSource implements Source {
        private final FileChannel channel;

        FileSource(FileChannel channel) {
            this.channel = channel;
        }

        @Override
        public int read(MemorySegment target, long at, long room) throws IOException {
            ByteBuffer view =
                    target.asSlice(at, Math.min(room, Integer.MAX_VALUE)).asByteBuffer();
            int read;
            do {
                read = channel.read(view);
            } while (read == 0);
            return read;
        }

        @Override
        public void close() throws IOException {
            channel.close();
        }
    }

    /**
     * A stream: {@link InputStream} can only fill a Java array, so each read lands in one
     * and is copied to the native buffer — the one copy a stream costs.
     */
    private static final class StreamSource implements Source {
        private static final int MAX_TRANSFER_BYTES = 1 << 20;

        private final InputStream stream;
        private final byte[] transfer;

        StreamSource(InputStream stream, int bufferBytes) {
            this.stream = stream;
            this.transfer = new byte[Math.min(bufferBytes, MAX_TRANSFER_BYTES)];
        }

        @Override
        public int read(MemorySegment target, long at, long room) throws IOException {
            int want = (int) Math.min(room, transfer.length);
            int read;
            do {
                read = stream.read(transfer, 0, want);
            } while (read == 0);
            if (read > 0) {
                MemorySegment.copy(transfer, 0, target, BYTE, at, read);
            }
            return read;
        }

        @Override
        public void close() throws IOException {
            stream.close();
        }
    }

    private final Dialect dialect;
    /** The plan's columns and the batch they are read into: {@code null} until a plan is bound. */
    private Columns columns;

    private Batch batch;
    /** Cell-table entries one row takes: one per source column the plan reaches, and one more. */
    private int perRow;

    // Everything the core is handed, allocated once: the fixed-size blocks in one arena,
    // and each buffer that can be outgrown in its own.
    private final Arena arena;
    private MemorySegment state;
    private MemorySegment filled;
    // The fill's cells and arena, as the core takes them: in a buffers block, the workbook
    // calls' own layout, of which the fill reads those two and nothing else (Mono's
    // interpreter, .NET's in the browser, passes no more than twelve integer arguments to a
    // native function, and the ABI is the same for every binding). Allocated zeroed.
    private MemorySegment buffers;
    private Block cells;
    private Block unescaped;
    /** The last batch came up short with the arena mostly used: it wants a larger one. */
    private boolean cramped;

    // The source: a stream or file read into `buffer`, or memory read in place.
    private Source source;
    private Block buffer;
    private MemorySegment memory;
    private MemorySegment memoryReadOnly;
    private long maxWindow = MAX_WINDOW_BYTES;
    private long start;
    private long end;
    private boolean eof;

    private Header header;
    private TabularException failure;
    private boolean closed;
    private long recordsAtClose;
    private int expectedAtClose;

    /**
     * Reads UTF-8 delimited text held in a byte array, {@value #DEFAULT_BATCH_ROWS} rows a
     * batch.
     *
     * @param utf8 the text
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured
     * @throws TabularException if the header record is structurally broken
     * @see #of(byte[], Dialect, List, int)
     */
    public static DelimitedReader of(byte[] utf8, Dialect dialect, List<Column> plan) {
        return of(utf8, dialect, plan, DEFAULT_BATCH_ROWS);
    }

    /**
     * Reads UTF-8 delimited text held in a byte array. The array is copied into native
     * memory once, up front — the core is not handed the Java heap — and the caller's array
     * is not touched again; {@link #of(MemorySegment, Dialect, List, int)} over a native
     * segment copies nothing.
     *
     * @param utf8 the text
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @param batchRows rows per batch
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured, or
     *     {@code batchRows} is not positive
     * @throws TabularException if the header record is structurally broken
     */
    public static DelimitedReader of(byte[] utf8, Dialect dialect, List<Column> plan, int batchRows) {
        return ofArray(utf8, dialect, Columns.checked(plan, batchRows), batchRows);
    }

    /**
     * Reads UTF-8 delimited text held in a byte array, copied into native memory once, and
     * reads its header, leaving the plan to {@link #bind(List, int)} — so that the plan can be
     * made from the header's names.
     *
     * @param utf8 the text
     * @param dialect the declared dialect
     * @return the reader, its header already read when the dialect declares one, and no plan
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured
     * @throws TabularException if the header record is structurally broken
     */
    public static DelimitedReader of(byte[] utf8, Dialect dialect) {
        return ofArray(utf8, dialect, null, 0);
    }

    private static DelimitedReader ofArray(byte[] utf8, Dialect dialect, Column[] plan, int batchRows) {
        Objects.requireNonNull(utf8, "utf8");
        DelimitedReader reader = new DelimitedReader(dialect, plan, batchRows);
        try {
            MemorySegment copy = reader.arena.allocate(utf8.length);
            MemorySegment.copy(utf8, 0, copy, BYTE, 0, utf8.length);
            return reader.overMemory(copy, MAX_WINDOW_BYTES).begin(dialect);
        } catch (RuntimeException | Error failure) {
            reader.abandon(failure);
            throw failure;
        }
    }

    /**
     * Reads UTF-8 delimited text held in a memory segment, {@value #DEFAULT_BATCH_ROWS}
     * rows a batch.
     *
     * @param utf8 the text
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured
     * @throws TabularException if the header record is structurally broken
     * @see #of(MemorySegment, Dialect, List, int)
     */
    public static DelimitedReader of(MemorySegment utf8, Dialect dialect, List<Column> plan) {
        return of(utf8, dialect, plan, DEFAULT_BATCH_ROWS);
    }

    /**
     * Reads UTF-8 delimited text held in a memory segment. A native segment — a mapped
     * file, an arena allocation — is read in place: nothing is copied, and it must stay
     * alive and unchanged for as long as the reader is open. A heap segment is copied into
     * native memory once, as a byte array is.
     *
     * @param utf8 the text, of any size
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @param batchRows rows per batch
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured, or
     *     {@code batchRows} is not positive
     * @throws TabularException if the header record is structurally broken
     */
    public static DelimitedReader of(MemorySegment utf8, Dialect dialect, List<Column> plan, int batchRows) {
        return windowed(utf8, dialect, Objects.requireNonNull(plan, "plan"), batchRows, MAX_WINDOW_BYTES);
    }

    /**
     * Reads UTF-8 delimited text held in a memory segment — read in place when it is native,
     * as {@link #of(MemorySegment, Dialect, List, int)} reads it — and reads its header,
     * leaving the plan to {@link #bind(List, int)}.
     *
     * @param utf8 the text, of any size
     * @param dialect the declared dialect
     * @return the reader, its header already read when the dialect declares one, and no plan
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured
     * @throws TabularException if the header record is structurally broken
     */
    public static DelimitedReader of(MemorySegment utf8, Dialect dialect) {
        return windowed(utf8, dialect, null, 0, MAX_WINDOW_BYTES);
    }

    /**
     * {@link #of(MemorySegment, Dialect, List, int)}, with a stated limit on how much of the
     * memory one native call is handed. The limit is otherwise the ABI's own, just under
     * 2 GiB: memory larger than that is read a window at a time, and stating a small one is
     * how the suite reaches that path without two gibibytes of input. A {@code null} plan
     * opens the reader header-first.
     */
    static DelimitedReader windowed(
            MemorySegment utf8, Dialect dialect, List<Column> plan, int batchRows, long windowBytes) {
        Objects.requireNonNull(utf8, "utf8");
        DelimitedReader reader =
                new DelimitedReader(dialect, plan == null ? null : Columns.checked(plan, batchRows), batchRows);
        try {
            MemorySegment text = utf8;
            if (!utf8.isNative()) {
                text = reader.arena.allocate(utf8.byteSize());
                text.copyFrom(utf8);
            }
            return reader.overMemory(text, windowBytes).begin(dialect);
        } catch (RuntimeException | Error failure) {
            reader.abandon(failure);
            throw failure;
        }
    }

    /**
     * Reads UTF-8 delimited text from a stream, {@value #DEFAULT_BATCH_ROWS} rows a batch
     * through a {@value #DEFAULT_BUFFER_BYTES}-byte buffer.
     *
     * @param utf8 the stream
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the stream fails
     * @see #of(InputStream, Dialect, List, int, int)
     */
    public static DelimitedReader of(InputStream utf8, Dialect dialect, List<Column> plan) {
        return of(utf8, dialect, plan, DEFAULT_BATCH_ROWS, DEFAULT_BUFFER_BYTES);
    }

    /**
     * Reads UTF-8 delimited text from a stream. The stream is read forward only, and is the
     * reader's from here on: {@link #close()} closes it, and so does this method when it
     * throws.
     *
     * @param utf8 the stream
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @param batchRows rows per batch
     * @param bufferBytes the initial read buffer; it doubles when a record does not fit
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured, or
     *     {@code batchRows} or {@code bufferBytes} is not positive
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the stream fails
     */
    public static DelimitedReader of(
            InputStream utf8, Dialect dialect, List<Column> plan, int batchRows, int bufferBytes) {
        Objects.requireNonNull(plan, "plan");
        return ofStream(utf8, dialect, plan, batchRows, bufferBytes);
    }

    /**
     * Reads UTF-8 delimited text from a stream through a {@value #DEFAULT_BUFFER_BYTES}-byte
     * buffer, and reads its header, leaving the plan to {@link #bind(List, int)}.
     *
     * @param utf8 the stream
     * @param dialect the declared dialect
     * @return the reader, its header already read when the dialect declares one, and no plan
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the stream fails
     * @see #of(InputStream, Dialect, int)
     */
    public static DelimitedReader of(InputStream utf8, Dialect dialect) {
        return of(utf8, dialect, DEFAULT_BUFFER_BYTES);
    }

    /**
     * Reads UTF-8 delimited text from a stream, and reads its header, leaving the plan to
     * {@link #bind(List, int)}. The stream is the reader's from here on, as for
     * {@link #of(InputStream, Dialect, List, int, int)}.
     *
     * @param utf8 the stream
     * @param dialect the declared dialect
     * @param bufferBytes the initial read buffer; it doubles when a record does not fit
     * @return the reader, its header already read when the dialect declares one, and no plan
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured, or
     *     {@code bufferBytes} is not positive
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the stream fails
     */
    public static DelimitedReader of(InputStream utf8, Dialect dialect, int bufferBytes) {
        return ofStream(utf8, dialect, null, 0, bufferBytes);
    }

    private static DelimitedReader ofStream(
            InputStream utf8, Dialect dialect, List<Column> plan, int batchRows, int bufferBytes) {
        Objects.requireNonNull(utf8, "utf8");
        DelimitedReader reader;
        try {
            if (bufferBytes <= 0) {
                throw new IllegalArgumentException("bufferBytes must be positive; got " + bufferBytes);
            }
            reader = new DelimitedReader(dialect, plan == null ? null : Columns.checked(plan, batchRows), batchRows);
        } catch (RuntimeException | Error failure) {
            // No reader came to be, so the stream it would have owned is closed here.
            try {
                utf8.close();
            } catch (IOException | RuntimeException closing) {
                failure.addSuppressed(closing);
            }
            throw failure;
        }
        return reader.overSource(new StreamSource(utf8, bufferBytes), bufferBytes)
                .begin(dialect);
    }

    /**
     * Opens a file of UTF-8 delimited text, {@value #DEFAULT_BATCH_ROWS} rows a batch
     * through a {@value #DEFAULT_BUFFER_BYTES}-byte buffer.
     *
     * @param path the file
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the file cannot be opened or read
     * @see #open(Path, Dialect, List, int, int)
     */
    public static DelimitedReader open(Path path, Dialect dialect, List<Column> plan) {
        return open(path, dialect, plan, DEFAULT_BATCH_ROWS, DEFAULT_BUFFER_BYTES);
    }

    /**
     * Opens a file of UTF-8 delimited text. The file is read through its channel straight
     * into the reader's native buffer.
     *
     * @param path the file
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @param batchRows rows per batch
     * @param bufferBytes the initial read buffer; it doubles when a record does not fit
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured, or
     *     {@code batchRows} or {@code bufferBytes} is not positive
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the file cannot be opened or read
     */
    public static DelimitedReader open(Path path, Dialect dialect, List<Column> plan, int batchRows, int bufferBytes) {
        Objects.requireNonNull(plan, "plan");
        return openFile(path, dialect, plan, batchRows, bufferBytes);
    }

    /**
     * Opens a file of UTF-8 delimited text through a {@value #DEFAULT_BUFFER_BYTES}-byte
     * buffer, and reads its header, leaving the plan to {@link #bind(List, int)}.
     *
     * @param path the file
     * @param dialect the declared dialect
     * @return the reader, its header already read when the dialect declares one, and no plan
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the file cannot be opened or read
     * @see #open(Path, Dialect, int)
     */
    public static DelimitedReader open(Path path, Dialect dialect) {
        return open(path, dialect, DEFAULT_BUFFER_BYTES);
    }

    /**
     * Opens a file of UTF-8 delimited text, and reads its header, leaving the plan to
     * {@link #bind(List, int)}.
     *
     * @param path the file
     * @param dialect the declared dialect
     * @param bufferBytes the initial read buffer; it doubles when a record does not fit
     * @return the reader, its header already read when the dialect declares one, and no plan
     * @throws IllegalArgumentException if the dialect's separator cannot be honoured, or
     *     {@code bufferBytes} is not positive
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the file cannot be opened or read
     */
    public static DelimitedReader open(Path path, Dialect dialect, int bufferBytes) {
        return openFile(path, dialect, null, 0, bufferBytes);
    }

    private static DelimitedReader openFile(
            Path path, Dialect dialect, List<Column> plan, int batchRows, int bufferBytes) {
        Objects.requireNonNull(path, "path");
        if (bufferBytes <= 0) {
            throw new IllegalArgumentException("bufferBytes must be positive; got " + bufferBytes);
        }
        DelimitedReader reader =
                new DelimitedReader(dialect, plan == null ? null : Columns.checked(plan, batchRows), batchRows);
        FileChannel channel;
        try {
            channel = FileChannel.open(path, StandardOpenOption.READ);
        } catch (IOException e) {
            reader.close();
            throw new UncheckedIOException(e);
        } catch (RuntimeException | Error failure) {
            reader.abandon(failure);
            throw failure;
        }
        return reader.overSource(new FileSource(channel), bufferBytes).begin(dialect);
    }

    /**
     * Makes a reader of {@code dialect}, bound to {@code plan} — already checked, so that a
     * plan the reader cannot take is refused before the header is read — or with no plan
     * when it is {@code null}.
     */
    private DelimitedReader(Dialect dialect, Column[] plan, int batchRows) {
        this.dialect = Objects.requireNonNull(dialect, "dialect");
        if (dialect.separator() > 0x7E) {
            throw new IllegalArgumentException(
                    String.format("Separator U+%04X is not a single ASCII byte.", (int) dialect.separator()));
        }

        // Asked of the library before anything is allocated: this is the call that loads
        // it, and a library that will not load must not leave an arena behind.
        long stateBytes = Native.stateSize();
        if (stateBytes < Native.STATE_READ_BYTES) {
            throw new IllegalStateException("libhypertabular's state block is " + stateBytes
                    + " bytes; this binding reads " + Native.STATE_READ_BYTES + " of it.");
        }
        this.arena = Arena.ofConfined();
        try {
            state = arena.allocate(stateBytes, 8);
            filled = arena.allocate(Native.FILLED_BYTES, 8);
            buffers = arena.allocate(Native.BUFFERS_BYTES, 8);
            unescaped = new Block(4096, 1);

            MemorySegment raw = arena.allocate(Native.DIALECT_BYTES, 1);
            raw.set(BYTE, 0, (byte) dialect.separator());
            raw.set(BYTE, 1, (byte) (dialect.quoting() ? 1 : 0));
            raw.set(BYTE, 2, (byte) (dialect.skipBlankLines() ? 1 : 0));
            raw.set(BYTE, 3, (byte) 0);
            if (Native.init(state, raw) != Native.OK) {
                throw new IllegalArgumentException(
                        "Separator '" + dialect.separator() + "' is not tab or printable ASCII other than '\"'.");
            }
            if (plan != null) {
                attach(plan, batchRows);
            }
        } catch (RuntimeException | Error failure) {
            release();
            throw failure;
        }
    }

    /** Sizes everything a batch is read into by the plan's columns. */
    private void attach(Column[] plan, int batchRows) {
        Columns bound = new Columns(plan, batchRows, arena);
        int width = bound.width + 1;
        Block table = new Block(Math.multiplyExact((long) width * batchRows, Native.SPAN_BYTES), 4);
        perRow = width;
        cells = table;
        batch = new Batch(bound, false);
        columns = bound;
    }

    /**
     * Declares the plan a reader opened without one reads through,
     * {@value #DEFAULT_BATCH_ROWS} rows a batch.
     *
     * @param plan the output columns, in output order
     * @throws IllegalStateException if the reader already has a plan, or is closed
     * @throws IllegalArgumentException if a column cannot be honoured; the reader is left
     *     without a plan
     * @see #bind(List, int)
     */
    public void bind(List<Column> plan) {
        bind(plan, DEFAULT_BATCH_ROWS);
    }

    /**
     * Declares the plan a reader opened without one reads through — once, before the first
     * read, and usually after {@link #header()} has said where each column is
     * ({@code Column.i32(reader.header().ordinal("id"))}). Every column array, the cell table
     * and the batch are sized here, by the plan and {@code batchRows}.
     *
     * @param plan the output columns, in output order
     * @param batchRows rows per batch
     * @throws IllegalStateException if the reader already has a plan, or is closed
     * @throws IllegalArgumentException if a column cannot be honoured, or {@code batchRows}
     *     is not positive; the reader is left without a plan
     */
    public void bind(List<Column> plan, int batchRows) {
        if (closed) {
            throw new IllegalStateException("The reader is closed.");
        }
        if (columns != null) {
            throw new IllegalStateException("The reader already has a plan; a plan is bound once.");
        }
        attach(Columns.checked(plan, batchRows), batchRows);
    }

    /**
     * Whether a plan has been bound: always, for a reader opened with one.
     *
     * @return {@code true} once the reader has a plan
     */
    public boolean isBound() {
        return columns != null;
    }

    private DelimitedReader overMemory(MemorySegment text, long windowBytes) {
        memory = text;
        memoryReadOnly = text.asReadOnly();
        maxWindow = windowBytes;
        end = text.byteSize();
        eof = true;
        return this;
    }

    /** Takes the source over: from here it is closed with the reader, on failure included. */
    private DelimitedReader overSource(Source from, int bufferBytes) {
        source = from;
        try {
            buffer = new Block(Math.min(bufferBytes, MAX_ROW_BYTES), 1);
        } catch (RuntimeException | Error failure) {
            abandon(failure);
            throw failure;
        }
        return this;
    }

    /** Closes a reader that is not going to be handed out, keeping the failure that ended it. */
    private void abandon(Throwable failure) {
        try {
            close();
        } catch (RuntimeException | Error closing) {
            failure.addSuppressed(closing);
        }
    }

    private DelimitedReader begin(Dialect dialect) {
        try {
            if (dialect.hasHeader()) {
                readHeader();
            }
            return this;
        } catch (RuntimeException | Error failure) {
            abandon(failure);
            throw failure;
        }
    }

    /**
     * The header's names, when the dialect declares one — read when the reader is opened, so
     * they are to hand before the plan is bound: unmodifiable, and empty for an input with no
     * record. {@code null} when the dialect declares no header.
     *
     * @return the header's names, or {@code null}
     */
    public Header header() {
        return header;
    }

    /**
     * How many cells a record has: the header's count once it has been read, otherwise the
     * first record's once it has been; empty until then.
     *
     * @return the record width, or empty
     */
    public OptionalInt columnCount() {
        int expected = closed ? expectedAtClose : state.get(INT, Native.STATE_EXPECTED);
        return expected == 0 ? OptionalInt.empty() : OptionalInt.of(expected);
    }

    /**
     * The dialect the reader was built with.
     *
     * @return the dialect
     */
    public Dialect dialect() {
        return dialect;
    }

    /**
     * The plan the reader reads through: column {@code i} of every batch is
     * {@code plan().get(i)}. Empty until one is bound.
     *
     * @return the plan, unmodifiable
     */
    public List<Column> plan() {
        return columns == null ? List.of() : columns.planList;
    }

    /**
     * Records finished so far — the header and skipped blank lines included.
     *
     * @return the record count
     */
    public long records() {
        return closed ? recordsAtClose : state.get(LONG, Native.STATE_RECORDS);
    }

    /** The memory the core reads next from: the caller's, or the read buffer. */
    private MemorySegment base() {
        return source == null ? memory : buffer.segment;
    }

    private TabularException tooLong() {
        return failure = new TabularException(
                TabularFailure.ROW_TOO_LONG,
                state.get(LONG, Native.STATE_RECORDS),
                state.get(INT, Native.STATE_LINE),
                state.get(LONG, Native.STATE_OFFSET),
                0,
                0);
    }

    /** Puts the unfinished record at the front of the buffer and reads more behind it. */
    private void refill() {
        if (source == null) {
            // Memory is read a window at a time, and a record that does not fit the largest
            // window there is cannot be read at all.
            throw tooLong();
        }
        long pending = end - start;
        if (start > 0) {
            MemorySegment.copy(buffer.segment, start, buffer.segment, 0, pending);
            start = 0;
            end = pending;
        }
        if (end == buffer.bytes()) {
            // One record fills the buffer: it needs a bigger one.
            if (buffer.bytes() >= MAX_ROW_BYTES) {
                throw tooLong();
            }
            buffer.resize(Math.min(buffer.bytes() * 2, MAX_ROW_BYTES), end);
        }
        int read;
        try {
            read = source.read(buffer.segment, end, buffer.bytes() - end);
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
        if (read < 0) {
            eof = true;
        } else {
            end += read;
        }
    }

    private TabularException structural() {
        return failure = TabularException.from(filled, Native.FAILURE_CODE);
    }

    private void readHeader() {
        // The name table is the header's alone, and gone when it has been read.
        try (Arena scope = Arena.ofConfined()) {
            long capacity = 64;
            MemorySegment names = scope.allocate(capacity * Native.SPAN_BYTES, 4);
            while (true) {
                long length = Math.min(end - start, maxWindow);
                boolean last = eof && start + length == end;
                if (length == 0 && !last) {
                    refill();
                    continue;
                }
                MemorySegment base = base();
                int code = Native.header(
                        state,
                        base.asSlice(start, length),
                        length,
                        last,
                        names,
                        capacity,
                        unescaped.segment,
                        unescaped.bytes(),
                        filled);
                switch (code) {
                    case Native.OK -> {
                        long consumed = filled.get(LONG, Native.FILLED_CONSUMED);
                        int count = (int) filled.get(LONG, Native.FILLED_ROWS);
                        if (count > 0) {
                            Header.Builder found = new Header.Builder(count);
                            for (int index = 0; index < count; index++) {
                                int offset = names.get(INT, index * Native.SPAN_BYTES);
                                int span = names.get(INT, index * Native.SPAN_BYTES + 4);
                                // A flagged name had "" inside, and is in the arena, unescaped.
                                found.add(
                                        span < 0
                                                ? unescaped.segment.asSlice(offset, span & Native.SPAN_LENGTH)
                                                : base.asSlice(start + offset, span));
                            }
                            header = found.build();
                            start += consumed;
                            return;
                        }
                        start += consumed;
                        if (last && (consumed == length || consumed == 0)) {
                            // An empty input has no header and no rows; the width is unknown.
                            header = Header.EMPTY;
                            return;
                        }
                        if (consumed == 0) {
                            refill();
                        }
                    }
                    case Native.ERR_CELLS -> {
                        capacity = filled.get(LONG, Native.FILLED_NEEDED);
                        names = scope.allocate(Math.multiplyExact(capacity, Native.SPAN_BYTES), 4);
                    }
                    case Native.ERR_ARENA -> unescaped.resize(filled.get(LONG, Native.FILLED_NEEDED), 0);
                    case Native.ERR_STRUCTURE -> throw structural();
                    default -> throw new IllegalStateException(CONTRACT_VIOLATION);
                }
            }
        }
    }

    /**
     * Reads the next batch: up to the reader's batch size of rows, or {@code null} once the
     * input is exhausted. The batch is valid until the next call.
     *
     * @return the batch, or {@code null}
     * @throws TabularException if the input is structurally broken — raised after every
     *     intact row before the break has been delivered, and again, the same exception, on
     *     every later call
     * @throws UncheckedIOException if the stream or file fails
     * @throws IllegalStateException if the reader is closed, or no plan has been bound — not
     *     final: bind one and read
     */
    public Batch read() {
        if (closed) {
            throw new IllegalStateException("The reader is closed.");
        }
        if (columns == null) {
            throw new IllegalStateException("The reader has no plan yet: bind one before reading.");
        }
        batch.clear();
        if (failure != null) {
            throw failure;
        }
        // The core stops a batch at the row the arena has no room for, so an arena too small
        // for a batch's unescaped text makes for short batches, not for an error. The batch
        // that was short has been given back by now, and the arena can move.
        if (cramped) {
            cramped = false;
            unescaped.resize(unescaped.bytes() * 2, 0);
        }
        int batchRows = columns.batchRows;
        while (true) {
            long length = Math.min(end - start, maxWindow);
            boolean last = eof && start + length == end;
            if (length == 0 && !last) {
                // Nothing buffered yet and more to come: nothing for the core to look at.
                refill();
                continue;
            }
            MemorySegment base = base();
            // The cells and the arena may have moved since the last call.
            buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_CELLS, cells.segment);
            buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_CELLS + 8, cells.bytes() / Native.SPAN_BYTES);
            buffers.set(ValueLayout.ADDRESS, Native.BUFFERS_ARENA, unescaped.segment);
            buffers.set(ValueLayout.JAVA_LONG, Native.BUFFERS_ARENA + 8, unescaped.bytes());
            int code = Native.fill(
                    state,
                    base.asSlice(start, length),
                    length,
                    last,
                    columns.specs,
                    columns.buffers,
                    columns.plan.length,
                    batchRows,
                    buffers,
                    filled);
            switch (code) {
                case Native.OK -> {
                    long consumed = filled.get(LONG, Native.FILLED_CONSUMED);
                    long count = filled.get(LONG, Native.FILLED_ROWS);
                    long at = start;
                    start += consumed;
                    if (count > 0) {
                        int rows = (int) count;
                        cramped =
                                rows < batchRows && filled.get(LONG, Native.FILLED_ARENA_USED) * 2 >= unescaped.bytes();
                        return batch.fill(rows, cells.segment, perRow, base, at, unescaped.segment);
                    }
                    if (last && (consumed == length || consumed == 0)) {
                        return null;
                    }
                    if (consumed == 0) {
                        refill();
                    }
                }
                // The core says how many entries one row takes; the table holds a batch of them.
                case Native.ERR_CELLS -> {
                    long needed = filled.get(LONG, Native.FILLED_NEEDED);
                    perRow = (int) Math.max(perRow, needed);
                    cells.resize(Math.multiplyExact(Math.multiplyExact(needed, batchRows), Native.SPAN_BYTES), 0);
                }
                case Native.ERR_ARENA ->
                    unescaped.resize(Math.max(filled.get(LONG, Native.FILLED_NEEDED), unescaped.bytes() * 2), 0);
                case Native.ERR_STRUCTURE -> throw structural();
                default -> throw new IllegalStateException(CONTRACT_VIOLATION);
            }
        }
    }

    /**
     * Reads every row left, batch by batch, handing each to {@code action} — the row view of a
     * reader, whose batches are over when the next is read. Each {@link Row} is a view of the
     * current batch, valid only for its own call: do not keep one. A failure, the reader's or
     * {@code action}'s, ends the loop and is thrown from here.
     *
     * {@snippet :
     * reader.forEachRow(row -> orders.add(new Order(row.get(0, Integer.class), row.string(1))));
     * }
     *
     * @param action what is done with each row
     * @throws TabularException if the input is structurally broken, after every intact row
     *     before the break has been handed over
     * @throws UncheckedIOException if the stream or file fails
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

    private void release() {
        if (batch != null) {
            batch.close();
        }
        if (cells != null) {
            cells.close();
        }
        if (unescaped != null) {
            unescaped.close();
        }
        if (buffer != null) {
            buffer.close();
        }
        arena.close();
    }

    /**
     * Releases the native memory, and closes the stream or file the reader was built over.
     * Closing twice is harmless.
     *
     * @throws UncheckedIOException if closing the stream or file fails
     */
    @Override
    public void close() {
        if (closed) {
            return;
        }
        recordsAtClose = state.get(LONG, Native.STATE_RECORDS);
        expectedAtClose = state.get(INT, Native.STATE_EXPECTED);
        closed = true;
        release();
        if (source != null) {
            try {
                source.close();
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            }
        }
    }
}
