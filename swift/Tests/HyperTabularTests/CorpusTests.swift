import Foundation
import HyperTabularCore
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
        let input: String?
        let input_hex: String?
        let dialect: Declared
        let plan: [Entry]
        let header: [String]?
        let rows: [[Cell]]
        let problems: [Listed]
        let failure: Failure?
        let suggest: Suggested?
    }

    struct Declared: Decodable {
        let separator: String
        let quoting: Bool
        let has_header: Bool
        let skip_blank_lines: Bool
        let skip: UInt32?
        let header_row: UInt32?
        let short_rows: String?
        let long_rows: String?
        let tokens: [String]?
        let encoding: String?
    }

    struct Entry: Decodable {
        let door: String
        let ordinal: Int
        let precision: UInt32?
        let order: UInt32?
        let epoch: UInt32?
        let format: Notation?
        let tokens: [String]?
        let options: DatesDeclared?
    }

    struct DatesDeclared: Decodable {
        let flags: [String]?
        let pivot: Int?
        let months: [Month]?
    }

    /// `[month, name]`.
    struct Month: Decodable {
        let month: Int
        let name: String

        init(from decoder: Decoder) throws {
            var pair = try decoder.unkeyedContainer()
            month = try pair.decode(Int.self)
            name = try pair.decode(String.self)
        }
    }

    /// One entry of a case's `problems`.
    struct Listed: Decodable, Equatable {
        var row: Int
        var line: Int
        var plan_column: Int?
        var source_column: Int?
        var reason: String?
        var detail: String?
        var origin: String?
        var fault: [Int]?
        var expected: Int?
        var found: Int?
    }

    /// What a case says of its suggestion.
    struct Suggested: Decodable {
        let columns: [SuggestedEntry]
        let truncated: Bool
        let sample_rows: Int?
        let format: Notation?
    }

    struct SuggestedEntry: Decodable {
        let door: String
        let param: UInt32
        let status: String
        let matched: Int
        let sampled: Int
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
        let raw_hex: String?
        let detail: String?
        let origin: String?
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

    /// The bytes of a case's input.
    func bytes(of vector: Case) -> [UInt8] {
        vector.input_hex.map(Self.hex) ?? Array(vector.input!.utf8)
    }

    static func hex(_ text: String) -> [UInt8] {
        var bytes = [UInt8]()
        var index = text.startIndex
        while index < text.endIndex {
            let next = text.index(index, offsetBy: 2)
            bytes.append(UInt8(text[index..<next], radix: 16)!)
            index = next
        }
        return bytes
    }

    func dialect(of vector: Case, notes: Bool = true) -> Dialect {
        let declared = vector.dialect
        return Dialect(
            separator: declared.separator.unicodeScalars.first!,
            quoting: declared.quoting,
            hasHeader: declared.has_header,
            skipBlankLines: declared.skip_blank_lines,
            skip: declared.skip ?? 0,
            headerRow: declared.header_row ?? 0,
            shortRows: declared.short_rows.map {
                switch $0 {
                case "reject": .reject
                case "pad": .pad
                case let other: fatalError("unknown short_rows \(other)")
                }
            } ?? .reject,
            longRows: declared.long_rows.map {
                switch $0 {
                case "reject": .reject
                case "report": .report
                case "truncate": .truncate
                case let other: fatalError("unknown long_rows \(other)")
                }
            } ?? .reject,
            encoding: declared.encoding.map {
                switch $0 {
                case "utf-8": .utf8
                case "windows-1252": .windows1252
                case "latin-1": .latin1
                case "utf-16le": .utf16LE
                case let other: fatalError("unknown encoding \(other)")
                }
            } ?? .utf8,
            tokens: declared.tokens ?? [],
            notes: notes)
    }

    /// A corpus flags value mixes leniences and grouping, which HyperCast's type keeps apart.
    func format(_ format: Notation) -> NumFormat {
        NumFormat(
            decimalSeparator: format.decimal_sep, groupSeparator: format.group_sep,
            styles: NumStyles(rawValue: format.flags & 0xFF),
            grouping: Grouping(rawValue: format.flags & ~0xFF)!,
            currencySymbol: format.currency ?? "")
    }

    func format(of entry: Entry) -> NumFormat {
        entry.format.map(format) ?? .invariant
    }

    func column(of entry: Entry) -> Column {
        var column = door(of: entry)
        if let tokens = entry.tokens {
            column = column.withTokens(tokens)
        }
        if let options = entry.options {
            var flags: DateOptions.Flags = []
            for flag in options.flags ?? [] {
                switch flag {
                case "compact_date": flags.insert(.compactDate)
                case "space_separator": flags.insert(.spaceSeparator)
                case "hour_offset": flags.insert(.hourOffset)
                default: fatalError("unknown date flag \(flag)")
                }
            }
            column = column.withDateOptions(
                DateOptions(
                    flags: flags, pivot: options.pivot,
                    months: options.months.map { MonthNames($0.map { (month: $0.month, name: $0.name) }) }))
        }
        return column
    }

    func door(of entry: Entry) -> Column {
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
            // The Double nearest the instant, but never one in the next second.
            let rounded = Double(cell.seconds!) + Double(cell.nanos!) / 1_000_000_000
            return Date(timeIntervalSince1970: rounded < Double(cell.seconds! + 1) ? rounded : rounded.nextDown)
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
            return Self.hex(cell.value!.string).withUnsafeBytes { UUID(uuid: $0.load(as: uuid_t.self)) }
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
        _ label: String, _ batch: Batch, _ column: Int, _ row: Int, _ expected: Cell, notes: Bool = true
    ) {
        let verdict = batch.verdict(column, row: row)
        XCTAssertEqual(batch.verdicts(column)[row], verdict, label)
        let door = batch.columns[column].door

        // The note beside the verdict: with notes off there is none, and it says so.
        let detail = FaultDetail(code: UInt32(batch.note(column, row).detail))
        XCTAssertEqual(
            batch.note(column, row).detail == 0 ? nil : detail.description, notes ? expected.detail : nil,
            "\(label): detail")
        XCTAssertEqual(
            Self.originName(batch.origin(column, row: row)), notes ? expected.origin ?? "cell" : "cell",
            "\(label): origin")
        XCTAssertEqual(batch.fault(column, row: row) == nil, expected.expect == "ok", label)
        if let listed = batch.fault(column, row: row) {
            XCTAssertEqual(listed, verdict.fault, label)
            XCTAssertEqual(listed.detailCode, UInt32(batch.note(column, row).detail), label)
        }

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
                if let hex = expected.raw_hex {
                    XCTAssertEqual(Array(batch.raw(column, row: row)), Self.hex(hex), "\(label): raw bytes")
                } else {
                    XCTAssertEqual(
                        String(decoding: batch.raw(column, row: row), as: UTF8.self), expected.raw,
                        "\(label): raw text")
                }
            }
            let answered = typed(batch, column, row)
            XCTAssertEqual(answered, verdict.fault.map { .fault($0) }, label)
            if case .fault(let fault) = answered {
                XCTAssertEqual(
                    fault.detailCode, UInt32(batch.note(column, row).detail), "\(label): the accessor's detail")
            }
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
        "shared_string": .sharedString, "too_large": .tooLarge, "iwa": .iwa, "encoding": .encoding,
    ]

    static func originName(_ origin: Origin) -> String {
        switch origin {
        case .cell: "cell"
        case .token: "token"
        case .padded: "padded"
        case .uncached: "uncached"
        }
    }

    /// The problems a batch lists, in the corpus's shape, its rows counted across the read.
    func listed(_ label: String, _ batch: Batch, base: Int) -> [Listed] {
        batch.faults().map { problem in
            XCTAssertEqual(problem.line, batch.line(problem.row), label)
            var entry = Listed(row: base + problem.row, line: problem.line)
            if let plan = problem.planColumn {
                entry.plan_column = plan
                entry.source_column = problem.sourceColumn
                switch problem.reason {
                case .empty:
                    entry.reason = "empty"
                    XCTAssertEqual(problem.offset, 0, label)
                    XCTAssertEqual(problem.length, 0, label)
                case .malformed:
                    entry.reason = "malformed"
                    entry.fault = [problem.offset, problem.length]
                default:
                    entry.reason = "out_of_range"
                    entry.fault = [problem.offset, problem.length]
                }
                XCTAssertEqual(problem.expected, 0, label)
                XCTAssertEqual(problem.found, 0, label)
            } else {
                XCTAssertNil(problem.sourceColumn, label)
                XCTAssertNil(problem.reason, label)
                XCTAssertEqual(problem.offset, 0, label)
                XCTAssertEqual(problem.length, 0, label)
                XCTAssertEqual(problem.detailCode, 0, label)
                entry.expected = problem.expected
                entry.found = problem.found
            }
            if problem.detailCode != 0 { entry.detail = problem.detail?.description }
            if problem.origin != .cell { entry.origin = Self.originName(problem.origin) }
            return entry
        }
    }

    /// What the corpus lists, as a read with these notes sees it: with notes off there is no detail
    /// or origin to give, and no formula with no cached value.
    func expectedProblems(_ problems: [Listed], notes: Bool) -> [Listed] {
        notes
            ? problems
            : problems.filter { $0.origin != "uncached" }.map {
                var entry = $0
                entry.detail = nil
                entry.origin = nil
                return entry
            }
    }

    /// Holds every ``Row`` of a batch, met by `for`, to the batch's own accessors.
    func assertRows(_ label: String, _ batch: Batch) {
        var index = 0
        for row in batch {
            XCTAssertEqual(row.index, index, label)
            XCTAssertEqual(row.line, batch.line(index), label)
            for column in batch.columns.indices {
                XCTAssertEqual(row.verdict(column), batch.verdict(column, row: index), label)
                XCTAssertEqual(row.origin(column), batch.origin(column, row: index), label)
                XCTAssertEqual(row.fault(column), batch.fault(column, row: index), label)
                XCTAssertEqual(row.fault(column)?.detailCode, batch.fault(column, row: index)?.detailCode, label)
                XCTAssertEqual(Array(row.raw(column)), Array(batch.raw(column, row: index)), label)
                if batch.columns[column].door == .text {
                    XCTAssertEqual(row.string(column), batch.string(column, row: index), label)
                    XCTAssertEqual(
                        row.text(column).map { Array($0) }, batch.text(column, row: index).map { Array($0) }, label)
                }
            }
            index += 1
        }
        XCTAssertEqual(index, batch.rows, label)
    }

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
            replay(
                headerFirst ? "\(label), header first" : label, vector, headerFirst: headerFirst, notes: true,
                open: open)
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
            // Every name finds its own column, or the first of its name, and is refused by the strict
            // lookup if it is not the only one.
            for (ordinal, name) in names.enumerated() {
                let first = names.firstIndex(of: name)
                XCTAssertEqual(first, names.firstIndex { $0.utf8.elementsEqual(name.utf8) }, label)
                XCTAssertLessThanOrEqual(try XCTUnwrap(first), ordinal, label)
                XCTAssertEqual(names.firstIndex(of: names.bytes(at: ordinal)), first, label)
                if names.filter({ $0.utf8.elementsEqual(name.utf8) }).count > 1 {
                    XCTAssertThrowsError(try names.ordinal(of: name), label) {
                        XCTAssertTrue($0 is AmbiguousColumn, label)
                    }
                } else {
                    XCTAssertEqual(try names.ordinal(of: name), ordinal, label)
                    XCTAssertEqual(try names.ordinal(of: names.bytes(at: ordinal)), ordinal, label)
                }
            }
        }
        try reader.bind(plan)
        XCTAssertTrue(reader.isBound, label)
        XCTAssertThrowsError(try reader.bind(plan), label) {
            XCTAssertEqual($0 as? PlanError, .alreadyBound, label)
        }
    }

    func replay(
        _ label: String, _ vector: Case, headerFirst: Bool, notes: Bool,
        open: (Dialect, [Column]?) throws -> DelimitedReader
    ) {
        let plan = vector.plan.map(column(of:))
        let rows = vector.rows
        var seen = 0
        var problems = [Listed]()
        var failure: TabularError?
        do {
            // A header that is itself broken fails the open, after no rows at all.
            let reader = try open(dialect(of: vector, notes: notes), headerFirst ? nil : plan)
            XCTAssertEqual(reader.header.map(Array.init), vector.header, "\(label): header")
            if headerFirst {
                try bindHeaderFirst(label, reader, plan)
            }
            XCTAssertEqual(reader.plan, plan, label)
            do {
                while let batch = try reader.read() {
                    XCTAssertGreaterThan(batch.rows, 0, label)
                    assertRows(label, batch)
                    problems += listed(label, batch, base: seen)
                    for row in 0..<batch.rows {
                        guard seen < rows.count else {
                            XCTFail("\(label): more rows than the corpus lists")
                            return
                        }
                        XCTAssertEqual(rows[seen].count, plan.count, label)
                        for column in plan.indices {
                            assertCell(
                                "\(label), row \(seen), column \(column)", batch, column, row, rows[seen][column],
                                notes: notes)
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
        XCTAssertEqual(problems, expectedProblems(vector.problems, notes: notes), "\(label): problems")
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
        XCTAssertGreaterThanOrEqual(corpus.count, 123)
        // The decoder kept what the file says: a byte-order mark is three bytes of input.
        XCTAssertTrue(corpus.contains { Array(bytes(of: $0).prefix(3)) == [0xEF, 0xBB, 0xBF] })
        for vector in corpus {
            let name = vector.name
            let input = bytes(of: vector)
            for batchRows in [1, 2, 1024] {
                replay("\(name) (memory, \(batchRows) rows a batch)", vector) { dialect, plan in
                    guard let plan else {
                        return try DelimitedReader(bytes: input, dialect: dialect, batchRows: batchRows)
                    }
                    return try DelimitedReader(bytes: input, dialect: dialect, plan: plan, batchRows: batchRows)
                }
                replay(
                    "\(name) (memory, notes off, \(batchRows) rows a batch)", vector, headerFirst: false, notes: false
                ) { dialect, plan in
                    try DelimitedReader(bytes: input, dialect: dialect, plan: plan ?? [], batchRows: batchRows)
                }
                if let text = vector.input {
                    replay("\(name) (a string's utf8, \(batchRows) rows a batch)", vector) { dialect, plan in
                        guard let plan else {
                            return try DelimitedReader(bytes: text.utf8, dialect: dialect, batchRows: batchRows)
                        }
                        return try DelimitedReader(
                            bytes: text.utf8, dialect: dialect, plan: plan, batchRows: batchRows)
                    }
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
            let input = bytes(of: vector)
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
        var problems = [Listed]()
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
                    problems += listed(label, batch, base: seen)
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
        XCTAssertEqual(problems, expectedProblems(vector.problems, notes: true), "\(label): problems")
    }

    func testDelimitedCorpusFromFiles() throws {
        let corpus = try corpus()
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("hypertabular-corpus-\(UUID().uuidString).csv").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        for vector in corpus {
            try Data(bytes(of: vector)).write(to: URL(fileURLWithPath: path))
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

    // MARK: - suggestions

    /// A door's name in the corpus.
    static func doorName(_ door: Door) -> String {
        switch door {
        case .bool: "bool"
        case .i8: "i8"
        case .i16: "i16"
        case .i32: "i32"
        case .i64: "i64"
        case .u8: "u8"
        case .u16: "u16"
        case .u32: "u32"
        case .u64: "u64"
        case .f32: "f32"
        case .f64: "f64"
        case .decimal: "decimal"
        case .uuid: "uuid"
        case .timestamp: "timestamp"
        case .unix: "unix"
        case .excelSerial: "excel_serial"
        case .date: "date"
        case .dateOrdered: "date_ordered"
        case .dateTime: "datetime"
        case .time: "time"
        case .duration: "duration"
        case .text: "text"
        }
    }

    static let commaDecimal = NumFormat(decimalSeparator: ",", groupSeparator: ".", styles: .all)

    /// Holds a suggestion to what a corpus case says of it, its param put back together from the
    /// report.
    func assertSuggestion(_ label: String, _ found: Suggestion, _ expected: Suggested) {
        XCTAssertEqual(found.truncated, expected.truncated, label)
        XCTAssertEqual(found.report.count, expected.columns.count, label)
        XCTAssertEqual(found.plan.count, expected.columns.count, label)
        let declared = expected.format.map(format)
        for (index, report) in found.report.enumerated() where index < expected.columns.count {
            let want = expected.columns[index]
            let place = "\(label), column \(index)"
            var param = want.param
            var got = report.param
            if !report.candidates.isEmpty {
                got = 0
                for candidate in report.candidates {
                    switch candidate {
                    case .numFormat(let notation): got |= notation.decimalSeparator == "." ? 1 << 1 : 1 << 2
                    case .dateOrder(let order): got |= 1 << order.rawValue
                    }
                }
            }
            if report.status == .resolved && Column.isNumeric(report.door) {
                // The plan is in the one notation the param left, and comma groups is also what's used
                // with no separator to prove it. The export's param is held to in its own replay.
                let planned = declared ?? (param == 1 << 2 ? Self.commaDecimal : .invariant)
                param = 0
                XCTAssertEqual(report.format, planned, place)
                XCTAssertEqual(found.plan[index].format, planned, place)
            } else {
                XCTAssertNil(report.format, place)
            }
            XCTAssertEqual(Self.doorName(report.door), want.door, place)
            XCTAssertEqual(got, param, place)
            XCTAssertEqual("\(report.status)", want.status, place)
            XCTAssertEqual(report.matched, want.matched, place)
            XCTAssertEqual(report.sampled, want.sampled, place)
            XCTAssertEqual(report.ordinal, index, place)
            XCTAssertEqual(found.plan[index].ordinal, index, place)
            XCTAssertEqual(found.plan[index].door, report.status == .resolved ? report.door : .text, place)
        }
    }

    /// What the corpus declares for a case's suggestion.
    func options(of suggest: Suggested) -> SuggestOptions {
        SuggestOptions(sampleRows: suggest.sample_rows ?? 1000, format: suggest.format.map(format))
    }

    /// Every case that says what to suggest: from memory and from a file; through the export, over
    /// the cells the case's own rows say; and a plan that reads back without refusing what it resolved.
    func testEverySuggestionIsWhatTheCorpusSays() throws {
        let path = FileManager.default.temporaryDirectory
            .appendingPathComponent("hypertabular-suggest-\(UUID().uuidString).csv").path
        defer { try? FileManager.default.removeItem(atPath: path) }
        var seen = 0
        for vector in try corpus() {
            guard let suggest = vector.suggest else { continue }
            seen += 1
            let input = bytes(of: vector)
            let dialect = dialect(of: vector)
            let options = options(of: suggest)
            let found = try DelimitedReader.suggest(bytes: input, dialect: dialect, options: options)
            assertSuggestion("\(vector.name) (memory)", found, suggest)
            try Data(input).write(to: URL(fileURLWithPath: path))
            assertSuggestion(
                "\(vector.name) (file)",
                try DelimitedReader.suggest(contentsOfFile: path, dialect: dialect, options: options),
                suggest)
            assertTheExportSays(vector, suggest, options)

            // What's resolved takes every cell it saw, and refuses none.
            let sample = suggest.sample_rows ?? Int.max
            let reader = try DelimitedReader(bytes: input, dialect: dialect, plan: found.plan)
            var read = 0
            while read < sample, let batch = try reader.read() {
                for problem in batch.faults() where problem.row + read < sample {
                    if let plan = problem.planColumn, found.report[plan].status == .resolved,
                        found.report[plan].door != .text
                    {
                        XCTFail("\(vector.name): read back refused \(problem)")
                    }
                }
                read += batch.rows
            }
        }
        XCTAssertGreaterThanOrEqual(seen, 19)
    }

    /// The export, over a case's own rows: a cell with no text in the case is blank here, as a read's
    /// empty verdict is.
    private func assertTheExportSays(_ vector: Case, _ suggest: Suggested, _ options: SuggestOptions) {
        let sample = suggest.sample_rows ?? vector.rows.count
        let width = vector.plan.count
        var text = [UInt8]()
        var cells = [HyperTabularSpan]()
        for column in 0..<width {
            for row in 0..<sample {
                let cell = vector.rows[row][column]
                let bytes = cell.raw_hex.map(Self.hex) ?? cell.text.map { Array($0.utf8) } ?? []
                var span = HyperTabularSpan()
                span.offset = UInt32(text.count)
                span.len = UInt32(bytes.count)
                cells.append(span)
                text += bytes
            }
        }
        var formats = [HyperTabularRawNumFormat](repeating: HyperTabularRawNumFormat(), count: width)
        if let format = options.format {
            var raw = Interop.rawFormat(format)
            for index in 0..<width {
                withUnsafeBytes(of: &raw) { source in
                    withUnsafeMutableBytes(of: &formats[index]) { $0.copyMemory(from: source) }
                }
            }
        }
        var found = [HyperTabularRawSuggestion](repeating: HyperTabularRawSuggestion(), count: width)
        XCTAssertEqual(
            hypertabular_suggest(
                text, UInt(text.count), nil, 0, cells, nil, UInt(sample), UInt(width), formats, &found),
            HYPERTABULAR_OK, vector.name)
        for column in 0..<width {
            let want = suggest.columns[column]
            let place = "\(vector.name): the export, column \(column)"
            XCTAssertEqual(found[column].door, Self.doorCode(want.door), place)
            XCTAssertEqual(found[column].param, want.param, place)
            XCTAssertEqual(found[column].status, ["resolved": 1, "ambiguous": 2, "unresolved": 3][want.status], place)
            XCTAssertEqual(Int(found[column].matched), want.matched, place)
            XCTAssertEqual(Int(found[column].sampled), want.sampled, place)
        }
    }

    private static func doorCode(_ name: String) -> UInt32? {
        (1...22).lazy.compactMap { Door(rawValue: UInt32($0)) }.first { doorName($0) == name }?.rawValue
    }

    /// Every case the corpus has: a suggestion fails where a read of the case fails, and nowhere else.
    func testASuggestionFailsOnlyWhereAReadOfTheCaseFails() throws {
        var planned = 0
        var failed = 0
        for vector in try corpus() {
            do {
                _ = try DelimitedReader.suggest(bytes: bytes(of: vector), dialect: dialect(of: vector))
                planned += 1
            } catch let error as TabularError {
                failed += 1
                XCTAssertNotNil(vector.failure, "\(vector.name): \(error)")
            }
        }
        XCTAssertGreaterThan(planned, 80)
        XCTAssertGreaterThan(failed, 0)
    }
}
