package io.github.skunkwerkx.hypertabular.aotsmoketest;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.Checked;
import io.github.skunkwerkx.hypercast.Constraints;
import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.Keyword;
import io.github.skunkwerkx.hypercast.Success;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import io.github.skunkwerkx.hypercast.Verdict;
import io.github.skunkwerkx.hypercast.Violation;
import io.github.skunkwerkx.hypertabular.Batch;
import io.github.skunkwerkx.hypertabular.Column;
import io.github.skunkwerkx.hypertabular.DelimitedReader;
import io.github.skunkwerkx.hypertabular.Dialect;
import io.github.skunkwerkx.hypertabular.Door;
import io.github.skunkwerkx.hypertabular.Encoding;
import io.github.skunkwerkx.hypertabular.Header;
import io.github.skunkwerkx.hypertabular.Problem;
import io.github.skunkwerkx.hypertabular.Range;
import io.github.skunkwerkx.hypertabular.Sheet;
import io.github.skunkwerkx.hypertabular.SheetOptions;
import io.github.skunkwerkx.hypertabular.Suggestion;
import io.github.skunkwerkx.hypertabular.Tabular;
import io.github.skunkwerkx.hypertabular.TabularException;
import io.github.skunkwerkx.hypertabular.TabularFailure;
import io.github.skunkwerkx.hypertabular.Workbook;
import io.github.skunkwerkx.hypertabular.WorkbookFormat;
import java.io.ByteArrayInputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.lang.foreign.ValueLayout;
import java.lang.reflect.Modifier;
import java.math.BigDecimal;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.Instant;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.LocalTime;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import java.util.Objects;
import java.util.UUID;

/**
 * Crosses every native entry point the binding declares — the version probe, the header,
 * the fill through every door and through the checked doors, the unescape behind a raw read, and every workbook entry point — against the real
 * native library: on an ordinary JVM ({@code ./gradlew :aot-smoke-test:run}, where JDK 25
 * takes the jar's FFM path), on the floor JDK ({@code :aot-smoke-test:runFloor}, where its JNI
 * base loads) and as a GraalVM Native Image binary ({@code :aot-smoke-test:nativeRun}). Exit
 * code 0 only if every cell lands as expected.
 */
public final class Main {
    private Main() {}

    private static int failures;

    private static <T> void check(String name, Verdict<T> verdict, T expected) {
        switch (verdict) {
            case Success<T> success
            when success.value().equals(expected) -> System.out.println("ok   " + name + " = " + success.value());
            case Success<T> success -> {
                System.out.println("FAIL " + name + ": " + success.value() + " (expected " + expected + ")");
                failures++;
            }
            case Fault<T> fault -> {
                System.out.println("FAIL " + name + ": " + fault + " (expected " + expected + ")");
                failures++;
            }
        }
    }

    private static void expect(String name, boolean condition) {
        System.out.println((condition ? "ok   " : "FAIL ") + name);
        if (!condition) {
            failures++;
        }
    }

    /**
     * Reads one row through all twenty-two doors, one file header-first, one broken file, one workbook and the 1.1 reader's declarations, prints every answer,
     * and exits non-zero if any of them was wrong.
     *
     * @param args ignored
     */
    public static void main(String[] args) {
        // The non-throwing gate first: a library that did not load is this line, not a
        // stack trace out of the first read.
        System.out.println("available: " + Tabular.isAvailable());
        if (!Tabular.isAvailable()) {
            System.out.println("smoke test FAILED: the native library did not load.");
            System.exit(1);
        }
        requireExpectedBackend();
        if (System.getProperty("org.graalvm.nativeimage.imagecode") != null) {
            requireNoJniLibraryEmbedded();
        }
        String version = Tabular.nativeVersion();
        expect("native library " + version, version.matches("\\d+\\.\\d+\\.\\d+"));

        String text = String.join(
                        ",",
                        "flag",
                        "i8",
                        "i16",
                        "i32",
                        "i64",
                        "u8",
                        "u16",
                        "u32",
                        "u64",
                        "f32",
                        "f64",
                        "money",
                        "id",
                        "at",
                        "unix",
                        "serial",
                        "day",
                        "us",
                        "civil",
                        "time",
                        "span",
                        "\"say \"\"hi\"\"\"")
                + "\n"
                + String.join(
                        ",",
                        "yes",
                        "-128",
                        "32767",
                        "(1234)",
                        "9223372036854775807",
                        "255",
                        "65535",
                        "4294967295",
                        "18446744073709551615",
                        "2.5",
                        "25.5%",
                        "1234.50",
                        "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
                        "2026-01-02T15:04:05.123456789+05:00",
                        "1700000000123",
                        "45292.75",
                        "2024-01-31",
                        "1/7/2026",
                        "1/7/2026 3:04 PM",
                        "15:04:05.5",
                        "PT1H30M",
                        "\"a \"\"quoted\"\" cell\"")
                + "\n";

        List<Column> plan = List.of(
                Column.bool(0),
                Column.i8(1),
                Column.i16(2),
                Column.i32(3),
                Column.i64(4),
                Column.u8(5),
                Column.u16(6),
                Column.u32(7),
                Column.u64(8),
                Column.f32(9),
                Column.f64(10),
                Column.decimal(11),
                Column.uuid(12),
                Column.timestamp(13),
                Column.unix(14, UnixPrecision.MILLISECONDS),
                Column.excelSerial(15, ExcelEpoch.Y1900),
                Column.date(16),
                Column.date(17, DateOrder.MONTH_DAY_YEAR),
                Column.dateTime(18, DateOrder.MONTH_DAY_YEAR),
                Column.time(19),
                Column.duration(20),
                Column.text(21),
                Column.i32(21));

        // Through a stream and a buffer far smaller than a record, so the refill and the
        // buffer's growth are crossed too.
        try (DelimitedReader reader = DelimitedReader.of(
                new ByteArrayInputStream(text.getBytes(StandardCharsets.UTF_8)), Dialect.CSV, plan, 8, 16)) {
            List<String> header = reader.header();
            expect(
                    "header",
                    header != null && header.size() == 22 && header.get(21).equals("say \"hi\""));
            Batch batch = reader.read();
            expect("read", batch != null && batch.rows() == 1);
            check("bool", batch.get(0, 0, Boolean.class), true);
            check("i8", batch.get(1, 0, Byte.class), (byte) -128);
            check("i16", batch.get(2, 0, Short.class), (short) 32767);
            check("i32", batch.get(3, 0, Integer.class), -1234);
            check("i64", batch.get(4, 0, Long.class), Long.MAX_VALUE);
            check("u8", batch.get(5, 0, Integer.class), 255);
            check("u16", batch.get(6, 0, Integer.class), 65535);
            check("u32", batch.get(7, 0, Long.class), 4_294_967_295L);
            // u64::MAX arrives as the two's-complement bit pattern.
            check("u64", batch.get(8, 0, Long.class), -1L);
            check("f32", batch.get(9, 0, Float.class), 2.5f);
            check("f64", batch.get(10, 0, Double.class), 0.255);
            check("decimal", batch.get(11, 0, BigDecimal.class), new BigDecimal("1234.5"));
            check("uuid", batch.get(12, 0, UUID.class), UUID.fromString("6ba7b810-9dad-11d1-80b4-00c04fd430c8"));
            check("timestamp", batch.get(13, 0, Instant.class), Instant.parse("2026-01-02T10:04:05.123456789Z"));
            check("unix", batch.get(14, 0, Instant.class), Instant.ofEpochMilli(1_700_000_000_123L));
            check("excel serial", batch.get(15, 0, Instant.class), Instant.parse("2024-01-01T18:00:00Z"));
            check("date", batch.get(16, 0, LocalDate.class), LocalDate.of(2024, 1, 31));
            check("ordered date", batch.get(17, 0, LocalDate.class), LocalDate.of(2026, 1, 7));
            check("civil", batch.get(18, 0, LocalDateTime.class), LocalDateTime.of(2026, 1, 7, 15, 4));
            check("time", batch.get(19, 0, LocalTime.class), LocalTime.of(15, 4, 5, 500_000_000));
            check("duration", batch.get(20, 0, Duration.class), Duration.ofMinutes(90));
            check("text", batch.get(21, 0, String.class), "a \"quoted\" cell");
            expect("column", batch.values(3).getAtIndex(ValueLayout.JAVA_INT, 0) == -1234 && batch.isOk(3, 0));
            expect("line", batch.line(0) == 2);
            // A cell that does not cast: the union's fault case, and its text still to hand.
            // The exhaustive two-arm switch must survive AOT too.
            expect(
                    "fault",
                    switch (batch.get(22, 0, Integer.class)) {
                        case Success<Integer> success -> false;
                        case Fault<Integer> fault ->
                            fault.reason() == CastFailure.MALFORMED
                                    && batch.rawString(22, 0).equals("a \"quoted\" cell");
                    });
            expect("end", reader.read() == null);
        }

        // Header first: opened with no plan, the plan made from the header's names, and the
        // rows read as rows, a text cell as chars without a string.
        try (DelimitedReader reader =
                DelimitedReader.of("id,name\n7,zoë\n".getBytes(StandardCharsets.UTF_8), Dialect.CSV)) {
            Header header = reader.header();
            reader.bind(List.of(Column.i32(header.ordinal("id")), Column.text(header.ordinal("name"))));
            List<String> seen = new ArrayList<>();
            reader.forEachRow(row -> seen.add(
                    (row.get(0, Integer.class) instanceof Success<Integer> id ? id.value() : -1) + " " + row.chars(1)));
            expect("header first " + seen, seen.equals(List.of("7 zoë")));
        }

        // Columns that declare HyperCast's constraints: a value judged and passed, a violation keeping
        // the value as cast, a text enum match read as its declared entry, and the violation listed.
        try (DelimitedReader reader = DelimitedReader.of(
                "n,sex\n11,MALE\n7.5,x\n".getBytes(StandardCharsets.UTF_8),
                Dialect.CSV,
                List.of(
                        Column.decimal(
                                0,
                                Constraints.decimal()
                                        .maximum("10")
                                        .multipleOf("0.5")
                                        .build()),
                        Column.text(
                                1, Constraints.text().enumOf("Female", "Male").build())))) {
            Batch batch = reader.read();
            if (batch != null && batch.rows() == 2) {
                expect(
                        "checked violation",
                        batch.getChecked(0, 0, BigDecimal.class) instanceof Violation<BigDecimal> broken
                                && broken.value().compareTo(new BigDecimal("11")) == 0
                                && broken.keyword().equals(Keyword.MAXIMUM));
                expect(
                        "checked passed",
                        batch.getChecked(0, 1, BigDecimal.class) instanceof Checked<BigDecimal> passed
                                && passed.value().compareTo(new BigDecimal("7.5")) == 0
                                && !passed.adjusted());
                expect(
                        "checked enum",
                        batch.getChecked(1, 0, String.class) instanceof Checked<String> passed
                                && passed.value().equals("Male")
                                && passed.enumIndex().orElse(-1) == 1);
                int listed = 0;
                for (Problem problem : batch.faults()) {
                    listed += problem.keyword() == null ? 0 : 1;
                }
                expect("checked listed", listed == 2);
            } else {
                expect("checked read", false);
            }
        }

        // A structural failure is an exception, after the intact rows.
        try (DelimitedReader broken = DelimitedReader.of(
                "a,b\n1,2\n3\n".getBytes(StandardCharsets.UTF_8), Dialect.CSV, List.of(Column.i32(0)))) {
            Batch intact = broken.read();
            expect("intact row", intact != null && intact.rows() == 1);
            broken.read();
            expect("structure", false);
        } catch (TabularException e) {
            expect(
                    "structure",
                    e.failure() == TabularFailure.COLUMN_COUNT
                            && e.record() == 2
                            && e.line() == 3
                            && e.byteOffset() == 8
                            && e.expected() == 2
                            && e.found() == 1);
        }

        // A workbook: opened, its sheets listed, its strings and styles loaded, a sheet
        // positioned on, its header read and its rows filled — every workbook entry point,
        // through the same batch.
        try (InputStream resource = Main.class.getResourceAsStream("/basic.xlsx")) {
            byte[] bytes = Objects.requireNonNull(resource, "basic.xlsx").readAllBytes();
            // From a stream, read to its end into memory the workbook owns.
            try (Workbook book = Workbook.of(new ByteArrayInputStream(bytes))) {
                expect(
                        "workbook",
                        book.format() == WorkbookFormat.XLSX
                                && book.dateSystem() == ExcelEpoch.Y1900
                                && !book.sheets().isEmpty());
                // Header first: the sheet's header row read before its plan is bound.
                try (Sheet sheet = book.sheet(0, SheetOptions.DEFAULT)) {
                    Header header = sheet.header();
                    expect("sheet header", header != null && header.size() > 2);
                    sheet.bind(List.of(
                            Column.i64(header.ordinal(header.get(0))),
                            Column.text(header.ordinal(header.get(1))),
                            Column.decimal(header.ordinal(header.get(2)))));
                    int rows = 0;
                    for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                        rows += batch.rows();
                    }
                    expect("sheet rows (" + rows + ")", rows > 0);
                }
            }
        } catch (IOException e) {
            expect("basic.xlsx: " + e, false);
        }
        try (Workbook book = Workbook.of("not a zip".getBytes(StandardCharsets.UTF_8))) {
            expect("not a workbook", false);
        } catch (TabularException e) {
            expect("not a workbook", e.failure() == TabularFailure.NOT_A_ZIP);
        }

        // The entry points only the 1.1 reader's declarations cross: a decoded encoding, a batch's
        // faults, a suggestion, a range, and a sheet read through it.
        try (DelimitedReader reader = DelimitedReader.of(
                new byte[] {'n', '\n', (byte) 0xE9, '\n'},
                Dialect.CSV.withEncoding(Encoding.LATIN_1),
                List.of(Column.text(0), Column.i32(0)))) {
            Batch batch = reader.read();
            expect("transcode", "\u00e9".equals(batch.string(0, 0)));
            Problem problem = batch.faults().iterator().next();
            expect("faults", problem.reason() == CastFailure.MALFORMED && problem.line() == 2);
        }
        Suggestion found = DelimitedReader.suggest("a\n1\n".getBytes(StandardCharsets.UTF_8), Dialect.CSV);
        expect(
                "suggest",
                found.plan().get(0).door() == Door.BOOL && found.report().get(0).matched() == 1);
        expect("range", Range.parse("B4:E12").equals(new Range(4, 2, 12, 5)));
        try (InputStream smoke = Main.class.getResourceAsStream("/basic.xlsx")) {
            byte[] bytes = Objects.requireNonNull(smoke, "basic.xlsx").readAllBytes();
            try (Workbook book = Workbook.of(bytes);
                    Sheet sheet =
                            book.sheet(0, SheetOptions.DEFAULT.withHeader(false).withRange(Range.parse("A1:B2")))) {
                expect("sheet with a range", !sheet.suggest().plan().isEmpty());
            }
        } catch (IOException e) {
            expect("basic.xlsx: " + e, false);
        }

        System.out.println(failures == 0 ? "smoke test passed." : "smoke test FAILED (" + failures + ").");
        System.exit(failures == 0 ? 0 : 1);
    }

    // The path the JDK picked from the multi-release jar: FFM on 25 and later, the JNI base
    // below that, told apart by Native's natives and, on a JVM, its class file major.
    private static void requireExpectedBackend() {
        boolean ffm = Runtime.version().feature() >= 25;
        String expected = ffm ? "ffm" : "jni";
        Class<?> core;
        try {
            core = Class.forName("io.github.skunkwerkx.hypertabular.Native");
        } catch (ClassNotFoundException e) {
            throw new AssertionError("Native is not loadable", e);
        }
        long natives = Arrays.stream(core.getDeclaredMethods())
                .filter(m -> Modifier.isNative(m.getModifiers()))
                .count();
        require(natives == (ffm ? 0 : 24), expected + " expected, but Native declares " + natives + " natives");
        String major = "n/a (image)";
        if (System.getProperty("org.graalvm.nativeimage.imagecode") == null) {
            try (InputStream in = core.getResourceAsStream("Native.class")) {
                require(in != null, "Native.class is not readable as a resource");
                DataInputStream data = new DataInputStream(in);
                data.readInt();
                data.readUnsignedShort();
                int found = data.readUnsignedShort();
                require(found == (ffm ? 69 : 65), expected + " expected, but Native is class file major " + found);
                major = Integer.toString(found);
            } catch (IOException e) {
                throw new AssertionError("Native.class is unreadable", e);
            }
        }
        System.out.println("backend: " + expected + " (" + natives + " natives, class major " + major + ")");
    }

    // The image must not carry the JNI library: only the FFM path survives into it (the base
    // Native is unreachable), and the resource globs cover the core library alone. The JNI
    // library names are the three the Java side resolves, for each of the eight RIDs.
    private static void requireNoJniLibraryEmbedded() {
        String[] rids = {
            "win-x64",
            "win-arm64",
            "osx-x64",
            "osx-arm64",
            "linux-x64",
            "linux-arm64",
            "linux-musl-x64",
            "linux-musl-arm64"
        };
        String[] names = {"libhypertabular_jni.so", "libhypertabular_jni.dylib", "hypertabular_jni.dll"};
        for (String rid : rids) {
            for (String name : names) {
                require(
                        Main.class.getResource("/native/" + rid + "/" + name) == null,
                        "the image embeds the JNI library /native/" + rid + "/" + name);
            }
        }
        System.out.println("image: no JNI library embedded");
    }

    private static void require(boolean condition, String message) {
        if (!condition) {
            throw new AssertionError(message);
        }
    }
}
