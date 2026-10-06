package hypertabular

import (
	"fmt"
	"math"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// Door is the door a column is cast through: HyperCast's, plus DoorText for the bytes
// themselves. The values are the native core's codes, verbatim; zero is never a door.
type Door uint32

// The twenty-two doors, named as HyperCast's Go module names the functions behind them.
const (
	// DoorBool is hypercast.Bool: a bool.
	DoorBool Door = 1
	// DoorI8 is hypercast.I8: an int8.
	DoorI8 Door = 2
	// DoorI16 is hypercast.I16: an int16.
	DoorI16 Door = 3
	// DoorI32 is hypercast.I32: an int32.
	DoorI32 Door = 4
	// DoorI64 is hypercast.I64: an int64.
	DoorI64 Door = 5
	// DoorU8 is hypercast.U8: a uint8.
	DoorU8 Door = 6
	// DoorU16 is hypercast.U16: a uint16.
	DoorU16 Door = 7
	// DoorU32 is hypercast.U32: a uint32.
	DoorU32 Door = 8
	// DoorU64 is hypercast.U64: a uint64.
	DoorU64 Door = 9
	// DoorF32 is hypercast.F32: a float32.
	DoorF32 Door = 10
	// DoorF64 is hypercast.F64: a float64.
	DoorF64 Door = 11
	// DoorUuid is hypercast.Uuid: a uuid.UUID.
	DoorUuid Door = 12
	// DoorTimestamp is hypercast.Timestamp: an RFC 3339 instant, as a UTC time.Time.
	DoorTimestamp Door = 13
	// DoorUnix is hypercast.Unix: a Unix-epoch integer at a declared precision, as a UTC
	// time.Time.
	DoorUnix Door = 14
	// DoorDateOnly is hypercast.DateOnly: a strict yyyy-MM-dd date, as a hypercast.Date.
	DoorDateOnly Door = 15
	// DoorTimeOfDay is hypercast.TimeOfDay: a 24-hour time of day, as a time.Duration since
	// midnight.
	DoorTimeOfDay Door = 16
	// DoorSpan is hypercast.Span: a duration, as a hypercast.Duration.
	DoorSpan Door = 17
	// DoorText is the cell's bytes themselves — no cast.
	DoorText Door = 18
	// DoorExact is hypercast.Exact: an exact hypercast.Decimal; no float is ever formed.
	DoorExact Door = 19
	// DoorDateOnlyOrdered is hypercast.DateOnlyOrdered: a separated calendar date under a
	// declared field order, as a hypercast.Date.
	DoorDateOnlyOrdered Door = 20
	// DoorDateTime is hypercast.DateTime: a zone-less civil date-time under a declared field
	// order, as a hypercast.CivilDateTime.
	DoorDateTime Door = 21
	// DoorExcelSerial is hypercast.ExcelSerial: an Excel date serial under a declared date
	// system, as a UTC time.Time.
	DoorExcelSerial Door = 22
)

var doorNames = [...]string{
	DoorBool: "Bool", DoorI8: "I8", DoorI16: "I16", DoorI32: "I32", DoorI64: "I64",
	DoorU8: "U8", DoorU16: "U16", DoorU32: "U32", DoorU64: "U64", DoorF32: "F32", DoorF64: "F64",
	DoorUuid: "Uuid", DoorTimestamp: "Timestamp", DoorUnix: "Unix", DoorDateOnly: "DateOnly",
	DoorTimeOfDay: "TimeOfDay", DoorSpan: "Span", DoorText: "Text", DoorExact: "Exact",
	DoorDateOnlyOrdered: "DateOnlyOrdered", DoorDateTime: "DateTime", DoorExcelSerial: "ExcelSerial",
}

// String names the door as its constructor is named — "I32", "DateOnlyOrdered", "Text".
func (d Door) String() string {
	if d >= DoorBool && int(d) < len(doorNames) {
		return doorNames[d]
	}
	return fmt.Sprintf("Door(%d)", uint32(d))
}

// numeric reports whether the door reads a hypercast.NumFormat: the integers, the reals and
// the exact decimal.
func (d Door) numeric() bool {
	return (d >= DoorI8 && d <= DoorF64) || d == DoorExact
}

// valueSize is the bytes one value of the door takes in a column buffer.
func (d Door) valueSize() uintptr {
	switch d {
	case DoorBool, DoorI8, DoorU8:
		return 1
	case DoorI16, DoorU16:
		return 2
	case DoorI32, DoorU32, DoorF32, DoorDateOnly, DoorDateOnlyOrdered:
		return 4
	case DoorI64, DoorU64, DoorF64, DoorTimeOfDay, DoorText:
		return 8
	default:
		return 16
	}
}

// Column is one output column of a plan: which source column it reads, the door it casts
// through, and — for the numeric doors — the notation. A plan is a projection: a
// forty-column file can be read into five typed columns, in any order, and a source column
// can be read through more than one door.
//
// Build one with the constructor named after its door; the zero Column is not a column, and
// a reader refuses a plan that holds one.
type Column struct {
	ordinal int
	door    Door
	param   uint32
	format  hypercast.NumFormat
}

// Ordinal is the zero-based ordinal of the source column. A column past a record's last
// cell reads as empty.
func (c Column) Ordinal() int { return c.ordinal }

// Door is the door the column is cast through.
func (c Column) Door() Door { return c.door }

// Format is the numeric notation a numeric door reads; the zero NumFormat for the others.
func (c Column) Format() hypercast.NumFormat { return c.format }

// Bool is a bool column: HyperCast's boolean lexicon.
func Bool(ordinal int) Column { return Column{ordinal: ordinal, door: DoorBool} }

// I8 is an int8 column under the declared notation.
func I8(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorI8, format: format}
}

// I16 is an int16 column under the declared notation.
func I16(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorI16, format: format}
}

// I32 is an int32 column under the declared notation.
func I32(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorI32, format: format}
}

// I64 is an int64 column under the declared notation.
func I64(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorI64, format: format}
}

// U8 is a uint8 column under the declared notation.
func U8(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorU8, format: format}
}

// U16 is a uint16 column under the declared notation.
func U16(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorU16, format: format}
}

// U32 is a uint32 column under the declared notation.
func U32(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorU32, format: format}
}

// U64 is a uint64 column under the declared notation.
func U64(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorU64, format: format}
}

// F32 is a float32 column under the declared notation.
func F32(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorF32, format: format}
}

// F64 is a float64 column under the declared notation.
func F64(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorF64, format: format}
}

// Exact is an exact hypercast.Decimal column under the declared notation.
func Exact(ordinal int, format hypercast.NumFormat) Column {
	return Column{ordinal: ordinal, door: DoorExact, format: format}
}

// Uuid is a uuid.UUID column.
func Uuid(ordinal int) Column { return Column{ordinal: ordinal, door: DoorUuid} }

// Timestamp is an RFC 3339 instant column.
func Timestamp(ordinal int) Column { return Column{ordinal: ordinal, door: DoorTimestamp} }

// Unix is a Unix-epoch column at the declared precision — never guessed from magnitude.
func Unix(ordinal int, precision hypercast.UnixPrecision) Column {
	return Column{ordinal: ordinal, door: DoorUnix, param: uint32(precision)}
}

// ExcelSerial is an Excel date-serial column under the declared date system.
func ExcelSerial(ordinal int, epoch hypercast.ExcelEpoch) Column {
	return Column{ordinal: ordinal, door: DoorExcelSerial, param: uint32(epoch)}
}

// DateOnly is a strict yyyy-MM-dd date column.
func DateOnly(ordinal int) Column { return Column{ordinal: ordinal, door: DoorDateOnly} }

// DateOnlyOrdered is a separated-date column under the declared field order.
func DateOnlyOrdered(ordinal int, order hypercast.DateOrder) Column {
	return Column{ordinal: ordinal, door: DoorDateOnlyOrdered, param: uint32(order)}
}

// DateTime is a zone-less civil date-time column under the declared field order.
func DateTime(ordinal int, order hypercast.DateOrder) Column {
	return Column{ordinal: ordinal, door: DoorDateTime, param: uint32(order)}
}

// TimeOfDay is a 24-hour time-of-day column.
func TimeOfDay(ordinal int) Column { return Column{ordinal: ordinal, door: DoorTimeOfDay} }

// Span is a duration column.
func Span(ordinal int) Column { return Column{ordinal: ordinal, door: DoorSpan} }

// Text is a text column: the cell's bytes themselves, untrimmed, quotes resolved.
func Text(ordinal int) Column { return Column{ordinal: ordinal, door: DoorText} }

// maxOrdinal keeps the cell table's row width — the widest ordinal plus two — inside the
// uint32 the core counts it in.
const maxOrdinal = math.MaxInt32 - 2

// spec is the column as the core reads it, or the reason it cannot be one. Everything here
// is a caller's mistake in the plan, reported by the reader's constructor before any input
// is read; the core judges the same things again and has the last word (the constructor's
// probe).
func (c Column) spec() (rawSpec, error) {
	spec := rawSpec{ordinal: uint32(c.ordinal), door: uint32(c.door), param: c.param}
	if c.door < DoorBool || c.door > DoorExcelSerial {
		return spec, fmt.Errorf("door %d is not a door; build a Column with a constructor such as I32 or Text", uint32(c.door))
	}
	if c.ordinal < 0 || c.ordinal > maxOrdinal {
		return spec, fmt.Errorf("ordinal %d is out of range", c.ordinal)
	}
	switch c.door {
	case DoorUnix:
		if _, ok := hypercast.UnixPrecisionFromCode(c.param); !ok {
			return spec, fmt.Errorf("undefined UnixPrecision %d", c.param)
		}
	case DoorDateOnlyOrdered, DoorDateTime:
		if _, ok := hypercast.DateOrderFromCode(c.param); !ok {
			return spec, fmt.Errorf("undefined DateOrder %d", c.param)
		}
	case DoorExcelSerial:
		if _, ok := hypercast.ExcelEpochFromCode(c.param); !ok {
			return spec, fmt.Errorf("undefined ExcelEpoch %d", c.param)
		}
	}
	if !c.door.numeric() {
		// All zeros is the invariant notation, which the other doors do not read.
		return spec, nil
	}

	// HyperCast's NumFormat in the core's 32-byte layout, validated by HyperCast as its own
	// doors validate it. A NUL decimal separator is this core's rule: its spec reads a zero
	// separator as "no format declared".
	format, err := c.format.Raw()
	if err != nil {
		return spec, err
	}
	if format.DecimalSep == 0 {
		return spec, fmt.Errorf("the decimal separator must not be NUL")
	}
	spec.format = format
	return spec, nil
}
