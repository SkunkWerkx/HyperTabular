import Foundation
import XCTest
@testable import HyperTabular

/// What the corpus does not reach: the binding's own surface. Nothing here but the file
/// test touches the file system, so this is the suite a WebAssembly run still has when the
/// corpus is out of its sandbox's reach.
final class ReaderTests: XCTestCase {
    private static let orders = Array("id,name,score\n1,alice,2.5\n2,\"bob, jr\",x\n3,,7\n".utf8)

    private func string(_ bytes: UnsafeRawBufferPointer) -> String {
        String(decoding: bytes, as: UTF8.self)
    }

    func testTheNativeLibraryAnswersTheProbe() {
        XCTAssertTrue(Tabular.isAvailable)
        let version = Tabular.nativeVersion().split(separator: ".")
        XCTAssertEqual(version.count, 3, "nativeVersion is major.minor.patch")
        XCTAssertTrue(version.allSatisfy { Int($0) != nil })
    }

    func testAColumnIsABufferAndACellIsAUnion() throws {
        let reader = try DelimitedReader(bytes: Self.orders, dialect: .csv, plan: [.i32(0), .text(1), .f64(2)])
        XCTAssertEqual(reader.header, ["id", "name", "score"])
        XCTAssertEqual(reader.rows, 0)
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(reader.rows, 3)

        XCTAssertEqual(Array(reader.values(0, as: Int32.self)), [1, 2, 3])
        XCTAssertEqual(Array(reader.values(2, as: Double.self)), [2.5, 0.0, 7.0])
        XCTAssertEqual(reader.verdicts(2).map(\.isOk), [true, false, true])
        XCTAssertEqual(reader.verdicts(2)[1].reason, .malformed)
        XCTAssertEqual(reader.verdict(2, row: 1).fault, Fault(reason: .malformed, offset: 0, length: 1))

        let described =
            switch reader.f64(2, row: 1) {
            case .success(let score): "\(score)"
            case .fault(let fault): "\(fault.reason) in \"\(string(reader.raw(2, row: 1)))\""
            }
        XCTAssertEqual(described, "malformed in \"x\"")
        XCTAssertEqual(reader.string(1, row: 1), "bob, jr")
        XCTAssertNil(reader.string(1, row: 2))
        XCTAssertEqual(reader.verdict(1, row: 2).reason, .empty)
        XCTAssertFalse(try reader.read())
        XCTAssertEqual(reader.rows, 0)
        XCTAssertEqual(reader.records, 4)
    }

    /// Every door, and each value as the Swift type HyperCast's own door presents for the
    /// same text — held by asking HyperCast directly.
    func testEveryDoorPresentsWhatHyperCastDoes() throws {
        let cells = [
            "yes", "-128", "32767", "(1234)", "9223372036854775807", "255", "65535", "4294967295",
            "18446744073709551615", "2.5", "25.5%", "79228162514264337593543950.335",
            "6ba7b810-9dad-11d1-80b4-00c04fd430c8", "2026-01-02T15:04:05.25+05:00", "1700000000123",
            "45292.75", "2024-01-31", "1/7/2026", "1/7/2026 3:04:05.000000007 PM", "15:04:05.5",
            "-PT1H30M0.000000001S", "\"a \"\"quoted\"\" cell\"",
        ]
        let input = Array((cells.joined(separator: ",") + "\n").utf8)
        var dialect = Dialect.csv
        dialect.hasHeader = false
        let plan: [Column] = [
            .bool(0), .i8(1), .i16(2), .i32(3), .i64(4), .u8(5), .u16(6), .u32(7), .u64(8), .f32(9),
            .f64(10), .decimal(11), .uuid(12), .timestamp(13), .unix(14, precision: .milliseconds),
            .excelSerial(15, epoch: .y1900), .date(16), .date(17, order: .monthDayYear),
            .dateTime(18, order: .monthDayYear), .time(19), .duration(20), .text(21),
        ]
        XCTAssertEqual(Set(plan.map(\.door)).count, 22, "one column per door")
        let reader = try DelimitedReader(bytes: input, dialect: dialect, plan: plan, batchRows: 8)
        XCTAssertNil(reader.header)
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(reader.rows, 1)
        func raw(_ column: Int) -> UnsafeRawBufferPointer { reader.raw(column, row: 0) }

        XCTAssertEqual(reader.bool(0, row: 0), .success(true))
        XCTAssertEqual(reader.bool(0, row: 0), try Cast.bool(raw(0)))
        XCTAssertEqual(reader.i8(1, row: 0), .success(-128))
        XCTAssertEqual(reader.i8(1, row: 0), try Cast.i8(raw(1), format: .invariant))
        XCTAssertEqual(reader.i16(2, row: 0), .success(32767))
        XCTAssertEqual(reader.i16(2, row: 0), try Cast.i16(raw(2), format: .invariant))
        XCTAssertEqual(reader.i32(3, row: 0), .success(-1234))
        XCTAssertEqual(reader.i32(3, row: 0), try Cast.i32(raw(3), format: .invariant))
        XCTAssertEqual(reader.i64(4, row: 0), .success(.max))
        XCTAssertEqual(reader.i64(4, row: 0), try Cast.i64(raw(4), format: .invariant))
        XCTAssertEqual(reader.u8(5, row: 0), .success(255))
        XCTAssertEqual(reader.u8(5, row: 0), try Cast.u8(raw(5), format: .invariant))
        XCTAssertEqual(reader.u16(6, row: 0), .success(65535))
        XCTAssertEqual(reader.u16(6, row: 0), try Cast.u16(raw(6), format: .invariant))
        XCTAssertEqual(reader.u32(7, row: 0), .success(4_294_967_295))
        XCTAssertEqual(reader.u32(7, row: 0), try Cast.u32(raw(7), format: .invariant))
        XCTAssertEqual(reader.u64(8, row: 0), .success(.max))
        XCTAssertEqual(reader.u64(8, row: 0), try Cast.u64(raw(8), format: .invariant))
        XCTAssertEqual(reader.f32(9, row: 0), .success(2.5))
        XCTAssertEqual(reader.f32(9, row: 0), try Cast.f32(raw(9), format: .invariant))
        XCTAssertEqual(reader.f64(10, row: 0), .success(0.255))
        XCTAssertEqual(reader.f64(10, row: 0), try Cast.f64(raw(10), format: .invariant))
        // 96 bits of magnitude: the high word is in play, and nothing is rounded.
        XCTAssertEqual(reader.decimal(11, row: 0), .success(Decimal(string: "79228162514264337593543950.335")!))
        XCTAssertEqual(reader.decimal(11, row: 0), try Cast.decimal(raw(11), format: .invariant))
        XCTAssertEqual(reader.uuid(12, row: 0), .success(UUID(uuidString: "6ba7b810-9dad-11d1-80b4-00c04fd430c8")!))
        XCTAssertEqual(reader.uuid(12, row: 0), try Cast.uuid(raw(12)))
        XCTAssertEqual(reader.timestamp(13, row: 0), .success(Date(timeIntervalSince1970: 1_767_348_245.25)))
        XCTAssertEqual(reader.timestamp(13, row: 0), try Cast.timestamp(raw(13)))
        XCTAssertEqual(reader.timestamp(14, row: 0), .success(Date(timeIntervalSince1970: 1_700_000_000.123)))
        XCTAssertEqual(reader.timestamp(14, row: 0), try Cast.unix(raw(14), precision: .milliseconds))
        XCTAssertEqual(reader.timestamp(15, row: 0), .success(Date(timeIntervalSince1970: 1_704_132_000)))
        XCTAssertEqual(reader.timestamp(15, row: 0), try Cast.excelSerial(raw(15), epoch: .y1900))
        XCTAssertEqual(reader.date(16, row: 0), .success(DateComponents(year: 2024, month: 1, day: 31)))
        XCTAssertEqual(reader.date(16, row: 0), try Cast.date(raw(16)))
        XCTAssertEqual(reader.date(17, row: 0), .success(DateComponents(year: 2026, month: 1, day: 7)))
        XCTAssertEqual(reader.date(17, row: 0), try Cast.date(raw(17), order: .monthDayYear))
        XCTAssertEqual(
            reader.dateTime(18, row: 0),
            .success(DateComponents(year: 2026, month: 1, day: 7, hour: 15, minute: 4, second: 5, nanosecond: 7)))
        XCTAssertEqual(reader.dateTime(18, row: 0), try Cast.dateTime(raw(18), order: .monthDayYear))
        XCTAssertEqual(
            reader.time(19, row: 0),
            .success(DateComponents(hour: 15, minute: 4, second: 5, nanosecond: 500_000_000)))
        XCTAssertEqual(reader.time(19, row: 0), try Cast.time(raw(19)))
        XCTAssertEqual(reader.duration(20, row: 0), .success(.seconds(-5400) - .nanoseconds(1)))
        XCTAssertEqual(reader.duration(20, row: 0), try Cast.duration(raw(20)))
        XCTAssertEqual(reader.string(21, row: 0), "a \"quoted\" cell")
        XCTAssertEqual(string(raw(21)), "a \"quoted\" cell")
        XCTAssertFalse(try reader.read())
    }

    /// The same doors on nothing at all: every one says `empty`, as HyperCast does.
    func testEveryDoorOnNothing() throws {
        var dialect = Dialect.csv
        dialect.hasHeader = false
        dialect.skipBlankLines = false
        let plan: [Column] = [
            .bool(0), .i8(0), .i16(0), .i32(0), .i64(0), .u8(0), .u16(0), .u32(0), .u64(0), .f32(0),
            .f64(0), .decimal(0), .uuid(0), .timestamp(0), .unix(0, precision: .seconds),
            .excelSerial(0, epoch: .y1904), .date(0), .date(0, order: .dayMonthYear),
            .dateTime(0, order: .yearMonthDay), .time(0), .duration(0), .text(0),
        ]
        let reader = try DelimitedReader(bytes: Array("\n".utf8), dialect: dialect, plan: plan)
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(reader.rows, 1)
        let empty = Fault(reason: .empty, offset: 0, length: 0)
        for column in plan.indices {
            XCTAssertEqual(reader.verdict(column, row: 0).fault, empty, "column \(column)")
            XCTAssertEqual(reader.raw(column, row: 0).count, 0, "column \(column)")
        }
        XCTAssertEqual(reader.bool(0, row: 0), .fault(empty))
        XCTAssertEqual(reader.decimal(11, row: 0), .fault(empty))
        XCTAssertEqual(reader.uuid(12, row: 0), .fault(empty))
        XCTAssertEqual(reader.timestamp(15, row: 0), .fault(empty))
        XCTAssertEqual(reader.date(17, row: 0), .fault(empty))
        XCTAssertEqual(reader.dateTime(18, row: 0), .fault(empty))
        XCTAssertEqual(reader.time(19, row: 0), .fault(empty))
        XCTAssertEqual(reader.duration(20, row: 0), .fault(empty))
        XCTAssertNil(reader.text(21, row: 0))
        XCTAssertEqual(Array(reader.values(0, as: Bool.self)), [false])
        XCTAssertEqual(Array(reader.values(8, as: UInt64.self)), [0])
    }

    /// A declared notation and a declared currency symbol reach the core as HyperCast's
    /// `NumFormat` states them — including a symbol that fills all sixteen bytes.
    func testADeclaredNotationReachesTheCore() throws {
        let euro = NumFormat(decimalSeparator: ",", groupSeparator: ".", styles: .all, currencySymbol: "kr.")
        let wide = NumFormat(decimalSeparator: ".", groupSeparator: ",", styles: .all, currencySymbol: "€€€€€x")
        XCTAssertEqual(wide.currencySymbol.utf8.count, 16)
        var dialect = Dialect(separator: ";")
        dialect.hasHeader = false
        let input = Array("1.234,50 kr.;€€€€€x 7;$7\n".utf8)
        let plan: [Column] = [.decimal(0, format: euro), .i32(1, format: wide), .i32(2, format: wide), .f64(0)]
        let reader = try DelimitedReader(bytes: input, dialect: dialect, plan: plan)
        XCTAssertEqual(reader.columns[0].format, euro)
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(reader.decimal(0, row: 0), .success(Decimal(string: "1234.5")!))
        XCTAssertEqual(reader.decimal(0, row: 0), try Cast.decimal(reader.raw(0, row: 0), format: euro))
        XCTAssertEqual(reader.i32(1, row: 0), .success(7))
        XCTAssertEqual(reader.i32(2, row: 0), try Cast.i32(reader.raw(2, row: 0), format: wide))
        XCTAssertEqual(reader.verdict(2, row: 0).reason, .malformed)
        // The same source column through the invariant notation is a different verdict.
        XCTAssertEqual(reader.f64(3, row: 0), try Cast.f64(reader.raw(3, row: 0), format: .invariant))
        XCTAssertEqual(reader.verdict(3, row: 0).reason, .malformed)
    }

    /// Text is a view of the input wherever the cell had no escaped quote in it; only a
    /// cell with `""` inside is unescaped, into the reader's arena.
    func testTextIsAViewOfTheInput() throws {
        let input = Array("name,note\nalice,\"a, b\"\nbob,\"say \"\"hi\"\"\"\n".utf8)
        try input.withUnsafeBytes { bytes in
            let reader = try DelimitedReader(bytesNoCopy: bytes, dialect: .csv, plan: [.text(0), .text(1), .i32(1)])
            XCTAssertTrue(try reader.read())
            XCTAssertEqual(reader.rows, 2)
            func inside(_ text: UnsafeRawBufferPointer?) -> Bool {
                guard let text, let from = text.baseAddress, let base = bytes.baseAddress else { return false }
                return from >= base && from + text.count <= base + bytes.count
            }
            XCTAssertTrue(inside(reader.text(0, row: 0)))
            XCTAssertEqual(reader.string(0, row: 0), "alice")
            XCTAssertTrue(inside(reader.text(1, row: 0)), "quoted, nothing escaped: still the input")
            XCTAssertEqual(reader.string(1, row: 0), "a, b")
            XCTAssertTrue(inside(reader.raw(2, row: 0)))
            XCTAssertFalse(inside(reader.text(1, row: 1)), "an escaped cell is unescaped elsewhere")
            XCTAssertEqual(reader.string(1, row: 1), "say \"hi\"")
            // The raw text of a cell that failed to cast, escaped quotes resolved.
            XCTAssertEqual(reader.i32(2, row: 1), .fault(Fault(reason: .malformed, offset: 0, length: 1)))
            XCTAssertEqual(string(reader.raw(2, row: 1)), "say \"hi\"")
            XCTAssertFalse(try reader.read())
        }
    }

    func testAPlanIsAProjection() throws {
        let input = Array("a,b,c\n1,2,3\n4,5,6\n".utf8)
        // In its own order, one source column twice, and one past the record's last cell.
        let reader = try DelimitedReader(bytes: input, dialect: .csv, plan: [.i32(2), .text(0), .u8(2), .i32(7)])
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(Array(reader.values(0, as: Int32.self)), [3, 6])
        XCTAssertEqual(Array(reader.values(2, as: UInt8.self)), [3, 6])
        XCTAssertEqual(reader.string(1, row: 1), "4")
        XCTAssertEqual(reader.i32(3, row: 0), .fault(Fault(reason: .empty, offset: 0, length: 0)))
        XCTAssertEqual(reader.raw(3, row: 0).count, 0)

        // No columns at all still counts the rows.
        let counter = try DelimitedReader(bytes: input, dialect: .csv, plan: [], batchRows: 1)
        var rows = 0
        while try counter.read() { rows += counter.rows }
        XCTAssertEqual(rows, 2)
        XCTAssertEqual(counter.records, 3)
    }

    /// The buffers the reader starts with are not a limit: a header wider than the name
    /// table, names and cells that outgrow the arena, a record longer than the read buffer.
    func testEveryBufferGrows() throws {
        let width = 200
        let escaped = "\"" + String(repeating: "x\"\"", count: 3_000) + "\""  // 9,000 bytes; 6,000 unescaped
        let unescaped = String(repeating: "x\"", count: 3_000)
        let header = (0..<width).map { $0 == 0 ? escaped : "c\($0)" }.joined(separator: ",")
        let row = (0..<width).map { $0 == 1 ? escaped : "\($0)" }.joined(separator: ",")
        let input = Array("\(header)\n\(row)\n\(row)\n".utf8)
        let plan: [Column] = [.text(1), .i32(1), .i32(width - 1), .text(0)]

        for bufferBytes in [1, 64, DelimitedReader.defaultBufferBytes] {
            var position = 0
            let reader = try DelimitedReader(
                reading: { buffer in
                    let count = min(buffer.count, input.count - position)
                    buffer.copyBytes(from: input[position..<position + count])
                    position += count
                    return count
                }, dialect: .csv, plan: plan, batchRows: 1, bufferBytes: bufferBytes)
            XCTAssertEqual(reader.header?.count, width)
            XCTAssertEqual(reader.header?.first, unescaped)
            XCTAssertEqual(reader.header?.last, "c\(width - 1)")
            for _ in 0..<2 {
                XCTAssertTrue(try reader.read())
                XCTAssertEqual(reader.rows, 1)
                XCTAssertEqual(reader.string(0, row: 0), unescaped)
                XCTAssertEqual(reader.verdict(1, row: 0).reason, .malformed)
                XCTAssertEqual(string(reader.raw(1, row: 0)), unescaped)
                XCTAssertEqual(reader.i32(2, row: 0), .success(Int32(width - 1)))
                XCTAssertEqual(reader.string(3, row: 0), "0")
            }
            XCTAssertFalse(try reader.read())
        }

        // And in memory, shown to the core one byte at first: the window widens instead.
        let reader = try DelimitedReader(bytes: input, dialect: .csv, plan: plan, batchRows: 1024, windowBytes: 1)
        XCTAssertEqual(reader.header?.count, width)
        var rows = 0
        while try reader.read() {
            for index in 0..<reader.rows {
                XCTAssertEqual(reader.string(0, row: index), unescaped)
            }
            rows += reader.rows
        }
        XCTAssertEqual(rows, 2)
    }

    func testAStreamIsReadThroughASmallBuffer() throws {
        var next = 0
        var pending = [UInt8]()
        let total = 20_000
        let reader = try DelimitedReader(
            reading: { buffer in
                // A source that produces as it goes, and hands over odd-sized pieces.
                while pending.count < buffer.count && next <= total {
                    pending.append(contentsOf: (next == 0 ? "n\n" : "\(next)\n").utf8)
                    next += 1
                }
                let count = min(buffer.count, pending.count, 7)
                buffer.copyBytes(from: pending[..<count])
                pending.removeFirst(count)
                return count
            }, dialect: .csv, plan: [.i64(0)], batchRows: 512, bufferBytes: 16)
        XCTAssertEqual(reader.header, ["n"])
        var sum: Int64 = 0
        var rows = 0
        while try reader.read() {
            sum += reader.values(0, as: Int64.self).reduce(0, +)
            rows += reader.rows
        }
        XCTAssertEqual(rows, total)
        XCTAssertEqual(sum, Int64(total) * Int64(total + 1) / 2)
        XCTAssertEqual(reader.records, Int64(total + 1))
    }

    func testAStructuralFailureIsThrownAfterTheIntactRows() throws {
        let reader = try DelimitedReader(bytes: Array("a,b\n1,2\n3\n".utf8), dialect: .csv, plan: [.i32(0)])
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(reader.rows, 1)
        XCTAssertEqual(reader.i32(0, row: 0), .success(1))
        let expected = TabularError(kind: .columnCount, record: 2, line: 3, byte: 8, expected: 2, found: 1)
        for _ in 0..<2 {
            XCTAssertThrowsError(try reader.read()) { error in
                XCTAssertEqual(error as? TabularError, expected)
                XCTAssertEqual(
                    "\(error)", "Record 2 (line 3, byte 8) has 1 cells; the first record had 2.")
                XCTAssertEqual(error.localizedDescription, "\(error)")
            }
            XCTAssertEqual(reader.rows, 0)
        }

        // A header that never closes its quote fails the open itself.
        XCTAssertThrowsError(try DelimitedReader(bytes: Array("a,\"b\n1,2\n".utf8), dialect: .csv, plan: [.i32(0)])) {
            XCTAssertEqual(($0 as? TabularError)?.kind, .unclosedQuote)
        }
    }

    func testWhatASourceThrowsComesOutUnchanged() throws {
        struct Unplugged: Error, Equatable {}
        var calls = 0
        let reader = try DelimitedReader(
            reading: { buffer in
                calls += 1
                if calls == 2 { throw Unplugged() }
                let bytes = Array((calls == 1 ? "n\n1\n2" : "\n3\n").utf8)
                guard calls <= 3 else { return 0 }
                buffer.copyBytes(from: bytes)
                return bytes.count
            }, dialect: .csv, plan: [.i32(0)], batchRows: 1)
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(reader.i32(0, row: 0), .success(1))
        XCTAssertThrowsError(try reader.read()) { XCTAssertEqual($0 as? Unplugged, Unplugged()) }
        // Not a structural failure, so not final: nothing was lost, and the read goes on.
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(reader.i32(0, row: 0), .success(2))
        XCTAssertTrue(try reader.read())
        XCTAssertEqual(reader.i32(0, row: 0), .success(3))
        XCTAssertFalse(try reader.read())
    }

    func testAFileIsReadThroughTheSameReader() throws {
        #if os(WASI)
        throw XCTSkip("a WASI sandbox has no file system to write a file into")
        #else
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("hypertabular-\(UUID().uuidString).csv").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        var text = "n\n"
        for row in 0..<100_000 { text += "\(row)\n" }
        try Data(text.utf8).write(to: URL(fileURLWithPath: path))

        let reader = try DelimitedReader(contentsOfFile: path, dialect: .csv, plan: [.i64(0)], bufferBytes: 4096)
        var sum: Int64 = 0
        var rows = 0
        while try reader.read() {
            sum += reader.values(0, as: Int64.self).reduce(0, +)
            rows += reader.rows
        }
        XCTAssertEqual(rows, 100_000)
        XCTAssertEqual(sum, 4_999_950_000)

        XCTAssertThrowsError(
            try DelimitedReader(contentsOfFile: path + ".missing", dialect: .csv, plan: [.i64(0)])
        ) { XCTAssertFalse($0 is TabularError) }
        #endif
    }

    /// The shapes this binding and the core share, as the header declares them.
    func testTheSharedShapes() {
        XCTAssertEqual(MemoryLayout<CellVerdict>.size, 12)
        XCTAssertEqual(MemoryLayout<CellVerdict>.stride, 12)
        XCTAssertEqual(Door.bool.rawValue, 1)
        XCTAssertEqual(Door.excelSerial.rawValue, 22)
        XCTAssertEqual(Column.text(3).valueSize, 8)
        XCTAssertEqual(Column.unix(3, precision: .nanoseconds).spec.param, 4)
        XCTAssertEqual(Column.date(3, order: .dayMonthYear).spec.door, 20)
        XCTAssertEqual(Column.date(3, order: .dayMonthYear).spec.param, 3)
        XCTAssertEqual(Column.excelSerial(3, epoch: .y1904).spec.param, 2)
        XCTAssertEqual(Dialect.tsv.separator, "\t")
        XCTAssertEqual(Dialect.psv, Dialect(separator: "|", quoting: true, hasHeader: true, skipBlankLines: true))
    }
}
