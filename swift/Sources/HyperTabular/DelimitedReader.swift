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
/// A reader opened without a plan reads the header first, so that the plan can be built
/// from the names it declares, and is then bound to it — once, before the first read:
///
/// ```swift
/// let reader = try DelimitedReader(contentsOfFile: "orders.csv", dialect: .csv)
/// let header = reader.header!
/// try reader.bind([.i32(header.ordinal(of: "id")), .text(header.ordinal(of: "name")), .f64(header.ordinal(of: "score"))])
/// while let batch = try reader.read() {
///     for row in batch {
///         switch row.get(2, as: Double.self) {
///         case .success(let score): print(row.string(1) ?? "", score)
///         case .fault(let fault):
///             print("line \(row.line): \(fault.reason) in", String(decoding: row.raw(2), as: UTF8.self))
///         }
///     }
/// }
/// ```
///
/// A reader whose columns are known by position is opened with its plan instead —
/// `DelimitedReader(contentsOfFile: "orders.csv", dialect: .csv, plan: [.i32(0), .text(1), .f64(2)])`
/// — and reads the same way.
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
    /// Rows per batch: what the column buffers are sized by when a plan is bound.
    private let batchRows: Int
    // The plan and the batch it is read into, once one is bound.
    private var columnSet: Columns?
    private var batch: Batch?
    /// Cell-table entries one row takes: the widest ordinal the plan reads, plus two, or
    /// more if the core asked for more.
    private var perRow = 0

    // What the core is handed beside the plan's buffers, allocated once and moved only to
    // grow: it is given addresses.
    private let state: UnsafeMutablePointer<hypertabular_delimited_state>
    private var cells: UnsafeMutableBufferPointer<hypertabular_span>
    private var arena: UnsafeMutableBufferPointer<UInt8>
    /// Whether the last batch ended early because the arena filled: the next one starts with
    /// it doubled, so that escaped text costs a few batches, not one per row.
    private var cramped = false

    // The source: a stream read into `input` — through a closure, or a byte at a time from
    // an asynchronous sequence — or memory read in place.
    private let source: Source?
    private let nextByte: (() async throws -> UInt8?)?
    private var input: UnsafeMutableRawBufferPointer
    private let ownsInput: Bool
    private var start = 0
    private var end: Int
    private var eof: Bool
    /// The most of `input` the core is shown at once.
    private var windowLimit: Int

    private var failure: TabularError?

    /// The header's names, when the dialect declares one — empty for an input with no
    /// record — and `nil` when it declares none. Read when the reader is opened, before any
    /// plan, so that a plan can be built from it: see `Header.ordinal(of:)`.
    public private(set) var header: Header?

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
            bytes: bytes, dialect: dialect, plan: plan, batchRows: batchRows, windowBytes: Self.callLimit)
    }

    /// ``init(bytes:dialect:plan:batchRows:)`` with the plan bound later: the header is read
    /// here, and the plan built from it is bound with ``bind(_:)`` before the first read.
    ///
    /// - Throws: ``TabularError`` when the header record is structurally broken.
    public convenience init<Bytes: Collection>(
        bytes: Bytes, dialect: Dialect, batchRows: Int = DelimitedReader.defaultBatchRows
    ) throws where Bytes.Element == UInt8 {
        try self.init(bytes: bytes, dialect: dialect, plan: nil, batchRows: batchRows, windowBytes: Self.callLimit)
    }

    /// ``init(bytes:dialect:plan:batchRows:)``, plan or none, showing the core `windowBytes`
    /// of the text at a time to begin with — what a text over 2 GiB gets, made reachable for
    /// the tests.
    convenience init<Bytes: Collection>(
        bytes: Bytes, dialect: Dialect, plan: [Column]?, batchRows: Int, windowBytes: Int
    ) throws where Bytes.Element == UInt8 {
        let copy = UnsafeMutableRawBufferPointer.allocate(byteCount: bytes.count, alignment: 1)
        copy.copyBytes(from: bytes)
        self.init(
            dialect: dialect, batchRows: batchRows, input: copy, ownsInput: true, filled: copy.count,
            source: nil, nextByte: nil, windowLimit: windowBytes)
        try open(plan)
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
        try self.init(bytesNoCopy: bytes, dialect: dialect, plan: Optional(plan), batchRows: batchRows)
    }

    /// ``init(bytesNoCopy:dialect:plan:batchRows:)`` with the plan bound later: the header
    /// is read here, and the plan built from it is bound with ``bind(_:)`` before the first
    /// read.
    ///
    /// - Throws: ``TabularError`` when the header record is structurally broken.
    public convenience init(
        bytesNoCopy bytes: UnsafeRawBufferPointer, dialect: Dialect,
        batchRows: Int = DelimitedReader.defaultBatchRows
    ) throws {
        try self.init(bytesNoCopy: bytes, dialect: dialect, plan: nil, batchRows: batchRows)
    }

    private convenience init(
        bytesNoCopy bytes: UnsafeRawBufferPointer, dialect: Dialect, plan: [Column]?, batchRows: Int
    ) throws {
        // Never written through: in memory the core only reads the input.
        self.init(
            dialect: dialect, batchRows: batchRows, input: UnsafeMutableRawBufferPointer(mutating: bytes),
            ownsInput: false, filled: bytes.count, source: nil, nextByte: nil, windowLimit: Self.callLimit)
        try open(plan)
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
        try self.init(
            reading: source, dialect: dialect, plan: Optional(plan), batchRows: batchRows, bufferBytes: bufferBytes)
    }

    /// ``init(reading:dialect:plan:batchRows:bufferBytes:)`` with the plan bound later: the
    /// header is read here, and the plan built from it is bound with ``bind(_:)`` before the
    /// first read.
    ///
    /// - Throws: ``TabularError`` when the header record is structurally broken, and
    ///   whatever `source` throws.
    public convenience init(
        reading source: @escaping Source, dialect: Dialect,
        batchRows: Int = DelimitedReader.defaultBatchRows,
        bufferBytes: Int = DelimitedReader.defaultBufferBytes
    ) throws {
        try self.init(reading: source, dialect: dialect, plan: nil, batchRows: batchRows, bufferBytes: bufferBytes)
    }

    private convenience init(
        reading source: @escaping Source, dialect: Dialect, plan: [Column]?, batchRows: Int, bufferBytes: Int
    ) throws {
        self.init(
            dialect: dialect, batchRows: batchRows, input: Self.streamBuffer(bufferBytes), ownsInput: true,
            filled: 0, source: source, nextByte: nil, windowLimit: Self.callLimit)
        try open(plan)
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
        try self.init(
            reading: try Self.file(path), dialect: dialect, plan: Optional(plan), batchRows: batchRows,
            bufferBytes: bufferBytes)
    }

    /// ``init(contentsOfFile:dialect:plan:batchRows:bufferBytes:)`` with the plan bound
    /// later: the header is read here, and the plan built from it is bound with ``bind(_:)``
    /// before the first read.
    ///
    /// - Throws: Foundation's error when the file cannot be opened or read, and
    ///   ``TabularError`` when the header record is structurally broken.
    public convenience init(
        contentsOfFile path: String, dialect: Dialect,
        batchRows: Int = DelimitedReader.defaultBatchRows,
        bufferBytes: Int = DelimitedReader.defaultBufferBytes
    ) throws {
        try self.init(
            reading: try Self.file(path), dialect: dialect, plan: nil, batchRows: batchRows, bufferBytes: bufferBytes)
    }

    /// Reads UTF-8 delimited text from an asynchronous sequence of bytes — `URL.resourceBytes`,
    /// `FileHandle.bytes`, a network body — into the reader's own buffer as the bytes arrive.
    /// The header is awaited here; the plan built from it is bound with ``bind(_:)``, and
    /// every batch is read with ``readAsync()``.
    ///
    /// - Parameters:
    ///   - bytes: The text, as it arrives.
    ///   - dialect: The declared dialect.
    ///   - batchRows: Rows per batch.
    ///   - bufferBytes: The initial read buffer; it doubles when a record does not fit.
    /// - Throws: `CancellationError` when the task is cancelled while the header is awaited,
    ///   ``TabularError`` when the header record is structurally broken, and whatever the
    ///   sequence throws.
    public convenience init<Bytes: AsyncSequence>(
        bytes: Bytes, dialect: Dialect, batchRows: Int = DelimitedReader.defaultBatchRows,
        bufferBytes: Int = DelimitedReader.defaultBufferBytes
    ) async throws where Bytes.Element == UInt8 {
        try await self.init(bytes: bytes, dialect: dialect, plan: nil, batchRows: batchRows, bufferBytes: bufferBytes)
    }

    /// Reads UTF-8 delimited text from an asynchronous sequence of bytes, as the plan-less
    /// initializer of the same shape does, through `plan`.
    ///
    /// - Throws: `CancellationError` when the task is cancelled while the header is awaited,
    ///   ``TabularError`` when the header record is structurally broken, and whatever the
    ///   sequence throws.
    public convenience init<Bytes: AsyncSequence>(
        bytes: Bytes, dialect: Dialect, plan: [Column], batchRows: Int = DelimitedReader.defaultBatchRows,
        bufferBytes: Int = DelimitedReader.defaultBufferBytes
    ) async throws where Bytes.Element == UInt8 {
        try await self.init(
            bytes: bytes, dialect: dialect, plan: Optional(plan), batchRows: batchRows, bufferBytes: bufferBytes)
    }

    private convenience init<Bytes: AsyncSequence>(
        bytes: Bytes, dialect: Dialect, plan: [Column]?, batchRows: Int, bufferBytes: Int
    ) async throws where Bytes.Element == UInt8 {
        var iterator = bytes.makeAsyncIterator()
        self.init(
            dialect: dialect, batchRows: batchRows, input: Self.streamBuffer(bufferBytes), ownsInput: true,
            filled: 0, source: nil, nextByte: { try await iterator.next() }, windowLimit: Self.callLimit)
        if let plan {
            try bind(plan)
        }
        guard dialect.hasHeader else { return }
        var names = UnsafeMutableBufferPointer<hypertabular_span>.allocate(capacity: 64)
        defer { names.deallocate() }
        while try !headerStep(&names) {
            try await refillAsync()
        }
    }

    private init(
        dialect: Dialect, batchRows: Int, input: UnsafeMutableRawBufferPointer, ownsInput: Bool, filled: Int,
        source: Source?, nextByte: (() async throws -> UInt8?)?, windowLimit: Int
    ) {
        precondition(batchRows > 0, "batchRows must be positive; got \(batchRows)")
        precondition(windowLimit > 0, "windowBytes must be positive; got \(windowLimit)")
        precondition(Self.layoutChecked)
        precondition(
            dialect.separator.isASCII,
            "Separator U+\(String(dialect.separator.value, radix: 16, uppercase: true)) is not a single ASCII byte")

        self.dialect = dialect
        self.batchRows = batchRows
        self.source = source
        self.nextByte = nextByte
        self.input = input
        self.ownsInput = ownsInput
        self.end = filled
        self.eof = source == nil && nextByte == nil
        self.windowLimit = min(windowLimit, Self.callLimit)

        state = .allocate(capacity: 1)
        var raw = hypertabular_dialect(
            separator: UInt8(dialect.separator.value), quoting: dialect.quoting ? 1 : 0,
            skip_blank_lines: dialect.skipBlankLines ? 1 : 0, engine: 0)
        precondition(
            hypertabular_delimited_init(state, &raw) == HYPERTABULAR_OK,
            "Separator '\(dialect.separator)' is not tab or printable ASCII other than '\"'")

        cells = .allocate(capacity: 0)
        arena = .allocate(capacity: 4096)
    }

    deinit {
        state.deallocate()
        cells.deallocate()
        arena.deallocate()
        if ownsInput { input.deallocate() }
    }

    /// The read buffer a streamed source starts with.
    private static func streamBuffer(_ bufferBytes: Int) -> UnsafeMutableRawBufferPointer {
        precondition(bufferBytes > 0, "bufferBytes must be positive; got \(bufferBytes)")
        return .allocate(byteCount: min(bufferBytes, Self.maxRowBytes), alignment: 1)
    }

    /// A source that reads the file at `path`, which it keeps open for as long as it is held.
    private static func file(_ path: String) throws -> Source {
        let file = try FileHandle(forReadingFrom: URL(fileURLWithPath: path))
        return { buffer in
            guard let chunk = try file.read(upToCount: buffer.count) else { return 0 }
            chunk.withUnsafeBytes { buffer.copyMemory(from: $0) }
            return chunk.count
        }
    }

    /// What every synchronous opening does once the reader exists: binds the plan, when it
    /// was given one — before the header, as ``bind(_:)`` would after it — and reads the
    /// header the dialect declares.
    private func open(_ plan: [Column]?) throws {
        if let plan {
            try bind(plan)
        }
        guard dialect.hasHeader else { return }
        var names = UnsafeMutableBufferPointer<hypertabular_span>.allocate(capacity: 64)
        defer { names.deallocate() }
        while try !headerStep(&names) {
            try refill()
        }
    }

    // MARK: - the plan and the position

    /// Declares the plan a reader opened without one reads through — once, before the first
    /// read. Every column buffer and the table that locates each cell are sized here, by the
    /// plan and the reader's batch size.
    ///
    /// - Throws: ``PlanError/alreadyBound`` when the reader has a plan, whether it was opened
    ///   with one or bound one already; the reader goes on with the plan it has.
    public func bind(_ plan: [Column]) throws {
        guard columnSet == nil else {
            throw PlanError.alreadyBound
        }
        let columns = Columns(plan: plan, batchRows: batchRows)
        perRow = columns.width + 1
        cells.deallocate()
        cells = .allocate(capacity: perRow * batchRows)
        columnSet = columns
        batch = Batch(columns, workbook: false)
    }

    /// Whether a plan has been bound: always, for a reader opened with one.
    public var isBound: Bool { columnSet != nil }

    /// The plan the text is read through: column `i` of every batch is `plan[i]`. Empty until
    /// one is bound.
    public var plan: [Column] { columnSet?.plan ?? [] }

    /// How many cells a record has: the header's count once it has been read, otherwise the
    /// first record's once it has been — `nil` before either.
    public var columnCount: Int? {
        let expected = Int(state.pointee.expected)
        return expected == 0 ? nil : expected
    }

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

    /// Readies the read buffer for more of a stream: the unfinished record goes to the front
    /// of it, and it doubles when that record fills it. Returns the room behind the record —
    /// or, for memory, widens the window the core is shown and returns `nil`.
    private func room() throws -> UnsafeMutableRawBufferPointer? {
        guard source != nil || nextByte != nil else {
            guard windowLimit < Self.callLimit else { throw rowTooLong() }
            windowLimit = windowLimit > Self.callLimit / 2 ? Self.callLimit : windowLimit * 2
            return nil
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
        return UnsafeMutableRawBufferPointer(rebasing: input[end...])
    }

    /// Gives the core more to look at behind the record it could not finish: for a stream,
    /// more is read behind it; in memory, the window it is shown widens.
    private func refill() throws {
        precondition(
            nextByte == nil, "A reader of an asynchronous sequence is read with readAsync(), not read()")
        guard let room = try room(), let source else { return }
        let read = try source(room)
        precondition(
            read >= 0 && read <= room.count,
            "A source returns how many bytes it wrote; got \(read) for room for \(room.count)")
        if read == 0 {
            eof = true
        }
        end += read
    }

    /// ``refill()`` for any reader, awaiting an asynchronous sequence's bytes until the room
    /// is full or the sequence ends. Each byte counts as soon as it lands, so a cancellation
    /// or a failure of the sequence part-way loses nothing, and the next read goes on from it.
    private func refillAsync() async throws {
        try Task.checkCancellation()
        guard let nextByte else {
            try refill()
            return
        }
        guard let room = try room() else { return }
        let limit = end + room.count
        while end < limit {
            guard let byte = try await nextByte() else {
                eof = true
                return
            }
            input[end] = byte
            end += 1
        }
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

    /// Reads the header from what is buffered: `true` once it has been read, `false` when
    /// the core needs more input first. `names` grows when the core asks.
    private func headerStep(_ names: inout UnsafeMutableBufferPointer<hypertabular_span>) throws -> Bool {
        var filled = hypertabular_filled()
        while true {
            let (base, length, last) = nextWindow()
            let code = hypertabular_delimited_header(
                state, base, UInt(length), last ? 1 : 0, names.baseAddress, UInt(names.count),
                arena.baseAddress, UInt(arena.count), &filled)
            switch code {
            case HYPERTABULAR_OK where filled.rows > 0:
                header = Header(
                    bytes: (0..<Int(filled.rows)).map { index in
                        Array(located(names[index], in: UnsafeRawPointer(base)))
                    })
                start += Int(filled.consumed)
                return true
            case HYPERTABULAR_OK:
                let consumed = Int(filled.consumed)
                start += consumed
                if last && (consumed == length || consumed == 0) {
                    // An empty input has no header and no rows; the width is unknown.
                    header = []
                    return true
                }
                if consumed == 0 {
                    return false
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

    /// What a fill from the buffered input came to.
    private enum Step {
        /// A batch of this many rows, located in the input from `base`.
        case rows(Int, base: UnsafeRawPointer?)
        /// The input is over.
        case end
        /// The core needs more input to finish a row.
        case input
    }

    /// What every read does first: ends the batch in hand, and says what there is to read
    /// through — a failure is thrown again, and a reader with no plan cannot be read yet.
    private func begin() throws -> (Columns, Batch) {
        batch?.clear()
        if let failure {
            throw failure
        }
        guard let columnSet, let batch else {
            throw PlanError.unbound
        }
        if cramped {
            arena.deallocate()
            arena = .allocate(capacity: arena.count * 2)
            cramped = false
        }
        return (columnSet, batch)
    }

    /// Fills a batch from what is buffered.
    private func fillStep(_ columnSet: Columns) throws -> Step {
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
                    return .rows(rows, base: UnsafeRawPointer(base))
                }
                if last && (consumed == length || consumed == 0) {
                    return .end
                }
                if consumed == 0 {
                    return .input
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

    /// Reads the next batch: up to `batchRows` whole rows, as many as one call of the core
    /// found — fewer when the input, or what the read buffer holds of it, ran out first — or
    /// `nil` once the input is exhausted. The batch is valid until the next read.
    ///
    /// A reader opened from an asynchronous sequence is read with ``readAsync()`` instead;
    /// reading one here is a precondition failure.
    ///
    /// - Throws: ``TabularError`` when the input is structurally broken — after every
    ///   intact row before the break has been delivered, and again on every later call —
    ///   ``PlanError/unbound`` while the reader has no plan, and whatever a streamed source
    ///   throws.
    public func read() throws -> Batch? {
        let (columnSet, batch) = try begin()
        while true {
            switch try fillStep(columnSet) {
            case .rows(let rows, let base):
                return batch.fill(rows: rows, cells: cells, perRow: perRow, base: base, arena: arena)
            case .end:
                return nil
            case .input:
                try refill()
            }
        }
    }

    /// ``read()``, awaiting the source whenever the core needs more of it: how a reader
    /// opened from an asynchronous sequence is read, and the same as ``read()`` for any other
    /// — a reader of memory never suspends.
    ///
    /// Cancellation is checked before the read begins and before each wait for more input,
    /// and is thrown as `CancellationError`. A cancelled read loses nothing: every byte that
    /// had arrived stays buffered, and the next read goes on from there.
    ///
    /// - Throws: `CancellationError` when the task is cancelled, and whatever ``read()``
    ///   throws.
    public func readAsync() async throws -> Batch? {
        try Task.checkCancellation()
        let (columnSet, batch) = try begin()
        while true {
            switch try fillStep(columnSet) {
            case .rows(let rows, let base):
                return batch.fill(rows: rows, cells: cells, perRow: perRow, base: base, arena: arena)
            case .end:
                return nil
            case .input:
                try await refillAsync()
            }
        }
    }

    /// Reads every row left, batch by batch, handing each to `body` — the row view of a
    /// reader, whose batches are each over when the next is read: a ``Row`` is valid only
    /// while `body` has it. Stops at the first error, `body`'s or the reader's.
    ///
    /// - Throws: Whatever ``read()`` or `body` throws.
    public func forEachRow(_ body: (Row) throws -> Void) throws {
        while let batch = try read() {
            for row in batch {
                try body(row)
            }
        }
    }
}
