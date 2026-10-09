import Foundation

/// The names a source's header declares, in column order: what a plan's ordinals are
/// looked up by when the columns are known by name rather than by position.
///
/// A header is a collection of `String` — `header[0]`, `header.count`, `for name in header`,
/// a literal `["id", "name"]` to compare against — and each name is also kept as the bytes
/// the source held, so that a lookup is exact: byte for byte, case and spaces included, with
/// no Unicode normalization, the first of two columns with one name.
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

    /// The names a source held, as its bytes; a name that is not UTF-8 reads with its
    /// malformed sequences replaced, as every text cell does.
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

    /// The ordinal of the first column named `name`, or `nil` when no column is. The match
    /// is exact — the name's UTF-8 against the bytes the source held — so `"é"` precomposed
    /// does not find `"é"` decomposed, as `String`'s own `==` would.
    public func firstIndex(of name: String) -> Int? {
        firstIndex(of: name.utf8)
    }

    /// The ordinal of the first column whose name is exactly these bytes, or `nil` when no
    /// column's is.
    public func firstIndex<Name: Collection>(of name: Name) -> Int? where Name.Element == UInt8 {
        utf8.firstIndex { $0.elementsEqual(name) }
    }

    /// The ordinal of the first column named `name`, matched as `firstIndex(of:)`
    /// matches: what a plan built from names reads best with.
    ///
    /// - Throws: ``NoSuchColumn``, naming `name`, when no column is named that.
    public func ordinal(of name: String) throws -> Int {
        guard let ordinal = firstIndex(of: name) else {
            throw NoSuchColumn(name: name)
        }
        return ordinal
    }

    /// The ordinal of the first column whose name is exactly these bytes.
    ///
    /// - Throws: ``NoSuchColumn``, naming the bytes as text, when no column's name is them.
    public func ordinal<Name: Collection>(of name: Name) throws -> Int where Name.Element == UInt8 {
        guard let ordinal = firstIndex(of: name) else {
            throw NoSuchColumn(name: String(decoding: Array(name), as: UTF8.self))
        }
        return ordinal
    }
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

/// A plan declared at the wrong moment: a reader or sheet opened without one is bound once,
/// with ``DelimitedReader/bind(_:)`` or ``Sheet/bind(_:)``, before its first read.
public enum PlanError: Error, Equatable, Sendable, CustomStringConvertible, LocalizedError {
    /// A read before a plan was bound. Nothing is lost: bind one, and read.
    case unbound
    /// A second plan, for a reader or sheet that already has one.
    case alreadyBound

    /// What was done out of order, in a sentence.
    public var description: String {
        switch self {
        case .unbound: "The reader has no plan yet: bind one before reading."
        case .alreadyBound: "The reader already has a plan: a plan is bound once, before the first read."
        }
    }

    /// ``description``, for `localizedDescription`.
    public var errorDescription: String? { description }
}
