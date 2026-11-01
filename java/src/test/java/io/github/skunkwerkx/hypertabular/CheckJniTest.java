package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.Success;
import java.io.ByteArrayInputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import org.junit.jupiter.api.Test;

/**
 * {@code -Xcheck:jni} prints its findings natively, past every Gradle listener, so a suite that
 * merely runs under the flag cannot fail on them. This runs every native the JNI library
 * registers — the delimited calls with their retries and unescape, and every workbook call —
 * in a child JVM of its own while another thread churns the heap, and reads the output. On 25
 * the same program runs the FFM path, where the flag has nothing to check.
 */
class CheckJniTest {
    @Test
    void noCheckJniWarningUnderAllocationPressure() throws Exception {
        var java = ProcessHandle.current().info().command().orElse("java");
        var log = Files.createTempFile("hypertabular-checkjni", ".log");
        try {
            var process = new ProcessBuilder(
                            java,
                            "--enable-native-access=ALL-UNNAMED",
                            "-Xcheck:jni",
                            "-cp",
                            System.getProperty("java.class.path"),
                            PressureMain.class.getName(),
                            CorpusTest.CORPUS_DIRECTORY
                                    .resolve("workbook")
                                    .resolve("basic.xlsx")
                                    .toString())
                    .redirectErrorStream(true)
                    .redirectOutput(log.toFile())
                    .start();
            if (!process.waitFor(300, TimeUnit.SECONDS)) {
                process.destroyForcibly();
                throw new AssertionError("the -Xcheck:jni child did not finish in 300 s");
            }
            var output = Files.readString(log, StandardCharsets.UTF_8);
            assertEquals(0, process.exitValue(), output);
            assertTrue(output.contains("checked"), output);
            // HotSpot words its findings three ways: "WARNING in native method" (a call made with
            // an exception pending, say), "Warning: Calling other JNI functions in the scope of
            // Get/ReleasePrimitiveArrayCritical", and "FATAL ERROR in native method" (which also
            // fails the exit code).
            assertFalse(output.contains("WARNING"), output);
            assertFalse(output.contains("Warning:"), output);
            assertFalse(output.contains("FATAL ERROR"), output);
        } finally {
            Files.deleteIfExists(log);
        }
    }

    /** Every native, repeatedly, while another thread churns the heap. */
    static final class PressureMain {
        private static void expect(Object want, Object got) {
            if (!want.equals(got)) {
                throw new AssertionError(want + " != " + got);
            }
        }

        public static void main(String[] args) throws Exception {
            var stop = new AtomicBoolean();
            var churn = new Thread(() -> {
                byte[] keep = null;
                while (!stop.get()) {
                    keep = new byte[64 * 1024];
                }
                if (keep == null) {
                    throw new IllegalStateException();
                }
            });
            churn.setDaemon(true);
            churn.start();
            byte[] workbook = Files.readAllBytes(Path.of(args[0]));
            StringBuilder text = new StringBuilder("n,quoted\n");
            for (int i = 0; i < 500; i++) {
                text.append(i).append(",\"say \"\"").append(i).append("\"\"\"\n");
            }
            text.append("x,y\n");
            byte[] csv = text.toString().getBytes(StandardCharsets.UTF_8);
            try {
                for (int round = 0; round < 50; round++) {
                    // A stream through a small buffer, so the header, the fill and its retries all
                    // cross many times; a raw read of an escaped cell crosses unescape.
                    try (DelimitedReader reader = DelimitedReader.of(
                            new ByteArrayInputStream(csv),
                            Dialect.CSV,
                            List.of(Column.i32(0), Column.text(1)),
                            7,
                            16)) {
                        expect(List.of("n", "quoted"), reader.header());
                        int rows = 0;
                        for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                            for (int row = 0; row < batch.rows(); row++, rows++) {
                                if (rows < 500) {
                                    expect(new Success<>(rows), batch.get(0, row, Integer.class));
                                    expect("say \"" + rows + "\"", batch.string(1, row));
                                } else {
                                    expect(
                                            new Fault<Integer>(CastFailure.MALFORMED, 0, 1),
                                            batch.get(0, row, Integer.class));
                                }
                            }
                        }
                        expect(501, rows);
                    }
                    // Every workbook call: open, sheets, strings, styles, a sheet, its header and
                    // its fill.
                    try (Workbook book = Workbook.of(workbook);
                            Sheet sheet = book.sheet(0, SheetOptions.DEFAULT)) {
                        Header header = sheet.header();
                        sheet.bind(List.of(Column.text(header.ordinal(header.get(0)))));
                        int rows = 0;
                        for (Batch batch = sheet.read(); batch != null; batch = sheet.read()) {
                            rows += batch.rows();
                        }
                        if (rows == 0) {
                            throw new AssertionError("no rows in " + args[0]);
                        }
                    }
                }
                expect(Tabular.nativeVersion(), Tabular.nativeVersion());
            } finally {
                stop.set(true);
            }
            System.out.println("checked");
        }
    }
}
