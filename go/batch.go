package hypertabular

import (
	"fmt"
	"iter"
	"math"
	"time"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	"github.com/google/uuid"
)

// Batch is the rows one Read delivered, as typed columns: the same type for delimited text
// and for a sheet of a workbook.
//
// A column is handed out whole — I32, F64, Text and the rest, one element per row, with
// Verdicts beside them — or a cell at a time through Get (GetChecked for a column that declares
// constraints), or a row at a time through Row and All. Everything it hands out points into its reader's own buffers (and, for text in
// memory, into that text itself) and is valid until the reader's next Read: views, not
// copies, so do not write through them. The reader hands out the same *Batch every time,
// with the next rows in it.
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
	// scratch is where Raw unescapes a quoted delimited cell, and core the reader's instance of
	// the core, which does the unescaping and lists the faults.
	scratch []byte
	core    *native
	// expected is the width a delimited read's rows are cut to under LongRowsReport, and firstColumn
	// how far a sheet's range starts from its column A.
	expected    uint32
	firstColumn int
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

// Verdicts is a column's verdicts, one per row, as the core wrote them. A verdict that is OK is
// a value. On a column that declares constraints, one whose Violation reports true cast and broke
// one, and GetChecked names it. Any other is the cell's fault (Fault), its span in the cell's text.
func (b *Batch) Verdicts(column int) []CellVerdict {
	return b.columns.columns[column].verdicts[:b.rows:b.rows]
}

// Fault is the verdict of the cell at (column, row) as HyperCast's own union: nil for a
// cell that cast, otherwise the fault — the one allocation this makes.
func (b *Batch) Fault(column, row int) *hypercast.Fault {
	return b.Verdicts(column)[row].Fault()
}

// Origin is where a cell's verdict came from: the door, or something that spared it being asked.
// The values are the core's codes. A later release may add one, so a switch over origins needs a
// default arm.
type Origin uint16

const (
	// OriginCell is a door's verdict on the cell's text: every cell that cast, and every refusal.
	OriginCell Origin = 0
	// OriginToken is a declared token: Empty, and no door asked.
	OriginToken Origin = 1
	// OriginPadded is a cell its row doesn't have, under ShortRowsPad or past a sheet row's last
	// cell: Empty, and no door asked.
	OriginPadded Origin = 2
	// OriginUncached is a workbook formula with no cached value: Empty, and no door asked. The core
	// never evaluates a formula.
	OriginUncached Origin = 3
)

// String names the origin as the corpus does: "cell", "token", "padded", "uncached".
func (o Origin) String() string {
	switch o {
	case OriginCell:
		return "cell"
	case OriginToken:
		return "token"
	case OriginPadded:
		return "padded"
	case OriginUncached:
		return "uncached"
	default:
		return fmt.Sprintf("Origin(%d)", uint16(o))
	}
}

// note is what the core wrote beside the verdict of the cell at (column, row): the zero note with
// notes off.
func (b *Batch) note(column, row int) rawNote {
	b.checkRow(row)
	c := &b.columns.columns[column]
	if c.notes == nil {
		return rawNote{}
	}
	return c.notes[row]
}

// Detail is what failed in the cell at (column, row), HyperCast's own name for it: 0, whose String
// is "none", for a cell that cast and for an Empty no door gave, and for every cell with notes off
// (Notes). A number this module's HyperCast doesn't name is kept, as Detail says. hypercast.Fault
// has no room for this, so it's read here and on Faults' entries.
func (b *Batch) Detail(column, row int) hypercast.Detail {
	return hypercast.Detail(b.note(column, row).detail)
}

// Origin is where the verdict of the cell at (column, row) came from. With notes off (Notes) every
// cell says OriginCell: a token's or a padded cell's verdict is still Empty, and nothing was kept
// of why.
func (b *Batch) Origin(column, row int) Origin {
	return Origin(b.note(column, row).origin)
}

// Problem is one entry of Batch.Faults: a cell that went wrong, or a row longer than the width.
type Problem struct {
	// Row is the row in the batch, from zero.
	Row int
	// Line is where the row came from, as Batch.Line says.
	Line int
	// PlanColumn is the column of the plan, or -1 for an entry about the row as a whole.
	PlanColumn int
	// SourceColumn is the column in the source, from zero, or -1 for an entry about the row as a
	// whole. For delimited text it's the plan column's ordinal, and for a sheet its own column, A
	// being 0, wherever a range starts.
	SourceColumn int
	// Reason is the verdict's reason: Malformed, OutOfRange, or Empty for a formula with no cached
	// value. It's zero, which is no CastFailure, for an entry about the row as a whole and for a
	// violation, which Keyword names.
	Reason hypercast.CastFailure
	// Detail is what failed in the cell, as Batch.Detail says.
	Detail hypercast.Detail
	// Origin is where the verdict came from, as Batch.Origin says. An entry about the row as a whole
	// has no verdict, and says OriginCell.
	Origin Origin
	// Offset is the byte offset of the fault's span within the cell's decoded, unescaped text, and
	// Length its length: 0 where there's no span.
	Offset int
	Length int
	// Expected and Found are a long row's widths, the width the first record fixed and the cells
	// it had: 0 otherwise.
	Expected int
	Found    int
	// Keyword is the constraint a cell broke, in a column that declares them: zero, which is no
	// Keyword, for anything else.
	Keyword hypercast.Keyword
}

// Faults is what went wrong in this batch, in order by row and then by column, a row's own entry
// first: cells at Malformed or OutOfRange, cells that broke a constraint their column declares
// (Problem.Keyword), formulas with no cached value (OriginUncached), and rows
// longer than the width under LongRowsReport. Blanks, declared tokens, padding and a row cut under
// LongRowsTruncate aren't problems, and aren't listed.
//
//	for problem := range batch.Faults() { … }
//
// It reads what the fill already wrote, when it's asked for, a few entries at a time. A list ranged
// over after the reader has moved on panics: stopping there would read as no more faults, and the
// list can't answer for the next batch.
//
// With notes off (Notes) the list still holds every refused cell and long row, with no Detail, but
// no formula with no cached value: nothing was kept to tell one from a blank.
//
// The list can't name a row it wasn't given. A row of a sheet whose only cells are formulas with
// nothing spelled where a value goes is an empty row, so by default SkipEmptyRows leaves it out, and
// it's in no batch. Turn that off to have those rows delivered and listed.
func (b *Batch) Faults() iter.Seq[Problem] {
	generation := b.generation
	return func(yield func(Problem) bool) {
		longRows := uint32(LongRowsReject)
		if b.columns.widths && !b.workbook {
			longRows = uint32(LongRowsReport)
		}
		var held [problemsHeld]rawProblem
		var cursor uint64
		for {
			if b.generation != generation {
				panic(staleFaults)
			}
			if b.rows == 0 {
				return
			}
			next, written := b.core.faults(b.columns, b.rows, b.expected, longRows, cursor, held[:])
			cursor = next
			for index := range held[:written] {
				if !yield(b.problem(&held[index])) {
					return
				}
				if b.generation != generation {
					panic(staleFaults)
				}
			}
			if written < len(held) {
				return
			}
		}
	}
}

// staleFaults is what a list says when it's ranged over past its batch.
const staleFaults = "hypertabular: the batch these faults belong to is over: the reader has read on"

// problem is a list entry, with what only the batch knows.
func (b *Batch) problem(raw *rawProblem) Problem {
	row := int(raw.row)
	problem := Problem{
		Row:          row,
		Line:         b.Line(row),
		PlanColumn:   -1,
		SourceColumn: -1,
		Reason:       hypercast.CastFailure(raw.reason),
		Detail:       hypercast.Detail(raw.detail),
		Origin:       Origin(raw.origin),
		Offset:       int(raw.offset),
		Length:       int(raw.len),
		Expected:     int(raw.expected),
		Found:        int(raw.found),
	}
	if raw.reason == verdictViolation {
		problem.Reason = 0
		problem.Keyword = hypercast.Keyword(raw.keyword)
	}
	if raw.planColumn != rowLevel {
		problem.PlanColumn = int(raw.planColumn)
		problem.SourceColumn = b.columns.columns[raw.planColumn].plan.ordinal + b.firstColumn
	}
	return problem
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
//
// A Text column that declares constraints panics here: read it with GetChecked, which would not
// read a violation as passed.
func (b *Batch) Text(column int) [][]byte {
	b.unchecked(column, "Text")
	return b.text(column)
}

// text is a Text column's cells, a constrained one's included: a violation has its bytes.
func (b *Batch) text(column int) [][]byte {
	c := b.of(column, "Text", DoorText, DoorText, DoorText)
	out := c.texts[:b.rows:b.rows]
	if c.batch != b.generation {
		c.batch = b.generation
		spans := unsafe.Slice((*rawSpan)(c.values), b.rows)
		for row := range out {
			if !c.verdicts[row].OK() && !c.verdicts[row].Violation() {
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
	written := b.core.unescape(raw, b.scratch)
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
// A column that declares constraints panics here: read it with GetChecked, which would not read a
// violation as passed.
//
// Get is a function rather than a method because Go methods cannot take type parameters.
func Get[T Value](b *Batch, column, row int) (T, *hypercast.Fault) {
	b.unchecked(column, "Get")
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

// unchecked panics when a column declares constraints: a plain getter would read a violation as
// passed.
func (b *Batch) unchecked(column int, accessor string) {
	if b.columns.columns[column].plan.checked {
		panic(fmt.Sprintf("hypertabular: column %d declares constraints; %s does not read it: use GetChecked", column, accessor))
	}
}

// GetChecked is the cell at (column, row) as HyperCast's checked doors judge it — the same pair
// hypercast.Check (or CheckString) returns for the same text: the value, with whether
// MultipleOf rounding adjusted it and the enum entry a text matched, and a nil rejection; or the
// zero Checked and the rejection, a *hypercast.Fault for text that isn't the type or a
// *hypercast.Violation[T] naming the keyword broken, with the value as cast. A text enum match
// reads as the entry as declared. A column that declares no constraints reads too: every cell
// that cast passes, unadjusted. T is the Go type of the column's door, one HyperCast checks (a
// numeric door's, or string for Text); asking for another panics, as Get does.
//
// GetChecked is a function rather than a method because Go methods cannot take type parameters.
func GetChecked[T hypercast.CheckedTarget](b *Batch, column, row int) (hypercast.Checked[T], hypercast.Rejection) {
	var value T
	switch p := any(&value).(type) {
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
	case *string:
		*p = string(b.text(column)[row])
	}
	c := &b.columns.columns[column]
	verdict := b.Verdicts(column)[row]
	switch {
	case verdict.Violation():
		check := c.checks[row]
		if check.Keyword == 0 {
			panic(contractViolation)
		}
		return hypercast.Checked[T]{EnumIndex: -1}, &hypercast.Violation[T]{
			Keyword: hypercast.Keyword(check.Keyword), Value: value, Offset: verdict.Offset(), Length: verdict.Length(),
		}
	case !verdict.OK():
		return hypercast.Checked[T]{EnumIndex: -1}, verdict.Fault()
	case c.checks == nil:
		return hypercast.Checked[T]{Value: value, EnumIndex: -1}, nil
	}
	check := c.checks[row]
	passed := hypercast.Checked[T]{Value: value, Adjusted: check.Adjusted != 0, EnumIndex: -1}
	if check.EnumIndex != math.MaxUint32 {
		passed.EnumIndex = int(check.EnumIndex)
		if entry, ok := any(c.plan.enumEntry(check.EnumIndex)).(T); ok {
			passed.Value = entry
		}
	}
	return passed, nil
}
