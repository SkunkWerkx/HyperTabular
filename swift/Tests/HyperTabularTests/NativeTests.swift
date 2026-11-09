import Foundation
import HyperTabularCore
import XCTest

@testable import HyperTabular

/// What no read reaches: the 1.0 exports the ABI promises and no read path takes, and the shapes the
/// core reads.
final class NativeTests: XCTestCase {
    private static let capacity = 64

    /// Memory a 1.0 fill writes: each column's values and verdicts.
    private final class Columns10 {
        let values: [UnsafeMutablePointer<UInt64>]
        let verdicts: [UnsafeMutablePointer<HyperTabularCellVerdict>]
        var specs: [HyperTabularColumnSpec]
        var buffers: [HyperTabularColumnBuffer]

        init(_ plan: [(ordinal: UInt32, door: Door, format: HyperTabularRawNumFormat)]) {
            values = plan.map { _ in .allocate(capacity: NativeTests.capacity * 2) }
            verdicts = plan.map { _ in .allocate(capacity: NativeTests.capacity) }
            specs = plan.map { column in
                var spec = HyperTabularColumnSpec()
                spec.ordinal = column.ordinal
                spec.door = column.door.rawValue
                spec.format = column.format
                return spec
            }
            buffers = zip(values, verdicts).map { value, verdict in
                var buffer = HyperTabularColumnBuffer()
                buffer.values = UnsafeMutableRawPointer(value)
                buffer.verdicts = verdict
                return buffer
            }
        }

        deinit {
            values.forEach { $0.deallocate() }
            verdicts.forEach { $0.deallocate() }
        }

        func verdict(_ column: Int, _ row: Int) -> CellVerdict { CellVerdict(verdicts[column][row]) }

        func value<T>(_ column: Int, _ row: Int, as type: T.Type) -> T {
            UnsafeRawPointer(values[column]).load(fromByteOffset: row * MemoryLayout<T>.stride, as: T.self)
        }
    }

    /// The word a format holds its flags in, after its size: the header keeps the layout opaque.
    private static func put(_ flags: UInt32, into raw: inout HyperTabularRawNumFormat) {
        withUnsafeMutableBytes(of: &raw) { $0.storeBytes(of: flags, toByteOffset: 4, as: UInt32.self) }
    }

    private static func raw(_ format: NumFormat, flags: UInt32? = nil) -> HyperTabularRawNumFormat {
        var raw = HyperTabularRawNumFormat()
        var given = Interop.rawFormat(format)
        withUnsafeBytes(of: &given) { source in
            withUnsafeMutableBytes(of: &raw) { $0.copyMemory(from: source) }
        }
        if let flags { put(flags, into: &raw) }
        return raw
    }

    /// A whole input read through the 1.0 calls.
    private func fill10(_ input: [UInt8], separator: UInt8, _ columns: Columns10) -> HyperTabularFilled {
        let state = UnsafeMutablePointer<HyperTabularDelimitedState>.allocate(capacity: 1)
        defer { state.deallocate() }
        XCTAssertEqual(FirstSurface.delimitedInit(state, separator: separator), HYPERTABULAR_OK)
        let arena = UnsafeMutablePointer<UInt8>.allocate(capacity: 4096)
        defer { arena.deallocate() }
        let cells = UnsafeMutablePointer<HyperTabularSpan>.allocate(capacity: Self.capacity)
        defer { cells.deallocate() }
        var memory = HyperTabularBuffers()
        memory.arena = arena
        memory.arena_cap = 4096
        memory.cells = cells
        memory.cells_cap = UInt(Self.capacity)
        var filled = HyperTabularFilled()
        let code = FirstSurface.delimitedFill(
            state, input, input.count, last: true, specs: columns.specs, buffers: columns.buffers,
            count: columns.specs.count, maxRows: Self.capacity, memory: &memory, filled: &filled)
        XCTAssertEqual(code, HYPERTABULAR_OK)
        return filled
    }

    /// A policy is the one thing 1.0 cannot be told: its fill reads the low 8 flag bits, so every format
    /// is Western there, and a read of a Western format says the same of every cell.
    func testTheDelimited10ExportsReadWhatAReadDoes() throws {
        let input = Array("1;a b;2.5\n12x;c;x\n1,23,456;d;4\n".utf8)
        let invariant = Self.raw(.invariant)
        let columns = Columns10([
            (0, .i32, invariant), (1, .text, HyperTabularRawNumFormat()), (2, .f64, invariant),
        ])
        XCTAssertEqual(fill10(input, separator: UInt8(ascii: ";"), columns).rows, 3)

        let reader = try DelimitedReader(
            bytes: input, dialect: Dialect(separator: ";", hasHeader: false, notes: false),
            plan: [.i32(0), .text(1), .f64(2)])
        let batch = try XCTUnwrap(reader.read())
        XCTAssertEqual(batch.rows, 3)
        XCTAssertEqual(batch.verdict(0, row: 2).reason, .malformed)
        for row in 0..<3 {
            for column in 0..<3 {
                XCTAssertEqual(columns.verdict(column, row), batch.verdict(column, row: row), "\(column), \(row)")
            }
            XCTAssertEqual(columns.value(0, row, as: Int32.self), batch.values(0, as: Int32.self)[row])
            XCTAssertEqual(columns.value(2, row, as: Double.self), batch.values(2, as: Double.self)[row])
            let span = columns.value(1, row, as: HyperTabularSpan.self)
            XCTAssertEqual(
                Array(input[Int(span.offset)..<Int(span.offset) + Int(span.len)]),
                Array(try XCTUnwrap(batch.text(1, row: row))))
        }
    }

    /// Every word, the retired one and Indian's included, reads Western through 1.0: 1,234,567 taken and
    /// 1,23,456 refused.
    func testADelimited10FillReadsEveryWordWestern() {
        for flags: UInt32 in [351, 607, 863] {
            let columns = Columns10([(0, .i32, Self.raw(.invariant, flags: flags))])
            let filled = fill10(Array("\"1,234,567\"\n\"1,23,456\"\n".utf8), separator: UInt8(ascii: ","), columns)
            XCTAssertEqual(filled.rows, 2, "\(flags)")
            XCTAssertTrue(columns.verdict(0, 0).isOk, "\(flags)")
            XCTAssertEqual(columns.value(0, 0, as: Int32.self), 1_234_567, "\(flags)")
            XCTAssertEqual(columns.verdict(0, 1).reason, .malformed, "\(flags)")
        }
    }

    /// A size-tagged fill told the retired word, past the binding's refusal, is a contract error before
    /// any cell; told Western or Indian, it reads.
    func testAStaleWordPastTheBindingIsTheCoresContractError() {
        let table: [(UInt32, Int32, UInt64)] = [
            (351, HYPERTABULAR_ERR_CONTRACT, 0), (863, HYPERTABULAR_ERR_CONTRACT, 0), (95, HYPERTABULAR_OK, 1),
            (607, HYPERTABULAR_OK, 1),
        ]
        for (flags, code, rows) in table {
            let state = UnsafeMutablePointer<HyperTabularDelimitedState>.allocate(capacity: 1)
            defer { state.deallocate() }
            var options = HyperTabularRawDelimitedOptions()
            options.size = UInt32(HyperTabularRawDelimitedOptions_SIZE)
            options.separator = UInt8(ascii: ",")
            options.quoting = 1
            options.skip_blank_lines = 1
            XCTAssertEqual(hypertabular_delimited_init_with(state, &options), HYPERTABULAR_OK)
            var spec = Column.i32(0).spec
            Self.put(flags, into: &spec.format)
            let values = UnsafeMutablePointer<Int32>.allocate(capacity: Self.capacity)
            defer { values.deallocate() }
            let verdicts = UnsafeMutablePointer<HyperTabularCellVerdict>.allocate(capacity: Self.capacity)
            defer { verdicts.deallocate() }
            var column = HyperTabularColumnBufferWith()
            column.size = UInt32(MemoryLayout<HyperTabularColumnBufferWith>.size)
            column.values = UnsafeMutableRawPointer(values)
            column.verdicts = verdicts
            let arena = UnsafeMutablePointer<UInt8>.allocate(capacity: 4096)
            defer { arena.deallocate() }
            let cells = UnsafeMutablePointer<HyperTabularSpan>.allocate(capacity: Self.capacity)
            defer { cells.deallocate() }
            var memory = HyperTabularBuffersWith()
            memory.size = UInt32(MemoryLayout<HyperTabularBuffersWith>.size)
            memory.arena = arena
            memory.arena_cap = 4096
            memory.cells = cells
            memory.cells_cap = UInt(Self.capacity)
            var filled = HyperTabularFilled()
            let input = Array("\"1,234\"\n".utf8)
            XCTAssertEqual(
                hypertabular_delimited_fill_with(
                    state, input, UInt(input.count), 1, &spec, &column, 1, UInt(Self.capacity), &memory, &filled), code,
                "\(flags)")
            if code == HYPERTABULAR_OK {
                XCTAssertEqual(filled.rows, rows, "\(flags)")
                XCTAssertEqual(values[0], 1234, "\(flags)")
            }
        }
    }

    func testTheWorkbook10ExportsReadWhatAReadDoes() throws {
        let book = try Workbook(contentsOfFile: try Support.package("basic.xlsx"))
        let info = book.sheets[0]
        let state = UnsafeMutableRawBufferPointer.allocate(byteCount: book.state.count, alignment: 8)
        defer { state.deallocate() }
        state.copyMemory(from: UnsafeRawBufferPointer(book.state))
        var filled = HyperTabularFilled()
        XCTAssertEqual(
            FirstSurface.workbookSheet(
                state, book.containerBytes, part: info.part, index: info.index, hasHeader: true, skipEmptyRows: true,
                filled: &filled), HYPERTABULAR_OK)

        let scratch = Workbook.Scratch(window: 1 << 20, arena: 1 << 20, cells: 4096, row: 64)
        XCTAssertEqual(scratch.drive(.header, state, book.containerBytes, tables: book).code, HYPERTABULAR_OK)
        var memory = scratch.buffers()
        book.tables(&memory)
        let columns = Columns10([(0, .text, HyperTabularRawNumFormat()), (1, .text, HyperTabularRawNumFormat())])
        XCTAssertEqual(
            FirstSurface.workbookFill(
                state, book.containerBytes, specs: columns.specs, buffers: columns.buffers, count: 2, maxRows: 16,
                memory: &memory, filled: &filled), HYPERTABULAR_OK)

        let sheet = try book.sheet(0, options: SheetOptions(batchRows: 16, notes: false), plan: [.text(0), .text(1)])
        let batch = try XCTUnwrap(sheet.read())
        XCTAssertGreaterThan(filled.rows, 0)
        XCTAssertEqual(Int(filled.rows), batch.rows)
        for column in 0..<2 {
            for row in 0..<batch.rows {
                XCTAssertEqual(columns.verdict(column, row), batch.verdict(column, row: row))
                guard let text = batch.text(column, row: row) else { continue }
                let span = columns.value(column, row, as: HyperTabularSpan.self)
                let length = Int(span.len & ~spanFlag)
                let from = span.len & spanFlag != 0 ? scratch.arena : book.strings
                XCTAssertEqual(
                    Array(from[Int(span.offset)..<Int(span.offset) + length]), Array(text), "\(column), \(row)")
            }
        }
    }

    /// What the header says of each shape's size, held against the module the header makes, by the
    /// header's name for it.
    func testEveryShapeIsTheSizeTheHeaderSays() throws {
        guard let corpus = CorpusTests.corpusDirectory else {
            throw XCTSkip("a WASI sandbox has no view of the source tree, so of the header")
        }
        let path = corpus.deletingLastPathComponent().appendingPathComponent("include/hypertabular.h").path
        guard let header = try? String(contentsOfFile: path, encoding: .utf8) else {
            throw XCTSkip("the header is not beside the corpus on this host")
        }
        func said(_ name: String) throws -> Int {
            let prefix = "#define HyperTabular\(name)_SIZE "
            let line = try XCTUnwrap(header.split(separator: "\n").first { $0.hasPrefix(prefix) }, name)
            return try XCTUnwrap(Int(line.dropFirst(prefix.count)), name)
        }
        XCTAssertEqual(try said("RawDelimitedOptions"), MemoryLayout<HyperTabularRawDelimitedOptions>.size)
        XCTAssertEqual(try said("RawDialect"), MemoryLayout<HyperTabularRawDialect>.size)
        XCTAssertEqual(try said("RawTranscoded"), MemoryLayout<HyperTabularRawTranscoded>.size)
        XCTAssertEqual(try said("Span"), MemoryLayout<HyperTabularSpan>.size)
        XCTAssertEqual(try said("Filled"), MemoryLayout<HyperTabularFilled>.size)
        XCTAssertEqual(try said("Failure"), MemoryLayout<HyperTabularFailure>.size)
        XCTAssertEqual(try said("ColumnSpec"), MemoryLayout<HyperTabularColumnSpec>.size)
        XCTAssertEqual(try said("CellNote"), MemoryLayout<HyperTabularCellNote>.size)
        XCTAssertEqual(try said("RawProblem"), MemoryLayout<HyperTabularRawProblem>.size)
        XCTAssertEqual(try said("Slot"), MemoryLayout<HyperTabularSlot>.size)
        XCTAssertEqual(try said("Opened"), MemoryLayout<HyperTabularOpened>.size)
        XCTAssertEqual(try said("RawNumFormat"), MemoryLayout<HyperTabularRawNumFormat>.size)
        XCTAssertEqual(try said("RawDateOptions"), MemoryLayout<HyperTabularRawDateOptions>.size)
        XCTAssertEqual(try said("RawSheetOptions"), MemoryLayout<HyperTabularRawSheetOptions>.size)
        XCTAssertEqual(try said("RawRange"), MemoryLayout<HyperTabularRawRange>.size)
        XCTAssertEqual(try said("RawSuggestion"), MemoryLayout<HyperTabularRawSuggestion>.size)
        XCTAssertEqual(try said("CellVerdict"), MemoryLayout<CellVerdict>.size)

        // Those that hold pointers are a sizeof in the header: the core's asserts say what they come to.
        let word = MemoryLayout<UInt>.size
        XCTAssertEqual(MemoryLayout<HyperTabularColumnSpecWith>.size, 120 + 4 * word)
        XCTAssertEqual(MemoryLayout<HyperTabularColumnBufferWith>.size, 4 * word)
        XCTAssertEqual(MemoryLayout<HyperTabularBuffersWith>.size, 18 * word)
        XCTAssertEqual(MemoryLayout<HyperTabularColumnSpecWith>.offset(of: \.date), 108)
        XCTAssertEqual(MemoryLayout<HyperTabularColumnSpecWith>.offset(of: \.months), 120)
        XCTAssertEqual(MemoryLayout<HyperTabularColumnSpecWith>.offset(of: \.tokens), 120 + 2 * word)
        XCTAssertEqual(MemoryLayout<HyperTabularColumnBufferWith>.offset(of: \.notes), 3 * word)
        XCTAssertEqual(MemoryLayout<HyperTabularBuffersWith>.offset(of: \.tokens), 15 * word)
        XCTAssertEqual(MemoryLayout<HyperTabularBuffersWith>.offset(of: \.row_widths), 17 * word)
        XCTAssertEqual(MemoryLayout<HyperTabularRawProblem>.offset(of: \.row), 0)
        XCTAssertEqual(MemoryLayout<HyperTabularRawSheetOptions>.offset(of: \.last_column), 32)
    }
}
