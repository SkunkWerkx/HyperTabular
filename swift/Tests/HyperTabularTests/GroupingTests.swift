import Foundation
import XCTest

@testable import HyperTabular

/// One Indian column beside a Western one in one read, and the retired policy word refused at every
/// door with nothing read. The bytes hold a shape of each policy in each column, so a plan reads them
/// both ways.
final class GroupingTests: XCTestCase {
    private static let bytes = Array("indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n".utf8)
    private static let headerless = Dialect(separator: ",", hasHeader: false)

    /// Holds a door to what a row wants: refused for the format, naming the column; or let through.
    private func holds(_ what: String, _ refused: Bool, column: Int = 0, _ call: () throws -> Void) {
        if refused {
            XCTAssertThrowsError(try call(), what) { XCTAssertEqual($0 as? PlanError, .refused(column: column), what) }
        } else {
            XCTAssertNoThrow(try call(), what)
        }
    }

    func testThePoliciesHereAreTheWordsTheySay() {
        for (flags, format, _) in Support.policies {
            XCTAssertEqual(Interop.rawFormat(format).1, flags)
        }
    }

    func testAnIndianColumnReadsBesideAWesternOne() throws {
        // Twice: named, and with no policy said, which is Western.
        for western in [NumFormat.invariant, NumFormat(decimalSeparator: ".", groupSeparator: ",", styles: .all)] {
            let reader = try DelimitedReader(
                bytes: Self.bytes, dialect: .csv, plan: [.i32(0, format: Support.indian), .i32(1, format: western)])
            let batch = try XCTUnwrap(reader.read())
            XCTAssertEqual(batch.rows, 2)
            XCTAssertTrue(batch.verdict(0, row: 0).isOk)
            XCTAssertEqual(batch.values(0, as: Int32.self)[0], 123_456)
            XCTAssertEqual(batch.verdict(0, row: 1).reason, .malformed)
            XCTAssertEqual(batch.verdict(1, row: 0).reason, .malformed)
            XCTAssertTrue(batch.verdict(1, row: 1).isOk)
            XCTAssertEqual(batch.values(1, as: Int32.self)[1], 1_234_567)
        }
    }

    func testAColumnWithNoFormatIsWestern() throws {
        let reader = try DelimitedReader(bytes: Self.bytes, dialect: .csv, plan: [.i32(0)])
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.verdict(0, row: 0).reason, .malformed)
        XCTAssertEqual(batch.values(0, as: Int32.self)[1], 1_234_567)
        XCTAssertEqual(Interop.rawFormat(Column.i32(0).format).1, 95)
    }

    func testAnIndianSheetColumnReadsBesideAWesternOne() throws {
        for file in ["grouping.xlsx", "grouping.ods", "grouping.numbers"] {
            let book = try Workbook(contentsOfFile: try Support.package(file))
            let sheet = try book.sheet(0, plan: [.i32(2, format: Support.indian), .i32(2, format: .invariant)])
            let batch = try XCTUnwrap(sheet.read())
            XCTAssertEqual(batch.rows, 3, file)
            XCTAssertTrue(batch.verdict(0, row: 0).isOk, file)
            XCTAssertEqual(batch.values(0, as: Int32.self)[0], 123_456, file)
            XCTAssertEqual(batch.verdict(0, row: 1).reason, .malformed, file)
            XCTAssertEqual(batch.values(0, as: Int32.self)[2], 1234, file)
            XCTAssertEqual(batch.verdict(1, row: 0).reason, .malformed, file)
            XCTAssertTrue(batch.verdict(1, row: 1).isOk, file)
            XCTAssertEqual(batch.values(1, as: Int32.self)[1], 1_234_567, file)
            XCTAssertEqual(batch.values(1, as: Int32.self)[2], 1234, file)
        }
    }

    func testAStalePolicyIsRefusedAtEveryDelimitedDoor() async throws {
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("hypertabular-grouping-\(UUID().uuidString).csv").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        try Data(Self.bytes).write(to: URL(fileURLWithPath: path))
        let bytes = Self.bytes
        for (flags, format, refused) in Support.policies {
            let plan: [Column] = [.i32(0, format: format)]
            holds("memory, \(flags)", refused) { _ = try DelimitedReader(bytes: bytes, dialect: .csv, plan: plan) }
            try bytes.withUnsafeBytes { memory in
                holds("in place, \(flags)", refused) {
                    _ = try DelimitedReader(bytesNoCopy: memory, dialect: .csv, plan: plan)
                }
                let reader = try DelimitedReader(bytesNoCopy: memory, dialect: .csv)
                holds("bind in place, \(flags)", refused) { try reader.bind(plan) }
            }
            holds("stream, \(flags)", refused) {
                _ = try DelimitedReader(reading: chunks(bytes), dialect: .csv, plan: plan)
            }
            holds("path, \(flags)", refused) {
                _ = try DelimitedReader(contentsOfFile: path, dialect: .csv, plan: plan)
            }
            do {
                _ = try await DelimitedReader(
                    bytes: CorpusTests.ShortReads(bytes, chunk: 7), dialect: .csv, plan: plan)
                XCTAssertFalse(refused, "async, \(flags)")
            } catch {
                XCTAssertTrue(refused, "async, \(flags): \(error)")
                XCTAssertEqual(error as? PlanError, .refused(column: 0), "async, \(flags)")
            }

            let unbound: [DelimitedReader] = [
                try DelimitedReader(bytes: bytes, dialect: .csv),
                try DelimitedReader(reading: chunks(bytes), dialect: .csv),
                try DelimitedReader(contentsOfFile: path, dialect: .csv),
                try await DelimitedReader(bytes: CorpusTests.ShortReads(bytes, chunk: 7), dialect: .csv),
            ]
            for reader in unbound {
                holds("bind, \(flags)", refused) { try reader.bind(plan) }
                XCTAssertEqual(reader.isBound, !refused, "bind, \(flags)")
                if refused {
                    do {
                        _ = try await reader.readAsync()
                        XCTFail("bind, \(flags): read with no plan")
                    } catch {
                        XCTAssertEqual(error as? PlanError, .unbound)
                    }
                } else {
                    let batch = try await reader.readAsync()
                    XCTAssertEqual(batch?.rows, 2, "\(flags)")
                }
            }

            holds("suggest memory, \(flags)", refused) {
                _ = try DelimitedReader.suggest(bytes: bytes, dialect: .csv, options: SuggestOptions(format: format))
            }
            holds("suggest path, \(flags)", refused, column: 1) {
                _ = try DelimitedReader.suggest(
                    contentsOfFile: path, dialect: .csv, options: SuggestOptions(columnFormats: [1: format]))
            }
        }
    }

    private func chunks(_ bytes: [UInt8]) -> DelimitedReader.Source {
        var position = 0
        return { buffer in
            let count = min(buffer.count, bytes.count - position)
            buffer.copyBytes(from: bytes[position..<position + count])
            position += count
            return count
        }
    }

    func testAStalePolicyIsRefusedAtEverySheetDoor() throws {
        let book = try Workbook(contentsOfFile: try Support.package("grouping.xlsx"))
        for (flags, format, refused) in Support.policies {
            let plan: [Column] = [.i32(0, format: format)]
            holds("sheet, \(flags)", refused) { _ = try book.sheet(0, plan: plan) }
            holds("sheet by name, \(flags)", refused) { _ = try book.sheet(named: book.sheets[0].name, plan: plan) }

            let sheet = try book.sheet(0)
            holds("suggest, \(flags)", refused) { _ = try sheet.suggest(SuggestOptions(format: format)) }
            holds("bind, \(flags)", refused) { try sheet.bind(plan) }
            XCTAssertEqual(sheet.isBound, !refused)
            if refused {
                XCTAssertThrowsError(try sheet.read()) { XCTAssertEqual($0 as? PlanError, .unbound) }
            } else {
                XCTAssertNotNil(try sheet.read())
            }
        }
    }

    /// A suggest judges every format it is given before it opens anything: one never met, one replaced,
    /// one past the sample, an empty source and a broken one. A word without the retired bit is the
    /// control, which gets as far as the source does.
    func testAStaleFormatIsRefusedBeforeASuggestOpensAnything() throws {
        for (flags, format, refused) in Support.policies {
            let every = SuggestOptions(format: format)
            let past = SuggestOptions(columnFormats: [99: format])
            let replaced = SuggestOptions(format: format, columnFormats: [0: Support.indian])
            let table: [(String, String, Dialect, SuggestOptions, Int)] = [
                ("a sample", "a\n1\n", .csv, every, 0),
                ("empty", "", .csv, every, 0),
                ("empty, headerless", "", Self.headerless, every, 0),
                ("past the sample", "a\n1\n", .csv, past, 99),
                ("replaced for a column", "a\n1\n", .csv, replaced, 0),
            ]
            for (what, text, dialect, options, column) in table {
                holds("\(what), \(flags)", refused, column: column) {
                    _ = try DelimitedReader.suggest(bytes: Array(text.utf8), dialect: dialect, options: options)
                }
            }

            // A broken source says so only of a format that is sound.
            for (text, dialect) in [("a\n\"1\n", Dialect.csv), ("\"1\n", Self.headerless)] {
                XCTAssertThrowsError(
                    try DelimitedReader.suggest(bytes: Array(text.utf8), dialect: dialect, options: every),
                    "\(text), \(flags)"
                ) { error in
                    if refused {
                        XCTAssertEqual(error as? PlanError, .refused(column: 0))
                    } else {
                        XCTAssertTrue(error is TabularError, "\(error)")
                    }
                }
            }
            let missing = FileManager.default.temporaryDirectory.appendingPathComponent("hypertabular-no-such-file.csv")
                .path
            XCTAssertThrowsError(try DelimitedReader.suggest(contentsOfFile: missing, dialect: .csv, options: every)) {
                error in
                if refused {
                    XCTAssertEqual(error as? PlanError, .refused(column: 0))
                } else {
                    XCTAssertFalse(error is PlanError || error is TabularError, "\(error)")
                }
            }

            // A sheet: the same, and one whose sample would meet its broken part.
            let book = try Workbook(contentsOfFile: try Support.package("grouping.xlsx"))
            let sheet = try book.sheet(0)
            for (what, options, column) in [("every", every, 0), ("past", past, 99), ("replaced", replaced, 0)] {
                holds("a sheet, \(what), \(flags)", refused, column: column) { _ = try sheet.suggest(options) }
            }
            let broken = try Workbook(contentsOfFile: try Support.package("broken.xlsx"))
            let part = try broken.sheet(0)
            XCTAssertThrowsError(try part.suggest(every)) { error in
                if refused {
                    XCTAssertEqual(error as? PlanError, .refused(column: 0))
                } else {
                    XCTAssertTrue(error is TabularError, "\(error)")
                }
            }
        }
    }

    /// What the delimiter rule says stays: nothing says 1,23,456 is Indian. Declared, it is a number.
    func testAnIndianShapeHasNoCultureUntilOneIsDeclared() throws {
        func ask(_ text: String, _ options: SuggestOptions = SuggestOptions()) throws -> SuggestedColumn {
            try DelimitedReader.suggest(bytes: Array(text.utf8), dialect: .csv, options: options).report[0]
        }
        let alone = try ask("a\n\"1,23,456\"\n")
        XCTAssertEqual(alone.door, .text)
        XCTAssertEqual(alone.status, .resolved)
        let beside = try ask("a\n\"1,23,456\"\n5\n")
        XCTAssertEqual(beside.door, .text)
        XCTAssertEqual(beside.status, .unresolved)
        XCTAssertEqual(try ask("a\n\"1,234,567\"\n").format.map { Interop.rawFormat($0).1 }, 95)

        let declared = SuggestOptions(columnFormats: [0: Support.indian])
        let read = try ask("a\n\"1,23,456\"\n", declared)
        XCTAssertEqual(read.door, .i32)
        XCTAssertEqual(read.status, .resolved)
        XCTAssertEqual(read.format, Support.indian)
        XCTAssertEqual(read.format.map { Interop.rawFormat($0).1 }, 607)
        let western = try ask("a\n\"1,234,567\"\n", declared)
        XCTAssertEqual(western.door, .text)
        XCTAssertNil(western.format)
    }

    #if os(macOS) || os(Linux)
        /// HyperCast's own door traps at construction: a word with the retired bit is a caller bug there,
        /// and the only way past is the byte route ``Support/worded(_:flags:)`` takes.
        func testHyperCastsOwnDoorTrapsAtConstruction() {
            XCTAssertEqual(
                Support.ending {
                    _ = NumFormat(decimalSeparator: ".", groupSeparator: ",", styles: NumStyles(rawValue: 351))
                }, .trapped)
            XCTAssertEqual(
                Support.ending {
                    _ = NumFormat(decimalSeparator: ".", groupSeparator: ",", styles: NumStyles(rawValue: 95))
                }, .returned)
        }
    #endif
}
