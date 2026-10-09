package hypertabular

// The allocation claims, held as tests: every buffer is allocated when the reader is built,
// so a batch — the one cgo crossing, and every column of it handed out — allocates nothing.
// backend_static.go's shim is what makes the crossing itself free: nothing is pinned and
// nothing escapes that was not already on the heap.

import (
	"bytes"
	"fmt"
	"runtime"
	"strings"
	"testing"
	"time"

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
	b, err := r.Read()
	if err != nil || b.Rows() < atLeast {
		t.Fatalf("Read returned %v, %v; want a batch of at least %d", b, err, atLeast)
	}
	ids, verdicts := b.I32(0), b.Verdicts(0)
	names, instants, amounts := b.Text(1), b.Timestamp(2), b.Exact(3)
	days, notes, flags, civils, reals := b.DateOnly(4), b.Text(5), b.Bool(6), b.DateTime(7), b.F64(8)
	for row := 0; row < b.Rows(); row++ {
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
	b, err := r.Read()
	if err != nil || b.Rows() != 8 {
		t.Fatalf("Read returned %v, %v", b, err)
	}
	if string(b.Raw(0, 3)) != "12x4" || string(b.Raw(5, 3)) != `said "3"` {
		t.Fatalf("raw %q and %q", b.Raw(0, 3), b.Raw(5, 3))
	}
	assertAllocs(t, "Raw (in place)", 0, func() { b.Raw(0, 3) })
	assertAllocs(t, "Raw (unescaped)", 0, func() { b.Raw(5, 3) })
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
	b, err := r.Read()
	if err != nil || b.Rows() != 8 {
		t.Fatalf("Read returned %v, %v", b, err)
	}
	if b.Fault(0, 0) != nil || b.Fault(0, 3) == nil {
		t.Fatalf("faults %v and %v", b.Fault(0, 0), b.Fault(0, 3))
	}
	assertAllocs(t, "Fault (a cell that cast)", 0, func() { kept = b.Fault(0, 0) })
	assertAllocs(t, "Fault (a cell that did not)", 1, func() { kept = b.Fault(0, 3) })
	// Get is the same: a value costs nothing, whatever its type, and a fault is the one
	// allocation.
	assertAllocs(t, "Get (a value)", 0, func() { keptTime, kept = Get[time.Time](b, 2, 0) })
	assertAllocs(t, "Get (text)", 0, func() { keptText, kept = Get[[]byte](b, 1, 0) })
	assertAllocs(t, "Get (a fault)", 1, func() { _, kept = Get[int32](b, 0, 3) })
}

var (
	keptTime time.Time
	keptText []byte
)

// kept is where the faults above go, so that the compiler cannot prove they are unused and
// leave them on the stack.
var kept *hypercast.Fault

// The row view is a (batch, index) pair: iterating a batch's rows, and reading each cell
// through them — a value, a verdict, text as bytes and as a string — allocates nothing.
func TestRowsAllocateNothing(t *testing.T) {
	const batchRows = 32
	input, plan := allocationInput(batchRows * 4)
	r, err := NewDelimitedReaderBytes(input, CSV, plan, BatchRows(batchRows))
	if err != nil {
		t.Fatal(err)
	}
	b, err := r.Read()
	if err != nil || b.Rows() != batchRows {
		t.Fatalf("Read returned %v, %v", b, err)
	}
	// What the loop reads is checked after it, so that nothing in it is boxed for a message.
	rows, sum, bad := 0, 0, 0
	assertAllocs(t, "Batch.All", 0, func() {
		rows, sum, bad = 0, 0, 0
		for row := range b.All() {
			// A fault is the one allocation a cell makes, here as through Get: the verdict
			// is free to ask first.
			if row.Verdict(0).OK() {
				if id, _ := Cell[int32](row, 0); int(id) != row.Index() {
					bad++
				}
			}
			if len(row.TextString(1)) < 6 || len(row.Text(5)) < 8 || row.Line() < 2 || !row.Verdict(1).OK() {
				bad++
			}
			keptString = row.TextString(1)
			sum += row.Index()
			rows++
		}
	})
	if rows != batchRows || bad != 0 || sum != batchRows*(batchRows-1)/2 {
		t.Fatalf("%d rows, %d of them bad", rows, bad)
	}
	assertAllocs(t, "Batch.TextString", 0, func() { keptString = b.TextString(5, 3) })
}

// Rows read across batches through the reader cost nothing a row and nothing a batch: what
// the loop allocates, if anything, is the iterator itself — at most a couple of allocations,
// for four batches as for a hundred and twenty-eight (the runtime's own, which MemStats
// counts too, are the slack).
func TestRowsAcrossBatchesAllocateNothingARow(t *testing.T) {
	const batchRows = 32
	across := func(batches int) (rows int, allocs uint64) {
		input, plan := allocationInput(batchRows * batches)
		r, err := NewDelimitedReaderBytes(input, CSV, plan, BatchRows(batchRows))
		if err != nil {
			t.Fatal(err)
		}
		var before, after runtime.MemStats
		runtime.GC()
		runtime.ReadMemStats(&before)
		var ended error
		for row, err := range r.All() {
			if err != nil {
				ended = err
				break
			}
			keptString = row.TextString(1)
			keptTime, _ = Cell[time.Time](row, 2)
			rows++
		}
		runtime.ReadMemStats(&after)
		if ended != nil {
			t.Fatal(ended)
		}
		return rows, after.Mallocs - before.Mallocs
	}
	// Allocation-free a row means what the loop allocates does not grow with what it reads,
	// and that is what is measured: 4 batches against 128. MemStats counts the whole
	// process, so the count of one pass is not ours alone — on Alpine arm64 the 4-batch pass
	// has come in at 7 and the 128-batch pass at 0, run after run — and no fixed allowance
	// survives that; the iterator itself is a constant too. None of it grows with the
	// input, and an allocation of ours a row or a batch does, the same every time — so an
	// attempt that shows no growth clears it, and the runtime gets three.
	var attempts []string
	for range 3 {
		fewRows, few := across(4)
		manyRows, many := across(128)
		if fewRows != batchRows*4 || manyRows != batchRows*128 {
			t.Fatalf("%d and %d rows across the batches", fewRows, manyRows)
		}
		if many <= few {
			return
		}
		attempts = append(attempts, fmt.Sprintf("%d allocs across 4 batches, %d across 128", few, many))
	}
	t.Errorf("the loop's allocations grow with the input: %s", strings.Join(attempts, "; "))
}

var keptString string
