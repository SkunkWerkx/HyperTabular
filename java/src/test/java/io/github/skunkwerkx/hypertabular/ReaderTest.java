package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

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
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.NoSuchFileException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.concurrent.atomic.AtomicReference;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

/** What the corpus does not reach: the binding's own surface, its buffers and its caller bugs. */
final class ReaderTest {
    private static final byte[] ORDERS =
            utf8("id,name,score\n1,alice,2.5\n2,\"bob, jr\",x\n3,,7\n");

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
            assertEquals(3, reader.columnCount());
            assertEquals(Door.F64, reader.column(2).door());
            assertEquals(0, reader.rows());
            assertTrue(reader.read());
            assertEquals(3, reader.rows());

            // A column at a time, as the core wrote it.
            MemorySegment ids = reader.values(0);
            assertEquals(3 * Integer.BYTES, ids.byteSize());
            assertTrue(ids.isReadOnly());
            assertEquals(List.of(1, 2, 3), List.of(
                    ids.getAtIndex(ValueLayout.JAVA_INT, 0),
                    ids.getAtIndex(ValueLayout.JAVA_INT, 1),
                    ids.getAtIndex(ValueLayout.JAVA_INT, 2)));
            MemorySegment scores = reader.values(2);
            assertEquals(2.5, scores.getAtIndex(ValueLayout.JAVA_DOUBLE, 0));
            assertEquals(0.0, scores.getAtIndex(ValueLayout.JAVA_DOUBLE, 1));
            assertEquals(7.0, scores.getAtIndex(ValueLayout.JAVA_DOUBLE, 2));
            assertEquals(3 * CellVerdict.LAYOUT.byteSize(), reader.verdicts(2).byteSize());
            assertEquals(new CellVerdict(CastFailure.MALFORMED, 0, 1), reader.verdict(2, 1));
            assertTrue(reader.isOk(2, 0));
            assertFalse(reader.isOk(2, 1));

            // A cell at a time, as HyperCast's union.
            String described = switch (reader.f64(2, 1)) {
                case Success<Double> score -> String.valueOf(score.value());
                case Fault<Double> fault -> fault.reason() + " in \"" + reader.rawString(2, 1) + "\"";
            };
            assertEquals("MALFORMED in \"x\"", described);
            assertEquals(new Success<>(2), reader.i32(0, 1));
            assertEquals("bob, jr", reader.string(1, 1));
            assertNull(reader.string(1, 2));
            assertNull(reader.text(1, 2));
            assertEquals(new CellVerdict(CastFailure.EMPTY, 0, 0), reader.verdict(1, 2));

            assertFalse(reader.read());
            assertEquals(0, reader.rows());
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
                assertTrue(reader.read());
                // A cell with no escaped quote is the input's own bytes: same address, no copy.
                MemorySegment plain = reader.text(0, 0);
                assertEquals(input.address(), plain.address());
                assertEquals(5, plain.byteSize());
                assertTrue(plain.isReadOnly());
                assertEquals(input.address(), reader.raw(0, 0).address());
                // One with "" inside is unescaped, into the reader's arena.
                assertEquals("say \"hi\"", reader.string(1, 0));
                assertEquals("say \"hi\"", reader.rawString(1, 0));
                assertEquals(8, reader.text(1, 0).byteSize());
            }
        }
    }

    @Test
    void readingAColumnAsTheWrongTypeIsACallerBug() {
        List<Column> plan = List.of(Column.i32(0), Column.timestamp(1));
        try (DelimitedReader reader = DelimitedReader.of(
                utf8("1,2024-01-31T10:30:00Z\n"), Dialect.CSV.withHeader(false), plan)) {
            assertTrue(reader.read());
            assertThrows(IllegalStateException.class, () -> reader.i64(0, 0));
            assertThrows(IllegalStateException.class, () -> reader.values(1));
            assertThrows(IllegalStateException.class, () -> reader.date(1, 0));
            assertThrows(IllegalStateException.class, () -> reader.timestamp(0, 0));
            assertThrows(IllegalStateException.class, () -> reader.text(0, 0));
            assertThrows(IllegalStateException.class, () -> reader.string(1, 0));
            assertThrows(IndexOutOfBoundsException.class, () -> reader.i32(0, 1));
            assertThrows(IndexOutOfBoundsException.class, () -> reader.i32(0, -1));
            assertThrows(IndexOutOfBoundsException.class, () -> reader.raw(0, 1));
            assertThrows(IndexOutOfBoundsException.class, () -> reader.isOk(2, 0));
        }
    }

    @Test
    void aPlanOrADialectTheCoreCannotHonourIsRefusedUpFront() {
        List<Column> plan = List.of(Column.i32(0));
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(ORDERS, new Dialect('"'), plan));
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(ORDERS, new Dialect('é'), plan));
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(ORDERS, new Dialect('\n'), plan));
        assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(ORDERS, Dialect.CSV, plan, 0));
        assertThrows(IllegalArgumentException.class,
                () -> DelimitedReader.of(new ByteArrayInputStream(ORDERS), Dialect.CSV, plan, 1, 0));
        assertThrows(NullPointerException.class,
                () -> DelimitedReader.of(ORDERS, Dialect.CSV, Arrays.asList(Column.i32(0), null)));
        assertThrows(IllegalArgumentException.class, () -> Column.i32(-1));
        assertThrows(NullPointerException.class, () -> Column.unix(0, null));
        assertThrows(NullPointerException.class, () -> Column.date(0, null));
        assertThrows(NullPointerException.class, () -> Column.dateTime(0, null));
        assertThrows(NullPointerException.class, () -> Column.excelSerial(0, null));
        assertThrows(NullPointerException.class, () -> Column.f64(0, null));
        // The core reads a zero decimal separator as "no format": it must not get one.
        assertThrows(IllegalArgumentException.class,
                () -> Column.f64(0, new NumFormat('\0', ',', NumFormat.STYLE_ALL)));
    }

    @Test
    void aColumnIsAValue() {
        assertEquals(Column.i32(3), Column.i32(3, NumFormat.INVARIANT));
        assertEquals(Column.i32(3).hashCode(), Column.i32(3, NumFormat.INVARIANT).hashCode());
        assertFalse(Column.i32(3).equals(Column.i32(4)));
        assertFalse(Column.i32(3).equals(Column.u32(3)));
        assertFalse(Column.date(0, DateOrder.DAY_MONTH_YEAR).equals(Column.date(0, DateOrder.MONTH_DAY_YEAR)));
        assertFalse(Column.f64(0).equals(Column.f64(0, NumFormat.DETECT)));
        assertEquals("Column[ordinal=2, door=UNIX, declared=2]",
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
        try (DelimitedReader reader = DelimitedReader.open(path, Dialect.CSV, List.of(Column.i64(0)),
                DelimitedReader.DEFAULT_BATCH_ROWS, 4096)) {
            long sum = 0;
            long rows = 0;
            while (reader.read()) {
                MemorySegment values = reader.values(0);
                for (int row = 0; row < reader.rows(); row++) {
                    sum += values.getAtIndex(ValueLayout.JAVA_LONG, row);
                }
                rows += reader.rows();
            }
            assertEquals(100_000L, rows);
            assertEquals(4_999_950_000L, sum);
            assertEquals(100_001L, reader.records());
        }
        UncheckedIOException missing = assertThrows(UncheckedIOException.class,
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
                assertTrue(reader.read());
                assertEquals(new Success<>(1), reader.i32(0, 0));
                assertEquals(plain, reader.string(1, 0));
                // The same source column through a second door: its raw text, unescaped.
                assertEquals(CastFailure.MALFORMED, reader.verdict(2, 0).reason());
                assertEquals(plain, reader.rawString(2, 0));
                // The batch may have ended where the arena did; the next row follows either way.
                int row = reader.rows() == 2 ? 1 : 0;
                if (reader.rows() == 1) {
                    assertTrue(reader.read());
                }
                assertEquals(new Success<>(2), reader.i32(0, row));
                assertEquals("short", reader.string(1, row));
                assertFalse(reader.read());
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
                while (reader.read()) {
                    for (int row = 0; row < reader.rows(); row++, rows++) {
                        sum += reader.values(0).getAtIndex(ValueLayout.JAVA_INT, row);
                    }
                }
                assertEquals(1000, rows);
                assertEquals(499_500, sum);
            }
            // A record no window can hold cannot be read, and says so — once, for good.
            MemorySegment wide = arena.allocateFrom(ValueLayout.JAVA_BYTE, utf8("1,x\n2,a very long cell indeed\n3,x\n"));
            try (DelimitedReader reader = DelimitedReader.windowed(wide, dialect, plan, 64, 17)) {
                assertTrue(reader.read());
                assertEquals(1, reader.rows());
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
        assertTrue(reader.read());
        MemorySegment name = reader.text(0, 0);
        assertEquals("alice", new String(name.toArray(ValueLayout.JAVA_BYTE), StandardCharsets.UTF_8));
        long records = reader.records();
        reader.close();
        reader.close();
        assertTrue(stream.closed);
        assertEquals(0, reader.rows());
        assertEquals(records, reader.records());
        assertThrows(IllegalStateException.class, reader::read);
        assertThrows(IndexOutOfBoundsException.class, () -> reader.text(0, 0));
        // A view handed out before the close is refused by the runtime, not left dangling.
        assertThrows(IllegalStateException.class, () -> name.get(ValueLayout.JAVA_BYTE, 0));
    }

    @Test
    void aReaderThatIsNeverHandedOutClosesItsStream() {
        // The header is read by the factory, so a broken one is thrown from it.
        Watched broken = new Watched(utf8("a,\"b\n"));
        TabularException failure = assertThrows(TabularException.class,
                () -> DelimitedReader.of(broken, Dialect.CSV, List.of(Column.i32(0))));
        assertEquals(TabularFailure.UNCLOSED_QUOTE, failure.failure());
        assertTrue(broken.closed);

        Watched refused = new Watched(ORDERS);
        assertThrows(IllegalArgumentException.class,
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
            assertTrue(reader.read());
            assertEquals(2, reader.rows());
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
            assertTrue(reader.read());
            assertEquals(3, reader.rows());
        }
    }

    @Test
    void aPlanCanBeAProjectionOrNothingAtAll() {
        // A source column read twice, out of order, and one past every record's end.
        List<Column> plan = List.of(Column.f64(2), Column.text(0), Column.i32(0), Column.text(7));
        try (DelimitedReader reader = DelimitedReader.of(ORDERS, Dialect.CSV, plan, 2)) {
            assertTrue(reader.read());
            assertEquals(2, reader.rows());
            assertEquals(new Success<>(2.5), reader.f64(0, 0));
            assertEquals("2", reader.string(1, 1));
            assertEquals(new Success<>(2), reader.i32(2, 1));
            assertNull(reader.string(3, 0));
            assertEquals("", reader.rawString(3, 0));
            assertTrue(reader.read());
            assertEquals(1, reader.rows());
            assertFalse(reader.read());
        }
        // No columns at all: the rows are still counted, and the structure still checked.
        try (DelimitedReader reader = DelimitedReader.of(ORDERS, Dialect.CSV, List.of())) {
            assertEquals(0, reader.columnCount());
            assertTrue(reader.read());
            assertEquals(3, reader.rows());
            assertFalse(reader.read());
        }
    }

    @Test
    void aStructuralFailureComesAfterTheIntactRows() {
        try (DelimitedReader reader = DelimitedReader.of(
                utf8("a,b\n1,2\n3\n"), Dialect.CSV, List.of(Column.i32(0)))) {
            assertTrue(reader.read());
            assertEquals(1, reader.rows());
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
}
