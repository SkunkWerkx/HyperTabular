import Foundation
import HyperTabularCore

/// Delimited text — CSV, TSV, any single-byte ASCII separator — read a batch at a time into
/// typed columns, every cell a HyperCast `Verdict`.
///
/// The native core (`libhypertabular`) owns no memory and reads no files. It is handed a
/// chunk of input and the buffers to fill, casts each plan column in one native loop, and
/// says how many rows it wrote and how many bytes it is finished with. Everything else is
/// here, which is the point of the design: this class allocates the input buffer, one value
/// array and one verdict array per column, the table that locates each cell, and the arena
/// for the rare escaped cell — once, reused for every batch — and puts what the core did
/// not consume back in front of it. The native boundary is crossed once per batch, not once
/// per cell.
///
/// ```swift
/// let reader = try DelimitedReader(contentsOfFile: "orders.csv", dialect: .csv, plan: [.i32(0), .text(1), .f64(2)])
/// while try reader.read() {
///     for row in 0..<reader.rows {
///         switch reader.f64(2, row: row) {
///         case .success(let score): print(reader.string(1, row: row) ?? "", score)
///         case .fault(let fault): print("row \(row): \(fault.reason) in", String(decoding: reader.raw(2, row: row), as: UTF8.self))
///         }
///     }
/// }
/// ```
///
/// A value that does not cast is that cell's verdict, and the read goes on. Input that is
/// not rows of cells at all — a record of the wrong width, a quote never closed — is a
/// ``TabularError``, thrown after every intact row before it has been delivered.
///
/// A caller bug — a column read through an accessor that is not its door's, a row outside
/// the batch, a separator the core cannot honour — is a precondition failure, never an
/// error to catch: the same line HyperCast's Swift binding draws.
///
/// Buffers handed out for a batch — ``values(_:as:)``, ``verdicts(_:)``,
/// ``text(_:row:)``, ``raw(_:row:)`` — point into the reader's own memory and are valid
/// until the next ``read()`` (``raw(_:row:)`` until the next call to itself, too) and no
/// longer than the reader. Not thread-safe.
public final class DelimitedReader {
    /// The row ceiling: a single record larger than this is a ``TabularFailure/rowTooLong``.
    public static let maxRowBytes = 1 << 30

    /// The initial read buffer for a streamed source.
    public static let defaultBufferBytes = 256 * 1024

    /// Rows per batch unless told otherwise.
    public static let defaultBatchRows = 4096

    /// Where streamed input comes from: called with room to write into, it writes as many
    /// bytes as it has (at least one, unless the input is over) and returns how many — `0`
    /// once the input is exhausted. Whatever it throws comes out of the reader unchanged.
    public typealias Source = (UnsafeMutableRawBufferPointer) throws -> Int

    private static let contractViolation =
        "libhypertabular reported a contract violation — a binding bug, please report it"

    /// The most input one native call may be shown: the core's spans are 31 bits.
    private static let callLimit = Int(Int32.max)

    /// 2⁶⁴ — the weight of a decimal's high word, exact in `Decimal`.
    private static let highWordWeight = Decimal(UInt64.max) + Decimal(1)

    // What this file assumes of the shapes hypertabular.h declares, held once: the state
    // block is the size the core says it is, and each value is as wide as the core writes it.
    private static let layoutChecked: Bool = {
        precondition(
            MemoryLayout<hypertabular_delimited_state>.size == Int(hypertabular_delimited_state_size()),
            "hypertabular.h and the linked libhypertabular disagree on the state block")
        precondition(
            MemoryLayout<hypertabular_cell_verdict>.stride == 12 && MemoryLayout<CellVerdict>.stride == 12
                && MemoryLayout<hypertabular_span>.stride == 8
                && MemoryLayout<hypertabular_column_spec>.stride == 44
                && MemoryLayout<hypertabular_timestamp>.stride == 16
                && MemoryLayout<hypertabular_date>.stride == 4
                && MemoryLayout<hypertabular_civil>.stride == 16
                && MemoryLayout<hypertabular_duration>.stride == 16
                && MemoryLayout<hypertabular_decimal>.stride == 16
                && MemoryLayout<uuid_t>.stride == 16,
            "hypertabular.h's shapes are not the sizes the core writes")
        return true
    }()

    private let plan: [Column]
    private let batchRows: Int
    private let hasHeader: Bool
    /// Cell-table entries one row takes: the widest ordinal the plan reads, plus two.
    private let perRow: Int

    // Everything the core is handed, allocated once and never moved: it is given addresses.
    private let state: UnsafeMutablePointer<hypertabular_delimited_state>
    private let specs: UnsafeMutablePointer<hypertabular_column_spec>
    private let buffers: UnsafeMutablePointer<hypertabular_column_buffer>
    private let valueBase: [UnsafeMutableRawPointer]
    private let verdictBase: [UnsafeMutablePointer<CellVerdict>]
    private var cells: UnsafeMutableBufferPointer<hypertabular_span>
    private var arena: UnsafeMutableBufferPointer<UInt8>
    private var scratch: UnsafeMutableBufferPointer<UInt8>

    // The source: a stream read into `input`, or memory read in place.
    private let source: Source?
    private var input: UnsafeMutableRawBufferPointer
    private let ownsInput: Bool
    private var start = 0
    private var end: Int
    private var eof: Bool
    /// The most of `input` the core is shown at once.
    private var windowLimit: Int

    /// The input the batch in hand's spans index.
    private var window: UnsafeRawPointer?
    private var failure: TabularError?

    /// Rows in the batch in hand; `0` before the first ``read()`` and after the last.
    public private(set) var rows = 0

    /// The header's names, when the dialect declares one — empty for an input with no
    /// record — and `nil` when it declares none.
    public private(set) var header: [String]?

    // MARK: - opening

    /// Reads UTF-8 delimited text already in memory: an array, a `Data`, a string's `utf8`.
    /// The bytes are copied once, into the reader's own buffer, so the reader does not
    /// depend on the collection outliving it; ``init(bytesNoCopy:dialect:plan:batchRows:)``
    /// reads in place instead.
    ///
    /// - Parameters:
    ///   - bytes: The text.
    ///   - dialect: The declared dialect.
    ///   - plan: The output columns, in output order.
    ///   - batchRows: Rows per batch.
    /// - Throws: ``TabularError`` when the header record is structurally broken.
    public convenience init<Bytes: Collection>(
        bytes: Bytes, dialect: Dialect, plan: [Column],
        batchRows: Int = DelimitedReader.defaultBatchRows
    ) throws where Bytes.Element == UInt8 {
        try self.init(
            bytes: bytes, dialect: dialect, plan: plan, batchRows: batchRows,
            windowBytes: Self.callLimit)
    }

    /// ``init(bytes:dialect:plan:batchRows:)``, showing the core `windowBytes` of the text
    /// at a time to begin with — what a text over 2 GiB gets, made reachable for the tests.
    convenience init<Bytes: Collection>(
        bytes: Bytes, dialect: Dialect, plan: [Column], batchRows: Int, windowBytes: Int
    ) throws where Bytes.Element == UInt8 {
        let copy = UnsafeMutableRawBufferPointer.allocate(byteCount: bytes.count, alignment: 1)
        copy.copyBytes(from: bytes)
        self.init(
            dialect: dialect, plan: plan, batchRows: batchRows, input: copy, ownsInput: true,
            filled: copy.count, source: nil, windowLimit: windowBytes)
        try readHeaderIfDeclared()
    }

    /// Reads UTF-8 delimited text already in memory, in place — nothing is copied, and a
    /// text cell is a view of the caller's own bytes.
    ///
    /// The caller keeps `bytes` alive and unchanged for as long as the reader is used.
    ///
    /// - Parameters:
    ///   - bytes: The text.
    ///   - dialect: The declared dialect.
    ///   - plan: The output columns, in output order.
    ///   - batchRows: Rows per batch.
    /// - Throws: ``TabularError`` when the header record is structurally broken.
    public convenience init(
        bytesNoCopy bytes: UnsafeRawBufferPointer, dialect: Dialect, plan: [Column],
        batchRows: Int = DelimitedReader.defaultBatchRows
    ) throws {
        // Never written through: in memory the core only reads the input.
        self.init(
            dialect: dialect, plan: plan, batchRows: batchRows,
            input: UnsafeMutableRawBufferPointer(mutating: bytes), ownsInput: false,
            filled: bytes.count, source: nil, windowLimit: Self.callLimit)
        try readHeaderIfDeclared()
    }

    /// Reads UTF-8 delimited text from a stream of any kind — a socket, a decompressor, a
    /// file the caller opened — through a closure that fills the reader's own buffer, so
    /// nothing is copied on the way in.
    ///
    /// - Parameters:
    ///   - source: Where the bytes come from; see ``Source``.
    ///   - dialect: The declared dialect.
    ///   - plan: The output columns, in output order.
    ///   - batchRows: Rows per batch.
    ///   - bufferBytes: The initial read buffer; it doubles when a record does not fit.
    /// - Throws: ``TabularError`` when the header record is structurally broken, and
    ///   whatever `source` throws.
    public convenience init(
        reading source: @escaping Source, dialect: Dialect, plan: [Column],
        batchRows: Int = DelimitedReader.defaultBatchRows,
        bufferBytes: Int = DelimitedReader.defaultBufferBytes
    ) throws {
        precondition(bufferBytes > 0, "bufferBytes must be positive; got \(bufferBytes)")
        let buffer = UnsafeMutableRawBufferPointer.allocate(
            byteCount: min(bufferBytes, Self.maxRowBytes), alignment: 1)
        self.init(
            dialect: dialect, plan: plan, batchRows: batchRows, input: buffer, ownsInput: true,
            filled: 0, source: source, windowLimit: Self.callLimit)
        try readHeaderIfDeclared()
    }

    /// Opens a file of UTF-8 delimited text and reads it through a buffer — the file is
    /// never in memory whole. It is closed when the reader is released.
    ///
    /// - Parameters:
    ///   - path: The file.
    ///   - dialect: The declared dialect.
    ///   - plan: The output columns, in output order.
    ///   - batchRows: Rows per batch.
    ///   - bufferBytes: The initial read buffer; it doubles when a record does not fit.
    /// - Throws: Foundation's error when the file cannot be opened or read, and
    ///   ``TabularError`` when the header record is structurally broken.
    public convenience init(
        contentsOfFile path: String, dialect: Dialect, plan: [Column],
        batchRows: Int = DelimitedReader.defaultBatchRows,
        bufferBytes: Int = DelimitedReader.defaultBufferBytes
    ) throws {
        let file = try FileHandle(forReadingFrom: URL(fileURLWithPath: path))
        try self.init(
            reading: { buffer in
                guard let chunk = try file.read(upToCount: buffer.count) else { return 0 }
                chunk.withUnsafeBytes { buffer.copyMemory(from: $0) }
                return chunk.count
            },
            dialect: dialect, plan: plan, batchRows: batchRows, bufferBytes: bufferBytes)
    }

    private init(
        dialect: Dialect, plan: [Column], batchRows: Int, input: UnsafeMutableRawBufferPointer,
        ownsInput: Bool, filled: Int, source: Source?, windowLimit: Int
    ) {
        precondition(batchRows > 0, "batchRows must be positive; got \(batchRows)")
        precondition(windowLimit > 0, "windowBytes must be positive; got \(windowLimit)")
        precondition(Self.layoutChecked)
        precondition(
            dialect.separator.isASCII,
            "Separator U+\(String(dialect.separator.value, radix: 16, uppercase: true)) is not a single ASCII byte")

        self.plan = plan
        self.batchRows = batchRows
        self.hasHeader = dialect.hasHeader
        self.source = source
        self.input = input
        self.ownsInput = ownsInput
        self.end = filled
        self.eof = source == nil
        self.windowLimit = min(windowLimit, Self.callLimit)

        state = .allocate(capacity: 1)
        var raw = hypertabular_dialect(
            separator: UInt8(dialect.separator.value), quoting: dialect.quoting ? 1 : 0,
            skip_blank_lines: dialect.skipBlankLines ? 1 : 0, engine: 0)
        precondition(
            hypertabular_delimited_init(state, &raw) == HYPERTABULAR_OK,
            "Separator '\(dialect.separator)' is not tab or printable ASCII other than '\"'")

        specs = .allocate(capacity: plan.count)
        buffers = .allocate(capacity: plan.count)
        var values = [UnsafeMutableRawPointer]()
        var verdicts = [UnsafeMutablePointer<CellVerdict>]()
        var widest = -1
        for (index, column) in plan.enumerated() {
            widest = max(widest, column.ordinal)
            // 16-aligned, the widest any value is, so every door's values sit as Swift
            // lays that type out and can be handed back as a buffer of it.
            let value = UnsafeMutableRawPointer.allocate(
                byteCount: batchRows * column.valueSize, alignment: 16)
            let verdict = UnsafeMutablePointer<CellVerdict>.allocate(capacity: batchRows)
            values.append(value)
            verdicts.append(verdict)
            (specs + index).initialize(to: column.spec)
            (buffers + index).initialize(
                to: hypertabular_column_buffer(
                    values: value,
                    verdicts: UnsafeMutableRawPointer(verdict).assumingMemoryBound(
                        to: hypertabular_cell_verdict.self)))
        }
        valueBase = values
        verdictBase = verdicts
        perRow = widest + 2
        cells = .allocate(capacity: perRow * batchRows)
        arena = .allocate(capacity: 4096)
        scratch = .allocate(capacity: 0)
    }

    deinit {
        state.deallocate()
        specs.deallocate()
        buffers.deallocate()
        for value in valueBase { value.deallocate() }
        for verdict in verdictBase { verdict.deallocate() }
        cells.deallocate()
        arena.deallocate()
        scratch.deallocate()
        if ownsInput { input.deallocate() }
    }

    // MARK: - the plan and the position

    /// The plan: the output columns, in output order.
    public var columns: [Column] { plan }

    /// Records finished so far — the header and skipped blank lines included.
    public var records: Int64 { Int64(clamping: state.pointee.records) }

    // MARK: - reading

    /// What the core is to read next, and whether nothing follows it.
    private func nextWindow() -> (base: UnsafePointer<UInt8>?, length: Int, last: Bool) {
        let available = end - start
        let length = min(available, windowLimit)
        let base = input.baseAddress.map {
            UnsafePointer(($0 + start).assumingMemoryBound(to: UInt8.self))
        }
        return (base, length, eof && length == available)
    }

    /// Gives the core more to look at behind the record it could not finish: for a stream,
    /// the unfinished record goes to the front of the buffer and more is read behind it;
    /// in memory, the window it is shown widens.
    private func refill() throws {
        guard let source else {
            guard windowLimit < Self.callLimit else { throw rowTooLong() }
            windowLimit = windowLimit > Self.callLimit / 2 ? Self.callLimit : windowLimit * 2
            return
        }
        let pending = end - start
        if start > 0 {
            if pending > 0, let base = input.baseAddress {
                base.copyMemory(from: base + start, byteCount: pending)
            }
            start = 0
            end = pending
        }
        if end == input.count {
            // One record fills the buffer: it needs a bigger one.
            guard input.count < Self.maxRowBytes else { throw rowTooLong() }
            let larger = UnsafeMutableRawBufferPointer.allocate(
                byteCount: min(input.count * 2, Self.maxRowBytes), alignment: 1)
            larger.copyMemory(from: UnsafeRawBufferPointer(rebasing: input[..<end]))
            input.deallocate()
            input = larger
        }
        let room = UnsafeMutableRawBufferPointer(rebasing: input[end...])
        let read = try source(room)
        precondition(
            read >= 0 && read <= room.count,
            "A source returns how many bytes it wrote; got \(read) for room for \(room.count)")
        if read == 0 {
            eof = true
        }
        end += read
    }

    private func rowTooLong() -> TabularError {
        let error = TabularError(
            kind: .rowTooLong, record: Int64(clamping: state.pointee.records),
            line: Int(clamping: state.pointee.line), byte: Int64(clamping: state.pointee.offset))
        failure = error
        return error
    }

    private func structural(_ raw: hypertabular_failure) -> TabularError {
        let error = TabularError(
            kind: raw.code == 2 ? .columnCount : .unclosedQuote,
            record: Int64(clamping: raw.record), line: Int(clamping: raw.line),
            byte: Int64(clamping: raw.byte), expected: Int(clamping: raw.expected),
            found: Int(clamping: raw.found))
        failure = error
        return error
    }

    private func growArena(toHold needed: UInt64) {
        let capacity = max(Int(clamping: needed), arena.count * 2)
        arena.deallocate()
        arena = .allocate(capacity: capacity)
    }

    /// The bytes a text value or a header name locates: in the input as it was shown to
    /// the core, or — flagged — in the arena, where the core unescaped them.
    private func located(_ span: hypertabular_span, in base: UnsafeRawPointer?) -> UnsafeRawBufferPointer {
        let from = span.len & HYPERTABULAR_SPAN_FLAG != 0 ? UnsafeRawPointer(arena.baseAddress) : base
        return UnsafeRawBufferPointer(
            start: from.map { $0 + Int(span.offset) },
            count: Int(span.len & ~HYPERTABULAR_SPAN_FLAG))
    }

    private func readHeaderIfDeclared() throws {
        guard hasHeader else { return }
        var names = UnsafeMutableBufferPointer<hypertabular_span>.allocate(capacity: 64)
        defer { names.deallocate() }
        var filled = hypertabular_filled()
        while true {
            let (base, length, last) = nextWindow()
            let code = hypertabular_delimited_header(
                state, base, UInt(length), last ? 1 : 0, names.baseAddress, UInt(names.count),
                arena.baseAddress, UInt(arena.count), &filled)
            switch code {
            case HYPERTABULAR_OK where filled.rows > 0:
                header = (0..<Int(filled.rows)).map { index in
                    String(decoding: located(names[index], in: UnsafeRawPointer(base)), as: UTF8.self)
                }
                start += Int(filled.consumed)
                return
            case HYPERTABULAR_OK:
                let consumed = Int(filled.consumed)
                start += consumed
                if last && (consumed == length || consumed == 0) {
                    // An empty input has no header and no rows; the width is unknown.
                    header = []
                    return
                }
                if consumed == 0 {
                    try refill()
                }
            case HYPERTABULAR_ERR_CELLS:
                names.deallocate()
                names = .allocate(capacity: Int(clamping: filled.needed))
            case HYPERTABULAR_ERR_ARENA:
                growArena(toHold: filled.needed)
            case HYPERTABULAR_ERR_STRUCTURE:
                throw structural(filled.failure)
            default:
                preconditionFailure(Self.contractViolation)
            }
        }
    }

    /// Reads the next batch. `true` with ``rows`` rows in hand; `false` once the input is
    /// exhausted.
    ///
    /// - Throws: ``TabularError`` when the input is structurally broken — after every
    ///   intact row before the break has been delivered, and again on every later call —
    ///   and whatever a streamed source throws.
    public func read() throws -> Bool {
        if let failure {
            throw failure
        }
        rows = 0
        var filled = hypertabular_filled()
        while true {
            let (base, length, last) = nextWindow()
            let code = hypertabular_delimited_fill(
                state, base, UInt(length), last ? 1 : 0, specs, buffers, UInt(plan.count),
                UInt(batchRows), cells.baseAddress, UInt(cells.count), arena.baseAddress,
                UInt(arena.count), &filled)
            switch code {
            case HYPERTABULAR_OK:
                let consumed = Int(filled.consumed)
                start += consumed
                if filled.rows > 0 {
                    rows = Int(filled.rows)
                    window = UnsafeRawPointer(base)
                    return true
                }
                if last && (consumed == length || consumed == 0) {
                    return false
                }
                if consumed == 0 {
                    try refill()
                }
            case HYPERTABULAR_ERR_CELLS:
                cells.deallocate()
                cells = .allocate(capacity: Int(clamping: filled.needed) * batchRows)
            case HYPERTABULAR_ERR_ARENA:
                growArena(toHold: filled.needed)
            case HYPERTABULAR_ERR_STRUCTURE:
                throw structural(filled.failure)
            default:
                preconditionFailure(Self.contractViolation)
            }
        }
    }

    // MARK: - a column at a time

    /// A column's verdicts for the batch in hand, one per row. Valid until the next
    /// ``read()``.
    public func verdicts(_ column: Int) -> UnsafeBufferPointer<CellVerdict> {
        UnsafeBufferPointer(start: verdictBase[column], count: rows)
    }

    /// A column's values for the batch in hand, one per row, exactly as the core wrote them
    /// — for the doors whose value is a primitive (``ColumnValue``): ``Door/bool`` as
    /// `Bool`, the integer and floating-point doors as their own type. The value of a row
    /// whose verdict is not ok is zero. Columns of other doors are read a cell at a time.
    /// Valid until the next ``read()``.
    ///
    /// - Precondition: `T` is the type the column's door writes.
    public func values<T: ColumnValue>(_ column: Int, as type: T.Type = T.self) -> UnsafeBufferPointer<T> {
        let door = plan[column].door
        precondition(
            door == T.columnDoor, "Column \(column) is cast through \(door); it has no buffer of \(T.self)")
        return UnsafeBufferPointer(
            start: valueBase[column].bindMemory(to: T.self, capacity: batchRows), count: rows)
    }

    // MARK: - a cell at a time

    /// The verdict of the cell at (`column`, `row`), checked against the doors the caller's
    /// accessor reads.
    private func checked(
        _ column: Int, _ row: Int, _ door: Door, _ also: Door? = nil, _ orElse: Door? = nil
    ) -> hypertabular_cell_verdict {
        precondition(row >= 0 && row < rows, "Row \(row) is outside the batch in hand (\(rows) rows)")
        let actual = plan[column].door
        precondition(
            actual == door || actual == also || actual == orElse,
            "Column \(column) is cast through \(actual), not \(door)")
        return verdictBase[column][row].raw
    }

    private func fault(_ verdict: hypertabular_cell_verdict) -> Fault {
        guard let reason = CastFailure(rawValue: Int32(truncatingIfNeeded: verdict.reason)) else {
            preconditionFailure("libhypertabular wrote unknown verdict code \(verdict.reason)")
        }
        return Fault(reason: reason, offset: Int(verdict.offset), length: Int(verdict.len))
    }

    /// One cell as HyperCast's union: the value the core wrote, presented, or the fault.
    private func cell<Raw, T>(
        _ column: Int, _ row: Int, _ door: Door, _ also: Door? = nil, _ orElse: Door? = nil,
        present: (Raw) -> T
    ) -> Verdict<T> {
        let verdict = checked(column, row, door, also, orElse)
        guard verdict.reason == 0 else {
            return .fault(fault(verdict))
        }
        return .success(
            present(valueBase[column].load(fromByteOffset: row * MemoryLayout<Raw>.stride, as: Raw.self)))
    }

    /// The verdict of the cell at (`column`, `row`), whatever its door.
    public func verdict(_ column: Int, row: Int) -> CellVerdict {
        precondition(row >= 0 && row < rows, "Row \(row) is outside the batch in hand (\(rows) rows)")
        return verdictBase[column][row]
    }

    /// The cell of a ``Door/bool`` column.
    public func bool(_ column: Int, row: Int) -> Verdict<Bool> {
        cell(column, row, .bool) { (raw: UInt8) in raw != 0 }
    }

    /// The cell of a ``Door/i8`` column.
    public func i8(_ column: Int, row: Int) -> Verdict<Int8> {
        cell(column, row, .i8) { (raw: Int8) in raw }
    }

    /// The cell of a ``Door/i16`` column.
    public func i16(_ column: Int, row: Int) -> Verdict<Int16> {
        cell(column, row, .i16) { (raw: Int16) in raw }
    }

    /// The cell of a ``Door/i32`` column.
    public func i32(_ column: Int, row: Int) -> Verdict<Int32> {
        cell(column, row, .i32) { (raw: Int32) in raw }
    }

    /// The cell of a ``Door/i64`` column.
    public func i64(_ column: Int, row: Int) -> Verdict<Int64> {
        cell(column, row, .i64) { (raw: Int64) in raw }
    }

    /// The cell of a ``Door/u8`` column.
    public func u8(_ column: Int, row: Int) -> Verdict<UInt8> {
        cell(column, row, .u8) { (raw: UInt8) in raw }
    }

    /// The cell of a ``Door/u16`` column.
    public func u16(_ column: Int, row: Int) -> Verdict<UInt16> {
        cell(column, row, .u16) { (raw: UInt16) in raw }
    }

    /// The cell of a ``Door/u32`` column.
    public func u32(_ column: Int, row: Int) -> Verdict<UInt32> {
        cell(column, row, .u32) { (raw: UInt32) in raw }
    }

    /// The cell of a ``Door/u64`` column.
    public func u64(_ column: Int, row: Int) -> Verdict<UInt64> {
        cell(column, row, .u64) { (raw: UInt64) in raw }
    }

    /// The cell of a ``Door/f32`` column.
    public func f32(_ column: Int, row: Int) -> Verdict<Float> {
        cell(column, row, .f32) { (raw: Float) in raw }
    }

    /// The cell of a ``Door/f64`` column.
    public func f64(_ column: Int, row: Int) -> Verdict<Double> {
        cell(column, row, .f64) { (raw: Double) in raw }
    }

    /// The cell of a ``Door/decimal`` column, exact: Foundation's `Decimal`, as HyperCast's
    /// own `Cast.decimal` presents it. The core's magnitude is at most 2⁹⁶ − 1 (29 digits),
    /// inside `Decimal`'s 38-digit mantissa, so nothing is rounded.
    public func decimal(_ column: Int, row: Int) -> Verdict<Decimal> {
        cell(column, row, .decimal) { (raw: hypertabular_decimal) in
            let magnitude =
                raw.hi == 0 ? Decimal(raw.lo) : Decimal(raw.hi) * Self.highWordWeight + Decimal(raw.lo)
            return Decimal(
                sign: raw.negative != 0 ? .minus : .plus, exponent: -Int(raw.scale),
                significand: magnitude)
        }
    }

    /// The cell of a ``Door/uuid`` column.
    public func uuid(_ column: Int, row: Int) -> Verdict<UUID> {
        // uuid_t's tuple layout is the RFC byte order exactly, which is what the core writes.
        cell(column, row, .uuid) { (raw: uuid_t) in UUID(uuid: raw) }
    }

    /// The cell of a ``Door/timestamp``, ``Door/unix`` or ``Door/excelSerial`` column: an
    /// instant, as Foundation's `Date` — a `Double` of seconds, so sub-microsecond fidelity
    /// degrades toward the window's edges, exactly as HyperCast's own doors present it.
    public func timestamp(_ column: Int, row: Int) -> Verdict<Date> {
        cell(column, row, .timestamp, .unix, .excelSerial) { (raw: hypertabular_timestamp) in
            Date(timeIntervalSince1970: Double(raw.seconds) + Double(raw.nanos) / 1_000_000_000)
        }
    }

    /// The cell of a ``Door/date`` or ``Door/dateOrdered`` column: year, month and day,
    /// digit-perfect, with no calendar or zone attached.
    public func date(_ column: Int, row: Int) -> Verdict<DateComponents> {
        cell(column, row, .date, .dateOrdered) { (raw: hypertabular_date) in
            DateComponents(year: Int(raw.year), month: Int(raw.month), day: Int(raw.day))
        }
    }

    /// The cell of a ``Door/dateTime`` column: year through nanosecond with no zone — the
    /// text named none and none is invented.
    public func dateTime(_ column: Int, row: Int) -> Verdict<DateComponents> {
        cell(column, row, .dateTime) { (raw: hypertabular_civil) in
            let secondOfDay = raw.nanos_of_day / 1_000_000_000
            return DateComponents(
                year: Int(raw.date.year), month: Int(raw.date.month), day: Int(raw.date.day),
                hour: Int(secondOfDay / 3_600),
                minute: Int(secondOfDay % 3_600 / 60),
                second: Int(secondOfDay % 60),
                nanosecond: Int(raw.nanos_of_day % 1_000_000_000))
        }
    }

    /// The cell of a ``Door/time`` column: hour, minute, second and nanosecond.
    public func time(_ column: Int, row: Int) -> Verdict<DateComponents> {
        cell(column, row, .time) { (nanosOfDay: UInt64) in
            let (secondOfDay, nano) = nanosOfDay.quotientAndRemainder(dividingBy: 1_000_000_000)
            let (hour, rest) = secondOfDay.quotientAndRemainder(dividingBy: 3_600)
            let (minute, second) = rest.quotientAndRemainder(dividingBy: 60)
            return DateComponents(
                hour: Int(hour), minute: Int(minute), second: Int(second), nanosecond: Int(nano))
        }
    }

    /// The cell of a ``Door/duration`` column, as Swift's `Duration` — the core's
    /// nanoseconds carried exactly, both signs.
    public func duration(_ column: Int, row: Int) -> Verdict<Duration> {
        cell(column, row, .duration) { (raw: hypertabular_duration) in
            Duration.seconds(raw.seconds) + .nanoseconds(Int64(raw.nanos))
        }
    }

    /// The cell of a ``Door/text`` column: its bytes, untrimmed, quotes resolved — or `nil`
    /// for a cell with no bytes at all, which is the one way text fails. The buffer points
    /// into the reader's input — zero-copy for every cell that had no escaped quote in it —
    /// and is valid until the next ``read()``.
    public func text(_ column: Int, row: Int) -> UnsafeRawBufferPointer? {
        guard checked(column, row, .text).reason == 0 else {
            return nil
        }
        let span = valueBase[column].load(
            fromByteOffset: row * MemoryLayout<hypertabular_span>.stride, as: hypertabular_span.self)
        return located(span, in: window)
    }

    /// The cell of a ``Door/text`` column as a `String`, or `nil` for an empty cell.
    public func string(_ column: Int, row: Int) -> String? {
        text(column, row: row).map { String(decoding: $0, as: UTF8.self) }
    }

    /// The text the cell at (`column`, `row`) was cast from, whatever its door and whatever
    /// its verdict — what a fault's span indexes, and what to show for a value that did not
    /// cast. Valid until the next call to ``raw(_:row:)`` or ``read()``.
    public func raw(_ column: Int, row: Int) -> UnsafeRawBufferPointer {
        precondition(row >= 0 && row < rows, "Row \(row) is outside the batch in hand (\(rows) rows)")
        let cell = cells[row * perRow + plan[column].ordinal]
        let length = Int(cell.len & ~HYPERTABULAR_SPAN_FLAG)
        let written = UnsafeRawBufferPointer(start: window.map { $0 + Int(cell.offset) }, count: length)
        guard cell.len & HYPERTABULAR_SPAN_FLAG != 0 else {
            return written
        }
        // A cell with an escaped quote in it: unescaped, as the core cast it.
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
