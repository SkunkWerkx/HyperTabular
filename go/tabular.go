// Package hypertabular reads delimited text — CSV, TSV, any single-byte ASCII separator — and
// workbooks — XLSX and ODS — a batch at a time into typed columns, with a HyperCast verdict
// for every cell.
//
// The native core (libhypertabular) owns no memory and reads no files. It is handed a chunk
// of input and the buffers to fill, casts each plan column in one native loop, and says how
// many rows it wrote and how far it got. Everything else is here: a DelimitedReader or a
// Sheet allocates the column buffers, the table that locates each cell and the arena for the
// rare text that is not in the input — once, as ordinary Go memory, reused for every batch.
// The cgo boundary is crossed once per batch, not once per cell.
//
//   - One batch type. Read returns a *Batch — Rows, Columns, Line, Verdicts, a whole column
//     at a time (I32, F64, Text and the rest), and Get for any one cell — for delimited text
//     and for a sheet alike.
//   - Nothing is sniffed. The Dialect states the separator, the quoting and the header;
//     SheetOptions states a sheet's header and whether empty rows are skipped; the plan
//     states each column's door and, for numbers, its hypercast.NumFormat.
//   - HyperCast is the judge. Fault, CastFailure, NumFormat, Decimal, Date, CivilDateTime,
//     Duration, UnixPrecision, DateOrder and ExcelEpoch are HyperCast's own types, from its
//     own module. A text cell means exactly what the HyperCast door of the same name would
//     say of the same text; a typed workbook cell is converted by the door directly.
//   - A bad value is a verdict; a broken file is an error. A cell that does not cast is a
//     fault in its column and the read goes on. A record of the wrong width, input that ends
//     inside a quoted cell, a workbook whose container or parts cannot be read, is a
//     *Failure, returned after every intact row before it.
//   - Text is not copied. A Text column's cells are slices of the input itself, or of a
//     workbook's shared strings; only a cell with "" inside, or a typed workbook cell said as
//     text, is written to an arena.
//
// The core is a static library linked in through cgo (backend_static.go), as HyperCast's is:
// nothing is embedded, extracted or loaded at run time. It builds on Linux, macOS and
// Windows, on amd64 and arm64, with a C compiler present. There is no WebAssembly backend —
// the tabular layer is out of scope there — so a TinyGo build, like a build without cgo,
// is a compile error that says so (unsupported.go, unsupported_tinygo.go).
package hypertabular

import (
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	"github.com/google/uuid"
)

// NativeVersion reports the linked core's own version as "major.minor.patch" — read from the
// core itself, not from this module — so a deployment can confirm which build of the archive
// went into the binary. The core is linked in, so there is nothing to load and the probe
// cannot fail.
func NativeVersion() string {
	return hypercast.FormatVersion(packedVersion())
}

// The core's return codes (rust/src/kernel/abi.rs), and the two the C shims add.
const (
	codeOK        = 0
	errContract   = -1
	errStructure  = -2
	errArena      = -3
	errCells      = -4
	errStateSize  = -100 // the linked core's state block is not the one this module mirrors
	errShimMemory = -101 // the fill shim could not allocate its column table
)

const contractViolation = "hypertabular: libhypertabular reported a contract violation — a binding bug, please report it"

// spanFlag is the top bit of a span's length. On a cell-table entry: the cell is quoted
// with "" inside and has to be unescaped to be read. On a Text value: the bytes are in the
// arena rather than in the input.
const spanFlag = 1 << 31

// The #[repr(C)] shapes that cross the ABI (rust/src/kernel/abi.rs), field for field. Every
// one is free of Go pointers, which is what lets a pointer to it be handed to C as it is.

type rawSpan struct {
	offset uint32
	len    uint32
}

func (s rawSpan) length() int   { return int(s.len &^ spanFlag) }
func (s rawSpan) flagged() bool { return s.len&spanFlag != 0 }

// ColumnSpec: the ordinal, the door, what the door declares, and HyperCast's RawNumFormat.
type rawSpec struct {
	ordinal uint32
	door    uint32
	param   uint32
	format  hypercast.RawNumFormat
}

type rawFailure struct {
	code     uint32
	line     uint32
	record   uint64
	byte     uint64
	expected uint32
	found    uint32
}

type rawFilled struct {
	rows      uint64
	consumed  uint64
	arenaUsed uint64
	needed    uint64
	failure   rawFailure
}

// The core's state block (kernel::delimited::fill::State): where the input stands. The core
// writes it; this package reads the position out of it. Its size is checked against the
// linked core's own on every init (backend_static.go's ht_init).
type rawState struct {
	separator      uint8
	quoting        uint8
	skipBlankLines uint8
	engine         uint8
	line           uint32
	records        uint64
	offset         uint64
	expected       uint32
	started        uint8
	_              [3]uint8
	failure        rawFailure
}

// The layouts above held to the sizes the ABI gives them. Each pair of arrays compiles only
// when the two numbers are equal: one of the lengths is negative otherwise. The values the
// core writes are HyperCast's: its raw shapes, and its Decimal and Duration, which it lays
// out as the core does and checks itself.
type (
	_ [unsafe.Sizeof(rawSpan{}) - 8]struct{}
	_ [8 - unsafe.Sizeof(rawSpan{})]struct{}
	_ [unsafe.Sizeof(rawSpec{}) - 44]struct{}
	_ [44 - unsafe.Sizeof(rawSpec{})]struct{}
	_ [unsafe.Sizeof(rawFailure{}) - 32]struct{}
	_ [32 - unsafe.Sizeof(rawFailure{})]struct{}
	_ [unsafe.Sizeof(rawFilled{}) - 64]struct{}
	_ [64 - unsafe.Sizeof(rawFilled{})]struct{}
	_ [unsafe.Sizeof(rawState{}) - 64]struct{}
	_ [64 - unsafe.Sizeof(rawState{})]struct{}
	_ [unsafe.Sizeof(CellVerdict{}) - 12]struct{}
	_ [12 - unsafe.Sizeof(CellVerdict{})]struct{}
	// uuid.UUID is the sixteen bytes the core writes, in RFC 9562 order.
	_ [unsafe.Sizeof(uuid.UUID{}) - 16]struct{}
	_ [16 - unsafe.Sizeof(uuid.UUID{})]struct{}
)
