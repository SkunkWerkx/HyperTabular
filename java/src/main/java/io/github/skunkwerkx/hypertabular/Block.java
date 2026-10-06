package io.github.skunkwerkx.hypertabular;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;

/**
 * Native memory that can be outgrown: its own confined arena, closed when a larger one
 * replaces it, so a buffer that doubled ten times holds one allocation, not eleven.
 */
final class Block {
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
