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

    /// Reads the file at `path` into memory and opens it.
    ///
    /// - Throws: Foundation's error when the file cannot be read, and ``TabularError`` when
    ///   it is not a workbook this reader can read.
    public convenience init(contentsOfFile path: String) throws {
        try self.init(bytes: try Data(contentsOf: URL(fileURLWithPath: path)))
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
            dateSystem = opened.epoch == 2 ? .y1904 : .y1900

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
        guard sheets.indices.contains(index) else {
            throw NoSuchSheet(which: "at index \(index)")
        }
        return try Sheet(self, sheets[index], options: options, plan: plan)
    }

    /// Starts a read of the first sheet named `name` through `plan`, as
    /// ``sheet(_:options:plan:)`` does.
    ///
    /// - Throws: ``NoSuchSheet`` when the workbook has no sheet by that name, and
    ///   ``TabularError`` when the sheet, or its header row, is structurally broken.
    public func sheet(named name: String, options: SheetOptions = .default, plan: [Column]) throws -> Sheet {
        guard let info = sheets.first(where: { $0.name == name }) else {
            throw NoSuchSheet(which: "named \"\(name)\"")
        }
        return try Sheet(self, info, options: options, plan: plan)
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
        let row: UnsafeMutableBufferPointer<hypertabular_slot>

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
        func drive(
            _ call: Call, _ state: UnsafeMutableRawBufferPointer, _ container: UnsafeMutableRawBufferPointer,
            tables: Workbook?, columns: Columns? = nil
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
/// A sheet has its own buffers and its own copy of the workbook's read state, so several
/// can be read at once; each keeps its workbook alive. A typed workbook cell is converted
/// directly by its door — a stored `42.0` never passes through text to become an `Int32` —
/// and a text cell goes through the door as delimited text would. Not thread-safe.
public final class Sheet {
    private let book: Workbook
    /// The options the sheet is read with.
    public let options: SheetOptions
    private let columnSet: Columns
    private let batch: Batch
    private let state: UnsafeMutableRawBufferPointer
    private let scratch: Workbook.Scratch
    /// Cell-table entries one row takes: one per plan column, and one more.
    private let perRow: Int
    /// A failure met with rows before it: those went out first, and this is next.
    private var pending: TabularError?
    private var failure: TabularError?

    /// The header row's names — a typed cell said the way the text door says it — or `nil`
    /// when the options declare no header. A header with no names is a sheet with no rows.
    public private(set) var header: [String]?

    init(_ book: Workbook, _ info: SheetInfo, options: SheetOptions, plan: [Column]) throws {
        precondition(options.batchRows > 0, "batchRows must be positive; got \(options.batchRows)")
        self.book = book
        self.options = options
        columnSet = Columns(plan: plan, batchRows: options.batchRows)
        batch = Batch(columnSet, workbook: true)
        perRow = plan.count + 1
        // A copy of the opened state, which the core allows, so that this sheet's read is
        // its own.
        state = .allocate(byteCount: book.state.count, alignment: 8)
        state.copyMemory(from: UnsafeRawBufferPointer(book.state))
        scratch =
            Workbook.stingy
            ? Workbook.Scratch(window: 1, arena: 1, cells: 1, row: columnSet.width)
            : Workbook.Scratch(window: 0, arena: 4096, cells: options.batchRows * perRow, row: columnSet.width)

        var filled = hypertabular_filled()
        let container = book.containerBytes
        let code = info.part.withUnsafeBufferPointer { part in
            hypertabular_workbook_sheet(
                state.baseAddress, Workbook.address(container), UInt(container.count), part.baseAddress,
                UInt(part.count), info.index, options.hasHeader ? 1 : 0, options.skipEmptyRows ? 1 : 0, &filled)
        }
        _ = try Workbook.settled((code, filled))
        if options.hasHeader {
            let filled = try Workbook.settled(scratch.drive(.header, state, container, tables: book))
            header = (0..<Int(filled.rows)).map { index in
                let name = scratch.cells[index]
                let length = Int(name.len & ~HYPERTABULAR_SPAN_FLAG)
                let from = name.len & HYPERTABULAR_SPAN_FLAG != 0 ? scratch.arena : book.strings
                return String(
                    decoding: UnsafeBufferPointer(rebasing: from[Int(name.offset)..<Int(name.offset) + length]),
                    as: UTF8.self)
            }
        }
    }

    deinit {
        state.deallocate()
    }

    /// The plan the sheet is read through: column `i` of every batch is `plan[i]`.
    public var plan: [Column] { columnSet.plan }

    /// Reads the next batch: up to the options' `batchRows` rows, or `nil` when the sheet
    /// has no more. The batch is valid until the next read.
    ///
    /// - Throws: ``TabularError`` when the sheet is structurally broken — after every intact
    ///   row before the break has been delivered, and again on every later call.
    public func read() throws -> Batch? {
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
}
