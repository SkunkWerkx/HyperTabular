import Foundation
import XCTest
@testable import HyperTabular

/// Replays the shared conformance corpus (`corpus/delimited.json` at the repository root) —
/// the same file the Rust and C# bindings replay — through this binding, more than one
/// way: from memory copied and in place, from memory shown to the core a few bytes at a
/// time, from a stream read through buffers too small for a record, and from a file; in
/// batches of one row, of two, and of many. How the input is cut up is the binding's
/// business and must not change the answer.
final class CorpusTests: XCTestCase {
    /// `nil` only under WASI, where the test module runs sandboxed with no view of the
    /// source tree; the corpus is skipped there, and the reader is exercised by
    /// `ReaderTests` and by swift/StaticSmokeTest instead.
    static let corpusDirectory: URL? = {
        #if os(WASI)
            return nil
        #else
            // A device run (Android, through adb) has the corpus pushed beside the test
            // bundle, nowhere above the host path #filePath names.
            if let pushed = ProcessInfo.processInfo.environment["HYPERTABULAR_CORPUS"] {
                return URL(fileURLWithPath: pushed)
            }
            var dir = URL(fileURLWithPath: #filePath)
            while dir.path != "/" {
                let candidate = dir.appendingPathComponent("corpus")
                if FileManager.default.fileExists(atPath: candidate.appendingPathComponent("delimited.json").path) {
                    return candidate
                }
                dir.deleteLastPathComponent()
            }
            fatalError("corpus directory not found above \(#filePath)")
        #endif
    }()

    // MARK: - the corpus file's shapes

    // Decoded with JSONDecoder, not JSONSerialization: on Linux the latter drops a U+FEFF
    // that begins a string, which would quietly turn every byte-order-mark case into one
    // without a mark — and the positions a failure reports into the wrong ones.
    struct Case: Decodable {
        let name: String
        let input: String
        let dialect: Declared
        let plan: [Entry]
        let header: [String]?
        let rows: [[Cell]]
        let failure: Failure?
    }

    struct Declared: Decodable {
        let separator: String
        let quoting: Bool
        let has_header: Bool
        let skip_blank_lines: Bool
    }

    struct Entry: Decodable {
        let door: String
        let ordinal: Int
        let precision: UInt32?
        let order: UInt32?
        let epoch: UInt32?
        let format: Notation?
    }

    struct Notation: Decodable {
        let decimal_sep: String
        let group_sep: String
        let flags: UInt32
        let currency: String?
    }

    struct Failure: Decodable {
        let kind: String
        let record: Int64
        let line: Int
        let byte: Int64
        let expected: Int?
        let found: Int?
    }

    /// A cell in HyperCast's corpus shape: the verdict, then whichever fields its door has.
    struct Cell: Decodable {
        let expect: String
        let fault: [Int]?
        let raw: String?
        let text: String?
        let value: Value?
        let magnitude: String?
        let negative: Bool?
        let scale: Int?
        let seconds: Int64?
        let nanos: Int64?
        let nanos_of_day: UInt64?
        let year: Int?
        let month: Int?
        let day: Int?
    }

    /// `value` is a boolean, an integer anywhere from i64's minimum to u64's maximum, a
    /// real, or a string, as the door has it.
    enum Value: Decodable {
        case bool(Bool)
        case signed(Int64)
        case unsigned(UInt64)
        case real(Double)
        case string(String)

        init(from decoder: Decoder) throws {
            let container = try decoder.singleValueContainer()
            if let value = try? container.decode(Bool.self) {
                self = .bool(value)
            } else if let value = try? container.decode(Int64.self) {
                self = .signed(value)
            } else if let value = try? container.decode(UInt64.self) {
                self = .unsigned(value)
            } else if let value = try? container.decode(Double.self) {
                self = .real(value)
            } else {
                self = .string(try container.decode(String.self))
            }
        }

        var bool: Bool {
            guard case .bool(let value) = self else { fatalError("not a boolean: \(self)") }
            return value
        }

        var signed: Int64 {
            guard case .signed(let value) = self else { fatalError("not a signed integer: \(self)") }
            return value
        }

        var unsigned: UInt64 {
            switch self {
            case .unsigned(let value): return value
            case .signed(let value): return UInt64(value)
            default: fatalError("not an unsigned integer: \(self)")
            }
        }

        var real: Double {
            switch self {
            case .real(let value): return value
            case .signed(let value): return Double(value)
            case .unsigned(let value): return Double(value)
            default: fatalError("not a real: \(self)")
            }
        }

        var string: String {
            guard case .string(let value) = self else { fatalError("not a string: \(self)") }
            return value
        }
    }

    func corpus() throws -> [Case] {
        guard let directory = Self.corpusDirectory else {
            throw XCTSkip("the conformance corpus is not reachable from a WASI sandbox")
        }
        let data = try Data(contentsOf: directory.appendingPathComponent("delimited.json"))
        return try JSONDecoder().decode([Case].self, from: data)
    }

    // MARK: - the case, as this binding's types

    func dialect(of vector: Case) -> Dialect {
        Dialect(
            separator: vector.dialect.separator.unicodeScalars.first!,
            quoting: vector.dialect.quoting,
            hasHeader: vector.dialect.has_header,
            skipBlankLines: vector.dialect.skip_blank_lines)
    }

    func format(of entry: Entry) -> NumFormat {
        guard let format = entry.format else { return .invariant }
        return NumFormat(
            decimalSeparator: format.decimal_sep.unicodeScalars.first!,
            groupSeparator: format.group_sep.unicodeScalars.first!,
            styles: NumStyles(rawValue: format.flags),
            currencySymbol: format.currency ?? "")
    }

    func column(of entry: Entry) -> Column {
        let ordinal = entry.ordinal
        let format = format(of: entry)
        switch entry.door {
        case "bool": return .bool(ordinal)
        case "i8": return .i8(ordinal, format: format)
        case "i16": return .i16(ordinal, format: format)
        case "i32": return .i32(ordinal, format: format)
        case "i64": return .i64(ordinal, format: format)
        case "u8": return .u8(ordinal, format: format)
        case "u16": return .u16(ordinal, format: format)
        case "u32": return .u32(ordinal, format: format)
        case "u64": return .u64(ordinal, format: format)
        case "f32": return .f32(ordinal, format: format)
        case "f64": return .f64(ordinal, format: format)
        case "decimal": return .decimal(ordinal, format: format)
        case "uuid": return .uuid(ordinal)
        case "timestamp": return .timestamp(ordinal)
        case "unix": return .unix(ordinal, precision: UnixPrecision(rawValue: entry.precision!)!)
        case "excel_serial": return .excelSerial(ordinal, epoch: ExcelEpoch(rawValue: entry.epoch!)!)
        case "date": return .date(ordinal)
        case "date_ordered": return .date(ordinal, order: DateOrder(rawValue: entry.order!)!)
        case "datetime": return .dateTime(ordinal, order: DateOrder(rawValue: entry.order!)!)
        case "time": return .time(ordinal)
        case "duration": return .duration(ordinal)
        case "text": return .text(ordinal)
        case let other: fatalError("unknown door \(other)")
        }
    }

    // MARK: - one cell

    /// A typed cell with its type set aside, so that one comparison serves every door.
    func erased<T: Hashable>(_ verdict: Verdict<T>) -> Verdict<AnyHashable> {
        switch verdict {
        case .success(let value): .success(AnyHashable(value))
        case .fault(let fault): .fault(fault)
        }
    }

    /// The cell through ``Batch/get(_:row:as:)``, as the type its door presents.
    func typed(_ batch: Batch, _ column: Int, _ row: Int) -> Verdict<AnyHashable> {
        switch batch.columns[column].door {
        case .bool: erased(batch.get(column, row: row, as: Bool.self))
        case .i8: erased(batch.get(column, row: row, as: Int8.self))
        case .i16: erased(batch.get(column, row: row, as: Int16.self))
        case .i32: erased(batch.get(column, row: row, as: Int32.self))
        case .i64: erased(batch.get(column, row: row, as: Int64.self))
        case .u8: erased(batch.get(column, row: row, as: UInt8.self))
        case .u16: erased(batch.get(column, row: row, as: UInt16.self))
        case .u32: erased(batch.get(column, row: row, as: UInt32.self))
        case .u64: erased(batch.get(column, row: row, as: UInt64.self))
        case .f32: erased(batch.get(column, row: row, as: Float.self))
        case .f64: erased(batch.get(column, row: row, as: Double.self))
        case .decimal: erased(batch.get(column, row: row, as: Decimal.self))
        case .uuid: erased(batch.get(column, row: row, as: UUID.self))
        case .timestamp, .unix, .excelSerial: erased(batch.get(column, row: row, as: Date.self))
        case .date, .dateOrdered, .dateTime, .time: erased(batch.get(column, row: row, as: DateComponents.self))
        case .duration: erased(batch.get(column, row: row, as: Duration.self))
        case .text: erased(batch.get(column, row: row, as: String.self))
        }
    }

    /// The value the corpus states for an `ok` cell, as the Swift type the door presents.
    func expectedValue(_ door: Door, _ cell: Cell) -> AnyHashable {
        func instant() -> Date {
            Date(timeIntervalSince1970: Double(cell.seconds!) + Double(cell.nanos!) / 1_000_000_000)
        }
        func day() -> DateComponents {
            DateComponents(year: cell.year!, month: cell.month!, day: cell.day!)
        }
        switch door {
        case .bool: return cell.value!.bool
        case .i8: return Int8(cell.value!.signed)
        case .i16: return Int16(cell.value!.signed)
        case .i32: return Int32(cell.value!.signed)
        case .i64: return cell.value!.signed
        case .u8: return UInt8(cell.value!.unsigned)
        case .u16: return UInt16(cell.value!.unsigned)
        case .u32: return UInt32(cell.value!.unsigned)
        case .u64: return cell.value!.unsigned
        case .f32: return Float(cell.value!.real)
        case .f64: return cell.value!.real
        case .decimal:
            // The raw triple is the contract; Decimal's own initializer is exact for it.
            return Decimal(
                sign: cell.negative! ? .minus : .plus, exponent: -cell.scale!,
                significand: Decimal(string: cell.magnitude!)!)
        case .uuid:
            let hex = cell.value!.string
            var bytes = [UInt8]()
            var index = hex.startIndex
            while index < hex.endIndex {
                let next = hex.index(index, offsetBy: 2)
                bytes.append(UInt8(hex[index..<next], radix: 16)!)
                index = next
            }
            return bytes.withUnsafeBytes { UUID(uuid: $0.load(as: uuid_t.self)) }
        case .timestamp, .unix, .excelSerial: return instant()
        case .date, .dateOrdered: return day()
        case .dateTime:
            let nanos = cell.nanos_of_day!
            let secondOfDay = nanos / 1_000_000_000
            var civil = day()
            civil.hour = Int(secondOfDay / 3_600)
            civil.minute = Int(secondOfDay % 3_600 / 60)
            civil.second = Int(secondOfDay % 60)
            civil.nanosecond = Int(nanos % 1_000_000_000)
            return civil
        case .time:
            let (secondOfDay, nano) = UInt64(cell.nanos!).quotientAndRemainder(dividingBy: 1_000_000_000)
            let (hour, rest) = secondOfDay.quotientAndRemainder(dividingBy: 3_600)
            let (minute, second) = rest.quotientAndRemainder(dividingBy: 60)
            return DateComponents(hour: Int(hour), minute: Int(minute), second: Int(second), nanosecond: Int(nano))
        case .duration:
            return Duration.seconds(cell.seconds!) + .nanoseconds(cell.nanos!)
        case .text:
            return cell.text!
        }
    }

    /// Holds one cell of the batch in hand to what the corpus says of it.
    func assertCell(
        _ label: String, _ batch: Batch, _ column: Int, _ row: Int, _ expected: Cell
    ) {
        let verdict = batch.verdict(column, row: row)
        XCTAssertEqual(batch.verdicts(column)[row], verdict, label)
        let door = batch.columns[column].door

        guard expected.expect == "ok" else {
            let reason: CastFailure? =
                switch expected.expect {
                case "empty": .empty
                case "malformed": .malformed
                case "out_of_range": .outOfRange
                default: nil
                }
            XCTAssertNotNil(reason, "\(label): unknown expectation \(expected.expect)")
            XCTAssertFalse(verdict.isOk, label)
            XCTAssertEqual(verdict.reason, reason, label)
            if let span = expected.fault {
                XCTAssertEqual(verdict.offset, span[0], "\(label): fault offset")
                XCTAssertEqual(verdict.length, span[1], "\(label): fault length")
                // The cell's own text is still to hand, for the diagnostic a fault deserves.
                XCTAssertEqual(
                    String(decoding: batch.raw(column, row: row), as: UTF8.self), expected.raw,
                    "\(label): raw text")
            }
            XCTAssertEqual(typed(batch, column, row), verdict.fault.map { .fault($0) }, label)
            if door == .text {
                XCTAssertNil(batch.text(column, row: row), label)
                XCTAssertNil(batch.string(column, row: row), label)
            }
            return
        }

        XCTAssertTrue(verdict.isOk, label)
        XCTAssertNil(verdict.reason, label)
        XCTAssertNil(verdict.fault, label)
        XCTAssertEqual(typed(batch, column, row), .success(expectedValue(door, expected)), label)
        if door == .text {
            let text = expected.text!
            XCTAssertEqual(batch.text(column, row: row).map { Array($0) }, Array(text.utf8), label)
            XCTAssertEqual(batch.string(column, row: row), text, label)
            // Text is the bytes as they are, so the cell's raw text is the same bytes.
            XCTAssertEqual(String(decoding: batch.raw(column, row: row), as: UTF8.self), text, "\(label): raw text")
        }
    }

    // MARK: - one case, one way

    /// The corpus names a failure as the core does, in snake case.
    static let failureKinds: [String: TabularFailure] = [
        "unclosed_quote": .unclosedQuote, "column_count": .columnCount, "row_too_long": .rowTooLong,
        "not_a_zip": .notAZip, "container": .container, "encrypted": .encrypted, "method": .method,
        "missing_part": .missingPart, "xml": .xml, "deflate": .deflate, "not_a_workbook": .notAWorkbook,
        "shared_string": .sharedString, "too_large": .tooLarge,
    ]

    func expectedFailure(_ failure: Failure?) -> TabularError? {
        failure.map { failure in
            TabularError(
                kind: Self.failureKinds[failure.kind]!,
                record: failure.record, line: failure.line, byte: failure.byte,
                expected: failure.expected ?? 0, found: failure.found ?? 0)
        }
    }

    /// Replays the case twice: opened with its plan, and opened header first — `open` given
    /// no plan — with the plan bound once the header has been read. The two must agree.
    func replay(_ label: String, _ vector: Case, open: (Dialect, [Column]?) throws -> DelimitedReader) {
        for headerFirst in [false, true] {
            replay(headerFirst ? "\(label), header first" : label, vector, headerFirst: headerFirst, open: open)
        }
    }

    /// Holds a reader just opened without a plan to what an unbound reader promises, and
    /// binds `plan` to it.
    func bindHeaderFirst(_ label: String, _ reader: DelimitedReader, _ plan: [Column]) throws {
        XCTAssertFalse(reader.isBound, label)
        XCTAssertEqual(reader.plan, [], label)
        // Reading before a plan is an error that does not stick.
        for _ in 0..<2 {
            XCTAssertThrowsError(try reader.read(), label) { XCTAssertEqual($0 as? PlanError, .unbound, label) }
        }
        if let names = reader.header, !names.isEmpty {
            XCTAssertEqual(reader.columnCount, names.count, label)
            // Every name finds its own column, or the first of its name.
            for (ordinal, name) in names.enumerated() {
                XCTAssertEqual(names.firstIndex(of: name), names.firstIndex { $0.utf8.elementsEqual(name.utf8) }, label)
                XCTAssertLessThanOrEqual(try names.ordinal(of: name), ordinal, label)
                XCTAssertEqual(try names.ordinal(of: names.bytes(at: ordinal)), try names.ordinal(of: name), label)
            }
        }
        try reader.bind(plan)
        XCTAssertTrue(reader.isBound, label)
        XCTAssertThrowsError(try reader.bind(plan), label) {
            XCTAssertEqual($0 as? PlanError, .alreadyBound, label)
        }
    }

    private func replay(
        _ label: String, _ vector: Case, headerFirst: Bool, open: (Dialect, [Column]?) throws -> DelimitedReader
    ) {
        let plan = vector.plan.map(column(of:))
        let rows = vector.rows
        var seen = 0
        var failure: TabularError?
        do {
            // A header that is itself broken fails the open, after no rows at all.
            let reader = try open(dialect(of: vector), headerFirst ? nil : plan)
            XCTAssertEqual(reader.header.map(Array.init), vector.header, "\(label): header")
            if headerFirst {
                try bindHeaderFirst(label, reader, plan)
            }
            XCTAssertEqual(reader.plan, plan, label)
            do {
                while let batch = try reader.read() {
                    XCTAssertGreaterThan(batch.rows, 0, label)
                    for row in 0..<batch.rows {
                        guard seen < rows.count else {
                            XCTFail("\(label): more rows than the corpus lists")
                            return
                        }
                        for column in plan.indices {
                            assertCell(
                                "\(label), row \(seen), column \(column)", batch, column, row, rows[seen][column])
                        }
                        seen += 1
                    }
                }
            } catch let error as TabularError {
                failure = error
                // A structural failure is final: the same one, again.
                XCTAssertThrowsError(try reader.read(), label) { XCTAssertEqual($0 as? TabularError, error, label) }
            }
        } catch let error as TabularError {
            failure = error
        } catch {
            XCTFail("\(label): \(error)")
        }
        XCTAssertEqual(seen, rows.count, "\(label): rows delivered")
        XCTAssertEqual(failure, expectedFailure(vector.failure), "\(label): structural failure")
    }

    /// A source that hands the input over at most `chunk` bytes at a time.
    func chunked(_ input: [UInt8], _ chunk: Int) -> DelimitedReader.Source {
        var position = 0
        return { buffer in
            let count = min(chunk, buffer.count, input.count - position)
            buffer.copyBytes(from: input[position..<position + count])
            position += count
            return count
        }
    }

    /// An asynchronous sequence of bytes that hands them over `chunk` at a time, suspending
    /// between chunks as a network body does — and, given `failAt`, throws
    /// `CancellationError` once on reaching that byte, as a cancelled download does, then
    /// goes on where it was.
    struct ShortReads: AsyncSequence, Sendable {
        typealias Element = UInt8

        let bytes: [UInt8]
        let chunk: Int
        let failAt: Int?

        init(_ bytes: [UInt8], chunk: Int, failAt: Int? = nil) {
            self.bytes = bytes
            self.chunk = chunk
            self.failAt = failAt
        }

        struct AsyncIterator: AsyncIteratorProtocol {
            let bytes: [UInt8]
            let chunk: Int
            var failAt: Int?
            var position = 0

            mutating func next() async throws -> UInt8? {
                if position == failAt {
                    failAt = nil
                    throw CancellationError()
                }
                guard position < bytes.count else { return nil }
                if position % chunk == 0 {
                    await Task.yield()
                }
                defer { position += 1 }
                return bytes[position]
            }
        }

        func makeAsyncIterator() -> AsyncIterator {
            AsyncIterator(bytes: bytes, chunk: chunk, failAt: failAt)
        }
    }

    // MARK: - the corpus

    func testDelimitedCorpus() throws {
        let corpus = try corpus()
        XCTAssertGreaterThanOrEqual(corpus.count, 30)
        // The decoder kept what the file says: a byte-order mark is three bytes of input.
        XCTAssertTrue(corpus.contains { Array($0.input.utf8.prefix(3)) == [0xEF, 0xBB, 0xBF] })
        for vector in corpus {
            let name = vector.name
            let input = Array(vector.input.utf8)
            for batchRows in [1, 2, 1024] {
                replay("\(name) (memory, \(batchRows) rows a batch)", vector) { dialect, plan in
                    guard let plan else {
                        return try DelimitedReader(bytes: input, dialect: dialect, batchRows: batchRows)
                    }
                    return try DelimitedReader(bytes: input, dialect: dialect, plan: plan, batchRows: batchRows)
                }
                replay("\(name) (a string's utf8, \(batchRows) rows a batch)", vector) { dialect, plan in
                    guard let plan else {
                        return try DelimitedReader(bytes: vector.input.utf8, dialect: dialect, batchRows: batchRows)
                    }
                    return try DelimitedReader(
                        bytes: vector.input.utf8, dialect: dialect, plan: plan, batchRows: batchRows)
                }
                input.withUnsafeBytes { bytes in
                    replay("\(name) (memory in place, \(batchRows) rows a batch)", vector) { dialect, plan in
                        guard let plan else {
                            return try DelimitedReader(bytesNoCopy: bytes, dialect: dialect, batchRows: batchRows)
                        }
                        return try DelimitedReader(
                            bytesNoCopy: bytes, dialect: dialect, plan: plan, batchRows: batchRows)
                    }
                }
                for windowBytes in [1, 5] {
                    replay("\(name) (memory shown \(windowBytes) bytes at first, \(batchRows) rows a batch)", vector) {
                        dialect, plan in
                        try DelimitedReader(
                            bytes: input, dialect: dialect, plan: plan, batchRows: batchRows, windowBytes: windowBytes)
                    }
                }
                for chunk in [1, 5, 64, DelimitedReader.defaultBufferBytes] {
                    replay("\(name) (stream in chunks of \(chunk), \(batchRows) rows a batch)", vector) {
                        dialect, plan in
                        guard let plan else {
                            return try DelimitedReader(
                                reading: self.chunked(input, chunk), dialect: dialect, batchRows: batchRows,
                                bufferBytes: chunk)
                        }
                        return try DelimitedReader(
                            reading: self.chunked(input, chunk), dialect: dialect, plan: plan, batchRows: batchRows,
                            bufferBytes: chunk)
                    }
                }
            }
        }
    }

    /// The corpus from an asynchronous sequence that hands its bytes over a few at a time,
    /// suspending between them, read with `readAsync()` — opened with the plan, and header
    /// first.
    func testDelimitedCorpusFromAnAsyncSequence() async throws {
        let corpus = try corpus()
        for vector in corpus {
            let input = Array(vector.input.utf8)
            for (chunk, batchRows, bufferBytes) in [
                (1, 1, 3), (5, 2, 7), (64, 1024, DelimitedReader.defaultBufferBytes),
            ] {
                for headerFirst in [false, true] {
                    let label =
                        "\(vector.name) (async sequence in chunks of \(chunk), \(batchRows) rows a batch"
                        + "\(headerFirst ? ", header first" : ""))"
                    await replayAsync(label, vector, headerFirst: headerFirst) { dialect, plan in
                        let bytes = ShortReads(input, chunk: chunk)
                        guard let plan else {
                            return try await DelimitedReader(
                                bytes: bytes, dialect: dialect, batchRows: batchRows, bufferBytes: bufferBytes)
                        }
                        return try await DelimitedReader(
                            bytes: bytes, dialect: dialect, plan: plan, batchRows: batchRows, bufferBytes: bufferBytes)
                    }
                }
            }
        }
    }

    private func replayAsync(
        _ label: String, _ vector: Case, headerFirst: Bool,
        open: (Dialect, [Column]?) async throws -> DelimitedReader
    ) async {
        let plan = vector.plan.map(column(of:))
        let rows = vector.rows
        var seen = 0
        var failure: TabularError?
        do {
            let reader = try await open(dialect(of: vector), headerFirst ? nil : plan)
            XCTAssertEqual(reader.header.map(Array.init), vector.header, "\(label): header")
            if headerFirst {
                do {
                    _ = try await reader.readAsync()
                    XCTFail("\(label): read before a plan")
                } catch {
                    XCTAssertEqual(error as? PlanError, .unbound, label)
                }
                try reader.bind(plan)
            }
            do {
                while let batch = try await reader.readAsync() {
                    for row in batch {
                        guard seen < rows.count else {
                            XCTFail("\(label): more rows than the corpus lists")
                            return
                        }
                        for column in plan.indices {
                            assertCell(
                                "\(label), row \(seen), column \(column)", batch, column, row.index, rows[seen][column])
                        }
                        seen += 1
                    }
                }
            } catch let error as TabularError {
                failure = error
                do {
                    _ = try await reader.readAsync()
                    XCTFail("\(label): a structural failure is final")
                } catch {
                    XCTAssertEqual(error as? TabularError, failure, label)
                }
            }
        } catch let error as TabularError {
            failure = error
        } catch {
            XCTFail("\(label): \(error)")
        }
        XCTAssertEqual(seen, rows.count, "\(label): rows delivered")
        XCTAssertEqual(failure, expectedFailure(vector.failure), "\(label): structural failure")
    }

    func testDelimitedCorpusFromFiles() throws {
        let corpus = try corpus()
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("hypertabular-corpus-\(UUID().uuidString).csv").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        for vector in corpus {
            try Data(vector.input.utf8).write(to: URL(fileURLWithPath: path))
            for (batchRows, bufferBytes) in [(1, 3), (2, 7), (1024, DelimitedReader.defaultBufferBytes)] {
                replay("\(vector.name) (file through \(bufferBytes) bytes, \(batchRows) rows a batch)", vector) {
                    dialect, plan in
                    guard let plan else {
                        return try DelimitedReader(
                            contentsOfFile: path, dialect: dialect, batchRows: batchRows, bufferBytes: bufferBytes)
                    }
                    return try DelimitedReader(
                        contentsOfFile: path, dialect: dialect, plan: plan, batchRows: batchRows,
                        bufferBytes: bufferBytes)
                }
            }
        }
    }
}
