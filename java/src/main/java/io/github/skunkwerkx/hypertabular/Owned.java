package io.github.skunkwerkx.hypertabular;

import java.lang.foreign.Arena;
import java.lang.ref.Cleaner;
import java.util.ArrayList;
import java.util.concurrent.ConcurrentLinkedQueue;

/**
 * The native memory one reader, workbook or sheet owns, for the day it is dropped without being
 * closed: {@code close()} frees the memory there and then, and this is what frees it when
 * {@code close()} never comes.
 *
 * <p>The memory is confined arenas', which only the thread that made them can close, so the
 * collector's own thread cannot free it. When the collector finds a reader nothing can reach, a
 * {@link Cleaner} hands that reader's memory to its thread's queue, and the thread closes the
 * arenas the next time it opens a reader, a workbook or a sheet. So a thread that goes on working
 * takes back what it dropped, and nothing is paid for it in a read. A thread that ends first leaves
 * what it dropped: no other thread can close it.
 *
 * <p>Nothing here refers to the reader, or the collector would never find it. A batch refers to its
 * reader or sheet, and a sheet to its workbook, so none of them is found while a batch is still held.
 * A {@code MemorySegment} view alone holds nothing: once its reader is collected it is refused by
 * the runtime, as after {@code close()}.
 */
final class Owned implements Runnable {
    /** One thread for the library, started with the first reader. */
    private static final Cleaner CLEANER = Cleaner.create();

    /** What each thread built and dropped unclosed, waiting for that thread to come back for it. */
    private static final ThreadLocal<ConcurrentLinkedQueue<Owned>> DROPPED =
            ThreadLocal.withInitial(ConcurrentLinkedQueue::new);

    /** The queue of the thread that built this, which is the only one that can close its arenas. */
    private final ConcurrentLinkedQueue<Owned> dropped;

    private final ArrayList<Arena> arenas = new ArrayList<>(2);
    private final ArrayList<Block> blocks = new ArrayList<>(8);
    /** {@code close()} freed the memory: there is nothing for the collector to hand over. */
    private volatile boolean released;

    private Cleaner.Cleanable watch;

    private Owned(ConcurrentLinkedQueue<Owned> dropped) {
        this.dropped = dropped;
    }

    /**
     * Starts the account of a reader this thread is about to build, first closing whatever the
     * collector has found of the ones it built before and dropped.
     */
    static Owned begin() {
        ConcurrentLinkedQueue<Owned> queue = DROPPED.get();
        for (Owned found = queue.poll(); found != null; found = queue.poll()) {
            found.free();
        }
        return new Owned(queue);
    }

    /** A confined arena of the reader's. */
    Arena arena() {
        return hold(Arena.ofConfined());
    }

    /** Takes an arena the reader was built around. */
    Arena hold(Arena arena) {
        arenas.add(arena);
        return arena;
    }

    /** A block of the reader's. */
    Block block(long bytes, long alignment) {
        return hold(new Block(bytes, alignment));
    }

    /** Takes a block the reader was built around. */
    Block hold(Block block) {
        blocks.add(block);
        return block;
    }

    /** Has the collector say when {@code owner}, now whole, can no longer be reached. */
    void watch(Object owner) {
        watch = CLEANER.register(owner, this);
    }

    /** The owner closed everything itself. */
    void release() {
        released = true;
        if (watch != null) {
            watch.clean();
        }
    }

    /** On the cleaner's thread, once the owner is unreachable: hands the memory to the owner's thread. */
    @Override
    public void run() {
        if (!released) {
            dropped.add(this);
        }
    }

    /** On the owner's thread: closes what the owner never did. */
    private void free() {
        for (Block block : blocks) {
            block.close();
        }
        for (Arena arena : arenas) {
            if (arena.scope().isAlive()) {
                arena.close();
            }
        }
    }
}
