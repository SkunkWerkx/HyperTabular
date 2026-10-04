package hypertabular

// The allocation claims, held as tests: every buffer is allocated when the reader is built,
// so a batch — the one cgo crossing, and every column of it handed out — allocates nothing.
// backend_static.go's shim is what makes the crossing itself free: nothing is pinned and
// nothing escapes that was not already on the heap.

import (
	"bytes"
	"fmt"
	"testing"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

func assertAllocs(t *testing.T, name string, want float64, f func()) {
	t.Helper()
	if got := testing.AllocsPerRun(100, f); got != want {
		t.Errorf("%s: %v allocs per call, want %v", name, got, want)
	}
}

// A file with a column for every kind of buffer the reader keeps: values handed out as the
// core wrote them, values converted on the way out, text in the input, text in the arena,
// and cells that do not cast.
func allocationInput(rows int) ([]byte, []Column) {
	var input bytes.Buffer
	input.WriteString("id,name,when,amount,day,note,flag\n")
	for row := 0; row < rows; row++ {
		id := fmt.Sprint(row)
		if row%7 == 3 {
			id = "12x4"
		}
		fmt.Fprintf(&input, "%s,name %d,2026-01-02T15:04:05Z,\"1,234.5\",1/7/2026,\"said \"\"%d\"\"\",yes\n", id, row, row)
	}
	return input.Bytes(), []Column{
		I32(0, hypercast.Invariant),
		Text(1),
		Timestamp(2),
		Exact(3, hypercast.Invariant),
		DateOnlyOrdered(4, hypercast.MonthDayYear),
		Text(5),
		Bool(6),
		DateTime(4, hypercast.MonthDayYear),
		F64(3, hypercast.Invariant),
	}
}

// One batch, every column of it read: what a call of the loops below does. A batch from
// memory is full until the input runs out; one from a stream is as many whole rows as the
// buffer held, which can be fewer.
func readBatch(t *testing.T, r *DelimitedReader, atLeast int) {
	rows, err := r.Read()
	if err != nil || rows < atLeast {
		t.Fatalf("Read returned %d, %v; want a batch of at least %d", rows, err, atLeast)
	}
	ids, verdicts := r.I32(0), r.Verdicts(0)
	names, instants, amounts := r.Text(1), r.Timestamp(2), r.Exact(3)
	days, notes, flags, civils, reals := r.DateOnly(4), r.Text(5), r.Bool(6), r.DateTime(7), r.F64(8)
	for row := 0; row < rows; row++ {
		if !verdicts[row].OK() && (ids[row] != 0 || verdicts[row].Reason() != hypercast.Malformed) {
			t.Fatalf("row %d: id %d, verdict %v", row, ids[row], verdicts[row])
		}
		if len(names[row]) < 6 || instants[row].Year() != 2026 || amounts[row].Scale != 1 || days[row].Day != 7 ||
			len(notes[row]) < 8 || !flags[row] || civils[row].Date.Year != 2026 || reals[row] != 1234.5 {
			t.Fatalf("row %d did not read back", row)
		}
	}
}

func TestABatchFromMemoryAllocatesNothing(t *testing.T) {
	const batchRows = 32
	// AllocsPerRun calls once to warm up and then a hundred times: 101 batches, and more
	// than that in the file so that every one of them is full.
	input, plan := allocationInput(batchRows * 128)
	r, err := NewDelimitedReaderBytes(input, CSV, plan, BatchRows(batchRows))
	if err != nil {
		t.Fatal(err)
	}
	assertAllocs(t, "Read (memory)", 0, func() { readBatch(t, r, batchRows) })
}

func TestABatchFromAStreamAllocatesNothing(t *testing.T) {
	const batchRows = 32
	// A buffer that holds every record but not the file: the reader moves what the core did
	// not consume to the front and reads more behind it, over and over, and allocates
	// nothing doing it.
	input, plan := allocationInput(batchRows * 128)
	r, err := NewDelimitedReader(bytes.NewReader(input), CSV, plan, BatchRows(batchRows), BufferBytes(64*1024))
	if err != nil {
		t.Fatal(err)
	}
	assertAllocs(t, "Read (stream)", 0, func() { readBatch(t, r, 1) })
}

// A cell's raw text is a slice of the input, and an escaped one is unescaped into a scratch
// buffer the reader keeps.
func TestRawAllocatesNothing(t *testing.T) {
	input, plan := allocationInput(8)
	r, err := NewDelimitedReaderBytes(input, CSV, plan)
	if err != nil {
		t.Fatal(err)
	}
	if rows, err := r.Read(); err != nil || rows != 8 {
		t.Fatalf("Read returned %d, %v", rows, err)
	}
	if string(r.Raw(0, 3)) != "12x4" || string(r.Raw(5, 3)) != `said "3"` {
		t.Fatalf("raw %q and %q", r.Raw(0, 3), r.Raw(5, 3))
	}
	assertAllocs(t, "Raw (in place)", 0, func() { r.Raw(0, 3) })
	assertAllocs(t, "Raw (unescaped)", 0, func() { r.Raw(5, 3) })
}

// The verdict array is free to scan. Asking for HyperCast's *Fault is the one allocation a
// cell that did not cast makes, as it is for HyperCast's own doors; a cell that cast makes
// none.
func TestAFaultIsTheOnlyAllocation(t *testing.T) {
	input, plan := allocationInput(8)
	r, err := NewDelimitedReaderBytes(input, CSV, plan)
	if err != nil {
		t.Fatal(err)
	}
	if rows, err := r.Read(); err != nil || rows != 8 {
		t.Fatalf("Read returned %d, %v", rows, err)
	}
	if r.Fault(0, 0) != nil || r.Fault(0, 3) == nil {
		t.Fatalf("faults %v and %v", r.Fault(0, 0), r.Fault(0, 3))
	}
	assertAllocs(t, "Fault (a cell that cast)", 0, func() { kept = r.Fault(0, 0) })
	assertAllocs(t, "Fault (a cell that did not)", 1, func() { kept = r.Fault(0, 3) })
}

// kept is where the faults above go, so that the compiler cannot prove they are unused and
// leave them on the stack.
var kept *hypercast.Fault
