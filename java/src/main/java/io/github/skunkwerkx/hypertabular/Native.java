package io.github.skunkwerkx.hypertabular;

import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.ByteOrder;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/**
 * The native core's C ABI: {@code libhypertabular}'s six exports and the sizes and offsets
 * of the {@code #[repr(C)]} shapes that cross them ({@code rust/src/kernel/abi.rs},
 * {@code rust/src/kernel/exports.rs}). Every pointer handed over is this binding's own
 * memory for the length of the call; the core keeps nothing.
 *
 * <p>Nothing here is linked {@link Linker.Option#critical(boolean) critical}. A fill reads a
 * whole batch — thousands of rows — which is not the "as short as an empty function" call
 * that option is for: a critical call holds every other thread's safepoint for as long as
 * it runs. So the buffers are native memory, allocated once by the reader, and the call is
 * an ordinary downcall the collector can run beside.
 */
final class Native {
    private Native() {}

    /** The call did what it could; the result says how far it got. */
    static final int OK = 0;
    /** The data is structurally broken; the result's failure says where. */
    static final int ERR_STRUCTURE = -2;
    /** The arena cannot hold what one row needs; the result says how much that is. */
    static final int ERR_ARENA = -3;
    /** The cell table cannot hold one row; the result says how many entries one takes. */
    static final int ERR_CELLS = -4;

    /** {@code Span}: {@code {u32 offset, u32 len}}, the top bit of {@code len} a flag. */
    static final long SPAN_BYTES = 8;
    /** The length bits of a span's {@code len}; the flag is the sign bit of the Java int. */
    static final int SPAN_LENGTH = Integer.MAX_VALUE;

    /** {@code CellVerdict}: {@code {u32 offset, u32 len, u32 reason}}. */
    static final long VERDICT_BYTES = 12;

    /**
     * {@code ColumnSpec}: {@code {u32 ordinal, u32 door, u32 param, RawNumFormat format}},
     * the format {@code {u32 decimal_sep, u32 group_sep, u32 flags, u32 currency_len,
     * u8[16] currency}}.
     */
    static final long SPEC_BYTES = 44;
    static final long SPEC_ORDINAL = 0;
    static final long SPEC_DOOR = 4;
    static final long SPEC_PARAM = 8;
    static final long SPEC_DECIMAL_SEP = 12;
    static final long SPEC_GROUP_SEP = 16;
    static final long SPEC_FLAGS = 20;
    static final long SPEC_CURRENCY_LEN = 24;
    static final long SPEC_CURRENCY = 28;
    static final int CURRENCY_BYTES = 16;

    /** {@code ColumnBuffer}: {@code {void* values, CellVerdict* verdicts}}. */
    static final long BUFFER_BYTES = 2 * ValueLayout.ADDRESS.byteSize();
    static final long BUFFER_VERDICTS = ValueLayout.ADDRESS.byteSize();

    /** {@code RawDialect}: {@code {u8 separator, u8 quoting, u8 skip_blank_lines, u8 engine}}. */
    static final long DIALECT_BYTES = 4;

    /**
     * {@code Filled}: {@code {u64 rows, u64 consumed, u64 arena_used, u64 needed,
     * Failure failure}}, the failure {@code {u32 code, u32 line, u64 record, u64 byte,
     * u32 expected, u32 found}}.
     */
    static final long FILLED_BYTES = 64;
    static final long FILLED_ROWS = 0;
    static final long FILLED_CONSUMED = 8;
    static final long FILLED_NEEDED = 24;
    static final long FAILURE_CODE = 32;
    static final long FAILURE_LINE = 36;
    static final long FAILURE_RECORD = 40;
    static final long FAILURE_BYTE = 48;
    static final long FAILURE_EXPECTED = 56;
    static final long FAILURE_FOUND = 60;

    /**
     * The fields of the state block this binding reads: {@code line} (u32), {@code records}
     * and {@code offset} (u64). The rest is the core's; the block's size is asked of the
     * library ({@link #stateSize()}), never assumed.
     */
    static final long STATE_LINE = 4;
    static final long STATE_RECORDS = 8;
    static final long STATE_OFFSET = 16;
    /** The bytes of the state block the offsets above reach into. */
    static final long STATE_READ_BYTES = 24;

    /**
     * The downcall handles: one per export, none of them bound to an address. Each takes
     * the export's address as its leading argument, which {@link Core} looks up once the
     * library is loaded.
     *
     * <p>They live apart from {@link Core} for GraalVM Native Image, as HyperCast's do: an
     * image can only compile a call through a {@code MethodHandle} that is already a
     * constant when the image is built, and a handle bound to a symbol's address cannot be
     * one. Nothing in this class needs the library, so
     * {@code META-INF/native-image/.../native-image.properties} has it initialized at image
     * build time. On the JVM the split changes nothing.
     */
    static final class Downcalls {
        private Downcalls() {}

        private static final Linker LINKER = Linker.nativeLinker();

        // usize is a long on every platform this jar carries a library for: all are 64-bit.
        private static final ValueLayout.OfLong USIZE = ValueLayout.JAVA_LONG;
        private static final ValueLayout PTR = ValueLayout.ADDRESS;
        private static final ValueLayout.OfInt I32 = ValueLayout.JAVA_INT;

        // hypertabular_version() -> u32
        private static final MethodHandle VERSION = LINKER.downcallHandle(FunctionDescriptor.of(I32));
        // hypertabular_delimited_state_size() -> usize
        private static final MethodHandle STATE_SIZE = LINKER.downcallHandle(FunctionDescriptor.of(USIZE));
        // hypertabular_delimited_init(state, dialect) -> i32
        private static final MethodHandle INIT = LINKER.downcallHandle(FunctionDescriptor.of(I32, PTR, PTR));
        // hypertabular_delimited_header(state, input, input_len, last, names, names_cap,
        //                               arena, arena_cap, out) -> i32
        private static final MethodHandle HEADER = LINKER.downcallHandle(
                FunctionDescriptor.of(I32, PTR, PTR, USIZE, I32, PTR, USIZE, PTR, USIZE, PTR));
        // hypertabular_delimited_fill(state, input, input_len, last, specs, columns,
        //                             column_count, max_rows, cells, cells_cap, arena,
        //                             arena_cap, out) -> i32
        private static final MethodHandle FILL = LINKER.downcallHandle(
                FunctionDescriptor.of(
                        I32, PTR, PTR, USIZE, I32, PTR, PTR, USIZE, USIZE, PTR, USIZE, PTR, USIZE, PTR));
        // hypertabular_delimited_unescape(cell, len, out, cap) -> usize
        private static final MethodHandle UNESCAPE =
                LINKER.downcallHandle(FunctionDescriptor.of(USIZE, PTR, USIZE, PTR, USIZE));

        // A uuid value read as two big-endian longs. Here for the same reason the handles
        // are: a layout is read through a VarHandle, which Native Image wants constant.
        static final ValueLayout.OfLong BIG_ENDIAN_LONG = ValueLayout.JAVA_LONG.withOrder(ByteOrder.BIG_ENDIAN);
    }

    /**
     * The loaded library and the address of every export in it — all {@code static final},
     * all resolved in this holder's own class init. A holder so that nothing loads until
     * the first reader (or {@link Tabular#nativeVersion()}) touches it, and so that
     * {@link Tabular#isAvailable()} can observe a load failure without anything else having
     * failed to initialize.
     */
    private static final class Core {
        private Core() {}

        private static final SymbolLookup LOOKUP = loadLibrary(NativePlatform.current());

        // Looked up here, once, so an export missing from an older core fails this class's
        // init (and isAvailable() says so) rather than the first read.
        private static final MemorySegment VERSION = export("hypertabular_version");
        private static final MemorySegment STATE_SIZE = export("hypertabular_delimited_state_size");
        private static final MemorySegment INIT = export("hypertabular_delimited_init");
        private static final MemorySegment HEADER = export("hypertabular_delimited_header");
        private static final MemorySegment FILL = export("hypertabular_delimited_fill");
        private static final MemorySegment UNESCAPE = export("hypertabular_delimited_unescape");

        private static MemorySegment export(String symbol) {
            return LOOKUP.find(symbol).orElseThrow(
                    () -> new IllegalStateException("libhypertabular does not export " + symbol));
        }

        // The library must outlive every downcall made through it, so it is loaded into the
        // JDK-provided global arena that lives for the process's lifetime.
        private static SymbolLookup loadLibrary(NativePlatform.Target target) {
            if (target == null) {
                throw new IllegalStateException(
                        "hypertabular: this jar carries no native library for " + NativePlatform.describe());
            }
            try (InputStream resource = Native.class.getResourceAsStream(target.resourcePath())) {
                if (resource == null) {
                    throw new IllegalStateException(target.resourcePath() + " classpath resource not found "
                            + "(this jar was built without a native library for this platform)");
                }
                String libraryFileName = target.libraryFileName();
                String extension = libraryFileName.substring(libraryFileName.lastIndexOf('.'));
                Path tmp = Files.createTempFile("hypertabular", extension);
                tmp.toFile().deleteOnExit();
                Files.copy(resource, tmp, StandardCopyOption.REPLACE_EXISTING);
                return SymbolLookup.libraryLookup(tmp, Arena.global());
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            }
        }
    }

    // A downcall handle declares Throwable and throws nothing checked. What it can throw is
    // let through as it is: the library failing to load (a LinkageError out of Core's
    // init, which Tabular.isAvailable() reads), or a segment used from the wrong thread.
    private static AssertionError unexpected(String symbol, Throwable t) {
        return new AssertionError("hypertabular: " + symbol + " downcall failed unexpectedly", t);
    }

    /** The loaded library's version, packed {@code major << 16 | minor << 8 | patch}. */
    static int version() {
        try {
            return (int) Downcalls.VERSION.invokeExact(Core.VERSION);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_version", t);
        }
    }

    /** The size of a delimited state block, which wants 8-byte alignment. */
    static long stateSize() {
        try {
            return (long) Downcalls.STATE_SIZE.invokeExact(Core.STATE_SIZE);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_state_size", t);
        }
    }

    static int init(MemorySegment state, MemorySegment dialect) {
        try {
            return (int) Downcalls.INIT.invokeExact(Core.INIT, state, dialect);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_init", t);
        }
    }

    static int header(MemorySegment state, MemorySegment input, long inputLen, boolean last,
            MemorySegment names, long namesCap, MemorySegment arena, long arenaCap, MemorySegment out) {
        try {
            return (int) Downcalls.HEADER.invokeExact(
                    Core.HEADER, state, input, inputLen, last ? 1 : 0, names, namesCap, arena, arenaCap, out);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_header", t);
        }
    }

    static int fill(MemorySegment state, MemorySegment input, long inputLen, boolean last,
            MemorySegment specs, MemorySegment columns, long columnCount, long maxRows,
            MemorySegment cells, long cellsCap, MemorySegment arena, long arenaCap, MemorySegment out) {
        try {
            return (int) Downcalls.FILL.invokeExact(
                    Core.FILL, state, input, inputLen, last ? 1 : 0, specs, columns, columnCount, maxRows,
                    cells, cellsCap, arena, arenaCap, out);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_fill", t);
        }
    }

    static long unescape(MemorySegment cell, long len, MemorySegment out, long cap) {
        try {
            return (long) Downcalls.UNESCAPE.invokeExact(Core.UNESCAPE, cell, len, out, cap);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_unescape", t);
        }
    }
}
