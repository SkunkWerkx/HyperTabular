package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.interop.NativePlatform;
import io.github.skunkwerkx.hypercast.interop.NativeValues;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.lang.reflect.InvocationTargetException;

/**
 * The native core's C ABI: {@code libhypertabular}'s fourteen exports and the sizes and offsets
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
    /** A workbook part's window cannot hold the token being read; the result says what would. */
    static final int ERR_WINDOW = -5;

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
    /** Where the format sits in a spec: HyperCast's {@link NativeValues#FORMAT_BYTES} of it. */
    static final long SPEC_FORMAT = 12;

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
    static final long FILLED_ARENA_USED = 16;
    static final long FILLED_NEEDED = 24;
    static final long FAILURE_CODE = 32;
    static final long FAILURE_LINE = 36;
    static final long FAILURE_RECORD = 40;
    static final long FAILURE_BYTE = 48;
    static final long FAILURE_EXPECTED = 56;
    static final long FAILURE_FOUND = 60;

    /**
     * {@code Opened}: {@code {u32 format, u32 epoch, u64 strings_bytes, u64 strings_count,
     * u64 needed, Failure failure}} — the failure at the same offset as in {@code Filled}.
     */
    static final long OPENED_BYTES = 64;

    static final long OPENED_FORMAT = 0;
    static final long OPENED_EPOCH = 4;
    static final long OPENED_STRINGS_BYTES = 8;
    static final long OPENED_NEEDED = 24;

    /** {@code Slot}: {@code {u32 tag, u32 aux, u64 bits}}, the row a workbook read assembles. */
    static final long SLOT_BYTES = 16;

    /**
     * {@code Buffers}: seven pointer-and-size pairs — {@code window}, {@code arena},
     * {@code cells}, {@code row}, then the workbook's {@code strings}, {@code table} and
     * {@code kinds} — each a pointer and a {@code usize}.
     */
    static final long BUFFERS_BYTES = 14 * 8;

    static final long BUFFERS_WINDOW = 0;
    static final long BUFFERS_ARENA = 16;
    static final long BUFFERS_CELLS = 32;
    static final long BUFFERS_ROW = 48;
    static final long BUFFERS_STRINGS = 64;
    static final long BUFFERS_TABLE = 80;
    static final long BUFFERS_KINDS = 96;

    /** The window a workbook part is inflated through starts at the size the core requires. */
    static final long WINDOW_MIN = 64 * 1024;

    /**
     * The fields of the state block this binding reads: {@code line} (u32), {@code records}
     * and {@code offset} (u64), and {@code expected} (u32, the cells a record has; 0 until
     * known). The rest is the core's; the block's size is asked of the
     * library ({@link #stateSize()}), never assumed.
     */
    static final long STATE_LINE = 4;

    static final long STATE_RECORDS = 8;
    static final long STATE_OFFSET = 16;
    static final long STATE_EXPECTED = 24;
    /** The bytes of the state block the offsets above reach into. */
    static final long STATE_READ_BYTES = 28;

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
        private static final MethodHandle HEADER =
                LINKER.downcallHandle(FunctionDescriptor.of(I32, PTR, PTR, USIZE, I32, PTR, USIZE, PTR, USIZE, PTR));
        // hypertabular_delimited_fill(state, input, input_len, last, specs, columns,
        //                             column_count, max_rows, buffers, out) -> i32
        private static final MethodHandle FILL = LINKER.downcallHandle(
                FunctionDescriptor.of(I32, PTR, PTR, USIZE, I32, PTR, PTR, USIZE, USIZE, PTR, PTR));
        // hypertabular_delimited_unescape(cell, len, out, cap) -> usize
        private static final MethodHandle UNESCAPE =
                LINKER.downcallHandle(FunctionDescriptor.of(USIZE, PTR, USIZE, PTR, USIZE));
        // hypertabular_workbook_{open,sheets,strings,styles,header}(state, container,
        //                                                         container_len, buffers, out) -> i32
        private static final MethodHandle BOOK =
                LINKER.downcallHandle(FunctionDescriptor.of(I32, PTR, PTR, USIZE, PTR, PTR));
        // hypertabular_workbook_sheet(state, container, container_len, part, part_len, index,
        //                             has_header, skip_empty_rows, out) -> i32
        private static final MethodHandle SHEET =
                LINKER.downcallHandle(FunctionDescriptor.of(I32, PTR, PTR, USIZE, PTR, USIZE, I32, I32, I32, PTR));
        // hypertabular_workbook_fill(state, container, container_len, specs, columns,
        //                            column_count, max_rows, buffers, out) -> i32
        private static final MethodHandle BOOK_FILL =
                LINKER.downcallHandle(FunctionDescriptor.of(I32, PTR, PTR, USIZE, PTR, PTR, USIZE, USIZE, PTR, PTR));
    }

    /**
     * The loaded core: which path won, the library it resolved to, and the address of every
     * export in it — all {@code static final}, all resolved in this holder's own class init.
     * A holder so that nothing loads until the first reader (or
     * {@link Tabular#nativeVersion()}) touches it, and so that {@link Tabular#isAvailable()}
     * can observe a load failure without anything else having failed to initialize.
     */
    private static final class Core {
        private Core() {}

        /** The library's base name: {@code libhypertabular.so}, {@code hypertabular.dll}. */
        private static final String LIBRARY = "hypertabular";

        /**
         * Non-null only when the wasm path was selected. Every call checks this one
         * {@code static final} against {@code null} before its FFM path; the JIT folds the
         * check away, so the native path costs what it did before a second backend existed.
         */
        private static final Backend WASM;

        // Null on the wasm path: there is no library to look symbols up in, and the native
        // linker is never asked for, so a platform the JDK has no linker for can still run
        // the module.
        private static final SymbolLookup LOOKUP;

        /*
         * Decides the path once, at class init, as HyperCast's Cast does.
         * Tabular.BACKEND_PROPERTY set to "wasm" forces the GraalWasm backend; "native" forces
         * FFM, and fails loudly when this platform has no bundled library or it will not load.
         * Unset takes FFM when this platform's library is bundled and loads, and the wasm
         * module otherwise — an OS, architecture or C library this jar has no native build for
         * still reads, through the module, and so does a bundled library that will not open.
         * When that fallback cannot start either, the failure thrown is the native one, with
         * the wasm one suppressed on it.
         */
        static {
            String choice = System.getProperty(Tabular.BACKEND_PROPERTY);
            if (choice != null && !"native".equals(choice) && !"wasm".equals(choice)) {
                throw new IllegalStateException(
                        Tabular.BACKEND_PROPERTY + " must be \"native\" or \"wasm\"; got \"" + choice + "\"");
            }
            NativePlatform.Target target = NativePlatform.current(LIBRARY);
            Backend wasm = null;
            SymbolLookup lookup = null;
            if ("wasm".equals(choice)) {
                wasm = startWasm(null);
            } else if ("native".equals(choice)) {
                lookup = NativePlatform.load(Native.class, LIBRARY, target);
            } else if (target == null || Native.class.getResource(target.resourcePath()) == null) {
                wasm = startWasm(NativePlatform.missing(LIBRARY, target));
            } else {
                try {
                    lookup = NativePlatform.load(Native.class, LIBRARY, target);
                } catch (RuntimeException | LinkageError nativeFailure) {
                    try {
                        wasm = startWasm("the bundled native library would not load (" + nativeFailure + ")");
                    } catch (RuntimeException wasmFailure) {
                        nativeFailure.addSuppressed(wasmFailure);
                        throw nativeFailure;
                    }
                }
            }
            WASM = wasm;
            LOOKUP = lookup;
        }

        // Looked up here, once, so an export missing from an older core fails this class's
        // init (and isAvailable() says so) rather than the first read.
        private static final MemorySegment VERSION = export("hypertabular_version");
        private static final MemorySegment STATE_SIZE = export("hypertabular_delimited_state_size");
        private static final MemorySegment INIT = export("hypertabular_delimited_init");
        private static final MemorySegment HEADER = export("hypertabular_delimited_header");
        private static final MemorySegment FILL = export("hypertabular_delimited_fill");
        private static final MemorySegment UNESCAPE = export("hypertabular_delimited_unescape");
        private static final MemorySegment BOOK_STATE_SIZE = export("hypertabular_workbook_state_size");
        private static final MemorySegment BOOK_OPEN = export("hypertabular_workbook_open");
        private static final MemorySegment BOOK_SHEETS = export("hypertabular_workbook_sheets");
        private static final MemorySegment BOOK_STRINGS = export("hypertabular_workbook_strings");
        private static final MemorySegment BOOK_STYLES = export("hypertabular_workbook_styles");
        private static final MemorySegment BOOK_SHEET = export("hypertabular_workbook_sheet");
        private static final MemorySegment BOOK_HEADER = export("hypertabular_workbook_header");
        private static final MemorySegment BOOK_FILL = export("hypertabular_workbook_fill");

        // Null on the wasm path, where the addresses above are never used.
        private static MemorySegment export(String symbol) {
            return LOOKUP == null
                    ? null
                    : LOOKUP.find(symbol)
                            .orElseThrow(() -> new IllegalStateException("libhypertabular does not export " + symbol));
        }

        /**
         * Starts the GraalWasm backend. {@code nativeUnavailable} is why the native path was
         * not taken, or {@code null} when wasm was asked for by name; it only shapes the
         * message of a failure here. {@link WasmBackend} is instantiated by name so that
         * {@code org.graalvm.polyglot}, a {@code compileOnly} dependency of this jar, is never
         * loaded unless it is going to be used.
         */
        private static Backend startWasm(String nativeUnavailable) {
            if (Native.class.getResource(WasmBackend.RESOURCE_PATH) == null) {
                throw new IllegalStateException(
                        nativeUnavailable == null
                                ? WasmBackend.RESOURCE_PATH + " classpath resource not found (this jar was built "
                                        + "without the wasm module)"
                                : nativeUnavailable + ", and " + WasmBackend.RESOURCE_PATH + " is not bundled either");
            }
            try {
                return (Backend) Class.forName(Native.class.getPackageName() + ".WasmBackend")
                        .getDeclaredConstructor()
                        .newInstance();
            } catch (ReflectiveOperationException | LinkageError e) {
                // The constructor is where GraalWasm is first touched, so its absence arrives
                // wrapped: newInstance hands back whatever the constructor threw inside an
                // InvocationTargetException.
                Throwable cause = e instanceof InvocationTargetException && e.getCause() != null ? e.getCause() : e;
                if (cause instanceof NoClassDefFoundError) {
                    throw new IllegalStateException(
                            WasmBackend.GRAALWASM_MISSING
                                    + (nativeUnavailable == null
                                            ? ""
                                            : "; wasm was selected because " + nativeUnavailable),
                            cause);
                }
                if (cause instanceof RuntimeException re) {
                    throw re;
                }
                throw new IllegalStateException("hypertabular: could not start the wasm backend", cause);
            }
        }
    }

    /** Which path this process uses: {@code "native"} or {@code "wasm"}. Loads the core. */
    static String backend() {
        return Core.WASM == null ? "native" : Core.WASM.name();
    }

    // A downcall handle declares Throwable and throws nothing checked. What it can throw is
    // let through as it is: the library failing to load (a LinkageError out of Core's
    // init, which Tabular.isAvailable() reads), or a segment used from the wrong thread.
    private static AssertionError unexpected(String symbol, Throwable t) {
        return new AssertionError("hypertabular: " + symbol + " downcall failed unexpectedly", t);
    }

    /** The loaded library's version, packed {@code major << 16 | minor << 8 | patch}. */
    static int version() {
        if (Core.WASM != null) {
            return Core.WASM.version();
        }
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
        if (Core.WASM != null) {
            return Core.WASM.stateSize();
        }
        try {
            return (long) Downcalls.STATE_SIZE.invokeExact(Core.STATE_SIZE);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_state_size", t);
        }
    }

    static int init(MemorySegment state, MemorySegment dialect) {
        if (Core.WASM != null) {
            return Core.WASM.init(state, dialect);
        }
        try {
            return (int) Downcalls.INIT.invokeExact(Core.INIT, state, dialect);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_init", t);
        }
    }

    static int header(
            MemorySegment state,
            MemorySegment input,
            long inputLen,
            boolean last,
            MemorySegment names,
            long namesCap,
            MemorySegment arena,
            long arenaCap,
            MemorySegment out) {
        if (Core.WASM != null) {
            return Core.WASM.header(state, input, inputLen, last, names, namesCap, arena, arenaCap, out);
        }
        try {
            return (int) Downcalls.HEADER.invokeExact(
                    Core.HEADER, state, input, inputLen, last ? 1 : 0, names, namesCap, arena, arenaCap, out);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_header", t);
        }
    }

    static int fill(
            MemorySegment state,
            MemorySegment input,
            long inputLen,
            boolean last,
            MemorySegment specs,
            MemorySegment columns,
            long columnCount,
            long maxRows,
            MemorySegment buffers,
            MemorySegment out) {
        if (Core.WASM != null) {
            return Core.WASM.fill(state, input, inputLen, last, specs, columns, columnCount, maxRows, buffers, out);
        }
        try {
            return (int) Downcalls.FILL.invokeExact(
                    Core.FILL,
                    state,
                    input,
                    inputLen,
                    last ? 1 : 0,
                    specs,
                    columns,
                    columnCount,
                    maxRows,
                    buffers,
                    out);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_fill", t);
        }
    }

    static long unescape(MemorySegment cell, long len, MemorySegment out, long cap) {
        if (Core.WASM != null) {
            return Core.WASM.unescape(cell, len, out, cap);
        }
        try {
            return (long) Downcalls.UNESCAPE.invokeExact(Core.UNESCAPE, cell, len, out, cap);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_delimited_unescape", t);
        }
    }

    /** The size of a workbook state block, which wants 8-byte alignment. */
    static long workbookStateSize() {
        if (Core.WASM != null) {
            return Core.WASM.workbookStateSize();
        }
        try {
            return (long) Downcalls.STATE_SIZE.invokeExact(Core.BOOK_STATE_SIZE);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_workbook_state_size", t);
        }
    }

    /** The workbook calls that share one shape: which export is the caller's to name. */
    enum Book {
        OPEN("hypertabular_workbook_open"),
        SHEETS("hypertabular_workbook_sheets"),
        STRINGS("hypertabular_workbook_strings"),
        STYLES("hypertabular_workbook_styles"),
        HEADER("hypertabular_workbook_header");

        private final String symbol;

        Book(String symbol) {
            this.symbol = symbol;
        }

        /** The export's name, which the wasm backend looks it up by. */
        String symbol() {
            return symbol;
        }

        private MemorySegment address() {
            return switch (this) {
                case OPEN -> Core.BOOK_OPEN;
                case SHEETS -> Core.BOOK_SHEETS;
                case STRINGS -> Core.BOOK_STRINGS;
                case STYLES -> Core.BOOK_STYLES;
                case HEADER -> Core.BOOK_HEADER;
            };
        }
    }

    static int book(Book call, MemorySegment state, MemorySegment container, MemorySegment buffers, MemorySegment out) {
        if (Core.WASM != null) {
            return Core.WASM.book(call, state, container, buffers, out);
        }
        try {
            return (int)
                    Downcalls.BOOK.invokeExact(call.address(), state, container, container.byteSize(), buffers, out);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected(call.symbol, t);
        }
    }

    static int sheet(
            MemorySegment state,
            MemorySegment container,
            MemorySegment part,
            int index,
            boolean hasHeader,
            boolean skipEmptyRows,
            MemorySegment out) {
        if (Core.WASM != null) {
            return Core.WASM.sheet(state, container, part, index, hasHeader, skipEmptyRows, out);
        }
        try {
            return (int) Downcalls.SHEET.invokeExact(
                    Core.BOOK_SHEET,
                    state,
                    container,
                    container.byteSize(),
                    part,
                    part.byteSize(),
                    index,
                    hasHeader ? 1 : 0,
                    skipEmptyRows ? 1 : 0,
                    out);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_workbook_sheet", t);
        }
    }

    static int bookFill(
            MemorySegment state,
            MemorySegment container,
            MemorySegment specs,
            MemorySegment columns,
            long columnCount,
            long maxRows,
            MemorySegment buffers,
            MemorySegment out) {
        if (Core.WASM != null) {
            return Core.WASM.bookFill(state, container, specs, columns, columnCount, maxRows, buffers, out);
        }
        try {
            return (int) Downcalls.BOOK_FILL.invokeExact(
                    Core.BOOK_FILL,
                    state,
                    container,
                    container.byteSize(),
                    specs,
                    columns,
                    columnCount,
                    maxRows,
                    buffers,
                    out);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_workbook_fill", t);
        }
    }
}
