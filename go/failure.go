package hypertabular

import "fmt"

// FailureKind is why an input could not be read as rows at all.
type FailureKind int

const (
	// UnclosedQuote means the input ended inside a quoted cell.
	UnclosedQuote FailureKind = 1
	// ColumnCount means a record's cell count disagrees with the first record's.
	ColumnCount FailureKind = 2
	// RowTooLong means a single record is larger than MaxRowBytes.
	RowTooLong FailureKind = 3
)

// String names the kind in lower case — "unclosed quote", "column count", "row too long".
func (k FailureKind) String() string {
	switch k {
	case UnclosedQuote:
		return "unclosed quote"
	case ColumnCount:
		return "column count"
	case RowTooLong:
		return "row too long"
	default:
		return fmt.Sprintf("FailureKind(%d)", int(k))
	}
}

// Failure is a structural failure: the input is not rows of cells — a record of the wrong
// width, input that ends inside a quoted cell. Never a cell's verdict: a value that does
// not cast is a fault in its column, and the read goes on. A structural failure ends the
// input, after every intact row before it has been delivered. It is the error
// DelimitedReader.Read returns then, and on every call after; reach it with errors.As.
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
	default:
		return fmt.Sprintf("hypertabular: record %d (line %d, byte %d) exceeds the %d-byte row ceiling",
			f.Record, f.Line, f.Byte, MaxRowBytes)
	}
}

func structural(raw rawFailure) *Failure {
	kind := UnclosedQuote
	if raw.code == 2 {
		kind = ColumnCount
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
