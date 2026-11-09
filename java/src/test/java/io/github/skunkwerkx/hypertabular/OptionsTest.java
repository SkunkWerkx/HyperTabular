package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTimeoutPreemptively;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.sun.management.OperatingSystemMXBean;
import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.DateOptions;
import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.FaultDetail;
import io.github.skunkwerkx.hypercast.Grouping;
import io.github.skunkwerkx.hypercast.NumFormat;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InterruptedIOException;
import java.io.UncheckedIOException;
import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.lang.management.ManagementFactory;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.DirectoryStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.ConcurrentModificationException;
import java.util.Iterator;
import java.util.List;
import java.util.OptionalInt;
import java.util.concurrent.CountDownLatch;
import java.util.stream.Collectors;
import java.util.stream.IntStream;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import org.junit.jupiter.api.Assumptions;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
import org.junit.jupiter.params.provider.CsvSource;
import org.junit.jupiter.params.provider.EnumSource;
import org.junit.jupiter.params.provider.ValueSource;

/**
 * What the corpus cannot say of the declarations a read takes and of what it gives back: the
 * errors, the limits, and what is left unsaid in a case's JSON.
 */
final class OptionsTest {
    private static final NumFormat COMMA_DECIMAL = new NumFormat(',', '.', NumFormat.STYLE_ALL);
    private static final NumFormat INDIAN = NumFormat.INVARIANT.withGrouping(Grouping.INDIAN);
    private static final Dialect HEADERLESS = Dialect.CSV.withHeader(false);

    private static byte[] utf8(String text) {
        return text.getBytes(StandardCharsets.UTF_8);
    }

    private static Path corpus(String file) {
        return CorpusTest.CORPUS_DIRECTORY.resolve("workbook").resolve(file);
    }

    private static int i32(Batch batch, int column, int row) {
        return batch.values(column).getAtIndex(ValueLayout.JAVA_INT, row);
    }

    private static double f64(Batch batch, int column, int row) {
        return batch.values(column).getAtIndex(ValueLayout.JAVA_DOUBLE, row);
    }

    @Test
    void aDeclarationThatIsNotOneIsRefused() {
        assertThrows(IllegalArgumentException.class, () -> Dialect.CSV.withSkip(-1));
        assertThrows(IllegalArgumentException.class, () -> Dialect.CSV.withHeaderRow(-1));
        assertThrows(NullPointerException.class, () -> Dialect.CSV.withShortRows(null));
        assertThrows(NullPointerException.class, () -> Dialect.CSV.withLongRows(null));
        assertThrows(NullPointerException.class, () -> Dialect.CSV.withEncoding(null));
        assertThrows(NullPointerException.class, () -> Dialect.CSV.withTokens((Tokens) null));
        assertThrows(IllegalArgumentException.class, () -> SheetOptions.DEFAULT.withSkip(-1));
        assertThrows(IllegalArgumentException.class, () -> SheetOptions.DEFAULT.withHeaderRow(-1));
        assertThrows(NullPointerException.class, () -> SheetOptions.DEFAULT.withRange(null));
        assertThrows(IllegalArgumentException.class, () -> new DateOptions(0, 5));
        assertThrows(IllegalArgumentException.class, () -> new DateOptions(1 << 7, 0));
        assertThrows(NullPointerException.class, () -> Tokens.of("NA", null));
    }

    /** Each declaration, on and off, on one input, so that each is seen to be what changes it. */
    @Test
    void everyDelimitedDeclarationDoesWhatItSays() {
        // skip: the first line is passed over, an unclosed quote and all.
        List<String> skipped = column(Dialect.CSV.withSkip(1), "\"junk\na\n1\n");
        assertEquals(List.of("1"), skipped);
        assertThrows(TabularException.class, () -> column(Dialect.CSV, "\"junk\na\n1\n"));
        // headerRow: a record is discarded ahead of the header.
        assertEquals(List.of("2"), column(Dialect.CSV.withHeaderRow(1), "x\na\n2\n"));
        assertEquals(List.of("a", "2"), column(Dialect.CSV, "x\na\n2\n"));
        // short rows.
        assertEquals(List.of("1", "3"), column(Dialect.CSV.withShortRows(ShortRows.PAD), "a,b\n1\n3,4\n"));
        assertEquals(
                TabularFailure.COLUMN_COUNT,
                assertThrows(TabularException.class, () -> column(Dialect.CSV, "a,b\n1\n3,4\n"))
                        .failure());
        // long rows.
        for (LongRows cut : List.of(LongRows.REPORT, LongRows.TRUNCATE)) {
            assertEquals(List.of("1", "3"), column(Dialect.CSV.withLongRows(cut), "a,b\n1,2,x\n3,4\n"));
        }
        assertEquals(
                TabularFailure.COLUMN_COUNT,
                assertThrows(TabularException.class, () -> column(Dialect.CSV, "a,b\n1,2,x\n3,4\n"))
                        .failure());
        // tokens.
        assertEquals(List.of(""), column(Dialect.CSV.withTokens("NA"), "a\nNA\n"));
        assertEquals(List.of("NA"), column(Dialect.CSV, "a\nNA\n"));
        // an encoding.
        assertEquals(
                List.of("é"),
                new ArrayList<>(
                        column(Dialect.CSV.withEncoding(Encoding.LATIN_1), "a\n", new byte[] {(byte) 0xE9, '\n'})));
        // With no encoding declared those bytes aren't UTF-8, so they aren't text.
        assertEquals(List.of(""), column(Dialect.CSV, "a\n", new byte[] {(byte) 0xE9, '\n'}));
    }

    private static List<String> column(Dialect dialect, String text, byte[]... more) {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        bytes.writeBytes(utf8(text));
        for (byte[] part : more) {
            bytes.writeBytes(part);
        }
        List<String> rows = new ArrayList<>();
        try (DelimitedReader reader = DelimitedReader.of(bytes.toByteArray(), dialect, List.of(Column.text(0)))) {
            reader.forEachRow(row -> {
                String cell = row.string(0);
                rows.add(cell != null ? cell : "");
            });
        }
        return rows;
    }

    /** Two lists are the same list when they hold the same tokens in order, and a dialect and a column stay comparable. */
    @Test
    void tokensAreComparable() {
        assertEquals(Tokens.of("NA", "-"), Tokens.of("NA", "-"));
        assertNotEquals(Tokens.of("NA", "-"), Tokens.of("-", "NA"));
        assertEquals(Tokens.of(), Tokens.NONE);
        assertEquals(Tokens.of("NA").hashCode(), Tokens.of("NA").hashCode());
        // A token's length is its own: these two would be one string if they were joined.
        assertNotEquals(Tokens.of("ab", "c"), Tokens.of("a", "bc"));
        // A column that opts out of its dialect's tokens is not one that inherits them.
        assertNotEquals(Column.text(0).withTokens(), Column.text(0));
        assertEquals(Column.text(0).withTokens("NA"), Column.text(0).withTokens("NA"));
        assertEquals(
                Column.text(0).withTokens("NA").hashCode(),
                Column.text(0).withTokens("NA").hashCode());
        assertNotEquals(Dialect.CSV.withTokens("NA"), Dialect.CSV);
        assertEquals(Dialect.CSV.withTokens("NA"), Dialect.CSV.withTokens("NA"));
        assertEquals("Tokens[\"NA\", \"-\"]", Tokens.of("NA", "-").toString());
    }

    /** A column's list wins over the dialect's, an empty one takes it off, and neither reaches a column that says nothing. */
    @Test
    void aColumnsTokensReplaceItsDialects() {
        try (DelimitedReader reader = DelimitedReader.of(
                utf8("a,b,c\nNA,NA,NA\nx,x,x\n"),
                Dialect.CSV.withTokens("NA"),
                List.of(
                        Column.text(0),
                        Column.text(1).withTokens(),
                        Column.text(2).withTokens("x")))) {
            Batch batch = reader.read();
            assertEquals(Origin.TOKEN, batch.origin(0, 0));
            assertEquals(Origin.CELL, batch.origin(1, 0));
            assertEquals("NA", batch.string(1, 0));
            assertEquals(Origin.CELL, batch.origin(2, 0));
            assertEquals(Origin.TOKEN, batch.origin(2, 1));
            assertEquals(CastFailure.EMPTY, batch.verdict(2, 1).reason());
        }
    }

    /** A sheet's tokens reach its text cells, and a column's replace them. */
    @Test
    void aSheetsTokensReachItsTextCells() throws IOException {
        try (Workbook book = strings("a", "NA", "x");
                Sheet sheet = book.sheet(
                        0,
                        SheetOptions.DEFAULT.withTokens("NA"),
                        List.of(Column.text(0), Column.text(0).withTokens()))) {
            Batch batch = sheet.read();
            assertEquals(Origin.TOKEN, batch.origin(0, 0));
            assertEquals(Origin.CELL, batch.origin(1, 0));
            assertEquals("NA", batch.string(1, 0));
            assertEquals("x", batch.string(0, 1));
        }
        try (Workbook book = strings("a", "NA");
                Sheet sheet = book.sheet(0, SheetOptions.DEFAULT, List.of(Column.text(0)))) {
            assertEquals("NA", sheet.read().string(0, 0));
        }
    }

    /** A column's date options and month names are what its door reads by; without them, it refuses. */
    @Test
    void aColumnsDateOptionsAreItsOwn() {
        try (DelimitedReader reader = DelimitedReader.of(
                utf8("a,b\n20260131,20260131\n"),
                Dialect.CSV,
                List.of(Column.date(0).withDateOptions(new DateOptions(DateOptions.COMPACT_DATE)), Column.date(1)))) {
            Batch batch = reader.read();
            assertTrue(batch.isOk(0, 0));
            assertEquals(
                    java.time.LocalDate.of(2026, 1, 31),
                    ((io.github.skunkwerkx.hypercast.Success<java.time.LocalDate>)
                                    batch.get(0, 0, java.time.LocalDate.class))
                            .value());
            assertFalse(batch.isOk(1, 0));
        }
        io.github.skunkwerkx.hypercast.MonthNames.Builder names = io.github.skunkwerkx.hypercast.MonthNames.builder();
        String[] months = {"janv", "fevr", "mars", "avr", "mai", "juin", "juil", "aout", "sept", "oct", "nov", "dec"};
        for (int month = 1; month <= 12; month++) {
            names.add(month, months[month - 1]);
        }
        var french = names.build();
        Column named = Column.date(0, DateOrder.DAY_MONTH_YEAR).withDateOptions(DateOptions.NONE, french);
        assertEquals(french, named.months());
        assertEquals(DateOptions.NONE, named.dateOptions());
        assertNotEquals(named, Column.date(0, DateOrder.DAY_MONTH_YEAR));
        try (DelimitedReader reader = DelimitedReader.of(
                utf8("a,b\n2-fevr-2026,2-fevr-2026\n"),
                Dialect.CSV,
                List.of(named, Column.date(1, DateOrder.DAY_MONTH_YEAR)))) {
            Batch batch = reader.read();
            assertTrue(batch.isOk(0, 0), batch.verdict(0, 0).toString());
            assertEquals(
                    java.time.LocalDate.of(2026, 2, 2),
                    ((io.github.skunkwerkx.hypercast.Success<java.time.LocalDate>)
                                    batch.get(0, 0, java.time.LocalDate.class))
                            .value());
            assertFalse(batch.isOk(1, 0));
        }
    }

    private static byte[] utf16(String text, boolean lone) {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        bytes.writeBytes(text.getBytes(StandardCharsets.UTF_16LE));
        if (lone) {
            // A high surrogate, and nothing to pair it with.
            bytes.write(0x00);
            bytes.write(0xD8);
        }
        return bytes.toByteArray();
    }

    private record Read(List<String> rows, Exception ended) {}

    private static Read readAll(DelimitedReader reader) {
        List<String> rows = new ArrayList<>();
        try {
            for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                for (int row = 0; row < batch.rows(); row++) {
                    String cell = batch.string(0, row);
                    rows.add(cell != null ? cell : "");
                }
            }
        } catch (TabularException | UncheckedIOException e) {
            return new Read(rows, e);
        }
        return new Read(rows, null);
    }

    /** A \r the fill is holding when the encoding fails ends its row, and the row comes first. */
    @ParameterizedTest
    @CsvSource({"'a\nb\n', 'a,b', 2, 3, 8", "'a\nb\r', 'a,b', 2, 3, 8", "'a\nb', a, 1, 2, 6", "'a\n\"b\r', a, 1, 2, 10"
    })
    void anEncodingFailureComesAfterEveryIntactRow(String text, String rows, long record, int line, long at) {
        Dialect dialect = HEADERLESS.withEncoding(Encoding.UTF_16LE);
        byte[] input = utf16(text, true);
        for (int buffer : new int[] {1, 7, DelimitedReader.DEFAULT_BUFFER_BYTES}) {
            try (DelimitedReader reader = DelimitedReader.of(
                    new ByteArrayInputStream(input),
                    dialect,
                    List.of(Column.text(0)),
                    DelimitedReader.DEFAULT_BATCH_ROWS,
                    buffer)) {
                holdsAnEncodingFailure(reader, rows, record, line, at);
            }
        }
        try (DelimitedReader memory = DelimitedReader.of(input, dialect, List.of(Column.text(0)))) {
            holdsAnEncodingFailure(memory, rows, record, line, at);
        }
    }

    private static void holdsAnEncodingFailure(DelimitedReader reader, String rows, long record, int line, long at) {
        Read read = readAll(reader);
        assertEquals(rows, String.join(",", read.rows()));
        TabularException failure = assertInstanceOf(TabularException.class, read.ended());
        assertEquals(TabularFailure.ENCODING, failure.failure());
        assertEquals(27, failure.failure().code());
        assertEquals(record, failure.record());
        assertEquals(line, failure.line());
        assertEquals(at, failure.byteOffset());
        assertEquals(
                "Record " + record + " (line " + line + ") holds text with no character in its encoding, at byte " + at
                        + ".",
                failure.getMessage());
        // Final, as any structure failure is: the same one, again.
        assertSame(failure, assertThrows(TabularException.class, reader::read));
    }

    /** Hands over what it has, fails the next read, once, and from then on says it has ended. */
    private static final class FailingStream extends InputStream {
        private final ByteArrayInputStream bytes;
        private IOException failure;

        FailingStream(byte[] bytes, IOException failure) {
            this.bytes = new ByteArrayInputStream(bytes);
            this.failure = failure;
        }

        private int ended() throws IOException {
            if (failure == null) {
                return -1;
            }
            IOException once = failure;
            failure = null;
            throw once;
        }

        @Override
        public int read() throws IOException {
            int next = bytes.read();
            return next < 0 ? ended() : next;
        }

        @Override
        public int read(byte[] into, int offset, int length) throws IOException {
            int read = bytes.read(into, offset, length);
            return read <= 0 ? ended() : read;
        }
    }

    /** Bytes a source gave and then a failure: the intact rows come out, and the source's failure is what the read ends on. */
    @ParameterizedTest
    @EnumSource(
            value = Encoding.class,
            names = {"UTF_8", "LATIN_1"})
    void aDecodedSourceFailsAsAnyOtherDoes(Encoding encoding) {
        IOException broken = new IOException("the wire is cut");
        // One byte at a time, a failure comes behind bytes the same call took.
        for (int buffer : new int[] {1, DelimitedReader.DEFAULT_BUFFER_BYTES}) {
            try (DelimitedReader reader = DelimitedReader.of(
                    new FailingStream(utf8("a\n1\n2\n"), broken),
                    Dialect.CSV.withEncoding(encoding),
                    List.of(Column.text(0)),
                    DelimitedReader.DEFAULT_BATCH_ROWS,
                    buffer)) {
                Read read = readAll(reader);
                assertEquals(List.of("1", "2"), read.rows());
                assertSame(
                        broken,
                        assertInstanceOf(UncheckedIOException.class, read.ended())
                                .getCause());
            }
        }
    }

    /** Text in memory under an encoding goes through a buffer of its own, and the ceiling holds there too. */
    @ParameterizedTest
    @EnumSource(
            value = Encoding.class,
            names = {"UTF_8", "LATIN_1"})
    void aRecordPastTheCeilingIsAFailure(Encoding encoding) {
        // Longer than the buffer a reader of memory starts with, as well as a stream's one byte.
        byte[] input = utf8("x".repeat(DelimitedReader.DEFAULT_BUFFER_BYTES + 64) + "\n");
        Dialect dialect = HEADERLESS.withEncoding(encoding);
        try (DelimitedReader stream = DelimitedReader.of(
                new ByteArrayInputStream(input),
                dialect,
                List.of(Column.text(0)),
                DelimitedReader.DEFAULT_BATCH_ROWS,
                1)) {
            stream.rowCeiling = 8;
            assertEquals(
                    TabularFailure.ROW_TOO_LONG,
                    assertThrows(TabularException.class, stream::read).failure());
        }
        if (encoding != Encoding.UTF_8) {
            try (DelimitedReader memory = DelimitedReader.of(input, dialect, List.of(Column.text(0)))) {
                memory.rowCeiling = 8;
                assertEquals(
                        TabularFailure.ROW_TOO_LONG,
                        assertThrows(TabularException.class, memory::read).failure());
            }
        }
    }

    /** Hands over its chunks, each no more than asked for, then blocks as a quiet socket does until interrupted. */
    private static final class Stalling extends InputStream {
        private final byte[][] chunks;
        private int chunk;
        private int at;

        Stalling(byte[]... chunks) {
            this.chunks = chunks;
        }

        @Override
        public int read() {
            throw new UnsupportedOperationException();
        }

        @Override
        public int read(byte[] into, int offset, int length) throws IOException {
            if (chunk == chunks.length) {
                try {
                    new CountDownLatch(1).await();
                } catch (InterruptedException e) {
                    throw new InterruptedIOException();
                }
            }
            int given = Math.min(length, chunks[chunk].length - at);
            System.arraycopy(chunks[chunk], at, into, offset, given);
            at += given;
            if (at == chunks[chunk].length) {
                chunk++;
                at = 0;
            }
            return given;
        }
    }

    /** {@code bytes} of eight-byte records, the first of them the header, as one chunk. */
    private static byte[] records(int bytes) {
        byte[] records = new byte[bytes];
        java.util.Arrays.fill(records, (byte) '1');
        for (int end = 7; end < bytes; end += 8) {
            records[end] = '\n';
        }
        return records;
    }

    private static void everyRowArrives(int bufferBytes, Encoding encoding, int expected, byte[]... chunks) {
        assertTimeoutPreemptively(Duration.ofSeconds(30), () -> {
            try (DelimitedReader reader = DelimitedReader.of(
                    new Stalling(chunks),
                    Dialect.CSV.withEncoding(encoding),
                    List.of(Column.text(0)),
                    DelimitedReader.DEFAULT_BATCH_ROWS,
                    bufferBytes)) {
                int rows = 0;
                // Never past the last of them: that read has nothing to give, and is right to block.
                while (rows < expected) {
                    rows += reader.read().rows();
                }
                assertEquals(expected, rows);
            }
        });
    }

    /**
     * A stream that gives what it has and goes quiet still gets its finished rows out, whether what
     * it gave fills the read or not, however much room is left behind it. A hung read fails here.
     */
    @ParameterizedTest
    @EnumSource(
            value = Encoding.class,
            names = {"UTF_8", "LATIN_1"})
    void finishedRowsDoNotWaitForAQuietStream(Encoding encoding) {
        int transfer = 1 << 20;
        // Exactly one transfer, with room for four: the header comes from the factory, the rows from read.
        everyRowArrives(4 << 20, encoding, transfer / 8 - 1, records(transfer));
        // Less than one.
        everyRowArrives(4 << 20, encoding, 100 / 8 - 1, records(100 / 8 * 8));
        // A long record grows the buffer, and then exactly what a transfer holds.
        byte[] longRecord = new byte[100 * 1024];
        java.util.Arrays.fill(longRecord, (byte) 'x');
        longRecord[7] = '\n';
        longRecord[longRecord.length - 1] = '\n';
        everyRowArrives(64 * 1024, encoding, 1 + 64 * 1024 / 8, longRecord, records(64 * 1024));
    }

    /** A stream that fills every read is asked for more each time, up to the cap: few reads, and few looks from the start. */
    @Test
    void aLongRecordFromAStreamTakesFewReads() {
        long[] asked = new long[2];
        try (DelimitedReader reader = DelimitedReader.of(
                new OneLongRecord(200L << 20) {
                    @Override
                    public int read(byte[] into, int offset, int length) {
                        asked[0]++;
                        asked[1] = Math.max(asked[1], length);
                        return super.read(into, offset, length);
                    }
                },
                HEADERLESS,
                List.of(Column.text(0)))) {
            assertEquals(1, reader.read().rows());
        }
        assertTrue(asked[0] < 30, asked[0] + " reads");
        assertEquals(64 << 20, asked[1]);
    }

    /** A read that isn't filled says the stream has no more to hand: nothing grows, and the record still reads. */
    @Test
    void aStreamGivingShortReadsIsAskedForNoMore() {
        long[] asked = new long[1];
        try (DelimitedReader reader = DelimitedReader.of(
                new OneLongRecord(8L << 20) {
                    @Override
                    public int read(byte[] into, int offset, int length) {
                        asked[0] = Math.max(asked[0], length);
                        return super.read(into, offset, Math.min(length, 64 * 1024));
                    }
                },
                HEADERLESS,
                List.of(Column.text(0)),
                DelimitedReader.DEFAULT_BATCH_ROWS,
                1 << 20)) {
            assertEquals(1, reader.read().rows());
        }
        assertEquals(1 << 20, asked[0]);
    }

    /** One record of {@code x} and its line ending, as long as it's told, from no memory of its own. */
    private static class OneLongRecord extends InputStream {
        private long left;

        OneLongRecord(long bytes) {
            this.left = bytes;
        }

        @Override
        public int read() {
            throw new UnsupportedOperationException();
        }

        @Override
        public int read(byte[] into, int offset, int length) {
            if (left < 0) {
                return -1;
            }
            if (left == 0) {
                left = -1;
                into[offset] = '\n';
                return 1;
            }
            int written = (int) Math.min(length, left);
            java.util.Arrays.fill(into, offset, offset + written, (byte) 'x');
            left -= written;
            return written;
        }
    }

    /**
     * The ceiling itself, the one Rust's MAX_ROW_BYTES says, either side of it: half a second, and
     * 2.2 GiB resident while it runs on either path (measured on an Apple M6), so a host with less than
     * 6 GiB skips it out loud.
     */
    @Test
    void aRecordIsReadUpToTheRealCeilingAndNoFurther() {
        long available = ((OperatingSystemMXBean) ManagementFactory.getOperatingSystemMXBean()).getTotalMemorySize();
        Assumptions.assumeTrue(
                available >= 6L << 30,
                () -> "The real ceiling needs 6 GiB of memory; this host has " + available + " bytes.");
        assertEquals(1 << 30, DelimitedReader.MAX_ROW_BYTES);
        try (DelimitedReader under = DelimitedReader.of(
                new OneLongRecord(DelimitedReader.MAX_ROW_BYTES - 8L),
                HEADERLESS,
                List.of(Column.text(0)),
                DelimitedReader.DEFAULT_BATCH_ROWS,
                DelimitedReader.DEFAULT_BUFFER_BYTES)) {
            assertEquals(1, under.read().rows());
        }
        try (DelimitedReader past = DelimitedReader.of(
                new OneLongRecord(DelimitedReader.MAX_ROW_BYTES + 8L),
                HEADERLESS,
                List.of(Column.text(0)),
                DelimitedReader.DEFAULT_BATCH_ROWS,
                DelimitedReader.DEFAULT_BUFFER_BYTES)) {
            assertEquals(
                    TabularFailure.ROW_TOO_LONG,
                    assertThrows(TabularException.class, past::read).failure());
        }
    }

    @Test
    void aRangeIsWhatTheCoreSaysItIs() {
        assertEquals(new Range(4, 2, 12, 5), Range.parse("B4:E12"));
        for (String text : List.of("", "A5", "a5:f200", "F200:A5", "A0:B2", "A1:B", " A1:B2")) {
            assertTrue(
                    assertThrows(IllegalArgumentException.class, () -> Range.parse(text))
                            .getMessage()
                            .contains("\"" + text + "\""),
                    text);
            assertTrue(Range.tryParse(text).isEmpty(), text);
        }
        assertEquals(5, new Range(5, 0, 0, 0).firstRow());
        assertThrows(IllegalArgumentException.class, () -> new Range(5, 0, 4, 0));
        assertThrows(IllegalArgumentException.class, () -> new Range(0, 3, 0, 2));
        assertThrows(IllegalArgumentException.class, () -> new Range(-1, 0, 0, 0));
        assertEquals(new Range(0, 0, 0, 0), Range.ALL);
    }

    @Test
    void aSheetTakesItsOptions() {
        try (Workbook book = Workbook.open(corpus("ranges.xlsx"))) {
            SheetOptions options = SheetOptions.DEFAULT
                    .withRange(Range.parse("B4:E12"))
                    .withBatchRows(2)
                    .withNotes(false);
            try (Sheet sheet = book.sheet(0, options, List.of(Column.text(0)))) {
                assertEquals(options, sheet.options());
                assertEquals(2, sheet.read().rows());
            }
            assertThrows(
                    IllegalArgumentException.class,
                    () -> book.sheet(0, options.withBatchRows(0), List.of(Column.text(0))));
            // A range's last column ends the header and the record, here two columns short of the table's.
            try (Sheet sheet = book.sheet(0, SheetOptions.DEFAULT.withRange(Range.parse("B4:C12")))) {
                assertEquals(List.of("id", "name"), sheet.header());
            }
            // A plan's ordinal counts from the range's first column; a problem's source column is the sheet's.
            try (Sheet sheet =
                    book.sheet(0, SheetOptions.DEFAULT.withRange(Range.parse("B4:E12")), List.of(Column.uuid(1)))) {
                Problem first = sheet.read().faults().iterator().next();
                assertEquals(OptionalInt.of(0), first.planColumn());
                assertEquals(OptionalInt.of(2), first.sourceColumn());
            }
        }
    }

    /** Notes off is what a read asks the core for: no word on a refused cell, where the default has one. */
    @ParameterizedTest
    @ValueSource(booleans = {true, false})
    void notesOffAsksForNoNotes(boolean notes) {
        try (DelimitedReader reader =
                DelimitedReader.of(utf8("a\n1x\n"), Dialect.CSV.withNotes(notes), List.of(Column.i32(0)))) {
            Batch batch = reader.read();
            assertEquals(CastFailure.MALFORMED, batch.verdict(0, 0).reason());
            assertEquals(notes, batch.detail(0, 0) != null);
        }
        assertTrue(Dialect.CSV.notes());
        assertTrue(new Dialect(';', false, false, false).notes());
        assertTrue(SheetOptions.DEFAULT.notes());
        assertTrue(new SheetOptions(false, false, 1).notes());
    }

    /** Fault detail reaches every door the verdict does: the row's, the batch's and the list's. */
    @Test
    void aFaultCarriesItsDetailEverywhereItIsGiven() {
        try (DelimitedReader reader =
                DelimitedReader.of(utf8("a\n\"1,23,456\"\n"), Dialect.CSV, List.of(Column.i32(0)))) {
            Batch batch = reader.read();
            assertEquals(FaultDetail.GROUP_SIZE, batch.detail(0, 0));
            assertEquals(FaultDetail.GROUP_SIZE, batch.row(0).detail(0));
            List<Problem> listed = new ArrayList<>();
            batch.faults().forEach(listed::add);
            assertEquals(1, listed.size());
            assertEquals(FaultDetail.GROUP_SIZE, listed.get(0).detail());
        }
        // A cell that cast has none.
        try (DelimitedReader reader = DelimitedReader.of(utf8("a\n1\n"), Dialect.CSV, List.of(Column.i32(0)))) {
            assertNull(reader.read().detail(0, 0));
        }
    }

    /** A batch's list can be longer than a call gives back, and asked again it starts again. */
    @Test
    void aBatchsFaultsAreAsLongAsTheyAre() {
        int rows = 3 * 64 + 1;
        String input = "a,b\n" + "x,q\n".repeat(rows);
        try (DelimitedReader reader =
                DelimitedReader.of(utf8(input), Dialect.CSV, List.of(Column.i32(0), Column.text(1), Column.bool(1)))) {
            Batch batch = reader.read();
            int listed = 0;
            for (Problem problem : batch.faults()) {
                int column = listed % 2 == 1 ? 2 : 0;
                assertEquals(listed / 2, problem.row());
                assertEquals(OptionalInt.of(column), problem.planColumn());
                assertEquals(OptionalInt.of(column / 2), problem.sourceColumn());
                assertEquals(CastFailure.MALFORMED, problem.reason());
                assertEquals(listed / 2 + 2, problem.line());
                assertNotNull(problem.detail());
                listed++;
            }
            assertEquals(2 * rows, listed);
            assertEquals(0, batch.faults().iterator().next().row());
            int again = 0;
            for (Problem ignored : batch.faults()) {
                again++;
            }
            assertEquals(2 * rows, again);
        }
    }

    /**
     * The list is the batch's it was asked of: asked and then walked after the reader moved on, it
     * says so, whether the reader found another batch or the end, and whether the walk had begun.
     */
    @Test
    void faultsAskedOfABatchStayThatBatchs() {
        byte[] input = utf8("a\nx\ny\n");
        // Start over after the next batch.
        try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV, List.of(Column.i32(0)), 1)) {
            Iterable<Problem> list = reader.read().faults();
            assertNotNull(reader.read());
            assertThrows(
                    ConcurrentModificationException.class, () -> list.iterator().hasNext());
        }
        // Started after the end.
        try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV, List.of(Column.i32(0)), 1)) {
            Iterable<Problem> list = reader.read().faults();
            while (reader.read() != null) {}
            Iterator<Problem> walk = list.iterator();
            assertThrows(ConcurrentModificationException.class, walk::hasNext);
            assertThrows(ConcurrentModificationException.class, walk::next);
        }
        // Going on with a walk that had begun.
        try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV, List.of(Column.i32(0)))) {
            Iterator<Problem> walk = reader.read().faults().iterator();
            assertTrue(walk.hasNext());
            walk.next();
            assertNull(reader.read());
            assertThrows(ConcurrentModificationException.class, walk::hasNext);
        }
        // After close, the memory is gone and the walk says so rather than touching it.
        Iterator<Problem> closed;
        try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV, List.of(Column.i32(0)))) {
            closed = reader.read().faults().iterator();
            closed.next();
        }
        assertThrows(ConcurrentModificationException.class, closed::hasNext);
        // The list and a walk asked of the current batch answer it.
        try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV, List.of(Column.i32(0)), 1)) {
            reader.read();
            Batch second = reader.read();
            assertEquals(3, second.faults().iterator().next().line());
        }
        // A sheet's, too.
        try (Workbook book = Workbook.open(corpus("basic.xlsx"));
                Sheet sheet = book.sheet(0, SheetOptions.DEFAULT.withBatchRows(1), List.of(Column.uuid(0)))) {
            Iterable<Problem> list = sheet.read().faults();
            assertNotNull(sheet.read());
            assertThrows(
                    ConcurrentModificationException.class, () -> list.iterator().next());
        }
    }

    /**
     * A walk of a batch's rows is that batch's: the batch object goes on to the next batch, and the
     * walk says so rather than following it, whether the reader found another batch or the end.
     */
    @Test
    void aWalkOfRowsStaysItsBatchs() {
        byte[] input = utf8("a\n1\n2\n3\n4\n5\n6\n");
        try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV, List.of(Column.text(0)), 2)) {
            Batch batch = reader.read();
            Iterator<Row> walk = batch.iterator();
            assertEquals("1", walk.next().string(0));
            assertSame(batch, reader.read());
            assertThrows(ConcurrentModificationException.class, walk::hasNext);
            assertThrows(ConcurrentModificationException.class, walk::next);
            // The batch asked again walks what it now holds.
            assertEquals("3", batch.iterator().next().string(0));
            Iterator<Row> last = batch.iterator();
            assertNull(skipTo(reader));
            assertThrows(ConcurrentModificationException.class, last::next);
        }
        Iterator<Row> closed;
        try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV, List.of(Column.text(0)), 2)) {
            closed = reader.read().iterator();
            closed.next();
        }
        assertThrows(ConcurrentModificationException.class, closed::next);
        // A read in the middle of a loop, by way of forEachRow, ends the loop the same way.
        try (DelimitedReader reader = DelimitedReader.of(input, Dialect.CSV, List.of(Column.text(0)), 2)) {
            assertThrows(ConcurrentModificationException.class, () -> reader.forEachRow(row -> reader.read()));
        }
        try (Workbook book = Workbook.open(corpus("basic.xlsx"));
                Sheet sheet = book.sheet(0, SheetOptions.DEFAULT.withBatchRows(1), List.of(Column.text(0)))) {
            Iterator<Row> walk = sheet.read().iterator();
            assertNotNull(sheet.read());
            assertThrows(ConcurrentModificationException.class, walk::next);
        }
    }

    private static Batch skipTo(DelimitedReader end) {
        Batch batch = end.read();
        while (batch != null) {
            batch = end.read();
        }
        return batch;
    }

    @Test
    void aSuggestionsColumnIsOneThereCouldBe() {
        assertThrows(IllegalArgumentException.class, () -> SuggestOptions.DEFAULT.withColumnFormat(-1, COMMA_DECIMAL));
        assertThrows(
                IllegalArgumentException.class,
                () -> SuggestOptions.DEFAULT.withColumnDateOrder(-1, DateOrder.DAY_MONTH_YEAR));
        assertEquals(
                COMMA_DECIMAL,
                SuggestOptions.DEFAULT.withColumnFormat(0, COMMA_DECIMAL).declaredFormat(0));
        assertEquals(
                DateOrder.DAY_MONTH_YEAR,
                SuggestOptions.DEFAULT
                        .withColumnDateOrder(0, DateOrder.DAY_MONTH_YEAR)
                        .declaredOrder(0));
    }

    /** The slot a refused cell leaves is not what the batch before it wrote there. */
    @Test
    void aTextCellThatFailedHasNoCharsWhateverCameBefore() {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        bytes.writeBytes(utf8("ok\n"));
        bytes.writeBytes(new byte[] {(byte) 0xFF, '\n', '\n', 'z', '\n'});
        try (DelimitedReader reader = DelimitedReader.of(
                bytes.toByteArray(), HEADERLESS.withSkipBlankLines(false), List.of(Column.text(0)), 1)) {
            assertEquals("ok", reader.read().string(0, 0));
            char[] into = {'-', '-', '-'};
            for (CastFailure reason : List.of(CastFailure.MALFORMED, CastFailure.EMPTY)) {
                Batch failed = reader.read();
                assertEquals(reason, failed.verdict(0, 0).reason());
                assertEquals(0, failed.getChars(0, 0, into, 0));
                assertEquals("---", new String(into));
                assertNull(failed.chars(0, 0));
                assertNull(failed.string(0, 0));
                assertNull(failed.text(0, 0));
            }
            assertEquals(1, reader.read().getChars(0, 0, into, 0));
            assertEquals('z', into[0]);
        }
    }

    /**
     * One string past what a window holds, deflated: the window is grown mid-part, with the inflated
     * bytes and inflate's history in it, and they have to come along.
     */
    @Test
    void aStringLongerThanTheWindowIsReadWhole() throws IOException {
        char[] letters = new char[(int) Abi.WINDOW_MIN * 8];
        java.util.Random random = new java.util.Random(7);
        for (int at = 0; at < letters.length; at++) {
            letters[at] = (char) ('a' + random.nextInt(26));
        }
        String cell = new String(letters);
        long before = Workbook.grown[0];
        try (Workbook book = strings("a", cell, "z");
                Sheet sheet = book.sheet(0, SheetOptions.DEFAULT, List.of(Column.text(0)))) {
            Batch batch = sheet.read();
            assertEquals(2, batch.rows());
            assertEquals(cell, batch.string(0, 0));
            assertEquals("z", batch.string(0, 1));
        }
        assertTrue(Workbook.grown[0] > before, "the window never grew");
    }

    /** A long row is on the list with the width it had, under REPORT alone. */
    @Test
    void aLongRowIsListedUnderReport() {
        byte[] input = utf8("a,b\n1,2,3\n4,5\n");
        try (DelimitedReader reader =
                DelimitedReader.of(input, Dialect.CSV.withLongRows(LongRows.REPORT), List.of(Column.text(0)))) {
            Problem problem = reader.read().faults().iterator().next();
            assertEquals(
                    new Problem(0, 2, OptionalInt.empty(), OptionalInt.empty(), null, null, Origin.CELL, 0, 0, 2, 3),
                    problem);
        }
        try (DelimitedReader reader =
                DelimitedReader.of(input, Dialect.CSV.withLongRows(LongRows.TRUNCATE), List.of(Column.text(0)))) {
            assertFalse(reader.read().faults().iterator().hasNext());
        }
    }

    /**
     * Every door code there could be, 0 and past the last included, with a param beside it: a report
     * and its plan say a door that exists and a param it declares, and only a resolved one is read
     * through.
     */
    @Test
    void everyDoorTheCoreCanNameIsAColumn() {
        for (int code = 0; code <= Door.EXCEL_SERIAL.code() + 1; code++) {
            Door door = Door.ofCode(code);
            for (int param : new int[] {0, 1, 2, 3, 99}) {
                SuggestedColumn report = Sample.reportOf(0, code, param, 1, 1, 1, null, null);
                if (door == null) {
                    assertEquals(Door.TEXT, report.door(), "door " + code);
                    continue;
                }
                Column planned = Sample.planOf(report);
                try (DelimitedReader read = DelimitedReader.of(utf8("1\n"), HEADERLESS, List.of(planned))) {
                    assertEquals(planned, read.read().columns().get(0), "door " + door + ", param " + param);
                }
                boolean takes = door == Door.UNIX
                        || door == Door.EXCEL_SERIAL
                        || door == Door.DATE_ORDERED
                        || door == Door.DATETIME;
                boolean valid = switch (door) {
                    case UNIX -> UnixPrecision.fromCode(param).isPresent();
                    case EXCEL_SERIAL -> ExcelEpoch.fromCode(param).isPresent();
                    case DATE_ORDERED, DATETIME -> DateOrder.fromCode(param).isPresent();
                    default -> true;
                };
                assertEquals(valid ? door : Door.TEXT, report.door(), "door " + door + ", param " + param);
                assertEquals(takes && valid ? param : 0, planned.declared(), "door " + door + ", param " + param);
            }
            // Ambiguous and unresolved read as text, whatever the door.
            for (int status : new int[] {2, 3, 0, 9}) {
                Column planned = Sample.planOf(Sample.reportOf(0, code, 1 << 2, status, 0, 0, null, null));
                assertEquals(Door.TEXT, planned.door(), "door " + code + ", status " + status);
                assertEquals(0, planned.ordinal());
            }
        }
    }

    private static SuggestedColumn reported(Door door, int param, int status, NumFormat declared) {
        return Sample.reportOf(0, door.code(), param, status, 0, 0, null, declared);
    }

    /** The notations the suggest export names by number, as this binding declares them: each Western. */
    @Test
    void aProvedNotationIsWestern() {
        assertEquals(95, NumFormat.INVARIANT.flags());
        assertEquals(95, COMMA_DECIMAL.flags());
        int commaGroups = 1 << 1;
        int commaDecimal = 1 << 2;
        // Proved by a , or not, the plan is the same: a later 1,23,456 is refused.
        for (int param : new int[] {0, commaGroups}) {
            SuggestedColumn report = reported(Door.I32, param, 1, null);
            assertEquals(NumFormat.INVARIANT, report.format());
            assertEquals(0, report.param());
            try (DelimitedReader reader = DelimitedReader.of(
                    utf8("a\n\"1,234\"\n\"1,23,456\"\n"), Dialect.CSV, List.of(Sample.planOf(report)))) {
                Batch batch = reader.read();
                assertEquals(1234, i32(batch, 0, 0));
                assertEquals(CastFailure.MALFORMED, batch.verdict(0, 1).reason());
            }
        }
        // What only the comma decimal reads is planned in it, and a later group it can't read is refused.
        SuggestedColumn decimals = reported(Door.F64, commaDecimal, 1, null);
        assertEquals(COMMA_DECIMAL, decimals.format());
        assertEquals(0, decimals.param());
        assertEquals(List.of(), decimals.candidates());
        try (DelimitedReader reader = DelimitedReader.of(
                utf8("a\n\"0,125\"\n\"1.234,5\"\n\"1,234,567\"\n"), Dialect.CSV, List.of(Sample.planOf(decimals)))) {
            Batch batch = reader.read();
            assertEquals(0.125, f64(batch, 0, 0));
            assertEquals(1234.5, f64(batch, 0, 1));
            assertEquals(CastFailure.MALFORMED, batch.verdict(0, 2).reason());
        }
        // A date's param of 2 is its order, no notation, and it names no format.
        assertNull(reported(Door.DATE_ORDERED, 2, 1, null).format());
        // A column's declaration wins, and the report names it, whatever param says.
        for (int param : new int[] {commaGroups, commaDecimal}) {
            assertEquals(
                    NumFormat.INVARIANT,
                    reported(Door.I32, param, 1, NumFormat.INVARIANT).format());
        }
        SuggestedColumn declared = reported(Door.I32, commaGroups, 1, COMMA_DECIMAL);
        assertEquals(COMMA_DECIMAL, declared.format());
        assertEquals(COMMA_DECIMAL, Sample.planOf(declared).format());
        // A column that isn't a resolved number names no format.
        assertNull(reported(Door.TEXT, 0, 1, null).format());
        assertNull(reported(Door.I32, commaGroups | commaDecimal, 2, null).format());
        assertNull(reported(Door.I32, 0, 3, null).format());
        // Ambiguous numbers offer both notations, and ambiguous dates, either date door, the orders.
        assertEquals(
                List.of(new Candidate.Notation(NumFormat.INVARIANT), new Candidate.Notation(COMMA_DECIMAL)),
                reported(Door.I32, commaGroups | commaDecimal, 2, null).candidates());
        for (Door door : List.of(Door.DATE_ORDERED, Door.DATETIME)) {
            SuggestedColumn open = reported(door, 1 << 2 | 1 << 3, 2, null);
            assertEquals(door, open.door());
            assertEquals(2, open.param());
            assertEquals(
                    List.of(
                            new Candidate.Order(DateOrder.MONTH_DAY_YEAR),
                            new Candidate.Order(DateOrder.DAY_MONTH_YEAR)),
                    open.candidates());
        }
    }

    /** An arena that fails rather than hands out more than a plan's own small tables take. */
    private static final class GuardArena implements Arena {
        private final Arena inner = Arena.ofConfined();

        @Override
        public MemorySegment allocate(long byteSize, long byteAlignment) {
            if (byteSize > 1 << 20) {
                throw new AssertionError("asked for " + byteSize + " bytes before the core judged them");
            }
            return inner.allocate(byteSize, byteAlignment);
        }

        @Override
        public MemorySegment.Scope scope() {
            return inner.scope();
        }

        @Override
        public void close() {
            inner.close();
        }
    }

    /**
     * The core decides how much a batch may ask for, and every way in asks it before allocating. An arena writes what
     * it allocates, so the first statement holds the one check {@code Columns} has to a guard arena: if that check went,
     * this stops here, having written nothing, before the public ways in reach for the memory.
     */
    @Test
    void theCoreJudgesABatchsSize() {
        byte[] input = utf8("a\n1\n");
        List<Column> plan = List.of(Column.i32(0));
        String limit = "past the limit of 4294967296";
        try (GuardArena guard = new GuardArena()) {
            assertEquals(
                    "batchRows asks for 5377130496 bytes, " + limit,
                    assertThrows(
                                    IllegalArgumentException.class,
                                    () -> new Columns(Columns.checked(plan), 1 << 28, guard, true, false, false))
                            .getMessage());
        }
        assertEquals(
                "batchRows asks for 5377130496 bytes, " + limit,
                assertThrows(
                                IllegalArgumentException.class,
                                () -> DelimitedReader.of(input, Dialect.CSV, plan, 1 << 28))
                        .getMessage());
        assertTrue(assertThrows(
                        IllegalArgumentException.class,
                        () -> DelimitedReader.of(new ByteArrayInputStream(input), Dialect.CSV, plan, 1 << 28, 1024))
                .getMessage()
                .contains(limit));

        try (DelimitedReader late = DelimitedReader.of(input, Dialect.CSV)) {
            assertTrue(assertThrows(IllegalArgumentException.class, () -> late.bind(plan, 1 << 28))
                    .getMessage()
                    .contains(limit));
            assertEquals(
                    "batchRows must be at least 1",
                    assertThrows(IllegalArgumentException.class, () -> late.bind(plan, 0))
                            .getMessage());
            assertFalse(late.isBound());
            late.bind(plan);
            assertEquals(1, late.read().rows());
        }

        try (Workbook book = Workbook.open(corpus("basic.xlsx"))) {
            SheetOptions large = SheetOptions.DEFAULT.withBatchRows(1 << 27);
            assertEquals(
                    "batchRows asks for 4831838224 bytes, " + limit,
                    assertThrows(IllegalArgumentException.class, () -> book.sheet(0, large, plan))
                            .getMessage());
            try (Sheet unplanned = book.sheet(0, large)) {
                assertTrue(assertThrows(IllegalArgumentException.class, () -> unplanned.bind(plan))
                        .getMessage()
                        .contains(limit));
                assertFalse(unplanned.isBound());
            }
        }

        try (DelimitedReader modest = DelimitedReader.of(input, Dialect.CSV, plan, 1 << 16)) {
            assertEquals(1, modest.read().rows());
        }
    }

    /** A sampler's batch is never why a suggestion is refused: nobody set its size. */
    @Test
    void aSuggestionIsNotRefusedForTheSizeOfItsOwnBatch() {
        int width = 268_000;
        StringBuilder names = new StringBuilder("c0");
        StringBuilder row = new StringBuilder("1");
        for (int column = 1; column < width; column++) {
            names.append(",c").append(column);
            row.append(",1");
        }
        Suggestion wide = DelimitedReader.suggest(
                utf8(names + "\n" + row + "\n" + row + "\n"), Dialect.CSV, SuggestOptions.DEFAULT);
        assertEquals(width, wide.report().size());
        assertEquals(2, wide.report().get(width - 1).sampled());
        assertFalse(wide.truncated());

        // At exactly the limit with no plan, and a sample past every batch.
        try (Workbook book = Workbook.open(corpus("basic.xlsx"));
                Sheet atTheLimit = book.sheet(0, SheetOptions.DEFAULT.withBatchRows(1 << 29));
                Sheet modest = book.sheet(0, SheetOptions.DEFAULT)) {
            SuggestOptions many = SuggestOptions.DEFAULT.withSampleRows(Integer.MAX_VALUE);
            assertEquals(
                    words(modest.suggest(SuggestOptions.DEFAULT.withSampleRows(4096))),
                    words(atTheLimit.suggest(many)));
        }
    }

    /** A sample says the same in any number of batches, delimited and sheet. */
    @Test
    void aSampleSaysTheSameInAnyNumberOfBatches() {
        byte[] text = utf8(
                "id,price,when,note\n1,\"1,234.50\",2024-01-02,plain\n2,1.234,2024-02-03,\n3,,2024-03-04,\"a, b\"\n"
                        + "x,0.5,03/04/2024,\"two\nlines\"\n5,12,2024-05-06,more\n6,13,,more\n");
        Dialect unnoted = Dialect.CSV.withNotes(false);
        for (int limit : new int[] {1, 2, 3, 5, 6, 1000}) {
            SuggestOptions options = SuggestOptions.DEFAULT.withSampleRows(limit);
            String whole = words(DelimitedReader.suggest(text, Dialect.CSV, options));
            for (int rows : new int[] {1, 2, 3, 7}) {
                try (DelimitedReader reader = DelimitedReader.of(text, unnoted)) {
                    Sample sample = Sample.of(
                            reader::read,
                            plan -> reader.bind(plan, rows),
                            reader.header().size(),
                            limit,
                            false);
                    assertEquals(whole, words(sample.judge(reader.header(), options)), limit + " in " + rows);
                }
            }
        }
        try (Workbook book = Workbook.open(corpus("basic.xlsx"))) {
            for (int limit : new int[] {1, 2, 3, 1000}) {
                SuggestOptions options = SuggestOptions.DEFAULT.withSampleRows(limit);
                String whole;
                try (Sheet sheet = book.sheet(0, SheetOptions.DEFAULT)) {
                    whole = words(sheet.suggest(options));
                }
                for (int rows : new int[] {1, 2, 3}) {
                    try (Sheet sheet = book.sheet(
                            0, SheetOptions.DEFAULT.withBatchRows(rows).withNotes(true))) {
                        Sample sample = Sample.of(
                                sheet::read, sheet::bind, sheet.header().size(), limit, true);
                        assertEquals(whole, words(sample.judge(sheet.header(), options)), limit + " in " + rows);
                    }
                }
            }
        }
        // A control: the sample's size does move the answer, so the comparison above can fail.
        assertNotEquals(
                words(DelimitedReader.suggest(text, Dialect.CSV, SuggestOptions.DEFAULT.withSampleRows(1))),
                words(DelimitedReader.suggest(text, Dialect.CSV, SuggestOptions.DEFAULT.withSampleRows(6))));
    }

    /** With no header, a short and a long row, and row widths kept, across each batch boundary. */
    @Test
    void aRaggedHeaderlessSampleSaysTheSameInAnyNumberOfBatches() {
        byte[] ragged = utf8("1,x\n2,y,long\n3\n4,z\n5,w\n");
        Dialect headerless =
                Dialect.CSV.withHeader(false).withShortRows(ShortRows.PAD).withLongRows(LongRows.REPORT);
        for (int limit : new int[] {1, 2, 3, 4, 5, 6}) {
            SuggestOptions options = SuggestOptions.DEFAULT.withSampleRows(limit);
            Suggestion whole = DelimitedReader.suggest(ragged, headerless, options);
            assertEquals(Math.min(limit, 5), whole.report().get(0).sampled());
            assertEquals(limit < 5, whole.truncated());
            for (int rows : new int[] {1, 2, 3, 4}) {
                try (DelimitedReader reader = DelimitedReader.of(ragged, headerless.withNotes(false))) {
                    Sample sample = Sample.of(reader::read, plan -> reader.bind(plan, rows), 2, limit, false);
                    assertEquals(words(whole), words(sample.judge(reader.header(), options)), limit + " in " + rows);
                }
            }
        }
    }

    private static String words(Suggestion found) {
        StringBuilder out = new StringBuilder();
        for (SuggestedColumn column : found.report()) {
            out.append(column.ordinal())
                    .append(':')
                    .append(column.door())
                    .append(':')
                    .append(column.status())
                    .append(':')
                    .append(column.matched())
                    .append('/')
                    .append(column.sampled())
                    .append(';');
        }
        return out.append(found.truncated()).toString();
    }

    /** One row past the limit at any batch size names its width: no public plan gets there, so this asks directly. */
    @Test
    void aRowPastTheLimitNamesItsWidth() {
        try (Arena scope = Arena.ofConfined()) {
            MemorySegment spec = scope.allocate(Abi.SPEC_WITH_BYTES, 8);
            spec.set(ValueLayout.JAVA_INT, 0, (int) Abi.SPEC_WITH_BYTES);
            spec.set(ValueLayout.JAVA_INT, Abi.SPEC_WITH_ORDINAL, -1);
            spec.set(
                    ValueLayout.JAVA_INT,
                    Abi.SPEC_WITH_DOOR,
                    Column.i32(0).door().code());
            assertEquals(
                    "A row of 4294967296 columns asks for 68719476768 bytes, past the limit of 4294967296",
                    assertThrows(
                                    IllegalArgumentException.class,
                                    () -> Columns.checkSizes(spec, 1, 1, 1, Abi.BATCH_SHEET))
                            .getMessage());
        }
    }

    /** An explicit zero is refused by the core, for each option, where an omitted option has its default. */
    @Test
    void theCoreRefusesAZeroWhereAnOmittedSizeHasItsDefault() {
        byte[] input = utf8("a\n1\n");
        List<Column> plan = List.of(Column.i32(0));
        String noRows = "batchRows must be at least 1";
        assertEquals(
                noRows,
                assertThrows(IllegalArgumentException.class, () -> DelimitedReader.of(input, Dialect.CSV, plan, 0))
                        .getMessage());
        assertEquals(
                "bufferBytes must be at least 1",
                assertThrows(
                                IllegalArgumentException.class,
                                () -> DelimitedReader.of(new ByteArrayInputStream(input), Dialect.CSV, 0))
                        .getMessage());
        try (Workbook book = Workbook.open(corpus("basic.xlsx"))) {
            SheetOptions none = SheetOptions.DEFAULT.withBatchRows(0);
            assertEquals(
                    noRows,
                    assertThrows(IllegalArgumentException.class, () -> book.sheet(0, none, plan))
                            .getMessage());
            assertEquals(
                    noRows,
                    assertThrows(IllegalArgumentException.class, () -> book.sheet(0, none))
                            .getMessage());
        }
        assertEquals(DelimitedReader.DEFAULT_BATCH_ROWS, SheetOptions.DEFAULT.batchRows());
        byte[] rows = utf8("a\n" + "1\n".repeat(DelimitedReader.DEFAULT_BATCH_ROWS + 1));
        try (DelimitedReader omitted = DelimitedReader.of(rows, Dialect.CSV, plan)) {
            assertEquals(DelimitedReader.DEFAULT_BATCH_ROWS, omitted.read().rows());
        }
        try (DelimitedReader omitted = DelimitedReader.of(new ByteArrayInputStream(input), Dialect.CSV, plan)) {
            assertEquals(1, omitted.read().rows());
        }
        assertEquals(
                1,
                DelimitedReader.suggest(input, Dialect.CSV, SuggestOptions.DEFAULT.withSampleRows(0))
                        .report()
                        .get(0)
                        .sampled());
    }

    @Test
    void suggestTakesItsOptions() {
        byte[] text = utf8("a,b\n\"1,234\",x\n\"5,678\",y\n9,z\n");
        assertThrows(IllegalArgumentException.class, () -> SuggestOptions.DEFAULT.withSampleRows(-1));
        // As Rust's sample_rows: 0 is one row, and the row past it says there's more.
        Suggestion zero =
                DelimitedReader.suggest(utf8("a\n1\n2.5\n"), Dialect.CSV, SuggestOptions.DEFAULT.withSampleRows(0));
        assertEquals(Door.BOOL, zero.report().get(0).door());
        assertEquals(1, zero.report().get(0).sampled());
        assertTrue(zero.truncated());
        Suggestion whole = DelimitedReader.suggest(utf8("a\n1\n2.5\n"), Dialect.CSV);
        assertEquals(Door.F64, whole.report().get(0).door());
        assertEquals(2, whole.report().get(0).sampled());
        assertFalse(whole.truncated());
        Suggestion one = DelimitedReader.suggest(text, Dialect.CSV, SuggestOptions.DEFAULT.withSampleRows(1));
        assertTrue(one.truncated());
        assertEquals(1, one.report().get(0).sampled());
        assertEquals("a", one.report().get(0).name());
        assertEquals("b", one.report().get(1).name());

        // A notation for one column, ahead of another for all of them.
        Suggestion own = DelimitedReader.suggest(
                text,
                Dialect.CSV,
                SuggestOptions.DEFAULT.withFormat(NumFormat.INVARIANT).withColumnFormat(0, COMMA_DECIMAL));
        assertEquals(Door.F64, own.plan().get(0).door());
        assertEquals(COMMA_DECIMAL, own.plan().get(0).format());

        // With no header, the width is the first record's, and no column has a name.
        Suggestion none = DelimitedReader.suggest(utf8("1,x,2\n3,y,4\n"), HEADERLESS);
        assertEquals(3, none.plan().size());
        assertNull(none.report().get(0).name());
        // Nothing to read is nothing to say.
        Suggestion nothing = DelimitedReader.suggest(new byte[0], HEADERLESS);
        assertEquals(0, nothing.plan().size());
        assertEquals(0, nothing.report().size());
        assertFalse(nothing.truncated());
        // A failure among the sampled rows is the suggestion's, and just past them says only that there's more.
        byte[] broken = utf8("a,b\n1,2\n3\n");
        assertThrows(TabularException.class, () -> DelimitedReader.suggest(broken, Dialect.CSV));
        assertTrue(DelimitedReader.suggest(broken, Dialect.CSV, SuggestOptions.DEFAULT.withSampleRows(1))
                .truncated());
        // A file that isn't there is the open's error, not a suggestion's.
        assertThrows(
                UncheckedIOException.class,
                () -> DelimitedReader.suggest(CorpusTest.CORPUS_DIRECTORY.resolve("missing.csv"), Dialect.CSV));
        // The sample's rows are capped by the batch it reads.
        assertEquals(
                DelimitedReader.DEFAULT_BATCH_ROWS,
                DelimitedReader.suggest(
                                utf8("a\n" + "1\n".repeat(DelimitedReader.DEFAULT_BATCH_ROWS + 1)),
                                Dialect.CSV,
                                SuggestOptions.DEFAULT.withSampleRows(Integer.MAX_VALUE - 1))
                        .report()
                        .get(0)
                        .sampled());
    }

    /** {@code y} and {@code 0}/{@code 1} are bools, and a declaration doesn't change that. */
    @Test
    void aColumnOfBooleanSpellingsIsABool() {
        assertEquals(
                Door.BOOL,
                DelimitedReader.suggest(utf8("a\ny\nn\n"), Dialect.CSV)
                        .plan()
                        .get(0)
                        .door());
        assertEquals(
                Door.BOOL,
                DelimitedReader.suggest(utf8("a\n0\n1\n"), Dialect.CSV)
                        .plan()
                        .get(0)
                        .door());
    }

    /** A source too wide to plan is refused before it is read, and one with no columns takes none. */
    @Test
    void aSampleFitsAPlan() {
        assertTrue(assertThrows(
                        IllegalArgumentException.class,
                        () -> Sample.of(
                                () -> {
                                    throw new AssertionError("read");
                                },
                                plan -> {
                                    throw new AssertionError("bound");
                                },
                                Columns.MAX_ORDINAL + 2,
                                1,
                                false))
                .getMessage()
                .contains("2^20"));
        assertNotNull(Sample.of(
                () -> {
                    throw new AssertionError("read");
                },
                plan -> {
                    throw new AssertionError("bound");
                },
                0,
                1,
                false));
    }

    /**
     * A sheet's suggestion, as delimited text's: a read of its own, from the start, that leaves this
     * one where it was; a header's names, or a width from the first row; and a range's ordinals.
     */
    @Test
    void aSheetSuggests() {
        try (Workbook book = Workbook.open(corpus("ranges.xlsx"))) {
            SheetOptions options = SheetOptions.DEFAULT.withRange(Range.parse("B4:E12"));
            try (Sheet sheet = book.sheet(0, options)) {
                Suggestion found = sheet.suggest();
                assertFalse(sheet.isBound());
                assertEquals(
                        List.of("id", "name", "amt", "day"),
                        found.report().stream().map(SuggestedColumn::name).toList());
                // "merged" beside the ids is a cell no door takes, beside cells one does.
                assertEquals(Door.TEXT, found.plan().get(0).door());
                assertEquals(SuggestStatus.UNRESOLVED, found.report().get(0).status());
                assertEquals(6, found.report().get(0).matched());
                assertEquals(Door.TEXT, found.plan().get(1).door());
                // A typed date's canonical text isn't taken as a date: it's text, resolved, by hand.
                assertEquals(Door.TEXT, found.plan().get(3).door());
                // The plan reads this sheet, from its start.
                sheet.bind(found.plan());
                assertEquals(7, sheet.read().rows());
            }
            // No header: the width is the first row's.
            try (Sheet sheet = book.sheet(0, options.withHeader(false))) {
                Suggestion headless = sheet.suggest(SuggestOptions.DEFAULT.withSampleRows(2));
                assertEquals(4, headless.plan().size());
                assertNull(headless.report().get(0).name());
                assertTrue(headless.truncated());
                assertEquals(2, headless.report().get(0).sampled());
            }
        }
    }

    private static boolean readFails(Workbook book, int index) {
        try (Sheet sheet = book.sheet(index, SheetOptions.DEFAULT, List.of(Column.text(0)))) {
            while (sheet.read() != null) {}
            return false;
        } catch (TabularException refused) {
            return true;
        }
    }

    /** Every sheet of every package, under every declaration: a plan that reads what it sampled, and a suggestion that fails only where a read does. */
    @Test
    void noSheetsPlanRefusesACellItSampled() throws IOException {
        int planned = 0;
        int typed = 0;
        int refused = 0;
        List<NumFormat> declarations = new ArrayList<>();
        declarations.add(null);
        declarations.addAll(List.of(NumFormat.INVARIANT, COMMA_DECIMAL, INDIAN));
        try (DirectoryStream<Path> packages =
                Files.newDirectoryStream(CorpusTest.CORPUS_DIRECTORY.resolve("workbook"))) {
            for (Path path : packages) {
                if (path.toString().endsWith(".py") || Files.isDirectory(path)) {
                    continue;
                }
                Workbook opened;
                try {
                    opened = Workbook.open(path);
                } catch (TabularException notOne) {
                    continue;
                }
                try (Workbook book = opened) {
                    for (int index = 0; index < book.sheets().size(); index++) {
                        for (NumFormat declared : declarations) {
                            Suggestion found;
                            try (Sheet sheet = book.sheet(index, SheetOptions.DEFAULT)) {
                                found = sheet.suggest(
                                        declared == null
                                                ? SuggestOptions.DEFAULT
                                                : SuggestOptions.DEFAULT.withFormat(declared));
                            } catch (TabularException failure) {
                                refused++;
                                assertTrue(
                                        readFails(book, index),
                                        path + ", sheet " + index + ": " + failure.getMessage());
                                continue;
                            }
                            if (found.plan().isEmpty()) {
                                continue;
                            }
                            planned++;
                            typed += (int) found.plan().stream()
                                    .filter(column -> column.door() != Door.TEXT)
                                    .count();
                            try (Sheet read = book.sheet(index, SheetOptions.DEFAULT, found.plan())) {
                                int rows = 0;
                                for (Batch batch = read.read(); batch != null && rows < 1000; batch = read.read()) {
                                    for (Problem problem : batch.faults()) {
                                        if (problem.row() + rows < 1000
                                                && problem.reason() != null
                                                && problem.reason() != CastFailure.EMPTY) {
                                            assertEquals(
                                                    Door.TEXT,
                                                    found.plan()
                                                            .get(problem.planColumn()
                                                                    .getAsInt())
                                                            .door(),
                                                    path + ", sheet " + index + ", " + declared + ": " + problem);
                                        }
                                    }
                                    rows += batch.rows();
                                }
                            } catch (TabularException pastTheSample) {
                                // The sample did not reach it: the read is held to what it sampled.
                            }
                        }
                    }
                }
            }
        }
        assertTrue(planned > 80, planned + " planned");
        assertTrue(refused >= 2, refused + " refused");
        assertTrue(typed > 350, typed + " typed");
    }

    private static String said(Suggestion found) {
        return found.report().stream()
                .map(report -> report.ordinal() + " " + report.name() + " " + report.door() + " " + report.param() + " "
                        + report.format() + " " + report.status() + " " + report.matched() + " " + report.sampled()
                        + " "
                        + report.candidates())
                .collect(Collectors.joining("\n"));
    }

    /** A typed number has no locale: a workbook's number column is a number whether a notation is declared or not, and whichever. */
    @ParameterizedTest
    @CsvSource({"basic.xlsx, amount, ratio", "basic.ods, amount, pct", "excel-web-data.xlsx, pct, money"})
    void aWorkbooksNumberColumnIsNumericUnderAnyFormat(String file, String first, String second) {
        List<SuggestOptions> asked = List.of(
                SuggestOptions.DEFAULT,
                SuggestOptions.DEFAULT.withFormat(NumFormat.INVARIANT),
                SuggestOptions.DEFAULT.withFormat(COMMA_DECIMAL));
        try (Workbook book = Workbook.open(corpus(file))) {
            for (SuggestOptions options : asked) {
                try (Sheet sheet = book.sheet(0, SheetOptions.DEFAULT);
                        Sheet unnoted = book.sheet(0, SheetOptions.DEFAULT.withNotes(false))) {
                    Suggestion found = sheet.suggest(options);
                    for (String name : List.of(first, second)) {
                        SuggestedColumn report = found.report().stream()
                                .filter(column -> name.equals(column.name()))
                                .findFirst()
                                .orElseThrow();
                        assertEquals(SuggestStatus.RESOLVED, report.status(), file + " " + name);
                        assertTrue(report.door().isNumeric(), file + " " + name);
                    }
                    // The sample's notes are its own, whatever a read of the sheet was to have.
                    assertEquals(said(found), said(unnoted.suggest(options)));
                }
            }
        }
    }

    /** The smallest package that holds these strings in column A, each as text in its own row. */
    private static Workbook strings(String... cells) throws IOException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        String ns = "http://schemas.openxmlformats.org";
        try (ZipOutputStream zip = new ZipOutputStream(bytes)) {
            part(
                    zip,
                    "[Content_Types].xml",
                    "<Types xmlns=\"" + ns
                            + "/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/></Types>");
            part(
                    zip,
                    "_rels/.rels",
                    "<Relationships xmlns=\"" + ns + "/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\""
                            + ns
                            + "/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>");
            part(
                    zip,
                    "xl/workbook.xml",
                    "<workbook xmlns=\"" + ns + "/spreadsheetml/2006/main\" xmlns:r=\"" + ns
                            + "/officeDocument/2006/relationships\"><sheets><sheet name=\"Data\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>");
            part(
                    zip,
                    "xl/_rels/workbook.xml.rels",
                    "<Relationships xmlns=\"" + ns + "/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\""
                            + ns
                            + "/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/></Relationships>");
            String rows = IntStream.range(0, cells.length)
                    .mapToObj(at -> "<row r=\"" + (at + 1) + "\"><c r=\"A" + (at + 1) + "\" t=\"inlineStr\"><is><t>"
                            + cells[at] + "</t></is></c></row>")
                    .collect(Collectors.joining());
            part(
                    zip,
                    "xl/worksheets/sheet1.xml",
                    "<worksheet xmlns=\"" + ns + "/spreadsheetml/2006/main\"><sheetData>" + rows
                            + "</sheetData></worksheet>");
        }
        return Workbook.of(bytes.toByteArray());
    }

    private static void part(ZipOutputStream zip, String name, String text) throws IOException {
        zip.putNextEntry(new ZipEntry(name));
        zip.write(utf8(text));
        zip.closeEntry();
    }

    /** One batch is the most a sample can be, whatever sampleRows asks. */
    @Test
    void aSheetsSampleIsOneOfItsBatches() throws IOException {
        try (Workbook book = strings("a", "1", "2", "3", "4", "5", "6", "7", "8");
                Sheet five = book.sheet(0, SheetOptions.DEFAULT.withBatchRows(5));
                Sheet all = book.sheet(0, SheetOptions.DEFAULT)) {
            Suggestion found = five.suggest();
            assertEquals(5, found.report().get(0).sampled());
            assertTrue(found.truncated());
            Suggestion whole = all.suggest();
            assertEquals(8, whole.report().get(0).sampled());
            assertFalse(whole.truncated());
        }
    }

    /**
     * The sample takes notes of its own: with none, every text cell the second door refuses would pass
     * for a typed one, and {@code 1,234} in a string would settle as a number.
     */
    @Test
    void aSheetWithNotesOffStillTellsTextFromATypedCell() throws IOException {
        try (Workbook book = strings("a", "1,234", "5,678");
                Sheet noted = book.sheet(0, SheetOptions.DEFAULT);
                Sheet quiet = book.sheet(0, SheetOptions.DEFAULT.withNotes(false))) {
            Suggestion found = noted.suggest();
            assertEquals(SuggestStatus.AMBIGUOUS, found.report().get(0).status());
            assertEquals(said(found), said(quiet.suggest()));
        }
    }

    /** A failure just past a whole sample only says there's more; among the sampled rows it's the caller's. */
    @Test
    void aFailureJustPastTheSampleSaysTruncated() {
        assertTrue(sampled(2).judge(null, SuggestOptions.DEFAULT).report().size() == 1);
        assertTrue(new SampledTruncation(2).truncated());
        assertEquals(
                "the source broke",
                assertThrows(UncheckedIOException.class, () -> sampled(1))
                        .getCause()
                        .getMessage());
        // The file door's own: a stream is what fails, past a whole sample of a real reader.
        assertTrue(truncatedByAStream());
    }

    private record SampledTruncation(boolean truncated) {
        SampledTruncation(int batchRows) {
            this(sampled(batchRows).judge(null, SuggestOptions.DEFAULT).truncated());
        }
    }

    /** Delivers one batch, then breaks, as a source does. */
    private static Sample sampled(int batchRows) {
        try (DelimitedReader reader =
                DelimitedReader.of(utf8("a\n1\n2\n3\n4\n"), Dialect.CSV, List.of(Column.text(0)), batchRows)) {
            int[] left = {1};
            return Sample.of(
                    () -> {
                        if (left[0]-- > 0) {
                            return reader.read();
                        }
                        throw new UncheckedIOException(new IOException("the source broke"));
                    },
                    plan -> {},
                    1,
                    2,
                    false);
        }
    }

    private static boolean truncatedByAStream() {
        try (DelimitedReader reader = DelimitedReader.of(
                new FailingStream(utf8("a\n1\n2\n"), new IOException("the wire is cut")),
                Dialect.CSV,
                List.of(Column.text(0)))) {
            Sample sample = Sample.of(reader::read, plan -> {}, 1, 2, false);
            return sample.judge(reader.header(), SuggestOptions.DEFAULT).truncated();
        }
    }

    /** A declared order answers the sample's question and only that. */
    @Test
    void aDeclaredDateOrderSettlesADateTheSampleLeftOpen() {
        byte[] open = utf8("day\n01/02/2026\n03/04/2026\n");
        SuggestedColumn report =
                DelimitedReader.suggest(open, Dialect.CSV).report().get(0);
        Column plan = DelimitedReader.suggest(open, Dialect.CSV).plan().get(0);
        assertEquals(SuggestStatus.AMBIGUOUS, report.status());
        assertEquals(Door.TEXT, plan.door());

        Suggestion day = DelimitedReader.suggest(
                open, Dialect.CSV, SuggestOptions.DEFAULT.withDateOrder(DateOrder.DAY_MONTH_YEAR));
        report = day.report().get(0);
        assertEquals(SuggestStatus.RESOLVED, report.status());
        assertEquals(List.of(), report.candidates());
        assertEquals(Door.DATE_ORDERED, day.plan().get(0).door());
        assertEquals(DateOrder.DAY_MONTH_YEAR.code(), report.param());

        Suggestion own = DelimitedReader.suggest(
                open,
                Dialect.CSV,
                SuggestOptions.DEFAULT
                        .withDateOrder(DateOrder.DAY_MONTH_YEAR)
                        .withColumnDateOrder(0, DateOrder.MONTH_DAY_YEAR));
        assertEquals(DateOrder.MONTH_DAY_YEAR.code(), own.report().get(0).param());
        assertEquals(Door.DATE_ORDERED, own.plan().get(0).door());

        // A column whose cells rule the order out keeps the order they leave.
        report = DelimitedReader.suggest(
                        utf8("day\n13/02/2026\n"),
                        Dialect.CSV,
                        SuggestOptions.DEFAULT.withDateOrder(DateOrder.MONTH_DAY_YEAR))
                .report()
                .get(0);
        assertEquals(SuggestStatus.RESOLVED, report.status());
        assertEquals(DateOrder.DAY_MONTH_YEAR.code(), report.param());

        // An order that is not a candidate leaves the question open.
        Suggestion year = DelimitedReader.suggest(
                open, Dialect.CSV, SuggestOptions.DEFAULT.withDateOrder(DateOrder.YEAR_MONTH_DAY));
        assertEquals(SuggestStatus.AMBIGUOUS, year.report().get(0).status());
        assertEquals(Door.TEXT, year.plan().get(0).door());
    }

    /** A sheet answers from its first row whatever it has been up to, and carries on where it was. */
    @Test
    void aPartReadSheetAnswersTheSameAndStaysWhereItIs() {
        try (Workbook book = Workbook.open(corpus("libreoffice-data.xlsx"))) {
            SheetOptions options = SheetOptions.DEFAULT.withBatchRows(1);
            SuggestOptions sample = SuggestOptions.DEFAULT.withSampleRows(1);
            Suggestion fresh;
            try (Sheet sheet = book.sheet(0, options)) {
                fresh = sheet.suggest(sample);
            }
            assertTrue(fresh.plan().size() > 0);
            List<Column> plan = IntStream.range(0, fresh.plan().size())
                    .mapToObj(Column::text)
                    .toList();
            int line;
            try (Sheet whole = book.sheet(0, options, plan)) {
                whole.read();
                line = whole.read().line(0);
            }
            try (Sheet part = book.sheet(0, options, plan)) {
                part.read();
                Suggestion answer = part.suggest(sample);
                assertEquals(fresh.plan(), answer.plan());
                assertEquals(said(fresh), said(answer));
                assertEquals(line, part.read().line(0));
            }
        }
    }

    /** Declared at a little-endian length, as the core reads it, whatever the host. */
    @Test
    void aTokensLengthIsLittleEndian() {
        byte[] packed = Tokens.of("NA").packed();
        assertEquals(2, ByteBuffer.wrap(packed).order(ByteOrder.LITTLE_ENDIAN).getInt());
        assertEquals(6, packed.length);
    }

    /** A batch handed back past what it reads from refuses, and never reads memory that has been freed. */
    @Test
    void aBatchOutlivingItsOwnerRefusesRatherThanReadsFreedMemory() {
        Batch overReader;
        try (DelimitedReader reader = DelimitedReader.of(
                "name\nalice\n".getBytes(StandardCharsets.UTF_8), Dialect.CSV, List.of(Column.text(0)))) {
            overReader = reader.read();
            assertEquals("alice", overReader.string(0, 0));
        }
        assertEquals(0, overReader.rows());
        Batch overReaderToo = overReader;
        assertThrows(RuntimeException.class, () -> overReaderToo.line(0));
        assertThrows(RuntimeException.class, () -> overReaderToo.string(0, 0));
        assertThrows(RuntimeException.class, () -> overReaderToo.raw(0, 0));

        for (String file : List.of("ranges.xlsx", "ranges.ods", "ranges.numbers")) {
            SheetOptions options = SheetOptions.DEFAULT.withRange(Range.parse("B4:C12"));
            List<Column> plan = List.of(Column.text(0), Column.text(1));

            // The sheet closed: its buffers are gone, and the batch is over.
            Batch overSheet;
            try (Workbook book = Workbook.open(corpus(file));
                    Sheet sheet = book.sheet(0, options, plan)) {
                overSheet = sheet.read();
                assertEquals("alpha", overSheet.string(1, 0), file);
            }
            Batch overSheetToo = overSheet;
            assertEquals(0, overSheetToo.rows(), file);
            assertThrows(RuntimeException.class, () -> overSheetToo.string(1, 0), file);

            // The workbook closed under an open sheet: the strings are gone, and a cell in them refuses.
            Workbook book = Workbook.open(corpus(file));
            try (Sheet sheet = book.sheet(0, options, plan)) {
                Batch held = sheet.read();
                assertEquals("NA", held.string(1, 1), file);
                book.close();
                // An xlsx keeps this one among the shared strings. A format that keeps it in the
                // sheet's own buffers still has it, and either way it's never something else.
                if (file.endsWith(".xlsx")) {
                    assertThrows(RuntimeException.class, () -> held.string(1, 1), file);
                    assertThrows(RuntimeException.class, () -> held.raw(1, 1).get(ValueLayout.JAVA_BYTE, 0), file);
                } else {
                    try {
                        assertEquals("NA", held.string(1, 1), file);
                    } catch (IllegalStateException refused) {
                        assertNotNull(refused.getMessage(), file);
                    }
                }
            } finally {
                book.close();
            }
        }
    }
}
