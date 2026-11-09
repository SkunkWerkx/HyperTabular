import Foundation
import HyperTabularCore

// read(2) and errno, for a file source (Foundation brings Darwin's; elsewhere the C library
// is named).
#if canImport(Android)
    import Android
#elseif canImport(Glibc)
    import Glibc
#elseif canImport(Musl)
    import Musl
#elseif canImport(WASILibc)
    import WASILibc
#endif

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
/// not rows of cells at all — a record of the wrong width, a quote never closed, bytes with no
/// character in a declared ``TextEncoding`` — is a ``TabularError``, thrown after every intact row
/// before it has been delivered.
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
    ///
    /// It runs in the middle of a read, and a ``read()``, ``bind(_:)`` or ``forEachRow(_:)`` it
    /// calls on the same reader throws ``PlanError/busy``; the read it is in goes on unharmed.
    public typealias Source = (UnsafeMutableRawBufferPointer) throws -> Int

    private static let contractViolation =
        "libhypertabular reported a contract violation — a binding bug, please report it"

    /// For the tests: the size the read buffer stops growing at, in place of ``maxRowBytes``, so a
    /// row past the ceiling takes no gigabyte to prove. `0` is the real one.
    nonisolated(unsafe) static var rowCeiling = 0

    private static var ceiling: Int { rowCeiling > 0 ? rowCeiling : maxRowBytes }

    /// The most input one native call may be shown: the core's spans are 31 bits.
    private static let callLimit = Int(Int32.max)

    // What this file assumes of the shapes hypertabular.h declares, held once: the state
    // block is the size the core says it is, and each value is as wide as the core writes it.
    private static let layoutChecked: Bool = {
        precondition(
            MemoryLayout<HyperTabularDelimitedState>.size == Int(hypertabular_delimited_state_size()),
            "hypertabular.h and the linked libhypertabular disagree on the state block")
        precondition(
            MemoryLayout<HyperTabularCellVerdict>.stride == HyperTabularCellVerdict_SIZE
                && MemoryLayout<CellVerdict>.stride == HyperTabularCellVerdict_SIZE
                && MemoryLayout<HyperTabularSpan>.stride == HyperTabularSpan_SIZE
                && MemoryLayout<HyperTabularColumnSpec>.stride == HyperTabularColumnSpec_SIZE
                && MemoryLayout<HyperTabularRawDelimitedOptions>.size == HyperTabularRawDelimitedOptions_SIZE
                && MemoryLayout<HyperTabularRawSheetOptions>.size == HyperTabularRawSheetOptions_SIZE
                && MemoryLayout<HyperTabularCellNote>.stride == 4
                && MemoryLayout<HyperTabularRawProblem>.stride == 32
                && MemoryLayout<HyperTabularRawSuggestion>.stride == 20
                && MemoryLayout<HyperTabularRawTranscoded>.size == 32
                && MemoryLayout<HyperTabularRawRange>.size == 16
                && MemoryLayout<HyperTabularRawNumFormat>.size == HyperTabularRawNumFormat_SIZE
                && MemoryLayout<HyperTabularDelimitedState>.size == HyperTabularDelimitedState_SIZE
                && MemoryLayout<HyperTabularFilled>.size == HyperTabularFilled_SIZE
                && MemoryLayout<HyperTabularSlot>.stride == HyperTabularSlot_SIZE
                && MemoryLayout<HyperTabularOpened>.size == HyperTabularOpened_SIZE
                && MemoryLayout<HyperTabularTimestamp>.stride == 16
                && MemoryLayout<HyperTabularDate>.stride == 4
                && MemoryLayout<HyperTabularCivilDateTime>.stride == 16
                && MemoryLayout<HyperTabularDuration>.stride == 16
                && MemoryLayout<HyperTabularDecimal>.stride == 16
                && MemoryLayout<uuid_t>.stride == 16,
            "hypertabular.h's shapes are not the sizes the core writes")
        return true
    }()

    /// The dialect the text is read in.
    public let dialect: Dialect
    /// Rows per batch: what the column buffers are sized by when a plan is bound.
    private var batchRows: Int
    // The plan and the batch it is read into, once one is bound.
    private var columnSet: Columns?
    private var batch: Batch?
    /// Cell-table entries one row takes: the widest ordinal the plan reads, plus two, or
    /// more if the core asked for more.
    private var perRow = 0

    // What the core is handed beside the plan's buffers, allocated once and moved only to
    // grow: it is given addresses.
    private let state: UnsafeMutablePointer<HyperTabularDelimitedState>
    private(set) var cells: UnsafeMutableBufferPointer<HyperTabularSpan>
    private var arena: UnsafeMutableBufferPointer<UInt8>
    /// Whether the last batch ended early because the arena filled: the next one starts with
    /// it doubled, so that escaped text costs a few batches, not one per row.
    private var cramped = false

    // The source: a stream read into `input` — through a closure, or from an asynchronous
    // sequence through the pump that awaits it — or memory read in place.
    private let source: Source?
    private let pump: Pump?
    private var input: UnsafeMutableRawBufferPointer
    private let ownsInput: Bool
    private var start = 0
    private var end: Int
    private var eof: Bool
    /// The most of `input` the core is shown at once.
    private var windowLimit: Int
    /// The step ahead of the fill, for a dialect that declares an encoding: what is read lands
    /// there, and what it decodes to lands in `input`.
    private let decoder: Decoder?
    /// The dialect's tokens as the core reads them, at an address that is never null.
    private let tokens: UnsafeMutableBufferPointer<UInt8>

    private var failure: TabularError?
    /// Whether a read is in progress: the source runs inside one, and a read or bind it called
    /// on this reader would move or free the buffer it was handed.
    private var reading = false

    /// The header's names, when the dialect declares one — empty for an input with no
    /// record — and `nil` when it declares none. Read when the reader is opened, before any
    /// plan, so that a plan can be built from it: see `Header.ordinal(of:)`.
    public private(set) var header: Header?

    // MARK: - opening

    /// Reads delimited text already in memory: an array, a `Data`, a string's `utf8`.
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
        _ = try Self.checkSizes(dialect, batchRows: batchRows, bufferBytes: 1)
        let copy = UnsafeMutableRawBufferPointer.allocate(byteCount: bytes.count, alignment: 1)
        copy.copyBytes(from: bytes)
        guard dialect.encoding != .utf8 else {
            self.init(
                dialect: dialect, batchRows: batchRows, input: copy, ownsInput: true, filled: copy.count,
                source: nil, pump: nil, windowLimit: windowBytes, decoder: nil)
            try open(plan)
            return
        }
        // The memory is the decoder's, and what it decodes to lands in a buffer.
        self.init(
            dialect: dialect, batchRows: batchRows, input: Self.streamBuffer(Self.defaultBufferBytes), ownsInput: true,
            filled: 0, source: nil, pump: nil, windowLimit: Self.callLimit,
            decoder: Decoder(dialect.encoding, holding: copy))
        try open(plan)
    }

    /// Reads delimited text already in memory, in place — nothing is copied, and a text cell
    /// is a view of the caller's own bytes. Under a declared ``TextEncoding`` other than
    /// ``TextEncoding/utf8`` a cell is a view of what they decoded to, in the reader's own buffer.
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
        _ = try Self.checkSizes(dialect, batchRows: batchRows, bufferBytes: 1)
        guard dialect.encoding != .utf8 else {
            // Never written through: in memory the core only reads the input.
            self.init(
                dialect: dialect, batchRows: batchRows, input: UnsafeMutableRawBufferPointer(mutating: bytes),
                ownsInput: false, filled: bytes.count, source: nil, pump: nil, windowLimit: Self.callLimit,
                decoder: nil)
            try open(plan)
            return
        }
        self.init(
            dialect: dialect, batchRows: batchRows, input: Self.streamBuffer(Self.defaultBufferBytes), ownsInput: true,
            filled: 0, source: nil, pump: nil, windowLimit: Self.callLimit,
            decoder: Decoder(dialect.encoding, over: bytes))
        try open(plan)
    }

    /// Reads delimited text from a stream of any kind — a socket, a decompressor, a
    /// file the caller opened — through a closure that fills the reader's own buffer, so
    /// nothing is copied on the way in.
    ///
    /// - Parameters:
    ///   - source: Where the bytes come from; see ``Source``.
    ///   - dialect: The declared dialect.
    ///   - plan: The output columns, in output order.
    ///   - batchRows: Rows per batch.
    ///   - bufferBytes: The initial read buffer; it doubles when a record does not fit. Under a
    ///     declared encoding it is also the size of the buffer the original bytes are read into.
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
        let start = try Self.checkSizes(dialect, batchRows: batchRows, bufferBytes: bufferBytes)
        self.init(
            dialect: dialect, batchRows: batchRows, input: Self.streamBuffer(start), ownsInput: true,
            filled: 0, source: source, pump: nil, windowLimit: Self.callLimit,
            decoder: Decoder(dialect.encoding, size: start))
        try open(plan)
    }

    /// Opens a file of delimited text and reads it through a buffer — the file is
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
        // The core's answer on these sizes, ahead of touching the file.
        _ = try DelimitedReader.checkSizes(
            dialect, plan: Columns.sizeSpecs(plan), batchRows: batchRows, bufferBytes: bufferBytes)
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
        // The core's answer on these sizes, ahead of touching the file.
        _ = try DelimitedReader.checkSizes(dialect, plan: [], batchRows: batchRows, bufferBytes: bufferBytes)
        try self.init(
            reading: try Self.file(path), dialect: dialect, plan: nil, batchRows: batchRows, bufferBytes: bufferBytes)
    }

    /// Reads delimited text from an asynchronous sequence of bytes — `URL.resourceBytes`,
    /// `FileHandle.bytes`, a network body — into the reader's own buffer as the bytes arrive.
    /// The header is awaited here; the plan built from it is bound with ``bind(_:)``, and
    /// every batch is read with ``readAsync()``.
    ///
    /// - Parameters:
    ///   - bytes: The text, as it arrives.
    ///   - dialect: The declared dialect.
    ///   - batchRows: Rows per batch.
    ///   - bufferBytes: The initial read buffer; it doubles when a record does not fit. The
    ///     task that awaits the sequence reads up to this much ahead of the reader.
    /// - Throws: `CancellationError` when the task is cancelled while the header is awaited,
    ///   ``TabularError`` when the header record is structurally broken, and whatever the
    ///   sequence throws.
    public convenience init<Bytes: AsyncSequence>(
        bytes: sending Bytes, dialect: Dialect, batchRows: Int = DelimitedReader.defaultBatchRows,
        bufferBytes: Int = DelimitedReader.defaultBufferBytes
    ) async throws where Bytes.Element == UInt8 {
        try await self.init(bytes: bytes, dialect: dialect, plan: nil, batchRows: batchRows, bufferBytes: bufferBytes)
    }

    /// Reads delimited text from an asynchronous sequence of bytes, as the plan-less
    /// initializer of the same shape does, through `plan`.
    ///
    /// - Throws: `CancellationError` when the task is cancelled while the header is awaited,
    ///   ``TabularError`` when the header record is structurally broken, and whatever the
    ///   sequence throws.
    public convenience init<Bytes: AsyncSequence>(
        bytes: sending Bytes, dialect: Dialect, plan: [Column], batchRows: Int = DelimitedReader.defaultBatchRows,
        bufferBytes: Int = DelimitedReader.defaultBufferBytes
    ) async throws where Bytes.Element == UInt8 {
        try await self.init(
            bytes: bytes, dialect: dialect, plan: Optional(plan), batchRows: batchRows, bufferBytes: bufferBytes)
    }

    private convenience init<Bytes: AsyncSequence>(
        bytes: sending Bytes, dialect: Dialect, plan: [Column]?, batchRows: Int, bufferBytes: Int
    ) async throws where Bytes.Element == UInt8 {
        let start = try Self.checkSizes(dialect, batchRows: batchRows, bufferBytes: bufferBytes)
        let input = Self.streamBuffer(start)
        self.init(
            dialect: dialect, batchRows: batchRows, input: input, ownsInput: true, filled: 0, source: nil,
            pump: Pump(bytes, ahead: input.count), windowLimit: Self.callLimit,
            decoder: Decoder(dialect.encoding, size: start))
        if let plan {
            try bind(plan)
        }
        guard dialect.hasHeader else { return }
        var names = UnsafeMutableBufferPointer<HyperTabularSpan>.allocate(capacity: 64)
        defer { names.deallocate() }
        while try !headerStep(&names) {
            try await refillAsync()
        }
        try fitsHeader()
    }

    private init(
        dialect: Dialect, batchRows: Int, input: UnsafeMutableRawBufferPointer, ownsInput: Bool, filled: Int,
        source: Source?, pump: Pump?, windowLimit: Int, decoder: Decoder?
    ) {
        precondition(windowLimit > 0, "windowBytes must be positive; got \(windowLimit)")
        precondition(Self.layoutChecked)
        precondition(
            dialect.separator.isASCII,
            "Separator U+\(String(dialect.separator.value, radix: 16, uppercase: true)) is not a single ASCII byte")

        self.dialect = dialect
        self.batchRows = batchRows
        self.source = source
        self.pump = pump
        self.input = input
        self.ownsInput = ownsInput
        self.end = filled
        self.eof = source == nil && pump == nil && decoder == nil
        self.decoder = decoder
        self.windowLimit = min(windowLimit, Self.callLimit)

        state = .allocate(capacity: 1)
        var raw = HyperTabularRawDelimitedOptions()
        raw.size = UInt32(HyperTabularRawDelimitedOptions_SIZE)
        raw.separator = UInt8(truncatingIfNeeded: dialect.separator.value)
        raw.quoting = dialect.quoting ? 1 : 0
        raw.skip_blank_lines = dialect.skipBlankLines ? 1 : 0
        raw.skip = dialect.skip
        raw.header_row = dialect.headerRow
        raw.short_rows = dialect.shortRows.rawValue
        raw.long_rows = dialect.longRows.rawValue
        precondition(
            hypertabular_delimited_init_with(state, &raw) == HYPERTABULAR_OK,
            "Separator '\(dialect.separator)' is not tab or printable ASCII other than '\"'")
        tokens = Columns.hold(packedTokens(dialect.tokens))

        cells = .allocate(capacity: 0)
        arena = .allocate(capacity: 4096)
    }

    deinit {
        pump?.stop()
        state.deallocate()
        tokens.deallocate()
        // The batch in hand points into the rest, so they go when it does: if nothing else holds it,
        // straight after this.
        let (cells, arena, input, ownsInput) = (cells, arena, input, ownsInput)
        let free = {
            cells.deallocate()
            arena.deallocate()
            if ownsInput { input.deallocate() }
        }
        if let batch {
            batch.outlive(free)
        } else {
            free()
        }
    }

    /// The read buffer a streamed source starts with.
    private static func streamBuffer(_ bytes: Int) -> UnsafeMutableRawBufferPointer {
        .allocate(byteCount: bytes, alignment: 1)
    }

    /// The core's answer on a reader's sizes, with no plan yet: what it holds whatever the plan.
    /// ``bind(_:)`` asks again with one.
    private static func checkSizes(
        _ dialect: Dialect, plan: [HyperTabularColumnSpecWith] = [], batchRows: Int, bufferBytes: Int
    ) throws -> Int {
        try Columns.checkSizes(
            specs: plan, batchRows: batchRows, bufferBytes: bufferBytes, notes: dialect.notes,
            widths: dialect.longRows == .report, sheet: false)
    }

    /// A source that reads the file at `path`, which it keeps open for as long as it is held.
    ///
    /// Off Windows each call is one `read(2)` straight into the room, which hands over what a
    /// pipe or a FIFO has so far: `FileHandle.read(upToCount:)` waits there for the whole
    /// count, and would hold a complete header or batch back until the room filled or the
    /// writer closed.
    private static func file(_ path: String) throws -> Source {
        let file = try FileHandle(forReadingFrom: URL(fileURLWithPath: path))
        #if os(Windows)
            return { buffer in
                guard let chunk = try file.read(upToCount: buffer.count) else { return 0 }
                chunk.withUnsafeBytes { buffer.copyMemory(from: $0) }
                return chunk.count
            }
        #else
            return { buffer in
                while true {
                    let count = systemRead(file.fileDescriptor, buffer.baseAddress, buffer.count)
                    if count >= 0 { return count }
                    guard errno == EINTR else {
                        throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
                    }
                }
            }
        #endif
    }

    /// What every synchronous opening does once the reader exists: binds the plan, when it
    /// was given one — before the header, as ``bind(_:)`` would after it — and reads the
    /// header the dialect declares.
    private func open(_ plan: [Column]?) throws {
        if let plan {
            try bind(plan)
        }
        guard dialect.hasHeader else { return }
        var names = UnsafeMutableBufferPointer<HyperTabularSpan>.allocate(capacity: 64)
        defer { names.deallocate() }
        while try !headerStep(&names) {
            try refill()
        }
        try fitsHeader()
    }

    /// A plan bound before the header was read, held to the width the header gives every
    /// record — before any row is delivered, as ``bind(_:)`` holds a plan bound after it.
    private func fitsHeader() throws {
        if let width = columnCount, let columnSet {
            try Self.refusePast(width, columnSet.plan)
        }
    }

    /// ``PlanError/outOfReach(column:)`` naming the first column of `plan` that reads at or
    /// past `width`: the header fixes the width every record must have, so a column there
    /// could only ever be empty.
    private static func refusePast(_ width: Int, _ plan: [Column]) throws {
        if let index = plan.firstIndex(where: { $0.ordinal >= width }) {
            throw PlanError.outOfReach(column: index)
        }
    }

    /// The cell-table entries the reader holds at most, unless one row takes more: the
    /// default 4096-row batch of a plan reaching 256 source columns. A row takes an entry for
    /// every source column up to the widest the plan reads, read or not, so the table is
    /// sized by the plan's reach and not by its length; sized for a whole batch, an ordinal
    /// in the millions would take gigabytes. A plan that reaches further gets batches of
    /// fewer rows — the core fills only as many as the table holds — and never less than one.
    static let cellBudget = 4096 * 257

    /// The rows a cell table of `perRow` entries a row is sized for: as many as
    /// ``cellBudget`` holds, at least one and at most `batchRows`.
    static func cellRows(perRow: Int, batchRows: Int) -> Int {
        max(1, min(batchRows, cellBudget / max(perRow, 1)))
    }

    // MARK: - a suggestion

    /// Reads a sample of the file at `path` through the text door and says what its columns look
    /// like: a plan to look at, change and hand to a read, and a report. Nothing is sniffed in a
    /// read, and nothing here reaches one: this is its own read of the file, opened and closed here.
    /// There is no form of this for a stream, which would lose the rows it sampled.
    ///
    /// Declaring the locale is the recommended route: a number format and a date order are the
    /// plan's to say, and with them nothing needs inferring. A suggestion is a starting point for a
    /// plan to inspect.
    ///
    /// The width is the header's, or the first record's, and a plan has to name the columns before
    /// that record can be read. So with no header the file is opened twice, once to read the first
    /// record: a path that can only be read once is not for this. The dialect is what the sample is
    /// read with, its tokens included, and a cell it says is empty is blank to the suggestion.
    ///
    /// What it says can surprise, so read the report before the plan:
    ///
    /// - A column of `0` and `1` is ``Door/bool``, the first door asked, and `F` or `No` read false.
    /// - `1.234` alone is ambiguous, 1.234 or 1234, and reads as text until a format is declared or
    ///   a value settles it, which takes in a column of prices at three decimals. Beside `2.5` it is
    ///   a decimal, and beside `2,5` it is 1234.
    /// - `2,5`, `0,125`, `1.234,5` and `1.234.567` read only with `,` as the decimal, so they are
    ///   resolved in that notation, and ``SuggestedColumn/format`` names it. `2,5` beside `2.5` is
    ///   unresolved.
    /// - `1,23,456`, Indian grouping, reads under neither notation with none declared, because
    ///   nothing says its culture: alone it is text, resolved, and beside `5` unresolved. Declaring
    ///   `NumFormat.invariant.withGrouping(.indian)` for the column makes it a number.
    /// - `007` and an integer past `Int64` are text, with no note saying why, so as not to lose the
    ///   zeros or the digits.
    /// - A sheet's date column is text: ``Sheet/suggest(_:)`` says why.
    ///
    /// - Parameters:
    ///   - path: The file.
    ///   - dialect: The declared dialect.
    ///   - options: How the sample is taken and read.
    /// - Throws: ``PlanError/refused(column:)`` for a format the options declare that no door
    ///   accepts: first, before the file is opened, though another replaces it or the sample never
    ///   reaches its ordinal. ``TabularError`` for a failure among the sampled rows: there is no
    ///   knowing what is past it, and a read would stop there too. A failure past them is not met,
    ///   or only says the source was truncated. Foundation's error when the file cannot be opened.
    public static func suggest(
        contentsOfFile path: String, dialect: Dialect, options: SuggestOptions = SuggestOptions()
    ) throws -> Suggestion {
        try suggest(dialect, options) { dialect, batchRows in
            try DelimitedReader(contentsOfFile: path, dialect: dialect, batchRows: batchRows)
        }
    }

    /// ``suggest(contentsOfFile:dialect:options:)`` over text in memory.
    ///
    /// - Throws: ``PlanError/refused(column:)`` for a format the options declare that no door
    ///   accepts, first, before anything is read, and ``TabularError`` for a failure among the
    ///   sampled rows.
    public static func suggest<Bytes: Collection>(
        bytes: Bytes, dialect: Dialect, options: SuggestOptions = SuggestOptions()
    ) throws -> Suggestion where Bytes.Element == UInt8 {
        try suggest(dialect, options) { dialect, batchRows in
            try DelimitedReader(bytes: bytes, dialect: dialect, batchRows: batchRows)
        }
    }

    private static func suggest(
        _ dialect: Dialect, _ options: SuggestOptions, open: (Dialect, Int) throws -> DelimitedReader
    ) throws -> Suggestion {
        try options.checkFormats()
        let limit = options.rows(within: defaultBatchRows)
        var unnoted = dialect
        unnoted.notes = false
        var width = -1
        if !dialect.hasHeader {
            // A headerless source's width is its first record's, which a read says through a plan
            // of one column.
            let first = try open(unnoted, 1)
            try first.bind([.text(0)])
            _ = try first.read()
            width = first.columnCount ?? 0
        }
        let reader = try open(unnoted, 1)
        if width < 0 {
            width = reader.header?.count ?? 0
        }
        // One row more than the sample, so that a row past it costs no second batch to find, in as many
        // batches as the core says that takes.
        let sample = try Sample.of(
            width: width, limit: limit, sheet: false,
            bind: { plan in
                reader.batchRows = try Columns.sampleRows(
                    plan, wanted: limit + 1, notes: false, widths: dialect.longRows == .report, sheet: false)
                try reader.bind(plan)
            }, read: { try reader.read() })
        return try sample.judge(header: reader.header, options: options)
    }

    // MARK: - the plan and the position

    /// Declares the plan a reader opened without one reads through — once, before the first
    /// read. Every column buffer and the table that locates each cell are sized here, by the
    /// plan and the reader's batch size.
    ///
    /// - Throws: ``PlanError/alreadyBound`` when the reader has a plan, whether it was opened
    ///   with one or bound one already; the reader goes on with the plan it has.
    ///   ``PlanError/busy`` from inside one of the reader's own reads.
    ///   ``PlanError/outOfReach(column:)`` when a column's ordinal is 2²⁰ or more, or at or
    ///   past the columns a header read already gives every record;
    ///   ``PlanError/refused(column:)`` when the core will not read a column as declared. The
    ///   reader stays unbound.
    public func bind(_ plan: [Column]) throws {
        guard !reading else {
            throw PlanError.busy
        }
        guard columnSet == nil else {
            throw PlanError.alreadyBound
        }
        let columns = try Columns(
            plan: plan, batchRows: batchRows, notes: dialect.notes, rowWidths: dialect.longRows == .report)
        if let width = columnCount {
            try Self.refusePast(width, plan)
        }
        let rows = Self.cellRows(perRow: columns.width + 1, batchRows: batchRows)
        // At most the budget, or one row of 2²⁰ + 1 entries: no overflow, even in 32 bits.
        perRow = columns.width + 1
        cells.deallocate()
        cells = .allocate(capacity: perRow * rows)
        columnSet = columns
        batch = Batch(columns, workbook: false, reportsLongRows: dialect.longRows == .report)
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
    /// of it, and it doubles when that record fills it. Returns the room behind the record — or,
    /// for memory, widens the window the core is shown and returns `nil`, as it does under an
    /// encoding once the decoder is holding a failure and the fill has one more look to take.
    private func room() throws -> UnsafeMutableRawBufferPointer? {
        guard source != nil || pump != nil || decoder != nil else {
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
        if let decoder, decoder.failed {
            // A \r the fill is holding ends a row whatever comes next, and only waits to see if a
            // \n is part of it; nothing is coming, and this says so. In a quote it ends nothing,
            // and the record is as cut as it was.
            guard end > 0, input[end - 1] == 0x0D else {
                let error = TabularError(
                    kind: .encoding, record: records, line: Int(clamping: state.pointee.line),
                    byte: Int64(clamping: decoder.failedAt))
                failure = error
                throw error
            }
            if end == input.count { try growInput() }
            input[end] = 0x0A
            end += 1
            return nil
        }
        // One record fills the buffer: it needs a bigger one.
        if end == input.count { try growInput() }
        return UnsafeMutableRawBufferPointer(rebasing: input[end...])
    }

    /// Doubles the read buffer, up to the row ceiling.
    private func growInput() throws {
        guard input.count < Self.ceiling else { throw rowTooLong() }
        let larger = UnsafeMutableRawBufferPointer.allocate(
            byteCount: min(input.count * 2, Self.maxRowBytes), alignment: 1)
        larger.copyMemory(from: UnsafeRawBufferPointer(rebasing: input[..<end]))
        input.deallocate()
        input = larger
    }

    /// Gives the core more to look at behind the record it could not finish: for a stream,
    /// more is read behind it; in memory, the window it is shown widens.
    private func refill() throws {
        precondition(
            pump == nil, "A reader of an asynchronous sequence is read with readAsync(), not read()")
        guard let room = try room() else { return }
        if let decoder {
            while try !decode() {
                guard let source else { return }
                let into = decoder.room
                decoder.landed(try checked(source(into), into))
            }
            return
        }
        guard let source else { return }
        let read = try checked(source(room), room)
        if read == 0 {
            eof = true
        }
        end += read
    }

    /// What a source returned, held to what it was given.
    private func checked(_ read: Int, _ room: UnsafeMutableRawBufferPointer) -> Int {
        precondition(
            read >= 0 && read <= room.count,
            "A source returns how many bytes it wrote; got \(read) for room for \(room.count)")
        return read
    }

    /// ``refill()`` for any reader. A reader of an asynchronous sequence takes everything
    /// its pump has received, up to the room, and waits only when nothing has arrived — as a
    /// stream's one `read` hands over what is there — so a batch is as large as what has
    /// arrived allows, and a live sequence's header and rows are read while it waits for more.
    /// A cancellation or a failure of the sequence loses nothing: whatever arrived stays with
    /// the pump, and the next read goes on from it.
    private func refillAsync() async throws {
        try Task.checkCancellation()
        guard let pump else {
            try refill()
            return
        }
        guard let room = try room() else { return }
        if let decoder {
            while try !decode() {
                decoder.landed(try await pump.take(into: decoder.room))
            }
            return
        }
        let read = try await pump.take(into: room)
        if read == 0 {
            eof = true
        }
        end += read
    }

    /// Decodes what the decoder holds behind what the read buffer holds, with whatever that wants
    /// first: `true` once the fill has something new to look at (text, the end, or a failure to
    /// raise when it next asks), `false` when the source has to bring more first.
    private func decode() throws -> Bool {
        guard let decoder else { return true }
        while true {
            let room = input.count - end
            var done = HyperTabularRawTranscoded()
            let code = hypertabular_transcode(
                decoder.encoding, decoder.pending, UInt(decoder.end - decoder.at), decoder.eof ? 1 : 0,
                input.baseAddress.map { ($0 + end).assumingMemoryBound(to: UInt8.self) }, UInt(room), &done)
            guard code == HYPERTABULAR_OK else {
                preconditionFailure(Self.contractViolation)
            }
            if done.failed != 0 {
                decoder.failed = true
                decoder.failedAt = decoder.offset + done.failure_at
            }
            decoder.at += Int(done.consumed)
            decoder.offset += done.consumed
            if done.written > 0 {
                end += Int(done.written)
                return true
            }
            // The next refill raises it, once the fill has had one more look.
            if decoder.failed { return true }
            if decoder.at < decoder.end && room < Decoder.maxCharacter {
                try growInput()
                continue
            }
            if decoder.eof {
                eof = true
                return true
            }
            decoder.compact()
            return false
        }
    }

    private func rowTooLong() -> TabularError {
        let error = TabularError(
            kind: .rowTooLong, record: Int64(clamping: state.pointee.records),
            line: Int(clamping: state.pointee.line), byte: Int64(clamping: state.pointee.offset))
        failure = error
        return error
    }

    private func structural(_ raw: HyperTabularFailure) -> TabularError {
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
    private func located(_ span: HyperTabularSpan, in base: UnsafeRawPointer?) -> UnsafeRawBufferPointer {
        let from = span.len & spanFlag != 0 ? UnsafeRawPointer(arena.baseAddress) : base
        return UnsafeRawBufferPointer(
            start: from.map { $0 + Int(span.offset) },
            count: Int(span.len & ~spanFlag))
    }

    /// Reads the header from what is buffered: `true` once it has been read, `false` when
    /// the core needs more input first. `names` grows when the core asks.
    private func headerStep(_ names: inout UnsafeMutableBufferPointer<HyperTabularSpan>) throws -> Bool {
        var filled = HyperTabularFilled()
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

    /// Marks a read in progress, or refuses one inside another: what every read does before
    /// anything else. The read clears the mark however it ends, a throw from the source included.
    private func enter() throws {
        guard !reading else {
            throw PlanError.busy
        }
        reading = true
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
        var filled = HyperTabularFilled()
        while true {
            let (base, length, last) = nextWindow()
            // The buffers travel in a block: Mono's interpreter passes no more than twelve integer
            // arguments to a native function, and the C ABI is the same for every binding.
            var buffers = HyperTabularBuffersWith()
            buffers.size = buffersSize
            buffers.cells = cells.baseAddress
            buffers.cells_cap = UInt(cells.count)
            buffers.arena = arena.baseAddress
            buffers.arena_cap = UInt(arena.count)
            buffers.tokens = UnsafePointer(tokens.baseAddress)
            buffers.tokens_len = UInt(dialect.tokens.isEmpty ? 0 : tokens.count)
            buffers.row_widths = columnSet.rowWidths
            let code = hypertabular_delimited_fill_with(
                state, base, UInt(length), last ? 1 : 0, columnSet.specs, columnSet.buffers,
                UInt(columnSet.plan.count), UInt(columnSet.batchRows), &buffers, &filled)
            switch code {
            case HYPERTABULAR_OK:
                let consumed = Int(filled.consumed)
                start += consumed
                if filled.rows > 0 {
                    let rows = Int(filled.rows)
                    // Short of what the cell table holds, not of the batch size: a plan that
                    // reaches far is given fewer rows than that by the table alone.
                    let holds = min(columnSet.batchRows, cells.count / max(perRow, 1))
                    cramped = rows < holds && filled.arena_used * 2 >= UInt64(arena.count)
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
                cells = .allocate(
                    capacity: perRow * Self.cellRows(perRow: perRow, batchRows: columnSet.batchRows))
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
    ///   ``PlanError/unbound`` while the reader has no plan, ``PlanError/busy`` from inside one
    ///   of the reader's own reads — called by its source — and whatever a streamed source
    ///   throws.
    public func read() throws -> Batch? {
        try enter()
        defer { reading = false }
        let (columnSet, batch) = try begin()
        while true {
            switch try fillStep(columnSet) {
            case .rows(let rows, let base):
                return batch.fill(
                    rows: rows, cells: cells, perRow: perRow, base: base, arena: arena, expected: state.pointee.expected
                )
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
    /// An asynchronous sequence is awaited by a task the reader owns, which reads up to
    /// `bufferBytes` ahead and stops when the reader is released. A read takes everything
    /// that has arrived without waiting for more, and waits only when nothing has: a batch is
    /// `batchRows` rows when that many have arrived, and a live sequence that pauses after a
    /// complete row has that row read before the pause ends (within 200 µs of its arrival). A sequence's error is thrown
    /// after every byte before it has been read, and the next read goes on from the sequence.
    ///
    /// Cancellation is checked before the read begins and before each wait for more input,
    /// and is thrown as `CancellationError`. A cancelled read loses nothing: every byte that
    /// had arrived stays buffered, and the next read goes on from there.
    ///
    /// - Throws: `CancellationError` when the task is cancelled, and whatever ``read()``
    ///   throws.
    public func readAsync() async throws -> Batch? {
        try enter()
        defer { reading = false }
        try Task.checkCancellation()
        let (columnSet, batch) = try begin()
        while true {
            switch try fillStep(columnSet) {
            case .rows(let rows, let base):
                return batch.fill(
                    rows: rows, cells: cells, perRow: perRow, base: base, arena: arena, expected: state.pointee.expected
                )
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

/// The step ahead of the fill, for a dialect that declares an encoding: the original bytes, which
/// go through the core's transcoder into the reader's buffer, where the fill reads them as it would
/// have read UTF-8.
private final class Decoder {
    /// The most bytes of UTF-8 one character takes: with this much room, a transcode that writes
    /// nothing and does not fail wants more input.
    static let maxCharacter = 4

    let encoding: UInt32
    /// The original bytes, `at` to `end` of them not yet decoded. A stream's reads land here: fixed
    /// in size, since what is left when more is wanted is less than a character.
    private let raw: UnsafeMutableRawBufferPointer
    private let owned: Bool
    var at = 0
    var end = 0
    /// Whether the source has ended.
    var eof = false
    /// The original bytes decoded so far: added to a call's `failure_at`, this is the input's offset.
    var offset: UInt64 = 0
    /// A unit has no character, at `failedAt` in the original input: raised only when the fill has
    /// had all that was decoded ahead of it, and wants more.
    var failed = false
    var failedAt: UInt64 = 0

    /// A decoder of a stream, or `nil` for UTF-8, which has none.
    convenience init?(_ encoding: TextEncoding, size: Int) {
        guard encoding != .utf8 else { return nil }
        self.init(
            encoding,
            raw: .allocate(byteCount: min(max(size, Self.maxCharacter), DelimitedReader.maxRowBytes), alignment: 1),
            owned: true, filled: 0, eof: false)
    }

    /// A decoder of memory it takes over, all there already.
    convenience init(_ encoding: TextEncoding, holding bytes: UnsafeMutableRawBufferPointer) {
        self.init(encoding, raw: bytes, owned: true, filled: bytes.count, eof: true)
    }

    /// A decoder of memory the caller holds, all there already, and never written through.
    convenience init(_ encoding: TextEncoding, over bytes: UnsafeRawBufferPointer) {
        self.init(
            encoding, raw: UnsafeMutableRawBufferPointer(mutating: bytes), owned: false, filled: bytes.count, eof: true)
    }

    private init(_ encoding: TextEncoding, raw: UnsafeMutableRawBufferPointer, owned: Bool, filled: Int, eof: Bool) {
        self.encoding = encoding.rawValue
        self.raw = raw
        self.owned = owned
        end = filled
        self.eof = eof
    }

    deinit {
        if owned { raw.deallocate() }
    }

    /// What is left to decode.
    var pending: UnsafePointer<UInt8>? {
        raw.baseAddress.map { UnsafePointer(($0 + at).assumingMemoryBound(to: UInt8.self)) }
    }

    /// The room behind what is left, for a read of the stream.
    var room: UnsafeMutableRawBufferPointer {
        UnsafeMutableRawBufferPointer(rebasing: raw[end...])
    }

    /// Puts what is left undecoded at the front.
    func compact() {
        guard at > 0, let base = raw.baseAddress else { return }
        base.copyMemory(from: base + at, byteCount: end - at)
        end -= at
        at = 0
    }

    /// Counts what a read of the stream brought in.
    func landed(_ read: Int) {
        if read == 0 { eof = true }
        end += read
    }
}

#if !os(Windows)
    /// `read(2)`, by a name the reader's own `read()` does not hide.
    private func systemRead(_ descriptor: Int32, _ buffer: UnsafeMutableRawPointer?, _ count: Int) -> Int {
        read(descriptor, buffer, count)
    }
#endif

/// The task that awaits an asynchronous sequence for a reader, and what it has received.
///
/// The sequence hands over a byte at a time and cannot say whether another is waiting, so
/// its task receives the bytes into a buffer of its own, and the reader takes them from
/// there without waiting. The task hands each line over as its end arrives — a record ends
/// only at a `\n`, or at a `\r` once the byte after it has told a CR from a CRLF — and
/// every 4 KiB of a longer one; it waits while the reader has `ahead` bytes still to take,
/// or a failure of the sequence, and ends with the sequence or when it is stopped. Only the
/// task ever calls the iterator.
///
/// A reader waiting for bytes is woken once `wakeBytes` have arrived, or the sequence ends
/// or fails, or else at most ``wakeDelay`` after the first line that arrived for it: a
/// sequence that pauses after a line has it read within that, and one that is ahead of the
/// reader is read a `wakeBytes` batch at a time, not a line per wake — a wake costs about
/// what a line does. Under WASI's one-thread executor the reader cannot run before the task
/// waits, so it is woken at once.
private final class Pump: @unchecked Sendable {
    /// The most of one line the task holds before it hands it over.
    private static let chunkBytes = 4096
    /// The longest a line that has arrived waits for the reader to be woken.
    static let wakeDelay: Duration = .microseconds(200)

    private let lock = NSLock()
    /// What has arrived and the reader has not taken: `arrived[head...]`.
    private var arrived: [UInt8] = []
    private var head = 0
    /// A failure of the sequence, for the reader to take once it has taken every byte before.
    private var failure: (any Error)?
    private var ended = false
    /// The reader, waiting for something to arrive; the task, waiting for the reader.
    private var reader: CheckedContinuation<Void, Never>?
    private var pumping: CheckedContinuation<Void, Never>?
    /// Whether a wake of the reader is due within ``wakeDelay``.
    private var wakeDue = false
    private let ahead: Int
    private let wakeBytes: Int
    private var task: Task<Void, Never>?

    init<Bytes: AsyncSequence>(_ bytes: sending Bytes, ahead: Int) where Bytes.Element == UInt8 {
        self.ahead = ahead
        wakeBytes = min(ahead, 64 * 1024)
        var iterator = bytes.makeAsyncIterator()
        let next: () async throws -> UInt8? = { try await iterator.next() }
        task = Task { [self] in await run(next) }
    }

    /// Ends the task: at once when it is waiting for the reader, or else as soon as the
    /// sequence hands it a line or ends.
    func stop() {
        task?.cancel()
    }

    /// The task: awaits the sequence a byte at a time, handing each line over as its end
    /// arrives.
    private func run(_ next: () async throws -> UInt8?) async {
        let line = UnsafeMutableBufferPointer<UInt8>.allocate(capacity: Self.chunkBytes)
        defer { line.deallocate() }
        var count = 0
        var afterCR = false
        while true {
            let byte: UInt8?
            do {
                byte = try await next()
            } catch {
                hand(line, &count, failure: error)
                guard await waitForReader() else { return }
                continue
            }
            guard let byte else {
                hand(line, &count, ended: true)
                return
            }
            line[count] = byte
            count += 1
            if byte == 0x0A || afterCR || count == Self.chunkBytes {
                if hand(line, &count) {
                    guard await waitForReader() else { return }
                } else if Task.isCancelled {
                    return
                }
            }
            afterCR = byte == 0x0D
        }
    }

    /// Hands the `count` bytes of `line` over, with the failure or the end that follows them,
    /// and wakes the reader if it is waiting and the wake is due. Returns whether the reader
    /// is now behind: `ahead` bytes to take, or a failure.
    @discardableResult
    private func hand(
        _ line: UnsafeMutableBufferPointer<UInt8>, _ count: inout Int, failure error: (any Error)? = nil,
        ended end: Bool = false
    ) -> Bool {
        lock.lock()
        arrived.append(contentsOf: UnsafeBufferPointer(rebasing: line[..<count]))
        failure = error
        ended = end
        var waking: CheckedContinuation<Void, Never>?
        var arming = false
        if let waiting = reader {
            #if os(WASI)
                let now = true
            #else
                let now = end || error != nil || arrived.count - head >= wakeBytes
            #endif
            if now {
                waking = waiting
                reader = nil
            } else if !wakeDue {
                wakeDue = true
                arming = true
            }
        }
        let behind = error != nil || arrived.count - head >= ahead
        lock.unlock()
        count = 0
        waking?.resume()
        if arming {
            Task { [weak self] in
                try? await Task.sleep(for: Self.wakeDelay)
                self?.wakeIfDue()
            }
        }
        return behind
    }

    /// The delayed wake: the reader, if it is still waiting, for what has arrived.
    private func wakeIfDue() {
        lock.lock()
        wakeDue = false
        let waking = arrived.count > head ? reader : nil
        if waking != nil { reader = nil }
        lock.unlock()
        waking?.resume()
    }

    /// Waits while the reader is behind: `false` once the task is stopped.
    private func waitForReader() async -> Bool {
        while !Task.isCancelled {
            let behind = lock.withLock { failure != nil || arrived.count - head >= ahead }
            guard behind else { break }
            await withTaskCancellationHandler {
                await withCheckedContinuation { continuation in
                    lock.lock()
                    if Task.isCancelled || (failure == nil && arrived.count - head < ahead) {
                        lock.unlock()
                        continuation.resume()
                    } else {
                        pumping = continuation
                        lock.unlock()
                    }
                }
            } onCancel: {
                wake(reader: false)
            }
        }
        return !Task.isCancelled
    }

    /// Wakes whichever of the two is waiting: on a cancellation, which it rechecks.
    private func wake(reader wakesReader: Bool) {
        lock.lock()
        let waiting = wakesReader ? reader : pumping
        if wakesReader { reader = nil } else { pumping = nil }
        lock.unlock()
        waiting?.resume()
    }

    /// What a take found: bytes moved, the sequence's failure, its end, or nothing yet.
    private enum Taken {
        case bytes(Int)
        case failure(any Error)
        case ended
        case nothing
    }

    /// The reader's side: moves what has arrived into `room`, as much as fits, and returns how
    /// much — `0` once the sequence has ended. It waits only when nothing has arrived, and
    /// throws the sequence's failure once every byte before it has been taken, or
    /// `CancellationError`, which takes nothing.
    func take(into room: UnsafeMutableRawBufferPointer) async throws -> Int {
        while true {
            try Task.checkCancellation()
            switch takeNow(into: room) {
            case .bytes(let count):
                return count
            case .failure(let error):
                throw error
            case .ended:
                return 0
            case .nothing:
                await withTaskCancellationHandler {
                    await withCheckedContinuation { continuation in
                        lock.lock()
                        if Task.isCancelled || arrived.count > head || failure != nil || ended {
                            lock.unlock()
                            continuation.resume()
                        } else {
                            reader = continuation
                            lock.unlock()
                        }
                    }
                } onCancel: {
                    wake(reader: true)
                }
            }
        }
    }

    /// ``take(into:)`` without the wait, waking the task when the reader is no longer behind.
    private func takeNow(into room: UnsafeMutableRawBufferPointer) -> Taken {
        lock.lock()
        let taken: Taken
        let available = arrived.count - head
        if available > 0 {
            let count = min(available, room.count)
            arrived.withUnsafeBytes {
                room.copyMemory(from: UnsafeRawBufferPointer(rebasing: $0[head..<head + count]))
            }
            head += count
            if head == arrived.count {
                arrived.removeAll(keepingCapacity: true)
                head = 0
            } else if head > arrived.count / 2 {
                arrived.removeFirst(head)
                head = 0
            }
            taken = .bytes(count)
        } else if let error = failure {
            failure = nil
            taken = .failure(error)
        } else {
            taken = ended ? .ended : .nothing
        }
        let waiting = failure == nil && arrived.count - head < ahead ? pumping : nil
        if waiting != nil { pumping = nil }
        lock.unlock()
        waiting?.resume()
        return taken
    }
}
