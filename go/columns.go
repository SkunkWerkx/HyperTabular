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
	// core writes. It and verdicts are pointed at the rows of every batch the core fills
	// (core.go's point), and are nil before the first.
	values   unsafe.Pointer
	verdicts []CellVerdict
	// notes is the door's word on each verdict, pointed at as verdicts is: nil with notes off.
	notes []rawNote
	// checks is what the checked door reports beside each verdict, pointed at as verdicts is: nil
	// for a column that declares no constraints.
	checks []hypercast.RawCheck

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
	// notes is whether a batch carries a detail and an origin for each cell; widths whether the
	// fill also says how many cells each row had (core.go's attach makes room for that).
	notes  bool
	widths bool

	// The plan as the core reads it, and where each column's arrays sit in the block of
	// blockBytes that holds them all: offsets[4i] is column i's values, offsets[4i+1] its
	// verdicts, offsets[4i+2] its notes and offsets[4i+3] its checks. The block is laid out in
	// the reader's instance of the core (core.go's attach); block here is the Go copy a host that
	// cannot view the instance's memory decodes it into, allocated the first time one does.
	specs      []rawSpec
	extras     []specExtra
	offsets    []uintptr
	blockBytes uintptr
	block      []uint64
}

// newColumns checks a plan and allocates its buffers, once the core has allowed their size. notes
// says whether a batch carries a detail and an origin for each cell, and flags says the rest of
// what the core counts: batchWidths, batchSheet.
func newColumns(plan []Column, batchRows int, notes bool, flags uint32) (*columns, error) {
	s := &columns{
		plan:      append([]Column(nil), plan...),
		columns:   make([]column, len(plan)),
		batchRows: batchRows,
		notes:     notes,
		specs:     make([]rawSpec, len(plan)),
		extras:    make([]specExtra, len(plan)),
		offsets:   make([]uintptr, 4*len(plan)),
	}
	widest := -1
	for index, planned := range plan {
		spec, extra, err := planned.spec()
		if err != nil {
			return nil, fmt.Errorf("hypertabular: plan column %d: %w", index, err)
		}
		s.specs[index], s.extras[index] = spec, extra
		s.columns[index].plan = planned
		widest = max(widest, planned.ordinal)
	}
	if notes {
		flags |= batchNotes
	}
	if _, err := checkSizes(s.specs, s.extras, batchRows, 1, flags); err != nil {
		return nil, err
	}
	// One allocation for every column's values and verdicts, each array on an 8-byte
	// boundary — the strictest alignment any of the Go types they are handed out as wants. The core
	// has allowed the bytes, and where int is 32 bits (TinyGo's wasm) what it allows may still not
	// be addressable.
	rows := uint64(batchRows)
	var words uint64
	for index, planned := range plan {
		s.offsets[4*index] = uintptr(words * 8)
		words += (rows*uint64(planned.door.valueSize()) + 7) / 8
		s.offsets[4*index+1] = uintptr(words * 8)
		words += (rows*uint64(unsafe.Sizeof(CellVerdict{})) + 7) / 8
		s.offsets[4*index+2] = uintptr(words * 8)
		if notes {
			words += (rows*uint64(unsafe.Sizeof(rawNote{})) + 7) / 8
		}
		// As many checks as there are verdicts: the core writes one beside each.
		s.offsets[4*index+3] = uintptr(words * 8)
		if planned.checked {
			words += (rows*uint64(hypercast.RawCheckSize) + 7) / 8
		}
		if words > math.MaxInt/8 {
			return nil, fmt.Errorf("hypertabular: a plan of %d columns at %d rows a batch is too large", len(plan), batchRows)
		}
	}
	s.width = widest + 1
	s.blockBytes = uintptr(words * 8)
	for index := range s.columns {
		c := &s.columns[index]
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

// grownTo is a capacity of old elements made at least needed — what a workbook call that
// stopped for room asks its buffer grown to. The instance keeps what the buffer held (core.go's
// ensure), which is what lets the core go on from where it stopped.
func grownTo(old int, needed uint64) int {
	length := old + 1
	if needed > uint64(length) {
		length = int(min(needed, uint64(math.MaxInt32)))
	}
	return length
}
