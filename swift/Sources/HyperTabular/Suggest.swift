import Foundation
import HyperTabularCore

/// How a sample is taken and read: what ``DelimitedReader/suggest(contentsOfFile:dialect:options:)``
/// and ``Sheet/suggest(_:)`` are told. Nothing declared, it takes 1,000 rows.
public struct SuggestOptions: Equatable, Sendable {
    /// The most rows the sample holds, and always at least one: 0 is 1. One batch is the most it
    /// can be: ``DelimitedReader/defaultBatchRows`` of delimited text, and a sheet's own batch size.
    public var sampleRows: Int {
        didSet { Self.check(sampleRows) }
    }
    /// Reads every column's numbers in this notation, which settles what `,` means in them.
    public var format: NumFormat?
    /// Reads the numbers of a source column, by ordinal, in a notation of its own, ahead of ``format``.
    public var columnFormats: [Int: NumFormat]
    /// Reads every column's dates in this order where the sample leaves the order open, as
    /// `01/02/2026` does. A column whose cells rule that order out keeps the order they leave: it
    /// answers what the sample cannot, and does not overrule what it can.
    public var dateOrder: DateOrder?
    /// Reads the dates of a source column, by ordinal, in an order of its own, ahead of ``dateOrder``,
    /// and as that does.
    public var columnDateOrders: [Int: DateOrder]

    /// The options, stated.
    ///
    /// - Precondition: `sampleRows` is not negative.
    public init(
        sampleRows: Int = 1000, format: NumFormat? = nil, columnFormats: [Int: NumFormat] = [:],
        dateOrder: DateOrder? = nil, columnDateOrders: [Int: DateOrder] = [:]
    ) {
        Self.check(sampleRows)
        self.sampleRows = sampleRows
        self.format = format
        self.columnFormats = columnFormats
        self.dateOrder = dateOrder
        self.columnDateOrders = columnDateOrders
    }

    private static func check(_ sampleRows: Int) {
        precondition(sampleRows >= 0, "sampleRows must not be negative; got \(sampleRows)")
    }

    /// The notation declared for a source column, if any.
    func declaredFormat(_ ordinal: Int) -> NumFormat? {
        columnFormats[ordinal] ?? format
    }

    /// The date order declared for a source column, if any.
    func declaredOrder(_ ordinal: Int) -> DateOrder? {
        columnDateOrders[ordinal] ?? dateOrder
    }

    /// Judges every format these options declare as a plan's would be, whichever would win for a
    /// column and whether or not the sample reaches its ordinal. A suggest asks this before it opens
    /// or reads anything, so a stale format is refused on an empty source and on a broken one: the
    /// format for every column first, as column 0, then each column's by ordinal.
    func checkFormats() throws {
        let judge = Judge()
        if let format, !judge.accepts(Column.f64(0, format: format).spec) {
            throw PlanError.refused(column: 0)
        }
        for (ordinal, format) in columnFormats.sorted(by: { $0.key < $1.key })
        where !judge.accepts(Column.f64(0, format: format).spec) {
            throw PlanError.refused(column: ordinal)
        }
    }

    /// How many rows to take, where a batch is `batchRows`.
    func rows(within batchRows: Int) -> Int {
        min(max(sampleRows, 1), max(batchRows, 1))
    }
}

/// How far a sample settles a column. The raw values are the core's codes.
public enum SuggestStatus: UInt32, Equatable, Sendable {
    /// One door and one declaration take every cell, and the plan reads the column through them.
    case resolved = 1
    /// Every cell is read, but the sample cannot say which declaration it is written in:
    /// ``SuggestedColumn/candidates`` are the choices. The plan reads the column as text until the
    /// caller chooses.
    case ambiguous = 2
    /// There is nothing to go on, or a cell no door took beside cells one did, or a cell that is not
    /// text at all. The plan reads the column as text.
    case unresolved = 3
}

/// A declaration that would settle a ``SuggestStatus/ambiguous`` column.
public enum Candidate: Equatable, Sendable {
    /// The order a date's fields come in.
    case dateOrder(DateOrder)
    /// The notation a number is in. Declare it with ``SuggestOptions/columnFormats`` and ask again:
    /// the door can differ under each, as `1,234` is an integer under one and a `Double` under the
    /// other.
    case numFormat(NumFormat)
}

/// What a sample says of one source column.
public struct SuggestedColumn: Equatable, Sendable {
    /// The zero-based source column, as a ``Column`` names it.
    public let ordinal: Int
    /// The header's name for it, or `nil` if the source has no header.
    public let name: String?
    /// The door found: ``Door/text`` when unresolved, and the first candidate's when ambiguous.
    public let door: Door
    /// What ``door`` declares, as the core numbers it: a date's `DateOrder`, the first candidate's
    /// when ambiguous, a Unix precision or an Excel epoch. 0 for another door.
    public let param: UInt32
    /// The notation the plan reads a resolved numeric column in: the one declared for it, or else
    /// the one candidate the sample left, comma decimal where only that reads the column, and
    /// otherwise `,` grouping in threes, whether or not the sample held a `,`. Both are Western, so a
    /// later `1,23,456` is refused. `nil` for any other column.
    public let format: NumFormat?
    /// What to choose between: empty unless ambiguous.
    public let candidates: [Candidate]
    /// How far the sample settles it.
    public let status: SuggestStatus
    /// The cells ``door`` took. Resolved or ambiguous, that is all of ``sampled``. Unresolved, it is
    /// the most that any one door but the text door took.
    public let matched: Int
    /// The cells that are not blank.
    public let sampled: Int
}

/// What a suggest found: a plan to change or to hand to a read, and why. It is never applied.
public struct Suggestion: Equatable, Sendable {
    /// A column for each source column, in order. Only a resolved column reads through its door:
    /// the rest read as text, which refuses nothing and loses nothing.
    public let plan: [Column]
    /// A report for each source column, in the plan's order.
    public let report: [SuggestedColumn]
    /// Whether the source has rows past the sample. A source with exactly the sample's rows has not.
    public let truncated: Bool
}

/// A sample, in the shape the core takes it: each column's spans into text.
final class Sample {
    /// Bit 1 of a numeric param: a format with `,` grouping digits in threes. Western.
    private static let commaGroups: UInt32 = 1 << 1
    /// Bit 2 of a numeric param: a format with `,` as the decimal separator. Western.
    private static let commaDecimal: UInt32 = 1 << 2
    private static let commaDecimalFormat = NumFormat(decimalSeparator: ",", groupSeparator: ".", styles: .all)

    private let limit: Int
    private var columns: [[HyperTabularSpan]]
    /// A byte for each span of a sheet's: not 0 for a typed cell. `nil` for delimited text.
    private var typed: [[UInt8]]?
    private var text = [UInt8]()
    private var rows = 0
    private var truncated = false

    private init(width: Int, limit: Int, sheet: Bool) {
        self.limit = limit
        columns = Array(repeating: [], count: width)
        typed = sheet ? Array(repeating: [], count: width) : nil
    }

    /// Reads a source of `width` columns through the text door until the sample is whole and a row
    /// past it, or the end, says whether it is the source's last. A failure among the sampled rows is
    /// the caller's, and a failure just past them only says there is more.
    ///
    /// A sheet's sample binds a second, UUID, door to every column, for what it says of a cell: no
    /// door but a text door takes a typed cell's kind, so there it is malformed with no detail, where
    /// a text cell that is not a UUID has one. That is the read's word for which cells were typed, and
    /// it is what keeps a typed number out of the question of what `,` and `.` mean.
    static func of(
        width: Int, limit: Int, sheet: Bool, bind: ([Column]) throws -> Void, read: () throws -> Batch?
    ) throws -> Sample {
        guard width <= Columns.maxOrdinal + 1 else {
            throw PlanError.outOfReach(column: Columns.maxOrdinal + 1)
        }
        let sample = Sample(width: width, limit: limit, sheet: sheet)
        guard width > 0 else { return sample }
        var plan = (0..<width).map { Column.text($0) }
        if sheet {
            plan += (0..<width).map { Column.uuid($0) }
        }
        try bind(plan)
        while !sample.truncated {
            let batch: Batch?
            do {
                batch = try read()
            } catch  where sample.rows == limit {
                sample.truncated = true
                break
            }
            guard let batch else { break }
            sample.take(batch)
        }
        return sample
    }

    /// Takes what the sample still wants of a batch. A blank, a token and padding are all a blank
    /// span. A refused cell keeps its bytes, for the core's own text door to refuse.
    private func take(_ batch: Batch) {
        let width = columns.count
        for row in 0..<batch.rows {
            if rows == limit {
                truncated = true
                return
            }
            let start = text.count
            var fits = true
            for column in 0..<width where fits {
                let verdict = batch.verdict(column, row: row)
                let bytes: UnsafeRawBufferPointer
                if verdict.isOk {
                    bytes = batch.text(column, row: row) ?? UnsafeRawBufferPointer(start: nil, count: 0)
                } else if verdict.reason != .empty {
                    bytes = batch.raw(column, row: row)
                } else {
                    bytes = UnsafeRawBufferPointer(start: nil, count: 0)
                }
                fits = append(bytes, column)
            }
            // A span holds a 32-bit offset. A sample that far along stops, and says it did.
            guard fits else {
                text.removeLast(text.count - start)
                for column in 0..<width {
                    columns[column].removeLast(columns[column].count - rows)
                }
                truncated = true
                return
            }
            if typed != nil {
                for column in 0..<width {
                    let second = width + column
                    let mark =
                        batch.verdict(second, row: row).reason == .malformed && batch.note(second, row).detail == 0
                    typed?[column].append(mark ? 1 : 0)
                }
            }
            rows += 1
        }
    }

    private func append(_ bytes: UnsafeRawBufferPointer, _ column: Int) -> Bool {
        let end = text.count + bytes.count
        guard end <= Int(Int32.max) else { return false }
        var span = HyperTabularSpan()
        span.offset = UInt32(text.count)
        span.len = UInt32(bytes.count)
        text.append(contentsOf: bytes)
        columns[column].append(span)
        return true
    }

    /// Asks the core, and builds the plan and the report from its answer.
    func judge(header: Header?, options: SuggestOptions) throws -> Suggestion {
        let width = columns.count
        var declared = [NumFormat?](repeating: nil, count: width)
        var formats = [HyperTabularRawNumFormat](repeating: HyperTabularRawNumFormat(), count: width)
        for ordinal in 0..<width {
            guard let format = options.declaredFormat(ordinal) else { continue }
            declared[ordinal] = format
            var raw = Interop.rawFormat(format)
            withUnsafeBytes(of: &raw) { source in
                withUnsafeMutableBytes(of: &formats[ordinal]) { $0.copyMemory(from: source) }
            }
        }
        let cells = columns.flatMap { $0 }
        let marks = typed?.flatMap { $0 }
        var found = [HyperTabularRawSuggestion](repeating: HyperTabularRawSuggestion(), count: width)
        // An array with no element may have no address the core can take: a sample with no rows is
        // handed one anyway, as it is whatever is asked of it.
        let code: Int32 = text.withUnsafeBufferPointer { input in
            cells.withUnsafeBufferPointer { spans in
                func call(_ typed: UnsafePointer<UInt8>?) -> Int32 {
                    hypertabular_suggest(
                        input.baseAddress, UInt(input.count), nil, 0, spans.baseAddress, typed, UInt(rows), UInt(width),
                        formats, &found)
                }
                guard let marks else { return call(nil) }
                return marks.withUnsafeBufferPointer { call($0.baseAddress) }
            }
        }
        guard code == HYPERTABULAR_OK else {
            preconditionFailure(Workbook.contractViolation)
        }
        var plan = [Column]()
        var report = [SuggestedColumn]()
        for ordinal in 0..<width {
            let column = Self.settled(
                Self.report(ordinal, found[ordinal], header, declared[ordinal]), options.declaredOrder(ordinal))
            report.append(column)
            plan.append(Self.plan(column))
        }
        return Suggestion(plan: plan, report: report, truncated: truncated)
    }

    /// What the core said of a column, in this binding's terms. Every door the core has is taken
    /// here, not only those it suggests today: a door it does not name, or a param its door does not
    /// declare, reads as text.
    static func report(
        _ ordinal: Int, _ found: HyperTabularRawSuggestion, _ header: Header?, _ declared: NumFormat?
    ) -> SuggestedColumn {
        let status = SuggestStatus(rawValue: found.status) ?? .unresolved
        let name = header.flatMap { ordinal < $0.count ? $0[ordinal] : nil }
        let told = Door(rawValue: found.door)
        var param = found.param
        var candidates = [Candidate]()
        if status == .ambiguous {
            // An ambiguous param is a set of candidates, bit n for code n.
            candidates = told.map { Self.candidates($0, found.param) } ?? []
            param = found.param == 0 ? 0 : UInt32(found.param.trailingZeroBitCount)
        }
        let numeric = told.map(Column.isNumeric) ?? false
        if numeric { param = 0 }
        let (door, declaring) = Self.door(told, param)
        let format: NumFormat? =
            status == .resolved && numeric
            ? declared ?? (found.param == commaDecimal ? commaDecimalFormat : .invariant) : nil
        return SuggestedColumn(
            ordinal: ordinal, name: name, door: door, param: declaring, format: format, candidates: candidates,
            status: status, matched: Int(found.matched), sampled: Int(found.sampled))
    }

    /// Answers a date the sample left open with the order declared for its column, if that is one of
    /// the candidates.
    private static func settled(_ report: SuggestedColumn, _ order: DateOrder?) -> SuggestedColumn {
        guard let order, report.status == .ambiguous, report.candidates.contains(.dateOrder(order)) else {
            return report
        }
        return SuggestedColumn(
            ordinal: report.ordinal, name: report.name, door: .dateOrdered, param: order.rawValue, format: nil,
            candidates: [], status: .resolved, matched: report.matched, sampled: report.sampled)
    }

    /// A door and what it declares, or the text door when the core's numbers are not one.
    private static func door(_ door: Door?, _ param: UInt32) -> (Door, UInt32) {
        guard let door else { return (.text, 0) }
        switch door {
        case .unix:
            return UnixPrecision(rawValue: param) != nil ? (door, param) : (.text, 0)
        case .dateOrdered, .dateTime:
            return DateOrder(rawValue: param) != nil ? (door, param) : (.text, 0)
        case .excelSerial:
            return ExcelEpoch(rawValue: param) != nil ? (door, param) : (.text, 0)
        default:
            return (door, 0)
        }
    }

    /// What an ambiguous column's param offers.
    private static func candidates(_ door: Door, _ param: UInt32) -> [Candidate] {
        if door == .dateOrdered || door == .dateTime {
            let orders: [DateOrder] = [.yearMonthDay, .monthDayYear, .dayMonthYear]
            return orders.filter { param & (1 << $0.rawValue) != 0 }.map { .dateOrder($0) }
        }
        var offered = [Candidate]()
        if param & commaGroups != 0 { offered.append(.numFormat(.invariant)) }
        if param & commaDecimal != 0 { offered.append(.numFormat(commaDecimalFormat)) }
        return offered
    }

    /// The column a report plans: only a resolved column reads through its door, a numeric one in
    /// the format the report names.
    static func plan(_ report: SuggestedColumn) -> Column {
        guard report.status == .resolved else { return .text(report.ordinal) }
        return Column(report.ordinal, report.door, declared: report.param, format: report.format ?? .invariant)
    }
}
