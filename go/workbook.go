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
)

// String names the format: "XLSX" or "ODS".
func (f WorkbookFormat) String() string {
	switch f {
	case XLSX:
		return "XLSX"
	case ODS:
		return "ODS"
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

	// What the core is given back to open it: the XLSX part, or the ODS table's index.
	part  []byte
	index uint32
}

// SheetOptions is how a sheet is read.
type SheetOptions struct {
	// HasHeader says whether the sheet's first row is a header, exposed through
	// Sheet.Header and never delivered as a row.
	HasHeader bool
	// SkipEmptyRows says whether a row with no cells is skipped rather than delivered as a
	// row of empty cells.
	SkipEmptyRows bool
	// BatchRows is the rows per batch — the capacity of every column buffer. It must be
	// positive.
	BatchRows int
}

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

// errWindow is the code a workbook call returns for a window too small to work in.
const errWindow = -5

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

// bookCall is which workbook call nativeBook makes; the values are backend_static.go's.
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
// one for each sheet. All of it Go memory with no Go pointer inside.
type scratch struct {
	window []byte
	arena  []byte
	cells  []rawSpan
	row    []rawSlot
	// rowFollowsCells is whether row grows with cells: while the header of a sheet opened
	// without a plan is read, there is a row slot for every cell the header call is given,
	// because the header row's cells are kept in the slots as well as named (see
	// Sheet.readHeader).
	rowFollowsCells bool
	// filled is where a call reports: its own allocation, so that C is never pointed into
	// a struct that holds Go pointers.
	filled *rawFilled
}

func newScratch(window, arena, cells, row int) *scratch {
	return &scratch{
		window: make([]byte, window),
		arena:  make([]byte, arena),
		cells:  make([]rawSpan, cells),
		row:    make([]rawSlot, row),
		filled: new(rawFilled),
	}
}

// drive makes call until it stops asking for room, growing the buffer it names each time —
// with what the buffer held kept, which is what lets the core go on from where it stopped —
// and returns the code it ended on; what it reported is in s.filled.
func (s *scratch) drive(call bookCall, book *Workbook, state []uint64, set *columns) int32 {
	var tables *Workbook
	if call == bookHeader || call == bookFill {
		tables = book
	}
	for {
		code := nativeBook(call, state, book.container, s, tables, set, unsafe.Pointer(s.filled))
		switch code {
		case errWindow:
			s.window = grownTo(s.window, s.filled.needed)
			grown[0]++
		case errArena:
			s.arena = grownTo(s.arena, s.filled.needed)
			grown[1]++
		case errCells:
			s.cells = grownTo(s.cells, s.filled.needed)
			grown[2]++
			if s.rowFollowsCells {
				s.row = atLeast(s.row, len(s.cells))
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
	case errShimMemory:
		return errors.New("hypertabular: out of memory for a plan's column table")
	default:
		panic(contractViolation)
	}
}

// Workbook is an XLSX or ODS workbook held in memory, its sheets listed and its shared
// strings and styles loaded: what a Sheet reads from.
//
// A workbook is read in place and holds nothing that needs releasing — every buffer is
// ordinary Go memory. Its sheets may be read at once, each with buffers of its own, but
// neither a Workbook nor a Sheet is safe for concurrent use.
type Workbook struct {
	container []byte
	// state is the state as opening left it: the template every sheet's own is copied from.
	state  []uint64
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

// NewWorkbook opens the workbook in container, in place — nothing is copied, so the bytes
// must not change while the workbook or any of its sheets is in use. A container that is
// not a workbook this module can read is a *Failure.
func NewWorkbook(container []byte) (*Workbook, error) {
	size := nativeWorkbookStateSize()
	if size <= 0 {
		panic(contractViolation)
	}
	w := &Workbook{container: container, state: make([]uint64, (size+7)/8)}
	s := newScratch(windowMin, 1024, 64, 0)
	if stingy {
		s = newScratch(1, 1, 1, 0)
	}
	opened, err := w.open(s)
	if err != nil {
		return nil, err
	}
	w.format = XLSX
	if opened.format == 2 {
		w.format = ODS
	}
	epoch, ok := hypercast.ExcelEpochFromCode(opened.epoch)
	if !ok {
		panic(contractViolation)
	}
	w.epoch = epoch

	if err := settled(s.drive(bookSheets, w, w.state, nil), s.filled); err != nil {
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
	if bound := min(opened.stringsBytes, 1<<28); !stingy && uint64(len(s.arena)) < bound {
		s.arena = grownTo(s.arena, bound)
	}
	if err := settled(s.drive(bookStrings, w, w.state, nil), s.filled); err != nil {
		return nil, err
	}
	w.strings = append([]byte(nil), s.arena[:s.filled.arenaUsed]...)
	w.table = append([]rawSpan(nil), s.cells[:s.filled.rows]...)

	if err := settled(s.drive(bookStyles, w, w.state, nil), s.filled); err != nil {
		return nil, err
	}
	w.kinds = append([]byte(nil), s.arena[:s.filled.rows]...)
	return w, nil
}

// open opens the container. Opening starts over when it is refused, and reports through its
// own shape.
func (w *Workbook) open(s *scratch) (*rawOpened, error) {
	opened := new(rawOpened)
	for {
		switch nativeBook(bookOpen, w.state, w.container, s, nil, nil, unsafe.Pointer(opened)) {
		case codeOK:
			return opened, nil
		case errWindow:
			s.window = grownTo(s.window, opened.needed)
		case errArena:
			s.arena = grownTo(s.arena, opened.needed)
		case errStructure:
			return nil, structural(opened.failure)
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
// An index the workbook has no sheet at is an error that wraps ErrNoSheet.
func (w *Workbook) Sheet(index int, options SheetOptions, plan []Column) (*Sheet, error) {
	info, err := w.sheetAt(index)
	if err != nil {
		return nil, err
	}
	return newSheet(w, info, options, plan, true)
}

// SheetUnbound starts a read of the sheet at index of Sheets without a plan: the header is
// read — when the options declare one — and the plan is bound later, with Sheet.Bind, once
// the header has said where each column is. Sheet.Read returns ErrUnbound until then.
func (w *Workbook) SheetUnbound(index int, options SheetOptions) (*Sheet, error) {
	info, err := w.sheetAt(index)
	if err != nil {
		return nil, err
	}
	return newSheet(w, info, options, nil, false)
}

// SheetNamed starts a read of the first sheet named name, as Sheet does. A name the
// workbook has no sheet by is an error that wraps ErrNoSheet.
func (w *Workbook) SheetNamed(name string, options SheetOptions, plan []Column) (*Sheet, error) {
	info, err := w.sheetNamed(name)
	if err != nil {
		return nil, err
	}
	return newSheet(w, info, options, plan, true)
}

// SheetNamedUnbound starts a read of the first sheet named name without a plan, as
// SheetUnbound does.
func (w *Workbook) SheetNamedUnbound(name string, options SheetOptions) (*Sheet, error) {
	info, err := w.sheetNamed(name)
	if err != nil {
		return nil, err
	}
	return newSheet(w, info, options, nil, false)
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
// A sheet has its own buffers and its own copy of the workbook's read state, so several can
// be read at once. A typed workbook cell is converted directly by its door — a stored 42.0
// never passes through text to become an int32 — and a text cell goes through the door as
// delimited text would. A Sheet is not safe for concurrent use.
type Sheet struct {
	book    *Workbook
	options SheetOptions
	// set is the plan and its buffers: nil until one is bound.
	set     *columns
	batch   Batch
	state   []uint64
	scratch *scratch
	// perRow is the cell-table entries one row takes: one per plan column, and one more.
	perRow int
	header Header
	// pending is a failure met with rows before it: those went out first, and this is next.
	pending error
	err     error
}

func newSheet(book *Workbook, info *SheetInfo, options SheetOptions, plan []Column, bound bool) (*Sheet, error) {
	s := &Sheet{
		book:    book,
		options: options,
		// A copy of the opened state, which the core allows, so that this sheet's read is
		// its own.
		state: append([]uint64(nil), book.state...),
	}
	if bound {
		// The plan is bound before the sheet is opened, so that its error comes first, and
		// the buffers are sized to it.
		set, perRow, err := sheetPlan(plan, options.BatchRows)
		if err != nil {
			return nil, err
		}
		s.scratch = newScratch(0, 4096, perRow*options.BatchRows, set.width)
		if stingy {
			s.scratch = newScratch(1, 1, 1, set.width)
		}
		s.attach(set, perRow)
	} else {
		if options.BatchRows <= 0 {
			return nil, fmt.Errorf("hypertabular: BatchRows must be positive, not %d", options.BatchRows)
		}
		// No plan to size the row's slots by: there is one for every cell the header call is
		// given, and they grow with the cell table (see readHeader).
		s.scratch = newScratch(0, 4096, 64, 64)
		if stingy {
			s.scratch = newScratch(1, 1, 1, 1)
		}
	}
	if err := settled(nativeSheet(s.state, book.container, info, options, s.scratch.filled), s.scratch.filled); err != nil {
		return nil, err
	}
	if options.HasHeader {
		if err := s.readHeader(); err != nil {
			return nil, err
		}
	}
	return s, nil
}

// sheetPlan checks a plan for a sheet and allocates its buffers; perRow is the cell-table
// entries one row takes: one per plan column, and one more.
func sheetPlan(plan []Column, batchRows int) (*columns, int, error) {
	set, err := newColumns(plan, batchRows)
	if err != nil {
		return nil, 0, err
	}
	perRow := len(plan) + 1
	// In int64: the bound is 1<<31, which overflows int where int is 32 bits (TinyGo on
	// WebAssembly).
	if int64(perRow) > (1<<31)/int64(batchRows) {
		return nil, 0, fmt.Errorf("hypertabular: a plan of %d columns at %d rows a batch needs too large a cell table",
			len(plan), batchRows)
	}
	return set, perRow, nil
}

// attach makes set the sheet's plan.
func (s *Sheet) attach(set *columns, perRow int) {
	s.set = set
	s.batch = Batch{columns: set, workbook: true}
	s.perRow = perRow
}

// readHeader reads the header row. Its cells are kept in the row's slots as well as named —
// the slots are what a row the sheet repeats (an ODS number-rows-repeated) is delivered again
// from. A plan says how many slots it reads; without one, there is a slot for every cell the
// call is given, grown with the cell table, and binding a plan later keeps them.
func (s *Sheet) readHeader() error {
	if s.set == nil {
		s.scratch.row = atLeast(s.scratch.row, len(s.scratch.cells))
		s.scratch.rowFollowsCells = true
	}
	code := s.scratch.drive(bookHeader, s.book, s.state, nil)
	s.scratch.rowFollowsCells = false
	if err := settled(code, s.scratch.filled); err != nil {
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
	set, perRow, err := sheetPlan(plan, s.options.BatchRows)
	if err != nil {
		return err
	}
	// The cell table a batch needs — unless the tests want every buffer to start too small —
	// and a slot for every column the plan reads. What the slots hold, a header row still to
	// be repeated, is kept: they are grown, never replaced or shrunk.
	if !stingy {
		s.scratch.cells = atLeast(s.scratch.cells, perRow*s.options.BatchRows)
	}
	s.scratch.row = atLeast(s.scratch.row, set.width)
	s.attach(set, perRow)
	return nil
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
	code := s.scratch.drive(bookFill, s.book, s.state, s.set)
	rows := int(s.scratch.filled.rows)
	if code != codeOK {
		failure := settled(code, s.scratch.filled)
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
