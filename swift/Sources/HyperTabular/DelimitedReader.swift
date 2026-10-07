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
/// A value that does not cast is that cell's verdict, and the read goes on. Input that is
/// not rows of cells at all — a record of the wrong width, a quote never closed — is a
/// ``TabularError``, thrown after every intact row before it has been delivered.
///
/// A caller bug — a column read through an accessor that is not its door's, a row outside
/// the batch, a separator the core cannot honour — is a precondition failure, never an
/// error to catch: the same line HyperCast's Swift binding draws.
///
/// The ``Batch`` a read returns, and every buffer it hands out, point into the reader's own
/// memory and are valid until the next ``read()`` and no longer than the reader. Not
/// thread-safe.
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

    /// The dialect the text is read in.
    public let dialect: Dialect
    private let columnSet: Columns
    private let batch: Batch
    /// Cell-table entries one row takes: the widest ordinal the plan reads, plus two, or
    /// more if the core asked for more.
    private var perRow: Int

    // What the core is handed beside the plan's buffers, allocated once and moved only to
    // grow: it is given addresses.
    private let state: UnsafeMutablePointer<hypertabular_delimited_state>
    private var cells: UnsafeMutableBufferPointer<hypertabular_span>
    private var arena: UnsafeMutableBufferPointer<UInt8>
    /// Whether the last batch ended early because the arena filled: the next one starts with
    /// it doubled, so that escaped text costs a few batches, not one per row.
    private var cramped = false

    // The source: a stream read into `input`, or memory read in place.
    private let source: Source?
    private var input: UnsafeMutableRawBufferPointer
    private let ownsInput: Bool
    private var start = 0
    private var end: Int
    private var eof: Bool
    /// The most of `input` the core is shown at once.
    private var windowLimit: Int

    private var failure: TabularError?

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

        self.dialect = dialect
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

        columnSet = Columns(plan: plan, batchRows: batchRows)
        batch = Batch(columnSet, workbook: false)
        perRow = columnSet.width + 1
        cells = .allocate(capacity: perRow * batchRows)
        arena = .allocate(capacity: 4096)
    }

    deinit {
        state.deallocate()
        cells.deallocate()
        arena.deallocate()
        if ownsInput { input.deallocate() }
    }

    // MARK: - the plan and the position

    /// The plan the text is read through: column `i` of every batch is `plan[i]`.
    public var plan: [Column] { columnSet.plan }

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
        let error = TabularError(raw)
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
        guard dialect.hasHeader else { return }
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

    /// Reads the next batch: up to `batchRows` whole rows, as many as one call of the core
    /// found — fewer when the input, or what the read buffer holds of it, ran out first — or
    /// `nil` once the input is exhausted. The batch is valid until the next read.
    ///
    /// - Throws: ``TabularError`` when the input is structurally broken — after every
    ///   intact row before the break has been delivered, and again on every later call —
    ///   and whatever a streamed source throws.
    public func read() throws -> Batch? {
        batch.clear()
        if let failure {
            throw failure
        }
        if cramped {
            arena.deallocate()
            arena = .allocate(capacity: arena.count * 2)
            cramped = false
        }
        var filled = hypertabular_filled()
        while true {
            let (base, length, last) = nextWindow()
            // The cells and the arena travel in a buffers block, the workbook calls' own, of
            // which the fill reads those two alone: Mono's interpreter passes no more than
            // twelve integer arguments to a native function, and the C ABI is the same for
            // every binding.
            var buffers = hypertabular_buffers()
            buffers.cells = cells.baseAddress
            buffers.cells_cap = UInt(cells.count)
            buffers.arena = arena.baseAddress
            buffers.arena_cap = UInt(arena.count)
            let code = hypertabular_delimited_fill(
                state, base, UInt(length), last ? 1 : 0, columnSet.specs, columnSet.buffers,
                UInt(columnSet.plan.count), UInt(columnSet.batchRows), &buffers, &filled)
            switch code {
            case HYPERTABULAR_OK:
                let consumed = Int(filled.consumed)
                start += consumed
                if filled.rows > 0 {
                    let rows = Int(filled.rows)
                    cramped = rows < columnSet.batchRows && filled.arena_used * 2 >= UInt64(arena.count)
                    return batch.fill(
                        rows: rows, cells: cells, perRow: perRow, base: UnsafeRawPointer(base), arena: arena)
                }
                if last && (consumed == length || consumed == 0) {
                    return nil
                }
                if consumed == 0 {
                    try refill()
                }
            case HYPERTABULAR_ERR_CELLS:
                perRow = max(perRow, Int(clamping: filled.needed))
                cells.deallocate()
                cells = .allocate(capacity: perRow * columnSet.batchRows)
            case HYPERTABULAR_ERR_ARENA:
                growArena(toHold: filled.needed)
            case HYPERTABULAR_ERR_STRUCTURE:
                throw structural(filled.failure)
            default:
                preconditionFailure(Self.contractViolation)
            }
        }
    }
}
