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
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.rows, 3)

        XCTAssertEqual(Array(batch.values(0, as: Int32.self)), [1, 2, 3])
        XCTAssertEqual(Array(batch.values(2, as: Double.self)), [2.5, 0.0, 7.0])
        XCTAssertEqual(batch.verdicts(2).map(\.isOk), [true, false, true])
        XCTAssertEqual(batch.verdicts(2)[1].reason, .malformed)
        XCTAssertEqual(batch.verdict(2, row: 1).fault, Fault(reason: .malformed, offset: 0, length: 1))

        let described =
            switch batch.get(2, row: 1, as: Double.self) {
            case .success(let score): "\(score)"
            case .fault(let fault): "\(fault.reason) in \"\(string(batch.raw(2, row: 1)))\""
            }
        XCTAssertEqual(described, "malformed in \"x\"")
        XCTAssertEqual(batch.string(1, row: 1), "bob, jr")
        XCTAssertNil(batch.string(1, row: 2))
        XCTAssertEqual(batch.verdict(1, row: 2).reason, .empty)
        XCTAssertNil(try reader.read())
        XCTAssertEqual(batch.rows, 0)
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
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.rows, 1)
        func raw(_ column: Int) -> UnsafeRawBufferPointer { batch.raw(column, row: 0) }

        XCTAssertEqual(batch.get(0, row: 0, as: Bool.self), .success(true))
        XCTAssertEqual(batch.get(0, row: 0, as: Bool.self), try Cast.bool(raw(0)))
        XCTAssertEqual(batch.get(1, row: 0, as: Int8.self), .success(-128))
        XCTAssertEqual(batch.get(1, row: 0, as: Int8.self), try Cast.i8(raw(1), format: .invariant))
        XCTAssertEqual(batch.get(2, row: 0, as: Int16.self), .success(32767))
        XCTAssertEqual(batch.get(2, row: 0, as: Int16.self), try Cast.i16(raw(2), format: .invariant))
        XCTAssertEqual(batch.get(3, row: 0, as: Int32.self), .success(-1234))
        XCTAssertEqual(batch.get(3, row: 0, as: Int32.self), try Cast.i32(raw(3), format: .invariant))
        XCTAssertEqual(batch.get(4, row: 0, as: Int64.self), .success(.max))
        XCTAssertEqual(batch.get(4, row: 0, as: Int64.self), try Cast.i64(raw(4), format: .invariant))
        XCTAssertEqual(batch.get(5, row: 0, as: UInt8.self), .success(255))
        XCTAssertEqual(batch.get(5, row: 0, as: UInt8.self), try Cast.u8(raw(5), format: .invariant))
        XCTAssertEqual(batch.get(6, row: 0, as: UInt16.self), .success(65535))
        XCTAssertEqual(batch.get(6, row: 0, as: UInt16.self), try Cast.u16(raw(6), format: .invariant))
        XCTAssertEqual(batch.get(7, row: 0, as: UInt32.self), .success(4_294_967_295))
        XCTAssertEqual(batch.get(7, row: 0, as: UInt32.self), try Cast.u32(raw(7), format: .invariant))
        XCTAssertEqual(batch.get(8, row: 0, as: UInt64.self), .success(.max))
        XCTAssertEqual(batch.get(8, row: 0, as: UInt64.self), try Cast.u64(raw(8), format: .invariant))
        XCTAssertEqual(batch.get(9, row: 0, as: Float.self), .success(2.5))
        XCTAssertEqual(batch.get(9, row: 0, as: Float.self), try Cast.f32(raw(9), format: .invariant))
        XCTAssertEqual(batch.get(10, row: 0, as: Double.self), .success(0.255))
        XCTAssertEqual(batch.get(10, row: 0, as: Double.self), try Cast.f64(raw(10), format: .invariant))
        // 96 bits of magnitude: the high word is in play, and nothing is rounded.
        XCTAssertEqual(
            batch.get(11, row: 0, as: Decimal.self), .success(Decimal(string: "79228162514264337593543950.335")!))
        XCTAssertEqual(batch.get(11, row: 0, as: Decimal.self), try Cast.decimal(raw(11), format: .invariant))
        XCTAssertEqual(
            batch.get(12, row: 0, as: UUID.self), .success(UUID(uuidString: "6ba7b810-9dad-11d1-80b4-00c04fd430c8")!))
        XCTAssertEqual(batch.get(12, row: 0, as: UUID.self), try Cast.uuid(raw(12)))
        XCTAssertEqual(batch.get(13, row: 0, as: Date.self), .success(Date(timeIntervalSince1970: 1_767_348_245.25)))
        XCTAssertEqual(batch.get(13, row: 0, as: Date.self), try Cast.timestamp(raw(13)))
        XCTAssertEqual(batch.get(14, row: 0, as: Date.self), .success(Date(timeIntervalSince1970: 1_700_000_000.123)))
        XCTAssertEqual(batch.get(14, row: 0, as: Date.self), try Cast.unix(raw(14), precision: .milliseconds))
        XCTAssertEqual(batch.get(15, row: 0, as: Date.self), .success(Date(timeIntervalSince1970: 1_704_132_000)))
        XCTAssertEqual(batch.get(15, row: 0, as: Date.self), try Cast.excelSerial(raw(15), epoch: .y1900))
        XCTAssertEqual(
            batch.get(16, row: 0, as: DateComponents.self), .success(DateComponents(year: 2024, month: 1, day: 31)))
        XCTAssertEqual(batch.get(16, row: 0, as: DateComponents.self), try Cast.date(raw(16)))
        XCTAssertEqual(
            batch.get(17, row: 0, as: DateComponents.self), .success(DateComponents(year: 2026, month: 1, day: 7)))
        XCTAssertEqual(batch.get(17, row: 0, as: DateComponents.self), try Cast.date(raw(17), order: .monthDayYear))
        XCTAssertEqual(
            batch.get(18, row: 0, as: DateComponents.self),
            .success(DateComponents(year: 2026, month: 1, day: 7, hour: 15, minute: 4, second: 5, nanosecond: 7)))
        XCTAssertEqual(batch.get(18, row: 0, as: DateComponents.self), try Cast.dateTime(raw(18), order: .monthDayYear))
        XCTAssertEqual(
            batch.get(19, row: 0, as: DateComponents.self),
            .success(DateComponents(hour: 15, minute: 4, second: 5, nanosecond: 500_000_000)))
        XCTAssertEqual(batch.get(19, row: 0, as: DateComponents.self), try Cast.time(raw(19)))
        XCTAssertEqual(batch.get(20, row: 0, as: Duration.self), .success(.seconds(-5400) - .nanoseconds(1)))
        XCTAssertEqual(batch.get(20, row: 0, as: Duration.self), try Cast.duration(raw(20)))
        XCTAssertEqual(batch.string(21, row: 0), "a \"quoted\" cell")
        XCTAssertEqual(string(raw(21)), "a \"quoted\" cell")
        XCTAssertNil(try reader.read())
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
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.rows, 1)
        let empty = Fault(reason: .empty, offset: 0, length: 0)
        for column in plan.indices {
            XCTAssertEqual(batch.verdict(column, row: 0).fault, empty, "column \(column)")
            XCTAssertEqual(batch.raw(column, row: 0).count, 0, "column \(column)")
        }
        XCTAssertEqual(batch.get(0, row: 0, as: Bool.self), .fault(empty))
        XCTAssertEqual(batch.get(11, row: 0, as: Decimal.self), .fault(empty))
        XCTAssertEqual(batch.get(12, row: 0, as: UUID.self), .fault(empty))
        XCTAssertEqual(batch.get(15, row: 0, as: Date.self), .fault(empty))
        XCTAssertEqual(batch.get(17, row: 0, as: DateComponents.self), .fault(empty))
        XCTAssertEqual(batch.get(18, row: 0, as: DateComponents.self), .fault(empty))
        XCTAssertEqual(batch.get(19, row: 0, as: DateComponents.self), .fault(empty))
        XCTAssertEqual(batch.get(20, row: 0, as: Duration.self), .fault(empty))
        XCTAssertNil(batch.text(21, row: 0))
        XCTAssertEqual(Array(batch.values(0, as: Bool.self)), [false])
        XCTAssertEqual(Array(batch.values(8, as: UInt64.self)), [0])
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
        XCTAssertEqual(reader.plan[0].format, euro)
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.get(0, row: 0, as: Decimal.self), .success(Decimal(string: "1234.5")!))
        XCTAssertEqual(batch.get(0, row: 0, as: Decimal.self), try Cast.decimal(batch.raw(0, row: 0), format: euro))
        XCTAssertEqual(batch.get(1, row: 0, as: Int32.self), .success(7))
        XCTAssertEqual(batch.get(2, row: 0, as: Int32.self), try Cast.i32(batch.raw(2, row: 0), format: wide))
        XCTAssertEqual(batch.verdict(2, row: 0).reason, .malformed)
        // The same source column through the invariant notation is a different verdict.
        XCTAssertEqual(batch.get(3, row: 0, as: Double.self), try Cast.f64(batch.raw(3, row: 0), format: .invariant))
        XCTAssertEqual(batch.verdict(3, row: 0).reason, .malformed)
    }

    /// Text is a view of the input wherever the cell had no escaped quote in it; only a
    /// cell with `""` inside is unescaped, into the reader's arena.
    func testTextIsAViewOfTheInput() throws {
        let input = Array("name,note\nalice,\"a, b\"\nbob,\"say \"\"hi\"\"\"\n".utf8)
        try input.withUnsafeBytes { bytes in
            let reader = try DelimitedReader(bytesNoCopy: bytes, dialect: .csv, plan: [.text(0), .text(1), .i32(1)])
            let batch = try XCTUnwrap(reader.read())
            XCTAssertEqual(batch.rows, 2)
            func inside(_ text: UnsafeRawBufferPointer?) -> Bool {
                guard let text, let from = text.baseAddress, let base = bytes.baseAddress else { return false }
                return from >= base && from + text.count <= base + bytes.count
            }
            XCTAssertTrue(inside(batch.text(0, row: 0)))
            XCTAssertEqual(batch.string(0, row: 0), "alice")
            XCTAssertTrue(inside(batch.text(1, row: 0)), "quoted, nothing escaped: still the input")
            XCTAssertEqual(batch.string(1, row: 0), "a, b")
            XCTAssertTrue(inside(batch.raw(2, row: 0)))
            XCTAssertFalse(inside(batch.text(1, row: 1)), "an escaped cell is unescaped elsewhere")
            XCTAssertEqual(batch.string(1, row: 1), "say \"hi\"")
            // The raw text of a cell that failed to cast, escaped quotes resolved.
            XCTAssertEqual(
                batch.get(2, row: 1, as: Int32.self), .fault(Fault(reason: .malformed, offset: 0, length: 1)))
            XCTAssertEqual(string(batch.raw(2, row: 1)), "say \"hi\"")
            XCTAssertNil(try reader.read())
        }
    }

    func testAPlanIsAProjection() throws {
        let input = Array("a,b,c\n1,2,3\n4,5,6\n".utf8)
        // In its own order, one source column twice, and one past the record's last cell.
        let reader = try DelimitedReader(bytes: input, dialect: .csv, plan: [.i32(2), .text(0), .u8(2), .i32(7)])
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(Array(batch.values(0, as: Int32.self)), [3, 6])
        XCTAssertEqual(Array(batch.values(2, as: UInt8.self)), [3, 6])
        XCTAssertEqual(batch.string(1, row: 1), "4")
        XCTAssertEqual(batch.get(3, row: 0, as: Int32.self), .fault(Fault(reason: .empty, offset: 0, length: 0)))
        XCTAssertEqual(batch.raw(3, row: 0).count, 0)

        // No columns at all still counts the rows.
        let counter = try DelimitedReader(bytes: input, dialect: .csv, plan: [], batchRows: 1)
        var rows = 0
        while let batch = try counter.read() { rows += batch.rows }
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
                let batch = try XCTUnwrap(reader.read())
                XCTAssertEqual(batch.rows, 1)
                XCTAssertEqual(batch.string(0, row: 0), unescaped)
                XCTAssertEqual(batch.verdict(1, row: 0).reason, .malformed)
                XCTAssertEqual(string(batch.raw(1, row: 0)), unescaped)
                XCTAssertEqual(batch.get(2, row: 0, as: Int32.self), .success(Int32(width - 1)))
                XCTAssertEqual(batch.string(3, row: 0), "0")
            }
            XCTAssertNil(try reader.read())
        }

        // And in memory, shown to the core one byte at first: the window widens instead.
        let reader = try DelimitedReader(bytes: input, dialect: .csv, plan: plan, batchRows: 1024, windowBytes: 1)
        XCTAssertEqual(reader.header?.count, width)
        var rows = 0
        while let batch = try reader.read() {
            for index in 0..<batch.rows {
                XCTAssertEqual(batch.string(0, row: index), unescaped)
            }
            rows += batch.rows
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
        while let batch = try reader.read() {
            sum += batch.values(0, as: Int64.self).reduce(0, +)
            rows += batch.rows
        }
        XCTAssertEqual(rows, total)
        XCTAssertEqual(sum, Int64(total) * Int64(total + 1) / 2)
        XCTAssertEqual(reader.records, Int64(total + 1))
    }

    func testAStructuralFailureIsThrownAfterTheIntactRows() throws {
        let reader = try DelimitedReader(bytes: Array("a,b\n1,2\n3\n".utf8), dialect: .csv, plan: [.i32(0)])
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.rows, 1)
        XCTAssertEqual(batch.get(0, row: 0, as: Int32.self), .success(1))
        let expected = TabularError(kind: .columnCount, record: 2, line: 3, byte: 8, expected: 2, found: 1)
        for _ in 0..<2 {
            XCTAssertThrowsError(try reader.read()) { error in
                XCTAssertEqual(error as? TabularError, expected)
                XCTAssertEqual(
                    "\(error)", "Record 2 (line 3, byte 8) has 1 cells; the first record had 2.")
                XCTAssertEqual(error.localizedDescription, "\(error)")
            }
            XCTAssertEqual(batch.rows, 0)
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
        var batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.get(0, row: 0, as: Int32.self), .success(1))
        XCTAssertThrowsError(try reader.read()) { XCTAssertEqual($0 as? Unplugged, Unplugged()) }
        // Not a structural failure, so not final: nothing was lost, and the read goes on.
        batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.get(0, row: 0, as: Int32.self), .success(2))
        batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.get(0, row: 0, as: Int32.self), .success(3))
        XCTAssertNil(try reader.read())
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
            while let batch = try reader.read() {
                sum += batch.values(0, as: Int64.self).reduce(0, +)
                rows += batch.rows
            }
            XCTAssertEqual(rows, 100_000)
            XCTAssertEqual(sum, 4_999_950_000)

            XCTAssertThrowsError(
                try DelimitedReader(contentsOfFile: path + ".missing", dialect: .csv, plan: [.i64(0)])
            ) { XCTAssertFalse($0 is TabularError) }
        #endif
    }

    /// The core ends a batch early when its arena fills; the batch after one that did starts
    /// with the arena doubled, so twenty thousand escaped rows are a handful of batches.
    func testAnArenaThatCrampsABatchIsGrown() throws {
        let row = "\"" + String(repeating: "say \"\"hi\"\" ", count: 8) + "\"\n"
        let expected = String(repeating: "say \"hi\" ", count: 8)
        let reader = try DelimitedReader(
            bytes: Array(String(repeating: row, count: 20_000).utf8),
            dialect: Dialect(separator: ",", hasHeader: false),
            plan: [.text(0)])
        var batches = 0
        var rows = 0
        while let batch = try reader.read() {
            batches += 1
            rows += batch.rows
            XCTAssertEqual(batch.get(0, row: batch.rows - 1, as: String.self), .success(expected))
        }
        XCTAssertEqual(rows, 20_000)
        XCTAssertLessThan(batches, 15)
    }

    // MARK: - header first

    private static let regions = Array(
        """
        Region Code,Region Name,M49 Code,Region Code
        002,Africa,4,dup
        019,Americas,8,dup
        142,Asia,12,dup
        150,Europe,20,dup
        009,Oceania,24,dup

        """.utf8)

    func testAPlanIsBuiltFromTheHeaderAndBoundOnce() throws {
        let reader = try DelimitedReader(bytes: Self.regions, dialect: .csv)
        XCTAssertFalse(reader.isBound)
        XCTAssertEqual(reader.plan, [])
        XCTAssertEqual(reader.columnCount, 4)
        // Reading before a plan is an error, and not one that sticks.
        for _ in 0..<2 {
            XCTAssertThrowsError(try reader.read()) { XCTAssertEqual($0 as? PlanError, .unbound) }
        }

        let header = try XCTUnwrap(reader.header)
        XCTAssertEqual(header, ["Region Code", "Region Name", "M49 Code", "Region Code"])
        XCTAssertEqual(header.count, 4)
        XCTAssertEqual(header[1], "Region Name")
        // The first of two columns with one name; a string and its bytes find the same.
        XCTAssertEqual(try header.ordinal(of: "Region Code"), 0)
        XCTAssertEqual(try header.ordinal(of: Array("M49 Code".utf8)), 2)
        XCTAssertEqual(header.firstIndex(of: "M49 Code".utf8), 2)
        XCTAssertNil(header.firstIndex(of: "m49 code"), "exact, case included")
        XCTAssertNil(header.firstIndex(of: " M49 Code"), "exact, spaces included")
        XCTAssertThrowsError(try header.ordinal(of: "Country")) { error in
            XCTAssertEqual(error as? NoSuchColumn, NoSuchColumn(name: "Country"))
            XCTAssertEqual(error.localizedDescription, "The header has no column named \"Country\".")
        }

        let plan: [Column] = [.i32(try header.ordinal(of: "M49 Code")), .text(try header.ordinal(of: "Region Name"))]
        try reader.bind(plan)
        XCTAssertTrue(reader.isBound)
        XCTAssertEqual(reader.plan, plan)
        XCTAssertThrowsError(try reader.bind(plan)) { XCTAssertEqual($0 as? PlanError, .alreadyBound) }

        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(Array(batch.values(0, as: Int32.self)), [4, 8, 12, 20, 24])
        XCTAssertEqual(batch.string(1, row: 4), "Oceania")
        XCTAssertNil(try reader.read())
        XCTAssertThrowsError(try reader.bind(plan)) { XCTAssertEqual($0 as? PlanError, .alreadyBound) }
    }

    /// A name is matched as the bytes the file holds: no Unicode equivalence, which `String`'s
    /// own `==` would apply.
    func testANameIsMatchedByItsBytes() throws {
        let composed = "caf\u{E9}"
        let decomposed = "cafe\u{301}"
        XCTAssertEqual(composed, decomposed, "String's == is canonical equivalence")
        let reader = try DelimitedReader(bytes: Array("\(decomposed),\(composed)\n1,2\n".utf8), dialect: .csv)
        let header = try XCTUnwrap(reader.header)
        XCTAssertEqual(try header.ordinal(of: composed), 1)
        XCTAssertEqual(try header.ordinal(of: decomposed), 0)
        XCTAssertEqual(header.bytes(at: 1), Array(composed.utf8))
    }

    func testAHeaderlessSourceIsBoundByPosition() throws {
        var dialect = Dialect.csv
        dialect.hasHeader = false
        let reader = try DelimitedReader(bytes: Array("1,a\n2,b\n".utf8), dialect: dialect)
        XCTAssertNil(reader.header)
        XCTAssertNil(reader.columnCount, "no record has been read")
        try reader.bind([.text(1), .i64(0)])
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(Array(batch.values(1, as: Int64.self)), [1, 2])
        XCTAssertEqual(reader.columnCount, 2)
    }

    func testEveryOpeningHasAPlanlessForm() throws {
        let expected: Header = ["Region Code", "Region Name", "M49 Code", "Region Code"]
        var position = 0
        let readers = [
            try DelimitedReader(bytes: Self.regions, dialect: .csv, batchRows: 2),
            try Self.regions.withUnsafeBytes { try DelimitedReader(bytesNoCopy: $0, dialect: .csv, batchRows: 2) },
            try DelimitedReader(
                reading: { buffer in
                    // Three bytes at a time.
                    let count = min(3, buffer.count, Self.regions.count - position)
                    buffer.copyBytes(from: Self.regions[position..<position + count])
                    position += count
                    return count
                }, dialect: .csv, batchRows: 2, bufferBytes: 4),
        ]
        for reader in readers {
            XCTAssertEqual(reader.header, expected)
            try reader.bind([.i32(2)])
            var sum: Int32 = 0
            try reader.forEachRow { row in
                if case .success(let value) = row.get(0, as: Int32.self) { sum += value }
            }
            XCTAssertEqual(sum, 68)
        }
    }

    // MARK: - rows

    func testARowIsItsBatchReadAcross() throws {
        let plan: [Column] = [.i32(2), .text(1), .i32(1), .text(3)]
        let reader = try DelimitedReader(bytes: Self.regions, dialect: .csv, plan: plan)
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.count, batch.rows)
        var seen = 0
        for (index, row) in batch.enumerated() {
            XCTAssertEqual(row.index, index)
            XCTAssertEqual(row.line, batch.line(index))
            XCTAssertEqual(row.get(0, as: Int32.self), batch.get(0, row: index, as: Int32.self))
            XCTAssertEqual(row.get(2, as: Int32.self), batch.get(2, row: index, as: Int32.self))
            XCTAssertEqual(row.verdict(2), batch.verdicts(2)[index])
            XCTAssertEqual(row.text(1).map(Array.init), batch.text(1, row: index).map(Array.init))
            XCTAssertEqual(row.string(1), batch.string(1, row: index))
            XCTAssertEqual(Array(row.raw(2)), Array(batch.raw(2, row: index)))
            seen += 1
        }
        XCTAssertEqual(seen, 5)
        XCTAssertEqual(batch.last?.index, 4)
        XCTAssertEqual(batch[1].string(1), "Americas")
        XCTAssertEqual(batch[1].line, 3)
    }

    func testForEachRowReadsAcrossBatchesToTheEnd() throws {
        var text = "n\n"
        for row in 1...7 { text += "\(row)\n" }
        let reader = try DelimitedReader(bytes: Array(text.utf8), dialect: .csv, plan: [.i64(0)], batchRows: 2)
        var values = [Int64]()
        var lines = [Int]()
        try reader.forEachRow { row in
            if case .success(let value) = row.get(0, as: Int64.self) { values.append(value) }
            lines.append(row.line)
        }
        // Seven rows in batches of two: four batches.
        XCTAssertEqual(values, [1, 2, 3, 4, 5, 6, 7])
        XCTAssertEqual(lines, Array(2...8))
        XCTAssertNil(try reader.read())

        struct Enough: Error {}
        let again = try DelimitedReader(bytes: Array(text.utf8), dialect: .csv, plan: [.i64(0)], batchRows: 2)
        var visited = 0
        XCTAssertThrowsError(
            try again.forEachRow { _ in
                visited += 1
                if visited == 3 { throw Enough() }
            }
        ) { XCTAssertTrue($0 is Enough) }
        XCTAssertEqual(visited, 3)
    }

    // MARK: - asynchronous

    func testAnAsyncSequenceIsReadAsItArrives() async throws {
        let total = 5_000
        var text = "n\n"
        for row in 1...total { text += "\(row)\n" }
        let reader = try await DelimitedReader(
            bytes: CorpusTests.ShortReads(Array(text.utf8), chunk: 3), dialect: .csv, batchRows: 256, bufferBytes: 16)
        XCTAssertEqual(reader.header, ["n"])
        try reader.bind([.i64(try XCTUnwrap(reader.header).ordinal(of: "n"))])
        var sum: Int64 = 0
        var rows = 0
        while let batch = try await reader.readAsync() {
            sum += batch.values(0, as: Int64.self).reduce(0, +)
            rows += batch.rows
        }
        XCTAssertEqual(rows, total)
        XCTAssertEqual(sum, Int64(total) * Int64(total + 1) / 2)

        // A reader of memory reads the same asynchronously, and never needs to wait.
        let memory = try DelimitedReader(bytes: Array(text.utf8), dialect: .csv, plan: [.i64(0)])
        var memoryRows = 0
        while let batch = try await memory.readAsync() { memoryRows += batch.rows }
        XCTAssertEqual(memoryRows, total)
    }

    /// A read the sequence's cancellation interrupts loses nothing: every byte that arrived
    /// stays buffered, and the next read goes on from it.
    func testACancelledReadIsResumable() async throws {
        let total = 2_000
        var text = "n\n"
        for row in 1...total { text += "\(row)\n" }
        let bytes = Array(text.utf8)
        // Past the first 64 bytes, which the opening awaits for the header.
        for failAt in [64, 100, 1_000, bytes.count - 1] {
            let reader = try await DelimitedReader(
                bytes: CorpusTests.ShortReads(bytes, chunk: 5, failAt: failAt), dialect: .csv, plan: [.i64(0)],
                batchRows: 100, bufferBytes: 64
            )
            var sum: Int64 = 0
            var rows = 0
            var cancelled = 0
            while true {
                do {
                    guard let batch = try await reader.readAsync() else { break }
                    sum += batch.values(0, as: Int64.self).reduce(0, +)
                    rows += batch.rows
                } catch is CancellationError {
                    cancelled += 1
                }
            }
            XCTAssertEqual(cancelled, 1, "failAt \(failAt)")
            XCTAssertEqual(rows, total, "failAt \(failAt)")
            XCTAssertEqual(sum, Int64(total) * Int64(total + 1) / 2, "failAt \(failAt)")
        }
    }

    /// A cancelled task's read throws before it reads anything; the reader is left as it was.
    func testACancelledTaskReadsNothing() async throws {
        final class Held: @unchecked Sendable {
            let reader: DelimitedReader
            init(_ reader: DelimitedReader) { self.reader = reader }
        }
        let held = Held(try DelimitedReader(bytes: Self.orders, dialect: .csv, plan: [.i32(0)]))
        let task = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await held.reader.readAsync()?.rows
        }
        do {
            _ = try await task.value
            XCTFail("a cancelled task read")
        } catch {
            XCTAssertTrue(error is CancellationError)
        }
        let read = try await held.reader.readAsync()
        let batch = try XCTUnwrap(read)
        XCTAssertEqual(Array(batch.values(0, as: Int32.self)), [1, 2, 3])

        let orders = CorpusTests.ShortReads(Self.orders, chunk: 4)
        let opening = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            let reader = try await DelimitedReader(bytes: orders, dialect: .csv)
            return reader.header?.count
        }
        do {
            _ = try await opening.value
            XCTFail("a cancelled task opened a reader")
        } catch {
            XCTAssertTrue(error is CancellationError)
        }
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
