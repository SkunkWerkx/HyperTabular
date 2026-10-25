import HyperTabularCore

/// A plan and everything the core casts it into: the plan as the core reads it, and every
/// column's values and verdicts for one batch — allocated once, never moved, and reused for
/// every batch. A ``DelimitedReader`` and a ``Sheet`` each own one.
final class Columns {
    let plan: [Column]
    let batchRows: Int
    /// The widest ordinal the plan reads, plus one.
    let width: Int
    /// The place in the plan of the column that reaches furthest (the first, if several do).
    let widest: Int

    // Everything the core is handed, allocated once and never moved: it is given addresses.
    let specs: UnsafeMutablePointer<hypertabular_column_spec>
    let buffers: UnsafeMutablePointer<hypertabular_column_buffer>
    let valueBase: [UnsafeMutableRawPointer]
    let verdictBase: [UnsafeMutablePointer<CellVerdict>]

    /// The widest ordinal a plan may name: 2²⁰ − 1. No workbook format has more columns
    /// (XLSX and ODS stop at 16,384, Numbers at 1,000), and at the cap one row of cell table
    /// is at most 8 MiB, so a headerless plan cannot ask for ~16 GiB of it.
    static let maxOrdinal = (1 << 20) - 1

    /// - Throws: ``PlanError/outOfReach(column:)`` naming the first column past
    ///   ``maxOrdinal``.
    init(plan: [Column], batchRows: Int) throws {
        precondition(batchRows > 0, "batchRows must be positive; got \(batchRows)")
        var furthest = -1
        var reaching = 0
        for (index, column) in plan.enumerated() {
            guard column.ordinal <= Self.maxOrdinal else { throw PlanError.outOfReach(column: index) }
            if column.ordinal > furthest { (furthest, reaching) = (column.ordinal, index) }
        }
        widest = reaching
        self.plan = plan
        self.batchRows = batchRows
        specs = .allocate(capacity: plan.count)
        buffers = .allocate(capacity: plan.count)
        var values = [UnsafeMutableRawPointer]()
        var verdicts = [UnsafeMutablePointer<CellVerdict>]()
        for (index, column) in plan.enumerated() {
            // 16-aligned, the widest any value is, so every door's values sit as Swift lays
            // that type out and can be handed back as a buffer of it.
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
        width = furthest + 1
    }

    deinit {
        specs.deallocate()
        buffers.deallocate()
        for value in valueBase { value.deallocate() }
        for verdict in verdictBase { verdict.deallocate() }
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
