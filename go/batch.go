package hypertabular

import (
	"fmt"
	"time"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	"github.com/google/uuid"
)

// Batch is the rows one Read delivered, as typed columns: the same type for delimited text
// and for a sheet of a workbook.
//
// A column is handed out whole — I32, F64, Text and the rest, one element per row, with
// Verdicts beside them — or a cell at a time through Get. Everything it hands out points
// into its reader's own buffers (and, for text in memory, into that text itself) and is
// valid until the reader's next Read: views, not copies, so do not write through them. The
// reader hands out the same *Batch every time, with the next rows in it.
type Batch struct {
	columns *columns
	rows    int
	// generation names the rows in hand, for the conversions columns keep per batch.
	generation uint64
	// cells is the table that locates each cell: perRow entries a row, the last of them
	// the row's line.
	cells  []rawSpan
	perRow int
	// base is what an unflagged span indexes: the window of input the batch was read from,
	// or the workbook's shared strings. arena is what a flagged one indexes.
	base     []byte
	arena    []byte
	workbook bool
	// scratch is where Raw unescapes a quoted delimited cell.
	scratch []byte
}

// fill makes the batch the rows the core just wrote.
func (b *Batch) fill(rows int, cells []rawSpan, perRow int, base, arena []byte) *Batch {
	b.rows = rows
	b.generation++
	b.cells = cells
	b.perRow = perRow
	b.base = base
	b.arena = arena
	return b
}

// clear ends the batch in hand: the reader is moving on.
func (b *Batch) clear() {
	b.rows = 0
	b.generation++
}

// Rows is how many rows the batch holds: never zero for a batch Read returned, and zero
// once the reader has moved on.
func (b *Batch) Rows() int { return b.rows }

// Columns is the plan the batch was read through: column i of the batch is Columns()[i].
// Do not modify it.
func (b *Batch) Columns() []Column { return b.columns.plan[:len(b.columns.plan):len(b.columns.plan)] }

// Line is where row came from: for delimited text the 1-based line its record starts on,
// for a sheet its 1-based row number.
func (b *Batch) Line(row int) int {
	b.checkRow(row)
	entry := b.cells[row*b.perRow+b.perRow-1]
	if b.workbook {
		return int(entry.offset)
	}
	return int(entry.len)
}

func (b *Batch) checkRow(row int) {
	if row < 0 || row >= b.rows {
		panic(fmt.Sprintf("hypertabular: row %d is outside the batch in hand, which has %d", row, b.rows))
	}
}

// Verdicts is a column's verdicts, one per row, as the core wrote them.
func (b *Batch) Verdicts(column int) []CellVerdict {
	return b.columns.columns[column].verdicts[:b.rows:b.rows]
}

// Fault is the verdict of the cell at (column, row) as HyperCast's own union: nil for a
// cell that cast, otherwise the fault — the one allocation this makes.
func (b *Batch) Fault(column, row int) *hypercast.Fault {
	return b.Verdicts(column)[row].Fault()
}

// of is the plan column at index column, checked against the doors the caller's accessor
// reads. Asking a column for a type its door does not write is a caller bug, and panics.
func (b *Batch) of(column int, accessor string, door, also, orElse Door) *column {
	c := &b.columns.columns[column]
	if actual := c.plan.door; actual != door && actual != also && actual != orElse {
		panic(fmt.Sprintf("hypertabular: column %d is cast through %v; %s does not read it", column, actual, accessor))
	}
	return c
}

// view is a column's values, exactly as the core wrote them, for the doors whose Go type has
// the core's own layout.
func view[T any](b *Batch, column int, accessor string, door Door) []T {
	c := b.of(column, accessor, door, door, door)
	return unsafe.Slice((*T)(c.values), b.columns.batchRows)[:b.rows:b.rows]
}

// Bool is a Bool column's values, one per row. Here and for every column accessor below,
// the value of a row whose verdict is not OK is the type's zero value, and asking a column
// for a type its door does not write panics.
func (b *Batch) Bool(column int) []bool { return view[bool](b, column, "Bool", DoorBool) }

// I8 is an I8 column's values.
func (b *Batch) I8(column int) []int8 { return view[int8](b, column, "I8", DoorI8) }

// I16 is an I16 column's values.
func (b *Batch) I16(column int) []int16 { return view[int16](b, column, "I16", DoorI16) }

// I32 is an I32 column's values.
func (b *Batch) I32(column int) []int32 { return view[int32](b, column, "I32", DoorI32) }

// I64 is an I64 column's values.
func (b *Batch) I64(column int) []int64 { return view[int64](b, column, "I64", DoorI64) }

// U8 is a U8 column's values.
func (b *Batch) U8(column int) []uint8 { return view[uint8](b, column, "U8", DoorU8) }

// U16 is a U16 column's values.
func (b *Batch) U16(column int) []uint16 { return view[uint16](b, column, "U16", DoorU16) }

// U32 is a U32 column's values.
func (b *Batch) U32(column int) []uint32 { return view[uint32](b, column, "U32", DoorU32) }

// U64 is a U64 column's values.
func (b *Batch) U64(column int) []uint64 { return view[uint64](b, column, "U64", DoorU64) }

// F32 is an F32 column's values.
func (b *Batch) F32(column int) []float32 { return view[float32](b, column, "F32", DoorF32) }

// F64 is an F64 column's values.
func (b *Batch) F64(column int) []float64 { return view[float64](b, column, "F64", DoorF64) }

// Exact is an Exact column's values: HyperCast's exact Decimal, which is the layout the core
// writes.
func (b *Batch) Exact(column int) []hypercast.Decimal {
	return view[hypercast.Decimal](b, column, "Exact", DoorExact)
}

// Uuid is a Uuid column's values.
func (b *Batch) Uuid(column int) []uuid.UUID {
	return view[uuid.UUID](b, column, "Uuid", DoorUuid)
}

// TimeOfDay is a TimeOfDay column's values: nanoseconds since midnight.
func (b *Batch) TimeOfDay(column int) []time.Duration {
	return view[time.Duration](b, column, "TimeOfDay", DoorTimeOfDay)
}

// Span is a Span column's values: HyperCast's seconds-and-nanos Duration, which is the
// layout the core writes.
func (b *Batch) Span(column int) []hypercast.Duration {
	return view[hypercast.Duration](b, column, "Span", DoorSpan)
}

// Timestamp is the values of a Timestamp, Unix or ExcelSerial column: UTC instants at full
// nanosecond fidelity, as HyperCast's own doors return them. They are converted from what
// the core wrote the first time a batch's column is asked for, into a slice the reader
// keeps.
func (b *Batch) Timestamp(column int) []time.Time {
	c := b.of(column, "Timestamp", DoorTimestamp, DoorUnix, DoorExcelSerial)
	out := c.times[:b.rows:b.rows]
	if c.batch != b.generation {
		c.batch = b.generation
		raw := unsafe.Slice((*hypercast.RawTimestamp)(c.values), b.rows)
		for row := range out {
			if c.verdicts[row].reason != 0 {
				out[row] = time.Time{}
				continue
			}
			out[row] = raw[row].Time()
		}
	}
	return out
}

// DateOnly is the values of a DateOnly or DateOnlyOrdered column, converted as Timestamp's
// are.
func (b *Batch) DateOnly(column int) []hypercast.Date {
	c := b.of(column, "DateOnly", DoorDateOnly, DoorDateOnlyOrdered, DoorDateOnly)
	out := c.dates[:b.rows:b.rows]
	if c.batch != b.generation {
		c.batch = b.generation
		raw := unsafe.Slice((*hypercast.RawDate)(c.values), b.rows)
		for row := range out {
			if c.verdicts[row].reason != 0 {
				out[row] = hypercast.Date{}
				continue
			}
			out[row] = raw[row].Date()
		}
	}
	return out
}

// DateTime is a DateTime column's values: wall clocks with no zone — the text named none
// and none is invented. Converted as Timestamp's are.
func (b *Batch) DateTime(column int) []hypercast.CivilDateTime {
	c := b.of(column, "DateTime", DoorDateTime, DoorDateTime, DoorDateTime)
	out := c.civils[:b.rows:b.rows]
	if c.batch != b.generation {
		c.batch = b.generation
		raw := unsafe.Slice((*hypercast.RawCivil)(c.values), b.rows)
		for row := range out {
			if c.verdicts[row].reason != 0 {
				out[row] = hypercast.CivilDateTime{}
				continue
			}
			out[row] = raw[row].CivilDateTime()
		}
	}
	return out
}

// bytes is what a span names: the arena's bytes when it is flagged, the base's when not.
func (b *Batch) bytes(span rawSpan) []byte {
	from := b.base
	if span.flagged() {
		from = b.arena
	}
	start := int(span.offset)
	end := start + span.length()
	return from[start:end:end]
}

// Text is a Text column's cells: each cell's bytes, untrimmed — quotes resolved for
// delimited text, and a typed workbook cell said the canonical way (42, true,
// 2024-01-31T10:30:00, PT1H30M) — and nil for a cell with no bytes at all, which is the one
// way text fails. Nothing is copied: a cell is a slice of the input itself (the reader's
// buffer, or the caller's own memory under NewDelimitedReaderBytes) or of a workbook's shared
// strings, and only a cell that had an escaped quote in it, or a typed workbook cell, is a
// slice of the reader's arena instead.
func (b *Batch) Text(column int) [][]byte {
	c := b.of(column, "Text", DoorText, DoorText, DoorText)
	out := c.texts[:b.rows:b.rows]
	if c.batch != b.generation {
		c.batch = b.generation
		spans := unsafe.Slice((*rawSpan)(c.values), b.rows)
		for row := range out {
			if c.verdicts[row].reason != 0 {
				out[row] = nil
				continue
			}
			out[row] = b.bytes(spans[row])
		}
	}
	return out
}

// Raw is the text the cell at (column, row) was cast from, whatever its door and whatever
// its verdict — what a fault's span indexes, and what to show for a value that did not
// cast. For a workbook this is a text cell's own text, and what a typed cell was said as
// when it failed its door (or went through the text door); a typed cell that cast has none,
// and neither has an empty cell.
//
// A slice of the input itself, except for a quoted delimited cell with an escaped quote in
// it, which is unescaped — as the core cast it — into a scratch buffer that the next call to
// Raw reuses. Valid until the next Raw or Read.
func (b *Batch) Raw(column, row int) []byte {
	b.checkRow(row)
	at := column
	if !b.workbook {
		at = b.columns.columns[column].plan.ordinal
	} else if column < 0 || column >= len(b.columns.columns) {
		panic(fmt.Sprintf("hypertabular: column %d of a plan of %d columns", column, len(b.columns.columns)))
	}
	cell := b.cells[row*b.perRow+at]
	if b.workbook {
		return b.bytes(cell)
	}
	start := int(cell.offset)
	end := start + cell.length()
	raw := b.base[start:end:end]
	if !cell.flagged() {
		return raw
	}
	if len(b.scratch) < len(raw) {
		b.scratch = make([]byte, max(len(raw), 256))
	}
	written := nativeUnescape(raw, b.scratch)
	return b.scratch[:written:written]
}

// Value is every type Get reads a cell as: the Go type of each door, which is the type of
// the column accessor of the same name — bool, the integers, the floats, hypercast.Decimal
// (Exact), uuid.UUID, time.Time (Timestamp, Unix, ExcelSerial), hypercast.Date (DateOnly,
// DateOnlyOrdered), hypercast.CivilDateTime (DateTime), time.Duration (TimeOfDay) and
// hypercast.Duration (Span) — and, for a Text column, []byte or string.
type Value interface {
	bool | int8 | int16 | int32 | int64 | uint8 | uint16 | uint32 | uint64 | float32 | float64 |
		hypercast.Decimal | uuid.UUID | time.Time | hypercast.Date | hypercast.CivilDateTime |
		time.Duration | hypercast.Duration | []byte | string
}

// Get is the cell at (column, row) as HyperCast judged it: its value and a nil fault, or
// T's zero value and the fault — the same pair the HyperCast door of the same name returns
// for the same text. T is the Go type of the column's door (see Value); asking for another
// panics, as the column accessors do. A string is the one T that allocates, and a fault.
//
// Get is a function rather than a method because Go methods cannot take type parameters.
func Get[T Value](b *Batch, column, row int) (T, *hypercast.Fault) {
	var value T
	switch p := any(&value).(type) {
	case *bool:
		*p = b.Bool(column)[row]
	case *int8:
		*p = b.I8(column)[row]
	case *int16:
		*p = b.I16(column)[row]
	case *int32:
		*p = b.I32(column)[row]
	case *int64:
		*p = b.I64(column)[row]
	case *uint8:
		*p = b.U8(column)[row]
	case *uint16:
		*p = b.U16(column)[row]
	case *uint32:
		*p = b.U32(column)[row]
	case *uint64:
		*p = b.U64(column)[row]
	case *float32:
		*p = b.F32(column)[row]
	case *float64:
		*p = b.F64(column)[row]
	case *hypercast.Decimal:
		*p = b.Exact(column)[row]
	case *uuid.UUID:
		*p = b.Uuid(column)[row]
	case *time.Time:
		*p = b.Timestamp(column)[row]
	case *hypercast.Date:
		*p = b.DateOnly(column)[row]
	case *hypercast.CivilDateTime:
		*p = b.DateTime(column)[row]
	case *time.Duration:
		*p = b.TimeOfDay(column)[row]
	case *hypercast.Duration:
		*p = b.Span(column)[row]
	case *[]byte:
		*p = b.Text(column)[row]
	case *string:
		*p = string(b.Text(column)[row])
	}
	if verdict := b.Verdicts(column)[row]; !verdict.OK() {
		var zero T
		return zero, verdict.Fault()
	}
	return value, nil
}
