package hypertabular

import (
	"errors"
	"fmt"
	"io"
	"os"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// WorkbookFormat is which kind of workbook a Workbook is.
type WorkbookFormat int

const (
	// XLSX is Office Open XML: .xlsx, .xlsm.
	XLSX WorkbookFormat = 1
	// ODS is OpenDocument: .ods.
	ODS WorkbookFormat = 2
	// Numbers is Apple Numbers: .numbers, each table a sheet.
	Numbers WorkbookFormat = 3
)

// String names the format: "XLSX", "ODS" or "Numbers".
func (f WorkbookFormat) String() string {
	switch f {
	case XLSX:
		return "XLSX"
	case ODS:
		return "ODS"
	case Numbers:
		return "Numbers"
	default:
		return fmt.Sprintf("WorkbookFormat(%d)", int(f))
	}
}

// SheetInfo is one sheet of a workbook, as Workbook.Sheets lists it.
type SheetInfo struct {
	// Name is the sheet's name.
	Name string
	// Hidden says whether the workbook hides the sheet.
	Hidden bool

	// What the core is given back to open it: the XLSX part, or the ODS or Numbers table's index.
	part  []byte
	index uint32
}

// SheetOptions is how a sheet is read.
type SheetOptions struct {
	// HasHeader says whether the sheet's first row is a header, exposed through
	// Sheet.Header and never delivered as a row.
	HasHeader bool
	// SkipEmptyRows says whether a row with no cells is skipped rather than delivered as a
	// row of empty cells. Set it to false to keep every row: on, a row whose only cells are formulas
	// with no cached value is left out too, and nothing says so.
	SkipEmptyRows bool
	// BatchRows is the rows per batch — the capacity of every column buffer. A struct can't tell
	// 0 from a field left out, so 0 here is DefaultBatchRows; the BatchRows option can, and refuses 0.
	// The core refuses a batch asking for more memory than it allows.
	BatchRows int
	// Skip is the rows of the sheet to pass over at the start of the range, empty ones included.
	// Their cells aren't judged, so a bad cell there doesn't matter.
	Skip uint32
	// HeaderRow is the rows to read and discard after those, as SkipEmptyRows delivers them, so
	// with it on an empty row doesn't count.
	HeaderRow uint32
	// Range is the part of the sheet to read; the zero Range is all of it.
	Range Range
	// Tokens is what stands for no value in every column without tokens of its own: text cells
	// alone can be one.
	Tokens Tokens
}

// ErrRange is the error ParseRange and NewRange wrap for what isn't a range; reach it with
// errors.Is.
var ErrRange = errors.New("hypertabular: not a range")

// Range is the part of a sheet a read keeps: its corners, 1-based like the A5:F200 they're written
// as, and included, a 0 meaning no limit on that side. The first column is ordinal 0 of a plan,
// while a cell's line and source column stay the sheet's own. A range that runs past the sheet's
// end reads what's there, and adds no rows of its own. Comparable.
type Range struct {
	firstRow, firstColumn, lastRow, lastColumn uint32
}

// ParseRange reads A5:F200: both corners, in capitals, the second no less than the first in either
// direction. The core reads it, so every binding agrees on what one is. Anything else, A5 alone,
// a5:f200 or F200:A5, is an error that wraps ErrRange.
func ParseRange(text string) (Range, error) {
	first, firstColumn, last, lastColumn, ok := nativeRangeParse(text)
	if !ok {
		return Range{}, fmt.Errorf("%w: %q", ErrRange, text)
	}
	return Range{first, firstColumn, last, lastColumn}, nil
}

// NewRange is a range from its four numbers, each 1-based, with 0 for no limit on that side. That's
// how to say what A5:F200 can't: every row from the fifth, or columns B to D of all of them. A last
// before its first is an error that wraps ErrRange.
func NewRange(firstRow, firstColumn, lastRow, lastColumn uint32) (Range, error) {
	inverted := func(first, last uint32) bool { return first != 0 && last != 0 && first > last }
	if inverted(firstRow, lastRow) || inverted(firstColumn, lastColumn) {
		return Range{}, fmt.Errorf("%w: rows %d to %d, columns %d to %d", ErrRange, firstRow, lastRow, firstColumn, lastColumn)
	}
	return Range{firstRow, firstColumn, lastRow, lastColumn}, nil
}

// FirstRow is the first row, or 0 for no limit.
func (r Range) FirstRow() uint32 { return r.firstRow }

// FirstColumn is the first column, A being 1, or 0 for no limit.
func (r Range) FirstColumn() uint32 { return r.firstColumn }

// LastRow is the last row, or 0 for no limit.
func (r Range) LastRow() uint32 { return r.lastRow }

// LastColumn is the last column, or 0 for no limit.
func (r Range) LastColumn() uint32 { return r.lastColumn }

// DefaultSheetOptions reads a header, skips empty rows, and reads DefaultBatchRows rows a
// batch.
var DefaultSheetOptions = SheetOptions{HasHeader: true, SkipEmptyRows: true, BatchRows: DefaultBatchRows}

// ErrNoSheet is the error Workbook.Sheet and Workbook.SheetNamed wrap for a sheet the
// workbook does not have; reach it with errors.Is.
var ErrNoSheet = errors.New("hypertabular: no such sheet")

// stingy is for the tests: every buffer a workbook call works in starts with room for one
// element and no shared-strings bound is asked for, so every call that can stop and resume
// does — the grow-and-keep path exercised mid-part. grown counts the window's, the arena's
// and the cell table's growth.
var (
	stingy bool
	grown  [3]int
)

// windowMin is the smallest window the core takes.
const windowMin = 64 * 1024

// The workbook shapes that cross the ABI (rust/src/kernel/abi.rs), field for field.

type rawSlot struct {
	tag  uint32
	aux  uint32
	bits uint64
}

type rawOpened struct {
	format       uint32
	epoch        uint32
	stringsBytes uint64
	stringsCount uint64
	needed       uint64
	failure      rawFailure
}

type (
	_ [unsafe.Sizeof(rawSlot{}) - 16]struct{}
	_ [16 - unsafe.Sizeof(rawSlot{})]struct{}
	_ [unsafe.Sizeof(rawOpened{}) - 64]struct{}
	_ [64 - unsafe.Sizeof(rawOpened{})]struct{}
)

// bookCall is which workbook call scratch.book makes.
type bookCall int

const (
	bookOpen bookCall = iota
	bookSheets
	bookStrings
	bookStyles
	bookHeader
	bookFill
)

// scratch is the memory a workbook call works in: one set for a workbook while it opens,
// one for each sheet, each in an instance of the core of its own (core.go), which holds
// the buffers and the container. window, arenaCap, cellsCap and row are the elements the
// core is told each buffer holds; arena and cells are views of the last call's.
type scratch struct {
	core     *native
	window   int
	arenaCap int
	cellsCap int
	row      int
	arena    []byte
	cells    []rawSpan
	// rowFollowsCells is whether row grows with cells: while the header of a sheet opened
	// without a plan is read, there is a row slot for every cell the header call is given,
	// because the header row's cells are kept in the slots as well as named (see
	// Sheet.readHeader).
	rowFollowsCells bool
	// filled is what the last call reported.
	filled rawFilled
}

func newScratch(core *native, window, arena, cells, row int) *scratch {
	return &scratch{core: core, window: window, arenaCap: arena, cellsCap: cells, row: row}
}

// drive makes call until it stops asking for room, growing the buffer it names each time —
// with what the buffer held kept, which is what lets the core go on from where it stopped —
// and returns the code it ended on; what it reported is in s.filled, and the cells of the
// rows it reported, stride entries a row, in s.cells.
func (s *scratch) drive(call bookCall, set *columns, stride int) int32 {
	for {
		code := s.book(call, set, nil, stride)
		switch code {
		case errWindow:
			s.window = grownTo(s.window, s.filled.needed)
			grown[0]++
		case errArena:
			s.arenaCap = grownTo(s.arenaCap, s.filled.needed)
			grown[1]++
		case errCells:
			s.cellsCap = grownTo(s.cellsCap, s.filled.needed)
			grown[2]++
			if s.rowFollowsCells {
				s.row = max(s.row, s.cellsCap)
			}
		default:
			return code
		}
	}
}

// settled is what a call's code means: done, or a structural failure. Anything else is
// this module's bug.
func settled(code int32, filled *rawFilled) error {
	switch code {
	case codeOK:
		return nil
	case errStructure:
		return structural(filled.failure)
	case errGuestMemory:
		return errWorkbookMemory
	default:
		panic(contractViolation)
	}
}

// errWorkbookMemory is the error for a workbook whose buffers the core's memory cannot hold.
var errWorkbookMemory = errors.New("hypertabular: the workbook's buffers do not fit the core's 4 GiB memory")

// Workbook is an XLSX, ODS or Numbers workbook held in memory, its sheets listed and its shared
// strings and styles loaded: what a Sheet reads from.
//
// A workbook holds nothing that needs releasing — every buffer is ordinary Go memory. Its
// sheets may be read at once, each with buffers of its own and its own instance of the core,
// which holds a copy of the container and of the workbook's shared strings: a sheet costs
// that memory while it is in use. Neither a Workbook nor a Sheet is safe for concurrent use.
type Workbook struct {
	container []byte
	// state is the state block as opening left it: the template every sheet's own is copied
	// from.
	state  []byte
	format WorkbookFormat
	epoch  hypercast.ExcelEpoch
	sheets []SheetInfo

	// The workbook's tables, which every read of a sheet is handed.
	strings []byte
	table   []rawSpan
	kinds   []byte
}

// NewWorkbookReader reads source to its end into memory the workbook owns, and opens it —
// for a workbook that is not a file or a []byte already: an embedded file, an HTTP response
// body, an archive member. The format is told from the bytes. The workbook does not close
// source; it is done with it when this function returns.
func NewWorkbookReader(source io.Reader) (*Workbook, error) {
	if source == nil {
		return nil, errors.New("hypertabular: the source is nil")
	}
	container, err := io.ReadAll(source)
	if err != nil {
		return nil, fmt.Errorf("hypertabular: reading the workbook: %w", err)
	}
	return NewWorkbook(container)
}

// OpenWorkbook reads the file at path into memory and opens it.
func OpenWorkbook(path string) (*Workbook, error) {
	container, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return NewWorkbook(container)
}

// NewWorkbook opens the workbook in container. The core reads a copy of it, made as it
// opens and as each sheet starts; the bytes must not change meanwhile. A container that is
// not a workbook this module can read is a *Failure.
func NewWorkbook(container []byte) (*Workbook, error) {
	size := nativeWorkbookStateSize()
	if size <= 0 {
		panic(contractViolation)
	}
	w := &Workbook{container: container, state: make([]byte, size)}
	core, ok := newBookNative(w, false)
	if !ok {
		return nil, errWorkbookMemory
	}
	s := newScratch(core, windowMin, 1024, 64, 0)
	if stingy {
		s = newScratch(core, 1, 1, 1, 0)
	}
	opened, err := w.open(s)
	if err != nil {
		return nil, err
	}
	switch opened.format {
	case 2:
		w.format = ODS
	case 3:
		w.format = Numbers
	default:
		w.format = XLSX
	}
	epoch, ok := hypercast.ExcelEpochFromCode(opened.epoch)
	if !ok {
		panic(contractViolation)
	}
	w.epoch = epoch

	if err := settled(s.drive(bookSheets, nil, 3), &s.filled); err != nil {
		return nil, err
	}
	w.sheets = make([]SheetInfo, s.filled.rows)
	for index := range w.sheets {
		name, part, last := s.cells[3*index], s.cells[3*index+1], s.cells[3*index+2]
		w.sheets[index] = SheetInfo{
			Name:   string(s.arena[name.offset : int(name.offset)+name.length()]),
			Hidden: last.offset&1 != 0,
			part:   append([]byte(nil), s.arena[part.offset:int(part.offset)+part.length()]...),
			index:  last.len,
		}
	}

	// The shared strings take no more room than their part inflates to; asking for it once
	// saves growing into it.
	if bound := min(opened.stringsBytes, 1<<28); !stingy && uint64(s.arenaCap) < bound {
		s.arenaCap = grownTo(s.arenaCap, bound)
	}
	if err := settled(s.drive(bookStrings, nil, 1), &s.filled); err != nil {
		return nil, err
	}
	w.strings = append([]byte(nil), s.arena[:s.filled.arenaUsed]...)
	w.table = append([]rawSpan(nil), s.cells[:s.filled.rows]...)

	if err := settled(s.drive(bookStyles, nil, 0), &s.filled); err != nil {
		return nil, err
	}
	w.kinds = append([]byte(nil), s.arena[:s.filled.rows]...)
	w.state = core.bookState()
	return w, nil
}

// open opens the container. Opening starts over when it is refused, and reports through its
// own shape.
func (w *Workbook) open(s *scratch) (*rawOpened, error) {
	opened := new(rawOpened)
	for {
		switch s.book(bookOpen, nil, opened, 0) {
		case codeOK:
			return opened, nil
		case errWindow:
			s.window = grownTo(s.window, opened.needed)
		case errArena:
			s.arenaCap = grownTo(s.arenaCap, opened.needed)
		case errStructure:
			return nil, structural(opened.failure)
		case errGuestMemory:
			return nil, errWorkbookMemory
		default:
			panic(contractViolation)
		}
	}
}

// Format is which kind of workbook this is.
func (w *Workbook) Format() WorkbookFormat { return w.format }

// DateSystem is the date system the workbook's serials count in: what a date-formatted
// number is read by.
func (w *Workbook) DateSystem() hypercast.ExcelEpoch { return w.epoch }

// Sheets is the workbook's sheets, in its own order. Sheets that hold no cells (chart
// sheets, macro sheets) are not among them. Do not modify it.
func (w *Workbook) Sheets() []SheetInfo { return w.sheets[:len(w.sheets):len(w.sheets)] }

// Sheet starts a read of the sheet at index of Sheets through plan. When the options declare
// a header it is read here, so a header that is structurally broken is this method's error.
// An index the workbook has no sheet at is an error that wraps ErrNoSheet. Notes and BatchRows
// reach the sheet from opts, BatchRows in place of the options' own. A sheet has no buffer for
// BufferBytes to set, and its 0 is refused here as on a reader.
func (w *Workbook) Sheet(index int, options SheetOptions, plan []Column, opts ...Option) (*Sheet, error) {
	info, err := w.sheetAt(index)
	if err != nil {
		return nil, err
	}
	return newSheet(w, info, options, plan, true, opts)
}

// SheetUnbound starts a read of the sheet at index of Sheets without a plan: the header is
// read — when the options declare one — and the plan is bound later, with Sheet.Bind, once
// the header has said where each column is. Sheet.Read returns ErrUnbound until then.
func (w *Workbook) SheetUnbound(index int, options SheetOptions, opts ...Option) (*Sheet, error) {
	info, err := w.sheetAt(index)
	if err != nil {
		return nil, err
	}
	return newSheet(w, info, options, nil, false, opts)
}

// SheetNamed starts a read of the first sheet named name, as Sheet does. A name the
// workbook has no sheet by is an error that wraps ErrNoSheet.
func (w *Workbook) SheetNamed(name string, options SheetOptions, plan []Column, opts ...Option) (*Sheet, error) {
	info, err := w.sheetNamed(name)
	if err != nil {
		return nil, err
	}
	return newSheet(w, info, options, plan, true, opts)
}

// SheetNamedUnbound starts a read of the first sheet named name without a plan, as
// SheetUnbound does.
func (w *Workbook) SheetNamedUnbound(name string, options SheetOptions, opts ...Option) (*Sheet, error) {
	info, err := w.sheetNamed(name)
	if err != nil {
		return nil, err
	}
	return newSheet(w, info, options, nil, false, opts)
}

func (w *Workbook) sheetAt(index int) (*SheetInfo, error) {
	if index < 0 || index >= len(w.sheets) {
		return nil, fmt.Errorf("%w: the workbook has %d sheets, and no sheet %d", ErrNoSheet, len(w.sheets), index)
	}
	return &w.sheets[index], nil
}

func (w *Workbook) sheetNamed(name string) (*SheetInfo, error) {
	for index := range w.sheets {
		if w.sheets[index].Name == name {
			return &w.sheets[index], nil
		}
	}
	return nil, fmt.Errorf("%w: the workbook has no sheet named %q", ErrNoSheet, name)
}

// Sheet is a forward-only read of one sheet of a Workbook, a batch at a time, through a
// plan — into the same Batch delimited text is read into.
//
// A sheet has its own buffers, its own copy of the workbook's read state and its own
// instance of the core, so several can be read at once. A typed workbook cell is converted directly by its door — a stored 42.0
// never passes through text to become an int32 — and a text cell goes through the door as
// delimited text would. A Sheet is not safe for concurrent use.
type Sheet struct {
	book    *Workbook
	info    *SheetInfo
	options SheetOptions
	notes   bool
	// set is the plan and its buffers: nil until one is bound.
	set     *columns
	batch   Batch
	scratch *scratch
	// perRow is the cell-table entries one row takes: one per plan column, and one more.
	perRow int
	header Header
	// pending is a failure met with rows before it: those went out first, and this is next.
	pending error
	err     error
}

func newSheet(book *Workbook, info *SheetInfo, declared SheetOptions, plan []Column, bound bool, opts []Option) (*Sheet, error) {
	o := options{batchRows: declared.BatchRows, bufferBytes: DefaultBufferBytes, notes: true}
	if o.batchRows == 0 {
		o.batchRows = DefaultBatchRows
	}
	for _, opt := range opts {
		opt(&o)
	}
	declared.BatchRows = o.batchRows
	// The core's answer on the size, ahead of the instance and its copy of the container. With no
	// plan yet it can only judge what a sheet holds whatever the plan; bind asks again with one.
	flags := batchSheet
	if o.notes {
		flags |= batchNotes
	}
	if _, err := checkSizes(nil, nil, o.batchRows, 1, flags); err != nil {
		return nil, err
	}
	// A sheet has no input buffer and the core doesn't judge one, but this option is shared with a
	// reader, so it's asked as a reader's.
	if _, err := checkSizes(nil, nil, 1, o.bufferBytes, 0); err != nil {
		return nil, err
	}
	s := &Sheet{book: book, info: info, options: declared, notes: o.notes}
	// An instance of the sheet's own, with a copy of the opened state, which the core
	// allows, so that this sheet's read is its own.
	core, ok := newBookNative(book, true)
	if !ok || !core.putTokens(declared.Tokens) {
		return nil, errWorkbookMemory
	}
	if bound {
		// The plan is bound before the sheet is opened, so that its error comes first, and
		// the buffers are sized to it.
		set, perRow, err := s.plan(plan)
		if err != nil {
			return nil, err
		}
		s.scratch = newScratch(core, 0, 4096, perRow*declared.BatchRows, set.width)
		if stingy {
			s.scratch = newScratch(core, 1, 1, 1, set.width)
		}
		s.attach(set, perRow)
	} else {
		// No plan to size the row's slots by: there is one for every cell the header call is
		// given, and they grow with the cell table (see readHeader).
		s.scratch = newScratch(core, 0, 4096, 64, 64)
		if stingy {
			s.scratch = newScratch(core, 1, 1, 1, 1)
		}
	}
	if err := settled(s.scratch.sheet(info, declared), &s.scratch.filled); err != nil {
		return nil, err
	}
	if declared.HasHeader {
		if err := s.readHeader(); err != nil {
			return nil, err
		}
	}
	return s, nil
}

// plan checks a plan for the sheet and allocates its buffers; perRow is the cell-table
// entries one row takes: one per plan column, and one more.
func (s *Sheet) plan(plan []Column) (*columns, int, error) {
	batchRows := s.options.BatchRows
	set, err := newColumns(plan, batchRows, s.notes, batchSheet)
	if err != nil {
		return nil, 0, err
	}
	// The core has allowed the cell table, so this fits an int even where an int is 32 bits.
	return set, len(plan) + 1, nil
}

// attach makes set the sheet's plan.
func (s *Sheet) attach(set *columns, perRow int) {
	s.set = set
	first := int(s.options.Range.firstColumn)
	s.batch = Batch{columns: set, workbook: true, core: s.scratch.core, firstColumn: max(first-1, 0)}
	s.perRow = perRow
}

// readHeader reads the header row. Its cells are kept in the row's slots as well as named —
// the slots are what a row the sheet repeats (an ODS number-rows-repeated) is delivered again
// from. A plan says how many slots it reads; without one, there is a slot for every cell the
// call is given, grown with the cell table, and binding a plan later keeps them.
func (s *Sheet) readHeader() error {
	if s.set == nil {
		s.scratch.row = max(s.scratch.row, s.scratch.cellsCap)
		s.scratch.rowFollowsCells = true
	}
	code := s.scratch.drive(bookHeader, nil, 1)
	s.scratch.rowFollowsCells = false
	if err := settled(code, &s.scratch.filled); err != nil {
		return err
	}
	s.header = make(Header, s.scratch.filled.rows)
	for index := range s.header {
		name := s.scratch.cells[index]
		from := s.book.strings
		if name.flagged() {
			from = s.scratch.arena
		}
		s.header[index] = string(from[name.offset : int(name.offset)+name.length()])
	}
	return nil
}

// Options is the options the sheet is read with.
func (s *Sheet) Options() SheetOptions { return s.options }

// Bind declares the plan a sheet opened without one (Workbook.SheetUnbound,
// Workbook.SheetNamedUnbound) reads through — once, before the first Read: DelimitedReader.Bind,
// for a sheet. A sheet with a plan already returns ErrAlreadyBound; a plan that cannot be
// honoured is this method's error and leaves the sheet unbound. The rows a batch are the
// options' BatchRows.
func (s *Sheet) Bind(plan []Column) error {
	if s.set != nil {
		return ErrAlreadyBound
	}
	set, perRow, err := s.plan(plan)
	if err != nil {
		return err
	}
	// The cell table a batch needs — unless the tests want every buffer to start too small —
	// and a slot for every column the plan reads. What the slots hold, a header row still to
	// be repeated, is kept: they are grown, never replaced or shrunk.
	if !stingy {
		s.scratch.cellsCap = max(s.scratch.cellsCap, perRow*s.options.BatchRows)
	}
	s.scratch.row = max(s.scratch.row, set.width)
	s.attach(set, perRow)
	return nil
}

// setBatchRows is the batches a sampler reads this sheet in, ahead of Bind.
func (s *Sheet) setBatchRows(rows int) { s.options.BatchRows = rows }

// sizeFlags is what the core counts of a batch of this sheet's beside its plan.
func (s *Sheet) sizeFlags() uint32 {
	flags := batchSheet
	if s.notes {
		flags |= batchNotes
	}
	return flags
}

// IsBound says whether the sheet has a plan: always, for a sheet opened with one.
func (s *Sheet) IsBound() bool { return s.set != nil }

// Plan is the plan the sheet is read through: column i of every batch is Plan()[i]; nil
// until one is bound. Do not modify it.
func (s *Sheet) Plan() []Column {
	if s.set == nil {
		return nil
	}
	return s.batch.Columns()
}

// Header is the header row's names — a typed cell said the way the text door says it — or
// nil when the options declare no header. A header with no names is a sheet with no rows.
// It is read when the sheet is opened, so it is there to build a plan from before Bind.
func (s *Sheet) Header() Header { return s.header }

// Read reads the next batch: up to the options' BatchRows rows. At the end of the sheet it
// returns nil and io.EOF. The batch is valid until the next Read.
//
// When the sheet is structurally broken it returns nil and a *Failure — after every intact
// row before the break has been delivered — and the same *Failure on every later call. A
// sheet opened without a plan returns ErrUnbound until Bind has given it one.
func (s *Sheet) Read() (*Batch, error) {
	s.batch.clear()
	if s.set == nil {
		return nil, ErrUnbound
	}
	if s.pending != nil {
		s.err, s.pending = s.pending, nil
	}
	if s.err != nil {
		return nil, s.err
	}
	code := s.scratch.drive(bookFill, s.set, s.perRow)
	rows := int(s.scratch.filled.rows)
	if code != codeOK {
		failure := settled(code, &s.scratch.filled)
		if rows == 0 {
			s.err = failure
			return nil, failure
		}
		s.pending = failure
	}
	if rows == 0 {
		s.err = io.EOF
		return nil, io.EOF
	}
	return s.batch.fill(rows, s.scratch.cells, s.perRow, s.book.strings, s.scratch.arena), nil
}
