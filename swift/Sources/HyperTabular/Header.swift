import Foundation

/// The names a source's header declares, in column order: what a plan's ordinals are
/// looked up by when the columns are known by name rather than by position.
///
/// A header is a collection of `String` — `header[0]`, `header.count`, `for name in header`,
/// a literal `["id", "name"]` to compare against — and each name is also kept as the bytes
/// the source held, so that a lookup is exact: byte for byte, case and spaces included, with
/// no Unicode normalization. ``ordinal(of:)-(String)`` refuses a name two columns have, since
/// a plan built from it would read the wrong one half the time; ``firstIndex(of:)-(String)``
/// keeps the first, for a caller who means that. A delimited header that is not valid UTF-8, in
/// whatever ``TextEncoding`` it was decoded from, is a ``TabularError`` when the reader opens.
///
/// ```swift
/// let reader = try DelimitedReader(contentsOfFile: "countries.csv", dialect: .csv)
/// let header = reader.header!
/// try reader.bind([.i32(header.ordinal(of: "M49 Code")), .text(header.ordinal(of: "ISO-alpha2 Code"))])
/// ```
public struct Header: RandomAccessCollection, Hashable, Sendable, CustomStringConvertible,
    ExpressibleByArrayLiteral
{
    private let names: [String]
    private let utf8: [[UInt8]]

    /// The names a source held, as its bytes.
    init(bytes: [[UInt8]]) {
        utf8 = bytes
        names = bytes.map { String(decoding: $0, as: UTF8.self) }
    }

    /// A header of these names — what a test comparing against one needs; a reader is what
    /// produces one otherwise.
    public init(_ names: [String]) {
        self.names = names
        utf8 = names.map { Array($0.utf8) }
    }

    /// A header of the names listed.
    public init(arrayLiteral names: String...) {
        self.init(names)
    }

    /// The first ordinal: `0`.
    public var startIndex: Int { 0 }

    /// One past the last ordinal: the number of names.
    public var endIndex: Int { names.count }

    /// The name of the column at `ordinal`.
    public subscript(ordinal: Int) -> String { names[ordinal] }

    /// The name of the column at `ordinal` as the bytes the source held.
    public func bytes(at ordinal: Int) -> [UInt8] { utf8[ordinal] }

    /// The names, as a list.
    public var description: String { names.description }

    /// The ordinal of the first column named `name`, or `nil` when no column is. Two columns of
    /// that name are not an error here: the first is the one asked for. The match is exact — the
    /// name's UTF-8 against the bytes the source held — so `"é"` precomposed does not find `"é"`
    /// decomposed, as `String`'s own `==` would.
    public func firstIndex(of name: String) -> Int? {
        firstIndex(of: name.utf8)
    }

    /// The ordinal of the first column whose name is exactly these bytes, or `nil` when no
    /// column's is.
    public func firstIndex<Name: Collection>(of name: Name) -> Int? where Name.Element == UInt8 {
        utf8.firstIndex { $0.elementsEqual(name) }
    }

    /// The ordinal of the one column named `name`, matched as `firstIndex(of:)` matches: what a
    /// plan built from names reads best with.
    ///
    /// - Throws: ``NoSuchColumn``, naming `name`, when no column is named that, and
    ///   ``AmbiguousColumn`` when two are: say which by position.
    public func ordinal(of name: String) throws -> Int {
        try ordinal(of: Array(name.utf8), named: name)
    }

    /// The ordinal of the one column whose name is exactly these bytes.
    ///
    /// - Throws: ``NoSuchColumn``, naming the bytes as text, when no column's name is them, and
    ///   ``AmbiguousColumn`` when two columns' are.
    public func ordinal<Name: Collection>(of name: Name) throws -> Int where Name.Element == UInt8 {
        let bytes = Array(name)
        return try ordinal(of: bytes, named: String(decoding: bytes, as: UTF8.self))
    }

    private func ordinal(of bytes: [UInt8], named name: String) throws -> Int {
        guard let first = utf8.firstIndex(where: { $0 == bytes }) else {
            throw NoSuchColumn(name: name)
        }
        if let second = utf8[(first + 1)...].firstIndex(where: { $0 == bytes }) {
            throw AmbiguousColumn(name: name, first: first, second: second)
        }
        return first
    }
}

/// The error `Header.ordinal(of:)` throws for a name two of the header's columns have.
public struct AmbiguousColumn: Error, Equatable, Sendable, CustomStringConvertible, LocalizedError {
    /// The name asked for.
    public let name: String
    /// The ordinal of the first column of that name.
    public let first: Int
    /// The ordinal of the second column of that name.
    public let second: Int

    /// Names the column that was asked for and the two that have the name.
    public init(name: String, first: Int, second: Int) {
        self.name = name
        self.first = first
        self.second = second
    }

    /// What was asked for and what stands in its way, in a sentence.
    public var description: String {
        "The header names two columns \"\(name)\", at positions \(first) and \(second): say which by position."
    }

    /// ``description``, for `localizedDescription`.
    public var errorDescription: String? { description }
}

/// The error `Header.ordinal(of:)` throws for a name the header does not have.
public struct NoSuchColumn: Error, Equatable, Sendable, CustomStringConvertible, LocalizedError {
    /// The name asked for.
    public let name: String

    /// Names the column that was asked for: what a missing column needs to be found.
    public init(name: String) {
        self.name = name
    }

    /// What was asked for, in a sentence.
    public var description: String { "The header has no column named \"\(name)\"." }

    /// ``description``, for `localizedDescription`.
    public var errorDescription: String? { description }
}

/// A size the core refuses when a reader or a sheet is opened or a plan is bound, before anything is
/// allocated for it. The core decides how much a batch may ask for.
public enum BatchSizeError: Error, Equatable, Sendable, CustomStringConvertible, LocalizedError {
    /// `batchRows` is 0, which reads nothing.
    case zeroBatchRows
    /// `bufferBytes` is 0.
    case zeroBufferBytes
    /// A batch asks for `needed` bytes, past the core's `limit`. `needed` is `UInt64.max` for an ask
    /// too big to count.
    case tooLarge(needed: UInt64, limit: UInt64)
    /// One row of `width` columns asks for `needed` bytes, past the core's `limit`, at any batch size.
    case rowTooWide(width: UInt64, needed: UInt64, limit: UInt64)

    /// What was refused, in a sentence.
    public var description: String {
        switch self {
        case .zeroBatchRows: "batchRows must be at least 1"
        case .zeroBufferBytes: "bufferBytes must be at least 1"
        case .tooLarge(let needed, let limit): "batchRows asks for \(needed) bytes, past the limit of \(limit)"
        case .rowTooWide(let width, let needed, let limit):
            "a row of \(width) columns asks for \(needed) bytes, past the limit of \(limit)"
        }
    }

    /// As ``description``.
    public var errorDescription: String? { description }
}

/// A plan declared at the wrong moment: a reader or sheet opened without one is bound once,
/// with ``DelimitedReader/bind(_:)`` or ``Sheet/bind(_:)``, before its first read.
public enum PlanError: Error, Equatable, Sendable, CustomStringConvertible, LocalizedError {
    /// A read before a plan was bound. Nothing is lost: bind one, and read.
    case unbound
    /// A second plan, for a reader or sheet that already has one.
    case alreadyBound
    /// A plan column whose ordinal is 2²⁰ (1,048,576) or more — more columns than any
    /// workbook format has — or at or past the columns a delimited header gives every record.
    /// The associated value is the column's place in the plan. The reader stays unbound.
    case outOfReach(column: Int)
    /// A plan column whose door, format, date options, vocabulary or tokens the core refuses, asked
    /// before any input is read: a `NumFormat` a decoder rebuilt from a stale word comes here. The
    /// associated value is the column's place in the plan. The reader stays unbound.
    case refused(column: Int)
    /// A read or a bind from inside one of the reader's own reads: by the ``DelimitedReader/Source``
    /// it reads, which runs in the middle of one. Refused before anything moves; the read it was
    /// called from goes on unharmed, and the reader is there to use once it returns.
    case busy

    /// What was done out of order, in a sentence.
    public var description: String {
        switch self {
        case .unbound: "The reader has no plan yet: bind one before reading."
        case .alreadyBound: "The reader already has a plan: a plan is bound once, before the first read."
        case .outOfReach(let column):
            "Plan column \(column) reads past the columns the header gives every record, or at or past ordinal 2^20 (1,048,576)."
        case .refused(let column):
            "Plan column \(column) declares a format or date options no door accepts."
        case .busy: "The reader is in the middle of a read; its source can't call back into it."
        }
    }

    /// ``description``, for `localizedDescription`.
    public var errorDescription: String? { description }
}
