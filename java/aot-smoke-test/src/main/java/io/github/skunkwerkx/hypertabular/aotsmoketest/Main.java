package io.github.skunkwerkx.hypertabular.aotsmoketest;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.DateOrder;
import io.github.skunkwerkx.hypercast.ExcelEpoch;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.Success;
import io.github.skunkwerkx.hypercast.UnixPrecision;
import io.github.skunkwerkx.hypercast.Verdict;
import io.github.skunkwerkx.hypertabular.Column;
import io.github.skunkwerkx.hypertabular.DelimitedReader;
import io.github.skunkwerkx.hypertabular.Dialect;
import io.github.skunkwerkx.hypertabular.Tabular;
import io.github.skunkwerkx.hypertabular.TabularException;
import io.github.skunkwerkx.hypertabular.TabularFailure;
import java.io.ByteArrayInputStream;
import java.lang.foreign.ValueLayout;
import java.math.BigDecimal;
import java.nio.charset.StandardCharsets;
import java.time.Duration;
import java.time.Instant;
import java.time.LocalDate;
import java.time.LocalDateTime;
import java.time.LocalTime;
import java.util.List;
import java.util.UUID;

/**
 * Crosses every native entry point the binding declares — the version probe, the header,
 * the fill through every door, and the unescape behind a raw read — against the real
 * native library: on an ordinary JVM ({@code ./gradlew :aot-smoke-test:run}) and as a
 * GraalVM Native Image binary ({@code :aot-smoke-test:nativeRun}). Exit code 0 only if
 * every cell lands as expected.
 */
public final class Main {
    private Main() {}

    private static int failures;

    private static <T> void check(String name, Verdict<T> verdict, T expected) {
        switch (verdict) {
            case Success<T> success when success.value().equals(expected) ->
                    System.out.println("ok   " + name + " = " + success.value());
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
     * Reads one row through all twenty-two doors and one broken file, prints every answer,
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
        String version = Tabular.nativeVersion();
        expect("native library " + version, version.matches("\\d+\\.\\d+\\.\\d+"));

        String text = String.join(",",
                "flag", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64", "money", "id",
                "at", "unix", "serial", "day", "us", "civil", "time", "span", "\"say \"\"hi\"\"\"") + "\n"
                + String.join(",",
                "yes", "-128", "32767", "(1234)", "9223372036854775807", "255", "65535",
                "4294967295", "18446744073709551615", "2.5", "25.5%", "1234.50",
                "6ba7b810-9dad-11d1-80b4-00c04fd430c8", "2026-01-02T15:04:05.123456789+05:00", "1700000000123",
                "45292.75", "2024-01-31", "1/7/2026", "1/7/2026 3:04 PM", "15:04:05.5", "PT1H30M",
                "\"a \"\"quoted\"\" cell\"") + "\n";

        List<Column> plan = List.of(
                Column.bool(0), Column.i8(1), Column.i16(2), Column.i32(3), Column.i64(4),
                Column.u8(5), Column.u16(6), Column.u32(7), Column.u64(8), Column.f32(9),
                Column.f64(10), Column.decimal(11), Column.uuid(12), Column.timestamp(13),
                Column.unix(14, UnixPrecision.MILLISECONDS), Column.excelSerial(15, ExcelEpoch.Y1900),
                Column.date(16), Column.date(17, DateOrder.MONTH_DAY_YEAR),
                Column.dateTime(18, DateOrder.MONTH_DAY_YEAR), Column.time(19), Column.duration(20),
                Column.text(21), Column.i32(21));

        // Through a stream and a buffer far smaller than a record, so the refill and the
        // buffer's growth are crossed too.
        try (DelimitedReader reader = DelimitedReader.of(
                new ByteArrayInputStream(text.getBytes(StandardCharsets.UTF_8)), Dialect.CSV, plan, 8, 16)) {
            List<String> header = reader.header();
            expect("header", header != null && header.size() == 22 && header.get(21).equals("say \"hi\""));
            expect("read", reader.read() && reader.rows() == 1);
            check("bool", reader.bool(0, 0), true);
            check("i8", reader.i8(1, 0), (byte) -128);
            check("i16", reader.i16(2, 0), (short) 32767);
            check("i32", reader.i32(3, 0), -1234);
            check("i64", reader.i64(4, 0), Long.MAX_VALUE);
            check("u8", reader.u8(5, 0), 255);
            check("u16", reader.u16(6, 0), 65535);
            check("u32", reader.u32(7, 0), 4_294_967_295L);
            // u64::MAX arrives as the two's-complement bit pattern.
            check("u64", reader.u64(8, 0), -1L);
            check("f32", reader.f32(9, 0), 2.5f);
            check("f64", reader.f64(10, 0), 0.255);
            check("decimal", reader.decimal(11, 0), new BigDecimal("1234.5"));
            check("uuid", reader.uuid(12, 0), UUID.fromString("6ba7b810-9dad-11d1-80b4-00c04fd430c8"));
            check("timestamp", reader.timestamp(13, 0), Instant.parse("2026-01-02T10:04:05.123456789Z"));
            check("unix", reader.timestamp(14, 0), Instant.ofEpochMilli(1_700_000_000_123L));
            check("excel serial", reader.timestamp(15, 0), Instant.parse("2024-01-01T18:00:00Z"));
            check("date", reader.date(16, 0), LocalDate.of(2024, 1, 31));
            check("ordered date", reader.date(17, 0), LocalDate.of(2026, 1, 7));
            check("civil", reader.dateTime(18, 0), LocalDateTime.of(2026, 1, 7, 15, 4));
            check("time", reader.time(19, 0), LocalTime.of(15, 4, 5, 500_000_000));
            check("duration", reader.duration(20, 0), Duration.ofMinutes(90));
            expect("text", "a \"quoted\" cell".equals(reader.string(21, 0)));
            expect("column", reader.values(3).getAtIndex(ValueLayout.JAVA_INT, 0) == -1234 && reader.isOk(3, 0));
            // A cell that does not cast: the union's fault case, and its text still to hand.
            // The exhaustive two-arm switch must survive AOT too.
            expect("fault", switch (reader.i32(22, 0)) {
                case Success<Integer> success -> false;
                case Fault<Integer> fault -> fault.reason() == CastFailure.MALFORMED
                        && reader.rawString(22, 0).equals("a \"quoted\" cell");
            });
            expect("end", !reader.read());
        }

        // A structural failure is an exception, after the intact rows.
        try (DelimitedReader broken = DelimitedReader.of(
                "a,b\n1,2\n3\n".getBytes(StandardCharsets.UTF_8), Dialect.CSV, List.of(Column.i32(0)))) {
            expect("intact row", broken.read() && broken.rows() == 1);
            broken.read();
            expect("structure", false);
        } catch (TabularException e) {
            expect("structure", e.failure() == TabularFailure.COLUMN_COUNT && e.record() == 2 && e.line() == 3
                    && e.byteOffset() == 8 && e.expected() == 2 && e.found() == 1);
        }

        System.out.println(failures == 0 ? "smoke test passed." : "smoke test FAILED (" + failures + ").");
        System.exit(failures == 0 ? 0 : 1);
    }
}
