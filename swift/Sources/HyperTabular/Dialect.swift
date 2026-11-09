/// What a record with fewer cells than the width is. The raw values are the core's codes.
public enum ShortRows: UInt32, Equatable, Sendable {
    /// A ``TabularFailure/columnCount`` failure: the default.
    case reject = 0
    /// A row whose missing cells are empty, at ``Origin/padded``.
    case pad = 1
}

/// What a record with more cells than the width is. The raw values are the core's codes.
public enum LongRows: UInt32, Equatable, Sendable {
    /// A ``TabularFailure/columnCount`` failure: the default.
    case reject = 0
    /// Cut to the width, and ``Batch/faults()`` lists it with the width it had.
    case report = 1
    /// Cut to the width, and nothing says so.
    case truncate = 2
}

/// What a delimited file's bytes are, declared and never sniffed. Anything but ``utf8`` is decoded
/// ahead of the read, so that everything a batch says, its spans included, is about the decoded
/// text. A unit that has no character is a ``TabularFailure/encoding`` failure, after every intact
/// row before it, whose ``TabularError/byte`` alone counts the original bytes: nothing is ever
/// replaced. A workbook declares its own encoding, so this is delimited text's alone. The raw values
/// are the core's codes.
public enum TextEncoding: UInt32, Equatable, Sendable {
    /// Read without a copy: the default. A cell that is not valid is that cell's malformed verdict.
    case utf8 = 0
    /// As WHATWG has it: the five bytes the code page leaves undefined are their C1 controls.
    /// `EF BB BF` is three characters here, not a byte-order mark.
    case windows1252 = 1
    /// ISO-8859-1: every byte the code point of its value.
    case latin1 = 2
    /// UTF-16, little-endian, with or without a byte-order mark. A lone surrogate, or half a code
    /// unit at the end of the input, is the failure.
    case utf16LE = 3
}

/// How the text is delimited, declared by the caller. Nothing is sniffed: the separator is
/// stated, quoting is stated, the header is stated — the same stance HyperCast's
/// `NumFormat` takes for numeric notation.
///
/// ```swift
/// var dialect = Dialect.csv
/// dialect.hasHeader = false
/// dialect.encoding = .windows1252
/// dialect.tokens = ["NA", "-"]
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
    /// The physical lines to pass over after a byte-order mark, before anything is read of them:
    /// a quote in one means nothing.
    public var skip: UInt32
    /// The records to discard after those, ahead of the header if there is one. A blank line this
    /// dialect skips is not a record.
    public var headerRow: UInt32
    /// What a record with fewer cells than the width is.
    public var shortRows: ShortRows
    /// What a record with more cells than the width is.
    public var longRows: LongRows
    /// What the input's bytes are.
    public var encoding: TextEncoding
    /// What stands for no value in every column without tokens of its own
    /// (``Column/withTokens(_:)``): a cell whose exact, unquoted bytes are one of these is empty,
    /// whatever its door, at ``Origin/token``. Matched as the tokens' UTF-8 bytes.
    public var tokens: [String]
    /// Whether a batch keeps what ``Batch/origin(_:row:)``, a fault's `detail` and
    /// ``Batch/faults()`` read: on unless this turns it off. Off, a fault has no detail, every
    /// origin is ``Origin/cell``, and a read saves the little it costs to ask a door why a cell
    /// failed and to keep the answer.
    public var notes: Bool

    /// Declares a dialect. Every declaration after the separator defaults to what ``csv`` has.
    public init(
        separator: Unicode.Scalar, quoting: Bool = true, hasHeader: Bool = true,
        skipBlankLines: Bool = true, skip: UInt32 = 0, headerRow: UInt32 = 0,
        shortRows: ShortRows = .reject, longRows: LongRows = .reject, encoding: TextEncoding = .utf8,
        tokens: [String] = [], notes: Bool = true
    ) {
        self.separator = separator
        self.quoting = quoting
        self.hasHeader = hasHeader
        self.skipBlankLines = skipBlankLines
        self.skip = skip
        self.headerRow = headerRow
        self.shortRows = shortRows
        self.longRows = longRows
        self.encoding = encoding
        self.tokens = tokens
        self.notes = notes
    }

    /// Comma-separated, quoted, with a header, blank lines skipped.
    public static let csv = Dialect(separator: ",")

    /// Tab-separated, otherwise as ``csv``.
    public static let tsv = Dialect(separator: "\t")

    /// Pipe-separated, otherwise as ``csv``.
    public static let psv = Dialect(separator: "|")
}

/// A list of tokens as the core reads it: each a little-endian 32-bit length, then its UTF-8 bytes.
func packedTokens(_ tokens: [String]) -> [UInt8] {
    var packed = [UInt8]()
    for token in tokens {
        withUnsafeBytes(of: UInt32(token.utf8.count).littleEndian) { packed.append(contentsOf: $0) }
        packed.append(contentsOf: token.utf8)
    }
    return packed
}
