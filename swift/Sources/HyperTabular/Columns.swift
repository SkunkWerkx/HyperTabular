import HyperTabularCore

/// The core's own word on a column, asked before any input is read: a fill of no rows judges its
/// door, what the door declares, its format, date options, vocabulary and tokens, and writes
/// nothing. It answers on a state of its own, so a sheet's plan is asked the same way. HyperCast's
/// `NumFormat` refuses a bad declaration when it is made, and this is what a plan holding one that
/// got past that (a stale word a decoder rebuilt) meets.
final class Judge {
    private let state: UnsafeMutablePointer<HyperTabularDelimitedState>
    // Room for one value of any door and its verdict, though the core writes neither.
    private let value: UnsafeMutableRawPointer
    private let verdict: UnsafeMutablePointer<HyperTabularCellVerdict>

    init() {
        state = .allocate(capacity: 1)
        value = .allocate(byteCount: 16, alignment: 16)
        verdict = .allocate(capacity: 1)
        var options = HyperTabularRawDelimitedOptions()
        options.size = UInt32(HyperTabularRawDelimitedOptions_SIZE)
        options.separator = UInt8(ascii: ",")
        options.quoting = 1
        options.skip_blank_lines = 1
        guard hypertabular_delimited_init_with(state, &options) == HYPERTABULAR_OK else {
            preconditionFailure(Workbook.contractViolation)
        }
    }

    deinit {
        state.deallocate()
        value.deallocate()
        verdict.deallocate()
    }

    /// Whether the core would read a column of this spec.
    func accepts(_ spec: HyperTabularColumnSpecWith) -> Bool {
        var spec = spec
        var buffer = HyperTabularColumnBufferWith()
        buffer.size = columnBufferSize
        buffer.values = value
        buffer.verdicts = verdict
        var memory = HyperTabularBuffersWith()
        memory.size = buffersSize
        var filled = HyperTabularFilled()
        return hypertabular_delimited_fill_with(state, nil, 0, 0, &spec, &buffer, 1, 0, &memory, &filled)
            != HYPERTABULAR_ERR_CONTRACT
    }
}

/// The bytes each size-tagged shape is, which it carries in its first word. The header says these
/// with a `sizeof`, which Swift does not import.
let columnSpecSize = UInt32(MemoryLayout<HyperTabularColumnSpecWith>.size)
let columnBufferSize = UInt32(MemoryLayout<HyperTabularColumnBufferWith>.size)
let buffersSize = UInt32(MemoryLayout<HyperTabularBuffersWith>.size)

/// A plan and everything the core casts it into: the plan as the core reads it, and every
/// column's values, verdicts and, unless they are turned off, notes for one batch — allocated once,
/// never moved, and reused for every batch. A ``DelimitedReader`` and a ``Sheet`` each own one.
final class Columns {
    let plan: [Column]
    let batchRows: Int
    /// The widest ordinal the plan reads, plus one.
    let width: Int
    /// The place in the plan of the column that reaches furthest (the first, if several do).
    let widest: Int

    // Everything the core is handed, allocated once and never moved: it is given addresses.
    let specs: UnsafeMutablePointer<HyperTabularColumnSpecWith>
    let buffers: UnsafeMutablePointer<HyperTabularColumnBufferWith>
    let valueBase: [UnsafeMutableRawPointer]
    let verdictBase: [UnsafeMutablePointer<CellVerdict>]
    /// Each column's notes: `nil` with notes off.
    let noteBase: [UnsafeMutablePointer<HyperTabularCellNote>]?
    /// How many cells each row had, for a read that asks: `nil` when none does.
    let rowWidths: UnsafeMutablePointer<UInt32>?
    /// The tokens and month vocabularies the specs point at, held for as long as they do.
    private let lists: [UnsafeMutableBufferPointer<UInt8>]

    /// The widest ordinal a plan may name: 2²⁰ − 1. No workbook format has more columns
    /// (XLSX and ODS stop at 16,384, Numbers at 1,000), and at the cap one row of cell table
    /// is at most 8 MiB, so a headerless plan cannot ask for ~16 GiB of it.
    static let maxOrdinal = (1 << 20) - 1

    /// The core's answer on a read's sizes, ahead of everything the read allocates: the core
    /// decides how much a batch may ask for, and says what size to start an input buffer at.
    ///
    /// - Returns: The size to start the input buffer at.
    /// - Throws: ``BatchSizeError`` for what the core refuses.
    static func checkSizes(
        specs: [HyperTabularColumnSpecWith] = [], batchRows: Int, bufferBytes: Int,
        notes: Bool, widths: Bool, sheet: Bool
    ) throws -> Int {
        // The core takes an unsigned count, which a negative one isn't.
        precondition(batchRows >= 0, "batchRows must not be negative; got \(batchRows)")
        precondition(bufferBytes >= 0, "bufferBytes must not be negative; got \(bufferBytes)")
        var flags: UInt32 = 0
        if notes { flags |= UInt32(HyperTabularRawBatchCheck_NOTES) }
        if widths { flags |= UInt32(HyperTabularRawBatchCheck_WIDTHS) }
        if sheet { flags |= UInt32(HyperTabularRawBatchCheck_SHEET) }
        var found = HyperTabularRawBatchCheck()
        let code = specs.withUnsafeBufferPointer {
            hypertabular_batch_check(
                $0.baseAddress, UInt($0.count), UInt64(batchRows), UInt64(bufferBytes), flags, &found)
        }
        switch code {
        case HYPERTABULAR_OK:
            return Int(found.buffer_bytes)
        case HYPERTABULAR_ERR_BATCH:
            switch found.refused {
            case UInt32(HyperTabularRawBatchCheck_REFUSED_BATCH_ROWS_ZERO):
                throw BatchSizeError.zeroBatchRows
            case UInt32(HyperTabularRawBatchCheck_REFUSED_BUFFER_BYTES_ZERO):
                throw BatchSizeError.zeroBufferBytes
            case UInt32(HyperTabularRawBatchCheck_REFUSED_ROW_TOO_WIDE):
                throw BatchSizeError.rowTooWide(width: found.width, needed: found.needed, limit: found.limit)
            default:
                throw BatchSizeError.tooLarge(needed: found.needed, limit: found.limit)
            }
        default:
            preconditionFailure("the core refused a plan its judge took")
        }
    }

    /// What the core is told of a plan for its sizes alone. A plan that can't be honoured says so
    /// where it always has, so its size is judged without it.
    static func sizeSpecs(_ plan: [Column]?) -> [HyperTabularColumnSpecWith] {
        guard let plan, plan.allSatisfy({ $0.ordinal <= maxOrdinal }) else { return [] }
        return plan.map(\.spec)
    }

    /// The rows of a sampler's batch for a sampler wanting `wanted` rows: the core's answer, however
    /// many batches that takes.
    ///
    /// - Throws: ``BatchSizeError/rowTooWide(width:needed:limit:)``: only a row past the limit is refused.
    static func sampleRows(_ plan: [Column], wanted: Int, notes: Bool, widths: Bool, sheet: Bool) throws -> Int {
        let specs = sizeSpecs(plan)
        var flags: UInt32 = 0
        if notes { flags |= UInt32(HyperTabularRawBatchCheck_NOTES) }
        if widths { flags |= UInt32(HyperTabularRawBatchCheck_WIDTHS) }
        if sheet { flags |= UInt32(HyperTabularRawBatchCheck_SHEET) }
        var found = HyperTabularRawBatchCheck()
        _ = specs.withUnsafeBufferPointer {
            hypertabular_batch_check($0.baseAddress, UInt($0.count), UInt64(wanted), 1, flags, &found)
        }
        if found.sample_rows == 0 {
            _ = try checkSizes(
                specs: specs, batchRows: wanted, bufferBytes: 1, notes: notes, widths: widths, sheet: sheet)
        }
        return Int(found.sample_rows)
    }

    /// - Throws: ``PlanError/outOfReach(column:)`` naming the first column past
    ///   ``maxOrdinal``, and ``PlanError/refused(column:)`` naming the first the core will not read.
    init(
        plan: [Column], batchRows: Int, notes: Bool = true, rowWidths wantsWidths: Bool = false,
        sheet: Bool = false
    ) throws {
        var furthest = -1
        var reaching = 0
        var held = [UnsafeMutableBufferPointer<UInt8>]()
        var built = [HyperTabularColumnSpecWith]()
        do {
            let judge = Judge()
            for (index, column) in plan.enumerated() {
                guard column.ordinal <= Self.maxOrdinal else { throw PlanError.outOfReach(column: index) }
                if column.ordinal > furthest { (furthest, reaching) = (column.ordinal, index) }
                var spec = column.spec
                if column.dateOptions != .none {
                    let options = column.dateOptions
                    spec.date.size = UInt32(Interop.rawDateOptionsSize)
                    spec.date.flags = options.flags.rawValue
                    spec.date.pivot = UInt32(options.pivot ?? 0)
                    if let months = options.months {
                        let list = Self.hold(months.packed)
                        held.append(list)
                        spec.months = UnsafePointer(list.baseAddress)
                        spec.months_len = UInt(months.packed.count)
                    }
                }
                // An empty list has an address: null would inherit the dialect's or the sheet's.
                if let tokens = column.tokens {
                    let packed = packedTokens(tokens)
                    let list = Self.hold(packed)
                    held.append(list)
                    spec.tokens = UnsafePointer(list.baseAddress)
                    spec.tokens_len = UInt(packed.count)
                }
                guard judge.accepts(spec) else { throw PlanError.refused(column: index) }
                built.append(spec)
            }
        } catch {
            for list in held { list.deallocate() }
            throw error
        }
        do {
            _ = try Self.checkSizes(
                specs: built, batchRows: batchRows, bufferBytes: 1,
                notes: notes, widths: wantsWidths, sheet: sheet)
        } catch {
            for list in held { list.deallocate() }
            throw error
        }
        lists = held
        widest = reaching
        self.plan = plan
        self.batchRows = batchRows
        specs = .allocate(capacity: plan.count)
        buffers = .allocate(capacity: plan.count)
        rowWidths = wantsWidths ? .allocate(capacity: batchRows) : nil
        var values = [UnsafeMutableRawPointer]()
        var verdicts = [UnsafeMutablePointer<CellVerdict>]()
        var noted = [UnsafeMutablePointer<HyperTabularCellNote>]()
        for (index, column) in plan.enumerated() {
            // 16-aligned, the widest any value is, so every door's values sit as Swift lays
            // that type out and can be handed back as a buffer of it.
            let value = UnsafeMutableRawPointer.allocate(
                byteCount: batchRows * column.valueSize, alignment: 16)
            let verdict = UnsafeMutablePointer<CellVerdict>.allocate(capacity: batchRows)
            values.append(value)
            verdicts.append(verdict)
            (specs + index).initialize(to: built[index])
            var buffer = HyperTabularColumnBufferWith()
            buffer.size = columnBufferSize
            buffer.values = value
            buffer.verdicts = UnsafeMutableRawPointer(verdict).assumingMemoryBound(to: HyperTabularCellVerdict.self)
            if notes {
                let note = UnsafeMutablePointer<HyperTabularCellNote>.allocate(capacity: batchRows)
                noted.append(note)
                buffer.notes = note
            }
            (buffers + index).initialize(to: buffer)
        }
        valueBase = values
        verdictBase = verdicts
        noteBase = notes ? noted : nil
        width = furthest + 1
    }

    deinit {
        specs.deallocate()
        buffers.deallocate()
        rowWidths?.deallocate()
        for value in valueBase { value.deallocate() }
        for verdict in verdictBase { verdict.deallocate() }
        for note in noteBase ?? [] { note.deallocate() }
        for list in lists { list.deallocate() }
    }

    /// A copy of `list` at an address that is never null, an empty list's included.
    static func hold(_ list: [UInt8]) -> UnsafeMutableBufferPointer<UInt8> {
        let held = UnsafeMutableBufferPointer<UInt8>.allocate(capacity: max(list.count, 1))
        _ = held.initialize(from: list)
        return held
    }
}

extension UnsafeMutableBufferPointer {
    /// This buffer made at least `needed` long, what it held kept — which is what lets the
    /// core go on from where it stopped when a workbook call asks for room. The old buffer
    /// is released.
    func grown(toHold needed: UInt64) -> UnsafeMutableBufferPointer {
        let larger = UnsafeMutableBufferPointer.allocate(
            capacity: Swift.max(Int(clamping: Swift.min(needed, UInt64(Int32.max))), count + 1))
        if let base = baseAddress, let into = larger.baseAddress {
            UnsafeMutableRawPointer(into).copyMemory(from: base, byteCount: count * MemoryLayout<Element>.stride)
        }
        deallocate()
        return larger
    }
}
