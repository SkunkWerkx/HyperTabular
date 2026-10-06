package hypertabular

import (
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"unsafe"
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

// DelimitedReader reads delimited text — CSV, TSV, any single-byte ASCII separator — a batch
// at a time into typed columns, every cell a HyperCast verdict.
//
// Build one over an io.Reader (NewDelimitedReader), over text already in memory
// (NewDelimitedReaderBytes) or over a file (OpenDelimited), with the Dialect and the plan of
// columns declared. Read returns the next *Batch, whose column accessors — I32, F64, Text
// and the rest, with Verdicts beside them — hand it out as slices, one element per row, and
// Get a cell at a time.
//
// A value that does not cast is that cell's verdict, and the read goes on. Input that is
// not rows of cells at all — a record of the wrong width, a quote never closed — is a
// *Failure, returned by Read after every intact row before it has been delivered.
//
// A DelimitedReader is not safe for concurrent use.
type DelimitedReader struct {
	dialect Dialect
	set     *columns
	batch   Batch
	// perRow is the cell-table entries one row takes: the widest ordinal the plan reads,
	// plus two, or more if the core asked for more.
	perRow int

	// What the core is handed beside the plan's buffers, all of it Go memory with no Go
	// pointer inside it.
	core  *core
	cells []rawSpan
	arena []byte
	// cramped is whether the last batch ended early because the arena filled: the next one
	// starts with it doubled, so that escaped text costs a few batches, not one per row.
	cramped bool

	// The source: an io.Reader read into buffer, or memory read in place.
	source      io.Reader
	closer      io.Closer
	buffer      []byte
	start       int
	end         int
	eof         bool
	maxRowBytes int

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
	if o.bufferBytes <= 0 {
		return nil, o, fmt.Errorf("hypertabular: BufferBytes must be positive, not %d", o.bufferBytes)
	}

	set, err := newColumns(plan, o.batchRows)
	if err != nil {
		return nil, o, err
	}
	// A cell table entry for every source column up to the widest the plan reads, and one
	// for the row's line.
	perRow := set.width + 1
	if perRow > math.MaxInt/int(unsafe.Sizeof(rawSpan{}))/o.batchRows {
		return nil, o, fmt.Errorf("hypertabular: a plan reading source column %d at %d rows a batch needs too large a cell table",
			set.width-1, o.batchRows)
	}
	r := &DelimitedReader{
		dialect:     dialect,
		set:         set,
		batch:       Batch{columns: set},
		perRow:      perRow,
		core:        new(core),
		cells:       make([]rawSpan, perRow*o.batchRows),
		arena:       make([]byte, 4096),
		maxRowBytes: o.maxRowBytes,
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
	return fmt.Errorf("hypertabular: out of memory for the column table of a %d-column plan", len(r.set.plan))
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

// Dialect is the dialect the text is read in.
func (r *DelimitedReader) Dialect() Dialect { return r.dialect }

// Plan is the plan the text is read through: column i of every batch is Plan()[i]. Do not
// modify it.
func (r *DelimitedReader) Plan() []Column { return r.batch.Columns() }

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

// Read reads the next batch: up to BatchRows whole rows, as many as one call of the core
// found — fewer when the input, or what the read buffer holds of it, ran out first. At the
// end of the input it returns nil and io.EOF. Each batch is one crossing into the core, and
// the batch is valid until the next Read.
//
// When the input is structurally broken it returns nil and a *Failure — after every intact
// row before the break has been delivered by earlier calls — and the same *Failure on
// every later call. An error from the underlying io.Reader ends the input the same way.
// After Close it returns os.ErrClosed.
func (r *DelimitedReader) Read() (*Batch, error) {
	r.batch.clear()
	if r.closed {
		return nil, os.ErrClosed
	}
	if r.err != nil {
		return nil, r.err
	}
	if r.cramped {
		r.arena = make([]byte, 2*len(r.arena))
		r.cramped = false
	}
	filled := &r.core.filled
	for {
		window, last := r.next()
		if len(window) == 0 && !last {
			if err := r.refill(); err != nil {
				r.err = err
				return nil, err
			}
			continue
		}
		switch code := r.nativeFill(window, last, r.set.batchRows); code {
		case codeOK:
			consumed := int(filled.consumed)
			r.start += consumed
			if rows := int(filled.rows); rows > 0 {
				r.cramped = rows < r.set.batchRows && filled.arenaUsed*2 >= uint64(len(r.arena))
				return r.batch.fill(rows, r.cells, r.perRow, window, r.arena), nil
			}
			if last && (consumed == len(window) || consumed == 0) {
				r.err = io.EOF
				return nil, io.EOF
			}
			if consumed == 0 {
				if err := r.refill(); err != nil {
					r.err = err
					return nil, err
				}
			}
		case errCells:
			r.perRow = max(r.perRow, int(filled.needed))
			r.cells = make([]rawSpan, r.perRow*r.set.batchRows)
		case errArena:
			r.arena = make([]byte, max(int(filled.needed), 2*len(r.arena)))
		case errStructure:
			r.err = structural(filled.failure)
			return nil, r.err
		case errShimMemory:
			r.err = r.shimMemory()
			return nil, r.err
		default:
			panic(contractViolation)
		}
	}
}

// Close releases the reader: the batch in hand is over, and a file opened by OpenDelimited
// is closed. A reader built over an io.Reader or over memory holds nothing that needs
// releasing — every buffer is ordinary Go memory — and does not close its source.
func (r *DelimitedReader) Close() error {
	if r.closed {
		return nil
	}
	r.closed = true
	r.batch.clear()
	if r.closer != nil {
		return r.closer.Close()
	}
	return nil
}
