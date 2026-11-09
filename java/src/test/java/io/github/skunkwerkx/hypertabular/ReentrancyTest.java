package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import java.io.IOException;
import java.io.InputStream;
import java.io.InterruptedIOException;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.channels.ClosedByInterruptException;
import java.nio.charset.Charset;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Random;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.atomic.AtomicReference;
import java.util.function.Supplier;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/**
 * Caller code in the middle of a read: the stream a reader reads runs between the reader's own
 * steps, and anything it calls back into that reader is refused, never answered from buffers the
 * read has offsets into. And memory another thread could free in the middle of a native call is
 * never read in place where that call can't hold it.
 */
final class ReentrancyTest {
    private static final String BUSY = "The reader is in the middle of a read; its stream can't call back into it.";
    private static final List<Column> PLAN = List.of(Column.text(0), Column.text(1));
    private static final List<Encoding> ENCODINGS = List.of(Encoding.UTF_8, Encoding.LATIN_1, Encoding.UTF_16LE);

    private record Line(int line, String id, String name) {}

    private record Data(byte[] bytes, List<Line> rows) {}

    /** A header and {@code count} rows of an id and a name of seeded length, an {@code é} every seventh. */
    private static Data data(int count, int seed, Encoding encoding) {
        Random random = new Random(seed);
        StringBuilder text = new StringBuilder("id,name\n");
        List<Line> rows = new ArrayList<>();
        for (int index = 0; index < count; index++) {
            String name = "n" + index
                    + String.valueOf((char) ('a' + index % 26)).repeat(random.nextInt(40))
                    + (index % 7 == 0 ? "é" : "");
            text.append(index).append(',').append(name).append('\n');
            rows.add(new Line(index + 2, Integer.toString(index), name));
        }
        Charset charset = switch (encoding) {
            case UTF_16LE -> StandardCharsets.UTF_16LE;
            case LATIN_1 -> StandardCharsets.ISO_8859_1;
            default -> StandardCharsets.UTF_8;
        };
        return new Data(text.toString().getBytes(charset), rows);
    }

    private static void take(Batch batch, List<Line> into) {
        for (int row = 0; batch != null && row < batch.rows(); row++) {
            into.add(new Line(batch.line(row), batch.string(0, row), batch.string(1, row)));
        }
    }

    private static List<Line> readAll(DelimitedReader reader, List<Line> into) {
        for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
            take(batch, into);
        }
        return into;
    }

    private static MemorySegment held(Arena arena, byte[] bytes) {
        MemorySegment segment = arena.allocate(bytes.length);
        MemorySegment.copy(bytes, 0, segment, ValueLayout.JAVA_BYTE, 0, bytes.length);
        return segment;
    }

    private static boolean jni() {
        return Native.backend().equals("jni");
    }

    /** What runs at the start of each read of a {@link HookStream}, with the read's number from 1. */
    @FunctionalInterface
    private interface Hook {
        void at(int read) throws IOException;
    }

    /** A stream over bytes, 1 to 64 of them a read in a seeded order, that runs a hook first. */
    private static final class HookStream extends InputStream {
        private final byte[] bytes;
        private final Random chunks;
        private int position;
        int reads;
        boolean closed;
        /** Once set, from the hook or before, every read says the stream has ended. */
        boolean ended;

        Hook hook = read -> {};

        HookStream(byte[] bytes, int seed) {
            this.bytes = bytes;
            this.chunks = new Random(seed);
        }

        @Override
        public int read() {
            throw new UnsupportedOperationException("read a byte at a time");
        }

        @Override
        public int read(byte[] into, int offset, int length) throws IOException {
            hook.at(++reads);
            if (ended || position == bytes.length) {
                return -1;
            }
            int chunk = Math.min(Math.min(length, 1 + chunks.nextInt(64)), bytes.length - position);
            System.arraycopy(bytes, position, into, offset, chunk);
            position += chunk;
            return chunk;
        }

        @Override
        public void close() {
            closed = true;
        }
    }

    @Test
    void aCallFromTheStreamBackIntoItsReaderIsRefusedAndTheReadGoesOnWhole() {
        for (Encoding encoding : ENCODINGS) {
            for (int seed = 0; seed < 20; seed++) {
                Data data = data(300, seed, encoding);
                HookStream stream = new HookStream(data.bytes(), seed);
                AtomicReference<DelimitedReader> opened = new AtomicReference<>();
                int[] refused = {0};
                boolean[] inside = {false};
                // Every read of the stream once the reader exists: with a 16-byte buffer and seven-row
                // batches that is mid-record, mid-batch, while the buffer grows and, under an encoding,
                // mid-character.
                stream.hook = read -> {
                    DelimitedReader reader = opened.get();
                    // Once per read of the stream: a call that got through would read the stream again.
                    if (reader == null || inside[0]) {
                        return;
                    }
                    inside[0] = true;
                    assertEquals(
                            BUSY,
                            assertThrows(IllegalStateException.class, reader::read)
                                    .getMessage());
                    assertEquals(
                            BUSY,
                            assertThrows(IllegalStateException.class, () -> reader.bind(PLAN))
                                    .getMessage());
                    assertEquals(
                            BUSY,
                            assertThrows(IllegalStateException.class, () -> reader.forEachRow(row -> {}))
                                    .getMessage());
                    refused[0]++;
                    inside[0] = false;
                };
                try (DelimitedReader reader =
                        DelimitedReader.of(stream, Dialect.CSV.withEncoding(encoding), PLAN, 7, 16)) {
                    opened.set(reader);
                    assertEquals(data.rows(), readAll(reader, new ArrayList<>()), encoding + " seed " + seed);
                    assertTrue(refused[0] > 10, encoding + " seed " + seed);
                }
            }
        }
    }

    @Test
    void aCloseFromTheStreamEndsThatReadAndEveryLaterOne() {
        // Every close point from the first refill on, over seeds that move the record and character
        // boundaries, with a stream that goes on giving bytes or says it has ended once closed: the
        // read the close fell in is the one that throws.
        for (Encoding encoding : ENCODINGS) {
            for (int point : new int[] {1, 2, 3, 5, 9, 17, 31, 60}) {
                for (int seed = 0; seed < 4; seed++) {
                    for (boolean ends : new boolean[] {false, true}) {
                        String context = encoding + " point " + point + " seed " + seed + " ends " + ends;
                        Data data = data(300, seed, encoding);
                        HookStream stream = new HookStream(data.bytes(), seed);
                        AtomicReference<DelimitedReader> opened = new AtomicReference<>();
                        int[] reads = {0};
                        int[] fired = {0};
                        int[] from = {Integer.MAX_VALUE};
                        stream.hook = read -> {
                            if (fired[0] == 0 && read >= from[0]) {
                                fired[0] = reads[0];
                                opened.get().close();
                                stream.ended = ends;
                            }
                        };
                        DelimitedReader reader =
                                DelimitedReader.of(stream, Dialect.CSV.withEncoding(encoding), PLAN, 7, 16);
                        opened.set(reader);
                        from[0] = stream.reads + point;
                        List<Line> got = new ArrayList<>();
                        Batch[] kept = {null};
                        IllegalStateException ended = assertThrows(
                                IllegalStateException.class,
                                () -> {
                                    for (; ; ) {
                                        reads[0]++;
                                        Batch batch = reader.read();
                                        if (batch == null) {
                                            break;
                                        }
                                        assertEquals(
                                                0,
                                                fired[0],
                                                "no batch comes out of the read the close fell in, " + context);
                                        kept[0] = batch;
                                        take(batch, got);
                                    }
                                },
                                context);
                        assertEquals("The reader was closed in the middle of a read.", ended.getMessage(), context);
                        assertEquals(
                                reads[0], fired[0], "the read the close fell in is the one that throws, " + context);
                        assertEquals(
                                data.rows().subList(0, got.size()),
                                got,
                                "what came before the close is a correct prefix, " + context);
                        assertTrue(stream.closed, "the close was done, the stream's included, " + context);
                        if (kept[0] != null) {
                            assertEquals(0, kept[0].rows(), "the batch handed out before is over, " + context);
                        }
                        assertEquals(
                                "The reader is closed.",
                                assertThrows(IllegalStateException.class, reader::read)
                                        .getMessage(),
                                context);
                        reader.close();
                    }
                }
            }
        }
    }

    @Test
    void aReadThatFailsLeavesTheReaderFreeForTheNext() {
        for (Encoding encoding : ENCODINGS) {
            Data data = data(300, 5, encoding);
            HookStream stream = new HookStream(data.bytes(), 5);
            boolean[] opened = {false};
            stream.hook = read -> {
                if (opened[0] && read % 6 == 0) {
                    throw new IOException("the stream failed");
                }
            };
            try (DelimitedReader reader = DelimitedReader.of(stream, Dialect.CSV.withEncoding(encoding), PLAN, 7, 16)) {
                opened[0] = true;
                List<Line> got = new ArrayList<>();
                int failures = 0;
                for (boolean done = false; !done; ) {
                    try {
                        Batch batch = reader.read();
                        done = batch == null;
                        take(batch, got);
                    } catch (UncheckedIOException failed) {
                        failures++;
                    }
                }
                assertTrue(failures > 0);
                assertEquals(data.rows(), got, encoding.toString());
            }
        }
    }

    @Test
    void aCallFromAnotherThreadInTheMiddleOfAReadIsRefusedWithoutTouchingTheRead() {
        Data data = data(300, 2, Encoding.UTF_8);
        HookStream stream = new HookStream(data.bytes(), 2);
        AtomicReference<DelimitedReader> opened = new AtomicReference<>();
        List<Throwable> seen = new ArrayList<>();
        int[] at = {Integer.MAX_VALUE};
        stream.hook = read -> {
            if (read != at[0]) {
                return;
            }
            DelimitedReader reader = opened.get();
            List<Runnable> calls = List.of(reader::read, reader::close, () -> reader.bind(PLAN));
            Thread other = new Thread(() -> {
                for (Runnable call : calls) {
                    try {
                        call.run();
                        seen.add(null);
                    } catch (RuntimeException refused) {
                        seen.add(refused);
                    }
                }
            });
            other.start();
            try {
                other.join();
            } catch (InterruptedException e) {
                throw new InterruptedIOException();
            }
        };
        try (DelimitedReader reader = DelimitedReader.of(stream, Dialect.CSV, PLAN, 7, 16)) {
            opened.set(reader);
            at[0] = stream.reads + 4;
            assertEquals(data.rows(), readAll(reader, new ArrayList<>()));
        }
        assertEquals(3, seen.size());
        for (Throwable refused : seen) {
            assertInstanceOf(WrongThreadException.class, refused);
        }
    }

    @Test
    void aSegmentAnotherThreadCouldFreeMidCallIsCopiedWhereTheNativeCallCantHoldIt() throws IOException {
        Data data = data(2000, 4, Encoding.UTF_8);
        Arena shared = Arena.ofShared();
        try (DelimitedReader reader = DelimitedReader.of(held(shared, data.bytes()), Dialect.CSV, PLAN, 64)) {
            shared.close();
            if (jni()) {
                // Nothing holds the arena across a JNI call, so the reader read its own copy.
                assertEquals(data.rows(), readAll(reader, new ArrayList<>()));
            } else {
                // A downcall holds it for the call; between calls it must stay alive, as documented.
                assertThrows(IllegalStateException.class, reader::read);
            }
        }

        // A confined, automatic or global arena's segment is read in place on either path: a change
        // after open is what the read sees.
        byte[] text = "id,name\n1,a\n".getBytes(StandardCharsets.UTF_8);
        for (Arena arena : List.of(Arena.ofConfined(), Arena.ofAuto(), Arena.global())) {
            MemorySegment segment = held(arena, text);
            try (DelimitedReader reader = DelimitedReader.of(segment, Dialect.CSV, PLAN)) {
                segment.set(ValueLayout.JAVA_BYTE, text.length - 2, (byte) 'b');
                assertEquals("b", reader.read().string(1, 0), arena.toString());
            }
        }

        Path basic = CorpusTest.CORPUS_DIRECTORY.resolve("workbook").resolve("basic.xlsx");
        List<Column> first = List.of(Column.text(0));
        List<String> want = new ArrayList<>();
        try (Workbook book = Workbook.open(basic);
                Sheet sheet = book.sheet(0, SheetOptions.DEFAULT, first)) {
            for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                for (int row = 0; row < batch.rows(); row++) {
                    want.add(batch.string(0, row));
                }
            }
        }
        Arena sharedBook = Arena.ofShared();
        try (Workbook book = Workbook.of(held(sharedBook, Files.readAllBytes(basic)))) {
            sharedBook.close();
            if (jni()) {
                List<String> got = new ArrayList<>();
                try (Sheet sheet = book.sheet(0, SheetOptions.DEFAULT, first)) {
                    for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                        for (int row = 0; row < batch.rows(); row++) {
                            got.add(batch.string(0, row));
                        }
                    }
                }
                assertEquals(want, got);
            } else {
                assertThrows(IllegalStateException.class, () -> {
                    try (Sheet sheet = book.sheet(0, SheetOptions.DEFAULT, first)) {
                        sheet.read();
                    }
                });
            }
        }
    }

    @Test
    void aSharedArenaClosedByAnotherThreadMidReadNeverEndsAReadShortWithoutAnError() throws InterruptedException {
        Data data = data(200_000, 5, Encoding.UTF_8);
        for (int trial = 0; trial < 6; trial++) {
            Arena shared = Arena.ofShared();
            MemorySegment text = held(shared, data.bytes());
            CountDownLatch reading = new CountDownLatch(1);
            long wait = trial;
            Thread closer = new Thread(() -> {
                try {
                    reading.await();
                    Thread.sleep(wait);
                    shared.close();
                } catch (IllegalStateException refused) {
                    // FFM: the arena is held by the call in progress.
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
            });
            List<Line> got = new ArrayList<>();
            try (DelimitedReader reader = DelimitedReader.of(text, Dialect.CSV, PLAN, 1_000_000)) {
                closer.start();
                reading.countDown();
                try {
                    readAll(reader, got);
                    assertEquals(data.rows(), got, "trial " + trial);
                } catch (IllegalStateException refused) {
                    // Only FFM refuses, between calls: the JNI path has its own copy.
                    assertFalse(jni(), "trial " + trial + ": " + refused);
                    assertEquals(data.rows().subList(0, got.size()), got);
                }
            }
            // JDK 25 can leave this thread interrupted when the arena's close races a Java-side access
            // of the segment; a flag carried on would end a later test's file read. Clear it before
            // the join too, which throws on a set flag while the closer is still alive.
            Thread.interrupted();
            closer.join();
            Thread.interrupted();
            if (shared.scope().isAlive()) {
                shared.close();
            }
        }
    }

    @Test
    void aThreadInterruptThatClosesAFileEndsTheReaderAndEveryLaterReadSaysSo(@TempDir Path directory)
            throws IOException {
        Data data = data(2000, 6, Encoding.UTF_8);
        Path file = directory.resolve("rows.csv");
        Files.write(file, data.bytes());
        try (DelimitedReader reader = DelimitedReader.open(file, Dialect.CSV, PLAN, 7, 64)) {
            List<Line> got = new ArrayList<>();
            take(reader.read(), got);
            UncheckedIOException first;
            Thread.currentThread().interrupt();
            try {
                first = assertThrows(UncheckedIOException.class, () -> {
                    for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                        take(batch, got);
                    }
                    fail("no read reached the file");
                });
            } finally {
                Thread.interrupted();
            }
            assertInstanceOf(ClosedByInterruptException.class, first.getCause());
            assertEquals(data.rows().subList(0, got.size()), got);
            UncheckedIOException later = assertThrows(UncheckedIOException.class, reader::read);
            InterruptedIOException why = assertInstanceOf(InterruptedIOException.class, later.getCause());
            assertEquals(
                    "A thread interrupt closed the file in the middle of a read; the reader can read no more of it.",
                    why.getMessage());
            assertSame(first.getCause(), why.getCause());
        }
    }

    @Test
    void pastAFullSampleOnlyAnIoFailureSaysTruncatedAndAnInterruptIsThrown() {
        byte[] text = "a,b\n1,x\n2,y\n3,z\n4,w\n".getBytes(StandardCharsets.UTF_8);
        RuntimeException io = new UncheckedIOException(new IOException("the disk failed"));
        assertTrue(sampleFailingPastIt(text, io)
                .judge(null, SuggestOptions.DEFAULT)
                .truncated());

        RuntimeException interrupt = new UncheckedIOException(new ClosedByInterruptException());
        assertSame(interrupt, assertThrows(UncheckedIOException.class, () -> sampleFailingPastIt(text, interrupt)));
        RuntimeException own = new IllegalStateException("the caller's own");
        assertSame(own, assertThrows(IllegalStateException.class, () -> sampleFailingPastIt(text, own)));
    }

    /** A sample of three rows whose source's first read is a reader's and whose next throws {@code failure}. */
    private static Sample sampleFailingPastIt(byte[] text, RuntimeException failure) {
        try (DelimitedReader reader = DelimitedReader.of(text, Dialect.CSV)) {
            int[] reads = {0};
            Supplier<Batch> source = () -> {
                if (reads[0]++ == 0) {
                    return reader.read();
                }
                throw failure;
            };
            return Sample.of(source, plan -> reader.bind(plan, 3), 2, 3, false);
        }
    }
}
