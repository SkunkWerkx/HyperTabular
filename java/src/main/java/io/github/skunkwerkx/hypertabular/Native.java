package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.interop.NativePlatform;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/**
 * The JNI path to the core: the JDK 21 base of the multi-release jar, used on 21 through 24
 * and wherever the {@code META-INF/versions/25} tree is not read. A library
 * ({@code libhypertabular_jni}) holds the core inside it and registers the natives below when
 * it loads, one per C export, each taking every pointer as the {@code long} address of native
 * memory and returning what the export returns. The JDK 25 tree has a class of the same name
 * and the same members that reaches the core through FFM instead; {@link Abi} has the shapes
 * both hand across.
 *
 * <p>Every buffer a reader hands the core is native memory it allocated from its own arena
 * ({@link java.lang.foreign.Arena}, whose memory API JDK 21 runs without
 * {@code --enable-preview}: only {@code javac} treats it as preview there), so nothing is
 * pinned or copied on the way across. What an FFM downcall checks of a segment before it
 * passes the address, this does too ({@link #at}): that the segment is native, that its
 * arena is still open, and that it belongs to the calling thread.
 *
 * <p>What it can't do is what a downcall does next: hold the segment's arena open until the
 * call returns, so that a close from another thread is refused meanwhile. Here the call has
 * only an address, and a shared arena closed mid-call would leave the core reading freed
 * memory. The reader's own buffers come from confined arenas, which no other thread can close.
 * A caller's segment is read in place only when the same holds of it ({@link #holdsInPlace}):
 * the global arena's, an automatic arena's (the reader keeps it reachable), or one confined to
 * the calling thread. Any other — a shared arena's — is copied into the reader's own memory
 * when the reader or workbook is opened.
 */
final class Native {
    private Native() {}

    /** The library's base name: {@code libhypertabular_jni.so}, {@code hypertabular_jni.dll}. */
    private static final String LIBRARY = "hypertabular_jni";

    // The natives, registered by JNI_OnLoad on this very class, one per C export and named
    // after it. Their names and descriptors are the library's table, java/jni/src/lib.rs.
    // Reached only through the members below, which touch Library first.
    private static native int hypertabularVersion();

    private static native long hypertabularDelimitedStateSize();

    private static native int hypertabularDelimitedInit(long state, long dialect);

    private static native int hypertabularDelimitedHeader(
            long state,
            long input,
            long inputLen,
            int last,
            long names,
            long namesCap,
            long arena,
            long arenaCap,
            long out);

    private static native int hypertabularDelimitedFill(
            long state,
            long input,
            long inputLen,
            int last,
            long specs,
            long columns,
            long columnCount,
            long maxRows,
            long buffers,
            long out);

    private static native long hypertabularDelimitedUnescape(long cell, long len, long out, long cap);

    private static native long hypertabularWorkbookStateSize();

    private static native int hypertabularWorkbookOpen(
            long state, long container, long containerLen, long buffers, long out);

    private static native int hypertabularWorkbookSheets(
            long state, long container, long containerLen, long buffers, long out);

    private static native int hypertabularWorkbookStrings(
            long state, long container, long containerLen, long buffers, long out);

    private static native int hypertabularWorkbookStyles(
            long state, long container, long containerLen, long buffers, long out);

    private static native int hypertabularWorkbookHeader(
            long state, long container, long containerLen, long buffers, long out);

    private static native int hypertabularWorkbookSheet(
            long state,
            long container,
            long containerLen,
            long part,
            long partLen,
            int index,
            int hasHeader,
            int skipEmptyRows,
            long out);

    private static native int hypertabularWorkbookFill(
            long state,
            long container,
            long containerLen,
            long specs,
            long columns,
            long columnCount,
            long maxRows,
            long buffers,
            long out);

    private static native int hypertabularDelimitedInitWith(long state, long options);

    private static native int hypertabularDelimitedFillWith(
            long state,
            long input,
            long inputLen,
            int last,
            long specs,
            long columns,
            long columnCount,
            long maxRows,
            long buffers,
            long out);

    private static native int hypertabularTranscode(
            int encoding, long input, long inputLen, int last, long out, long cap, long progress);

    private static native int hypertabularFaults(
            long columns,
            long columnCount,
            long rows,
            long rowWidths,
            int expectedWidth,
            int longRows,
            long cursor,
            long out,
            long cap,
            long written);

    private static native int hypertabularSuggest(
            long input,
            long inputLen,
            long arena,
            long arenaLen,
            long cells,
            long typed,
            long rows,
            long columns,
            long formats,
            long out);

    private static native int hypertabularWorkbookSheetWith(
            long state, long container, long containerLen, long part, long partLen, int index, long options, long out);

    private static native int hypertabularWorkbookFillWith(
            long state,
            long container,
            long containerLen,
            long specs,
            long columns,
            long columnCount,
            long maxRows,
            long buffers,
            long out);

    private static native int hypertabularRangeParse(long text, long len, long out);

    private static native int hypertabularBatchCheck(
            long specs, long columnCount, long batchRows, long bufferBytes, int flags, long out);

    private static native int jniNatives();

    /**
     * The library, loaded once in this holder's class init. A platform this jar ships no build
     * for, or a library that will not load (a temp directory mounted noexec, say), fails this
     * class's init with the reason, and {@link Tabular#isAvailable()} says so. A library whose
     * natives are not the ones this class declares fails the same way. Nothing loads until a
     * member below needs the core: every one touches this holder first.
     */
    private static final class Library {
        private Library() {}

        static {
            load(NativePlatform.current(LIBRARY));
            int implemented = jniNatives();
            int declared = 0;
            for (Method method : Native.class.getDeclaredMethods()) {
                if (Modifier.isNative(method.getModifiers())) {
                    declared++;
                }
            }
            if (implemented != declared) {
                throw new IllegalStateException("libhypertabular_jni implements " + implemented
                        + " natives; this binding declares " + declared);
            }
        }

        /** Forces the load. The call itself is the point; there is nothing to return. */
        static void ensure() {}

        private static void load(NativePlatform.Target target) {
            if (target == null) {
                // The same words as the FFM path's: no build of either library exists here.
                throw new IllegalStateException(NativePlatform.missing("hypertabular", null));
            }
            try (InputStream resource = Native.class.getResourceAsStream(target.resourcePath())) {
                if (resource == null) {
                    throw new IllegalStateException(NativePlatform.missing(LIBRARY, target));
                }
                String libraryFileName = target.libraryFileName();
                String extension = libraryFileName.substring(libraryFileName.lastIndexOf('.'));
                Path tmp = Files.createTempFile(LIBRARY, extension);
                tmp.toFile().deleteOnExit();
                Files.copy(resource, tmp, StandardCopyOption.REPLACE_EXISTING);
                // JNI_OnLoad registers the natives here; a table that does not match the
                // declarations above surfaces as the NoSuchMethodError it left pending.
                System.load(tmp.toAbsolutePath().toString());
            } catch (IOException e) {
                throw new UncheckedIOException(e);
            }
        }
    }

    /**
     * A segment's address, checked as an FFM downcall checks a segment it is handed: native
     * memory, its arena open, and this thread's to use. {@link MemorySegment#NULL} is address
     * 0, which the core reads as a null pointer.
     */
    private static long at(MemorySegment segment) {
        if (!segment.isNative()) {
            throw new IllegalArgumentException("Heap segment not allowed: " + segment);
        }
        if (!segment.scope().isAlive()) {
            throw new IllegalStateException("Already closed");
        }
        if (!segment.isAccessibleBy(Thread.currentThread())) {
            throw new WrongThreadException("Attempted access outside owning thread");
        }
        return segment.address();
    }

    /** Which path reached the core, for the diagnostics; loads nothing. */
    static String backend() {
        return "jni";
    }

    /** A thread that is never started: a segment it can reach is one any thread can, and close. */
    private static final Thread STRANGER = new Thread(() -> {});

    private static final MemorySegment.Scope GLOBAL = Arena.global().scope();

    /** An automatic arena's scope, by its kind: freed only once nothing reaches it. */
    private static final Class<?> AUTOMATIC = Arena.ofAuto().scope().getClass();

    /**
     * Whether a caller's native segment can be read in place by every call a reader or workbook
     * makes over it: whether no other thread can free it in the middle of one. A segment confined
     * to a thread can be closed only by that thread, which is inside the call; the global arena
     * never closes; an automatic arena's memory lives while the reader, which holds the segment,
     * does. Anything else, a shared arena's first, has to be copied, as an FFM downcall would
     * otherwise hold it.
     */
    static boolean holdsInPlace(MemorySegment segment) {
        MemorySegment.Scope scope = segment.scope();
        return !segment.isAccessibleBy(STRANGER) || scope.equals(GLOBAL) || scope.getClass() == AUTOMATIC;
    }

    /** The loaded library's version, packed {@code major << 16 | minor << 8 | patch}. */
    static int version() {
        Library.ensure();
        return hypertabularVersion();
    }

    /** The size of a delimited state block, which wants 8-byte alignment. */
    static long stateSize() {
        Library.ensure();
        return hypertabularDelimitedStateSize();
    }

    static int init(MemorySegment state, MemorySegment dialect) {
        Library.ensure();
        return hypertabularDelimitedInit(at(state), at(dialect));
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
        Library.ensure();
        return hypertabularDelimitedHeader(
                at(state), at(input), inputLen, last ? 1 : 0, at(names), namesCap, at(arena), arenaCap, at(out));
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
        Library.ensure();
        return hypertabularDelimitedFill(
                at(state),
                at(input),
                inputLen,
                last ? 1 : 0,
                at(specs),
                at(columns),
                columnCount,
                maxRows,
                at(buffers),
                at(out));
    }

    static long unescape(MemorySegment cell, long len, MemorySegment out, long cap) {
        Library.ensure();
        return hypertabularDelimitedUnescape(at(cell), len, at(out), cap);
    }

    /** The size of a workbook state block, which wants 8-byte alignment. */
    static long workbookStateSize() {
        Library.ensure();
        return hypertabularWorkbookStateSize();
    }

    static int book(
            Abi.Book call, MemorySegment state, MemorySegment container, MemorySegment buffers, MemorySegment out) {
        Library.ensure();
        long s = at(state);
        long c = at(container);
        long length = container.byteSize();
        long b = at(buffers);
        long o = at(out);
        return switch (call) {
            case OPEN -> hypertabularWorkbookOpen(s, c, length, b, o);
            case SHEETS -> hypertabularWorkbookSheets(s, c, length, b, o);
            case STRINGS -> hypertabularWorkbookStrings(s, c, length, b, o);
            case STYLES -> hypertabularWorkbookStyles(s, c, length, b, o);
            case HEADER -> hypertabularWorkbookHeader(s, c, length, b, o);
        };
    }

    static int sheet(
            MemorySegment state,
            MemorySegment container,
            MemorySegment part,
            int index,
            boolean hasHeader,
            boolean skipEmptyRows,
            MemorySegment out) {
        Library.ensure();
        return hypertabularWorkbookSheet(
                at(state),
                at(container),
                container.byteSize(),
                at(part),
                part.byteSize(),
                index,
                hasHeader ? 1 : 0,
                skipEmptyRows ? 1 : 0,
                at(out));
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
        Library.ensure();
        return hypertabularWorkbookFill(
                at(state),
                at(container),
                container.byteSize(),
                at(specs),
                at(columns),
                columnCount,
                maxRows,
                at(buffers),
                at(out));
    }

    static int initWith(MemorySegment state, MemorySegment options) {
        Library.ensure();
        return hypertabularDelimitedInitWith(at(state), at(options));
    }

    static int fillWith(
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
        Library.ensure();
        return hypertabularDelimitedFillWith(
                at(state),
                at(input),
                inputLen,
                last ? 1 : 0,
                at(specs),
                at(columns),
                columnCount,
                maxRows,
                at(buffers),
                at(out));
    }

    static int transcode(
            int encoding,
            MemorySegment input,
            long inputLen,
            boolean last,
            MemorySegment out,
            long cap,
            MemorySegment progress) {
        Library.ensure();
        return hypertabularTranscode(encoding, at(input), inputLen, last ? 1 : 0, at(out), cap, at(progress));
    }

    static int faults(
            MemorySegment columns,
            long columnCount,
            long rows,
            MemorySegment rowWidths,
            int expectedWidth,
            int longRows,
            MemorySegment cursor,
            MemorySegment out,
            long cap,
            MemorySegment written) {
        Library.ensure();
        return hypertabularFaults(
                at(columns),
                columnCount,
                rows,
                at(rowWidths),
                expectedWidth,
                longRows,
                at(cursor),
                at(out),
                cap,
                at(written));
    }

    static int suggest(
            MemorySegment input,
            long inputLen,
            MemorySegment arena,
            long arenaLen,
            MemorySegment cells,
            MemorySegment typed,
            long rows,
            long columns,
            MemorySegment formats,
            MemorySegment out) {
        Library.ensure();
        return hypertabularSuggest(
                at(input), inputLen, at(arena), arenaLen, at(cells), at(typed), rows, columns, at(formats), at(out));
    }

    static int sheetWith(
            MemorySegment state,
            MemorySegment container,
            MemorySegment part,
            int index,
            MemorySegment options,
            MemorySegment out) {
        Library.ensure();
        return hypertabularWorkbookSheetWith(
                at(state), at(container), container.byteSize(), at(part), part.byteSize(), index, at(options), at(out));
    }

    static int bookFillWith(
            MemorySegment state,
            MemorySegment container,
            MemorySegment specs,
            MemorySegment columns,
            long columnCount,
            long maxRows,
            MemorySegment buffers,
            MemorySegment out) {
        Library.ensure();
        return hypertabularWorkbookFillWith(
                at(state),
                at(container),
                container.byteSize(),
                at(specs),
                at(columns),
                columnCount,
                maxRows,
                at(buffers),
                at(out));
    }

    static int rangeParse(MemorySegment text, long len, MemorySegment out) {
        Library.ensure();
        return hypertabularRangeParse(at(text), len, at(out));
    }

    static int batchCheck(
            MemorySegment specs, long columnCount, long batchRows, long bufferBytes, int flags, MemorySegment out) {
        Library.ensure();
        return hypertabularBatchCheck(at(specs), columnCount, batchRows, bufferBytes, flags, at(out));
    }
}
