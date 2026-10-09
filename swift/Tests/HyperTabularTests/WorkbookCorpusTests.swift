import Foundation
import XCTest

@testable import HyperTabular

/// Replays `corpus/workbook.json` — the contract every binding replays, and the one the Rust
/// binding replays — through this binding: each package opened from memory copied, in
/// place, from its path, and from a stream, each sheet read by index and (where the name
/// finds it) by name, opened with its plan and header first, in batches of one row, of two,
/// and of more than any sheet has.
extension CorpusTests {
    struct WorkbookCase: Decodable {
        let name: String
        let file: String
        let format: String?
        let epoch: Int?
        let sheets: [Listed]?
        /// Absent for a package the core refuses to open.
        let sheet: Int?
        let options: Settings?
        let plan: [Entry]?
        let header: [String]?
        let rows: [[Cell]]?
        let numbers: [Int]?
        let failure: Failure?
    }

    struct Listed: Decodable {
        let name: String
        let hidden: Bool
    }

    struct Settings: Decodable {
        let has_header: Bool
        let skip_empty_rows: Bool
    }

    func workbookCorpus() throws -> [WorkbookCase] {
        guard let directory = Self.corpusDirectory else {
            throw XCTSkip("the conformance corpus is not reachable from a WASI sandbox")
        }
        let data = try Data(contentsOf: directory.appendingPathComponent("workbook.json"))
        return try JSONDecoder().decode([WorkbookCase].self, from: data)
    }

    func testWorkbookCorpus() throws {
        try replayWorkbookCorpus()
    }

    /// The corpus again with every buffer starting at one element and never asked to be large
    /// up front: each read stops wherever the core runs out of room and resumes in a grown
    /// buffer that has to have kept what the old one held. A grow that dropped it would read
    /// garbage, and the corpus would say so.
    func testWorkbookCorpusWithBuffersThatStartWithNoRoom() throws {
        Workbook.stingy = true
        Workbook.grown = (0, 0, 0)
        defer { Workbook.stingy = false }
        try replayWorkbookCorpus()
        // Not once per call: many times, mid-part, for each of the three.
        let grown = Workbook.grown
        XCTAssertGreaterThan(grown.window, 1_000)
        XCTAssertGreaterThan(grown.arena, 1_000)
        XCTAssertGreaterThan(grown.cells, 1_000)
    }

    private func replayWorkbookCorpus() throws {
        let corpus = try workbookCorpus()
        XCTAssertGreaterThanOrEqual(corpus.count, 80)
        var cells = 0
        for vector in corpus {
            let path = Self.corpusDirectory!.appendingPathComponent(vector.file).path
            let bytes = try Data(contentsOf: URL(fileURLWithPath: path))
            // Memory the test holds for the in-place opening, alive until the case is done.
            let inPlace = UnsafeMutableRawBufferPointer.allocate(byteCount: bytes.count, alignment: 1)
            defer { inPlace.deallocate() }
            inPlace.copyBytes(from: bytes)
            do {
                let openings: [(String, () throws -> Workbook)] = [
                    ("memory", { try Workbook(bytes: bytes) }),
                    ("in place", { try Workbook(bytesNoCopy: UnsafeRawBufferPointer(inPlace)) }),
                    ("path", { try Workbook(contentsOfFile: path) }),
                    ("input stream", { try Workbook(reading: InputStream(data: bytes)) }),
                    ("source in chunks of 7", { try Workbook(reading: self.chunked(Array(bytes), 7)) }),
                ]

                // A package the core refuses: every way of opening it gives the one failure.
                guard let index = vector.sheet else {
                    for (source, open) in openings {
                        XCTAssertThrowsError(try open(), "\(vector.name) (\(source))") {
                            XCTAssertEqual(
                                $0 as? TabularError, expectedFailure(vector.failure), "\(vector.name) (\(source))")
                        }
                    }
                    continue
                }

                let plan = vector.plan!.map(column(of:))
                let sheets = vector.sheets!
                let rows = vector.rows!
                cells += rows.count * plan.count
                for (source, open) in openings {
                    let book = try open()
                    XCTAssertEqual(book.format, vector.format == "xlsx" ? .xlsx : .ods, vector.name)
                    XCTAssertEqual(book.dateSystem, vector.epoch == 2 ? .y1904 : .y1900, vector.name)
                    XCTAssertEqual(book.sheets.map(\.name), sheets.map(\.name), vector.name)
                    XCTAssertEqual(book.sheets.map(\.hidden), sheets.map(\.hidden), vector.name)
                    let name = sheets[index].name
                    let byName = book.sheets.firstIndex { $0.name == name } == index

                    for batchRows in [1, 2, 1024] {
                        let options = SheetOptions(
                            hasHeader: vector.options!.has_header, skipEmptyRows: vector.options!.skip_empty_rows,
                            batchRows: batchRows)
                        for named in [false, true] where !named || byName {
                            let label =
                                "\(vector.name): \(source), \(batchRows) rows a batch\(named ? ", by name" : "")"
                            let sheet =
                                named
                                ? try book.sheet(named: name, options: options, plan: plan)
                                : try book.sheet(index, options: options, plan: plan)
                            replay(label, vector, sheet, batchRows: batchRows)
                            // Header first: the plan bound once the header has been read. The
                            // header row stays in the row's slots for a sheet that repeats it.
                            let unbound =
                                named
                                ? try book.sheet(named: name, options: options)
                                : try book.sheet(index, options: options)
                            try bindHeaderFirst("\(label), header first", unbound, plan)
                            replay("\(label), header first", vector, unbound, batchRows: batchRows)
                        }
                    }
                }
            }
        }
        XCTAssertGreaterThanOrEqual(cells, 12_000)
    }

    /// Holds a sheet just opened without a plan to what an unbound sheet promises, and binds
    /// `plan` to it.
    private func bindHeaderFirst(_ label: String, _ sheet: Sheet, _ plan: [Column]) throws {
        XCTAssertFalse(sheet.isBound, label)
        XCTAssertEqual(sheet.plan, [], label)
        for _ in 0..<2 {
            XCTAssertThrowsError(try sheet.read(), label) { XCTAssertEqual($0 as? PlanError, .unbound, label) }
        }
        try sheet.bind(plan)
        XCTAssertTrue(sheet.isBound, label)
        XCTAssertEqual(sheet.plan, plan, label)
        XCTAssertThrowsError(try sheet.bind(plan), label) { XCTAssertEqual($0 as? PlanError, .alreadyBound, label) }
    }

    private func replay(_ label: String, _ vector: WorkbookCase, _ sheet: Sheet, batchRows: Int) {
        XCTAssertEqual(sheet.header.map(Array.init), vector.header, "\(label): header")
        let rows = vector.rows!
        var seen = 0
        var failure: TabularError?
        do {
            while let batch = try sheet.read() {
                XCTAssertTrue((1...batchRows).contains(batch.rows), label)
                for row in 0..<batch.rows {
                    guard seen < rows.count else {
                        XCTFail("\(label): more rows than the corpus lists")
                        return
                    }
                    XCTAssertEqual(batch.line(row), vector.numbers![seen], "\(label), row \(seen)")
                    for column in rows[seen].indices {
                        assertCell("\(label), row \(seen), column \(column)", batch, column, row, rows[seen][column])
                    }
                    seen += 1
                }
            }
        } catch let error as TabularError {
            failure = error
            // A failed sheet stays failed: the same failure, again.
            XCTAssertThrowsError(try sheet.read(), label) { XCTAssertEqual($0 as? TabularError, error, label) }
        } catch {
            XCTFail("\(label): \(error)")
        }
        XCTAssertEqual(seen, rows.count, "\(label): rows delivered")
        XCTAssertEqual(failure, expectedFailure(vector.failure), "\(label): structural failure")
    }

    /// Every package awaited from an asynchronous sequence that suspends between short reads:
    /// the same workbook, or the same refusal, as from memory.
    func testWorkbookCorpusFromAnAsyncSequence() async throws {
        for vector in try workbookCorpus() {
            let path = Self.corpusDirectory!.appendingPathComponent(vector.file).path
            let bytes = Array(try Data(contentsOf: URL(fileURLWithPath: path)))
            let stream = ShortReads(bytes, chunk: 4093)
            guard let index = vector.sheet else {
                do {
                    _ = try await Workbook(bytes: stream)
                    XCTFail("\(vector.name): opened")
                } catch {
                    XCTAssertEqual(error as? TabularError, expectedFailure(vector.failure), vector.name)
                }
                continue
            }
            let book = try await Workbook(bytes: stream)
            XCTAssertEqual(book.sheets.map(\.name), vector.sheets!.map(\.name), vector.name)
            let options = SheetOptions(
                hasHeader: vector.options!.has_header, skipEmptyRows: vector.options!.skip_empty_rows, batchRows: 2)
            let plan = vector.plan!.map(column(of:))
            let sheet = try book.sheet(index, options: options)
            try bindHeaderFirst("\(vector.name): async", sheet, plan)
            replay("\(vector.name): async", vector, sheet, batchRows: 2)
        }
    }

    /// A workbook read from a sheet a row at a time, across batches, is the batches read down.
    func testASheetIsReadARowAtATime() throws {
        guard let directory = Self.corpusDirectory else {
            throw XCTSkip("the conformance corpus is not reachable from a WASI sandbox")
        }
        let book = try Workbook(contentsOfFile: directory.appendingPathComponent("workbook/basic.xlsx").path)
        let options = SheetOptions(batchRows: 1)
        let sheet = try book.sheet(0, options: options)
        let header = try XCTUnwrap(sheet.header)
        try sheet.bind([.text(header.ordinal(of: header[0]))])
        var lines = [Int]()
        var cells = [String]()
        try sheet.forEachRow { row in
            lines.append(row.line)
            cells.append(String(decoding: row.raw(0), as: UTF8.self))
        }

        let planned = try book.sheet(0, options: options, plan: [.text(0)])
        var batches = 0
        var expected = [(Int, String)]()
        while let batch = try planned.read() {
            batches += 1
            for row in 0..<batch.rows {
                expected.append((batch.line(row), String(decoding: batch.raw(0, row: row), as: UTF8.self)))
            }
        }
        XCTAssertGreaterThanOrEqual(batches, 3)
        XCTAssertEqual(lines, expected.map(\.0))
        XCTAssertEqual(cells, expected.map(\.1))
        XCTAssertNil(try sheet.read(), "forEachRow read to the end")
    }

    func testWhatIsNotThereIsAnErrorOfItsOwn() throws {
        guard let directory = Self.corpusDirectory else {
            throw XCTSkip("the conformance corpus is not reachable from a WASI sandbox")
        }
        let book = try Workbook(contentsOfFile: directory.appendingPathComponent("workbook/basic.xlsx").path)
        let plan: [Column] = [.text(0)]
        XCTAssertThrowsError(try book.sheet(named: "No such sheet", plan: plan)) {
            XCTAssertEqual($0 as? NoSuchSheet, NoSuchSheet(which: "named \"No such sheet\""))
        }
        XCTAssertThrowsError(try book.sheet(99, plan: plan)) { XCTAssertTrue($0 is NoSuchSheet) }
        XCTAssertThrowsError(try Workbook(contentsOfFile: directory.appendingPathComponent("no-such-file.xlsx").path))
        XCTAssertThrowsError(try Workbook(bytes: Array("not a zip at all".utf8))) {
            XCTAssertEqual(($0 as? TabularError)?.kind, .notAZip)
        }

        // Two sheets read at once, each with its own buffers, over one workbook — which they
        // keep alive between them.
        var first: Sheet? = try book.sheet(0, plan: plan)
        let second = try book.sheet(0, options: SheetOptions(hasHeader: false), plan: plan)
        let headed = try first!.read()!.rows
        first = nil
        XCTAssertEqual(try second.read()!.rows, headed + 1, "the header is one row more")
        XCTAssertNil(try second.read())
    }
}
