package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.Success;
import io.github.skunkwerkx.hypercast.Verdict;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.math.BigDecimal;
import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.time.Duration;
import java.time.Instant;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.LocalTime;
import java.util.List;
import java.util.Objects;
import java.util.UUID;

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
 * <p>A value that does not cast is that cell's verdict, and the read goes on. Input that is
 * not rows of cells at all — a record of the wrong width, a quote never closed — is a
 * {@link TabularException}, raised after every intact row before it has been delivered.
 *
 * {@snippet :
 * List<Column> plan = List.of(Column.i32(0), Column.text(1), Column.f64(2));
 * try (DelimitedReader reader = DelimitedReader.open(Path.of("orders.csv"), Dialect.CSV, plan)) {
 *     while (reader.read()) {
 *         for (int row = 0; row < reader.rows(); row++) {
 *             String line = switch (reader.f64(2, row)) {
 *                 case Success<Double> score -> reader.string(1, row) + ": " + score.value();
 *                 case Fault<Double> fault -> fault.reason() + " in \"" + reader.rawString(2, row) + "\"";
 *             };
 *         }
 *     }
 * }
 * }
 *
 * <p>Segments handed out for a batch — {@link #values}, {@link #verdicts}, {@link #text},
 * {@link #raw} — are read-only views into the reader's own buffers: valid until the next
 * {@link #read()}, and refused by the runtime after {@link #close()}. A failure of the
 * underlying stream or file is an {@link UncheckedIOException}, from the factory or from
 * {@link #read()}.
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

    /**
     * Native memory that can be outgrown: its own confined arena, closed when a larger one
     * replaces it, so a buffer that doubled ten times holds one allocation, not eleven.
     */
    private static final class Block {
        private final long alignment;
        private Arena arena;
        MemorySegment segment;
        MemorySegment readOnly;

        Block(long bytes, long alignment) {
            this.alignment = alignment;
            this.arena = Arena.ofConfined();
            try {
                this.segment = arena.allocate(bytes, alignment);
            } catch (RuntimeException | Error failure) {
                arena.close();
                throw failure;
            }
            this.readOnly = segment.asReadOnly();
        }

        long bytes() {
            return segment.byteSize();
        }

        /** Replaces the memory with {@code bytes} of it, the first {@code keep} carried over. */
        void resize(long bytes, long keep) {
            Arena next = Arena.ofConfined();
            MemorySegment larger;
            try {
                larger = next.allocate(bytes, alignment);
                MemorySegment.copy(segment, 0, larger, 0, keep);
            } catch (RuntimeException | Error failure) {
                next.close();
                throw failure;
            }
            arena.close();
            arena = next;
            segment = larger;
            readOnly = larger.asReadOnly();
        }

        void close() {
            arena.close();
        }
    }

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
            ByteBuffer view = target.asSlice(at, Math.min(room, Integer.MAX_VALUE)).asByteBuffer();
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

    private final Column[] plan;
    private final int batchRows;
    /** Cell-table entries one row takes: the widest ordinal the plan reads, plus two. */
    private final int perRow;

    // Everything the core is handed, allocated once: the fixed-size blocks in one arena,
    // and each buffer that can be outgrown in its own.
    private final Arena arena;
    private MemorySegment state;
    private MemorySegment specs;
    private MemorySegment buffers;
    private MemorySegment filled;
    private MemorySegment[] values;
    private MemorySegment[] verdicts;
    private Block cells;
    private Block unescaped;
    private Block scratch;

    // The source: a stream or file read into `buffer`, or memory read in place.
    private Source source;
    private Block buffer;
    private MemorySegment memory;
    private MemorySegment memoryReadOnly;
    private long maxWindow = MAX_WINDOW_BYTES;
    private long start;
    private long end;
    private boolean eof;

    // The batch in hand, and the window of input its spans point into.
    private MemorySegment window;
    private MemorySegment windowReadOnly;
    private long windowStart;
    private int rows;

    private List<String> header;
    private TabularException failure;
    private boolean closed;
    private long recordsAtClose;

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
        return windowed(utf8, dialect, plan, batchRows, MAX_WINDOW_BYTES);
    }

    /**
     * {@link #of(MemorySegment, Dialect, List, int)}, with a stated limit on how much of the
     * memory one native call is handed. The limit is otherwise the ABI's own, just under
     * 2 GiB: memory larger than that is read a window at a time, and stating a small one is
     * how the suite reaches that path without two gibibytes of input.
     */
    static DelimitedReader windowed(
            MemorySegment utf8, Dialect dialect, List<Column> plan, int batchRows, long windowBytes) {
        Objects.requireNonNull(utf8, "utf8");
        DelimitedReader reader = new DelimitedReader(dialect, plan, batchRows);
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
        Objects.requireNonNull(utf8, "utf8");
        DelimitedReader reader;
        try {
            if (bufferBytes <= 0) {
                throw new IllegalArgumentException("bufferBytes must be positive; got " + bufferBytes);
            }
            reader = new DelimitedReader(dialect, plan, batchRows);
        } catch (RuntimeException | Error failure) {
            // No reader came to be, so the stream it would have owned is closed here.
            try {
                utf8.close();
            } catch (IOException | RuntimeException closing) {
                failure.addSuppressed(closing);
            }
            throw failure;
        }
        return reader.overSource(new StreamSource(utf8, bufferBytes), bufferBytes).begin(dialect);
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
    public static DelimitedReader open(
            Path path, Dialect dialect, List<Column> plan, int batchRows, int bufferBytes) {
        Objects.requireNonNull(path, "path");
        if (bufferBytes <= 0) {
            throw new IllegalArgumentException("bufferBytes must be positive; got " + bufferBytes);
        }
        DelimitedReader reader = new DelimitedReader(dialect, plan, batchRows);
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

    private DelimitedReader(Dialect dialect, List<Column> plan, int batchRows) {
        Objects.requireNonNull(dialect, "dialect");
        this.plan = Objects.requireNonNull(plan, "plan").toArray(Column[]::new);
        if (batchRows <= 0) {
            throw new IllegalArgumentException("batchRows must be positive; got " + batchRows);
        }
        if (dialect.separator() > 0x7E) {
            throw new IllegalArgumentException(String.format(
                    "Separator U+%04X is not a single ASCII byte.", (int) dialect.separator()));
        }
        this.batchRows = batchRows;
        int widest = -1;
        for (int index = 0; index < this.plan.length; index++) {
            Column column = Objects.requireNonNull(this.plan[index], "plan column " + index);
            widest = Math.max(widest, column.ordinal());
        }
        if (widest > Integer.MAX_VALUE - 2) {
            throw new IllegalArgumentException("A plan ordinal of " + widest + " is out of range.");
        }
        this.perRow = widest + 2;

        // Asked of the library before anything is allocated: this is the call that loads
        // it, and a library that will not load must not leave an arena behind.
        long stateBytes = Native.stateSize();
        if (stateBytes < Native.STATE_READ_BYTES) {
            throw new IllegalStateException("libhypertabular's state block is " + stateBytes
                    + " bytes; this binding reads " + Native.STATE_READ_BYTES + " of it.");
        }
        this.arena = Arena.ofConfined();
        try {
            int count = this.plan.length;
            state = arena.allocate(stateBytes, 8);
            filled = arena.allocate(Native.FILLED_BYTES, 8);
            specs = arena.allocate(Math.max(count, 1) * Native.SPEC_BYTES, 4);
            buffers = arena.allocate(Math.max(count, 1) * Native.BUFFER_BYTES, 8);
            values = new MemorySegment[count];
            verdicts = new MemorySegment[count];
            for (int index = 0; index < count; index++) {
                Column column = this.plan[index];
                long spec = index * Native.SPEC_BYTES;
                specs.set(INT, spec + Native.SPEC_ORDINAL, column.ordinal());
                specs.set(INT, spec + Native.SPEC_DOOR, column.door().code());
                specs.set(INT, spec + Native.SPEC_PARAM, column.declared());
                NumFormat format = column.format();
                byte[] symbol = format.currencySymbol().getBytes(StandardCharsets.UTF_8);
                specs.set(INT, spec + Native.SPEC_DECIMAL_SEP, format.decimalSeparator());
                specs.set(INT, spec + Native.SPEC_GROUP_SEP, format.groupSeparator());
                specs.set(INT, spec + Native.SPEC_FLAGS, format.styles());
                specs.set(INT, spec + Native.SPEC_CURRENCY_LEN, symbol.length);
                // NumFormat's own constructor holds the symbol to the 16 bytes there are.
                MemorySegment.copy(symbol, 0, specs, BYTE, spec + Native.SPEC_CURRENCY, symbol.length);

                // 8-aligned whatever the door, so every value reads through an aligned layout.
                values[index] = arena.allocate((long) batchRows * column.door().valueBytes(), 8);
                verdicts[index] = arena.allocate(batchRows * Native.VERDICT_BYTES, 4);
                long entry = index * Native.BUFFER_BYTES;
                buffers.set(ValueLayout.ADDRESS, entry, values[index]);
                buffers.set(ValueLayout.ADDRESS, entry + Native.BUFFER_VERDICTS, verdicts[index]);
            }
            cells = new Block(Math.multiplyExact((long) perRow * batchRows, Native.SPAN_BYTES), 4);
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
        } catch (RuntimeException | Error failure) {
            release();
            throw failure;
        }
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
     * The header's names, when the dialect declares one: unmodifiable, and empty for an
     * input with no record. {@code null} when the dialect declares no header.
     *
     * @return the header's names, or {@code null}
     */
    public List<String> header() {
        return header;
    }

    /**
     * The number of plan columns.
     *
     * @return the plan's length
     */
    public int columnCount() {
        return plan.length;
    }

    /**
     * The plan column at {@code column}.
     *
     * @param column index into the plan
     * @return the plan column
     */
    public Column column(int column) {
        return plan[column];
    }

    /**
     * Rows in the batch in hand; {@code 0} before the first {@link #read()} and after the
     * last.
     *
     * @return the batch's row count
     */
    public int rows() {
        return rows;
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
                0, 0);
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
        return failure = new TabularException(
                filled.get(INT, Native.FAILURE_CODE) == 2
                        ? TabularFailure.COLUMN_COUNT
                        : TabularFailure.UNCLOSED_QUOTE,
                filled.get(LONG, Native.FAILURE_RECORD),
                filled.get(INT, Native.FAILURE_LINE),
                filled.get(LONG, Native.FAILURE_BYTE),
                filled.get(INT, Native.FAILURE_EXPECTED),
                filled.get(INT, Native.FAILURE_FOUND));
    }

    private static String decode(MemorySegment from, long offset, int length) {
        byte[] bytes = new byte[length];
        MemorySegment.copy(from, BYTE, offset, bytes, 0, length);
        return new String(bytes, StandardCharsets.UTF_8);
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
                        state, base.asSlice(start, length), length, last,
                        names, capacity, unescaped.segment, unescaped.bytes(), filled);
                switch (code) {
                    case Native.OK -> {
                        long consumed = filled.get(LONG, Native.FILLED_CONSUMED);
                        int count = (int) filled.get(LONG, Native.FILLED_ROWS);
                        if (count > 0) {
                            String[] found = new String[count];
                            for (int index = 0; index < count; index++) {
                                int offset = names.get(INT, index * Native.SPAN_BYTES);
                                int span = names.get(INT, index * Native.SPAN_BYTES + 4);
                                // A flagged name had "" inside, and is in the arena, unescaped.
                                found[index] = span < 0
                                        ? decode(unescaped.segment, offset, span & Native.SPAN_LENGTH)
                                        : decode(base, start + offset, span);
                            }
                            header = List.of(found);
                            start += consumed;
                            return;
                        }
                        start += consumed;
                        if (last && (consumed == length || consumed == 0)) {
                            // An empty input has no header and no rows; the width is unknown.
                            header = List.of();
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
     * Reads the next batch. {@code true} with {@link #rows()} rows in hand; {@code false}
     * once the input is exhausted.
     *
     * @return whether a batch was read
     * @throws TabularException if the input is structurally broken — raised after every
     *     intact row before the break has been delivered, and again, the same exception, on
     *     every later call
     * @throws UncheckedIOException if the stream or file fails
     * @throws IllegalStateException if the reader is closed
     */
    public boolean read() {
        if (closed) {
            throw new IllegalStateException("The reader is closed.");
        }
        if (failure != null) {
            throw failure;
        }
        rows = 0;
        while (true) {
            long length = Math.min(end - start, maxWindow);
            boolean last = eof && start + length == end;
            if (length == 0 && !last) {
                // Nothing buffered yet and more to come: nothing for the core to look at.
                refill();
                continue;
            }
            MemorySegment base = base();
            int code = Native.fill(
                    state, base.asSlice(start, length), length, last,
                    specs, buffers, plan.length, batchRows,
                    cells.segment, cells.bytes() / Native.SPAN_BYTES,
                    unescaped.segment, unescaped.bytes(), filled);
            switch (code) {
                case Native.OK -> {
                    long consumed = filled.get(LONG, Native.FILLED_CONSUMED);
                    long count = filled.get(LONG, Native.FILLED_ROWS);
                    long at = start;
                    start += consumed;
                    if (count > 0) {
                        rows = (int) count;
                        window = base;
                        windowReadOnly = source == null ? memoryReadOnly : buffer.readOnly;
                        windowStart = at;
                        return true;
                    }
                    if (last && (consumed == length || consumed == 0)) {
                        return false;
                    }
                    if (consumed == 0) {
                        refill();
                    }
                }
                case Native.ERR_CELLS -> cells.resize(
                        Math.multiplyExact(
                                Math.multiplyExact(filled.get(LONG, Native.FILLED_NEEDED), batchRows),
                                Native.SPAN_BYTES),
                        0);
                case Native.ERR_ARENA -> unescaped.resize(
                        Math.max(filled.get(LONG, Native.FILLED_NEEDED), unescaped.bytes() * 2), 0);
                case Native.ERR_STRUCTURE -> throw structural();
                default -> throw new IllegalStateException(CONTRACT_VIOLATION);
            }
        }
    }

    /** The reason code of the cell at ({@code column}, {@code row}): {@code 0} when it cast. */
    private int code(int column, int row) {
        Objects.checkIndex(row, rows);
        return verdicts[column].get(INT, row * Native.VERDICT_BYTES + 8);
    }

    private static CastFailure reason(int code) {
        return switch (code) {
            case 1 -> CastFailure.EMPTY;
            case 2 -> CastFailure.MALFORMED;
            case 3 -> CastFailure.OUT_OF_RANGE;
            default -> throw new IllegalStateException("libhypertabular returned unknown verdict code " + code);
        };
    }

    private IllegalStateException wrongDoor(int column, String asked) {
        return new IllegalStateException(
                "Column " + column + " is cast through " + plan[column].door() + ", not " + asked + ".");
    }

    /** Whether the cell cast, once the column is known to be one {@code door} reads. */
    private boolean cast(int column, int row, Door door) {
        if (plan[column].door() != door) {
            throw wrongDoor(column, door.toString());
        }
        return code(column, row) == 0;
    }

    private <T> Fault<T> fault(int column, int row) {
        MemorySegment verdict = verdicts[column];
        long at = row * Native.VERDICT_BYTES;
        return new Fault<>(reason(verdict.get(INT, at + 8)), verdict.get(INT, at), verdict.get(INT, at + 4));
    }

    /**
     * Whether the cell at ({@code column}, {@code row}) cast, whatever its door — the
     * allocation-free question.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return {@code true} when the cell cast
     */
    public boolean isOk(int column, int row) {
        return code(column, row) == 0;
    }

    /**
     * The verdict of the cell at ({@code column}, {@code row}), whatever its door: whether
     * it cast, and if not, why and where in the cell's own text.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the cell's verdict
     */
    public CellVerdict verdict(int column, int row) {
        int code = code(column, row);
        if (code == 0) {
            return new CellVerdict(null, 0, 0);
        }
        MemorySegment verdict = verdicts[column];
        long at = row * Native.VERDICT_BYTES;
        return new CellVerdict(reason(code), verdict.get(INT, at), verdict.get(INT, at + 4));
    }

    /**
     * A column's verdicts for the batch in hand, exactly as the core wrote them: one
     * {@link CellVerdict#LAYOUT} entry per row. Read-only, and valid until the next
     * {@link #read()}.
     *
     * @param column index into the plan
     * @return the column's verdict array
     */
    public MemorySegment verdicts(int column) {
        return verdicts[column].asSlice(0, rows * Native.VERDICT_BYTES).asReadOnly();
    }

    /**
     * A column's values for the batch in hand, one per row, exactly as the core wrote them
     * — for the doors whose value is a primitive: {@link Door#BOOL} as one byte ({@code 0}
     * or {@code 1}), the integer and floating-point doors as their own width, read with the
     * matching {@link ValueLayout} ({@code values.getAtIndex(ValueLayout.JAVA_INT, row)}
     * for {@link Door#I32} and {@link Door#U32}, and so on). The value of a row whose
     * verdict is not ok is zero. Columns of the other doors are read a cell at a time.
     * Read-only, and valid until the next {@link #read()}.
     *
     * @param column index into the plan
     * @return the column's value array
     * @throws IllegalStateException if the column's door does not write a primitive
     */
    public MemorySegment values(int column) {
        Door door = plan[column].door();
        if (!door.isPrimitive()) {
            throw new IllegalStateException(
                    "Column " + column + " is cast through " + door + "; it has no array of primitives.");
        }
        return values[column].asSlice(0, (long) rows * door.valueBytes()).asReadOnly();
    }

    /**
     * The cell of a {@link Door#BOOL} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Boolean> bool(int column, int row) {
        return cast(column, row, Door.BOOL)
                ? new Success<>(values[column].get(BYTE, row) != 0)
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#I8} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Byte> i8(int column, int row) {
        return cast(column, row, Door.I8)
                ? new Success<>(values[column].get(BYTE, row))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#I16} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Short> i16(int column, int row) {
        return cast(column, row, Door.I16)
                ? new Success<>(values[column].get(SHORT, row * 2L))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#I32} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Integer> i32(int column, int row) {
        return cast(column, row, Door.I32)
                ? new Success<>(values[column].get(INT, row * 4L))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#I64} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Long> i64(int column, int row) {
        return cast(column, row, Door.I64)
                ? new Success<>(values[column].get(LONG, row * 8L))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#U8} column, widened to an {@code int}.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Integer> u8(int column, int row) {
        return cast(column, row, Door.U8)
                ? new Success<>(Byte.toUnsignedInt(values[column].get(BYTE, row)))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#U16} column, widened to an {@code int}.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Integer> u16(int column, int row) {
        return cast(column, row, Door.U16)
                ? new Success<>(Short.toUnsignedInt(values[column].get(SHORT, row * 2L)))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#U32} column, widened to a {@code long}.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Long> u32(int column, int row) {
        return cast(column, row, Door.U32)
                ? new Success<>(Integer.toUnsignedLong(values[column].get(INT, row * 4L)))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#U64} column: the {@code long} with the same bits, to be
     * read with {@link Long#toUnsignedString(long)} and the other {@code Long} unsigned
     * methods.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Long> u64(int column, int row) {
        return cast(column, row, Door.U64)
                ? new Success<>(values[column].get(LONG, row * 8L))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#F32} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Float> f32(int column, int row) {
        return cast(column, row, Door.F32)
                ? new Success<>(values[column].get(ValueLayout.JAVA_FLOAT, row * 4L))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#F64} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Double> f64(int column, int row) {
        return cast(column, row, Door.F64)
                ? new Success<>(values[column].get(ValueLayout.JAVA_DOUBLE, row * 8L))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#DECIMAL} column, exact: built straight from the core's
     * sign, 96-bit magnitude and scale, with no {@code double} formed on the way.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<BigDecimal> decimal(int column, int row) {
        if (!cast(column, row, Door.DECIMAL)) {
            return fault(column, row);
        }
        // {u64 lo, u32 hi, u8 scale, u8 negative}. BigInteger takes its magnitude
        // big-endian, so the two words are laid out high word first; the core never hands
        // back a negative zero, so the signum needs no zero check.
        MemorySegment value = values[column];
        long at = row * 16L;
        byte[] magnitude = new byte[12];
        ByteBuffer.wrap(magnitude).putInt(value.get(INT, at + 8)).putLong(value.get(LONG, at));
        int signum = value.get(BYTE, at + 13) != 0 ? -1 : 1;
        return new Success<>(new BigDecimal(new BigInteger(signum, magnitude), value.get(BYTE, at + 12)));
    }

    /**
     * The cell of a {@link Door#UUID} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<UUID> uuid(int column, int row) {
        if (!cast(column, row, Door.UUID)) {
            return fault(column, row);
        }
        // RFC 9562 byte order is exactly UUID's msb/lsb decomposition: two big-endian longs.
        MemorySegment value = values[column];
        long at = row * 16L;
        return new Success<>(new UUID(
                value.get(Native.Downcalls.BIG_ENDIAN_LONG, at),
                value.get(Native.Downcalls.BIG_ENDIAN_LONG, at + 8)));
    }

    /**
     * The cell of a {@link Door#TIMESTAMP}, {@link Door#UNIX} or {@link Door#EXCEL_SERIAL}
     * column: an instant, at the full nanosecond fidelity the core parsed.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Instant> timestamp(int column, int row) {
        Door door = plan[column].door();
        if (door != Door.TIMESTAMP && door != Door.UNIX && door != Door.EXCEL_SERIAL) {
            throw wrongDoor(column, "TIMESTAMP, UNIX or EXCEL_SERIAL");
        }
        if (code(column, row) != 0) {
            return fault(column, row);
        }
        MemorySegment value = values[column];
        long at = row * 16L;
        return new Success<>(Instant.ofEpochSecond(value.get(LONG, at), value.get(INT, at + 8)));
    }

    private LocalDate day(int column, long at) {
        MemorySegment value = values[column];
        return LocalDate.of(
                Short.toUnsignedInt(value.get(SHORT, at)), value.get(BYTE, at + 2), value.get(BYTE, at + 3));
    }

    /**
     * The cell of a {@link Door#DATE} or {@link Door#DATE_ORDERED} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<LocalDate> date(int column, int row) {
        Door door = plan[column].door();
        if (door != Door.DATE && door != Door.DATE_ORDERED) {
            throw wrongDoor(column, "DATE or DATE_ORDERED");
        }
        return code(column, row) == 0 ? new Success<>(day(column, row * 4L)) : fault(column, row);
    }

    /**
     * The cell of a {@link Door#DATETIME} column: a wall clock with no zone — the text
     * named none and none is invented.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<LocalDateTime> dateTime(int column, int row) {
        if (!cast(column, row, Door.DATETIME)) {
            return fault(column, row);
        }
        long at = row * 16L;
        return new Success<>(LocalDateTime.of(
                day(column, at), LocalTime.ofNanoOfDay(values[column].get(LONG, at + 8))));
    }

    /**
     * The cell of a {@link Door#TIME} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<LocalTime> time(int column, int row) {
        return cast(column, row, Door.TIME)
                ? new Success<>(LocalTime.ofNanoOfDay(values[column].get(LONG, row * 8L)))
                : fault(column, row);
    }

    /**
     * The cell of a {@link Door#DURATION} column.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the value, or the fault
     */
    public Verdict<Duration> duration(int column, int row) {
        if (!cast(column, row, Door.DURATION)) {
            return fault(column, row);
        }
        // Duration.ofSeconds normalizes the core's same-signed nanos adjustment correctly.
        MemorySegment value = values[column];
        long at = row * 16L;
        return new Success<>(Duration.ofSeconds(value.get(LONG, at), value.get(INT, at + 8)));
    }

    /**
     * The cell of a {@link Door#TEXT} column: its bytes, untrimmed, quotes resolved, or
     * {@code null} for a cell with no bytes at all — which is the one way text fails. The
     * segment is a read-only view into the reader's buffers — zero-copy for every cell that
     * had no escaped quote in it — and is valid until the next {@link #read()}.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the cell's UTF-8 bytes, or {@code null} for an empty cell
     */
    public MemorySegment text(int column, int row) {
        if (!cast(column, row, Door.TEXT)) {
            return null;
        }
        MemorySegment value = values[column];
        int offset = value.get(INT, row * 8L);
        int span = value.get(INT, row * 8L + 4);
        // Flagged: the bytes are in the arena, unescaped, rather than in the input.
        return span < 0
                ? unescaped.readOnly.asSlice(offset, span & Native.SPAN_LENGTH)
                : windowReadOnly.asSlice(windowStart + offset, span);
    }

    /**
     * The cell of a {@link Door#TEXT} column as a string, or {@code null} for an empty cell.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the cell's text, or {@code null} for an empty cell
     */
    public String string(int column, int row) {
        if (!cast(column, row, Door.TEXT)) {
            return null;
        }
        MemorySegment value = values[column];
        int offset = value.get(INT, row * 8L);
        int span = value.get(INT, row * 8L + 4);
        return span < 0
                ? decode(unescaped.segment, offset, span & Native.SPAN_LENGTH)
                : decode(window, windowStart + offset, span);
    }

    /**
     * The text the cell at ({@code column}, {@code row}) was cast from, whatever its door
     * and whatever its verdict — what a fault's span indexes, and what to show for a value
     * that did not cast. A read-only view, valid until the next call to {@code raw},
     * {@link #rawString} or {@link #read()}.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the cell's UTF-8 bytes
     */
    public MemorySegment raw(int column, int row) {
        Objects.checkIndex(row, rows);
        long entry = ((long) row * perRow + plan[column].ordinal()) * Native.SPAN_BYTES;
        int offset = cells.segment.get(INT, entry);
        int span = cells.segment.get(INT, entry + 4);
        if (span >= 0) {
            return windowReadOnly.asSlice(windowStart + offset, span);
        }
        // A cell with an escaped quote in it: unescaped, as the core cast it.
        int length = span & Native.SPAN_LENGTH;
        if (scratch == null) {
            scratch = new Block(Math.max(length, 256), 1);
        } else if (scratch.bytes() < length) {
            scratch.resize(length, 0);
        }
        long written = Native.unescape(
                window.asSlice(windowStart + offset, length), length, scratch.segment, scratch.bytes());
        return scratch.readOnly.asSlice(0, written);
    }

    /**
     * {@link #raw} as a string: the text the cell was cast from, for a diagnostic.
     *
     * @param column index into the plan
     * @param row row of the batch in hand
     * @return the cell's text
     */
    public String rawString(int column, int row) {
        MemorySegment raw = raw(column, row);
        return decode(raw, 0, (int) raw.byteSize());
    }

    private void release() {
        if (cells != null) {
            cells.close();
        }
        if (unescaped != null) {
            unescaped.close();
        }
        if (scratch != null) {
            scratch.close();
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
        closed = true;
        rows = 0;
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
