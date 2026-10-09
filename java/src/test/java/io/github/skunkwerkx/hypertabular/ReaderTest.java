package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.Success;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import java.io.BufferedWriter;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.nio.CharBuffer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.time.Instant;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.NoSuchElementException;
import java.util.OptionalInt;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/** What the corpus does not reach: the binding's own surface, its buffers and its caller bugs. */
final class ReaderTest {
    private static final byte[] ORDERS = utf8("id,name,score\n1,alice,2.5\n2,\"bob, jr\",x\n3,,7\n");

    private static byte[] utf8(String text) {
        return text.getBytes(StandardCharsets.UTF_8);
    }

    /** A stream that says whether it was closed. */
    private static final class Watched extends ByteArrayInputStream {
        boolean closed;

        Watched(byte[] bytes) {
            super(bytes);
        }

        @Override
        public void close() {
            closed = true;
        }
    }

    @Test
    void theNativeLibraryAnswersTheProbe() {
        assertTrue(Tabular.isAvailable());
        // The library that loaded is the one this binding was built against:
        // build.gradle.kts hands its own version to the test JVM, and rust/Cargo.toml moves
        // with it. A CI override may append a prerelease tag; the core carries only
        // major.minor.patch, so compare that much.
        String expected = System.getProperty("hypertabular.version").split("-", 2)[0];
        assertEquals(expected, Tabular.nativeVersion());
    }

    @Test
    void aColumnIsASegmentAndACellIsAUnion() {
        List<Column> plan = List.of(Column.i32(0), Column.text(1), Column.f64(2));
        try (DelimitedReader reader = DelimitedReader.of(ORDERS, Dialect.CSV, plan)) {
            assertEquals(List.of("id", "name", "score"), reader.header());
            assertEquals(plan, reader.plan());
            Batch batch = reader.read();
            assertEquals(3, batch.rows());
            assertEquals(Door.F64, batch.columns().get(2).door());
            assertEquals(List.of(2, 3, 4), List.of(batch.line(0), batch.line(1), batch.line(2)));

            // A column at a time, as the core wrote it.
            MemorySegment ids = batch.values(0);
            assertEquals(3 * Integer.BYTES, ids.byteSize());
            assertTrue(ids.isReadOnly());
            assertEquals(
                    List.of(1, 2, 3),
                    List.of(
                            ids.getAtIndex(ValueLayout.JAVA_INT, 0),
                            ids.getAtIndex(ValueLayout.JAVA_INT, 1),
                            ids.getAtIndex(ValueLayout.JAVA_INT, 2)));
            MemorySegment scores = batch.values(2);
            assertEquals(2.5, scores.getAtIndex(ValueLayout.JAVA_DOUBLE, 0));
            assertEquals(0.0, scores.getAtIndex(ValueLayout.JAVA_DOUBLE, 1));
            assertEquals(7.0, scores.getAtIndex(ValueLayout.JAVA_DOUBLE, 2));
            assertEquals(3 * CellVerdict.LAYOUT.byteSize(), batch.verdicts(2).byteSize());
            assertEquals(new CellVerdict(CastFailure.MALFORMED, 0, 1), batch.verdict(2, 1));
            assertTrue(batch.isOk(2, 0));
            assertFalse(batch.isOk(2, 1));

            // A cell at a time, as HyperCast's union.
            String described = switch (batch.get(2, 1, Double.class)) {
                case Success<Double> score -> String.valueOf(score.value());
                case Fault<Double> fault -> fault.reason() + " in \"" + batch.rawString(2, 1) + "\"";
            };
            assertEquals("MALFORMED in \"x\"", described);
            assertEquals(new Success<>(2), batch.get(0, 1, Integer.class));
            assertEquals("bob, jr", batch.string(1, 1));
            assertNull(batch.string(1, 2));
            assertNull(batch.text(1, 2));
            assertEquals(new CellVerdict(CastFailure.EMPTY, 0, 0), batch.verdict(1, 2));

            assertNull(reader.read());
            // The batch handed out last is over once the reader moves on.
            assertEquals(0, batch.rows());
            assertEquals(4, reader.records());
        }
    }

    @Test
    void textIsAViewOfTheCallersOwnMemory() {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment input = arena.allocateFrom(ValueLayout.JAVA_BYTE, utf8("plain,\"say \"\"hi\"\"\"\n"));
            List<Column> plan = List.of(Column.text(0), Column.text(1));
            try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV.withHeader(false), plan)) {
                assertNull(reader.header());
                Batch batch = reader.read();
                // A cell with no escaped quote is the input's own bytes: same address, no copy.
                MemorySegment plain = batch.text(0, 0);
                assertEquals(input.address(), plain.address());
                assertEquals(5, plain.byteSize());
                assertTrue(plain.isReadOnly());
                assertEquals(input.address(), batch.raw(0, 0).address());
                // One with "" inside is unescaped, into the reader's arena.
                assertEquals("say \"hi\"", batch.string(1, 0));
                assertEquals("say \"hi\"", batch.rawString(1, 0));
                assertEquals(8, batch.text(1, 0).byteSize());
            }
        }
    }

    @Test
    void readingAColumnAsTheWrongTypeIsACallerBug() {
        List<Column> plan = List.of(Column.i32(0), Column.timestamp(1));
        try (DelimitedReader reader =
                DelimitedReader.of(utf8("1,2024-01-31T10:30:00Z\n"), Dialect.CSV.withHeader(false), plan)) {
            Batch batch = reader.read();
            assertThrows(IllegalStateException.class, () -> batch.get(0, 0, Long.class));
            assertThrows(IllegalStateException.class, () -> batch.get(0, 0, Object.class));
            assertThrows(IllegalStateException.class, () -> batch.values(1));
            assertThrows(IllegalStateException.class, () -> batch.get(1, 0, LocalDate.class));
            assertThrows(IllegalStateException.class, () -> batch.get(0, 0, Instant.class));
            assertThrows(IllegalStateException.class, () -> batch.text(0, 0));
            assertThrows(IllegalStateException.class, () -> batch.string(1, 0));
            assertThrows(IndexOutOfBoundsException.class, () -> batch.get(0, 1, Integer.class));
            assertThrows(IndexOutOfBoundsException.class, () -> batch.get(0, -1, Integer.class));
            assertThrows(IndexOutOfBoundsException.class, () -> batch.raw(0, 1));
            assertThrows(IndexOutOfBoundsException.class, () -> batch.line(1));
            assertThrows(IndexOutOfBoundsException.class, () -> batch.isOk(2, 0));
        }
    }

    @Test
    void aPlanOrADialectTheCoreCannotHonourIsRefusedUpFront() {
        List<Column> plan = List.of(Column.i32(0));
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(ORDERS, new Dialect('"'), plan));
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(ORDERS, new Dialect('é'), plan));
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(ORDERS, new Dialect('\n'), plan));
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(ORDERS, Dialect.CSV, plan, 0));
        assertThrows(
                IllegalArgumentException.class,
                () -> DelimitedReader.of(new ByteArrayInputStream(ORDERS), Dialect.CSV, plan, 1, 0));
        assertThrows(
                NullPointerException.class,
                () -> DelimitedReader.of(ORDERS, Dialect.CSV, Arrays.asList(Column.i32(0), null)));
        assertThrows(IllegalArgumentException.class, () -> Column.i32(-1));
        assertThrows(NullPointerException.class, () -> Column.unix(0, null));
        assertThrows(NullPointerException.class, () -> Column.date(0, null));
        assertThrows(NullPointerException.class, () -> Column.dateTime(0, null));
        assertThrows(NullPointerException.class, () -> Column.excelSerial(0, null));
        assertThrows(NullPointerException.class, () -> Column.f64(0, null));
        // The core reads a zero decimal separator as "no format": it must not get one.
        assertThrows(
                IllegalArgumentException.class, () -> Column.f64(0, new NumFormat('\0', ',', NumFormat.STYLE_ALL)));
    }

    @Test
    void aColumnIsAValue() {
        assertEquals(Column.i32(3), Column.i32(3, NumFormat.INVARIANT));
        assertEquals(
                Column.i32(3).hashCode(), Column.i32(3, NumFormat.INVARIANT).hashCode());
        assertFalse(Column.i32(3).equals(Column.i32(4)));
        assertFalse(Column.i32(3).equals(Column.u32(3)));
        assertFalse(Column.date(0, DateOrder.DAY_MONTH_YEAR).equals(Column.date(0, DateOrder.MONTH_DAY_YEAR)));
        assertFalse(Column.f64(0).equals(Column.f64(0, NumFormat.DETECT)));
        assertEquals(
                "Column[ordinal=2, door=UNIX, declared=2]",
                Column.unix(2, UnixPrecision.MILLISECONDS).toString());
        assertEquals(Door.EXCEL_SERIAL, Column.excelSerial(0, ExcelEpoch.Y1904).door());
        // The doors are numbered as the core numbers them.
        for (Door door : Door.values()) {
            assertEquals(door.ordinal() + 1, door.code());
        }
    }

    @Test
    void aFileIsReadThroughTheSameReader(@TempDir Path directory) throws IOException {
        Path path = directory.resolve("numbers.csv");
        try (BufferedWriter file = Files.newBufferedWriter(path)) {
            file.write("n\n");
            for (int row = 0; row < 100_000; row++) {
                file.write(row + "\n");
            }
        }
        try (DelimitedReader reader = DelimitedReader.open(
                path, Dialect.CSV, List.of(Column.i64(0)), DelimitedReader.DEFAULT_BATCH_ROWS, 4096)) {
            long sum = 0;
            long rows = 0;
            for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                MemorySegment values = batch.values(0);
                for (int row = 0; row < batch.rows(); row++) {
                    sum += values.getAtIndex(ValueLayout.JAVA_LONG, row);
                }
                rows += batch.rows();
            }
            assertEquals(100_000L, rows);
            assertEquals(4_999_950_000L, sum);
            assertEquals(100_001L, reader.records());
        }
        UncheckedIOException missing = assertThrows(
                UncheckedIOException.class,
                () -> DelimitedReader.open(directory.resolve("absent.csv"), Dialect.CSV, List.of(Column.i64(0))));
        assertTrue(missing.getCause() instanceof NoSuchFileException);
    }

    @Test
    void aRecordLargerThanEveryBufferGrowsThem() {
        // One cell far past the stream buffer (which doubles), with enough "" in it to
        // outgrow the unescape arena (which is sized to what the row needs), and a header
        // wide enough to outgrow the name table and with a name that outgrows the arena.
        String quoted = "he said \"\"hi\"\" ".repeat(1000);
        String plain = "he said \"hi\" ".repeat(1000);
        StringBuilder text = new StringBuilder();
        List<String> names = new ArrayList<>();
        for (int column = 0; column < 100; column++) {
            names.add(column == 99 ? plain : "c" + column);
            text.append(column == 99 ? "\"" + quoted + "\"" : "c" + column).append(column == 99 ? "\n" : ",");
        }
        text.append("1,".repeat(99)).append('"').append(quoted).append("\"\n");
        text.append("2,".repeat(99)).append("short\n");

        List<Column> plan = List.of(Column.i32(0), Column.text(99), Column.i32(99));
        for (int bufferBytes : new int[] {1, 16, DelimitedReader.DEFAULT_BUFFER_BYTES}) {
            try (DelimitedReader reader = DelimitedReader.of(
                    new ByteArrayInputStream(utf8(text.toString())), Dialect.CSV, plan, 8, bufferBytes)) {
                assertEquals(names, reader.header());
                Batch batch = reader.read();
                assertEquals(new Success<>(1), batch.get(0, 0, Integer.class));
                assertEquals(plain, batch.string(1, 0));
                // The same source column through a second door: its raw text, unescaped.
                assertEquals(CastFailure.MALFORMED, batch.verdict(2, 0).reason());
                assertEquals(plain, batch.rawString(2, 0));
                // The batch may have ended where the arena did; the next row follows either way.
                int row = batch.rows() == 2 ? 1 : 0;
                if (batch.rows() == 1) {
                    batch = reader.read();
                }
                assertEquals(new Success<>(2), batch.get(0, row, Integer.class));
                assertEquals("short", batch.string(1, row));
                assertNull(reader.read());
            }
        }
    }

    @Test
    void memoryIsReadAWindowAtATime() {
        try (Arena arena = Arena.ofConfined()) {
            StringBuilder text = new StringBuilder();
            for (int row = 0; row < 1000; row++) {
                text.append(row).append(",x\n");
            }
            MemorySegment input = arena.allocateFrom(ValueLayout.JAVA_BYTE, utf8(text.toString()));
            List<Column> plan = List.of(Column.i32(0));
            Dialect dialect = Dialect.CSV.withHeader(false);
            try (DelimitedReader reader = DelimitedReader.windowed(input, dialect, plan, 64, 17)) {
                long sum = 0;
                long rows = 0;
                for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                    for (int row = 0; row < batch.rows(); row++, rows++) {
                        sum += batch.values(0).getAtIndex(ValueLayout.JAVA_INT, row);
                    }
                }
                assertEquals(1000, rows);
                assertEquals(499_500, sum);
            }
            // A record no window can hold cannot be read, and says so — once, for good.
            MemorySegment wide =
                    arena.allocateFrom(ValueLayout.JAVA_BYTE, utf8("1,x\n2,a very long cell indeed\n3,x\n"));
            try (DelimitedReader reader = DelimitedReader.windowed(wide, dialect, plan, 64, 17)) {
                assertEquals(1, reader.read().rows());
                TabularException failure = assertThrows(TabularException.class, reader::read);
                assertEquals(TabularFailure.ROW_TOO_LONG, failure.failure());
                assertEquals(1, failure.record());
                assertEquals(2, failure.line());
                assertEquals(4, failure.byteOffset());
                assertSame(failure, assertThrows(TabularException.class, reader::read));
            }
        }
    }

    @Test
    void closingReleasesTheBuffersAndTheStream() {
        Watched stream = new Watched(ORDERS);
        DelimitedReader reader = DelimitedReader.of(stream, Dialect.CSV, List.of(Column.text(1)));
        Batch batch = reader.read();
        MemorySegment name = batch.text(0, 0);
        assertEquals("alice", new String(name.toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8));
        long records = reader.records();
        reader.close();
        reader.close();
        assertTrue(stream.closed);
        assertEquals(0, batch.rows());
        assertEquals(records, reader.records());
        assertThrows(IllegalStateException.class, reader::read);
        assertThrows(IndexOutOfBoundsException.class, () -> batch.text(0, 0));
        // A view handed out before the close is refused by the runtime, not left dangling.
        assertThrows(IllegalStateException.class, () -> name.get(ValueLayout.JAVA_BYTE, 0));
    }

    @Test
    void aReaderThatIsNeverHandedOutClosesItsStream() {
        // The header is read by the factory, so a broken one is thrown from it.
        Watched broken = new Watched(utf8("a,\"b\n"));
        TabularException failure = assertThrows(
                TabularException.class, () -> DelimitedReader.of(broken, Dialect.CSV, List.of(Column.i32(0))));
        assertEquals(TabularFailure.UNCLOSED_QUOTE, failure.failure());
        assertTrue(broken.closed);

        Watched refused = new Watched(ORDERS);
        assertThrows(
                IllegalArgumentException.class,
                () -> DelimitedReader.of(refused, new Dialect('"'), List.of(Column.i32(0))));
        assertTrue(refused.closed);
    }

    @Test
    void aFailingStreamIsAnUncheckedIOException() {
        InputStream failing = new InputStream() {
            private int served;

            @Override
            public int read() throws IOException {
                throw new IOException("one byte at a time is not how this is read");
            }

            @Override
            public int read(byte[] into, int offset, int length) throws IOException {
                if (served > 0) {
                    throw new IOException("the disk went away");
                }
                byte[] first = utf8("n\n1\n2\n");
                System.arraycopy(first, 0, into, offset, first.length);
                served = first.length;
                return served;
            }
        };
        try (DelimitedReader reader = DelimitedReader.of(failing, Dialect.CSV, List.of(Column.i32(0)))) {
            assertEquals(List.of("n"), reader.header());
            assertEquals(2, reader.read().rows());
            UncheckedIOException failure = assertThrows(UncheckedIOException.class, reader::read);
            assertEquals("the disk went away", failure.getCause().getMessage());
        }
    }

    @Test
    void aReaderBelongsToTheThreadThatBuiltIt() throws InterruptedException {
        try (DelimitedReader reader = DelimitedReader.of(ORDERS, Dialect.CSV, List.of(Column.i32(0)))) {
            AtomicReference<Throwable> thrown = new AtomicReference<>();
            Thread other = new Thread(() -> {
                try {
                    reader.read();
                } catch (Throwable t) {
                    thrown.set(t);
                }
            });
            other.start();
            other.join();
            assertTrue(thrown.get() instanceof WrongThreadException, String.valueOf(thrown.get()));
            // And is none the worse for having been asked.
            assertEquals(3, reader.read().rows());
        }
    }

    @Test
    void aPlanCanBeAProjectionOrNothingAtAll() {
        // A source column read twice, out of order, and one past every record's end.
        List<Column> plan = List.of(Column.f64(2), Column.text(0), Column.i32(0), Column.text(7));
        try (DelimitedReader reader = DelimitedReader.of(ORDERS, Dialect.CSV, plan, 2)) {
            Batch batch = reader.read();
            assertEquals(2, batch.rows());
            assertEquals(new Success<>(2.5), batch.get(0, 0, Double.class));
            assertEquals("2", batch.string(1, 1));
            assertEquals(new Success<>(2), batch.get(2, 1, Integer.class));
            assertNull(batch.string(3, 0));
            assertEquals("", batch.rawString(3, 0));
            assertEquals(1, reader.read().rows());
            assertNull(reader.read());
        }
        // No columns at all: the rows are still counted, and the structure still checked.
        try (DelimitedReader reader = DelimitedReader.of(ORDERS, Dialect.CSV, List.of())) {
            assertEquals(List.of(), reader.plan());
            Batch batch = reader.read();
            assertEquals(3, batch.rows());
            assertEquals(4, batch.line(2));
            assertNull(reader.read());
        }
    }

    @Test
    void aStructuralFailureComesAfterTheIntactRows() {
        try (DelimitedReader reader = DelimitedReader.of(utf8("a,b\n1,2\n3\n"), Dialect.CSV, List.of(Column.i32(0)))) {
            assertEquals(1, reader.read().rows());
            TabularException failure = assertThrows(TabularException.class, reader::read);
            assertEquals(TabularFailure.COLUMN_COUNT, failure.failure());
            assertEquals(2, failure.record());
            assertEquals(3, failure.line());
            assertEquals(8, failure.byteOffset());
            assertEquals(2, failure.expected());
            assertEquals(1, failure.found());
            assertEquals("Record 2 (line 3, byte 8) has 1 cells; the first record had 2.", failure.getMessage());
        }
    }

    @Test
    void anArenaThatCrampsABatchIsGrown() {
        // Every row's one cell is escaped, so every row writes to the arena: a batch that
        // ends short because the arena filled up has the arena doubled for the next, and
        // twenty thousand such rows are read in a handful of batches, not thousands.
        String row = "\"" + "say \"\"hi\"\" ".repeat(8) + "\"\n";
        byte[] text = utf8(row.repeat(20_000));
        String expected = "say \"hi\" ".repeat(8);
        try (DelimitedReader reader =
                DelimitedReader.of(text, Dialect.CSV.withHeader(false), List.of(Column.text(0)))) {
            int batches = 0;
            int rows = 0;
            for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                batches++;
                rows += batch.rows();
                assertEquals(expected, batch.string(0, batch.rows() - 1));
            }
            assertEquals(20_000, rows);
            assertTrue(batches < 15, batches + " batches");
        }
    }

    @Test
    void aReaderOpenedWithoutAPlanReadsItsHeaderAndIsBoundOnce() {
        byte[] text = utf8("id,name,score,name\n1,alice,2.5,x\n2,bob,3.5,y\n");
        try (DelimitedReader reader = DelimitedReader.of(text, Dialect.CSV)) {
            Header header = reader.header();
            assertEquals(List.of("id", "name", "score", "name"), header);
            assertEquals(4, reader.columnCount().getAsInt());
            assertFalse(reader.isBound());
            assertEquals(List.of(), reader.plan());
            // Read before bind is a caller bug, and not for good.
            assertThrows(IllegalStateException.class, reader::read);
            assertThrows(IllegalStateException.class, reader::read);

            // Exact, case-sensitive and untrimmed; a duplicate resolves to the first.
            assertEquals(1, header.ordinal("name"));
            assertEquals(1, header.indexOf("name"));
            assertEquals(1, header.ordinal(utf8("name")));
            assertEquals(OptionalInt.of(2), header.findOrdinal("score"));
            assertEquals(OptionalInt.empty(), header.findOrdinal("Score"));
            assertEquals(OptionalInt.empty(), header.findOrdinal(" score"));
            assertEquals(OptionalInt.empty(), header.findOrdinal(utf8("nam")));
            NoSuchElementException missing = assertThrows(NoSuchElementException.class, () -> header.ordinal("total"));
            assertTrue(missing.getMessage().contains("\"total\""), missing.getMessage());
            missing = assertThrows(NoSuchElementException.class, () -> header.ordinal(utf8("tötal")));
            assertTrue(missing.getMessage().contains("\"tötal\""), missing.getMessage());
            assertEquals("score", CorpusTest.utf8(header.utf8(2)));
            assertThrows(UnsupportedOperationException.class, () -> header.add("more"));
            assertThrows(IndexOutOfBoundsException.class, () -> header.get(4));

            // A plan refused leaves the reader without one, to be bound again.
            assertThrows(IllegalArgumentException.class, () -> reader.bind(List.of(Column.i32(0)), 0));
            assertFalse(reader.isBound());
            List<Column> plan = List.of(Column.i32(header.ordinal("id")), Column.f64(header.ordinal("score")));
            reader.bind(plan, 1);
            assertTrue(reader.isBound());
            assertEquals(plan, reader.plan());
            assertThrows(IllegalStateException.class, () -> reader.bind(plan));
            assertEquals(new Success<>(2.5), reader.read().get(1, 0, Double.class));
            assertEquals(new Success<>(2), reader.read().get(0, 0, Integer.class));
            assertNull(reader.read());
        }
        // A plan the reader is opened with is still checked before the header is read.
        assertThrows(
                IllegalArgumentException.class,
                () -> DelimitedReader.of(utf8("a,\"b"), Dialect.CSV, List.of(Column.i32(0)), 0));
        try (DelimitedReader reader = DelimitedReader.of(text, Dialect.CSV, List.of(Column.i32(0)))) {
            assertTrue(reader.isBound());
            assertThrows(IllegalStateException.class, () -> reader.bind(List.of(Column.i32(0))));
        }
    }

    @Test
    void theColumnCountIsTheFirstRecordsWithoutAHeader() {
        try (DelimitedReader reader = DelimitedReader.of(utf8("1,2,3\n4,5,6\n"), Dialect.CSV.withHeader(false))) {
            assertNull(reader.header());
            assertEquals(OptionalInt.empty(), reader.columnCount());
            reader.bind(List.of(Column.i32(2)));
            assertEquals(2, reader.read().rows());
            assertEquals(OptionalInt.of(3), reader.columnCount());
            reader.close();
            assertEquals(OptionalInt.of(3), reader.columnCount());
            assertThrows(IllegalStateException.class, () -> reader.bind(List.of()));
        }
        try (DelimitedReader reader = DelimitedReader.of(new byte[0], Dialect.CSV)) {
            assertEquals(List.of(), reader.header());
            assertEquals(OptionalInt.empty(), reader.columnCount());
        }
    }

    @Test
    void everyFactoryHasAFormWithoutAPlan(@TempDir Path directory) throws IOException {
        Path file = directory.resolve("orders.csv");
        Files.write(file, ORDERS);
        Watched stream = new Watched(ORDERS);
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment memory = arena.allocateFrom(ValueLayout.JAVA_BYTE, ORDERS);
            List<DelimitedReader> readers = List.of(
                    DelimitedReader.of(ORDERS, Dialect.CSV),
                    DelimitedReader.of(memory, Dialect.CSV),
                    DelimitedReader.of(new ByteArrayInputStream(ORDERS), Dialect.CSV),
                    DelimitedReader.of(stream, Dialect.CSV, 3),
                    DelimitedReader.open(file, Dialect.CSV),
                    DelimitedReader.open(file, Dialect.CSV, 3));
            for (DelimitedReader reader : readers) {
                try (reader) {
                    reader.bind(List.of(Column.text(reader.header().ordinal("name"))));
                    List<String> names = new ArrayList<>();
                    reader.forEachRow(row -> names.add(row.string(0)));
                    assertEquals(Arrays.asList("alice", "bob, jr", null), names);
                }
            }
        }
        assertTrue(stream.closed);
        Watched refused = new Watched(ORDERS);
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(refused, Dialect.CSV, 0));
        assertTrue(refused.closed, "a reader never handed out closes its stream");
        assertThrows(
                UncheckedIOException.class, () -> DelimitedReader.open(directory.resolve("missing.csv"), Dialect.CSV));
    }

    @Test
    void aRowIsTheBatchsAccessorsAtOneRow() {
        List<Column> plan = List.of(Column.i32(0), Column.text(1), Column.f64(2));
        try (DelimitedReader reader = DelimitedReader.of(ORDERS, Dialect.CSV, plan)) {
            Batch batch = reader.read();
            CorpusTest.assertRows("orders", batch);
            Row bob = batch.row(1);
            assertEquals(1, bob.index());
            assertEquals(3, bob.line());
            assertEquals(new Success<>(2), bob.get(0, Integer.class));
            assertEquals("bob, jr", bob.string(1));
            assertEquals("bob, jr", bob.chars(1).toString());
            assertEquals("bob, jr", CorpusTest.utf8(bob.text(1)));
            assertFalse(bob.isOk(2));
            assertEquals(CastFailure.MALFORMED, bob.verdict(2).reason());
            assertEquals("x", bob.rawString(2));
            assertEquals("x", CorpusTest.utf8(bob.raw(2)));
            char[] into = new char[7];
            assertEquals(7, bob.getChars(1, into, 0));
            assertEquals("bob, jr", new String(into));
            assertEquals(-1, bob.getChars(1, into, 1));
            assertNull(batch.row(2).chars(1));
            assertEquals(0, batch.row(2).getChars(1, into, 7));
            assertThrows(IndexOutOfBoundsException.class, () -> batch.row(3));
            assertThrows(IllegalStateException.class, () -> bob.chars(0));
            java.util.Iterator<Row> rows = batch.iterator();
            for (int row = 0; row < 3; row++) {
                assertEquals(row, rows.next().index());
            }
            assertFalse(rows.hasNext());
            assertThrows(NoSuchElementException.class, rows::next);
        }
    }

    @Test
    void forEachRowReadsEveryBatchAndStopsAtTheEnd() {
        StringBuilder text = new StringBuilder("n\n");
        for (int n = 0; n < 10; n++) {
            text.append(n).append('\n');
        }
        for (boolean headerFirst : new boolean[] {false, true}) {
            try (DelimitedReader reader = headerFirst
                    ? DelimitedReader.of(utf8(text.toString()), Dialect.CSV)
                    : DelimitedReader.of(utf8(text.toString()), Dialect.CSV, List.of(Column.i32(0)), 3)) {
                if (headerFirst) {
                    reader.bind(List.of(Column.i32(0)), 3);
                }
                List<Integer> seen = new ArrayList<>();
                List<Integer> lines = new ArrayList<>();
                reader.forEachRow(row -> {
                    seen.add(((Success<Integer>) row.get(0, Integer.class)).value());
                    lines.add(row.line());
                });
                // Four batches of at most three.
                assertEquals(List.of(0, 1, 2, 3, 4, 5, 6, 7, 8, 9), seen);
                assertEquals(List.of(2, 3, 4, 5, 6, 7, 8, 9, 10, 11), lines);
                assertNull(reader.read());
                reader.forEachRow(row -> fail("no rows are left"));
            }
        }
        // A structural failure comes out of the loop after the intact rows.
        try (DelimitedReader reader = DelimitedReader.of(utf8("a,b\n1,2\n3\n"), Dialect.CSV, List.of(Column.i32(0)))) {
            List<Integer> lines = new ArrayList<>();
            TabularException failure =
                    assertThrows(TabularException.class, () -> reader.forEachRow(row -> lines.add(row.line())));
            assertEquals(TabularFailure.COLUMN_COUNT, failure.failure());
            assertEquals(List.of(2), lines);
        }
    }

    @Test
    void charsAreDecodedIntoAnArenaThatKeepsEveryEarlierView() {
        // Wider than one arena, with characters outside the BMP (two chars from four bytes)
        // and inside it (one from two or three), so the char count is not the byte count.
        StringBuilder text = new StringBuilder();
        List<String> cells = new ArrayList<>();
        for (int n = 0; n < 300; n++) {
            String cell = "row " + n + " é€😀 " + "x".repeat(n % 17);
            cells.add(cell);
            text.append(cell).append('\n');
        }
        for (boolean stingy : new boolean[] {false, true}) {
            Batch.stingyChars = stingy;
            Batch.charsGrown = 0;
            try (DelimitedReader reader = DelimitedReader.of(
                    utf8(text.toString()), Dialect.CSV.withHeader(false), List.of(Column.text(0)), 200)) {
                int seen = 0;
                for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                    List<CharBuffer> views = new ArrayList<>();
                    for (int row = 0; row < batch.rows(); row++) {
                        views.add(batch.chars(0, row));
                    }
                    // Every view handed out this batch is still its cell's, after all the rest.
                    for (int row = 0; row < batch.rows(); row++) {
                        assertEquals(cells.get(seen + row), views.get(row).toString());
                        assertTrue(views.get(row).isReadOnly());
                        assertEquals(0, views.get(row).position());
                    }
                    seen += batch.rows();
                }
                assertEquals(300, seen);
            } finally {
                Batch.stingyChars = false;
            }
            // From one cell's room, the arena was replaced for nearly every cell; from the
            // ordinary start, a handful of times.
            assertTrue(stingy ? Batch.charsGrown > 200 : Batch.charsGrown <= 5, "grown " + Batch.charsGrown);
        }
    }

    @Test
    void textThatIsNotUtf8IsReplacedAsTheStringIs() {
        byte[][] cells = {
            {'a', (byte) 0xFF, 'b'},
            {(byte) 0xC3},
            {'x', (byte) 0xE2, (byte) 0x82},
            {(byte) 0xED, (byte) 0xA0, (byte) 0x80, 'z'},
            {(byte) 0xC0, (byte) 0xAF},
            {(byte) 0xF4, (byte) 0x90, (byte) 0x80, (byte) 0x80},
            {(byte) 0xF0, (byte) 0x9F, (byte) 0x98},
            {(byte) 0xE0, (byte) 0x80, (byte) 0x80, 'q'},
        };
        java.io.ByteArrayOutputStream text = new java.io.ByteArrayOutputStream();
        for (byte[] cell : cells) {
            text.writeBytes(cell);
            text.write('\n');
        }
        try (DelimitedReader reader = DelimitedReader.of(
                text.toByteArray(), Dialect.CSV.withHeader(false), List.of(Column.text(0), Column.i32(0)))) {
            Batch batch = reader.read();
            assertEquals(cells.length, batch.rows());
            for (int row = 0; row < batch.rows(); row++) {
                String label = "cell " + row;
                String expected = new String(cells[row], StandardCharsets.UTF_8);
                assertEquals(expected, batch.string(0, row), label);
                assertEquals(expected, batch.chars(0, row).toString(), label);
                char[] into = new char[expected.length()];
                assertEquals(expected.length(), batch.getChars(0, row, into, 0), label);
                assertEquals(expected, new String(into), label);
                if (expected.length() > 1) {
                    assertEquals(-1, batch.getChars(0, row, new char[expected.length() - 1], 0), label);
                }
            }
        }
    }
}
