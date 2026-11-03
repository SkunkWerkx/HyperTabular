package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.interop.NativePlatform;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;

/**
 * The FFM path to the core: the JDK 25 tree of the multi-release jar, read on 25 and later.
 * {@code libhypertabular}'s fourteen exports, each reached by a downcall, under the same
 * members as the JNI base's class of this name; {@link Abi} has the shapes that cross them.
 * Every pointer handed over is this binding's own memory for the length of the call; the
 * core keeps nothing.
 *
 * <p>Nothing here is linked {@link Linker.Option#critical(boolean) critical}. A fill reads a
 * whole batch — thousands of rows — which is not the "as short as an empty function" call
 * that option is for: a critical call holds every other thread's safepoint for as long as
 * it runs. So the buffers are native memory, allocated once by the reader, and the call is
 * an ordinary downcall the collector can run beside.
 */
final class Native {
    private Native() {}

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
     * The loaded core: the library it resolved to, and the address of every
     * export in it — all {@code static final}, all resolved in this holder's own class init.
     * A holder so that nothing loads until the first reader (or
     * {@link Tabular#nativeVersion()}) touches it, and so that {@link Tabular#isAvailable()}
     * can observe a load failure without anything else having failed to initialize.
     */
    private static final class Core {
        private Core() {}

        /** The library's base name: {@code libhypertabular.so}, {@code hypertabular.dll}. */
        private static final String LIBRARY = "hypertabular";

        private static final SymbolLookup LOOKUP =
                NativePlatform.load(Native.class, LIBRARY, NativePlatform.current(LIBRARY));

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

        private static MemorySegment export(String symbol) {
            return LOOKUP.find(symbol)
                    .orElseThrow(() -> new IllegalStateException("libhypertabular does not export " + symbol));
        }
    }

    // A downcall handle declares Throwable and throws nothing checked. What it can throw is
    // let through as it is: the library failing to load (a LinkageError out of Core's
    // init, which Tabular.isAvailable() reads), or a segment used from the wrong thread.
    private static AssertionError unexpected(String symbol, Throwable t) {
        return new AssertionError("hypertabular: " + symbol + " downcall failed unexpectedly", t);
    }

    /** Which path reached the core, for the diagnostics; loads nothing. */
    static String backend() {
        return "ffm";
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
        try {
            return (long) Downcalls.STATE_SIZE.invokeExact(Core.BOOK_STATE_SIZE);
        } catch (RuntimeException | Error failure) {
            throw failure;
        } catch (Throwable t) {
            throw unexpected("hypertabular_workbook_state_size", t);
        }
    }

    private static MemorySegment address(Abi.Book call) {
        return switch (call) {
            case OPEN -> Core.BOOK_OPEN;
            case SHEETS -> Core.BOOK_SHEETS;
            case STRINGS -> Core.BOOK_STRINGS;
            case STYLES -> Core.BOOK_STYLES;
            case HEADER -> Core.BOOK_HEADER;
        };
    }

    static int book(
            Abi.Book call, MemorySegment state, MemorySegment container, MemorySegment buffers, MemorySegment out) {
        try {
            return (int)
                    Downcalls.BOOK.invokeExact(address(call), state, container, container.byteSize(), buffers, out);
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
