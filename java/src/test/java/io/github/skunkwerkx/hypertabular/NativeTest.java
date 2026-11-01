package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;

/**
 * What both paths check of a segment before its address crosses: an FFM downcall refuses a
 * heap segment, a closed one and another thread's, and the JNI path refuses each the same way,
 * with the same exception, before the core is called.
 */
class NativeTest {
    /** A delimited state and a CSV dialect, ready for {@link Native#init}. */
    private static MemorySegment[] block(Arena arena) {
        MemorySegment state = arena.allocate(Native.stateSize(), 8);
        MemorySegment dialect = arena.allocate(Abi.DIALECT_BYTES);
        dialect.set(ValueLayout.JAVA_BYTE, 0, (byte) ',');
        dialect.set(ValueLayout.JAVA_BYTE, 1, (byte) 1);
        return new MemorySegment[] {state, dialect};
    }

    @Test
    void aSegmentOfThisThreadsOpenArenaCrosses() {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment[] block = block(arena);
            assertEquals(Abi.OK, Native.init(block[0], block[1]));
        }
    }

    @Test
    void aHeapSegmentIsRefused() {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment[] block = block(arena);
            MemorySegment heap = MemorySegment.ofArray(new byte[(int) Abi.DIALECT_BYTES]);
            assertThrows(IllegalArgumentException.class, () -> Native.init(block[0], heap));
        }
    }

    @Test
    void aClosedSegmentIsRefused() {
        MemorySegment[] block;
        try (Arena arena = Arena.ofConfined()) {
            block = block(arena);
        }
        MemorySegment[] closed = block;
        assertThrows(IllegalStateException.class, () -> Native.init(closed[0], closed[1]));
    }

    @Test
    void anotherThreadsSegmentIsRefused() throws InterruptedException {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment[] block = block(arena);
            AtomicReference<Throwable> thrown = new AtomicReference<>();
            Thread other = new Thread(() -> {
                try {
                    Native.init(block[0], block[1]);
                } catch (Throwable t) {
                    thrown.set(t);
                }
            });
            other.start();
            other.join();
            assertEquals(WrongThreadException.class, thrown.get().getClass(), String.valueOf(thrown.get()));
        }
    }
}
