package hypertabular

import "fmt"

// FailureKind is why an input could not be read as rows at all. The values are the native
// core's codes, verbatim.
type FailureKind int

const (
	// UnclosedQuote means the input ended inside a quoted cell.
	UnclosedQuote FailureKind = 1
	// ColumnCount means a record's cell count disagrees with the first record's.
	ColumnCount FailureKind = 2
	// RowTooLong means a single record is larger than MaxRowBytes.
	RowTooLong FailureKind = 3
	// NotAZip means a workbook's container is not a zip file.
	NotAZip FailureKind = 16
	// Container means the zip's own structure is broken.
	Container FailureKind = 17
	// Encrypted means the workbook is encrypted.
	Encrypted FailureKind = 18
	// Method means a part is compressed by a method other than stored or deflate.
	Method FailureKind = 19
	// MissingPart means a part the workbook cannot be read without is not there.
	MissingPart FailureKind = 20
	// XML means a part's XML ends inside a construct.
	XML FailureKind = 21
	// Deflate means a part's bytes are not a deflate stream, or stop before the stream does.
	Deflate FailureKind = 22
	// NotAWorkbook means the zip is neither an XLSX nor an ODS workbook.
	NotAWorkbook FailureKind = 23
	// SharedString means a cell names a shared string the table does not have.
	SharedString FailureKind = 24
	// TooLarge means more text than can be addressed: over 4 GiB in a batch or in the
	// shared strings, or 2 GiB in a cell.
	TooLarge FailureKind = 25
)

var failureNames = map[FailureKind]string{
	UnclosedQuote: "unclosed quote",
	ColumnCount:   "column count",
	RowTooLong:    "row too long",
	NotAZip:       "not a zip",
	Container:     "container",
	Encrypted:     "encrypted",
	Method:        "method",
	MissingPart:   "missing part",
	XML:           "xml",
	Deflate:       "deflate",
	NotAWorkbook:  "not a workbook",
	SharedString:  "shared string",
	TooLarge:      "too large",
}

// String names the kind in lower case — "unclosed quote", "column count", "not a zip".
func (k FailureKind) String() string {
	if name, ok := failureNames[k]; ok {
		return name
	}
	return fmt.Sprintf("FailureKind(%d)", int(k))
}

// Failure is a structural failure: the input is not rows of cells — a record of the wrong
// width, input that ends inside a quoted cell, a workbook whose container or parts cannot be
// read. Never a cell's verdict: a value that does not cast is a fault in its column, and the
// read goes on. A structural failure ends the input, after every intact row before it has
// been delivered. It is the error DelimitedReader.Read and Sheet.Read return then, and on
// every call after, and the error NewWorkbook returns for a workbook it cannot open; reach it
// with errors.As.
//
// For a workbook, Record is the part the failure is in, Line the sheet row, and Byte the
// offset within the part's inflated bytes.
type Failure struct {
	// Kind is what is wrong.
	Kind FailureKind
	// Record is the zero-based index of the offending record — the header and skipped blank
	// lines included.
	Record int64
	// Line is the one-based line the offending record starts on.
	Line int
	// Byte is the absolute byte offset of the offending record's start.
	Byte int64
	// Expected is the number of cells in the first record, for ColumnCount.
	Expected int
	// Found is the number of cells in this record, for ColumnCount.
	Found int
}

// Error describes the failure and where it is.
func (f *Failure) Error() string {
	switch f.Kind {
	case ColumnCount:
		return fmt.Sprintf("hypertabular: record %d (line %d, byte %d) has %d cells; the first record had %d",
			f.Record, f.Line, f.Byte, f.Found, f.Expected)
	case UnclosedQuote:
		return fmt.Sprintf("hypertabular: the input ended inside a quoted cell in record %d (line %d, byte %d)",
			f.Record, f.Line, f.Byte)
	case RowTooLong:
		return fmt.Sprintf("hypertabular: record %d (line %d, byte %d) exceeds the %d-byte row ceiling",
			f.Record, f.Line, f.Byte, MaxRowBytes)
	case NotAZip:
		return "hypertabular: the workbook is not a zip file"
	case Encrypted:
		return "hypertabular: the workbook is encrypted"
	case Method:
		return fmt.Sprintf("hypertabular: part %d of the workbook is compressed by method %d, which is neither stored nor deflate",
			f.Record, f.Found)
	case MissingPart:
		return fmt.Sprintf("hypertabular: part %d, which the workbook cannot be read without, is missing", f.Record)
	case XML:
		return fmt.Sprintf("hypertabular: part %d of the workbook ends inside an XML construct (byte %d)", f.Record, f.Byte)
	case Deflate:
		return fmt.Sprintf("hypertabular: part %d of the workbook is not a whole deflate stream (byte %d)", f.Record, f.Byte)
	case NotAWorkbook:
		return "hypertabular: the zip is neither an XLSX nor an ODS workbook"
	case SharedString:
		return fmt.Sprintf("hypertabular: row %d names shared string %d; the table has %d", f.Line, f.Found, f.Expected)
	case TooLarge:
		return "hypertabular: the workbook holds more text than a batch can address"
	default:
		return "hypertabular: the workbook's zip structure is broken"
	}
}

func structural(raw rawFailure) *Failure {
	kind := FailureKind(raw.code)
	if _, ok := failureNames[kind]; !ok {
		// A code this module does not know is the container's: the most general refusal.
		kind = Container
	}
	return &Failure{
		Kind:     kind,
		Record:   int64(raw.record),
		Line:     int(raw.line),
		Byte:     int64(raw.byte),
		Expected: int(raw.expected),
		Found:    int(raw.found),
	}
}
