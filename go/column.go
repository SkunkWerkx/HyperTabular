package hypertabular

import (
	"encoding/binary"
	"fmt"

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
	// tokens is the column's own list, packed, in place of its dialect's when hasTokens says so.
	tokens    string
	hasTokens bool
	date      hypercast.DateOptions
	// checked says the column declares constraints: the declaration as the core reads it, and its
	// enum list packed in the guest's little-endian order, held as values so a Column stays
	// comparable.
	checked     bool
	constraints hypercast.RawConstraints
	enum        string
}

// Ordinal is the zero-based ordinal of the source column. A column past a record's last
// cell reads as empty.
func (c Column) Ordinal() int { return c.ordinal }

// Door is the door the column is cast through.
func (c Column) Door() Door { return c.door }

// Format is the numeric notation a numeric door reads; the zero NumFormat for the others.
func (c Column) Format() hypercast.NumFormat { return c.format }

// IsChecked reports whether the column declares HyperCast's constraints, and so is read with
// GetChecked and CellChecked.
func (c Column) IsChecked() bool { return c.checked }

// WithTokens is the same column answering to these tokens rather than its dialect's or sheet's: a
// cell whose exact, unquoted bytes are one of them is Empty, whatever its door, and Batch.Origin
// says OriginToken. No tokens at all means none, so a column can opt out of its dialect's. On a
// sheet only a text cell can be one.
func (c Column) WithTokens(tokens ...string) Column {
	c.tokens = NewTokens(tokens...).packed
	c.hasTokens = true
	return c
}

// WithDateOptions is the same column reading dates under these options and their month vocabulary:
// the Timestamp, DateOnly, DateOnlyOrdered and DateTime doors read them, as HyperCast's own detailed
// doors do, and the others ignore them. On a sheet they reach only a text cell, since a typed date
// cell has no text for them to read. Options HyperCast refuses are the error of whatever binds the
// plan.
func (c Column) WithDateOptions(options hypercast.DateOptions) Column {
	c.date = options
	return c
}

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

// Checked is a column of V's door under the declared notation — I32 for int32, Exact for
// hypercast.Decimal — whose cells must also meet constraints: each is cast, then judged as
// hypercast.Check judges it. Read it with GetChecked or CellChecked; Get and Cell panic on it.
// The constraints' type is the door's, so a column can't pair one door with another's, and the
// zero Constraints, never judged, panics here as a checked door does.
func Checked[V hypercast.Number](ordinal int, format hypercast.NumFormat, constraints hypercast.Constraints[V]) Column {
	var door Door
	switch any(*new(V)).(type) {
	case int8:
		door = DoorI8
	case int16:
		door = DoorI16
	case int32:
		door = DoorI32
	case int64:
		door = DoorI64
	case uint8:
		door = DoorU8
	case uint16:
		door = DoorU16
	case uint32:
		door = DoorU32
	case uint64:
		door = DoorU64
	case float32:
		door = DoorF32
	case float64:
		door = DoorF64
	default:
		door = DoorExact
	}
	column := Column{ordinal: ordinal, door: door, format: format, checked: true, constraints: constraints.Raw()}
	// The enum's decimals are in the host's order; the guest reads its own.
	packed := constraints.PackedEnum()
	for at := 0; at+16 <= len(packed); at += 16 {
		binary.LittleEndian.PutUint64(packed[at:], binary.NativeEndian.Uint64(packed[at:]))
		binary.LittleEndian.PutUint32(packed[at+8:], binary.NativeEndian.Uint32(packed[at+8:]))
	}
	column.enum = string(packed)
	return column
}

// CheckedText is a Text column whose cells must also meet constraints, judged as
// hypercast.CheckString judges them. Read it with GetChecked or CellChecked, where an enum match
// reads as the entry as declared; Get, Cell, Text and TextString panic on it.
func CheckedText(ordinal int, constraints hypercast.Constraints[string]) Column {
	return Column{
		ordinal: ordinal, door: DoorText, checked: true, constraints: constraints.Raw(),
		enum: string(constraints.PackedEnum()),
	}
}

// enumEntry is the text enum's entry at index, as declared: the packed list is each entry's
// little-endian 32-bit length and then its bytes.
func (c Column) enumEntry(index uint32) string {
	packed := c.enum
	for at := uint32(0); ; index-- {
		if int(at)+4 > len(packed) {
			panic(contractViolation)
		}
		length := binary.LittleEndian.Uint32([]byte(packed[at : at+4]))
		if int(at)+4+int(length) > len(packed) {
			panic(contractViolation)
		}
		if index == 0 {
			return packed[at+4 : at+4+length]
		}
		at += 4 + length
	}
}

// maxOrdinal is the widest ordinal a plan may name: below 2^20 (1,048,576). No workbook
// format has that many columns (XLSX and ODS stop at 16,384, Numbers at 1,000), and at the
// cap one row of cell table is at most 8 MiB, so a headerless plan cannot ask for ~16 GiB.
const maxOrdinal = 1<<20 - 1

// spec is the column as the core reads it, or the reason it cannot be one. Everything here
// is a caller's mistake in the plan, reported by the reader's constructor before any input
// is read; the core judges the same things again and has the last word (the constructor's
// probe).
func (c Column) spec() (rawSpec, specExtra, error) {
	spec := rawSpec{ordinal: uint32(c.ordinal), door: uint32(c.door), param: c.param}
	extra := specExtra{
		tokens: c.tokens, hasTokens: c.hasTokens, checked: c.checked, constraints: c.constraints, enum: c.enum,
	}
	if c.door < DoorBool || c.door > DoorExcelSerial {
		return spec, extra, fmt.Errorf("door %d is not a door; build a Column with a constructor such as I32 or Text", uint32(c.door))
	}
	if c.ordinal < 0 {
		return spec, extra, fmt.Errorf("ordinal %d is out of range", c.ordinal)
	}
	if c.ordinal > maxOrdinal {
		return spec, extra, fmt.Errorf("reads source column %d; an ordinal must be below 2^20 (1,048,576)", c.ordinal)
	}
	switch c.door {
	case DoorUnix:
		if _, ok := hypercast.UnixPrecisionFromCode(c.param); !ok {
			return spec, extra, fmt.Errorf("undefined UnixPrecision %d", c.param)
		}
	case DoorDateOnlyOrdered, DoorDateTime:
		if _, ok := hypercast.DateOrderFromCode(c.param); !ok {
			return spec, extra, fmt.Errorf("undefined DateOrder %d", c.param)
		}
	case DoorExcelSerial:
		if _, ok := hypercast.ExcelEpochFromCode(c.param); !ok {
			return spec, extra, fmt.Errorf("undefined ExcelEpoch %d", c.param)
		}
	}
	if c.date != (hypercast.DateOptions{}) {
		date, err := c.date.Raw()
		if err != nil {
			return spec, extra, err
		}
		extra.date = date
		extra.months = string(c.date.Months.Packed())
	}
	if !c.door.numeric() {
		// All zeros is the invariant notation, which the other doors do not read.
		return spec, extra, nil
	}

	// HyperCast's NumFormat in the core's 92-byte size-tagged layout, validated by HyperCast
	// as its own doors validate it.
	format, err := c.format.Raw()
	if err != nil {
		return spec, extra, err
	}
	spec.format = format
	return spec, extra, nil
}
