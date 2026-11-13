package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.Verdict;
import java.io.IOException;
import java.io.InputStream;
import java.io.InterruptedIOException;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.ByteBuffer;
import java.nio.channels.ClosedByInterruptException;
import java.nio.channels.FileChannel;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.List;
import java.util.Objects;
import java.util.OptionalInt;
import java.util.function.Consumer;
import java.util.function.Function;

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
 * <p>The text is UTF-8 unless the dialect declares another {@link Encoding}, which is decoded
 * ahead of the read: the parameters below say {@code utf8} for what is then that encoding's bytes.
 * A failure of the underlying stream or file is an {@link UncheckedIOException}, from the factory
 * or from {@link #read()}.
 *
 * <p>Not thread-safe, and confined: the buffers are a confined {@link Arena}'s, so a reader
 * belongs to the thread that built it, and the runtime refuses a call from any other.
 *
 * <p>The caller's stream runs in the middle of a read. A call from it back into the reader — a
 * read, a bind, a loop over {@link #forEachRow} — is refused with an {@link IllegalStateException};
 * a {@link #close()} from it closes the reader once the stream returns, and that read ends with an
 * {@link IllegalStateException} saying so, handing out no batch.
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

    private static final String BUSY = "The reader is in the middle of a read; its stream can't call back into it.";

    // One call's input is at most this: a span's offset and length are 31 bits.
    private static final long MAX_WINDOW_BYTES = Integer.MAX_VALUE;

    /**
     * The cell-table entries a batch is given at most: what the default batch of a plan
     * reaching 256 source columns takes (4096 rows of 257). A plan reaching further is read
     * in shorter batches, so that a wide ordinal costs one row's entries, not a batch's.
     */
    static final long CELL_BUDGET = 4096L * 257;

    private static final ValueLayout.OfByte BYTE = ValueLayout.JAVA_BYTE;
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
        /**
         * The interrupt that closed the channel mid-read: every read after it says so, where the
         * channel alone would only say it is closed.
         */
        private ClosedByInterruptException interrupted;

        FileSource(FileChannel channel) {
            this.channel = channel;
        }

        @Override
        public int read(MemorySegment target, long at, long room) throws IOException {
            if (interrupted != null) {
                InterruptedIOException refused = new InterruptedIOException(
                        "A thread interrupt closed the file in the middle of a read; the reader can read no more of it.");
                refused.initCause(interrupted);
                throw refused;
            }
            ByteBuffer view =
                    target.asSlice(at, Math.min(room, Integer.MAX_VALUE)).asByteBuffer();
            int read;
            try {
                do {
                    read = channel.read(view);
                } while (read == 0);
            } catch (ClosedByInterruptException e) {
                interrupted = e;
                throw e;
            }
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
        private static final int FIRST_TRANSFER_BYTES = 1 << 20;
        private static final int MAX_TRANSFER_BYTES = 64 << 20;

        private final InputStream stream;
        private byte[] transfer;
        private boolean full;

        StreamSource(InputStream stream, int bufferBytes) {
            this.stream = stream;
            this.transfer = new byte[Math.min(bufferBytes, FIRST_TRANSFER_BYTES)];
        }

        /**
         * One read of the stream, whatever it gives: what's finished in it reaches the caller however
         * long the stream then blocks. The array grows when a read fills it, because a fill looks at
         * the buffer from the record's start, so a long record handed over at a fixed size would cost
         * the square of its length. It stops at a cap, for a heap that's far smaller than a record.
         */
        @Override
        public int read(MemorySegment target, long at, long room) throws IOException {
            if (full && transfer.length < room && transfer.length < MAX_TRANSFER_BYTES) {
                transfer = new byte[(int) Math.min(Math.min(room, transfer.length * 2L), MAX_TRANSFER_BYTES)];
            }
            // The array the stream is handed is the one copied from, whatever the stream's own code
            // does in between.
            byte[] into = transfer;
            int want = (int) Math.min(room, into.length);
            int read;
            do {
                read = stream.read(into, 0, want);
            } while (read == 0);
            full = read == into.length;
            if (read > 0) {
                MemorySegment.copy(into, 0, target, BYTE, at, read);
            }
            return read;
        }

        @Override
        public void close() throws IOException {
            stream.close();
        }
    }

    /**
     * The step ahead of the fill, for a dialect that declares an encoding: the original bytes,
     * which go through the core's transcoder into the reader's buffer, where the fill reads them
     * as it would have read UTF-8.
     */
    private static final class Decoder {
        /**
         * The most bytes of UTF-8 one character takes: with this much room, a transcode that writes
         * nothing and does not fail wants more input.
         */
        static final int MAX_CHARACTER = 4;

        final int encoding;
        /** What a source's reads land in, fixed in size: {@code null} over memory. */
        final Block raw;
        /** The original bytes over memory: {@code null} over a source. */
        final MemorySegment memory;
        /** The original bytes, {@code at} to {@code end} of them not yet decoded. */
        long at;

        long end;
        /** Whether the source has ended. */
        boolean eof;
        /** The original bytes decoded so far: added to a call's failure, this is the input's offset. */
        long offset;
        /** A unit has no character, at {@code failedAt} in the original input: raised when the fill wants more. */
        boolean failed;

        long failedAt;

        /** A decoder of a source. */
        Decoder(int encoding, int size) {
            this.encoding = encoding;
            this.raw = new Block(Math.clamp(size, MAX_CHARACTER, MAX_ROW_BYTES), 1);
            this.memory = null;
        }

        /** A decoder of memory, all there already. */
        Decoder(int encoding, MemorySegment memory) {
            this.encoding = encoding;
            this.raw = null;
            this.memory = memory;
            this.end = memory.byteSize();
            this.eof = true;
        }

        MemorySegment input() {
            return raw != null ? raw.segment : memory;
        }

        /** Puts what is left undecoded at the front. */
        void compact() {
            MemorySegment.copy(raw.segment, at, raw.segment, 0, end - at);
            end -= at;
            at = 0;
        }

        void close() {
            if (raw != null) {
                raw.close();
            }
        }
    }

    private final Dialect dialect;
    /** The thread that built the reader, whose confined arena holds its buffers. */
    private final Thread owner = Thread.currentThread();
    /** The plan's columns and the batch they are read into: {@code null} until a plan is bound. */
    private Columns columns;

    private Batch batch;
    /** Cell-table entries one row takes: one per source column the plan reaches, and one more. */
    private long perRow;

    // Everything the core is handed, allocated once: the fixed-size blocks in one arena,
    // and each buffer that can be outgrown in its own.
    private final Arena arena;
    private MemorySegment state;
    private MemorySegment filled;
    // The fill's cells, arena, tokens and row widths, as the core takes them: in a buffers block,
    // of whose workbook fields the fill reads those two and nothing else (Mono's interpreter,
    // .NET's in the browser, passes no more than twelve integer arguments to a native function,
    // and the ABI is the same for every binding). Allocated zeroed.
    private MemorySegment buffers;
    private Block cells;
    private Block unescaped;
    /** The last batch came up short with the arena mostly used: it wants a larger one. */
    private boolean cramped;

    // The source: a stream or file read into `buffer`, or memory read in place.
    private Source source;
    private Block buffer;
    private MemorySegment memory;
    /** What decodes a declared encoding into {@code buffer}: {@code null} for UTF-8. */
    private Decoder decoder;

    private MemorySegment transcoded;
    private long maxWindow = MAX_WINDOW_BYTES;
    /** For the tests: the row ceiling this reader's buffer stops at, so a ceiling test costs megabytes. */
    int rowCeiling = MAX_ROW_BYTES;

    private long start;
    private long end;
    private boolean eof;

    private Header header;
    private TabularException failure;
    private boolean closed;
    /** A call is in progress: a read, a bind or the header. The caller's stream runs in the middle of one. */
    private boolean busy;
    /** {@link #close()} was called in the middle of a call, and is done when that call ends. */
    private boolean closeAsked;

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
     * @throws IllegalArgumentException if the dialect's separator or a column cannot be honoured
     *     (a column reading at or past the width the header gives every record among them), or
     *     {@code batchRows} is {@code 0}, negative or asks for more than the core allows
     * @throws TabularException if the header record is structurally broken
     */
    public static DelimitedReader of(byte[] utf8, Dialect dialect, List<Column> plan, int batchRows) {
        return ofArray(utf8, dialect, Columns.checked(plan), batchRows);
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
            reader.overMemory(copy, MAX_WINDOW_BYTES);
            reader.begin(dialect);
            return reader;
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
     * native memory once, as a byte array is. So, on JDK 21 to 24, is a segment of a shared
     * arena (or any other whose arena another thread could close): that path's native calls
     * can't hold the arena open as an FFM downcall does.
     *
     * @param utf8 the text, of any size
     * @param dialect the declared dialect
     * @param plan the output columns, in output order
     * @param batchRows rows per batch
     * @return the reader, its header already read when the dialect declares one
     * @throws IllegalArgumentException if the dialect's separator or a column cannot be honoured
     *     (a column reading at or past the width the header gives every record among them), or
     *     {@code batchRows} is {@code 0}, negative or asks for more than the core allows
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
        DelimitedReader reader = new DelimitedReader(dialect, plan == null ? null : Columns.checked(plan), batchRows);
        try {
            MemorySegment text = utf8;
            // A heap segment, or on JDK 21 to 24 one another thread could free mid-call, is copied.
            if (!utf8.isNative() || !Native.holdsInPlace(utf8)) {
                text = reader.arena.allocate(utf8.byteSize());
                text.copyFrom(utf8);
            }
            reader.overMemory(text, windowBytes);
            reader.begin(dialect);
            return reader;
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
     * @throws IllegalArgumentException if the dialect's separator or a column cannot be honoured
     *     (a column reading at or past the width the header gives every record among them), or
     *     {@code batchRows} or {@code bufferBytes} is {@code 0} or negative, or {@code batchRows}
     *     asks for more than the core allows
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
     *     {@code bufferBytes} is {@code 0} or negative
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
        int start;
        try {
            start = startingBuffer(plan, batchRows, bufferBytes);
            reader = new DelimitedReader(dialect, plan == null ? null : Columns.checked(plan), batchRows);
        } catch (RuntimeException | Error failure) {
            // No reader came to be, so the stream it would have owned is closed here.
            try {
                utf8.close();
            } catch (IOException | RuntimeException closing) {
                failure.addSuppressed(closing);
            }
            throw failure;
        }
        reader.overSource(new StreamSource(utf8, start), start);
        reader.begin(dialect);
        return reader;
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
     * @throws IllegalArgumentException if the dialect's separator or a column cannot be honoured
     *     (a column reading at or past the width the header gives every record among them), or
     *     {@code batchRows} or {@code bufferBytes} is {@code 0} or negative, or {@code batchRows}
     *     asks for more than the core allows
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
     *     {@code bufferBytes} is {@code 0} or negative
     * @throws TabularException if the header record is structurally broken
     * @throws UncheckedIOException if the file cannot be opened or read
     */
    public static DelimitedReader open(Path path, Dialect dialect, int bufferBytes) {
        return openFile(path, dialect, null, 0, bufferBytes);
    }

    private static DelimitedReader openFile(
            Path path, Dialect dialect, List<Column> plan, int batchRows, int bufferBytes) {
        Objects.requireNonNull(path, "path");
        int start = startingBuffer(plan, batchRows, bufferBytes);
        DelimitedReader reader = new DelimitedReader(dialect, plan == null ? null : Columns.checked(plan), batchRows);
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
        reader.overSource(new FileSource(channel), start);
        reader.begin(dialect);
        return reader;
    }

    /**
     * The size to start the input buffer at, the core's answer. The batch's own size is judged with
     * its plan, when that is bound; a zero says so here, ahead of a zero buffer.
     */
    private static int startingBuffer(List<Column> plan, int batchRows, int bufferBytes) {
        return Columns.checkSizes(MemorySegment.NULL, 0, plan == null ? 1 : batchRows, bufferBytes, 0);
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
        if (stateBytes < Abi.STATE_READ_BYTES) {
            throw new IllegalStateException("libhypertabular's state block is " + stateBytes
                    + " bytes; this binding reads " + Abi.STATE_READ_BYTES + " of it.");
        }
        this.arena = Arena.ofConfined();
        try {
            state = arena.allocate(stateBytes, 8);
            filled = arena.allocate(Abi.FILLED_BYTES, 8);
            buffers = arena.allocate(Abi.BUFFERS_WITH_BYTES, 8);
            buffers.set(INT, 0, (int) Abi.BUFFERS_WITH_BYTES);
            byte[] tokens = dialect.tokens().packed();
            if (tokens.length > 0) {
                buffers.set(ValueLayout.ADDRESS, Abi.BUFFERS_WITH_TOKENS, Columns.held(tokens, arena));
                buffers.set(LONG, Abi.BUFFERS_WITH_TOKENS + 8, tokens.length);
            }
            unescaped = new Block(4096, 1);

            MemorySegment raw = arena.allocate(Abi.DELIMITED_OPTIONS_BYTES, 4);
            raw.set(INT, 0, (int) Abi.DELIMITED_OPTIONS_BYTES);
            raw.set(BYTE, Abi.DELIMITED_OPTIONS_SEPARATOR, (byte) dialect.separator());
            raw.set(BYTE, Abi.DELIMITED_OPTIONS_QUOTING, (byte) (dialect.quoting() ? 1 : 0));
            raw.set(BYTE, Abi.DELIMITED_OPTIONS_SKIP_BLANK_LINES, (byte) (dialect.skipBlankLines() ? 1 : 0));
            raw.set(INT, Abi.DELIMITED_OPTIONS_SKIP, dialect.skip());
            raw.set(INT, Abi.DELIMITED_OPTIONS_HEADER_ROW, dialect.headerRow());
            raw.set(INT, Abi.DELIMITED_OPTIONS_SHORT_ROWS, dialect.shortRows().code());
            raw.set(INT, Abi.DELIMITED_OPTIONS_LONG_ROWS, dialect.longRows().code());
            if (Native.initWith(state, raw) != Abi.OK) {
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
        refusePastWidth(plan);
        // A cell-table entry for every source column up to the widest the plan reads, and one
        // for the row's line, for as many rows as the budget holds: at most 2^20 + 1 entries a
        // row, as an ordinal is below 2^20.
        long width = Columns.reach(plan) + 1;
        long bytes = cellRows(width, batchRows) * width * Abi.SPAN_BYTES;
        Columns bound =
                new Columns(plan, batchRows, arena, dialect.notes(), dialect.longRows() == LongRows.REPORT, false);
        Block table = new Block(bytes, 4);
        perRow = width;
        cells = table;
        batch = new Batch(bound, false, 0);
        columns = bound;
    }

    /** The cell-table entries the reader holds, for the suite. */
    long cellEntries() {
        return cells == null ? 0 : cells.bytes() / Abi.SPAN_BYTES;
    }

    /** The rows of {@code perRow} entries the cell table holds: the batch, within the budget. */
    static long cellRows(long perRow, int batchRows) {
        return Math.max(1, Math.min(CELL_BUDGET / perRow, batchRows));
    }

    /**
     * Refuses the first plan column that reads at or past the width the header gives every
     * record: it could only ever be empty. Before the header is read the width is unknown,
     * and nothing is refused.
     */
    private void refusePastWidth(Column[] plan) {
        int width = state.get(INT, Abi.STATE_EXPECTED);
        if (width == 0) {
            return;
        }
        for (int index = 0; index < plan.length; index++) {
            if (plan[index].ordinal() >= width) {
                throw new IllegalArgumentException("Plan column " + index + " reads source column "
                        + plan[index].ordinal() + ", past the " + width + " columns the header gives every record.");
            }
        }
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
     * @throws IllegalStateException if the reader already has a plan, is closed, or is in the
     *     middle of a read (the call came from its stream)
     * @throws IllegalArgumentException if a column cannot be honoured — a column reading at or
     *     past the width the header gives every record among them — or {@code batchRows} is
     *     {@code 0}, negative or asks for more than the core allows; the reader is left without a plan
     */
    public void bind(List<Column> plan, int batchRows) {
        enter();
        try {
            if (columns != null) {
                throw new IllegalStateException("The reader already has a plan; a plan is bound once.");
            }
            attach(Columns.checked(plan), batchRows);
        } finally {
            busy = false;
        }
    }

    /**
     * Starts a call that reads or changes the reader, refusing one made from inside another: the
     * caller's stream runs in the middle of a read, and the read has its buffers' offsets in hand.
     * A call from another thread is refused first, as the confined arena would refuse it, so that it
     * never touches the flag.
     */
    private void enter() {
        if (closed) {
            throw new IllegalStateException("The reader is closed.");
        }
        if (Thread.currentThread() != owner) {
            throw new WrongThreadException("The reader belongs to the thread that built it.");
        }
        if (busy) {
            throw new IllegalStateException(BUSY);
        }
        busy = true;
    }

    /**
     * Ends a read: a {@link #close()} asked for in the middle of it is done now that nothing holds
     * the buffers, and what it throws is kept beside what the read threw.
     */
    private void leave(Throwable thrown) {
        busy = false;
        if (closeAsked) {
            closeAsked = false;
            try {
                close();
            } catch (RuntimeException | Error closing) {
                if (thrown == null) {
                    throw closing;
                }
                thrown.addSuppressed(closing);
            }
        }
    }

    /**
     * Whether a plan has been bound: always, for a reader opened with one.
     *
     * @return {@code true} once the reader has a plan
     */
    public boolean isBound() {
        return columns != null;
    }

    private void overMemory(MemorySegment text, long windowBytes) {
        memory = text;
        maxWindow = windowBytes;
        if (dialect.encoding() == Encoding.UTF_8) {
            end = text.byteSize();
            eof = true;
            return;
        }
        // Under a declared encoding the memory is the decoder's, and what it decodes to lands in a
        // buffer.
        decoder = new Decoder(dialect.encoding().code(), text);
        buffer = new Block(DEFAULT_BUFFER_BYTES, 1);
        transcoded = arena.allocate(Abi.TRANSCODED_BYTES, 8);
    }

    /** Takes the source over: from here it is closed with the reader, on failure included. */
    private void overSource(Source from, int bufferBytes) {
        source = from;
        try {
            buffer = new Block(bufferBytes, 1);
            if (dialect.encoding() != Encoding.UTF_8) {
                decoder = new Decoder(dialect.encoding().code(), bufferBytes);
                transcoded = arena.allocate(Abi.TRANSCODED_BYTES, 8);
            }
        } catch (RuntimeException | Error failure) {
            abandon(failure);
            throw failure;
        }
    }

    /** Closes a reader that is not going to be handed out, keeping the failure that ended it. */
    private void abandon(Throwable failure) {
        try {
            close();
        } catch (RuntimeException | Error closing) {
            failure.addSuppressed(closing);
        }
    }

    private void begin(Dialect dialect) {
        try {
            if (dialect.hasHeader()) {
                enter();
                try {
                    readHeader();
                } finally {
                    busy = false;
                }
                // A plan given at open was bound before the header was read.
                if (columns != null) {
                    refusePastWidth(columns.plan);
                }
            }
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
        int expected = closed ? expectedAtClose : state.get(INT, Abi.STATE_EXPECTED);
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
        return closed ? recordsAtClose : state.get(LONG, Abi.STATE_RECORDS);
    }

    /** The memory the core reads next from: the caller's, or the read buffer. */
    private MemorySegment base() {
        return buffer == null ? memory : buffer.segment;
    }

    private TabularException tooLong() {
        return failure = new TabularException(
                TabularFailure.ROW_TOO_LONG,
                state.get(LONG, Abi.STATE_RECORDS),
                state.get(INT, Abi.STATE_LINE),
                state.get(LONG, Abi.STATE_OFFSET),
                0,
                0);
    }

    /** One record fills the buffer: it needs a bigger one, up to the ceiling. */
    private void grow() {
        if (buffer.bytes() >= rowCeiling) {
            throw tooLong();
        }
        buffer.resize(Math.min(buffer.bytes() * 2, MAX_ROW_BYTES), end);
    }

    /**
     * What every refill does ahead of the source: the unfinished record to the front, room behind
     * it, and a failure the decoder is holding, now that the fill has had all the text ahead of it.
     * Returns {@code false} if nothing is to be read: the fill has one more look.
     */
    private boolean makeRoom() {
        long pending = end - start;
        if (start > 0) {
            MemorySegment.copy(buffer.segment, start, buffer.segment, 0, pending);
            start = 0;
            end = pending;
        }
        if (decoder != null && decoder.failed) {
            // A \r the fill is holding ends a row whatever comes next, and only waits to see if a
            // \n is part of it; nothing is coming, and this says so. In a quote it ends nothing, and
            // the record is as cut as it was.
            if (end == 0 || buffer.segment.get(BYTE, end - 1) != (byte) '\r') {
                throw failure = new TabularException(
                        TabularFailure.ENCODING,
                        state.get(LONG, Abi.STATE_RECORDS),
                        state.get(INT, Abi.STATE_LINE),
                        decoder.failedAt,
                        0,
                        0);
            }
            if (end == buffer.bytes()) {
                grow();
            }
            buffer.segment.set(BYTE, end++, (byte) '\n');
            return false;
        }
        if (end == buffer.bytes()) {
            grow();
        }
        return true;
    }

    /** Puts the unfinished record at the front of the buffer and reads more behind it. */
    private void refill() {
        if (buffer == null) {
            // Memory is read a window at a time, and a record that does not fit the largest
            // window there is cannot be read at all.
            throw tooLong();
        }
        if (!makeRoom()) {
            return;
        }
        try {
            if (decoder == null) {
                int read = returned(source.read(buffer.segment, end, buffer.bytes() - end));
                if (read < 0) {
                    eof = true;
                } else {
                    end += read;
                }
                return;
            }
            while (!decode()) {
                int read = returned(source.read(decoder.raw.segment, decoder.end, decoder.raw.bytes() - decoder.end));
                if (read < 0) {
                    decoder.eof = true;
                } else {
                    decoder.end += read;
                }
            }
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    /**
     * What a read of the source brought in, once the source has given control back: a reader its
     * stream (or anything that ran) closed ends the read here, with no batch, rather than going on.
     */
    private int returned(int read) {
        if (closeAsked) {
            throw new IllegalStateException("The reader was closed in the middle of a read.");
        }
        return read;
    }

    /**
     * Decodes what the decoder holds behind what the buffer holds, with whatever that wants first.
     * Returns {@code true} once the fill has something new to look at: text, the end, or a failure
     * to raise when it next asks; {@code false} if the source has to bring more first.
     */
    private boolean decode() {
        while (true) {
            long room = buffer.bytes() - end;
            long available = decoder.end - decoder.at;
            int code = Native.transcode(
                    decoder.encoding,
                    decoder.input().asSlice(decoder.at, available),
                    available,
                    decoder.eof,
                    buffer.segment.asSlice(end, room),
                    room,
                    transcoded);
            if (code != Abi.OK) {
                throw new IllegalStateException(CONTRACT_VIOLATION);
            }
            if (transcoded.get(INT, Abi.TRANSCODED_FAILED) != 0) {
                decoder.failed = true;
                decoder.failedAt = decoder.offset + transcoded.get(LONG, Abi.TRANSCODED_FAILURE_AT);
            }
            long consumed = transcoded.get(LONG, Abi.TRANSCODED_CONSUMED);
            long written = transcoded.get(LONG, Abi.TRANSCODED_WRITTEN);
            decoder.at += consumed;
            decoder.offset += consumed;
            if (written > 0) {
                end += written;
                return true;
            }
            // The next refill raises it, once the fill has had one more look.
            if (decoder.failed) {
                return true;
            }
            if (decoder.at < decoder.end && room < Decoder.MAX_CHARACTER) {
                grow();
                continue;
            }
            if (decoder.eof) {
                eof = true;
                return true;
            }
            decoder.compact();
            return false;
        }
    }

    private TabularException structural() {
        return failure = TabularException.from(filled, Abi.FAILURE_CODE);
    }

    private void readHeader() {
        // The name table is the header's alone, and gone when it has been read.
        try (Arena scope = Arena.ofConfined()) {
            long capacity = 64;
            MemorySegment names = scope.allocate(capacity * Abi.SPAN_BYTES, 4);
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
                    case Abi.OK -> {
                        long consumed = filled.get(LONG, Abi.FILLED_CONSUMED);
                        int count = (int) filled.get(LONG, Abi.FILLED_ROWS);
                        if (count > 0) {
                            Header.Builder found = new Header.Builder(count);
                            for (int index = 0; index < count; index++) {
                                int offset = names.get(INT, index * Abi.SPAN_BYTES);
                                int span = names.get(INT, index * Abi.SPAN_BYTES + 4);
                                // A flagged name had "" inside, and is in the arena, unescaped.
                                found.add(
                                        span < 0
                                                ? unescaped.segment.asSlice(offset, span & Abi.SPAN_LENGTH)
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
                    case Abi.ERR_CELLS -> {
                        capacity = filled.get(LONG, Abi.FILLED_NEEDED);
                        names = scope.allocate(Math.multiplyExact(capacity, Abi.SPAN_BYTES), 4);
                    }
                    case Abi.ERR_ARENA -> unescaped.resize(filled.get(LONG, Abi.FILLED_NEEDED), 0);
                    case Abi.ERR_STRUCTURE -> throw structural();
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
     * @throws UncheckedIOException if the stream or file fails; a thread interrupt that closes a
     *     file ends the reader, and every later read says so
     * @throws IllegalStateException if the reader is closed (its stream's own {@link #close()}
     *     included), no plan has been bound — not final: bind one and read — or the reader is in
     *     the middle of a read (the call came from its stream)
     */
    public Batch read() {
        enter();
        Throwable thrown = null;
        try {
            return next();
        } catch (RuntimeException | Error failed) {
            thrown = failed;
            throw failed;
        } finally {
            leave(thrown);
        }
    }

    private Batch next() {
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
            buffers.set(ValueLayout.ADDRESS, Abi.BUFFERS_WITH_FIELDS + Abi.BUFFERS_CELLS, cells.segment);
            buffers.set(LONG, Abi.BUFFERS_WITH_FIELDS + Abi.BUFFERS_CELLS + 8, cells.bytes() / Abi.SPAN_BYTES);
            buffers.set(ValueLayout.ADDRESS, Abi.BUFFERS_WITH_FIELDS + Abi.BUFFERS_ARENA, unescaped.segment);
            buffers.set(LONG, Abi.BUFFERS_WITH_FIELDS + Abi.BUFFERS_ARENA + 8, unescaped.bytes());
            buffers.set(ValueLayout.ADDRESS, Abi.BUFFERS_WITH_ROW_WIDTHS, columns.rowWidths);
            int code = Native.fillWith(
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
                case Abi.OK -> {
                    long consumed = filled.get(LONG, Abi.FILLED_CONSUMED);
                    long count = filled.get(LONG, Abi.FILLED_ROWS);
                    long at = start;
                    start += consumed;
                    if (count > 0) {
                        int rows = (int) count;
                        // Short of what the cell table holds, not of the batch: a plan reaching
                        // past the budget is short by design.
                        long fits = Math.min(batchRows, cellEntries() / perRow);
                        cramped = rows < fits && filled.get(LONG, Abi.FILLED_ARENA_USED) * 2 >= unescaped.bytes();
                        return batch.fill(
                                rows,
                                cells.segment,
                                perRow,
                                base,
                                at,
                                unescaped.segment,
                                state.get(INT, Abi.STATE_EXPECTED));
                    }
                    if (last && (consumed == length || consumed == 0)) {
                        return null;
                    }
                    if (consumed == 0) {
                        refill();
                    }
                }
                // The core says how many entries one row takes; the table holds a batch of them,
                // within the budget.
                case Abi.ERR_CELLS -> {
                    long needed = filled.get(LONG, Abi.FILLED_NEEDED);
                    perRow = Math.max(perRow, needed);
                    cells.resize(cellRows(needed, batchRows) * needed * Abi.SPAN_BYTES, 0);
                }
                case Abi.ERR_ARENA ->
                    unescaped.resize(Math.max(filled.get(LONG, Abi.FILLED_NEEDED), unescaped.bytes() * 2), 0);
                case Abi.ERR_STRUCTURE -> throw structural();
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
        if (decoder != null) {
            decoder.close();
        }
        arena.close();
    }

    /**
     * Releases the native memory, and closes the stream or file the reader was built over.
     * Closing twice is harmless. Called from the reader's stream in the middle of a read, it is done
     * as soon as the stream returns, and that read ends with an {@link IllegalStateException},
     * handing out no batch.
     *
     * @throws UncheckedIOException if closing the stream or file fails
     */
    @Override
    public void close() {
        if (closed) {
            return;
        }
        if (Thread.currentThread() != owner) {
            throw new WrongThreadException("The reader belongs to the thread that built it.");
        }
        if (busy) {
            closeAsked = true;
            return;
        }
        recordsAtClose = state.get(LONG, Abi.STATE_RECORDS);
        expectedAtClose = state.get(INT, Abi.STATE_EXPECTED);
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

    /**
     * Reads a sample of a file and says what plan its cells would read through, with
     * {@link SuggestOptions#DEFAULT}.
     *
     * @param path the file
     * @param dialect the declared dialect
     * @return the suggestion
     * @throws TabularException if the sampled rows are structurally broken
     * @throws UncheckedIOException if the file cannot be opened, or fails among the sampled rows,
     *     or a thread interrupt closes it at any point: an interrupt is the caller asking to stop,
     *     so it never only says the source was truncated
     * @see #suggest(Path, Dialect, SuggestOptions)
     */
    public static Suggestion suggest(Path path, Dialect dialect) {
        return suggest(path, dialect, SuggestOptions.DEFAULT);
    }

    /**
     * Reads a sample of a file through the text door and says what its columns look like: a plan
     * and why. Nothing here is applied, since a read still takes only what it is told: declare a
     * format and a date order where you know them, with {@link SuggestOptions}, because nothing
     * needs inferring then. A suggestion is a starting point for a plan to inspect.
     *
     * <p>The width is the header's, or the first record's, and a plan has to name the columns
     * before that record can be read. So with no header the file is opened twice, once to read the
     * first record. The dialect is what the sample is read with, its tokens included, and a cell
     * it says is empty is blank to the suggestion.
     *
     * <p>What it says can surprise, so read the report before the plan:
     *
     * <ul>
     *   <li>A column of {@code 0} and {@code 1} is {@link Door#BOOL}, the first door asked, and
     *       {@code F} or {@code No} read false.
     *   <li>{@code 1.234} alone is ambiguous, 1.234 or 1234, and reads as text until a format is
     *       declared or a value settles it, which takes in a column of prices at three decimals.
     *       Beside {@code 2.5} it is a decimal, and beside {@code 2,5} it is 1234.
     *   <li>{@code 2,5}, {@code 0,125}, {@code 1.234,5} and {@code 1.234.567} read only with
     *       {@code ,} as the decimal, so they are resolved in that notation, and
     *       {@link SuggestedColumn#format()} names it. {@code 2,5} beside {@code 2.5} is
     *       unresolved.
     *   <li>{@code 1,23,456}, Indian grouping, reads under neither notation with none declared,
     *       because nothing says its culture: alone it is text, resolved, and beside {@code 5}
     *       unresolved. Declaring {@code NumFormat.INVARIANT.withGrouping(Grouping.INDIAN)} for the
     *       column makes it a number.
     *   <li>{@code 007} and an integer past {@code long} are text, with no note saying why, so as
     *       not to lose the zeros or the digits.
     *   <li>A sheet's date column is text: {@link Sheet#suggest(SuggestOptions)} says why.
     * </ul>
     *
     * @param path the file
     * @param dialect the declared dialect
     * @param options how the sample is taken and read
     * @return the suggestion
     * @throws TabularException if the sampled rows are structurally broken: there is no knowing
     *     what is past them, and a read would stop there too. A failure past them is not met, or
     *     only says the source was truncated
     * @throws UncheckedIOException if the file cannot be opened, or fails among the sampled rows,
     *     or a thread interrupt closes it at any point: an interrupt is the caller asking to stop,
     *     so it never only says the source was truncated
     */
    public static Suggestion suggest(Path path, Dialect dialect, SuggestOptions options) {
        Objects.requireNonNull(path, "path");
        return suggest(dialect, options, unnoted -> open(path, unnoted));
    }

    /**
     * {@link #suggest(Path, Dialect)} over text in a byte array.
     *
     * @param text the text
     * @param dialect the declared dialect
     * @return the suggestion
     * @throws TabularException if the sampled rows are structurally broken
     */
    public static Suggestion suggest(byte[] text, Dialect dialect) {
        return suggest(text, dialect, SuggestOptions.DEFAULT);
    }

    /**
     * {@link #suggest(Path, Dialect, SuggestOptions)} over text in a byte array.
     *
     * @param text the text
     * @param dialect the declared dialect
     * @param options how the sample is taken and read
     * @return the suggestion
     * @throws TabularException if the sampled rows are structurally broken
     */
    public static Suggestion suggest(byte[] text, Dialect dialect, SuggestOptions options) {
        Objects.requireNonNull(text, "text");
        return suggest(dialect, options, unnoted -> of(text, unnoted));
    }

    /**
     * {@link #suggest(Path, Dialect, SuggestOptions)} over text in a memory segment.
     *
     * @param text the text
     * @param dialect the declared dialect
     * @param options how the sample is taken and read
     * @return the suggestion
     * @throws TabularException if the sampled rows are structurally broken
     */
    public static Suggestion suggest(MemorySegment text, Dialect dialect, SuggestOptions options) {
        Objects.requireNonNull(text, "text");
        return suggest(dialect, options, unnoted -> of(text, unnoted));
    }

    private static Suggestion suggest(
            Dialect dialect, SuggestOptions options, Function<Dialect, DelimitedReader> open) {
        Objects.requireNonNull(dialect, "dialect");
        Objects.requireNonNull(options, "options");
        int limit = options.rowsWithin(DEFAULT_BATCH_ROWS);
        Dialect unnoted = dialect.withNotes(false);
        int width = -1;
        if (!dialect.hasHeader()) {
            // A headerless source's width is its first record's, which a read says through a plan
            // of one column.
            try (DelimitedReader first = open.apply(unnoted)) {
                first.bind(List.of(Column.text(0)), 1);
                first.read();
                width = first.columnCount().orElse(0);
            }
        }
        int flags = unnoted.longRows() == LongRows.REPORT ? Abi.BATCH_WIDTHS : 0;
        try (DelimitedReader reader = open.apply(unnoted)) {
            int columns = width < 0 ? reader.header().size() : width;
            // One row more than the sample, so that a row past it costs no second batch to find, in
            // as many batches as the core says that takes.
            Sample sample = Sample.of(
                    reader::read,
                    plan -> reader.bind(plan, Columns.sampleRows(plan, limit + 1, flags)),
                    columns,
                    limit,
                    false);
            return sample.judge(reader.header(), options);
        }
    }
}
