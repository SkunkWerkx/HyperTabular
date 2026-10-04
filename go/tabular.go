// Package hypertabular reads delimited text — CSV, TSV, any single-byte ASCII separator — a
// batch at a time into typed columns, with a HyperCast verdict for every cell.
//
// The native core (libhypertabular) owns no memory and reads no files. It is handed a chunk
// of input and the buffers to fill, casts each plan column in one native loop, and says how
// many rows it wrote and how many bytes it is finished with. Everything else is here: a
// DelimitedReader allocates the input buffer, the column buffers, the table that locates
// each cell and the arena for the rare escaped cell — once, as ordinary Go memory, reused
// for every batch — and puts what the core did not consume back in front of it. The cgo
// boundary is crossed once per batch, not once per cell.
//
//   - Nothing is sniffed. The Dialect states the separator, the quoting and the header; the
//     plan states each column's door and, for numbers, its hypercast.NumFormat.
//   - HyperCast is the judge. Fault, CastFailure, NumFormat, Decimal, Date, CivilDateTime,
//     Duration, UnixPrecision, DateOrder and ExcelEpoch are HyperCast's own types, from its
//     own module. A cell means exactly what the HyperCast door of the same name would say of
//     the same text.
//   - A bad value is a verdict; a broken file is an error. A cell that does not cast is a
//     fault in its column and the read goes on. A record of the wrong width, or input that
//     ends inside a quoted cell, is a *Failure, returned after every intact row before it.
//   - Text is not copied. A Text column's cells are slices of the input itself; only a cell
//     with "" inside is unescaped, into an arena.
//
// The core is a static library linked in through cgo (backend_static.go), as HyperCast's is:
// nothing is embedded, extracted or loaded at run time. It builds on Linux, macOS and
// Windows, on amd64 and arm64, with a C compiler present. There is no WebAssembly backend —
// the tabular layer is out of scope there — so a TinyGo build, like a build without cgo,
// is a compile error that says so (unsupported.go, unsupported_tinygo.go).
package hypertabular

import (
	"fmt"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	"github.com/google/uuid"
)

// NativeVersion reports the linked core's own version as "major.minor.patch" — read from the
// core itself, not from this module — so a deployment can confirm which build of the archive
// went into the binary. The core is linked in, so there is nothing to load and the probe
// cannot fail.
func NativeVersion() string {
	v := packedVersion()
	return fmt.Sprintf("%d.%d.%d", v>>16, (v>>8)&0xFF, v&0xFF)
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
	ordinal     uint32
	door        uint32
	param       uint32
	decimalSep  uint32
	groupSep    uint32
	flags       uint32
	currencyLen uint32
	currency    [currencyMaxBytes]byte
}

// currencyMaxBytes is the inline capacity of the ABI's currency symbol, in UTF-8 bytes.
const currencyMaxBytes = 16

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

// The values the core writes for the doors whose Go type is not the core's own layout.

type rawTimestamp struct {
	seconds int64
	nanos   int32
	_       int32
}

type rawDate struct {
	year  uint16
	month uint8
	day   uint8
}

type rawCivil struct {
	date       rawDate
	_          [4]byte
	nanosOfDay uint64
}

// The layouts above, and the HyperCast types this package hands the core's bytes out as
// without converting them, held to the sizes and offsets the ABI gives them. Each pair of
// arrays compiles only when the two numbers are equal: one of the lengths is negative
// otherwise.
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
	_ [unsafe.Sizeof(rawTimestamp{}) - 16]struct{}
	_ [16 - unsafe.Sizeof(rawTimestamp{})]struct{}
	_ [unsafe.Sizeof(rawDate{}) - 4]struct{}
	_ [4 - unsafe.Sizeof(rawDate{})]struct{}
	_ [unsafe.Sizeof(rawCivil{}) - 16]struct{}
	_ [16 - unsafe.Sizeof(rawCivil{})]struct{}
	_ [unsafe.Offsetof(rawCivil{}.nanosOfDay) - 8]struct{}
	_ [8 - unsafe.Offsetof(rawCivil{}.nanosOfDay)]struct{}

	// hypercast.Decimal is the core's Decimal: lo at 0, hi at 8, scale at 12, negative (a
	// 0/1 byte, which is what a Go bool is) at 13, 16 bytes.
	_ [unsafe.Sizeof(hypercast.Decimal{}) - 16]struct{}
	_ [16 - unsafe.Sizeof(hypercast.Decimal{})]struct{}
	_ [unsafe.Offsetof(hypercast.Decimal{}.Lo) - 0]struct{}
	_ [0 - unsafe.Offsetof(hypercast.Decimal{}.Lo)]struct{}
	_ [unsafe.Offsetof(hypercast.Decimal{}.Hi) - 8]struct{}
	_ [8 - unsafe.Offsetof(hypercast.Decimal{}.Hi)]struct{}
	_ [unsafe.Offsetof(hypercast.Decimal{}.Scale) - 12]struct{}
	_ [12 - unsafe.Offsetof(hypercast.Decimal{}.Scale)]struct{}
	_ [unsafe.Offsetof(hypercast.Decimal{}.Negative) - 13]struct{}
	_ [13 - unsafe.Offsetof(hypercast.Decimal{}.Negative)]struct{}

	// hypercast.Duration is the core's Duration: seconds at 0, nanos at 8, 16 bytes.
	_ [unsafe.Sizeof(hypercast.Duration{}) - 16]struct{}
	_ [16 - unsafe.Sizeof(hypercast.Duration{})]struct{}
	_ [unsafe.Offsetof(hypercast.Duration{}.Seconds) - 0]struct{}
	_ [0 - unsafe.Offsetof(hypercast.Duration{}.Seconds)]struct{}
	_ [unsafe.Offsetof(hypercast.Duration{}.Nanos) - 8]struct{}
	_ [8 - unsafe.Offsetof(hypercast.Duration{}.Nanos)]struct{}

	// uuid.UUID is the sixteen bytes the core writes, in RFC 9562 order.
	_ [unsafe.Sizeof(uuid.UUID{}) - 16]struct{}
	_ [16 - unsafe.Sizeof(uuid.UUID{})]struct{}
)
