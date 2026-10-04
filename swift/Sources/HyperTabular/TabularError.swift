import Foundation

/// Why an input could not be read as rows at all.
public enum TabularFailure: Int32, Equatable, Sendable {
    /// The input ended inside a quoted cell.
    case unclosedQuote = 1
    /// A record's cell count disagrees with the first record's.
    case columnCount = 2
    /// A single record is larger than ``DelimitedReader/maxRowBytes``.
    case rowTooLong = 3
}

/// A structural failure: the input is not rows of cells — a record of the wrong width,
/// input that ends inside a quoted cell. Never a cell's verdict: a value that does not
/// cast is a `Fault` in its column, and the read goes on. A structural failure ends the
/// input, after every intact row before it has been delivered.
public struct TabularError: Error, Equatable, Sendable, CustomStringConvertible, LocalizedError {
    /// What is wrong.
    public let kind: TabularFailure
    /// Zero-based index of the offending record — the header and skipped blank lines included.
    public let record: Int64
    /// One-based line the offending record starts on.
    public let line: Int
    /// Absolute byte offset of the offending record's start.
    public let byte: Int64
    /// Cells in the first record, for ``TabularFailure/columnCount``; `0` otherwise.
    public let expected: Int
    /// Cells in this record, for ``TabularFailure/columnCount``; `0` otherwise.
    public let found: Int

    /// Builds a failure from its parts — what a test asserting an expected failure needs;
    /// the reader is what produces one otherwise.
    public init(kind: TabularFailure, record: Int64, line: Int, byte: Int64, expected: Int = 0, found: Int = 0) {
        self.kind = kind
        self.record = record
        self.line = line
        self.byte = byte
        self.expected = expected
        self.found = found
    }

    /// What is wrong and where, in a sentence.
    public var description: String {
        switch kind {
        case .columnCount:
            "Record \(record) (line \(line), byte \(byte)) has \(found) cells; the first record had \(expected)."
        case .unclosedQuote:
            "The input ended inside a quoted cell in record \(record) (line \(line), byte \(byte))."
        case .rowTooLong:
            "Record \(record) (line \(line), byte \(byte)) exceeds the \(DelimitedReader.maxRowBytes)-byte row ceiling."
        }
    }

    /// ``description``, for `localizedDescription`.
    public var errorDescription: String? { description }
}
