import Foundation
import HyperTabularCore

/// Which kind of workbook a ``Workbook`` is.
public enum WorkbookFormat: Equatable, Sendable {
    /// Office Open XML: `.xlsx`, `.xlsm`.
    case xlsx
    /// OpenDocument: `.ods`.
    case ods
}

/// One sheet of a workbook, as ``Workbook/sheets`` lists it.
public struct SheetInfo: Equatable, Sendable {
    /// The sheet's name.
    public let name: String
    /// Whether the workbook hides the sheet.
    public let hidden: Bool

    // What the core is given back to open it: the XLSX part, or the ODS table's index.
    let part: [UInt8]
    let index: UInt32
}

/// How a sheet is read.
public struct SheetOptions: Equatable, Sendable {
    /// Whether the sheet's first row is a header, exposed through ``Sheet/header`` and
    /// never delivered as a row.
    public var hasHeader: Bool
    /// Whether a row with no cells is skipped rather than delivered as a row of empty cells.
    public var skipEmptyRows: Bool
    /// Rows per batch — the capacity of every column buffer. Must be positive.
    public var batchRows: Int

    /// The options, stated; each defaults to ``default``'s.
    public init(
        hasHeader: Bool = true, skipEmptyRows: Bool = true, batchRows: Int = DelimitedReader.defaultBatchRows
    ) {
        self.hasHeader = hasHeader
        self.skipEmptyRows = skipEmptyRows
        self.batchRows = batchRows
    }

    /// A header, empty rows skipped, ``DelimitedReader/defaultBatchRows`` rows a batch.
    public static let `default` = SheetOptions()
}

/// The error ``Workbook/sheet(_:options:plan:)`` and ``Workbook/sheet(named:options:plan:)``
/// throw for a sheet the workbook does not have.
public struct NoSuchSheet: Error, Equatable, Sendable, CustomStringConvertible, LocalizedError {
    /// The sheet asked for: `at index 3`, `named "Totals"`.
    public let which: String

    /// What was asked for, in a sentence.
    public var description: String { "The workbook has no sheet \(which)." }

    /// ``description``, for `localizedDescription`.
    public var errorDescription: String? { description }
}

/// An XLSX or ODS workbook held in memory, its sheets listed and its shared strings and
/// styles loaded: what a ``Sheet`` reads from.
///
/// ```swift
/// let book = try Workbook(contentsOfFile: "orders.xlsx")
/// let sheet = try book.sheet(named: "Orders", plan: [.i32(0), .text(1), .f64(2)])
/// while let batch = try sheet.read() { /* … */ }
/// ```
///
/// A workbook that cannot be read — not a zip, encrypted, a part missing or broken — is a
/// ``TabularError`` from the initializer. Its sheets may be read at once, each with buffers
/// of its own, and each keeps the workbook alive. Not thread-safe.
public final class Workbook {
    static let contractViolation =
        "libhypertabular reported a contract violation — a binding bug, please report it"

    /// For the tests: every buffer a workbook call works in starts with room for one element
    /// and no shared-strings bound is asked for, so every call that can stop and resume does
    /// — the grow-and-keep path exercised mid-part.
    nonisolated(unsafe) static var stingy = false

    /// For the tests: how many times the window, the arena and the cell table were grown.
    nonisolated(unsafe) static var grown = (window: 0, arena: 0, cells: 0)

    /// The window a part is inflated through starts at the size the core requires.
    private static let windowMin = 64 * 1024

    private let container: UnsafeMutableRawBufferPointer
    private let ownsContainer: Bool
    /// The state as opening left it: the template every sheet's own is copied from.
    let state: UnsafeMutableRawBufferPointer

    /// Which kind of workbook this is.
    public let format: WorkbookFormat
    /// The date system the workbook's serials count in: what a date-formatted number is
    /// read by.
    public let dateSystem: ExcelEpoch
    /// The workbook's sheets, in its own order. Sheets that hold no cells (chart sheets,
    /// macro sheets) are not among them.
    public let sheets: [SheetInfo]

    // The workbook's tables, which every read of a sheet is handed.
    let strings: UnsafeMutableBufferPointer<UInt8>
    private let table: UnsafeMutableBufferPointer<hypertabular_span>
    private let kinds: UnsafeMutableBufferPointer<UInt8>

    /// Opens a workbook held in memory: an array, a `Data`. The bytes are copied once, so
    /// the workbook does not depend on the collection outliving it.
    ///
    /// - Throws: ``TabularError`` when the bytes are not a workbook this reader can read.
    public convenience init<Bytes: Collection>(bytes: Bytes) throws where Bytes.Element == UInt8 {
        let copy = UnsafeMutableRawBufferPointer.allocate(byteCount: bytes.count, alignment: 1)
        copy.copyBytes(from: bytes)
        try self.init(container: copy, owns: true)
    }

    /// Opens a workbook in memory the caller holds, in place — nothing is copied. The caller
    /// keeps `bytes` alive and unchanged for as long as the workbook or any of its sheets is
    /// used.
    ///
    /// - Throws: ``TabularError`` when the bytes are not a workbook this reader can read.
    public convenience init(bytesNoCopy bytes: UnsafeRawBufferPointer) throws {
        // Never written through: the core only reads the container.
        try self.init(container: UnsafeMutableRawBufferPointer(mutating: bytes), owns: false)
    }

    /// Reads `source` to its end and opens what it held — an embedded resource, a network
    /// response, anything that is a stream rather than a file. A workbook is read from its
    /// end (a zip's directory is there), so the whole container is read first, into memory
    /// the workbook then owns; the format is told from the bytes.
    ///
    /// - Throws: Whatever `source` throws, and ``TabularError`` when the bytes are not a
    ///   workbook this reader can read.
    public convenience init(reading source: DelimitedReader.Source) throws {
        var container = Container()
        do {
            while true {
                let room = container.room()
                let read = try source(room)
                precondition(
                    read >= 0 && read <= room.count,
                    "A source returns how many bytes it wrote; got \(read) for room for \(room.count)")
                if read == 0 { break }
                container.count += read
            }
        } catch {
            container.release()
            throw error
        }
        try self.init(container: container.bytes, owns: true)
    }

    /// Reads `stream` to its end and opens what it held, as the initializer reading a
    /// ``DelimitedReader/Source`` does. The stream is opened if it is not open yet, and closed once it has been read.
    ///
    /// - Throws: The stream's error when it fails, and ``TabularError`` when the bytes are
    ///   not a workbook this reader can read.
    public convenience init(reading stream: InputStream) throws {
        if stream.streamStatus == .notOpen {
            stream.open()
        }
        defer { stream.close() }
        try self.init(reading: { room in
            guard let base = room.baseAddress, room.count > 0 else { return 0 }
            let read = stream.read(base.assumingMemoryBound(to: UInt8.self), maxLength: room.count)
            guard read >= 0 else {
                throw stream.streamError ?? CocoaError(.fileReadUnknown)
            }
            return read
        })
    }

    /// Awaits an asynchronous sequence of bytes to its end — `URL.resourceBytes`,
    /// `FileHandle.bytes`, a network body — and opens what it held, as the
    /// initializer reading a ``DelimitedReader/Source`` does.
    ///
    /// Cancellation is checked as the bytes arrive and thrown as `CancellationError`;
    /// what had arrived is released.
    ///
    /// - Throws: `CancellationError` when the task is cancelled, whatever the sequence
    ///   throws, and ``TabularError`` when the bytes are not a workbook this reader can read.
    public convenience init<Bytes: AsyncSequence>(bytes: Bytes) async throws where Bytes.Element == UInt8 {
        var container = Container()
        do {
            try Task.checkCancellation()
            var room = container.room()
            var filled = 0
            for try await byte in bytes {
                if filled == room.count {
                    container.count += filled
                    try Task.checkCancellation()
                    room = container.room()
                    filled = 0
                }
                room[filled] = byte
                filled += 1
            }
            container.count += filled
        } catch {
            container.release()
            throw error
        }
        try self.init(container: container.bytes, owns: true)
    }

    /// Reads the file at `path` into memory and opens it.
    ///
    /// - Throws: Foundation's error when the file cannot be read, and ``TabularError`` when
    ///   it is not a workbook this reader can read.
    public convenience init(contentsOfFile path: String) throws {
        try self.init(bytes: try Data(contentsOf: URL(fileURLWithPath: path)))
    }

    /// A container read from a stream, grown as it arrives.
    private struct Container {
        private var buffer = UnsafeMutableRawBufferPointer.allocate(byteCount: 64 * 1024, alignment: 1)
        /// How much of the buffer has been read into.
        var count = 0

        /// The room behind what has been read, doubled when there is none.
        mutating func room() -> UnsafeMutableRawBufferPointer {
            if count == buffer.count {
                let larger = UnsafeMutableRawBufferPointer.allocate(byteCount: buffer.count * 2, alignment: 1)
                larger.copyMemory(from: UnsafeRawBufferPointer(buffer))
                buffer.deallocate()
                buffer = larger
            }
            return UnsafeMutableRawBufferPointer(rebasing: buffer[count...])
        }

        /// What has been read: the buffer, whose memory the workbook takes over.
        var bytes: UnsafeMutableRawBufferPointer {
            UnsafeMutableRawBufferPointer(rebasing: buffer[..<count])
        }

        /// Releases the buffer, for a read that failed.
        func release() {
            buffer.deallocate()
        }
    }

    private init(container: UnsafeMutableRawBufferPointer, owns: Bool) throws {
        self.container = container
        self.ownsContainer = owns
        state = .allocate(byteCount: Int(hypertabular_workbook_state_size()), alignment: 8)
        let scratch =
            Self.stingy
            ? Scratch(window: 1, arena: 1, cells: 1, row: 0)
            : Scratch(window: Self.windowMin, arena: 1024, cells: 64, row: 0)
        var strings = UnsafeMutableBufferPointer<UInt8>.allocate(capacity: 0)
        var table = UnsafeMutableBufferPointer<hypertabular_span>.allocate(capacity: 0)
        var kinds = UnsafeMutableBufferPointer<UInt8>.allocate(capacity: 0)
        do {
            var opened = hypertabular_opened()
            open: while true {
                var buffers = scratch.buffers()
                switch hypertabular_workbook_open(
                    state.baseAddress, Self.address(container), UInt(container.count), &buffers, &opened)
                {
                case HYPERTABULAR_OK:
                    break open
                case HYPERTABULAR_ERR_WINDOW:
                    scratch.window = scratch.window.grown(toHold: opened.needed)
                case HYPERTABULAR_ERR_ARENA:
                    scratch.arena = scratch.arena.grown(toHold: opened.needed)
                case HYPERTABULAR_ERR_STRUCTURE:
                    throw TabularError(opened.failure)
                default:
                    preconditionFailure(Self.contractViolation)
                }
            }
            format = opened.format == 2 ? .ods : .xlsx
            guard let epoch = ExcelEpoch(rawValue: opened.epoch) else {
                preconditionFailure(Self.contractViolation)
            }
            dateSystem = epoch

            var filled = try Self.settled(scratch.drive(.sheets, state, container, tables: nil))
            sheets = (0..<Int(filled.rows)).map { index in
                let name = scratch.cells[3 * index]
                let part = scratch.cells[3 * index + 1]
                let last = scratch.cells[3 * index + 2]
                return SheetInfo(
                    name: String(
                        decoding: scratch.arena[Int(name.offset)..<Int(name.offset + name.len)], as: UTF8.self),
                    hidden: last.offset & 1 != 0,
                    part: Array(scratch.arena[Int(part.offset)..<Int(part.offset + part.len)]),
                    index: last.len)
            }

            // The shared strings take no more room than their part inflates to; asking for
            // it once saves growing into it.
            let bound = min(opened.strings_bytes, 1 << 28)
            if !Self.stingy && UInt64(scratch.arena.count) < bound {
                scratch.arena = scratch.arena.grown(toHold: bound)
            }
            filled = try Self.settled(scratch.drive(.strings, state, container, tables: nil))
            strings.deallocate()
            strings = Self.copy(scratch.arena, count: Int(filled.arena_used))
            table.deallocate()
            table = Self.copy(scratch.cells, count: Int(filled.rows))

            filled = try Self.settled(scratch.drive(.styles, state, container, tables: nil))
            kinds.deallocate()
            kinds = Self.copy(scratch.arena, count: Int(filled.rows))
        } catch {
            strings.deallocate()
            table.deallocate()
            kinds.deallocate()
            state.deallocate()
            if owns { container.deallocate() }
            throw error
        }
        self.strings = strings
        self.table = table
        self.kinds = kinds
    }

    deinit {
        strings.deallocate()
        table.deallocate()
        kinds.deallocate()
        state.deallocate()
        if ownsContainer { container.deallocate() }
    }

    private static func copy<T>(_ from: UnsafeMutableBufferPointer<T>, count: Int) -> UnsafeMutableBufferPointer<T> {
        let copy = UnsafeMutableBufferPointer<T>.allocate(capacity: count)
        if count > 0 {
            _ = copy.initialize(from: from[..<count])
        }
        return copy
    }

    static func address(_ bytes: UnsafeMutableRawBufferPointer) -> UnsafePointer<UInt8>? {
        bytes.baseAddress.map { UnsafePointer($0.assumingMemoryBound(to: UInt8.self)) }
    }

    /// What a call's code means: done, or a structural failure. Anything else is this
    /// binding's bug.
    static func settled(_ answer: (code: Int32, filled: hypertabular_filled)) throws -> hypertabular_filled {
        switch answer.code {
        case HYPERTABULAR_OK:
            return answer.filled
        case HYPERTABULAR_ERR_STRUCTURE:
            throw TabularError(answer.filled.failure)
        default:
            preconditionFailure(contractViolation)
        }
    }

    /// The container, as a sheet's calls are handed it.
    var containerBytes: UnsafeMutableRawBufferPointer { container }

    /// The workbook's tables in a buffers block, for a call that reads a sheet.
    func tables(_ buffers: inout hypertabular_buffers) {
        buffers.strings = UnsafePointer(strings.baseAddress)
        buffers.strings_len = UInt(strings.count)
        buffers.table = UnsafePointer(table.baseAddress)
        buffers.table_len = UInt(table.count)
        buffers.kinds = UnsafePointer(kinds.baseAddress)
        buffers.kinds_len = UInt(kinds.count)
    }

    /// Starts a read of the sheet at `index` of ``sheets`` through `plan`. When the options
    /// declare a header it is read here.
    ///
    /// - Throws: ``NoSuchSheet`` when the workbook has no sheet at that index, and
    ///   ``TabularError`` when the sheet, or its header row, is structurally broken.
    public func sheet(_ index: Int, options: SheetOptions = .default, plan: [Column]) throws -> Sheet {
        try Sheet(self, info(at: index), options: options, plan: plan)
    }

    /// Starts a read of the sheet at `index` of ``sheets`` and reads its header, leaving the
    /// plan to be bound with ``Sheet/bind(_:)`` once the header has said where each column is.
    ///
    /// - Throws: ``NoSuchSheet`` when the workbook has no sheet at that index, and
    ///   ``TabularError`` when the sheet, or its header row, is structurally broken.
    public func sheet(_ index: Int, options: SheetOptions = .default) throws -> Sheet {
        try Sheet(self, info(at: index), options: options, plan: nil)
    }

    /// Starts a read of the first sheet named `name` through `plan`, as
    /// ``sheet(_:options:plan:)`` does.
    ///
    /// - Throws: ``NoSuchSheet`` when the workbook has no sheet by that name, and
    ///   ``TabularError`` when the sheet, or its header row, is structurally broken.
    public func sheet(named name: String, options: SheetOptions = .default, plan: [Column]) throws -> Sheet {
        try Sheet(self, info(named: name), options: options, plan: plan)
    }

    /// Starts a read of the first sheet named `name` and reads its header, leaving the plan
    /// to be bound, as ``sheet(_:options:)`` does.
    ///
    /// - Throws: ``NoSuchSheet`` when the workbook has no sheet by that name, and
    ///   ``TabularError`` when the sheet, or its header row, is structurally broken.
    public func sheet(named name: String, options: SheetOptions = .default) throws -> Sheet {
        try Sheet(self, info(named: name), options: options, plan: nil)
    }

    private func info(at index: Int) throws -> SheetInfo {
        guard sheets.indices.contains(index) else {
            throw NoSuchSheet(which: "at index \(index)")
        }
        return sheets[index]
    }

    private func info(named name: String) throws -> SheetInfo {
        guard let info = sheets.first(where: { $0.name == name }) else {
            throw NoSuchSheet(which: "named \"\(name)\"")
        }
        return info
    }

    /// The workbook calls that take the shared buffers and may ask to have one grown.
    enum Call {
        case sheets, strings, styles, header, fill
    }

    /// The buffers a call to the core may ask to have grown: one set for the workbook while
    /// it opens, one for each sheet.
    final class Scratch {
        var window: UnsafeMutableBufferPointer<UInt8>
        var arena: UnsafeMutableBufferPointer<UInt8>
        var cells: UnsafeMutableBufferPointer<hypertabular_span>
        var row: UnsafeMutableBufferPointer<hypertabular_slot>

        init(window: Int, arena: Int, cells: Int, row: Int) {
            self.window = .allocate(capacity: window)
            self.arena = .allocate(capacity: arena)
            self.cells = .allocate(capacity: cells)
            self.row = .allocate(capacity: row)
        }

        deinit {
            window.deallocate()
            arena.deallocate()
            cells.deallocate()
            row.deallocate()
        }

        func buffers() -> hypertabular_buffers {
            var buffers = hypertabular_buffers()
            buffers.window = window.baseAddress
            buffers.window_cap = UInt(window.count)
            buffers.arena = arena.baseAddress
            buffers.arena_cap = UInt(arena.count)
            buffers.cells = cells.baseAddress
            buffers.cells_cap = UInt(cells.count)
            buffers.row = row.baseAddress
            buffers.row_cap = UInt(row.count)
            return buffers
        }

        /// Makes `call` until it stops asking for room, growing the buffer it names each time
        /// — with what the buffer held kept, which is what lets the core go on from where it
        /// stopped. Returns the code it ended on and what it reported.
        ///
        /// With `rowFollowsCells`, the row's slots are grown with the cell table, never to
        /// fewer: a sheet's header row is held in the slots as well as named in the cells,
        /// and a sheet with no plan yet has a slot for every name.
        func drive(
            _ call: Call, _ state: UnsafeMutableRawBufferPointer, _ container: UnsafeMutableRawBufferPointer,
            tables: Workbook?, columns: Columns? = nil, rowFollowsCells: Bool = false
        ) -> (code: Int32, filled: hypertabular_filled) {
            let input = Workbook.address(container)
            let length = UInt(container.count)
            while true {
                var filled = hypertabular_filled()
                var buffers = self.buffers()
                tables?.tables(&buffers)
                let code: Int32 =
                    switch call {
                    case .sheets: hypertabular_workbook_sheets(state.baseAddress, input, length, &buffers, &filled)
                    case .strings: hypertabular_workbook_strings(state.baseAddress, input, length, &buffers, &filled)
                    case .styles: hypertabular_workbook_styles(state.baseAddress, input, length, &buffers, &filled)
                    case .header: hypertabular_workbook_header(state.baseAddress, input, length, &buffers, &filled)
                    case .fill:
                        hypertabular_workbook_fill(
                            state.baseAddress, input, length, columns!.specs, columns!.buffers,
                            UInt(columns!.plan.count), UInt(columns!.batchRows), &buffers, &filled)
                    }
                switch code {
                case HYPERTABULAR_ERR_WINDOW:
                    window = window.grown(toHold: filled.needed)
                    Workbook.grown.window += 1
                case HYPERTABULAR_ERR_ARENA:
                    arena = arena.grown(toHold: filled.needed)
                    Workbook.grown.arena += 1
                case HYPERTABULAR_ERR_CELLS:
                    cells = cells.grown(toHold: filled.needed)
                    Workbook.grown.cells += 1
                    if rowFollowsCells && row.count < cells.count {
                        row = row.grown(toHold: UInt64(cells.count))
                    }
                default:
                    return (code, filled)
                }
            }
        }
    }
}

/// A forward-only read of one sheet of a ``Workbook``, a batch at a time, through a plan —
/// into the same ``Batch`` delimited text is read into.
///
/// A sheet opened without a plan has read its header, so that the plan can be built from
/// the names it declares, and is bound to it with ``bind(_:)`` — once, before the first read:
///
/// ```swift
/// let sheet = try book.sheet(named: "Orders")
/// let header = sheet.header!
/// try sheet.bind([.i32(header.ordinal(of: "id")), .text(header.ordinal(of: "name"))])
/// try sheet.forEachRow { row in print(row.line, row.string(1) ?? "") }
/// ```
///
/// A sheet has its own buffers and its own copy of the workbook's read state, so several
/// can be read at once; each keeps its workbook alive. A typed workbook cell is converted
/// directly by its door — a stored `42.0` never passes through text to become an `Int32` —
/// and a text cell goes through the door as delimited text would. Not thread-safe.
public final class Sheet {
    private let book: Workbook
    /// The options the sheet is read with.
    public let options: SheetOptions
    // The plan and the batch it is read into, once one is bound.
    private var columnSet: Columns?
    private var batch: Batch?
    private let state: UnsafeMutableRawBufferPointer
    private let scratch: Workbook.Scratch
    /// Cell-table entries one row takes: one per plan column, and one more.
    private var perRow = 0
    /// A failure met with rows before it: those went out first, and this is next.
    private var pending: TabularError?
    private var failure: TabularError?

    /// The header row's names — a typed cell said the way the text door says it — or `nil`
    /// when the options declare no header. A header with no names is a sheet with no rows.
    /// Read when the sheet is opened, before any plan, so that a plan can be built from it.
    public private(set) var header: Header?

    init(_ book: Workbook, _ info: SheetInfo, options: SheetOptions, plan: [Column]?) throws {
        precondition(options.batchRows > 0, "batchRows must be positive; got \(options.batchRows)")
        self.book = book
        self.options = options
        // A copy of the opened state, which the core allows, so that this sheet's read is
        // its own.
        state = .allocate(byteCount: book.state.count, alignment: 8)
        state.copyMemory(from: UnsafeRawBufferPointer(book.state))
        // The cell table and the row's slots are sized by the plan when one is bound; until
        // then, by the header.
        scratch =
            Workbook.stingy
            ? Workbook.Scratch(window: 1, arena: 1, cells: 1, row: 0)
            : Workbook.Scratch(window: 0, arena: 4096, cells: 64, row: 0)
        if let plan {
            try bind(plan)
        }

        var filled = hypertabular_filled()
        let container = book.containerBytes
        let code = info.part.withUnsafeBufferPointer { part in
            hypertabular_workbook_sheet(
                state.baseAddress, Workbook.address(container), UInt(container.count), part.baseAddress,
                UInt(part.count), info.index, options.hasHeader ? 1 : 0, options.skipEmptyRows ? 1 : 0, &filled)
        }
        _ = try Workbook.settled((code, filled))
        if options.hasHeader {
            try readHeader()
        }
    }

    deinit {
        state.deallocate()
    }

    private func readHeader() throws {
        // The header row's cells are kept in the row's slots as well as named — the slots
        // are what a row the sheet repeats (an ODS `number-rows-repeated`) is delivered
        // again from. A plan says how many slots it reads; without one, there is a slot for
        // every name the cell table can hold, the two grow together, and binding a plan later
        // keeps what they hold.
        let unbound = columnSet == nil
        if unbound && scratch.row.count < scratch.cells.count {
            scratch.row = scratch.row.grown(toHold: UInt64(scratch.cells.count))
        }
        let filled = try Workbook.settled(
            scratch.drive(.header, state, book.containerBytes, tables: book, rowFollowsCells: unbound))
        header = Header(
            bytes: (0..<Int(filled.rows)).map { index in
                let name = scratch.cells[index]
                let length = Int(name.len & ~HYPERTABULAR_SPAN_FLAG)
                let from = name.len & HYPERTABULAR_SPAN_FLAG != 0 ? scratch.arena : book.strings
                return Array(from[Int(name.offset)..<Int(name.offset) + length])
            })
    }

    /// Declares the plan a sheet opened without one reads through — once, before the first
    /// read: ``DelimitedReader/bind(_:)``, for a sheet.
    ///
    /// - Throws: ``PlanError/alreadyBound`` when the sheet has a plan, whether it was opened
    ///   with one or bound one already; the sheet goes on with the plan it has.
    public func bind(_ plan: [Column]) throws {
        guard columnSet == nil else {
            throw PlanError.alreadyBound
        }
        let columns = Columns(plan: plan, batchRows: options.batchRows)
        perRow = plan.count + 1
        // Grown, never replaced: the slots may hold a header row still to be repeated. The
        // stingy test pass leaves the cell table at its one entry, for the core to ask.
        let cells = options.batchRows * perRow
        if !Workbook.stingy && scratch.cells.count < cells {
            scratch.cells = scratch.cells.grown(toHold: UInt64(cells))
        }
        if scratch.row.count < columns.width {
            scratch.row = scratch.row.grown(toHold: UInt64(columns.width))
        }
        columnSet = columns
        batch = Batch(columns, workbook: true)
    }

    /// Whether a plan has been bound: always, for a sheet opened with one.
    public var isBound: Bool { columnSet != nil }

    /// The plan the sheet is read through: column `i` of every batch is `plan[i]`. Empty
    /// until one is bound.
    public var plan: [Column] { columnSet?.plan ?? [] }

    /// Reads the next batch: up to the options' `batchRows` rows, or `nil` when the sheet
    /// has no more. The batch is valid until the next read.
    ///
    /// - Throws: ``PlanError/unbound`` while the sheet has no plan, and ``TabularError``
    ///   when the sheet is structurally broken — after every intact row before the break has
    ///   been delivered, and again on every later call.
    public func read() throws -> Batch? {
        guard let columnSet, let batch else {
            throw PlanError.unbound
        }
        batch.clear()
        if let pending {
            failure = pending
            self.pending = nil
        }
        if let failure {
            throw failure
        }
        let (code, filled) = scratch.drive(.fill, state, book.containerBytes, tables: book, columns: columnSet)
        let rows = Int(filled.rows)
        if code != HYPERTABULAR_OK {
            guard code == HYPERTABULAR_ERR_STRUCTURE else {
                preconditionFailure(Workbook.contractViolation)
            }
            let broken = TabularError(filled.failure)
            if rows == 0 {
                failure = broken
                throw broken
            }
            pending = broken
        }
        guard rows > 0 else {
            return nil
        }
        return batch.fill(
            rows: rows, cells: scratch.cells, perRow: perRow, base: UnsafeRawPointer(book.strings.baseAddress),
            arena: scratch.arena)
    }

    /// Reads every row left, batch by batch, handing each to `body`:
    /// ``DelimitedReader/forEachRow(_:)``, for a sheet. A ``Row`` is valid only while `body`
    /// has it.
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
