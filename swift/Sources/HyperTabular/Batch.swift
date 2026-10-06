import Foundation
import HyperTabularCore

/// The rows one ``DelimitedReader/read()`` or ``Sheet/read()`` delivered, as typed columns:
/// the same type for delimited text and for a sheet of a workbook.
///
/// A column is handed out whole — ``values(_:as:)``, with ``verdicts(_:)`` beside it — or a
/// cell at a time through ``get(_:row:as:)``, which answers with HyperCast's own `Verdict`.
///
/// ```swift
/// while let batch = try reader.read() {
///     for row in 0..<batch.rows {
///         switch batch.get(2, row: row, as: Double.self) {
///         case .success(let score): print(batch.string(1, row: row) ?? "", score)
///         case .fault(let fault):
///             print("line \(batch.line(row)): \(fault.reason) in", String(decoding: batch.raw(2, row: row), as: UTF8.self))
///         }
///     }
/// }
/// ```
///
/// Everything a batch hands out points into its reader's own memory and is valid until the
/// reader's next read (``raw(_:row:)`` until the next call to itself, too) and no longer
/// than the reader. The reader hands out the same batch every time, with the next rows in it.
public final class Batch {
    private let columnSet: Columns
    private let workbook: Bool

    /// How many rows the batch holds: never zero for a batch a read returned, and zero once
    /// the reader has moved on.
    public private(set) var rows = 0

    // The batch in hand, as the core left it: the table that locates each cell — perRow
    // entries a row, the last of them the row's line — what an unflagged span indexes (the
    // window of input, or a workbook's shared strings), and what a flagged one indexes.
    private var cells: UnsafeBufferPointer<hypertabular_span> = UnsafeBufferPointer(start: nil, count: 0)
    private var perRow = 0
    private var base: UnsafeRawPointer?
    private var arena: UnsafeRawPointer?
    private var scratch = UnsafeMutableBufferPointer<UInt8>(start: nil, count: 0)

    init(_ columns: Columns, workbook: Bool) {
        self.columnSet = columns
        self.workbook = workbook
    }

    deinit {
        scratch.deallocate()
    }

    /// Makes the batch the rows the core just wrote.
    func fill(
        rows: Int, cells: UnsafeMutableBufferPointer<hypertabular_span>, perRow: Int,
        base: UnsafeRawPointer?, arena: UnsafeMutableBufferPointer<UInt8>
    ) -> Batch {
        self.rows = rows
        self.cells = UnsafeBufferPointer(cells)
        self.perRow = perRow
        self.base = base
        self.arena = UnsafeRawPointer(arena.baseAddress)
        return self
    }

    /// Ends the batch in hand: the reader is moving on.
    func clear() {
        rows = 0
    }

    /// The plan the batch was read through: column `i` of the batch is `columns[i]`.
    public var columns: [Column] { columnSet.plan }

    private func checkRow(_ row: Int) {
        precondition(row >= 0 && row < rows, "Row \(row) is outside the batch in hand (\(rows) rows)")
    }

    /// Where row `row` came from: for delimited text the 1-based line its record starts on,
    /// for a sheet its 1-based row number.
    public func line(_ row: Int) -> Int {
        checkRow(row)
        let entry = cells[row * perRow + perRow - 1]
        return Int(workbook ? entry.offset : entry.len)
    }

    // MARK: - a column at a time

    /// A column's verdicts, one per row.
    public func verdicts(_ column: Int) -> UnsafeBufferPointer<CellVerdict> {
        UnsafeBufferPointer(start: columnSet.verdictBase[column], count: rows)
    }

    /// A column's values, one per row, exactly as the core wrote them — for the doors whose
    /// value is a primitive (``ColumnValue``): ``Door/bool`` as `Bool`, the integer and
    /// floating-point doors as their own type. The value of a row whose verdict is not ok is
    /// zero. Columns of other doors are read a cell at a time, with ``get(_:row:as:)``.
    ///
    /// - Precondition: `T` is the type the column's door writes.
    public func values<T: ColumnValue>(_ column: Int, as type: T.Type = T.self) -> UnsafeBufferPointer<T> {
        let door = columnSet.plan[column].door
        precondition(
            door == T.columnDoor, "Column \(column) is cast through \(door); it has no buffer of \(T.self)")
        return UnsafeBufferPointer(
            start: columnSet.valueBase[column].bindMemory(to: T.self, capacity: columnSet.batchRows), count: rows)
    }

    // MARK: - a cell at a time

    /// The verdict of the cell at (`column`, `row`), whatever its door.
    public func verdict(_ column: Int, row: Int) -> CellVerdict {
        checkRow(row)
        return columnSet.verdictBase[column][row]
    }

    /// The cell at (`column`, `row`) as HyperCast judged it: the value, or the fault. `T` is
    /// the type the column's door is presented as (see ``CellValue``): `Bool`, the integer
    /// and floating-point types, `Decimal`, `UUID`, `Date` (the instant doors),
    /// `DateComponents` (the date, civil date-time and time doors), `Duration`, and `String`
    /// for text.
    ///
    /// - Precondition: the column's door is presented as `T`, and the row is in the batch.
    public func get<T: CellValue>(_ column: Int, row: Int, as type: T.Type = T.self) -> Verdict<T> {
        let door = columnSet.plan[column].door
        precondition(
            T.isPresented(by: door), "Column \(column) is cast through \(door), which is not read as \(T.self)")
        let verdict = self.verdict(column, row: row)
        if let fault = verdict.fault {
            return .fault(fault)
        }
        return .success(T.present(self, column, row))
    }

    /// The value the core wrote for the cell at (`column`, `row`), as the type it wrote.
    func load<Raw>(_ column: Int, _ row: Int, as type: Raw.Type) -> Raw {
        columnSet.valueBase[column].load(fromByteOffset: row * MemoryLayout<Raw>.stride, as: Raw.self)
    }

    /// The door a column is cast through.
    func door(_ column: Int) -> Door {
        columnSet.plan[column].door
    }

    /// The bytes a span names: in the arena when it is flagged, in the base when not.
    private func located(_ span: hypertabular_span) -> UnsafeRawBufferPointer {
        let from = span.len & HYPERTABULAR_SPAN_FLAG != 0 ? arena : base
        return UnsafeRawBufferPointer(
            start: from.map { $0 + Int(span.offset) }, count: Int(span.len & ~HYPERTABULAR_SPAN_FLAG))
    }

    /// The cell of a ``Door/text`` column: its bytes, untrimmed — quotes resolved for
    /// delimited text, and a typed workbook cell said the canonical way (`42`, `true`,
    /// `2024-01-31T10:30:00`, `PT1H30M`) — or `nil` for a cell with no bytes at all, which
    /// is the one way text fails. Zero-copy: the bytes are the input's own, or a workbook's
    /// shared strings, for every cell but one with an escaped quote in it or a typed
    /// workbook cell.
    ///
    /// - Precondition: the column is cast through ``Door/text``, and the row is in the batch.
    public func text(_ column: Int, row: Int) -> UnsafeRawBufferPointer? {
        let door = columnSet.plan[column].door
        precondition(door == .text, "Column \(column) is cast through \(door), not text")
        guard verdict(column, row: row).isOk else {
            return nil
        }
        return located(load(column, row, as: hypertabular_span.self))
    }

    /// The cell of a ``Door/text`` column as a `String`, or `nil` for an empty cell.
    public func string(_ column: Int, row: Int) -> String? {
        text(column, row: row).map { String(decoding: $0, as: UTF8.self) }
    }

    /// The text the cell at (`column`, `row`) was cast from, whatever its door and whatever
    /// its verdict — what a fault's span indexes, and what to show for a value that did not
    /// cast. For a workbook this is a text cell's own text, and what a typed cell was said as
    /// when it failed its door (or went through the text door); a typed cell that cast has
    /// none, and neither has an empty cell. Valid until the next call to ``raw(_:row:)`` or
    /// the next read.
    public func raw(_ column: Int, row: Int) -> UnsafeRawBufferPointer {
        checkRow(row)
        precondition(
            column >= 0 && column < columnSet.plan.count,
            "Column \(column) of a plan of \(columnSet.plan.count) columns")
        let cell = cells[row * perRow + (workbook ? column : columnSet.plan[column].ordinal)]
        if workbook {
            return located(cell)
        }
        let length = Int(cell.len & ~HYPERTABULAR_SPAN_FLAG)
        let written = UnsafeRawBufferPointer(start: base.map { $0 + Int(cell.offset) }, count: length)
        guard cell.len & HYPERTABULAR_SPAN_FLAG != 0 else {
            return written
        }
        // A quoted cell with an escaped quote in it: unescaped, as the core cast it.
        if scratch.count < length {
            scratch.deallocate()
            scratch = .allocate(capacity: max(length, 256))
        }
        let unescaped = hypertabular_delimited_unescape(
            written.baseAddress?.assumingMemoryBound(to: UInt8.self), UInt(length),
            scratch.baseAddress, UInt(scratch.count))
        return UnsafeRawBufferPointer(start: scratch.baseAddress, count: Int(unescaped))
    }
}

/// The closed set of types a cell can be read as with ``Batch/get(_:row:as:)``: the type
/// each door is presented as. The conformances are this binding's to declare — one per
/// presented type, below — so conforming anything else is unsupported.
public protocol CellValue {
    /// Whether a column cast through `door` is presented as this type.
    static func isPresented(by door: Door) -> Bool
    /// The value of the cell at (`column`, `row`), which has cast.
    static func present(_ batch: Batch, _ column: Int, _ row: Int) -> Self
}

extension CellValue where Self: ColumnValue {
    /// Whether `door` is the one that writes this type.
    public static func isPresented(by door: Door) -> Bool { door == columnDoor }
    /// The value as the core wrote it.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> Self {
        batch.load(column, row, as: Self.self)
    }
}

extension Bool: CellValue {
    /// The value as the core wrote it: one byte, `0` or `1`.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> Bool {
        batch.load(column, row, as: UInt8.self) != 0
    }
}

extension Int8: CellValue {}
extension Int16: CellValue {}
extension Int32: CellValue {}
extension Int64: CellValue {}
extension UInt8: CellValue {}
extension UInt16: CellValue {}
extension UInt32: CellValue {}
extension UInt64: CellValue {}
extension Float: CellValue {}
extension Double: CellValue {}

/// 2⁶⁴ — the weight of a decimal's high word, exact in `Decimal`.
private let highWordWeight = Decimal(UInt64.max) + Decimal(1)

extension Decimal: CellValue {
    /// ``Door/decimal``.
    public static func isPresented(by door: Door) -> Bool { door == .decimal }
    /// Exact, as HyperCast's own `Cast.decimal` presents it. The core's magnitude is at most
    /// 2⁹⁶ − 1 (29 digits), inside `Decimal`'s 38-digit mantissa, so nothing is rounded.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> Decimal {
        let raw = batch.load(column, row, as: hypertabular_decimal.self)
        let magnitude = raw.hi == 0 ? Decimal(raw.lo) : Decimal(raw.hi) * highWordWeight + Decimal(raw.lo)
        return Decimal(sign: raw.negative != 0 ? .minus : .plus, exponent: -Int(raw.scale), significand: magnitude)
    }
}

extension UUID: CellValue {
    /// ``Door/uuid``.
    public static func isPresented(by door: Door) -> Bool { door == .uuid }
    /// `uuid_t`'s tuple layout is the RFC byte order exactly, which is what the core writes.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> UUID {
        UUID(uuid: batch.load(column, row, as: uuid_t.self))
    }
}

extension Date: CellValue {
    /// ``Door/timestamp``, ``Door/unix`` and ``Door/excelSerial``.
    public static func isPresented(by door: Door) -> Bool {
        door == .timestamp || door == .unix || door == .excelSerial
    }
    /// An instant — a `Double` of seconds, so sub-microsecond fidelity degrades toward the
    /// window's edges, exactly as HyperCast's own doors present it.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> Date {
        let raw = batch.load(column, row, as: hypertabular_timestamp.self)
        return Date(timeIntervalSince1970: Double(raw.seconds) + Double(raw.nanos) / 1_000_000_000)
    }
}

extension DateComponents: CellValue {
    /// ``Door/date`` and ``Door/dateOrdered`` (year, month and day), ``Door/dateTime`` (year
    /// through nanosecond, with no zone — the text named none and none is invented) and
    /// ``Door/time`` (hour through nanosecond).
    public static func isPresented(by door: Door) -> Bool {
        door == .date || door == .dateOrdered || door == .dateTime || door == .time
    }
    /// The fields the door reads, digit-perfect, with no calendar or zone attached.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> DateComponents {
        switch batch.door(column) {
        case .dateTime:
            let raw = batch.load(column, row, as: hypertabular_civil.self)
            let secondOfDay = raw.nanos_of_day / 1_000_000_000
            return DateComponents(
                year: Int(raw.date.year), month: Int(raw.date.month), day: Int(raw.date.day),
                hour: Int(secondOfDay / 3_600), minute: Int(secondOfDay % 3_600 / 60), second: Int(secondOfDay % 60),
                nanosecond: Int(raw.nanos_of_day % 1_000_000_000))
        case .time:
            let nanosOfDay = batch.load(column, row, as: UInt64.self)
            let (secondOfDay, nano) = nanosOfDay.quotientAndRemainder(dividingBy: 1_000_000_000)
            let (hour, rest) = secondOfDay.quotientAndRemainder(dividingBy: 3_600)
            let (minute, second) = rest.quotientAndRemainder(dividingBy: 60)
            return DateComponents(hour: Int(hour), minute: Int(minute), second: Int(second), nanosecond: Int(nano))
        default:
            let raw = batch.load(column, row, as: hypertabular_date.self)
            return DateComponents(year: Int(raw.year), month: Int(raw.month), day: Int(raw.day))
        }
    }
}

extension Duration: CellValue {
    /// ``Door/duration``.
    public static func isPresented(by door: Door) -> Bool { door == .duration }
    /// The core's nanoseconds carried exactly, both signs.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> Duration {
        let raw = batch.load(column, row, as: hypertabular_duration.self)
        return .seconds(raw.seconds) + .nanoseconds(Int64(raw.nanos))
    }
}

extension String: CellValue {
    /// ``Door/text``.
    public static func isPresented(by door: Door) -> Bool { door == .text }
    /// The cell's text, as ``Batch/string(_:row:)`` says it.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> String {
        batch.string(column, row: row) ?? ""
    }
}
