import Foundation
import HyperTabularCore

/// Why an input could not be read as rows at all. The raw values are the core's codes.
public enum TabularFailure: Int32, Equatable, Sendable {
    /// The input ended inside a quoted cell.
    case unclosedQuote = 1
    /// A record's cell count disagrees with the first record's.
    case columnCount = 2
    /// A single record is larger than ``DelimitedReader/maxRowBytes``.
    case rowTooLong = 3
    /// The workbook's container is not a zip file.
    case notAZip = 16
    /// The zip's own structure is broken.
    case container = 17
    /// The workbook is encrypted.
    case encrypted = 18
    /// A part is compressed by a method other than stored or deflate.
    case method = 19
    /// A part the workbook cannot be read without is missing.
    case missingPart = 20
    /// A part's XML ends inside a construct.
    case xml = 21
    /// A part's bytes are not a deflate stream, or stop before the stream does.
    case deflate = 22
    /// The zip is neither an XLSX nor an ODS workbook.
    case notAWorkbook = 23
    /// A cell names a shared string the table does not have.
    case sharedString = 24
    /// More text than can be addressed: over 4 GiB in a batch or in the shared strings, or
    /// 2 GiB in a cell.
    case tooLarge = 25
}

/// A structural failure: the input is not rows of cells — a record of the wrong width,
/// input that ends inside a quoted cell, a workbook whose container or parts cannot be
/// read. Never a cell's verdict: a value that does not cast is a `Fault` in its column, and
/// the read goes on. A structural failure ends the input, after every intact row before it
/// has been delivered.
///
/// For a workbook, ``record`` is the part the failure is in, ``line`` the sheet row and
/// ``byte`` the offset within the part's inflated bytes.
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

    /// The failure the core reported. A code this module does not know is the
    /// container's: the most general refusal.
    init(_ raw: hypertabular_failure) {
        self.init(
            kind: TabularFailure(rawValue: Int32(bitPattern: raw.code)) ?? .container,
            record: Int64(clamping: raw.record), line: Int(clamping: raw.line),
            byte: Int64(clamping: raw.byte), expected: Int(clamping: raw.expected),
            found: Int(clamping: raw.found))
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
        case .notAZip:
            "The workbook is not a zip file."
        case .container:
            "The workbook's zip structure is broken."
        case .encrypted:
            "The workbook is encrypted."
        case .method:
            "Part \(record) of the workbook is compressed by method \(found), which is neither stored nor deflate."
        case .missingPart:
            "Part \(record), which the workbook cannot be read without, is missing."
        case .xml:
            "Part \(record) of the workbook ends inside an XML construct (byte \(byte))."
        case .deflate:
            "Part \(record) of the workbook is not a whole deflate stream (byte \(byte))."
        case .notAWorkbook:
            "The zip is neither an XLSX nor an ODS workbook."
        case .sharedString:
            "Row \(line) names shared string \(found); the table has \(expected)."
        case .tooLarge:
            "The workbook holds more text than a batch can address."
        }
    }

    /// ``description``, for `localizedDescription`.
    public var errorDescription: String? { description }
}
