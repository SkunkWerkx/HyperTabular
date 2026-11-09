import Foundation
import HyperTabularCore
import XCTest

@testable import HyperTabular

/// What the corpus cannot say of the declarations a read takes and of what it gives back: the errors,
/// the limits, and what is left unsaid in a case's JSON.
final class OptionsTests: XCTestCase {
    private func utf8(_ text: String) -> [UInt8] { Array(text.utf8) }

    func testTokensAreComparable() {
        XCTAssertNotEqual(Column.text(0).withTokens(["NA", "-"]), Column.text(0).withTokens(["-", "NA"]))
        // A token's length is its own: these two would be one string if they were joined.
        XCTAssertNotEqual(packedTokens(["ab", "c"]), packedTokens(["a", "bc"]))
        // A column that opts out of its dialect's tokens is not one that inherits them.
        XCTAssertNotEqual(Column.text(0).withTokens([]), Column.text(0))
        XCTAssertEqual(Column.text(0).withTokens(["NA"]), Column.text(0).withTokens(["NA"]))
        XCTAssertNotEqual(Dialect(separator: ",", tokens: ["NA"]), Dialect.csv)
        XCTAssertEqual(Dialect(separator: ",", tokens: ["NA"]), Dialect(separator: ",", tokens: ["NA"]))
    }

    /// A column's list wins over the dialect's, an empty one takes it off, and neither reaches a column
    /// that says nothing.
    func testAColumnsTokensReplaceItsDialects() throws {
        let reader = try DelimitedReader(
            bytes: utf8("a,b,c\nNA,NA,NA\nx,x,x\n"), dialect: Dialect(separator: ",", tokens: ["NA"]),
            plan: [.text(0), Column.text(1).withTokens([]), Column.text(2).withTokens(["x"])])
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.origin(0, row: 0), .token)
        XCTAssertEqual(batch.origin(1, row: 0), .cell)
        XCTAssertEqual(batch.string(1, row: 0), "NA")
        XCTAssertEqual(batch.origin(2, row: 0), .cell)
        XCTAssertEqual(batch.origin(2, row: 1), .token)
        XCTAssertEqual(batch.verdict(2, row: 1).reason, .empty)
    }

    private func utf16(_ text: String, lone: Bool) -> [UInt8] {
        var bytes = Array(text.utf16).flatMap { [UInt8($0 & 0xFF), UInt8($0 >> 8)] }
        // A high surrogate, and nothing to pair it with.
        if lone { bytes += [0x00, 0xD8] }
        return bytes
    }

    private func readAll(_ reader: DelimitedReader) -> (rows: [String], ended: (any Error)?) {
        var rows = [String]()
        do {
            while let batch = try reader.read() {
                rows += (0..<batch.rows).map { batch.string(0, row: $0) ?? "" }
            }
        } catch {
            return (rows, error)
        }
        return (rows, nil)
    }

    private func chunks(_ bytes: [UInt8], thenThrow failure: (any Error)? = nil) -> DelimitedReader.Source {
        var position = 0
        return { buffer in
            let count = min(buffer.count, bytes.count - position)
            if count == 0, !buffer.isEmpty, let failure { throw failure }
            buffer.copyBytes(from: bytes[position..<position + count])
            position += count
            return count
        }
    }

    /// A \r the fill is holding when the encoding fails ends its row, and the row comes first.
    func testAnEncodingFailureComesAfterEveryIntactRow() async throws {
        let table: [(String, String, Int64, Int, Int64)] = [
            ("a\nb\n", "a,b", 2, 3, 8), ("a\nb\r", "a,b", 2, 3, 8), ("a\nb", "a", 1, 2, 6), ("a\n\"b\r", "a", 1, 2, 10),
        ]
        let dialect = Dialect(separator: ",", hasHeader: false, encoding: .utf16LE)
        for (text, rows, record, line, at) in table {
            let expected = TabularError(kind: .encoding, record: record, line: line, byte: at)
            func holds(_ reader: DelimitedReader, _ label: String) {
                let (read, ended) = readAll(reader)
                XCTAssertEqual(read.joined(separator: ","), rows, label)
                XCTAssertEqual(ended as? TabularError, expected, label)
                XCTAssertEqual(
                    ended?.localizedDescription,
                    "Record \(record) (line \(line)) holds text with no character in its encoding, at byte \(at).",
                    label)
                XCTAssertThrowsError(try reader.read(), label) { XCTAssertEqual($0 as? TabularError, expected, label) }
            }
            let input = utf16(text, lone: true)
            for buffer in [1, 7, DelimitedReader.defaultBufferBytes] {
                holds(
                    try DelimitedReader(
                        reading: chunks(input), dialect: dialect, plan: [.text(0)], bufferBytes: buffer),
                    "\(text.debugDescription), \(buffer) bytes")
            }
            holds(
                try DelimitedReader(bytes: input, dialect: dialect, plan: [.text(0)]),
                "\(text.debugDescription), memory")
            try input.withUnsafeBytes {
                holds(
                    try DelimitedReader(bytesNoCopy: $0, dialect: dialect, plan: [.text(0)]),
                    "\(text.debugDescription), in place")
            }
            let awaited = try await DelimitedReader(
                bytes: CorpusTests.ShortReads(input, chunk: 3), dialect: dialect, plan: [.text(0)])
            var seen = [String]()
            do {
                while let batch = try await awaited.readAsync() {
                    seen += (0..<batch.rows).map { batch.string(0, row: $0) ?? "" }
                }
                XCTFail("\(text.debugDescription), async: no failure")
            } catch {
                XCTAssertEqual(error as? TabularError, expected, "\(text.debugDescription), async")
            }
            XCTAssertEqual(seen.joined(separator: ","), rows, "\(text.debugDescription), async")
        }
    }

    private struct Cut: Error, Equatable {}

    /// Bytes a source gave and then a failure: the intact rows in them come out, and the source's failure
    /// is what the read ends on, undisguised.
    func testADecodedSourceFailsAsAnyOtherDoes() async throws {
        for encoding in [TextEncoding.utf8, .latin1] {
            let dialect = Dialect(separator: ",", encoding: encoding)
            let reader = try DelimitedReader(
                reading: chunks(utf8("a\n1\n2\n"), thenThrow: Cut()), dialect: dialect, plan: [.text(0)])
            let (rows, ended) = readAll(reader)
            XCTAssertEqual(rows, ["1", "2"])
            XCTAssertEqual(ended as? Cut, Cut())

            let input = utf8("a\n1\n2\n")
            let awaited = try await DelimitedReader(
                bytes: CorpusTests.ShortReads(input, chunk: 7, failAt: input.count), dialect: dialect, plan: [.text(0)])
            var seen = [String]()
            do {
                while let batch = try await awaited.readAsync() {
                    seen += (0..<batch.rows).map { batch.string(0, row: $0) ?? "" }
                }
                XCTFail("no failure")
            } catch {
                XCTAssertTrue(error is CancellationError, "\(error)")
            }
            XCTAssertEqual(seen, ["1", "2"])
        }
    }

    /// A source is asked for more only when the fill wants it, once a time: a live source that has
    /// delivered a whole row has that row read before it is asked again.
    func testASourceIsNotAskedPastACompleteRow() throws {
        for encoding in [TextEncoding.utf8, .latin1, .windows1252] {
            let input = utf8("a\n1\n")
            var asked = 0
            let source: DelimitedReader.Source = { buffer in
                asked += 1
                guard asked == 1 else { return 0 }
                buffer.copyBytes(from: input)
                return input.count
            }
            let reader = try DelimitedReader(
                reading: source, dialect: Dialect(separator: ",", encoding: encoding), plan: [.text(0)])
            XCTAssertEqual(reader.header, ["a"], "\(encoding)")
            XCTAssertEqual(asked, 1, "\(encoding): the header waited")
            let batch = try XCTUnwrap(reader.read(), "\(encoding)")
            XCTAssertEqual(batch.rows, 1, "\(encoding)")
            XCTAssertEqual(asked, 1, "\(encoding): the row waited")
        }
    }

    /// Text in memory under an encoding goes through a buffer of its own, and the ceiling holds there too.
    func testARecordPastTheCeilingIsAFailure() async throws {
        // Longer than the buffer a reader of memory starts with, as well as a stream's one byte.
        let input = [UInt8](repeating: UInt8(ascii: "x"), count: DelimitedReader.defaultBufferBytes + 64) + [0x0A]
        DelimitedReader.rowCeiling = 8
        defer { DelimitedReader.rowCeiling = 0 }
        for encoding in [TextEncoding.utf8, .latin1] {
            let dialect = Dialect(separator: ",", hasHeader: false, encoding: encoding)
            let stream = try DelimitedReader(reading: chunks(input), dialect: dialect, plan: [.text(0)], bufferBytes: 1)
            XCTAssertThrowsError(try stream.read()) { XCTAssertEqual(($0 as? TabularError)?.kind, .rowTooLong) }
            if encoding != .utf8 {
                let memory = try DelimitedReader(bytes: input, dialect: dialect, plan: [.text(0)])
                XCTAssertThrowsError(try memory.read()) { XCTAssertEqual(($0 as? TabularError)?.kind, .rowTooLong) }
            }
            let waited = try await DelimitedReader(
                bytes: CorpusTests.ShortReads(input, chunk: 4096), dialect: dialect, plan: [.text(0)], bufferBytes: 1)
            do {
                _ = try await waited.readAsync()
                XCTFail("no failure")
            } catch {
                XCTAssertEqual((error as? TabularError)?.kind, .rowTooLong)
            }
        }
    }

    /// The ceiling itself, the one Rust's MAX_ROW_BYTES says, either side of it: a host with less than
    /// 12 GiB skips it out loud (3.25 GB resident and 1.8 s here, on an Apple M6).
    func testARecordIsReadUpToTheRealCeilingAndNoFurther() throws {
        let available = ProcessInfo.processInfo.physicalMemory
        guard available >= 12 << 30 else {
            throw XCTSkip("The real ceiling needs 12 GiB; this host has \(available) bytes.")
        }
        XCTAssertEqual(DelimitedReader.maxRowBytes, 1 << 30)
        func oneLongRecord(_ bytes: Int) -> DelimitedReader.Source {
            var left = bytes
            return { buffer in
                guard left >= 0, !buffer.isEmpty else { return 0 }
                guard left > 0 else {
                    left = -1
                    buffer[0] = 0x0A
                    return 1
                }
                let written = min(buffer.count, left)
                buffer[..<written].initializeMemory(as: UInt8.self, repeating: UInt8(ascii: "x"))
                left -= written
                return written
            }
        }
        let dialect = Dialect(separator: ",", hasHeader: false)
        let under = try DelimitedReader(
            reading: oneLongRecord(DelimitedReader.maxRowBytes - 8), dialect: dialect, plan: [.text(0)])
        XCTAssertEqual(try under.read()?.rows, 1)
        let past = try DelimitedReader(
            reading: oneLongRecord(DelimitedReader.maxRowBytes + 8), dialect: dialect, plan: [.text(0)])
        XCTAssertThrowsError(try past.read()) { XCTAssertEqual(($0 as? TabularError)?.kind, .rowTooLong) }
    }

    func testARangeIsWhatTheCoreSaysItIs() throws {
        let range = try SheetRange(parsing: "B4:E12")
        XCTAssertEqual(range.firstRow, 4)
        XCTAssertEqual(range.firstColumn, 2)
        XCTAssertEqual(range.lastRow, 12)
        XCTAssertEqual(range.lastColumn, 5)
        XCTAssertEqual(try SheetRange(firstRow: 4, firstColumn: 2, lastRow: 12, lastColumn: 5), range)
        XCTAssertEqual(range.description, "rows 4 to 12, columns 2 to 5")
        XCTAssertEqual(try SheetRange(firstRow: 5).description, "rows 5 to the last, every column")
        for text in ["", "A5", "a5:f200", "F200:A5", "A0:B2", "A1:B", " A1:B2"] {
            XCTAssertThrowsError(try SheetRange(parsing: text), text) { error in
                XCTAssertEqual(error as? RangeError, RangeError(text: text))
                XCTAssertTrue(error.localizedDescription.contains("\"\(text)\""), error.localizedDescription)
            }
        }
        let open = try SheetRange(firstRow: 5)
        XCTAssertEqual(open.firstRow, 5)
        XCTAssertEqual(open.lastRow, 0)
        XCTAssertThrowsError(try SheetRange(firstRow: 5, lastRow: 4))
        XCTAssertThrowsError(try SheetRange(firstColumn: 3, lastColumn: 2))
    }

    func testASheetTakesItsOptions() throws {
        let book = try Workbook(contentsOfFile: try Support.package("ranges.xlsx"))
        let options = SheetOptions(batchRows: 2, range: try SheetRange(parsing: "B4:E12"), notes: false)
        let sheet = try book.sheet(0, options: options, plan: [.text(0)])
        XCTAssertEqual(sheet.options, options)
        XCTAssertEqual(try sheet.read()?.rows, 2)
    }

    /// What a slice and a reversal hold is the rows they were cut to, in the order asked.
    func testASliceAndAReversalAreTheRowsCut() throws {
        let reader = try DelimitedReader(
            bytes: utf8("a\n1\n2\n3\n4\n"), dialect: .csv, plan: [.i32(0)], batchRows: 4)
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.dropFirst(1).map(\.line), [3, 4, 5])
        XCTAssertEqual(batch.dropLast(1).map(\.line), [2, 3, 4])
        XCTAssertEqual(batch.prefix(2).map(\.line), [2, 3])
        XCTAssertEqual(batch.suffix(1).map(\.line), [5])
        XCTAssertEqual(batch[1..<3].map(\.index), [1, 2])
        XCTAssertEqual(batch[1...].dropFirst().map(\.line), [4, 5])
        XCTAssertEqual(batch[1..<3][2].line, 4)
        XCTAssertEqual(batch[1..<3].count, 2)
        XCTAssertEqual(batch[1..<3].startIndex, 1)
        XCTAssertEqual(batch[1..<1].count, 0)
        XCTAssertEqual(batch.reversed().map(\.line), [5, 4, 3, 2])
        XCTAssertEqual(batch[1..<3].reversed().map(\.line), [4, 3])
        // A walk goes by the iterator, where `map` goes by index.
        var walked = [Int]()
        for row in batch.dropLast(1).dropFirst() { walked.append(row.line) }
        XCTAssertEqual(walked, [3, 4])
        XCTAssertEqual(batch.split { $0.line == 3 }.map { $0.map(\.line) }, [[2], [4, 5]])
    }

    /// A delimited batch that outlives its reader, from each place its text can sit, stays what was read.
    func testABatchOutlivesItsReader() throws {
        weak var gone: DelimitedReader?
        func kept(_ open: () throws -> DelimitedReader) throws -> Batch {
            let reader = try open()
            gone = reader
            return try XCTUnwrap(reader.read())
        }
        func keptRow(_ open: () throws -> DelimitedReader) throws -> Row {
            let reader = try open()
            gone = reader
            return try XCTUnwrap(reader.read())[1]
        }
        let plan: [Column] = [.text(0), .i32(1)]
        let text = utf8("name,n\nalice,1\n\"say \"\"hi\"\"\",2x\n")
        let path = NSTemporaryDirectory() + "hypertabular-outlives-\(UUID().uuidString).csv"
        XCTAssertTrue(FileManager.default.createFile(atPath: path, contents: Data(text)))
        defer { try? FileManager.default.removeItem(atPath: path) }
        var position = 0
        var latin1 = Dialect.csv
        latin1.encoding = .latin1
        let opens: [(String, () throws -> DelimitedReader)] = [
            ("bytes", { try DelimitedReader(bytes: text, dialect: .csv, plan: plan) }),
            ("file", { try DelimitedReader(contentsOfFile: path, dialect: .csv, plan: plan) }),
            (
                "source",
                {
                    position = 0
                    return try DelimitedReader(
                        reading: { buffer in
                            let count = min(buffer.count, text.count - position)
                            buffer.copyBytes(from: text[position..<position + count])
                            position += count
                            return count
                        }, dialect: .csv, plan: plan)
                }
            ),
            ("decoded", { try DelimitedReader(bytes: text, dialect: latin1, plan: plan) }),
        ]
        for (name, open) in opens {
            let batch = try kept(open)
            XCTAssertNil(gone, name)
            Self.churn()
            XCTAssertEqual(batch.rows, 2, name)
            XCTAssertEqual(batch.line(1), 3, name)
            XCTAssertEqual(batch.string(0, row: 0), "alice", name)
            XCTAssertEqual(batch.string(0, row: 1), "say \"hi\"", name)
            XCTAssertEqual(batch.get(1, row: 0, as: Int32.self), .success(1), name)
            XCTAssertEqual(batch.verdicts(1).filter { !$0.isOk }.count, 1, name)
            XCTAssertEqual(batch.faults().map(\.line), [3], name)
            XCTAssertEqual(batch.map(\.line), [2, 3], name)
            XCTAssertEqual(batch.reversed().map { $0.string(0) }, ["say \"hi\"", "alice"], name)

            let row = try keptRow(open)
            XCTAssertNil(gone, name)
            Self.churn()
            XCTAssertEqual(row.line, 3, name)
            XCTAssertEqual(row.string(0), "say \"hi\"", name)
            XCTAssertEqual(row.fault(1)?.reason, .malformed, name)
        }
    }

    /// A sheet's batch reaches into its sheet's buffers and its workbook's strings, and keeps them.
    func testABatchOutlivesItsSheetAndItsWorkbook() throws {
        for package in ["ranges.xlsx", "ranges.ods", "ranges.numbers"] {
            let file = try Support.package(package)
            let plan: [Column] = [.text(0), .text(1), .text(2)]
            let options = SheetOptions(range: try SheetRange(parsing: "B4:D12"))
            let live = try Workbook(contentsOfFile: file).sheet(0, options: options, plan: plan)
            let liveBatch = try XCTUnwrap(live.read())
            let texts = (0..<liveBatch.rows).map { row in (0..<3).map { liveBatch.string($0, row: row) } }
            let lines = liveBatch.map(\.line)
            XCTAssertFalse(texts.isEmpty, package)

            weak var goneSheet: Sheet?
            weak var goneBook: Workbook?
            func open() throws -> Sheet {
                let book = try Workbook(contentsOfFile: file)
                goneBook = book
                let sheet = try book.sheet(0, options: options, plan: plan)
                goneSheet = sheet
                return sheet
            }
            let batch = try { try XCTUnwrap(try open().read()) }()
            XCTAssertNil(goneSheet, package)
            XCTAssertNotNil(goneBook, "the batch holds what it reaches: \(package)")
            Self.churn()
            XCTAssertEqual((0..<batch.rows).map { row in (0..<3).map { batch.string($0, row: row) } }, texts, package)
            XCTAssertEqual(batch.map(\.line), lines, package)

            var row: Row? = try { try XCTUnwrap(try open().read())[0] }()
            XCTAssertNil(goneSheet, package)
            XCTAssertNotNil(goneBook, package)
            Self.churn()
            XCTAssertEqual((0..<3).map { row?.string($0) }, texts[0], package)
            // And lets it go with the last thing that read it.
            row = nil
            XCTAssertNil(goneBook, package)
        }
    }

    /// Takes the freed blocks back, so that memory read after it was freed doesn't still say what it said.
    private static func churn() {
        for size in [16, 64, 256, 1024, 4096, 1 << 16, 1 << 20] {
            for _ in 0..<64 {
                let block = UnsafeMutableRawBufferPointer.allocate(byteCount: size, alignment: 8)
                block.initializeMemory(as: UInt8.self, repeating: 0xA5)
                block.deallocate()
            }
        }
    }

    /// Notes off is what a read asks the core for: no word on a refused cell, where the default has one,
    /// delimited and sheet.
    func testNotesOffAsksForNoNotes() throws {
        for notes in [true, false] {
            let reader = try DelimitedReader(
                bytes: utf8("a\n1x\n"), dialect: Dialect(separator: ",", notes: notes), plan: [.i32(0)])
            let batch = try XCTUnwrap(reader.read())
            XCTAssertEqual(batch.verdict(0, row: 0).reason, .malformed)
            XCTAssertEqual(try XCTUnwrap(batch.fault(0, row: 0)).detailCode != 0, notes)
            XCTAssertEqual(batch.note(0, 0).detail != 0, notes)
        }
        XCTAssertTrue(Dialect.csv.notes)
        XCTAssertTrue(SheetOptions.default.notes)
        XCTAssertTrue(SheetOptions(hasHeader: false).notes)
    }

    /// Fault detail reaches every door the verdict does: the union's, the row's and the batch's.
    func testAFaultCarriesItsDetailEverywhereItIsGiven() throws {
        let reader = try DelimitedReader(bytes: utf8("a\n\"1,23,456\"\n"), dialect: .csv, plan: [.i32(0)])
        let batch = try XCTUnwrap(reader.read())
        guard case .fault(let fault) = batch.get(0, row: 0, as: Int32.self) else {
            return XCTFail("no fault")
        }
        XCTAssertEqual(fault.detail, .groupSize)
        XCTAssertEqual(batch.fault(0, row: 0)?.detail, .groupSize)
        XCTAssertEqual(batch[0].fault(0)?.detail, .groupSize)
        if case .fault(let across) = batch[0].get(0, as: Int32.self) {
            XCTAssertEqual(across.detail, .groupSize)
        } else {
            XCTFail("no fault across the row")
        }
        XCTAssertEqual(batch.faults().map(\.detail), [.groupSize])
        // HyperCast's verdict has no room for one, and a fault's equality leaves it out.
        XCTAssertEqual(batch.verdict(0, row: 0).fault?.detailCode, 0)
        XCTAssertEqual(batch.verdict(0, row: 0).fault, fault)
    }

    /// A batch's list can be longer than a call gives back, and asked again it starts again.
    func testABatchsFaultsAreAsLongAsTheyAre() throws {
        let rows = 3 * 64 + 1
        let input = "a,b\n" + String(repeating: "x,q\n", count: rows)
        let reader = try DelimitedReader(bytes: utf8(input), dialect: .csv, plan: [.i32(0), .text(1), .bool(1)])
        let batch = try XCTUnwrap(reader.read())
        var listed = 0
        for problem in batch.faults() {
            let column = listed % 2 == 1 ? 2 : 0
            XCTAssertEqual(problem.row, listed / 2)
            XCTAssertEqual(problem.planColumn, column)
            XCTAssertEqual(problem.sourceColumn, column / 2)
            XCTAssertEqual(problem.reason, .malformed)
            XCTAssertEqual(problem.line, listed / 2 + 2)
            XCTAssertNotEqual(problem.detailCode, 0)
            listed += 1
        }
        XCTAssertEqual(listed, 2 * rows)
        XCTAssertEqual(batch.faults().first { _ in true }?.row, 0)
        XCTAssertEqual(batch.faults().reduce(0) { count, _ in count + 1 }, 2 * rows)
    }

    /// A range's last column ends the table: the header stops there, where the corpus's ranges run
    /// to the sheet's own edge and can't tell.
    func testARangesLastColumnEndsTheTable() throws {
        let book = try Workbook(contentsOfFile: try Support.package("ranges.xlsx"))
        let narrow = try book.sheet(0, options: SheetOptions(range: try SheetRange(parsing: "B4:C12")))
        XCTAssertEqual(narrow.header?.count, 2)
        let wide = try book.sheet(0, options: SheetOptions(range: try SheetRange(parsing: "B4:E12")))
        XCTAssertEqual(wide.header?.count, 4)
        XCTAssertEqual(narrow.header.map(Array.init), wide.header.map { Array($0.prefix(2)) })
    }

    /// A long row's entry says the width it had, and a range's source column is the sheet's own.
    func testAProblemNamesWhatItIsAbout() throws {
        let reader = try DelimitedReader(
            bytes: utf8("a,b\n1,2,3\nx,4\n"), dialect: Dialect(separator: ",", longRows: .report), plan: [.i32(0)])
        let batch = try XCTUnwrap(reader.read())
        let listed = Array(batch.faults())
        XCTAssertEqual(listed.count, 2)
        XCTAssertNil(listed[0].planColumn)
        XCTAssertEqual(listed[0].expected, 2)
        XCTAssertEqual(listed[0].found, 3)
        XCTAssertEqual(listed[0].line, 2)
        XCTAssertEqual(listed[1].planColumn, 0)

        let book = try Workbook(contentsOfFile: try Support.package("ranges.xlsx"))
        let sheet = try book.sheet(
            0, options: SheetOptions(range: try SheetRange(parsing: "B4:E12")), plan: [.i32(1), .i32(0)])
        let ranged = try XCTUnwrap(sheet.read())
        let problem = try XCTUnwrap(ranged.faults().first { $0.planColumn == 1 })
        XCTAssertEqual(problem.sourceColumn, 1, "B, not the range's first")
    }

    /// Every door code there could be, 0 and past the last included, with a param beside it: a report and
    /// its plan say a door that exists and a param it declares, and only a resolved one is read through.
    func testEveryDoorTheCoreCanNameIsAColumn() throws {
        func raw(door: UInt32, param: UInt32, status: UInt32) -> HyperTabularRawSuggestion {
            var raw = HyperTabularRawSuggestion()
            raw.door = door
            raw.param = param
            raw.status = status
            raw.matched = 1
            raw.sampled = 1
            return raw
        }
        for code in UInt32(0)...23 {
            let door = Door(rawValue: code)
            for param: UInt32 in [0, 1, 2, 3, 99] {
                let report = Sample.report(0, raw(door: code, param: param, status: 1), nil, nil)
                guard let door else {
                    XCTAssertEqual(report.door, .text, "door \(code)")
                    continue
                }
                let planned = Sample.plan(report)
                let read = try DelimitedReader(
                    bytes: utf8("1\n"), dialect: Dialect(separator: ",", hasHeader: false), plan: [planned])
                XCTAssertEqual(try read.read()?.columns[0], planned, "door \(door), param \(param)")
                let valid: Bool =
                    switch door {
                    case .unix: UnixPrecision(rawValue: param) != nil
                    case .excelSerial: ExcelEpoch(rawValue: param) != nil
                    case .dateOrdered, .dateTime: DateOrder(rawValue: param) != nil
                    default: true
                    }
                XCTAssertEqual(report.door, valid ? door : .text, "door \(door), param \(param)")
                let declares = door == .unix || door == .excelSerial || door == .dateOrdered || door == .dateTime
                XCTAssertEqual(planned.declared, declares && valid ? param : 0, "door \(door), param \(param)")
            }
            // Ambiguous and unresolved read as text, whatever the door.
            for status: UInt32 in [2, 3, 0, 9] {
                let planned = Sample.plan(Sample.report(0, raw(door: code, param: 1 << 2, status: status), nil, nil))
                XCTAssertEqual(planned.door, .text, "door \(code), status \(status)")
                XCTAssertEqual(planned.ordinal, 0, "door \(code), status \(status)")
            }
        }
    }

    /// The notations the suggest export names by number, as this binding declares them: each Western.
    func testAProvedNotationIsWestern() throws {
        XCTAssertEqual(Interop.rawFormat(.invariant).1, 95)
        XCTAssertEqual(Interop.rawFormat(Support.commaDecimal).1, 95)
        let commaGroups: UInt32 = 1 << 1
        let commaDecimal: UInt32 = 1 << 2
        func report(_ door: Door, _ param: UInt32, _ status: UInt32, _ declared: NumFormat? = nil) -> SuggestedColumn {
            var raw = HyperTabularRawSuggestion()
            raw.door = door.rawValue
            raw.param = param
            raw.status = status
            return Sample.report(0, raw, nil, declared)
        }

        // Proved by a , or not, the plan is the same: a later 1,23,456 is refused.
        for param in [0, commaGroups] {
            let found = report(.i32, param, 1)
            XCTAssertEqual(found.format, .invariant)
            XCTAssertEqual(found.param, 0)
            let reader = try DelimitedReader(
                bytes: utf8("a\n\"1,234\"\n\"1,23,456\"\n"), dialect: .csv, plan: [Sample.plan(found)])
            let batch = try XCTUnwrap(reader.read())
            XCTAssertEqual(batch.values(0, as: Int32.self)[0], 1234)
            XCTAssertEqual(batch.verdict(0, row: 1).reason, .malformed)
        }
        // What only the comma decimal reads is planned in it, and a later group it can't read is refused.
        let decimals = report(.f64, commaDecimal, 1)
        XCTAssertEqual(decimals.format, Support.commaDecimal)
        XCTAssertEqual(decimals.param, 0)
        XCTAssertEqual(decimals.candidates, [])
        let reader = try DelimitedReader(
            bytes: utf8("a\n\"0,125\"\n\"1.234,5\"\n\"1,234,567\"\n"), dialect: .csv, plan: [Sample.plan(decimals)])
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.values(0, as: Double.self)[0], 0.125)
        XCTAssertEqual(batch.values(0, as: Double.self)[1], 1234.5)
        XCTAssertEqual(batch.verdict(0, row: 2).reason, .malformed)
        // A date's param of 2 is its order, no notation, and it names no format.
        XCTAssertNil(report(.dateOrdered, 2, 1).format)
        // A column's declaration wins, and the report names it, whatever param says.
        for param in [commaGroups, commaDecimal] {
            XCTAssertEqual(report(.i32, param, 1, .invariant).format, .invariant)
        }
        let declared = report(.i32, commaGroups, 1, Support.commaDecimal)
        XCTAssertEqual(declared.format, Support.commaDecimal)
        XCTAssertEqual(Sample.plan(declared).format, Support.commaDecimal)
        // A column that isn't a resolved number names no format.
        XCTAssertNil(report(.text, 0, 1).format)
        XCTAssertNil(report(.i32, commaGroups | commaDecimal, 2).format)
        XCTAssertNil(report(.i32, 0, 3).format)
        // Ambiguous numbers offer both notations, and ambiguous dates, either date door, the orders.
        XCTAssertEqual(
            report(.i32, commaGroups | commaDecimal, 2).candidates,
            [.numFormat(.invariant), .numFormat(Support.commaDecimal)])
        for door in [Door.dateOrdered, .dateTime] {
            let open = report(door, 1 << 2 | 1 << 3, 2)
            XCTAssertEqual(open.door, door)
            XCTAssertEqual(open.param, 2)
            XCTAssertEqual(open.candidates, [.dateOrder(.monthDayYear), .dateOrder(.dayMonthYear)])
        }
    }

    /// The core decides how much a batch may ask for, and every way in asks it before allocating.
    /// These sizes are just over its limit, so that a regression asks for little more than the limit.
    func testTheCoreJudgesABatchsSize() throws {
        let input = utf8("a\n1\n")
        let limit: UInt64 = 4_294_967_296
        let over = BatchSizeError.tooLarge(needed: 5_377_130_496, limit: limit)
        XCTAssertThrowsError(try DelimitedReader(bytes: input, dialect: .csv, plan: [.i32(0)], batchRows: 1 << 28)) {
            XCTAssertEqual($0 as? BatchSizeError, over)
        }
        XCTAssertEqual(over.description, "batchRows asks for 5377130496 bytes, past the limit of 4294967296")
        let late = try DelimitedReader(bytes: input, dialect: .csv, batchRows: 1 << 28)
        XCTAssertThrowsError(try late.bind([.i32(0)])) { XCTAssertEqual($0 as? BatchSizeError, over) }
        XCTAssertFalse(late.isBound)

        let book = try Workbook(contentsOfFile: try Support.package("basic.xlsx"))
        let large = SheetOptions(batchRows: 1 << 27)
        let sheetOver = BatchSizeError.tooLarge(needed: 4_831_838_224, limit: limit)
        XCTAssertThrowsError(try book.sheet(0, options: large, plan: [.i32(0)])) {
            XCTAssertEqual($0 as? BatchSizeError, sheetOver)
        }
        let unplanned = try book.sheet(0, options: large)
        XCTAssertThrowsError(try unplanned.bind([.i32(0)])) { XCTAssertEqual($0 as? BatchSizeError, sheetOver) }
        XCTAssertFalse(unplanned.isBound)

        let modest = try DelimitedReader(bytes: input, dialect: .csv, plan: [.i32(0)], batchRows: 1 << 16)
        XCTAssertEqual(try modest.read()?.rows, 1)
    }

    /// A path's sizes are judged before the file is touched, so a bad one beats a missing file.
    func testTheCoreJudgesAPathsSizesBeforeTheFileIsOpened() throws {
        let nowhere = "no such file"
        XCTAssertThrowsError(try DelimitedReader(contentsOfFile: nowhere, dialect: .csv)) {
            XCTAssertNil($0 as? BatchSizeError)
        }
        XCTAssertThrowsError(try DelimitedReader(contentsOfFile: nowhere, dialect: .csv, bufferBytes: 0)) {
            XCTAssertEqual($0 as? BatchSizeError, .zeroBufferBytes)
        }
        XCTAssertThrowsError(try DelimitedReader(contentsOfFile: nowhere, dialect: .csv, plan: [.i32(0)], batchRows: 0))
        {
            XCTAssertEqual($0 as? BatchSizeError, .zeroBatchRows)
        }
        XCTAssertThrowsError(
            try DelimitedReader(contentsOfFile: nowhere, dialect: .csv, plan: [.i32(0)], batchRows: 1 << 28)
        ) {
            XCTAssertEqual($0 as? BatchSizeError, .tooLarge(needed: 5_377_130_496, limit: 4_294_967_296))
        }
    }

    /// A sampler's batch is never why a suggestion is refused: nobody set its size.
    func testTheCoreIsNotAskedToRefuseASamplersOwnBatch() throws {
        let width = 268_000
        let names = (0..<width).map { "c\($0)" }.joined(separator: ",")
        let row = [String](repeating: "1", count: width).joined(separator: ",")
        let wide = try DelimitedReader.suggest(bytes: utf8("\(names)\n\(row)\n\(row)\n"), dialect: .csv)
        XCTAssertEqual(wide.report.count, width)
        XCTAssertEqual(wide.report[width - 1].sampled, 2)
        XCTAssertFalse(wide.truncated)
        // At exactly the limit with no plan, and a sample past every batch.
        let book = try Workbook(contentsOfFile: try Support.package("basic.xlsx"))
        let atTheLimit = try book.sheet(0, options: SheetOptions(batchRows: 1 << 29))
        let large = try atTheLimit.suggest(SuggestOptions(sampleRows: 1 << 40))
        let modest = try book.sheet(0, options: SheetOptions()).suggest(SuggestOptions(sampleRows: 4096))
        XCTAssertEqual(Self.words(large), Self.words(modest))
    }

    /// A sample says the same in any number of batches, delimited and sheet.
    func testTheCoreSampleSaysTheSameInAnyNumberOfBatches() throws {
        let text = utf8(
            "id,price,when,note\n1,\"1,234.50\",2024-01-02,plain\n2,1.234,2024-02-03,\n3,,2024-03-04,\"a, b\"\n"
                + "x,0.5,03/04/2024,\"two\nlines\"\n5,12,2024-05-06,more\n6,13,,more\n")
        var unnoted = Dialect.csv
        unnoted.notes = false
        for limit in [1, 2, 3, 5, 6, 1000] {
            let options = SuggestOptions(sampleRows: limit)
            let whole = Self.words(try DelimitedReader.suggest(bytes: text, dialect: .csv, options: options))
            for rows in [1, 2, 3, 7] {
                let reader = try DelimitedReader(bytes: text, dialect: unnoted, batchRows: rows)
                let sample = try Sample.of(
                    width: reader.header?.count ?? 0, limit: limit, sheet: false,
                    bind: { try reader.bind($0) }, read: { try reader.read() })
                let found = try sample.judge(header: reader.header, options: options)
                XCTAssertEqual(Self.words(found), whole, "sample \(limit) in batches of \(rows)")
            }
        }
        let book = try Workbook(contentsOfFile: try Support.package("basic.xlsx"))
        for limit in [1, 2, 3, 1000] {
            let options = SuggestOptions(sampleRows: limit)
            let whole = Self.words(try book.sheet(0, options: SheetOptions()).suggest(options))
            for rows in [1, 2, 3] {
                let sheet = try book.sheet(0, options: SheetOptions(batchRows: rows))
                let sample = try Sample.of(
                    width: sheet.header?.count ?? 0, limit: limit, sheet: true,
                    bind: { try sheet.bind($0) }, read: { try sheet.read() })
                let found = try sample.judge(header: sheet.header, options: options)
                XCTAssertEqual(Self.words(found), whole, "sheet sample \(limit) in batches of \(rows)")
            }
        }
        // A control: the sample's size does move the answer, so the comparison above can fail.
        XCTAssertNotEqual(
            Self.words(try DelimitedReader.suggest(bytes: text, dialect: .csv, options: SuggestOptions(sampleRows: 1))),
            Self.words(try DelimitedReader.suggest(bytes: text, dialect: .csv, options: SuggestOptions(sampleRows: 6))))
    }

    /// With no header, a short and a long row, and row widths kept, across each batch boundary.
    func testTheCoreSampleSaysTheSameWithNoHeaderAndRaggedRows() throws {
        let ragged = utf8("1,x\n2,y,long\n3\n4,z\n5,w\n")
        var headerless = Dialect.csv
        headerless.hasHeader = false
        headerless.shortRows = .pad
        headerless.longRows = .report
        var unnoted = headerless
        unnoted.notes = false
        for limit in [1, 2, 3, 4, 5, 6] {
            let options = SuggestOptions(sampleRows: limit)
            let whole = try DelimitedReader.suggest(bytes: ragged, dialect: headerless, options: options)
            XCTAssertEqual(Int(whole.report[0].sampled), min(limit, 5))
            XCTAssertEqual(whole.truncated, limit < 5)
            for rows in [1, 2, 3, 4] {
                let reader = try DelimitedReader(bytes: ragged, dialect: unnoted, batchRows: rows)
                let sample = try Sample.of(
                    width: 2, limit: limit, sheet: false,
                    bind: { try reader.bind($0) }, read: { try reader.read() })
                let found = try sample.judge(header: reader.header, options: options)
                XCTAssertEqual(Self.words(found), Self.words(whole), "sample \(limit) in batches of \(rows)")
            }
        }
    }

    private static func words(_ found: Suggestion) -> String {
        found.report.map { "\($0.ordinal):\($0.door):\($0.status):\($0.matched)/\($0.sampled)" }.joined(separator: ";")
            + ":\(found.truncated)"
    }

    /// One row past the limit at any batch size names its width: no public plan gets there, so this
    /// asks directly.
    func testTheCoreNamesTheWidthOfARowPastTheLimit() throws {
        var far = Column.i32(0).spec
        far.ordinal = UInt32.max
        XCTAssertThrowsError(
            try Columns.checkSizes(specs: [far], batchRows: 1, bufferBytes: 1, notes: false, widths: false, sheet: true)
        ) {
            let refused = BatchSizeError.rowTooWide(width: 4_294_967_296, needed: 68_719_476_768, limit: 4_294_967_296)
            XCTAssertEqual($0 as? BatchSizeError, refused)
            XCTAssertEqual(
                refused.description,
                "a row of 4294967296 columns asks for 68719476768 bytes, past the limit of 4294967296")
        }
    }

    /// An explicit zero is refused by the core, for each option, where an omitted option has its default.
    func testTheCoreRefusesAZeroWhereAnOmittedSizeHasItsDefault() throws {
        let input = utf8("a\n1\n")
        XCTAssertThrowsError(try DelimitedReader(bytes: input, dialect: .csv, plan: [.i32(0)], batchRows: 0)) {
            XCTAssertEqual($0 as? BatchSizeError, .zeroBatchRows)
        }
        XCTAssertEqual(BatchSizeError.zeroBatchRows.description, "batchRows must be at least 1")
        XCTAssertEqual(BatchSizeError.zeroBufferBytes.description, "bufferBytes must be at least 1")
        let path = NSTemporaryDirectory() + "hypertabular-zero-\(UUID().uuidString).csv"
        FileManager.default.createFile(atPath: path, contents: Data(input))
        defer { try? FileManager.default.removeItem(atPath: path) }
        XCTAssertThrowsError(try DelimitedReader(contentsOfFile: path, dialect: .csv, plan: [.i32(0)], bufferBytes: 0))
        {
            XCTAssertEqual($0 as? BatchSizeError, .zeroBufferBytes)
        }
        let book = try Workbook(contentsOfFile: try Support.package("basic.xlsx"))
        XCTAssertThrowsError(try book.sheet(0, options: SheetOptions(batchRows: 0), plan: [.text(0)])) {
            XCTAssertEqual($0 as? BatchSizeError, .zeroBatchRows)
        }
        XCTAssertThrowsError(try book.sheet(0, options: SheetOptions(batchRows: 0))) {
            XCTAssertEqual($0 as? BatchSizeError, .zeroBatchRows)
        }
        XCTAssertEqual(SheetOptions().batchRows, DelimitedReader.defaultBatchRows)
        let rows = utf8("a\n" + String(repeating: "1\n", count: DelimitedReader.defaultBatchRows + 1))
        let omitted = try DelimitedReader(bytes: rows, dialect: .csv, plan: [.i32(0)])
        XCTAssertEqual(try omitted.read()?.rows, DelimitedReader.defaultBatchRows)
        let suggested = try DelimitedReader.suggest(bytes: input, dialect: .csv, options: SuggestOptions(sampleRows: 0))
        XCTAssertEqual(suggested.report[0].sampled, 1)
    }

    func testSuggestTakesItsOptions() throws {
        let text = utf8("a,b\n\"1,234\",x\n\"5,678\",y\n9,z\n")
        // As Rust's sample_rows: 0 is one row, and the row past it says there's more.
        let zero = try DelimitedReader.suggest(
            bytes: utf8("a\n1\n2.5\n"), dialect: .csv, options: SuggestOptions(sampleRows: 0))
        XCTAssertEqual(zero.report[0].door, .bool)
        XCTAssertEqual(zero.report[0].sampled, 1)
        XCTAssertTrue(zero.truncated)
        let whole = try DelimitedReader.suggest(bytes: utf8("a\n1\n2.5\n"), dialect: .csv)
        XCTAssertEqual(whole.report[0].door, .f64)
        XCTAssertEqual(whole.report[0].sampled, 2)
        XCTAssertFalse(whole.truncated)
        let one = try DelimitedReader.suggest(bytes: text, dialect: .csv, options: SuggestOptions(sampleRows: 1))
        XCTAssertTrue(one.truncated)
        XCTAssertEqual(one.report[0].sampled, 1)
        XCTAssertEqual(one.report.map(\.name), ["a", "b"])

        // A notation for one column, ahead of another for all of them.
        let own = try DelimitedReader.suggest(
            bytes: text, dialect: .csv,
            options: SuggestOptions(format: .invariant, columnFormats: [0: Support.commaDecimal]))
        XCTAssertEqual(own.plan[0].door, .f64)
        XCTAssertEqual(own.plan[0].format, Support.commaDecimal)

        // With no header, the width is the first record's, and no column has a name.
        let headerless = Dialect(separator: ",", hasHeader: false)
        let none = try DelimitedReader.suggest(bytes: utf8("1,x,2\n3,y,4\n"), dialect: headerless)
        XCTAssertEqual(none.plan.count, 3)
        XCTAssertNil(none.report[0].name)
        // Nothing to read is nothing to say.
        let nothing = try DelimitedReader.suggest(bytes: [UInt8](), dialect: headerless)
        XCTAssertEqual(nothing.plan, [])
        XCTAssertEqual(nothing.report, [])
        XCTAssertFalse(nothing.truncated)
        // A failure among the sampled rows is the suggestion's, and just past them says only that there's more.
        let broken = utf8("a,b\n1,2\n3\n")
        XCTAssertThrowsError(try DelimitedReader.suggest(bytes: broken, dialect: .csv)) {
            XCTAssertTrue($0 is TabularError)
        }
        XCTAssertTrue(
            try DelimitedReader.suggest(bytes: broken, dialect: .csv, options: SuggestOptions(sampleRows: 1)).truncated)
        // A file that isn't there is the open's error, not a suggestion's.
        let missing = FileManager.default.temporaryDirectory.appendingPathComponent("hypertabular-missing.csv").path
        XCTAssertThrowsError(try DelimitedReader.suggest(contentsOfFile: missing, dialect: .csv)) {
            XCTAssertFalse($0 is TabularError || $0 is PlanError, "\($0)")
        }
        // Under an encoding the sample is the decoded text, and a name is too.
        let latin = try DelimitedReader.suggest(
            bytes: [0x63, 0x61, 0x66, 0xE9, 0x0A, 0x31, 0x0A], dialect: Dialect(separator: ",", encoding: .latin1))
        XCTAssertEqual(latin.report[0].name, "caf\u{E9}")
    }

    /// A source too wide to plan is refused before it is read, and one with no columns takes none.
    func testASampleFitsAPlan() throws {
        func bind(_ plan: [Column]) { XCTFail("bound") }
        func read() -> Batch? {
            XCTFail("read")
            return nil
        }
        XCTAssertThrowsError(
            try Sample.of(width: Columns.maxOrdinal + 2, limit: 1, sheet: false, bind: bind, read: read)
        ) { XCTAssertTrue("\($0)".contains("2^20"), "\($0)") }
        XCTAssertNoThrow(try Sample.of(width: 0, limit: 1, sheet: false, bind: bind, read: read))
    }

    /// A sheet's suggestion, as delimited text's: a read of its own, from the start, that leaves this one
    /// where it was; a header's names, or a width from the first row; and a range's ordinals.
    func testASheetSuggests() throws {
        let book = try Workbook(contentsOfFile: try Support.package("ranges.xlsx"))
        let options = SheetOptions(range: try SheetRange(parsing: "B4:E12"))
        let sheet = try book.sheet(0, options: options)
        let found = try sheet.suggest()
        XCTAssertFalse(sheet.isBound)
        XCTAssertEqual(found.report.map(\.name), ["id", "name", "amt", "day"])
        // "merged" beside the ids is a cell no door takes, beside cells one does.
        XCTAssertEqual(found.plan[0].door, .text)
        XCTAssertEqual(found.report[0].status, .unresolved)
        XCTAssertEqual(found.report[0].matched, 6)
        XCTAssertEqual(found.plan[1].door, .text)
        // A typed date's canonical text isn't taken as a date: it's text, resolved, by hand.
        XCTAssertEqual(found.plan[3].door, .text)
        // The plan reads this sheet, from its start.
        try sheet.bind(found.plan)
        XCTAssertEqual(try sheet.read()?.rows, 7)

        // No header: the width is the first row's.
        var bare = options
        bare.hasHeader = false
        let headless = try book.sheet(0, options: bare).suggest(SuggestOptions(sampleRows: 2))
        XCTAssertEqual(headless.plan.count, 4)
        XCTAssertNil(headless.report[0].name)
        XCTAssertTrue(headless.truncated)
        XCTAssertEqual(headless.report[0].sampled, 2)
    }

    private func readFails(_ book: Workbook, _ index: Int) -> Bool {
        do {
            let sheet = try book.sheet(index, plan: [.text(0)])
            while try sheet.read() != nil {}
            return false
        } catch {
            return error is TabularError
        }
    }

    /// Every sheet of every package, under every declaration: a plan that reads what it sampled, and a
    /// suggestion that fails only where a read does.
    func testNoSheetsPlanRefusesACellItSampled() throws {
        let directory = URL(fileURLWithPath: try Support.package("")).path
        var planned = 0
        var typed = 0
        var refused = 0
        for file in try FileManager.default.contentsOfDirectory(atPath: directory) where !file.hasSuffix(".py") {
            let path = URL(fileURLWithPath: directory).appendingPathComponent(file).path
            guard let book = try? Workbook(contentsOfFile: path) else { continue }
            for index in book.sheets.indices {
                let declarations: [NumFormat?] = [nil, .invariant, Support.commaDecimal, Support.indian]
                for declared in declarations {
                    let found: Suggestion
                    do {
                        found = try book.sheet(index).suggest(SuggestOptions(format: declared))
                    } catch let failure as TabularError {
                        refused += 1
                        XCTAssertTrue(
                            readFails(book, index), "\(file), sheet \(index): \(failure), where a read has none")
                        continue
                    }
                    if found.plan.isEmpty { continue }
                    planned += 1
                    typed += found.plan.filter { $0.door != .text }.count
                    let read = try book.sheet(index, plan: found.plan)
                    var rows = 0
                    while rows < 1000, let batch = try read.read() {
                        for problem in batch.faults() where problem.row + rows < 1000 {
                            if let plan = problem.planColumn, problem.reason != .empty, found.plan[plan].door != .text {
                                XCTFail("\(file), sheet \(index), \(String(describing: declared)): \(problem)")
                            }
                        }
                        rows += batch.rows
                    }
                }
            }
        }
        XCTAssertGreaterThan(planned, 80)
        XCTAssertGreaterThanOrEqual(refused, 2)
        XCTAssertGreaterThan(typed, 350)
    }

    /// A typed number has no locale: a workbook's number column is a number whether a notation is declared
    /// or not, and whichever.
    func testAWorkbooksNumberColumnIsNumericUnderAnyFormat() throws {
        for (file, first, second) in [
            ("basic.xlsx", "amount", "ratio"), ("basic.ods", "amount", "pct"), ("excel-web-data.xlsx", "pct", "money"),
        ] {
            let book = try Workbook(contentsOfFile: try Support.package(file))
            let declarations: [NumFormat?] = [nil, .invariant, Support.commaDecimal]
            for declared in declarations {
                let options = SuggestOptions(format: declared)
                let found = try book.sheet(0).suggest(options)
                for name in [first, second] {
                    let report = try XCTUnwrap(found.report.first { $0.name == name }, "\(file) \(name)")
                    XCTAssertEqual(report.status, .resolved, "\(file) \(name)")
                    XCTAssertTrue(Column.isNumeric(report.door), "\(file) \(name)")
                }
                // The sample's notes are its own, whatever a read of the sheet was to have.
                let quiet = try book.sheet(0, options: SheetOptions(notes: false)).suggest(options)
                XCTAssertEqual(quiet, found, file)
            }
        }
    }

    /// One batch is the most a sample can be, whatever sampleRows asks.
    func testASheetsSampleIsOneOfItsBatches() throws {
        let book = try Support.strings(["a", "1", "2", "3", "4", "5", "6", "7", "8"])
        let found = try book.sheet(0, options: SheetOptions(batchRows: 5)).suggest()
        XCTAssertEqual(found.report[0].sampled, 5)
        XCTAssertTrue(found.truncated)
        let whole = try book.sheet(0).suggest()
        XCTAssertEqual(whole.report[0].sampled, 8)
        XCTAssertFalse(whole.truncated)
    }

    /// The sample takes notes of its own: with none, every text cell the second door refuses would pass
    /// for a typed one, and 1,234 in a string would settle as a number.
    func testASheetWithNotesOffStillTellsTextFromATypedCell() throws {
        let book = try Support.strings(["a", "1,234", "5,678"])
        let noted = try book.sheet(0).suggest()
        XCTAssertEqual(noted.report[0].status, .ambiguous)
        XCTAssertEqual(try book.sheet(0, options: SheetOptions(notes: false)).suggest(), noted)
    }

    private struct Broke: Error, Equatable {}

    /// A failure just past a whole sample only says there's more; among the sampled rows it's the caller's.
    func testAFailureJustPastTheSampleSaysTruncated() throws {
        func sampled(batchRows: Int) throws -> Sample {
            let reader = try DelimitedReader(
                bytes: utf8("a\n1\n2\n3\n4\n"), dialect: .csv, plan: [.text(0)], batchRows: batchRows)
            var left = 1
            return try Sample.of(
                width: 1, limit: 2, sheet: false, bind: { _ in },
                read: {
                    guard left > 0 else { throw Broke() }
                    left -= 1
                    return try reader.read()
                })
        }
        XCTAssertTrue(try sampled(batchRows: 2).judge(header: nil, options: SuggestOptions()).truncated)
        XCTAssertThrowsError(try sampled(batchRows: 1)) { XCTAssertEqual($0 as? Broke, Broke()) }
    }

    /// A declared order answers the sample's question and only that.
    func testADeclaredDateOrderSettlesADateTheSampleLeftOpen() throws {
        let open = utf8("day\n01/02/2026\n03/04/2026\n")
        func ask(_ input: [UInt8], _ options: SuggestOptions = SuggestOptions()) throws -> (SuggestedColumn, Column) {
            let found = try DelimitedReader.suggest(bytes: input, dialect: .csv, options: options)
            return (found.report[0], found.plan[0])
        }
        var (report, plan) = try ask(open)
        XCTAssertEqual(report.status, .ambiguous)
        XCTAssertEqual(plan.door, .text)

        (report, plan) = try ask(open, SuggestOptions(dateOrder: .dayMonthYear))
        XCTAssertEqual(report.status, .resolved)
        XCTAssertEqual(report.candidates, [])
        XCTAssertEqual(plan.door, .dateOrdered)
        XCTAssertEqual(report.param, DateOrder.dayMonthYear.rawValue)

        (report, plan) = try ask(open, SuggestOptions(dateOrder: .dayMonthYear, columnDateOrders: [0: .monthDayYear]))
        XCTAssertEqual(report.status, .resolved)
        XCTAssertEqual(plan.door, .dateOrdered)
        XCTAssertEqual(report.param, DateOrder.monthDayYear.rawValue)

        (report, _) = try ask(utf8("day\n13/02/2026\n"), SuggestOptions(dateOrder: .monthDayYear))
        XCTAssertEqual(report.status, .resolved)
        XCTAssertEqual(report.param, DateOrder.dayMonthYear.rawValue)

        (report, plan) = try ask(open, SuggestOptions(dateOrder: .yearMonthDay))
        XCTAssertEqual(report.status, .ambiguous)
        XCTAssertEqual(plan.door, .text)
    }

    /// A sheet answers from its first row whatever it has been up to, and carries on where it was.
    func testAPartReadSheetAnswersTheSameAndStaysWhereItIs() throws {
        let book = try Workbook(contentsOfFile: try Support.package("libreoffice-data.xlsx"))
        let options = SheetOptions(batchRows: 1)
        let sample = SuggestOptions(sampleRows: 1)
        let fresh = try book.sheet(0, options: options).suggest(sample)
        XCTAssertGreaterThan(fresh.plan.count, 0)
        let plan = fresh.plan.indices.map { Column.text($0) }
        let whole = try book.sheet(0, options: options, plan: plan)
        _ = try XCTUnwrap(whole.read())
        let line = try XCTUnwrap(whole.read()).line(0)

        let part = try book.sheet(0, options: options, plan: plan)
        _ = try XCTUnwrap(part.read())
        XCTAssertEqual(try part.suggest(sample), fresh)
        XCTAssertEqual(try XCTUnwrap(part.read()).line(0), line)
    }

    /// Skip, and what a short and a long row are, as a read is told them.
    func testADialectsOptionsReachARead() throws {
        // skip passes lines over before anything is read of them, header_row records after that.
        let reader = try DelimitedReader(
            bytes: utf8("junk\"\nmore\nid,name\n1,a\n2\n3,b,c\n"),
            dialect: Dialect(separator: ",", skip: 2, shortRows: .pad, longRows: .truncate), plan: [.i32(0), .text(1)])
        XCTAssertEqual(reader.header, ["id", "name"])
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.rows, 3)
        XCTAssertEqual(batch.origin(1, row: 1), .padded)
        XCTAssertEqual(batch.line(0), 4)
        XCTAssertEqual(Array(batch.faults()).count, 0, "neither a padded cell nor a truncated row is a problem")
    }

    #if os(macOS) || os(Linux)
        /// What was asked of a batch stays that batch's: going on after the reader moved on is a precondition
        /// failure, whether it found another batch or the end, and never an answer about the next rows. Each
        /// is run in a child of its own, beside the control that does the same in time and returns.
        func testWhatWasAskedOfABatchStaysThatBatchs() {
            let input = Array("a\nx\ny\n".utf8)
            func opened(batchRows: Int = 1) -> DelimitedReader {
                try! DelimitedReader(bytes: input, dialect: .csv, plan: [.i32(0)], batchRows: batchRows)
            }
            func step(_ reader: DelimitedReader) -> Batch { try! reader.read()! }

            // A list asked, and the walk not begun.
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let list = step(reader).faults()
                    _ = step(reader)
                    _ = list.makeIterator()
                }, .trapped)
            // Asked, and the reader at the end.
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    var walk = step(reader).faults().makeIterator()
                    while try! reader.read() != nil {}
                    _ = walk.next()
                }, .trapped)
            // A walk under way.
            XCTAssertEqual(
                Support.ending {
                    let reader = opened(batchRows: 8)
                    var walk = step(reader).faults().makeIterator()
                    _ = walk.next()
                    _ = try! reader.read()
                    _ = walk.next()
                }, .trapped)
            // A walk of the rows, and a row held.
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    var walk = step(reader).makeIterator()
                    _ = step(reader)
                    _ = walk.next()
                }, .trapped)
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let row = step(reader)[0]
                    _ = step(reader)
                    _ = row.line
                }, .trapped)
            // In time, the same calls return.
            XCTAssertEqual(
                Support.ending {
                    let reader = opened(batchRows: 8)
                    let batch = step(reader)
                    var walk = batch.faults().makeIterator()
                    _ = walk.next()
                    var rows = batch.makeIterator()
                    _ = rows.next()?.line
                    _ = batch[0].line
                }, .returned)
        }

        /// A slice or a reversal is cut from one batch, and refuses as the batch's own walk does.
        func testASliceOrAReversalStaysItsBatch() {
            func opened() -> DelimitedReader {
                try! DelimitedReader(bytes: Array("a\n1\n2\n3\n4\n".utf8), dialect: .csv, plan: [.i32(0)], batchRows: 2)
            }
            func step(_ reader: DelimitedReader) -> Batch { try! reader.read()! }
            func walked<Rows: Sequence>(_ rows: Rows) where Rows.Element == Row {
                for row in rows { _ = row.line }
            }

            // Kept past the next read, each way of cutting one, each way of reading it.
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader).dropFirst(1)
                    _ = step(reader)
                    walked(kept)
                }, .trapped, "a slice, walked")
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader).prefix(1)
                    _ = step(reader)
                    _ = kept.first?.line
                }, .trapped, "a slice, its first")
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader)[0..<2]
                    _ = step(reader)
                    _ = kept[1].line
                }, .trapped, "a slice, by index")
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader)[0..<2]
                    _ = step(reader)
                    _ = kept.dropFirst()
                }, .trapped, "a slice, cut again")
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader)[0..<2].reversed()
                    _ = step(reader)
                    walked(kept)
                }, .trapped, "a slice, reversed")
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader).reversed()
                    _ = step(reader)
                    walked(kept)
                }, .trapped, "a reversal, walked")
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader).reversed()
                    _ = step(reader)
                    _ = kept.first?.line
                }, .trapped, "a reversal, its first")
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader).dropFirst(0)
                    while try! reader.read() != nil {}
                    walked(kept)
                }, .trapped, "a slice, with the reader at the end")
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    let kept = step(reader)[0..<2]
                    _ = step(reader)
                    _ = kept[1]
                }, .trapped, "a slice's row, not read yet")
            // Outside what was cut, though still in the batch.
            XCTAssertEqual(Support.ending { _ = step(opened())[0..<1][1] }, .trapped, "past a slice's end")
            XCTAssertEqual(Support.ending { _ = step(opened())[1..<2][0] }, .trapped, "before a slice's start")
            XCTAssertEqual(Support.ending { _ = step(opened())[0..<1][0..<2] }, .trapped, "a cut past a slice's end")
            XCTAssertEqual(Support.ending { _ = step(opened())[0..<3] }, .trapped, "a cut past the batch's end")
            // A walk begun before the read, too.
            XCTAssertEqual(
                Support.ending {
                    let reader = opened()
                    var walk = step(reader).dropFirst(0).makeIterator()
                    _ = walk.next()
                    _ = step(reader)
                    _ = walk.next()
                }, .trapped, "a slice's walk under way")
            // In time, the same calls return.
            XCTAssertEqual(
                Support.ending {
                    let batch = step(opened())
                    walked(batch.dropFirst(1))
                    walked(batch.reversed())
                    walked(batch[0..<2].reversed())
                    _ = batch.prefix(1).first?.line
                    _ = batch[0..<2].dropFirst()[1].line
                }, .returned)
        }

        /// A negative sample is a caller bug: Swift can tell it from one unset.
        func testANegativeSampleTraps() {
            XCTAssertEqual(Support.ending { _ = SuggestOptions(sampleRows: -1) }, .trapped)
            XCTAssertEqual(Support.ending { _ = SuggestOptions(sampleRows: 0) }, .returned)
        }
    #endif
}
