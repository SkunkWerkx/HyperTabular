package hypertabular

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
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
	notes       bool
}

// Notes says whether a batch carries Batch.Detail and Batch.Origin, for a DelimitedReader and for a
// Sheet: on unless this turns them off. Off, every cell says hypercast.Detail 0 and OriginCell, and
// Batch.Faults lists no formula with no cached value, and a read saves the little it costs to ask a
// door why a cell failed and to keep the answer.
func Notes(on bool) Option {
	return func(o *options) { o.notes = on }
}

// BatchRows sets the rows per batch — the capacity of every column buffer. The core refuses 0 and
// a batch past its limit; left out, it's DefaultBatchRows.
func BatchRows(rows int) Option {
	return func(o *options) { o.batchRows = rows }
}

// BufferBytes sets the initial read buffer for an io.Reader or a file; it doubles whenever
// a record does not fit, up to MaxRowBytes. The core refuses 0 and starts one past MaxRowBytes
// there; left out, it's DefaultBufferBytes. Text already in memory is read in place and has no buffer, unless its dialect
// declares an encoding, and a sheet has none.
func BufferBytes(bytes int) Option {
	return func(o *options) { o.bufferBytes = bytes }
}

// DelimitedReader reads delimited text — CSV, TSV, any single-byte ASCII separator — a batch
// at a time into typed columns, every cell a HyperCast verdict.
//
// Build one over an io.Reader (NewDelimitedReader), over text already in memory
// (NewDelimitedReaderBytes) or over a file (OpenDelimited), with the Dialect and the plan of
// columns declared — or header first, with the Unbound variant of each: the header is read,
// its names looked up (Header().Ordinal), and the plan built from them handed to Bind. Read
// returns the next *Batch, whose column accessors — I32, F64, Text
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
	// set is the plan and its buffers: nil until one is bound.
	set       *columns
	batch     Batch
	batchRows int
	// perRow is the cell-table entries one row takes: the widest ordinal the plan reads,
	// plus two, or more if the core asked for more.
	perRow int

	// core is the reader's instance of the core (core.go): its state, its last report, and
	// the buffers it fills, which live in the instance's memory.
	core *native
	// cells and arena are the cell table and the arena as the last call left them, views of
	// the instance's memory; cellsCap and arenaCap are the entries and bytes each holds.
	cells    []rawSpan
	cellsCap int
	arena    []byte
	arenaCap int
	// cramped is whether the last batch ended early because the arena filled: the next one
	// starts with it doubled, so that escaped text costs a few batches, not one per row.
	cramped bool

	// The source: an io.Reader read into buffer, or memory read in place. decoder is what stands
	// between the source and the buffer when the dialect declares an encoding.
	source      io.Reader
	closer      io.Closer
	buffer      []byte
	start       int
	end         int
	eof         bool
	maxRowBytes int
	decoder     *decoder
	notes       bool
	// sourceErr is an error the source returned beside bytes: the bytes are read first, and this
	// is what the source says when more are wanted.
	sourceErr error

	header Header
	// err is how the input ended, once it has: io.EOF, a *Failure, or the source's error.
	err    error
	closed bool
	// busy is set for the whole of a Read: the io.Reader runs inside one, and a Read, Bind or
	// Close it calls on this reader would move the buffer and the core's state under it.
	busy bool
}

// NewDelimitedReader reads delimited text from an io.Reader, forward only, through a
// buffer of its own, as UTF-8 unless the dialect declares otherwise. When the dialect declares a header it is read here, so a header that
// is structurally broken — or a source that fails before the header is complete — is this
// function's error. The reader does not close source. The source runs in the middle of each
// Read, and a Read, Bind or Close it calls on this reader returns ErrBusy.
func NewDelimitedReader(source io.Reader, dialect Dialect, plan []Column, opts ...Option) (*DelimitedReader, error) {
	return newStreamReader(source, dialect, plan, true, opts)
}

// NewDelimitedReaderUnbound is NewDelimitedReader without a plan: the header is read — when
// the dialect declares one — and the plan is bound later, with Bind, once the header has
// said where each column is. Read returns ErrUnbound until then.
func NewDelimitedReaderUnbound(source io.Reader, dialect Dialect, opts ...Option) (*DelimitedReader, error) {
	return newStreamReader(source, dialect, nil, false, opts)
}

func newStreamReader(source io.Reader, dialect Dialect, plan []Column, bound bool, opts []Option) (*DelimitedReader, error) {
	if source == nil {
		return nil, errors.New("hypertabular: the source is nil")
	}
	r, o, err := newReader(dialect, plan, bound, opts)
	if err != nil {
		return nil, err
	}
	r.source = source
	r.buffer = make([]byte, min(o.bufferBytes, r.maxRowBytes))
	if dialect.Encoding != UTF8 {
		r.decoder = newDecoder(dialect.Encoding, len(r.buffer))
	}
	return r.begin(dialect)
}

// NewDelimitedReaderBytes reads UTF-8 delimited text already in memory, in place: a Text cell
// is a slice of it, and the core reads it a chunk at a time, each byte copied into the core's
// memory once, as it is reached — so the text must not change while the reader is in use.
// When the dialect declares a header it is read here, so a header that is structurally
// broken is this function's error. A dialect that declares an encoding has to be decoded, and
// reads the bytes as NewDelimitedReader would, copying them.
func NewDelimitedReaderBytes(utf8 []byte, dialect Dialect, plan []Column, opts ...Option) (*DelimitedReader, error) {
	return newBytesReader(utf8, dialect, plan, true, opts)
}

// NewDelimitedReaderBytesUnbound is NewDelimitedReaderBytes without a plan: the header is
// read, and the plan bound later with Bind.
func NewDelimitedReaderBytesUnbound(utf8 []byte, dialect Dialect, opts ...Option) (*DelimitedReader, error) {
	return newBytesReader(utf8, dialect, nil, false, opts)
}

func newBytesReader(utf8 []byte, dialect Dialect, plan []Column, bound bool, opts []Option) (*DelimitedReader, error) {
	if dialect.Encoding != UTF8 {
		return newStreamReader(bytes.NewReader(utf8), dialect, plan, bound, opts)
	}
	r, _, err := newReader(dialect, plan, bound, opts)
	if err != nil {
		return nil, err
	}
	r.buffer = utf8
	r.end = len(utf8)
	r.eof = true
	return r.begin(dialect)
}

// OpenDelimited opens a file of delimited text, UTF-8 unless the dialect declares otherwise. The
// reader owns the file: Close closes it.
func OpenDelimited(path string, dialect Dialect, plan []Column, opts ...Option) (*DelimitedReader, error) {
	return openFile(path, dialect, plan, true, opts)
}

// OpenDelimitedUnbound is OpenDelimited without a plan: the header is read, and the plan
// bound later with Bind.
func OpenDelimitedUnbound(path string, dialect Dialect, opts ...Option) (*DelimitedReader, error) {
	return openFile(path, dialect, nil, false, opts)
}

func openFile(path string, dialect Dialect, plan []Column, bound bool, opts []Option) (*DelimitedReader, error) {
	// The core's answer on the sizes, ahead of touching the file.
	specs, extras := sizeSpecs(plan)
	if _, _, err := judged(dialect, specs, extras, opts); err != nil {
		return nil, err
	}
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	r, err := newStreamReader(file, dialect, plan, bound, opts)
	if err != nil {
		_ = file.Close()
		return nil, err
	}
	r.closer = file
	return r, nil
}

// judged is opts in force, and the core's answer on their sizes for specs: the size to start an input
// buffer at. With no plan yet it can only judge what a read holds whatever the plan; Bind asks
// again with one.
func judged(dialect Dialect, specs []rawSpec, extras []specExtra, opts []Option) (options, int, error) {
	o := options{batchRows: DefaultBatchRows, bufferBytes: DefaultBufferBytes, maxRowBytes: MaxRowBytes, notes: true}
	for _, opt := range opts {
		opt(&o)
	}
	var flags uint32
	if o.notes {
		flags |= batchNotes
	}
	if dialect.LongRows == LongRowsReport {
		flags |= batchWidths
	}
	start, err := checkSizes(specs, extras, o.batchRows, o.bufferBytes, flags)
	return o, start, err
}

// newReader builds everything but the source: the core's state, and — when bound — the plan
// as the core reads it and every buffer the core fills, bound before any input is read so
// that a plan's error comes before the header's.
func newReader(dialect Dialect, plan []Column, bound bool, opts []Option) (*DelimitedReader, options, error) {
	o, start, err := judged(dialect, nil, nil, opts)
	if err != nil {
		return nil, o, err
	}
	o.bufferBytes = start

	if err := dialect.check(); err != nil {
		return nil, o, err
	}

	r := &DelimitedReader{
		dialect:     dialect,
		batchRows:   o.batchRows,
		arenaCap:    4096,
		maxRowBytes: o.maxRowBytes,
		notes:       o.notes,
	}
	switch r.nativeInit(dialect) {
	case codeOK:
	case errStateSize:
		return nil, o, errors.New("hypertabular: the core's state block is not the one this package mirrors — internal/core/core.go is not this module's core")
	case errGuestMemory:
		return nil, o, errors.New("hypertabular: the dialect's tokens do not fit the core's 4 GiB memory")
	default:
		return nil, o, fmt.Errorf("hypertabular: separator %q is not tab or printable ASCII other than '\"'", dialect.Separator)
	}
	if bound {
		if err := r.Bind(plan); err != nil {
			return nil, o, err
		}
	}
	return r, o, nil
}

// Bind declares the plan a reader opened without one reads through: column i of every
// batch is plan[i]. It is called once, before the first Read — typically with ordinals the
// header has given:
//
//	header := reader.Header()
//	id, err := header.Ordinal("id")
//	…
//	err = reader.Bind([]hypertabular.Column{hypertabular.I32(id, hypercast.Invariant)})
//
// A reader with a plan already — one it was opened with, or one bound before — returns
// ErrAlreadyBound; called by the source from inside a Read, Bind returns ErrBusy. A plan
// the reader cannot honour is this method's error, as it would have been the
// constructor's, and leaves the reader unbound — among them, once a header has been
// read, a column at or past the header's width, where no record has a cell. The rows a
// batch are the BatchRows option the reader was opened with.
func (r *DelimitedReader) Bind(plan []Column) error {
	if r.busy {
		return ErrBusy
	}
	if r.set != nil {
		return ErrAlreadyBound
	}
	var flags uint32
	if r.dialect.LongRows == LongRowsReport {
		flags = batchWidths
	}
	set, err := newColumns(plan, r.batchRows, r.notes, flags)
	if err != nil {
		return err
	}
	set.widths = flags != 0
	if err := r.withinHeader(plan); err != nil {
		return err
	}
	// A cell table entry for every source column up to the widest the plan reads, and one
	// for the row's line.
	// Every ordinal is below 2^20 and the table holds at most cellBudget entries or one
	// row, so its size fits an int even where an int is 32 bits.
	perRow := set.width + 1
	// The core's own word on the plan, before any rows: a fill of no rows still judges
	// every column's door and notation, and touches nothing else — not the header read
	// before it, nor the state the next fill goes on from.
	r.set = set
	r.perRow = perRow
	r.cellsCap = 0
	switch r.nativeFill(nil, false, 0) {
	case codeOK:
	case errGuestMemory:
		r.set = nil
		return guestMemory(len(plan))
	default:
		r.set = nil
		return errors.New("hypertabular: the core refused the plan: a column's door, declared parameter or numeric format is not one it can honour")
	}
	r.batch = Batch{columns: set, core: r.core}
	r.cellsCap = perRow * cellRows(perRow, r.batchRows)
	return nil
}

// setBatchRows is the batches a sampler reads this reader in, ahead of Bind.
func (r *DelimitedReader) setBatchRows(rows int) { r.batchRows = rows }

// sizeFlags is what the core counts of a batch of this reader's beside its plan.
func (r *DelimitedReader) sizeFlags() uint32 {
	var flags uint32
	if r.notes {
		flags |= batchNotes
	}
	if r.dialect.LongRows == LongRowsReport {
		flags |= batchWidths
	}
	return flags
}

// cellBudget is the most cell-table entries a delimited reader sizes its table for: what
// the default 4096-row batch of a plan reaching 256 source columns takes (a row is the
// widest ordinal plus two), about 8 MiB. A plan that reaches further gets a table of fewer
// rows — never fewer than one — and the core fills only as many rows a batch as the table
// holds, so a plan naming a far ordinal of a source with no header costs a row of entries
// that wide, not a batch of them. A plan within 256 source columns is untouched.
const cellBudget = 4096 * 257

// cellRows is the rows a cell table of perRow entries a row is sized for: as many as
// cellBudget holds, at least one and at most batchRows.
func cellRows(perRow, batchRows int) int {
	return min(max(cellBudget/perRow, 1), batchRows)
}

// withinHeader refuses the first plan column reading at or past the width a header has
// fixed: every record has exactly the header's cells, so such a column could only ever be
// empty. A source with no header, or none read yet, has no width to hold a plan to.
func (r *DelimitedReader) withinHeader(plan []Column) error {
	width := r.ColumnCount()
	if !r.dialect.HasHeader || width == 0 {
		return nil
	}
	for index, column := range plan {
		if column.ordinal >= width {
			return fmt.Errorf("hypertabular: plan column %d reads source column %d, past the %d columns the header gives every record",
				index, column.ordinal, width)
		}
	}
	return nil
}

// IsBound says whether the reader has a plan: always, for a reader opened with one.
func (r *DelimitedReader) IsBound() bool { return r.set != nil }

// guestMemory is the error for buffers the core's memory cannot hold: everything a reader
// fills lives in its instance of the core, whose memory WebAssembly caps at 4 GiB (core.go).
func guestMemory(columns int) error {
	return fmt.Errorf("hypertabular: the buffers of a %d-column plan do not fit the core's 4 GiB memory", columns)
}

// begin reads the header, when the dialect declares one.
func (r *DelimitedReader) begin(dialect Dialect) (*DelimitedReader, error) {
	if !dialect.HasHeader {
		return r, nil
	}
	if err := r.readHeader(); err != nil {
		return nil, err
	}
	// A plan the reader was opened with was bound before the header was read; the width
	// the header fixes holds it now, before any row is delivered.
	if r.set != nil {
		if err := r.withinHeader(r.set.plan); err != nil {
			return nil, err
		}
	}
	return r, nil
}

// Header is the header's names, when the dialect declares one: nil when it does not, and
// empty but not nil for an input with no record. It is read when the reader is built, so it
// is there to build a plan from before Bind.
func (r *DelimitedReader) Header() Header { return r.header }

// ColumnCount is how many cells a record has: the header's count once it has been read,
// otherwise the first record's once that has been — and zero while neither is known (a
// headerless input before its first Read, or an input with no record at all).
func (r *DelimitedReader) ColumnCount() int { return int(r.core.state.expected) }

// Dialect is the dialect the text is read in.
func (r *DelimitedReader) Dialect() Dialect { return r.dialect }

// Plan is the plan the text is read through: column i of every batch is Plan()[i]; nil until
// one is bound. Do not modify it.
func (r *DelimitedReader) Plan() []Column {
	if r.set == nil {
		return nil
	}
	return r.batch.Columns()
}

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

// grow doubles the buffer, up to the row ceiling.
func (r *DelimitedReader) grow() error {
	if len(r.buffer) >= r.maxRowBytes {
		return r.rowTooLong()
	}
	larger := make([]byte, min(2*len(r.buffer), r.maxRowBytes))
	copy(larger, r.buffer[:r.end])
	r.buffer = larger
	r.core.forget()
	return nil
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
		r.core.forget() // the bytes the core's copy mirrors move
		copy(r.buffer, r.buffer[r.start:r.end])
		r.start, r.end = 0, pending
	}
	if r.decoder != nil && r.decoder.failed {
		// All the text ahead of the failure has been read, but for what the fill can't finish. A
		// \r it's holding ends a row whatever comes next, and only waits to see if a \n is part of
		// it; nothing is coming, and this says so. In a quote it ends nothing, and the record is
		// as cut as it was.
		if r.end == 0 || r.buffer[r.end-1] != '\r' {
			state := &r.core.state
			return &Failure{Kind: Encoding, Record: int64(state.records), Line: int(state.line), Byte: int64(r.decoder.failedAt)}
		}
		if r.end == len(r.buffer) {
			if err := r.grow(); err != nil {
				return err
			}
		}
		r.buffer[r.end] = '\n'
		r.end++
		return nil
	}
	if r.end == len(r.buffer) {
		// One record fills the buffer: it needs a bigger one.
		if err := r.grow(); err != nil {
			return err
		}
	}
	if r.decoder != nil {
		return r.decode()
	}
	if r.sourceErr != nil {
		return r.sourceErr
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
			wrapped := fmt.Errorf("hypertabular: reading the input: %w", err)
			if n > 0 {
				r.sourceErr = wrapped
				return nil
			}
			return wrapped
		case n > 0:
			return nil
		case empty >= 100:
			return fmt.Errorf("hypertabular: reading the input: %w", io.ErrNoProgress)
		}
	}
}

// maxCharacter is the most bytes of UTF-8 one character takes: with this much room, a transcode
// that writes nothing and doesn't fail wants more input.
const maxCharacter = 4

// decoder is the step ahead of the fill, for a dialect that declares an encoding. The source's
// reads land here, as the original bytes, and go through the core's transcoder into the reader's
// buffer, where the fill reads them as it would have read UTF-8.
type decoder struct {
	encoding uint32
	// raw is the original bytes read, at..end of them not yet decoded. Fixed in size: what's left
	// when more is wanted is less than a character.
	raw []byte
	at  int
	end int
	// eof says the source has ended.
	eof bool
	// offset is the original bytes decoded so far: added to a call's failureAt, this is the input's
	// offset.
	offset uint64
	// failed says a unit has no character, and failedAt where it is in the original input. The
	// failure is raised only when the fill has had all that decoded ahead of it, and wants more.
	failed   bool
	failedAt uint64
}

func newDecoder(encoding TextEncoding, size int) *decoder {
	return &decoder{encoding: uint32(encoding), raw: make([]byte, max(size, maxCharacter))}
}

// decode reads more of the text behind what the buffer holds, as refill does for UTF-8: one turn of
// the transcoder, with whatever it wants first.
func (r *DelimitedReader) decode() error {
	d := r.decoder
	for empty := 0; ; {
		out := r.buffer[r.end:]
		done, ok := r.core.transcode(d.encoding, d.raw[d.at:d.end], d.eof, out)
		if !ok {
			return errors.New("hypertabular: the text being decoded does not fit the core's 4 GiB memory")
		}
		if done.failed != 0 {
			d.failed = true
			d.failedAt = d.offset + done.failureAt
		}
		d.at += int(done.consumed)
		d.offset += done.consumed
		switch {
		case done.written > 0:
			r.end += int(done.written)
			return nil
		case d.failed:
			// The next refill raises it, once the fill has had one more look.
			return nil
		case d.at < d.end && len(out) < maxCharacter:
			if err := r.grow(); err != nil {
				return err
			}
		case d.eof:
			r.eof = true
			return nil
		case r.sourceErr != nil:
			return r.sourceErr
		default:
			copy(d.raw, d.raw[d.at:d.end])
			d.end -= d.at
			d.at = 0
			n, err := r.source.Read(d.raw[d.end:])
			if n < 0 || n > len(d.raw)-d.end {
				return errors.New("hypertabular: the source returned an invalid count from Read")
			}
			d.end += n
			switch {
			case err == io.EOF:
				d.eof = true
			case err != nil:
				wrapped := fmt.Errorf("hypertabular: reading the input: %w", err)
				if n == 0 {
					return wrapped
				}
				r.sourceErr = wrapped
			case n > 0:
				empty = 0
			case empty >= 100:
				return fmt.Errorf("hypertabular: reading the input: %w", io.ErrNoProgress)
			default:
				empty++
			}
		}
	}
}

// readHeader reads the first record as the header.
func (r *DelimitedReader) readHeader() error {
	namesCap := 64
	filled := &r.core.filled
	for {
		window, last := r.next()
		if len(window) == 0 && !last {
			if err := r.refill(); err != nil {
				return err
			}
			continue
		}
		code, names := r.nativeHeader(window, last, namesCap)
		switch code {
		case codeOK:
			consumed := int(filled.consumed)
			r.start += consumed
			if filled.rows > 0 {
				header := make(Header, filled.rows)
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
				r.header = Header{}
				return nil
			}
			if consumed == 0 {
				if err := r.refill(); err != nil {
					return err
				}
			}
		case errCells:
			namesCap = int(filled.needed)
		case errArena:
			r.arenaCap = int(filled.needed)
		case errStructure:
			return structural(filled.failure)
		case errGuestMemory:
			return guestMemory(0)
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
// After Close it returns os.ErrClosed. A reader opened without a plan returns ErrUnbound
// until Bind has given it one. Called from inside a Read — by the io.Reader the reader reads —
// it returns ErrBusy and leaves the outer Read to finish.
func (r *DelimitedReader) Read() (*Batch, error) {
	if r.busy {
		return nil, ErrBusy
	}
	// Cleared by a defer, so that an io.Reader that panics leaves the reader to read on.
	r.busy = true
	defer r.idle()
	return r.read()
}

// idle ends a Read.
func (r *DelimitedReader) idle() { r.busy = false }

func (r *DelimitedReader) read() (*Batch, error) {
	r.batch.clear()
	if r.closed {
		return nil, os.ErrClosed
	}
	if r.set == nil {
		return nil, ErrUnbound
	}
	if r.err != nil {
		return nil, r.err
	}
	if r.cramped {
		r.arenaCap *= 2
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
				// Short of what the cell table holds, not of BatchRows: a table of fewer rows
				// (see cellBudget) ends every batch early, and that is no reason to grow the arena.
				r.cramped = rows < min(r.set.batchRows, r.cellsCap/r.perRow) && filled.arenaUsed*2 >= uint64(r.arenaCap)
				r.batch.expected = r.core.state.expected
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
			r.cellsCap = r.perRow * cellRows(r.perRow, r.set.batchRows)
		case errArena:
			r.arenaCap = max(int(filled.needed), 2*r.arenaCap)
		case errStructure:
			r.err = structural(filled.failure)
			return nil, r.err
		case errGuestMemory:
			r.err = guestMemory(len(r.set.plan))
			return nil, r.err
		default:
			panic(contractViolation)
		}
	}
}

// Close releases the reader: the batch in hand is over, and a file opened by OpenDelimited
// is closed. A reader built over an io.Reader or over memory holds nothing else that needs
// releasing — its instance of the core is ordinary Go memory — and does not close its source.
// Called from inside a Read, by the source, it returns ErrBusy and closes nothing: the Read
// finishes, and Close is for after it.
func (r *DelimitedReader) Close() error {
	if r.busy {
		return ErrBusy
	}
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
