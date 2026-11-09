package hypertabular

import (
	"errors"
	"fmt"
	"io"
	"strings"
	"testing"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// calling is an io.Reader that delivers its text a chunk at a time and, on the read numbered
// at, runs hook first: the caller's code, in the middle of a Read.
type calling struct {
	text  []byte
	pos   int
	chunk int
	calls int
	at    int
	hook  func()
}

func (s *calling) Read(p []byte) (int, error) {
	s.calls++
	if s.calls == s.at {
		s.hook()
	}
	if s.pos == len(s.text) {
		return 0, io.EOF
	}
	n := min(len(p), s.chunk, len(s.text)-s.pos)
	copy(p, s.text[s.pos:s.pos+n])
	s.pos += n
	return n, nil
}

// reentryText is a header and rows of `id,name-id`, the fifth name 40 bytes long, so that a small
// buffer grows in the middle of the read.
func reentryText(rows int) string {
	var b strings.Builder
	b.WriteString("id,name\n")
	for i := range rows {
		name := fmt.Sprintf("name-%d", i)
		if i == 5 {
			name += strings.Repeat("x", 40-len(name))
		}
		fmt.Fprintf(&b, "%d,%s\n", i, name)
	}
	return b.String()
}

type reentryRow struct {
	id   int32
	name string
	line int
}

func reentryRows(t *testing.T, r *DelimitedReader) []reentryRow {
	t.Helper()
	var rows []reentryRow
	for {
		b, err := r.Read()
		if err == io.EOF {
			return rows
		}
		if err != nil {
			t.Fatal(err)
		}
		for i := range b.Rows() {
			rows = append(rows, reentryRow{b.I32(0)[i], string(b.Text(1)[i]), b.Line(i)})
		}
	}
}

// A source calling back into the reader it feeds, in the middle of a Read, is refused: Read, Bind
// and Close each answer ErrBusy, nothing is closed, and the outer Read, and every one after it,
// delivers each row once, in order, from the right line. Before the reader marked a Read in
// progress, the inner Read moved the buffer under the outer one: rows came out lost, garbled or
// from the wrong line, with no error.
func TestASourceCallingBackIntoItsReaderIsRefused(t *testing.T) {
	const rows = 100
	text := reentryText(rows)
	plan := []Column{I32(0, hypercast.Invariant), Text(1)}
	whole := func(encoding TextEncoding) []reentryRow {
		dialect := CSV
		dialect.Encoding = encoding
		r, err := NewDelimitedReaderBytes([]byte(text), dialect, plan)
		if err != nil {
			t.Fatal(err)
		}
		return reentryRows(t, r)
	}
	for _, encoding := range []TextEncoding{UTF8, Latin1} {
		want := whole(encoding)
		if len(want) != rows {
			t.Fatalf("the control read %d rows", len(want))
		}
		for _, chunk := range []int{1, 7, 64} {
			for _, bufferBytes := range []int{8, 64} {
				for _, at := range []int{2, 3, 5, 8, 13} {
					label := fmt.Sprintf("encoding %d, %d-byte chunks, %d-byte buffer, call %d", encoding, chunk, bufferBytes, at)
					source := &calling{text: []byte(text), chunk: chunk}
					dialect := CSV
					dialect.Encoding = encoding
					r, err := NewDelimitedReader(source, dialect, plan, BatchRows(1), BufferBytes(bufferBytes))
					if err != nil {
						t.Fatalf("%s: %v", label, err)
					}
					// The header is read before the reader is there to call: count from after it.
					source.at = source.calls + at
					fired := false
					source.hook = func() {
						fired = true
						if b, err := r.Read(); b != nil || !errors.Is(err, ErrBusy) {
							t.Errorf("%s: an inner Read returned a batch (%t) and %v", label, b != nil, err)
						}
						if err := r.Bind(plan); !errors.Is(err, ErrBusy) {
							t.Errorf("%s: an inner Bind returned %v", label, err)
						}
						if err := r.Close(); !errors.Is(err, ErrBusy) {
							t.Errorf("%s: an inner Close returned %v", label, err)
						}
					}
					got := reentryRows(t, r)
					if !fired {
						t.Fatalf("%s: the source was read %d times, never at %d", label, source.calls, at)
					}
					if fmt.Sprint(got) != fmt.Sprint(want) {
						t.Errorf("%s: read\n%v\nwant\n%v", label, got, want)
					}
					if err := r.Close(); err != nil {
						t.Errorf("%s: Close after the read: %v", label, err)
					}
				}
			}
		}
	}
}

// A panic out of the source leaves no Read in progress: the reader reads on, and is not busy.
func TestASourceThatPanicsLeavesTheReaderIdle(t *testing.T) {
	source := &calling{text: []byte(reentryText(10)), chunk: 7}
	source.hook = func() { panic("the caller's source") }
	r, err := NewDelimitedReader(source, CSV, []Column{I32(0, hypercast.Invariant), Text(1)}, BatchRows(1), BufferBytes(8))
	if err != nil {
		t.Fatal(err)
	}
	source.at = source.calls + 3
	func() {
		defer func() {
			if recover() == nil {
				t.Error("the source's panic did not reach the caller")
			}
		}()
		for {
			if _, err := r.Read(); err != nil {
				t.Fatal(err)
			}
		}
	}()
	got := reentryRows(t, r)
	if len(got) == 0 || got[len(got)-1].id != 9 {
		t.Errorf("after the panic the reader read %v", got)
	}
}

// A batch's rows refuse after the next Read, as its faults do: ranged over past it, from the
// loop's body or after it, Batch.All panics rather than yield the next batch's rows; and a Read in
// the body of the reader's or a sheet's All panics, rather than lose what was left of the batch
// and yield the body's batch twice.
func TestRowsRefuseAfterTheNextRead(t *testing.T) {
	plan := []Column{I32(0, hypercast.Invariant)}
	text := "id\n0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n"
	reader := func() *DelimitedReader {
		r, err := NewDelimitedReaderBytes([]byte(text), CSV, plan, BatchRows(4))
		if err != nil {
			t.Fatal(err)
		}
		return r
	}
	mustPanicStaleRows := func(name string, ranged func()) {
		t.Helper()
		defer func() {
			if message, _ := recover().(string); message != staleRows {
				t.Errorf("%s: recovered %q, want %q", name, message, staleRows)
			}
		}()
		ranged()
	}

	// Batch.All, a Read in its body: two rows, then the refusal, never 6 and 7.
	{
		r := reader()
		b, _ := r.Read()
		var seen []int32
		mustPanicStaleRows("Batch.All, a Read in its body", func() {
			for row := range b.All() {
				id, _ := Cell[int32](row, 0)
				seen = append(seen, id)
				if len(seen) == 2 {
					if _, err := r.Read(); err != nil {
						t.Fatal(err)
					}
				}
			}
		})
		if fmt.Sprint(seen) != "[0 1]" {
			t.Errorf("Batch.All yielded %v past the next Read", seen)
		}
	}
	// Batch.All taken, then a Read, then ranged over: nothing.
	{
		r := reader()
		b, _ := r.Read()
		rows := b.All()
		if _, err := r.Read(); err != nil {
			t.Fatal(err)
		}
		var seen []int32
		mustPanicStaleRows("Batch.All after a Read", func() {
			for row := range rows {
				id, _ := Cell[int32](row, 0)
				seen = append(seen, id)
			}
		})
		if len(seen) != 0 {
			t.Errorf("Batch.All yielded %v after the next Read", seen)
		}
	}
	// The reader's All, a Read in its body: rows 0 and 1, then the refusal; neither 2 and 3 lost
	// nor 6 and 7 twice.
	{
		r := reader()
		var seen []int32
		mustPanicStaleRows("DelimitedReader.All, a Read in its body", func() {
			for row, err := range r.All() {
				if err != nil {
					t.Fatal(err)
				}
				id, _ := Cell[int32](row, 0)
				seen = append(seen, id)
				if len(seen) == 2 {
					if _, err := r.Read(); err != nil {
						t.Fatal(err)
					}
				}
			}
		})
		if fmt.Sprint(seen) != "[0 1]" {
			t.Errorf("DelimitedReader.All yielded %v", seen)
		}
	}
	// A sheet's All, the same.
	{
		book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
		if err != nil {
			t.Fatal(err)
		}
		sheet, err := book.Sheet(0, DefaultSheetOptions, []Column{Text(0)}, BatchRows(1))
		if err != nil {
			t.Fatal(err)
		}
		seen := 0
		mustPanicStaleRows("Sheet.All, a Read in its body", func() {
			for _, err := range sheet.All() {
				if err != nil {
					t.Fatal(err)
				}
				if seen++; seen == 1 {
					if _, err := sheet.Read(); err != nil {
						t.Fatal(err)
					}
				}
			}
		})
		if seen != 1 {
			t.Errorf("Sheet.All yielded %d rows", seen)
		}
	}
	// A Close in the loop's body ends the batch the same way: the refusal, not os.ErrClosed after
	// rows the closed reader no longer holds.
	{
		r := reader()
		seen := 0
		mustPanicStaleRows("DelimitedReader.All, a Close in its body", func() {
			for _, err := range r.All() {
				if err != nil {
					t.Fatal(err)
				}
				if seen++; seen == 1 {
					if err := r.Close(); err != nil {
						t.Fatal(err)
					}
				}
			}
		})
		if seen != 1 {
			t.Errorf("DelimitedReader.All yielded %d rows past a Close", seen)
		}
	}
	// Breaking out is not a Read: a later All goes on from the next batch.
	{
		r := reader()
		for range r.All() {
			break
		}
		var rest []int32
		for row, err := range r.All() {
			if err != nil {
				t.Fatal(err)
			}
			id, _ := Cell[int32](row, 0)
			rest = append(rest, id)
		}
		if fmt.Sprint(rest) != "[4 5 6 7 8 9]" {
			t.Errorf("after a break, All yielded %v", rest)
		}
	}
}
