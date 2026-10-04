package hypertabular

import (
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"time"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	"github.com/google/uuid"
)

const (
	// MaxRowBytes is the row ceiling: a single record larger than this is a RowTooLong
	// failure.
	MaxRowBytes = 1 << 30

	// DefaultBufferBytes is the initial read buffer for an io.Reader.
	DefaultBufferBytes = 256 * 1024

	// DefaultBatchRows is the rows per batch unless told otherwise.
	DefaultBatchRows = 4096
)

// Option adjusts how a DelimitedReader is built.
type Option func(*options)

type options struct {
	batchRows   int
	bufferBytes int
	maxRowBytes int
}

// BatchRows sets the rows per batch — the capacity of every column buffer. It must be
// positive; the default is DefaultBatchRows.
func BatchRows(rows int) Option {
	return func(o *options) { o.batchRows = rows }
}

// BufferBytes sets the initial read buffer for an io.Reader or a file; it doubles whenever
// a record does not fit, up to MaxRowBytes. It must be positive; the default is
// DefaultBufferBytes. Text already in memory is read in place and has no buffer.
func BufferBytes(bytes int) Option {
	return func(o *options) { o.bufferBytes = bytes }
}

// core is what the native core writes that is not a column: its state block and the result
// of the last call. Its own allocation, free of Go pointers, so that a pointer into it can
// be handed to C without the reader — which is full of them — being what C is pointed at.
type core struct {
	state  rawState
	filled rawFilled
}

// column is one plan column's share of the reader's buffers.
type column struct {
	plan Column
	// values is the door's value array inside the reader's block: batchRows values of the
	// layout the core writes.
	values   unsafe.Pointer
	verdicts []CellVerdict

	// The doors whose Go type is not the core's layout are converted here, once per batch,
	// the first time the column is asked for: at most one of these is allocated.
	times  []time.Time
	dates  []hypercast.Date
	civils []hypercast.CivilDateTime
	texts  [][]byte
	// batch is the batch the conversion above was last made for.
	batch uint64
}

// DelimitedReader reads delimited text — CSV, TSV, any single-byte ASCII separator — a batch
// at a time into typed columns, every cell a HyperCast verdict.
//
// Build one over an io.Reader (NewDelimitedReader), over text already in memory
// (NewDelimitedReaderBytes) or over a file (OpenDelimited), with the Dialect and the plan of
// columns declared. Read fills the next batch; the column accessors — I32, F64, Text and
// the rest, with Verdicts beside them — then hand that batch out as slices, one element
// per row.
//
// A value that does not cast is that cell's verdict, and the read goes on. Input that is
// not rows of cells at all — a record of the wrong width, a quote never closed — is a
// *Failure, returned by Read after every intact row before it has been delivered.
//
// The slices handed out for a batch point into the reader's own buffers — and, for Text
// and Raw over text in memory, into that text itself — and are valid until the next Read.
// They are views, not copies: do not write through them. A DelimitedReader is not safe for
// concurrent use.
type DelimitedReader struct {
	columns   []column
	batchRows int
	// perRow is the cell-table entries one row takes: the widest ordinal the plan reads,
	// plus two.
	perRow int

	// Everything the core is handed, all of it Go memory with no Go pointer inside it. The
	// column buffers are one allocation — block — and each column's two arrays are named
	// to the core by their offsets into it; see backend_static.go for why.
	core    *core
	specs   []rawSpec
	offsets []uintptr
	block   []uint64
	cells   []rawSpan
	arena   []byte
	scratch []byte

	// The source: an io.Reader read into buffer, or memory read in place.
	source      io.Reader
	closer      io.Closer
	buffer      []byte
	start       int
	end         int
	eof         bool
	maxRowBytes int

	// The batch in hand, and the window of input its spans point into.
	window []byte
	rows   int
	batch  uint64

	header []string
	// err is how the input ended, once it has: io.EOF, a *Failure, or the source's error.
	err    error
	closed bool
}

// NewDelimitedReader reads UTF-8 delimited text from an io.Reader, forward only, through a
// buffer of its own. When the dialect declares a header it is read here, so a header that
// is structurally broken — or a source that fails before the header is complete — is this
// function's error. The reader does not close source.
func NewDelimitedReader(source io.Reader, dialect Dialect, plan []Column, opts ...Option) (*DelimitedReader, error) {
	if source == nil {
		return nil, errors.New("hypertabular: the source is nil")
	}
	r, o, err := newReader(dialect, plan, opts)
	if err != nil {
		return nil, err
	}
	r.source = source
	r.buffer = make([]byte, min(o.bufferBytes, r.maxRowBytes))
	return r.begin(dialect)
}

// NewDelimitedReaderBytes reads UTF-8 delimited text already in memory, in place — nothing
// is copied, so the text must not change while the reader is in use. When the dialect
// declares a header it is read here, so a header that is structurally broken is this
// function's error.
func NewDelimitedReaderBytes(utf8 []byte, dialect Dialect, plan []Column, opts ...Option) (*DelimitedReader, error) {
	r, _, err := newReader(dialect, plan, opts)
	if err != nil {
		return nil, err
	}
	r.buffer = utf8
	r.end = len(utf8)
	r.eof = true
	return r.begin(dialect)
}

// OpenDelimited opens a file of UTF-8 delimited text. The reader owns the file: Close
// closes it.
func OpenDelimited(path string, dialect Dialect, plan []Column, opts ...Option) (*DelimitedReader, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	r, err := NewDelimitedReader(file, dialect, plan, opts...)
	if err != nil {
		_ = file.Close()
		return nil, err
	}
	r.closer = file
	return r, nil
}

// newReader builds everything but the source: the plan as the core reads it, and every
// buffer the core fills.
func newReader(dialect Dialect, plan []Column, opts []Option) (*DelimitedReader, options, error) {
	o := options{batchRows: DefaultBatchRows, bufferBytes: DefaultBufferBytes, maxRowBytes: MaxRowBytes}
	for _, opt := range opts {
		opt(&o)
	}
	if o.batchRows <= 0 {
		return nil, o, fmt.Errorf("hypertabular: BatchRows must be positive, not %d", o.batchRows)
	}
	if o.bufferBytes <= 0 {
		return nil, o, fmt.Errorf("hypertabular: BufferBytes must be positive, not %d", o.bufferBytes)
	}

	r := &DelimitedReader{
		columns:     make([]column, len(plan)),
		batchRows:   o.batchRows,
		core:        new(core),
		specs:       make([]rawSpec, len(plan)),
		offsets:     make([]uintptr, 2*len(plan)),
		arena:       make([]byte, 4096),
		maxRowBytes: o.maxRowBytes,
	}

	// One allocation for every column's values and verdicts, each array on an 8-byte
	// boundary — the strictest alignment any of the Go types they are handed out as wants.
	rows := uintptr(o.batchRows)
	if rows > (math.MaxInt-7)/16 {
		return nil, o, fmt.Errorf("hypertabular: BatchRows %d is too many", o.batchRows)
	}
	widest := -1
	var words uintptr
	for index, planned := range plan {
		spec, err := planned.spec()
		if err != nil {
			return nil, o, fmt.Errorf("hypertabular: plan column %d: %w", index, err)
		}
		r.specs[index] = spec
		r.columns[index].plan = planned
		widest = max(widest, planned.ordinal)
		r.offsets[2*index] = words * 8
		words += (rows*planned.door.valueSize() + 7) / 8
		r.offsets[2*index+1] = words * 8
		words += (rows*unsafe.Sizeof(CellVerdict{}) + 7) / 8
		if words > math.MaxInt/8 {
			return nil, o, fmt.Errorf("hypertabular: a plan of %d columns at %d rows a batch is too large", len(plan), o.batchRows)
		}
	}
	r.perRow = widest + 2
	if r.perRow > math.MaxInt/int(unsafe.Sizeof(rawSpan{}))/o.batchRows {
		return nil, o, fmt.Errorf("hypertabular: a plan reading source column %d at %d rows a batch needs too large a cell table",
			widest, o.batchRows)
	}
	r.block = make([]uint64, words)
	r.cells = make([]rawSpan, r.perRow*o.batchRows)
	base := unsafe.Pointer(unsafe.SliceData(r.block))
	for index := range r.columns {
		c := &r.columns[index]
		c.values = unsafe.Add(base, r.offsets[2*index])
		c.verdicts = unsafe.Slice((*CellVerdict)(unsafe.Add(base, r.offsets[2*index+1])), o.batchRows)
		switch c.plan.door {
		case DoorTimestamp, DoorUnix, DoorExcelSerial:
			c.times = make([]time.Time, o.batchRows)
		case DoorDateOnly, DoorDateOnlyOrdered:
			c.dates = make([]hypercast.Date, o.batchRows)
		case DoorDateTime:
			c.civils = make([]hypercast.CivilDateTime, o.batchRows)
		case DoorText:
			c.texts = make([][]byte, o.batchRows)
		}
	}

	switch r.nativeInit(dialect) {
	case codeOK:
	case errStateSize:
		return nil, o, errors.New("hypertabular: the linked libhypertabular's state block is not the one this module mirrors — the archive and the module are from different versions")
	default:
		return nil, o, fmt.Errorf("hypertabular: separator %q is not tab or printable ASCII other than '\"'", dialect.Separator)
	}
	// The core's own word on the plan, before any input: a fill of no rows still judges
	// every column's door and notation, and touches nothing else.
	switch r.nativeFill(nil, false, 0) {
	case codeOK:
	case errShimMemory:
		return nil, o, r.shimMemory()
	default:
		return nil, o, errors.New("hypertabular: the core refused the plan: a column's door, declared parameter or numeric format is not one it can honour")
	}
	return r, o, nil
}

// shimMemory is the error for a fill that could not allocate its column table (see
// backend_static.go's ht_fill): only a plan wider than the shim's stack table asks for one.
func (r *DelimitedReader) shimMemory() error {
	return fmt.Errorf("hypertabular: out of memory for the column table of a %d-column plan", len(r.columns))
}

// begin reads the header, when the dialect declares one.
func (r *DelimitedReader) begin(dialect Dialect) (*DelimitedReader, error) {
	if !dialect.HasHeader {
		return r, nil
	}
	if err := r.readHeader(); err != nil {
		return nil, err
	}
	return r, nil
}

// Header is the header's names, when the dialect declares one: nil when it does not, and
// empty but not nil for an input with no record.
func (r *DelimitedReader) Header() []string { return r.header }

// Columns is the number of plan columns.
func (r *DelimitedReader) Columns() int { return len(r.columns) }

// Column is the plan column at index column.
func (r *DelimitedReader) Column(column int) Column { return r.columns[column].plan }

// Rows is the number of rows in the batch in hand: zero before the first Read and after
// the last.
func (r *DelimitedReader) Rows() int { return r.rows }

// Records is the number of records finished so far — the header and skipped blank lines
// included.
func (r *DelimitedReader) Records() int64 { return int64(r.core.state.records) }

// next is what the core is to read next, and whether nothing follows it. Never more than
// the row ceiling at once, which is also inside the core's own limit on one chunk.
func (r *DelimitedReader) next() (window []byte, last bool) {
	end := r.end
	if end-r.start > r.maxRowBytes {
		end = r.start + r.maxRowBytes
	}
	return r.buffer[r.start:end:end], r.eof && end == r.end
}

// rowTooLong is the failure for a record that does not fit the row ceiling.
func (r *DelimitedReader) rowTooLong() error {
	state := &r.core.state
	return &Failure{Kind: RowTooLong, Record: int64(state.records), Line: int(state.line), Byte: int64(state.offset)}
}

// refill makes room for the unfinished record at the front of the input and reads more
// behind it.
func (r *DelimitedReader) refill() error {
	if r.source == nil {
		// Text in memory is all there already: the record at the front is longer than the
		// window the core is given.
		return r.rowTooLong()
	}
	pending := r.end - r.start
	if r.start > 0 {
		copy(r.buffer, r.buffer[r.start:r.end])
		r.start, r.end = 0, pending
	}
	if r.end == len(r.buffer) {
		// One record fills the buffer: it needs a bigger one.
		if len(r.buffer) >= r.maxRowBytes {
			return r.rowTooLong()
		}
		larger := make([]byte, min(2*len(r.buffer), r.maxRowBytes))
		copy(larger, r.buffer[:r.end])
		r.buffer = larger
	}
	for empty := 0; ; empty++ {
		n, err := r.source.Read(r.buffer[r.end:])
		if n < 0 || n > len(r.buffer)-r.end {
			return errors.New("hypertabular: the source returned an invalid count from Read")
		}
		r.end += n
		switch {
		case err == io.EOF:
			r.eof = true
			return nil
		case err != nil:
			return fmt.Errorf("hypertabular: reading the input: %w", err)
		case n > 0:
			return nil
		case empty >= 100:
			return fmt.Errorf("hypertabular: reading the input: %w", io.ErrNoProgress)
		}
	}
}

// readHeader reads the first record as the header.
func (r *DelimitedReader) readHeader() error {
	names := make([]rawSpan, 64)
	filled := &r.core.filled
	for {
		window, last := r.next()
		if len(window) == 0 && !last {
			if err := r.refill(); err != nil {
				return err
			}
			continue
		}
		switch r.nativeHeader(window, last, names) {
		case codeOK:
			consumed := int(filled.consumed)
			r.start += consumed
			if filled.rows > 0 {
				header := make([]string, filled.rows)
				for index := range header {
					name := names[index]
					from := window
					if name.flagged() {
						from = r.arena
					}
					header[index] = string(from[name.offset : int(name.offset)+name.length()])
				}
				r.header = header
				return nil
			}
			if last && (consumed == len(window) || consumed == 0) {
				// An empty input has no header and no rows; the width is unknown.
				r.header = []string{}
				return nil
			}
			if consumed == 0 {
				if err := r.refill(); err != nil {
					return err
				}
			}
		case errCells:
			names = make([]rawSpan, filled.needed)
		case errArena:
			r.arena = make([]byte, filled.needed)
		case errStructure:
			return structural(filled.failure)
		default:
			panic(contractViolation)
		}
	}
}

// Read reads the next batch and returns its number of rows, which the column accessors
// then hand out. At the end of the input it returns 0 and io.EOF.
//
// A batch is as many whole rows as one call of the core found, never more than BatchRows
// and never none: fewer when the input, or what the read buffer holds of it, ran out
// first. Each batch is one crossing into the core.
//
// When the input is structurally broken it returns 0 and a *Failure — after every intact
// row before the break has been delivered by earlier calls — and the same *Failure on
// every later call. An error from the underlying io.Reader ends the input the same way.
// After Close it returns os.ErrClosed.
func (r *DelimitedReader) Read() (int, error) {
	r.rows = 0
	r.batch++
	if r.closed {
		return 0, os.ErrClosed
	}
	if r.err != nil {
		return 0, r.err
	}
	filled := &r.core.filled
	for {
		window, last := r.next()
		if len(window) == 0 && !last {
			if err := r.refill(); err != nil {
				r.err = err
				return 0, err
			}
			continue
		}
		switch code := r.nativeFill(window, last, r.batchRows); code {
		case codeOK:
			consumed := int(filled.consumed)
			r.start += consumed
			if filled.rows > 0 {
				r.rows = int(filled.rows)
				r.window = window
				return r.rows, nil
			}
			if last && (consumed == len(window) || consumed == 0) {
				r.err = io.EOF
				return 0, io.EOF
			}
			if consumed == 0 {
				if err := r.refill(); err != nil {
					r.err = err
					return 0, err
				}
			}
		case errCells:
			r.cells = make([]rawSpan, int(filled.needed)*r.batchRows)
		case errArena:
			r.arena = make([]byte, max(int(filled.needed), 2*len(r.arena)))
		case errStructure:
			r.err = structural(filled.failure)
			return 0, r.err
		case errShimMemory:
			r.err = r.shimMemory()
			return 0, r.err
		default:
			panic(contractViolation)
		}
	}
}

// Close releases the reader: the batch in hand is gone, and a file opened by OpenDelimited
// is closed. A reader built over an io.Reader or over memory holds nothing that needs
// releasing — every buffer is ordinary Go memory — and does not close its source.
func (r *DelimitedReader) Close() error {
	if r.closed {
		return nil
	}
	r.closed = true
	r.rows = 0
	r.batch++
	if r.closer != nil {
		return r.closer.Close()
	}
	return nil
}

// Verdicts is a column's verdicts for the batch in hand, one per row, as the core wrote
// them.
func (r *DelimitedReader) Verdicts(column int) []CellVerdict {
	return r.columns[column].verdicts[:r.rows:r.rows]
}

// Fault is the verdict of the cell at (column, row) as HyperCast's own union: nil for a
// cell that cast, otherwise the fault — the one allocation this makes.
func (r *DelimitedReader) Fault(column, row int) *hypercast.Fault {
	return r.Verdicts(column)[row].Fault()
}

// of is the plan column at index column, checked against the doors the caller's accessor
// reads. Asking a column for a type its door does not write is a caller bug, and panics.
func (r *DelimitedReader) of(column int, accessor string, door, also, orElse Door) *column {
	c := &r.columns[column]
	if actual := c.plan.door; actual != door && actual != also && actual != orElse {
		panic(fmt.Sprintf("hypertabular: column %d is cast through %v; %s does not read it", column, actual, accessor))
	}
	return c
}

// view is a column's values for the batch in hand, exactly as the core wrote them, for the
// doors whose Go type has the core's own layout.
func view[T any](r *DelimitedReader, column int, accessor string, door Door) []T {
	c := r.of(column, accessor, door, door, door)
	return unsafe.Slice((*T)(c.values), r.batchRows)[:r.rows:r.rows]
}

// Bool is a Bool column's values for the batch in hand, one per row. Here and for every
// accessor below, the value of a row whose verdict is not OK is the type's zero value, and
// asking a column for a type its door does not write panics.
func (r *DelimitedReader) Bool(column int) []bool { return view[bool](r, column, "Bool", DoorBool) }

// I8 is an I8 column's values for the batch in hand.
func (r *DelimitedReader) I8(column int) []int8 { return view[int8](r, column, "I8", DoorI8) }

// I16 is an I16 column's values for the batch in hand.
func (r *DelimitedReader) I16(column int) []int16 { return view[int16](r, column, "I16", DoorI16) }

// I32 is an I32 column's values for the batch in hand.
func (r *DelimitedReader) I32(column int) []int32 { return view[int32](r, column, "I32", DoorI32) }

// I64 is an I64 column's values for the batch in hand.
func (r *DelimitedReader) I64(column int) []int64 { return view[int64](r, column, "I64", DoorI64) }

// U8 is a U8 column's values for the batch in hand.
func (r *DelimitedReader) U8(column int) []uint8 { return view[uint8](r, column, "U8", DoorU8) }

// U16 is a U16 column's values for the batch in hand.
func (r *DelimitedReader) U16(column int) []uint16 { return view[uint16](r, column, "U16", DoorU16) }

// U32 is a U32 column's values for the batch in hand.
func (r *DelimitedReader) U32(column int) []uint32 { return view[uint32](r, column, "U32", DoorU32) }

// U64 is a U64 column's values for the batch in hand.
func (r *DelimitedReader) U64(column int) []uint64 { return view[uint64](r, column, "U64", DoorU64) }

// F32 is an F32 column's values for the batch in hand.
func (r *DelimitedReader) F32(column int) []float32 {
	return view[float32](r, column, "F32", DoorF32)
}

// F64 is an F64 column's values for the batch in hand.
func (r *DelimitedReader) F64(column int) []float64 {
	return view[float64](r, column, "F64", DoorF64)
}

// Exact is an Exact column's values for the batch in hand: HyperCast's exact Decimal, which
// is the layout the core writes.
func (r *DelimitedReader) Exact(column int) []hypercast.Decimal {
	return view[hypercast.Decimal](r, column, "Exact", DoorExact)
}

// Uuid is a Uuid column's values for the batch in hand.
func (r *DelimitedReader) Uuid(column int) []uuid.UUID {
	return view[uuid.UUID](r, column, "Uuid", DoorUuid)
}

// TimeOfDay is a TimeOfDay column's values for the batch in hand: nanoseconds since
// midnight.
func (r *DelimitedReader) TimeOfDay(column int) []time.Duration {
	return view[time.Duration](r, column, "TimeOfDay", DoorTimeOfDay)
}

// Span is a Span column's values for the batch in hand: HyperCast's seconds-and-nanos
// Duration, which is the layout the core writes.
func (r *DelimitedReader) Span(column int) []hypercast.Duration {
	return view[hypercast.Duration](r, column, "Span", DoorSpan)
}

// Timestamp is the values of a Timestamp, Unix or ExcelSerial column for the batch in
// hand: UTC instants at full nanosecond fidelity, as HyperCast's own doors return them.
// They are converted from what the core wrote the first time a batch's column is asked
// for, into a slice the reader keeps.
func (r *DelimitedReader) Timestamp(column int) []time.Time {
	c := r.of(column, "Timestamp", DoorTimestamp, DoorUnix, DoorExcelSerial)
	out := c.times[:r.rows:r.rows]
	if c.batch != r.batch {
		c.batch = r.batch
		raw := unsafe.Slice((*rawTimestamp)(c.values), r.rows)
		for row := range out {
			if c.verdicts[row].reason != 0 {
				out[row] = time.Time{}
				continue
			}
			out[row] = time.Unix(raw[row].seconds, int64(raw[row].nanos)).UTC()
		}
	}
	return out
}

// DateOnly is the values of a DateOnly or DateOnlyOrdered column for the batch in hand,
// converted as Timestamp's are.
func (r *DelimitedReader) DateOnly(column int) []hypercast.Date {
	c := r.of(column, "DateOnly", DoorDateOnly, DoorDateOnlyOrdered, DoorDateOnly)
	out := c.dates[:r.rows:r.rows]
	if c.batch != r.batch {
		c.batch = r.batch
		raw := unsafe.Slice((*rawDate)(c.values), r.rows)
		for row := range out {
			out[row] = hypercast.Date{Year: int(raw[row].year), Month: time.Month(raw[row].month), Day: int(raw[row].day)}
		}
	}
	return out
}

// DateTime is a DateTime column's values for the batch in hand: wall clocks with no zone —
// the text named none and none is invented. Converted as Timestamp's are.
func (r *DelimitedReader) DateTime(column int) []hypercast.CivilDateTime {
	c := r.of(column, "DateTime", DoorDateTime, DoorDateTime, DoorDateTime)
	out := c.civils[:r.rows:r.rows]
	if c.batch != r.batch {
		c.batch = r.batch
		raw := unsafe.Slice((*rawCivil)(c.values), r.rows)
		for row := range out {
			date := raw[row].date
			out[row] = hypercast.CivilDateTime{
				Date:      hypercast.Date{Year: int(date.year), Month: time.Month(date.month), Day: int(date.day)},
				TimeOfDay: time.Duration(raw[row].nanosOfDay),
			}
		}
	}
	return out
}

// Text is a Text column's cells for the batch in hand: each cell's bytes, untrimmed, quotes
// resolved, and nil for a cell with no bytes at all — which is the one way text fails.
// Nothing is copied: a cell is a slice of the input itself (the reader's buffer, or the
// caller's own memory under NewDelimitedReaderBytes), and only a cell that had an escaped
// quote in it is a slice of the reader's arena instead.
func (r *DelimitedReader) Text(column int) [][]byte {
	c := r.of(column, "Text", DoorText, DoorText, DoorText)
	out := c.texts[:r.rows:r.rows]
	if c.batch != r.batch {
		c.batch = r.batch
		spans := unsafe.Slice((*rawSpan)(c.values), r.rows)
		for row := range out {
			if c.verdicts[row].reason != 0 {
				out[row] = nil
				continue
			}
			from := r.window
			if spans[row].flagged() {
				from = r.arena
			}
			start := int(spans[row].offset)
			end := start + spans[row].length()
			out[row] = from[start:end:end]
		}
	}
	return out
}

// Raw is the text the cell at (column, row) was cast from, whatever its door and whatever
// its verdict — what a fault's span indexes, and what to show for a value that did not
// cast. A slice of the input itself, except for a cell with an escaped quote in it, which
// is unescaped — as the core cast it — into a scratch buffer that the next call to Raw
// reuses. Valid until the next Raw or Read.
func (r *DelimitedReader) Raw(column, row int) []byte {
	if row < 0 || row >= r.rows {
		panic(fmt.Sprintf("hypertabular: row %d is outside the batch in hand, which has %d", row, r.rows))
	}
	cell := r.cells[row*r.perRow+r.columns[column].plan.ordinal]
	start := int(cell.offset)
	end := start + cell.length()
	raw := r.window[start:end:end]
	if !cell.flagged() {
		return raw
	}
	if len(r.scratch) < len(raw) {
		r.scratch = make([]byte, max(len(raw), 256))
	}
	written := nativeUnescape(raw, r.scratch)
	return r.scratch[:written:written]
}
