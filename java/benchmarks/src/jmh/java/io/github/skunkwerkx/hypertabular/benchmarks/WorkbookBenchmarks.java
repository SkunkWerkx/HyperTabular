package io.github.skunkwerkx.hypertabular.benchmarks;

import io.github.skunkwerkx.hypertabular.Batch;
import io.github.skunkwerkx.hypertabular.Column;
import io.github.skunkwerkx.hypertabular.Sheet;
import io.github.skunkwerkx.hypertabular.SheetOptions;
import io.github.skunkwerkx.hypertabular.Workbook;
import java.io.IOException;
import java.lang.foreign.MemorySegment;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.concurrent.TimeUnit;
import org.openjdk.jmh.annotations.Benchmark;
import org.openjdk.jmh.annotations.BenchmarkMode;
import org.openjdk.jmh.annotations.Mode;
import org.openjdk.jmh.annotations.OutputTimeUnit;
import org.openjdk.jmh.annotations.Param;
import org.openjdk.jmh.annotations.Scope;
import org.openjdk.jmh.annotations.Setup;
import org.openjdk.jmh.annotations.State;

/**
 * The workbook reader over the two real-application files of {@code corpus/README.md} —
 * Excel's {@code excel-win-300k.xlsx} and LibreOffice's {@code libreoffice-300k.ods}, 300 000
 * rows × 8 columns each — read whole through the plan {@code rust/benches/workbook_benchmarks.rs}
 * reads them through, so that this binding's number sits beside the core's: {@code open} is
 * the package opened and nothing read; {@code read} is opened, then the first sheet read in
 * batches of 4096, every column's verdicts looked at and every text cell's bytes — the
 * checksum every binding's benchmark arrives at, which says they all did the same work.
 * {@code readStrings} is {@code read} with the text column made into strings.
 *
 * <p>The files stay out of the tree ({@code corpus/generate/out/}, sha256 in the README's
 * table); {@code HYPERTABULAR_BENCH_DIR} names another directory holding them.
 */
@State(Scope.Benchmark)
@BenchmarkMode(Mode.AverageTime)
@OutputTimeUnit(TimeUnit.MILLISECONDS)
public class WorkbookBenchmarks {
    private static final List<Column> PLAN = List.of(
            Column.i64(0),
            Column.f64(1),
            Column.text(2),
            Column.date(3),
            Column.time(4),
            Column.bool(5),
            Column.duration(6),
            Column.i64(7));

    @Param({"excel-win-300k.xlsx", "libreoffice-300k.ods"})
    public String file;

    private byte[] container;

    @Setup
    public void setup() throws IOException {
        String directory = System.getenv("HYPERTABULAR_BENCH_DIR");
        if (directory == null) {
            directory = System.getProperty("hypertabular.corpus");
        }
        container = Files.readAllBytes(Path.of(directory, file));
        System.out.println(file + ": checksum " + read());
    }

    @Benchmark
    public int open() {
        try (Workbook book = Workbook.of(container)) {
            return book.sheets().size();
        }
    }

    @Benchmark
    public long read() {
        try (Workbook book = Workbook.of(container);
                Sheet sheet = book.sheet(0, SheetOptions.DEFAULT, PLAN)) {
            long checksum = 0;
            for (Batch batch; (batch = sheet.read()) != null; ) {
                for (int column = 0; column < PLAN.size(); column++) {
                    for (int row = 0; row < batch.rows(); row++) {
                        if (batch.isOk(column, row)) {
                            checksum++;
                        }
                    }
                }
                for (int row = 0; row < batch.rows(); row++) {
                    MemorySegment text = batch.text(2, row);
                    if (text != null) {
                        checksum += text.byteSize();
                    }
                }
            }
            return checksum;
        }
    }

    @Benchmark
    public long readStrings() {
        try (Workbook book = Workbook.of(container);
                Sheet sheet = book.sheet(0, SheetOptions.DEFAULT, PLAN)) {
            long checksum = 0;
            for (Batch batch; (batch = sheet.read()) != null; ) {
                for (int column = 0; column < PLAN.size(); column++) {
                    for (int row = 0; row < batch.rows(); row++) {
                        if (batch.isOk(column, row)) {
                            checksum++;
                        }
                    }
                }
                for (int row = 0; row < batch.rows(); row++) {
                    String text = batch.string(2, row);
                    if (text != null) {
                        checksum += text.length();
                    }
                }
            }
            return checksum;
        }
    }
}
