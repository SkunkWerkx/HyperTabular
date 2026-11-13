import Foundation
import HyperTabularCore

/// The bit of a span's `len` that says its text is in the arena (`HyperTabularSpan_FLAG`),
/// as the field's own type: the header spells it as a plain literal, which Swift imports as
/// `Int`.
let spanFlag = UInt32(HyperTabularSpan_FLAG)

/// Where a cell's verdict came from: the door, or something that spared it being asked. The raw
/// values are the core's codes.
public enum Origin: UInt16, Equatable, Sendable {
    /// A door's verdict on the cell's text: every cell that cast, and every refusal.
    case cell = 0
    /// A declared token: empty, and no door asked.
    case token = 1
    /// A cell its row does not have, under ``ShortRows/pad`` or past a sheet row's last cell:
    /// empty, and no door asked.
    case padded = 2
    /// A workbook formula with no cached value: empty, and no door asked.
    case uncached = 3
}

/// One entry of ``Batch/faults()``: a cell that went wrong, or a row longer than the width.
public struct Problem: Equatable, Sendable {
    /// The row, from zero, in the batch.
    public let row: Int
    /// Delimited text: the record's first physical line. A sheet: its absolute row. From one.
    public let line: Int
    /// The column of the plan; `nil` for an entry about the row as a whole.
    public let planColumn: Int?
    /// The source column, from zero; a sheet's own, so `A` is 0 wherever a range starts. `nil`
    /// for an entry about the row as a whole.
    public let sourceColumn: Int?
    /// Why the verdict is what it is: malformed, out of range, or empty for a formula with no
    /// cached value. `nil` for an entry about the row as a whole, and for a violation, which
    /// ``keyword`` names.
    public let reason: CastFailure?
    /// HyperCast's raw detail code, 0 for none; one this binding does not name is kept here.
    public let detailCode: UInt32
    /// Where the verdict came from. An entry about the row as a whole says ``Origin/cell``.
    public let origin: Origin
    /// Byte offset of the offending span in the cell's decoded, unescaped text; 0 where there is
    /// none.
    public let offset: Int
    /// Byte length of the offending span; 0 where there is none.
    public let length: Int
    /// A long row's width by its header or first record; otherwise 0.
    public let expected: Int
    /// How many cells a long row had; otherwise 0.
    public let found: Int
    /// The constraint a cell broke, in a column that declares them: `nil` for anything else.
    public let keyword: Keyword?

    /// What failed, named: `nil` for no detail, and otherwise HyperCast's own value, which keeps a
    /// code this binding does not name.
    public var detail: FaultDetail? { detailCode == 0 ? nil : FaultDetail(code: detailCode) }
}

/// The rows one ``DelimitedReader/read()`` or ``Sheet/read()`` delivered, as typed columns:
/// the same type for delimited text and for a sheet of a workbook.
///
/// A column is handed out whole — ``values(_:as:)``, with ``verdicts(_:)`` beside it — or a
/// cell at a time through ``get(_:row:as:)``, which answers with HyperCast's own `Verdict`
/// (``checked(_:row:as:)`` and its `CheckedVerdict` for a column that declares constraints).
/// A batch is also the collection of its rows: each a ``Row``, the same accessors read
/// across.
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
/// So a ``Row``, a walk of a batch's rows and ``faults()`` belong to the batch they were asked of:
/// going on with one after the next read is a precondition failure, never an answer about the next
/// batch.
public final class Batch {
    private let columnSet: Columns
    private let workbook: Bool
    /// What a plan's ordinal is short of the sheet's own column: a range's start.
    private let firstColumn: Int
    /// What ``faults()`` lists long rows under, in a read that reports them.
    private let reportsLongRows: Bool
    /// The width a long row is held to, as the read last knew it.
    private var expected: UInt32 = 0
    /// Moves whenever the batch is over, so that what was asked of it never answers for the next.
    private(set) var generation: UInt64 = 0

    /// How many rows the batch holds: never zero for a batch a read returned, and zero once
    /// the reader has moved on.
    public private(set) var rows = 0

    // The batch in hand, as the core left it: the table that locates each cell — perRow
    // entries a row, the last of them the row's line — what an unflagged span indexes (the
    // window of input, or a workbook's shared strings), and what a flagged one indexes.
    private var cells: UnsafeBufferPointer<HyperTabularSpan> = UnsafeBufferPointer(start: nil, count: 0)
    private var perRow = 0
    private var base: UnsafeRawPointer?
    private var arena: UnsafeRawPointer?
    private var scratch = UnsafeMutableBufferPointer<UInt8>(start: nil, count: 0)
    /// What the spans above point into, where the batch can keep it alive by holding it: a sheet's
    /// buffers and its workbook's strings.
    private let reaches: [AnyObject]
    /// Frees the buffers a reader that has gone left behind, once nothing can read them.
    private var leftBehind: (() -> Void)?

    init(
        _ columns: Columns, workbook: Bool, firstColumn: Int = 0, reportsLongRows: Bool = false,
        reaches: [AnyObject] = []
    ) {
        self.columnSet = columns
        self.workbook = workbook
        self.firstColumn = firstColumn
        self.reportsLongRows = reportsLongRows
        self.reaches = reaches
    }

    deinit {
        scratch.deallocate()
        leftBehind?()
    }

    /// Takes over a departing reader's buffers, which this batch's spans point into. Since the reader
    /// is gone no read can end the batch, so it stays readable for as long as anything holds it, and
    /// the buffers go with it. Where nothing else holds it, that is straight after the reader.
    func outlive(_ free: @escaping () -> Void) {
        leftBehind = free
    }

    /// Makes the batch the rows the core just wrote.
    func fill(
        rows: Int, cells: UnsafeMutableBufferPointer<HyperTabularSpan>, perRow: Int,
        base: UnsafeRawPointer?, arena: UnsafeMutableBufferPointer<UInt8>, expected: UInt32 = 0
    ) -> Batch {
        generation &+= 1
        self.rows = rows
        self.expected = expected
        self.cells = UnsafeBufferPointer(cells)
        self.perRow = perRow
        self.base = base
        self.arena = UnsafeRawPointer(arena.baseAddress)
        return self
    }

    /// Ends the batch in hand: the reader is moving on.
    func clear() {
        generation &+= 1
        rows = 0
    }

    /// The plan the batch was read through: column `i` of the batch is `columns[i]`.
    public var columns: [Column] { columnSet.plan }

    func checkRow(_ row: Int) {
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

    /// A column's verdicts, one per row, as the core wrote them. A verdict that `isOk` is a value.
    /// On a column that declares constraints, one that `isViolation` cast and broke one, and
    /// ``checked(_:row:as:)`` names it. Any other is the cell's fault, its span in the cell's text.
    public func verdicts(_ column: Int) -> UnsafeBufferPointer<CellVerdict> {
        UnsafeBufferPointer(start: columnSet.verdictBase[column], count: rows)
    }

    /// A column's values, one per row, exactly as the core wrote them — for the doors whose
    /// value is a primitive (``ColumnValue``): ``Door/bool`` as `Bool`, the integer and
    /// floating-point doors as their own type, and ``Door/time`` as `UInt64` nanoseconds since
    /// midnight. The value of a row whose verdict is not ok is zero. Columns of other doors are
    /// read a cell at a time, with ``get(_:row:as:)``: no type they are presented as is laid
    /// out as the core writes them.
    ///
    /// - Precondition: `T` is the type the column's door writes.
    public func values<T: ColumnValue>(_ column: Int, as type: T.Type = T.self) -> UnsafeBufferPointer<T> {
        let door = columnSet.plan[column].door
        precondition(
            Self.lends(T.self, door), "Column \(column) is cast through \(door); it has no buffer of \(T.self)")
        return UnsafeBufferPointer(
            start: columnSet.valueBase[column].bindMemory(to: T.self, capacity: columnSet.batchRows), count: rows)
    }

    /// Whether a column cast through `door` is a buffer of `T`.
    static func lends<T: ColumnValue>(_ type: T.Type, _ door: Door) -> Bool {
        door == T.columnDoor || (T.self == UInt64.self && door == .time)
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
    /// - Precondition: the column's door is presented as `T`, the column declares no constraints
    ///   (``checked(_:row:as:)`` reads one), and the row is in the batch.
    public func get<T: CellValue>(_ column: Int, row: Int, as type: T.Type = T.self) -> Verdict<T> {
        let door = columnSet.plan[column].door
        precondition(
            T.isPresented(by: door), "Column \(column) is cast through \(door), which is not read as \(T.self)")
        unchecked(column)
        if let fault = fault(column, row: row) {
            return .fault(fault)
        }
        return .success(T.present(self, column, row))
    }

    /// The cell at (`column`, `row`) as HyperCast's checked doors judge it: the value, with
    /// whether `multipleOf` rounding adjusted it and the `enum` entry it matched; or the fault,
    /// for text that isn't the type; or the violation, naming the keyword broken, with the value
    /// as cast. A text `enum` match reads as the entry as declared, the caller's own string. A
    /// column that declares no constraints reads too: every cell that cast passes, unadjusted.
    ///
    /// - Precondition: the column's door is a checked one (a numeric door or text) presented as
    ///   `T`, and the row is in the batch.
    public func checked<T: CellValue & CheckedCastTarget>(
        _ column: Int, row: Int, as type: T.Type = T.self
    ) -> CheckedVerdict<T> {
        let door = columnSet.plan[column].door
        precondition(
            (Column.isNumeric(door) || door == .text) && T.isPresented(by: door),
            "Column \(column) is cast through \(door), which has no checked door read as \(T.self)")
        let verdict = self.verdict(column, row: row)
        if let fault = fault(column, row: row) {
            return .rejection(.fault(fault))
        }
        let value = T.present(self, column, row)
        guard let checks = columnSet.checkBase[column] else {
            return .success(Checked(value: value, adjusted: false, enumIndex: nil))
        }
        let check = checks[row]
        if verdict.isViolation {
            guard let keyword = Keyword(rawValue: check.keyword) else {
                preconditionFailure(Workbook.contractViolation)
            }
            return .rejection(
                .violation(Violation(keyword: keyword, value: value, offset: verdict.offset, length: verdict.length)))
        }
        let index = check.enum_index == UInt32.max ? nil : Int(check.enum_index)
        if door == .text, let index, let entry = columnSet.plan[column].constraints?.entries?[index] as? T {
            return .success(Checked(value: entry, adjusted: check.adjusted != 0, enumIndex: index))
        }
        return .success(Checked(value: value, adjusted: check.adjusted != 0, enumIndex: index))
    }

    /// Traps when a column declares constraints: a plain getter would read a violation as passed.
    func unchecked(_ column: Int) {
        precondition(
            !columnSet.plan[column].isChecked,
            "Column \(column) declares constraints: read it with checked(_:row:as:).")
    }

    /// The fault of the cell at (`column`, `row`), with what failed there — `nil` for a cell that
    /// cast, and for one that cast and broke a constraint, which is no fault. With notes off
    /// (``Dialect/notes``, ``SheetOptions/notes``) it has no detail.
    public func fault(_ column: Int, row: Int) -> Fault? {
        let verdict = self.verdict(column, row: row)
        guard !verdict.isOk, !verdict.isViolation else { return nil }
        return Interop.fault(
            code: Int32(truncatingIfNeeded: verdict.raw.reason), offset: verdict.raw.offset, length: verdict.raw.len,
            detail: UInt32(note(column, row).detail))
    }

    /// The cell's note: zero, with notes off.
    func note(_ column: Int, _ row: Int) -> HyperTabularCellNote {
        columnSet.noteBase?[column][row] ?? HyperTabularCellNote()
    }

    /// Where the verdict of the cell at (`column`, `row`) came from. With notes off every cell says
    /// ``Origin/cell``: a token's or a padded cell's verdict is still empty, and nothing was kept
    /// of why.
    ///
    /// - Precondition: the row is in the batch.
    public func origin(_ column: Int, row: Int) -> Origin {
        checkRow(row)
        return Origin(rawValue: note(column, row).origin) ?? .cell
    }

    /// What went wrong in this batch, in order by row and then by column, a row's own entry first:
    /// cells at `malformed` or `outOfRange`, cells that broke a constraint their column declares
    /// (``Problem/keyword``), formulas with no cached value (``Origin/uncached``), and
    /// rows longer than the width under ``LongRows/report``. Blanks, tokens, padding and
    /// ``LongRows/truncate`` are not listed. A row the empty-row filter dropped cannot be named. With
    /// notes off a refused cell has no detail, and an uncached one is not listed.
    ///
    /// The list belongs to this batch: going on with it after the next read is a precondition
    /// failure, and never answers for the next batch.
    public func faults() -> Faults {
        Faults(batch: self, generation: generation)
    }

    /// Traps when what was asked of the batch has outlived the rows it was asked of.
    func stillCurrent(_ asked: UInt64) {
        precondition(asked == generation, "The batch this walks is over: the reader has read on.")
    }

    /// One call's worth of problems, into `held`: how many, and the cursor moved on.
    func nextFaults(_ cursor: inout UInt64, into held: UnsafeMutableBufferPointer<HyperTabularRawProblem>) -> Int {
        let widths = reportsLongRows ? columnSet.rowWidths : nil
        var written: UInt = 0
        let code = hypertabular_faults(
            columnSet.buffers, UInt(columnSet.plan.count), UInt(rows), widths, expected,
            widths == nil ? LongRows.reject.rawValue : LongRows.report.rawValue, &cursor, held.baseAddress,
            UInt(held.count), &written)
        guard code == HYPERTABULAR_OK else {
            preconditionFailure(Workbook.contractViolation)
        }
        return Int(written)
    }

    /// A problem as this binding says it.
    func problem(_ raw: HyperTabularRawProblem) -> Problem {
        let row = Int(raw.row)
        guard raw.plan_column != UInt32.max else {
            return Problem(
                row: row, line: line(row), planColumn: nil, sourceColumn: nil, reason: nil, detailCode: 0,
                origin: .cell, offset: 0, length: 0, expected: Int(raw.expected), found: Int(raw.found), keyword: nil)
        }
        let column = Int(raw.plan_column)
        let violated = raw.reason == UInt32(HyperTabularCellVerdict_VIOLATION)
        return Problem(
            row: row, line: line(row), planColumn: column, sourceColumn: columnSet.plan[column].ordinal + firstColumn,
            reason: violated
                ? nil : Interop.fault(code: Int32(truncatingIfNeeded: raw.reason), offset: 0, length: 0).reason,
            detailCode: UInt32(raw.detail), origin: Origin(rawValue: raw.origin) ?? .cell, offset: Int(raw.offset),
            length: Int(raw.len), expected: Int(raw.expected), found: Int(raw.found),
            keyword: violated ? Keyword(rawValue: raw.keyword) : nil)
    }

    /// The value the core wrote for the cell at (`column`, `row`), as the type it wrote.
    func load<Raw>(_ column: Int, _ row: Int, as type: Raw.Type) -> Raw {
        columnSet.valueBase[column].load(fromByteOffset: row * MemoryLayout<Raw>.stride, as: Raw.self)
    }

    /// The bytes the core wrote for the cell at (`column`, `row`) — one value of the column's
    /// door, `size` bytes — for HyperCast's ``Interop`` readers to present.
    func bytes(_ column: Int, _ row: Int, size: Int) -> UnsafeRawBufferPointer {
        UnsafeRawBufferPointer(start: columnSet.valueBase[column] + row * size, count: size)
    }

    /// The door a column is cast through.
    func door(_ column: Int) -> Door {
        columnSet.plan[column].door
    }

    /// The bytes a span names: in the arena when it is flagged, in the base when not.
    private func located(_ span: HyperTabularSpan) -> UnsafeRawBufferPointer {
        let from = span.len & spanFlag != 0 ? arena : base
        return UnsafeRawBufferPointer(
            start: from.map { $0 + Int(span.offset) }, count: Int(span.len & ~spanFlag))
    }

    /// The cell of a ``Door/text`` column: its bytes, untrimmed — quotes resolved for
    /// delimited text, and a typed workbook cell said the canonical way (`42`, `true`,
    /// `2024-01-31T10:30:00`, `PT1H30M`) — or `nil` for a cell with no bytes at all, which
    /// is the one way text fails. Zero-copy: the bytes are the input's own, or a workbook's
    /// shared strings, for every cell but one with an escaped quote in it or a typed
    /// workbook cell.
    ///
    /// - Precondition: the column is cast through ``Door/text`` and declares no constraints
    ///   (``checked(_:row:as:)`` reads one), and the row is in the batch.
    public func text(_ column: Int, row: Int) -> UnsafeRawBufferPointer? {
        let door = columnSet.plan[column].door
        precondition(door == .text, "Column \(column) is cast through \(door), not text")
        unchecked(column)
        return cellText(column, row)
    }

    /// The cell of a ``Door/text`` column as a `String`, or `nil` for an empty cell.
    ///
    /// - Precondition: as ``text(_:row:)``.
    public func string(_ column: Int, row: Int) -> String? {
        text(column, row: row).map { String(decoding: $0, as: UTF8.self) }
    }

    /// A text cell's bytes, a violation's included: `nil` for a cell with none.
    func cellText(_ column: Int, _ row: Int) -> UnsafeRawBufferPointer? {
        let verdict = self.verdict(column, row: row)
        guard verdict.isOk || verdict.isViolation else {
            return nil
        }
        return located(load(column, row, as: HyperTabularSpan.self))
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
        let length = Int(cell.len & ~spanFlag)
        let written = UnsafeRawBufferPointer(start: base.map { $0 + Int(cell.offset) }, count: length)
        guard cell.len & spanFlag != 0 else {
            return written
        }
        // A quoted cell with an escaped quote in it: unescaped, as the core cast it.
        if scratch.count < length {
            scratch.deallocate()
            scratch = .allocate(capacity: Swift.max(length, 256))
        }
        let unescaped = hypertabular_delimited_unescape(
            written.baseAddress?.assumingMemoryBound(to: UInt8.self), UInt(length),
            scratch.baseAddress, UInt(scratch.count))
        return UnsafeRawBufferPointer(start: scratch.baseAddress, count: Int(unescaped))
    }
}

extension Batch: RandomAccessCollection {
    /// The first row: `0`.
    public var startIndex: Int { 0 }

    /// One past the last row: ``rows``.
    public var endIndex: Int { rows }

    /// Row `index` of the batch, read across: valid while the batch is.
    ///
    /// - Precondition: the row is in the batch.
    public subscript(index: Int) -> Row {
        checkRow(index)
        return Row(batch: self, index: index, generation: generation)
    }

    /// Some of the batch's rows, cut from it: `dropFirst`, `prefix`, `batch[1..<3]` and the like.
    ///
    /// - Precondition: the range is in the batch.
    public subscript(bounds: Range<Int>) -> Rows {
        precondition(
            bounds.lowerBound >= 0 && bounds.upperBound <= rows,
            "Rows \(bounds) are outside the batch in hand (\(rows) rows)")
        return Rows(batch: self, generation: generation, bounds: bounds)
    }

    /// The batch's rows, last first. It belongs to this batch, as a slice does.
    public func reversed() -> ReversedCollection<Rows> {
        self[startIndex..<endIndex].reversed()
    }

    /// A walk of rows, which belongs to their batch: going on with it after the next read is a
    /// precondition failure, and never answers for the next batch.
    public struct Iterator: IteratorProtocol {
        private let batch: Batch
        private let generation: UInt64
        private var index: Int
        private let end: Int

        init(_ batch: Batch, generation: UInt64, bounds: Range<Int>) {
            self.batch = batch
            self.generation = generation
            index = bounds.lowerBound
            end = bounds.upperBound
        }

        /// The next row, or `nil` when there are no more.
        public mutating func next() -> Row? {
            batch.stillCurrent(generation)
            guard index < end else { return nil }
            defer { index += 1 }
            return Row(batch: batch, index: index, generation: generation)
        }
    }

    /// A walk of the batch's rows.
    public func makeIterator() -> Iterator {
        Iterator(self, generation: generation, bounds: 0..<rows)
    }

    /// Some of a batch's rows, which belong to that batch as a ``Row`` does: reading them after the next
    /// read, or cutting them again, is a precondition failure, and never answers for the next batch.
    /// Indices are the batch's own. A `Batch` itself is the reader's one batch and always the last read.
    public struct Rows: RandomAccessCollection {
        private let batch: Batch
        private let generation: UInt64
        /// The first row: its index in the batch.
        public let startIndex: Int
        /// One past the last row.
        public let endIndex: Int

        init(batch: Batch, generation: UInt64, bounds: Range<Int>) {
            self.batch = batch
            self.generation = generation
            startIndex = bounds.lowerBound
            endIndex = bounds.upperBound
        }

        /// Row `index` of the batch.
        ///
        /// - Precondition: the row is among these, and the batch is still the one they were cut from.
        public subscript(index: Int) -> Row {
            batch.stillCurrent(generation)
            precondition(
                index >= startIndex && index < endIndex, "Row \(index) is outside rows \(startIndex)..<\(endIndex)")
            return Row(batch: batch, index: index, generation: generation)
        }

        /// Fewer of these rows.
        ///
        /// - Precondition: the range is among these, and the batch is still the one they were cut from.
        public subscript(bounds: Range<Int>) -> Rows {
            batch.stillCurrent(generation)
            precondition(
                bounds.lowerBound >= startIndex && bounds.upperBound <= endIndex,
                "Rows \(bounds) are outside rows \(startIndex)..<\(endIndex)")
            return Rows(batch: batch, generation: generation, bounds: bounds)
        }

        /// A walk of these rows.
        public func makeIterator() -> Iterator {
            Iterator(batch, generation: generation, bounds: startIndex..<endIndex)
        }
    }
}

/// The list ``Batch/faults()`` answers with: the core's own, walked a few entries a call. It belongs to its batch: starting over or going on after the next
/// read is a precondition failure, and never answers for the next batch.
public struct Faults: Sequence {
    private let batch: Batch
    private let generation: UInt64

    init(batch: Batch, generation: UInt64) {
        self.batch = batch
        self.generation = generation
    }

    /// A walk of the list.
    public struct Iterator: IteratorProtocol {
        private static let held = 64
        private let batch: Batch
        private let generation: UInt64
        private var entries: [HyperTabularRawProblem] = []
        private var position = 0
        private var cursor: UInt64 = 0
        private var more = true

        init(batch: Batch, generation: UInt64) {
            self.batch = batch
            self.generation = generation
        }

        /// The next problem, or `nil` when the list has no more.
        public mutating func next() -> Problem? {
            batch.stillCurrent(generation)
            if position == entries.count {
                guard more else { return nil }
                var taken = [HyperTabularRawProblem](repeating: HyperTabularRawProblem(), count: Self.held)
                var at = cursor
                let written = taken.withUnsafeMutableBufferPointer { batch.nextFaults(&at, into: $0) }
                taken.removeLast(Self.held - written)
                entries = taken
                cursor = at
                position = 0
                more = written == Self.held
                if written == 0 { return nil }
            }
            defer { position += 1 }
            return batch.problem(entries[position])
        }
    }

    /// A walk of the list, from its start.
    public func makeIterator() -> Iterator {
        batch.stillCurrent(generation)
        return Iterator(batch: batch, generation: generation)
    }
}

/// One row of a ``Batch``, read across its columns: the batch's own accessors with the row
/// fixed. A row is a value with no storage of its own — reading a batch a row at a time
/// allocates nothing — and is valid exactly as long as its batch: reading a row after the
/// reader's next read is a precondition failure, and never answers for the next batch.
///
/// ```swift
/// for row in batch {
///     print(row.line, row.string(1) ?? "", row.get(2, as: Double.self))
/// }
/// ```
public struct Row {
    private let batch: Batch
    private let generation: UInt64

    /// The row's place in its batch.
    public let index: Int

    init(batch: Batch, index: Int, generation: UInt64) {
        self.batch = batch
        self.index = index
        self.generation = generation
    }

    /// The batch, for as long as the row is of it.
    private var current: Batch {
        batch.stillCurrent(generation)
        return batch
    }

    /// Where the row came from: ``Batch/line(_:)``.
    public var line: Int { current.line(index) }

    /// The verdict of the row's cell in `column`: ``Batch/verdict(_:row:)``.
    public func verdict(_ column: Int) -> CellVerdict {
        current.verdict(column, row: index)
    }

    /// The fault of the row's cell in `column`, with what failed there: ``Batch/fault(_:row:)``.
    public func fault(_ column: Int) -> Fault? {
        current.fault(column, row: index)
    }

    /// Where the verdict of the row's cell in `column` came from: ``Batch/origin(_:row:)``.
    public func origin(_ column: Int) -> Origin {
        current.origin(column, row: index)
    }

    /// The row's cell in `column` as HyperCast judged it: ``Batch/get(_:row:as:)``.
    ///
    /// - Precondition: the column's door is presented as `T`.
    public func get<T: CellValue>(_ column: Int, as type: T.Type = T.self) -> Verdict<T> {
        current.get(column, row: index, as: type)
    }

    /// The row's cell as HyperCast's checked doors judge it: ``Batch/checked(_:row:as:)``.
    ///
    /// - Precondition: the column's door is a checked one presented as `T`.
    public func checked<T: CellValue & CheckedCastTarget>(_ column: Int, as type: T.Type = T.self) -> CheckedVerdict<T>
    {
        current.checked(column, row: index, as: type)
    }

    /// The row's cell in a ``Door/text`` column, zero-copy: ``Batch/text(_:row:)``.
    ///
    /// - Precondition: the column is cast through ``Door/text``.
    public func text(_ column: Int) -> UnsafeRawBufferPointer? {
        current.text(column, row: index)
    }

    /// The row's cell in a ``Door/text`` column as a `String`: ``Batch/string(_:row:)``.
    ///
    /// - Precondition: the column is cast through ``Door/text``.
    public func string(_ column: Int) -> String? {
        current.string(column, row: index)
    }

    /// The text the row's cell in `column` was cast from: ``Batch/raw(_:row:)``, valid until
    /// the next call to it.
    public func raw(_ column: Int) -> UnsafeRawBufferPointer {
        current.raw(column, row: index)
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

extension Decimal: CellValue {
    /// ``Door/decimal``.
    public static func isPresented(by door: Door) -> Bool { door == .decimal }
    /// Exact, as HyperCast's own `Cast.decimal` presents it. The core's magnitude is at most
    /// 2⁹⁶ − 1 (29 digits), inside `Decimal`'s 38-digit mantissa, so nothing is rounded.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> Decimal {
        Interop.decimal(batch.bytes(column, row, size: 16))
    }
}

extension UUID: CellValue {
    /// ``Door/uuid``.
    public static func isPresented(by door: Door) -> Bool { door == .uuid }
    /// The 16 bytes the core writes, in RFC 9562 order.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> UUID {
        Interop.uuid(batch.bytes(column, row, size: 16))
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
        Interop.instant(batch.bytes(column, row, size: 16))
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
        case .dateTime: Interop.civil(batch.bytes(column, row, size: 16))
        case .time: Interop.time(batch.bytes(column, row, size: 8))
        default: Interop.date(batch.bytes(column, row, size: 4))
        }
    }
}

extension Duration: CellValue {
    /// ``Door/duration``.
    public static func isPresented(by door: Door) -> Bool { door == .duration }
    /// The core's nanoseconds carried exactly, both signs.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> Duration {
        Interop.duration(batch.bytes(column, row, size: 16))
    }
}

extension String: CellValue {
    /// ``Door/text``.
    public static func isPresented(by door: Door) -> Bool { door == .text }
    /// The cell's text, as ``Batch/string(_:row:)`` says it.
    public static func present(_ batch: Batch, _ column: Int, _ row: Int) -> String {
        batch.cellText(column, row).map { String(decoding: $0, as: UTF8.self) } ?? ""
    }
}
