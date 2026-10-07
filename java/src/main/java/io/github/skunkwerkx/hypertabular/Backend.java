package io.github.skunkwerkx.hypertabular;

import java.lang.foreign.MemorySegment;

/**
 * The one seam between {@link Native}'s calls and a non-FFM way of reaching the core, as
 * HyperCast's binding has one. The FFM downcalls are not behind it: they stay in
 * {@code Native} exactly as they were, and each call there only checks one
 * {@code static final} reference against {@code null} before taking its native path. This
 * exists so the wasm implementation ({@link WasmBackend}) is a separate class that is never
 * loaded, and whose GraalWasm dependency is never touched, unless it was selected — the
 * reference {@code Native} holds is typed as this interface, and the implementation is
 * instantiated by name.
 *
 * <p>Each method is one of {@code Native}'s, with the same arguments: the caller's own
 * segments, in this process's memory, laid out as the 64-bit native ABI lays them out. An
 * implementation performs the crossing and leaves every segment the core writes exactly as
 * the native call would have, so the readers above are one implementation for both paths —
 * which is what lets the whole test suite run unchanged against either.
 */
interface Backend {
    /** A short, stable name for diagnostics and tests: {@code "wasm"}. */
    String name();

    int version();

    long stateSize();

    int init(MemorySegment state, MemorySegment dialect);

    int header(
            MemorySegment state,
            MemorySegment input,
            long inputLen,
            boolean last,
            MemorySegment names,
            long namesCap,
            MemorySegment arena,
            long arenaCap,
            MemorySegment out);

    int fill(
            MemorySegment state,
            MemorySegment input,
            long inputLen,
            boolean last,
            MemorySegment specs,
            MemorySegment columns,
            long columnCount,
            long maxRows,
            MemorySegment buffers,
            MemorySegment out);

    long unescape(MemorySegment cell, long len, MemorySegment out, long cap);

    long workbookStateSize();

    int book(Native.Book call, MemorySegment state, MemorySegment container, MemorySegment buffers, MemorySegment out);

    int sheet(
            MemorySegment state,
            MemorySegment container,
            MemorySegment part,
            int index,
            boolean hasHeader,
            boolean skipEmptyRows,
            MemorySegment out);

    int bookFill(
            MemorySegment state,
            MemorySegment container,
            MemorySegment specs,
            MemorySegment columns,
            long columnCount,
            long maxRows,
            MemorySegment buffers,
            MemorySegment out);
}
