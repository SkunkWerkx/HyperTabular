package hypertabular

import (
	"fmt"
	"math"
	"time"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// column is one plan column's share of a reader's buffers.
type column struct {
	plan Column
	// values is the door's value array inside the block: batchRows values of the layout the
	// core writes.
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

// columns is a plan and everything the core casts it into: the plan as the core reads it,
// and every column's values and verdicts for one batch — allocated once, as ordinary Go
// memory, and reused for every batch. A DelimitedReader and a Sheet each own one.
type columns struct {
	plan      []Column
	columns   []column
	batchRows int
	// width is the widest ordinal the plan reads, plus one.
	width int

	// Everything the core is handed, all of it Go memory with no Go pointer inside it. The
	// column buffers are one allocation — block — and each column's two arrays are named
	// to the core by their offsets into it; see backend_static.go for why.
	specs   []rawSpec
	offsets []uintptr
	block   []uint64
}

// newColumns checks a plan and allocates its buffers.
func newColumns(plan []Column, batchRows int) (*columns, error) {
	if batchRows <= 0 {
		return nil, fmt.Errorf("hypertabular: BatchRows must be positive, not %d", batchRows)
	}
	// One allocation for every column's values and verdicts, each array on an 8-byte
	// boundary — the strictest alignment any of the Go types they are handed out as wants.
	rows := uintptr(batchRows)
	if rows > (math.MaxInt-7)/16 {
		return nil, fmt.Errorf("hypertabular: BatchRows %d is too many", batchRows)
	}
	s := &columns{
		plan:      append([]Column(nil), plan...),
		columns:   make([]column, len(plan)),
		batchRows: batchRows,
		specs:     make([]rawSpec, len(plan)),
		offsets:   make([]uintptr, 2*len(plan)),
	}
	widest := -1
	var words uintptr
	for index, planned := range plan {
		spec, err := planned.spec()
		if err != nil {
			return nil, fmt.Errorf("hypertabular: plan column %d: %w", index, err)
		}
		s.specs[index] = spec
		s.columns[index].plan = planned
		widest = max(widest, planned.ordinal)
		s.offsets[2*index] = words * 8
		words += (rows*planned.door.valueSize() + 7) / 8
		s.offsets[2*index+1] = words * 8
		words += (rows*unsafe.Sizeof(CellVerdict{}) + 7) / 8
		if words > math.MaxInt/8 {
			return nil, fmt.Errorf("hypertabular: a plan of %d columns at %d rows a batch is too large", len(plan), batchRows)
		}
	}
	s.width = widest + 1
	s.block = make([]uint64, words)
	base := unsafe.Pointer(unsafe.SliceData(s.block))
	for index := range s.columns {
		c := &s.columns[index]
		c.values = unsafe.Add(base, s.offsets[2*index])
		c.verdicts = unsafe.Slice((*CellVerdict)(unsafe.Add(base, s.offsets[2*index+1])), batchRows)
		switch c.plan.door {
		case DoorTimestamp, DoorUnix, DoorExcelSerial:
			c.times = make([]time.Time, batchRows)
		case DoorDateOnly, DoorDateOnlyOrdered:
			c.dates = make([]hypercast.Date, batchRows)
		case DoorDateTime:
			c.civils = make([]hypercast.CivilDateTime, batchRows)
		case DoorText:
			c.texts = make([][]byte, batchRows)
		}
	}
	return s, nil
}

// grownTo is old made at least needed long, what it held kept — which is what lets the core go
// on from where it stopped when a workbook call asks for room.
func grownTo[T any](old []T, needed uint64) []T {
	length := len(old) + 1
	if needed > uint64(length) {
		length = int(min(needed, uint64(math.MaxInt32)))
	}
	larger := make([]T, length)
	copy(larger, old)
	return larger
}

// atLeast is old made at least n long, what it held kept; old itself when it is long enough.
func atLeast[T any](old []T, n int) []T {
	if len(old) >= n {
		return old
	}
	larger := make([]T, n)
	copy(larger, old)
	return larger
}
