import Benchmark
import Foundation
import HyperTabular

// The workbook reader over the two real-application files of corpus/README.md — Excel's
// excel-win-300k.xlsx and LibreOffice's libreoffice-300k.ods, 300 000 rows × 8 columns each —
// read whole through the plan rust/benches/workbook_benchmarks.rs reads them through, so that
// this binding's number sits beside the core's: "open" is the package opened and nothing
// read; "read" is opened, then the first sheet read in batches of 4096, every column's
// verdicts looked at and every text cell's bytes — the checksum every binding's benchmark
// arrives at, which says they all did the same work. "read strings" is "read" with the text
// column made into Strings.
//
// The files stay out of the tree (corpus/generate/out/, sha256 in the README's table);
// HYPERTABULAR_BENCH_DIR names another directory holding them. A file that is not there is
// skipped.
let plan: [Column] = [.i64(0), .f64(1), .text(2), .date(3), .time(4), .bool(5), .duration(6), .i64(7)]

func read(_ container: Data, strings: Bool) throws -> Int {
    let book = try Workbook(bytes: container)
    let sheet = try book.sheet(0, plan: plan)
    var checksum = 0
    while let batch = try sheet.read() {
        for column in plan.indices {
            for verdict in batch.verdicts(column) where verdict.isOk {
                checksum += 1
            }
        }
        for row in 0..<batch.rows {
            if strings {
                checksum += batch.string(2, row: row)?.utf8.count ?? 0
            } else {
                checksum += batch.text(2, row: row)?.count ?? 0
            }
        }
    }
    return checksum
}

let benchmarks: @Sendable () -> Void = {
    Benchmark.defaultConfiguration = .init(
        metrics: [.wallClock, .mallocCountTotal],
        timeUnits: .milliseconds,
        maxDuration: .seconds(10),
        maxIterations: 10
    )

    let here = URL(fileURLWithPath: #filePath)
    let directory =
        ProcessInfo.processInfo.environment["HYPERTABULAR_BENCH_DIR"].map { URL(fileURLWithPath: $0) }
        ?? here.appending(path: "../../../../../corpus/generate/out").standardized
    for name in ["excel-win-300k.xlsx", "libreoffice-300k.ods"] {
        guard let container = try? Data(contentsOf: directory.appending(path: name)) else {
            print("\(name): not in \(directory.path), skipped")
            continue
        }
        // swiftlint:disable:next force_try
        print("\(name): checksum \(try! read(container, strings: false))")

        Benchmark("\(name) open") { benchmark in
            for _ in benchmark.scaledIterations {
                blackHole(try Workbook(bytes: container).sheets.count)
            }
        }
        Benchmark("\(name) read") { benchmark in
            for _ in benchmark.scaledIterations {
                blackHole(try read(container, strings: false))
            }
        }
        Benchmark("\(name) read strings") { benchmark in
            for _ in benchmark.scaledIterations {
                blackHole(try read(container, strings: true))
            }
        }
    }
}
