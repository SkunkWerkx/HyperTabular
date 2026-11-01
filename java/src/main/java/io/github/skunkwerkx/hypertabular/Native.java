package io.github.skunkwerkx.hypertabular;

import io.github.skunkwerkx.hypercast.interop.NativePlatform;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
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
}
