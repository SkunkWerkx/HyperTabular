/// How the text is delimited, declared by the caller. Nothing is sniffed: the separator is
/// stated, quoting is stated, the header is stated — the same stance HyperCast's
/// `NumFormat` takes for numeric notation.
///
/// ```swift
/// var dialect = Dialect.csv
/// dialect.hasHeader = false
/// ```
public struct Dialect: Equatable, Sendable {
    /// The single-byte separator: tab, or any printable ASCII character except `"`.
    /// Anything else is a caller bug, and ``DelimitedReader`` refuses it with a
    /// precondition failure.
    public var separator: Unicode.Scalar
    /// Whether `"` quotes cells (RFC 4180, `""` for a literal quote). Off, a quote is an
    /// ordinary byte.
    public var quoting: Bool
    /// Whether the first record is a header, exposed through ``DelimitedReader/header`` and
    /// never delivered as a row.
    public var hasHeader: Bool
    /// Whether a completely empty line is skipped rather than read as a one-cell row.
    public var skipBlankLines: Bool

    /// Declares a dialect.
    public init(
        separator: Unicode.Scalar, quoting: Bool = true, hasHeader: Bool = true,
        skipBlankLines: Bool = true
    ) {
        self.separator = separator
        self.quoting = quoting
        self.hasHeader = hasHeader
        self.skipBlankLines = skipBlankLines
    }

    /// Comma-separated, quoted, with a header, blank lines skipped.
    public static let csv = Dialect(separator: ",")

    /// Tab-separated, otherwise as ``csv``.
    public static let tsv = Dialect(separator: "\t")

    /// Pipe-separated, otherwise as ``csv``.
    public static let psv = Dialect(separator: "|")
}
