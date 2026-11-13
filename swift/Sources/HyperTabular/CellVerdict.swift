import HyperTabularCore

/// One cell's verdict as the core writes it into a column's verdict array: HyperCast's
/// reason code, with none for a cell that cast and its own for a constraint broken, and the
/// offending span within the cell's own text. ``Batch/verdicts(_:)`` hands a column's verdicts out as a buffer of
/// these, for code that wants to scan a batch without a ``Verdict`` per cell.
public struct CellVerdict: Equatable, Sendable {
    // The core's own 12 bytes, so that a verdict array is a buffer of these as it stands.
    let raw: HyperTabularCellVerdict

    init(_ raw: HyperTabularCellVerdict) {
        self.raw = raw
    }

    /// `true` when the cell cast and, in a column that declares constraints, kept them.
    public var isOk: Bool { raw.reason == 0 }

    /// `true` when the cell cast but broke a constraint its column declares: no fault, and its
    /// keyword is what ``Batch/checked(_:row:as:)`` names.
    public var isViolation: Bool { raw.reason == UInt32(HyperTabularCellVerdict_VIOLATION) }

    /// Why the cell did not cast — `nil` when it did, a violation's included.
    public var reason: CastFailure? { fault?.reason }

    /// Byte offset of the offending span within the cell's text.
    public var offset: Int { Int(raw.offset) }

    /// Byte length of the offending span.
    public var length: Int { Int(raw.len) }

    /// The verdict as HyperCast's `Fault` — `nil` for a cell that cast, a violation's included.
    public var fault: Fault? {
        isOk || isViolation
            ? nil : Interop.fault(code: Int32(truncatingIfNeeded: raw.reason), offset: raw.offset, length: raw.len)
    }

    /// Two verdicts are equal when they say the same thing of the same span.
    public static func == (left: CellVerdict, right: CellVerdict) -> Bool {
        left.raw.reason == right.raw.reason && left.raw.offset == right.raw.offset
            && left.raw.len == right.raw.len
    }
}
