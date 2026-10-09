package hypertabular

// Header first: a source opened without a plan, its header's names looked up, and the plan
// built from them bound after — and the row view, read a batch at a time and across
// batches. The corpus replays both through every source; these are what it cannot say.

import (
	"bytes"
	"errors"
	"os"
	"strings"
	"testing"
	"testing/iotest"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

const regions = "Region Code,Region Name,M49 Code,Country,M49 Code\n" +
	"002,Africa,012,Algeria,999\n" +
	"019,Americas,032,Argentina,999\n"

func TestAPlanIsBuiltFromTheHeaderAndBoundOnce(t *testing.T) {
	r, err := NewDelimitedReaderUnbound(strings.NewReader(regions), CSV)
	if err != nil {
		t.Fatal(err)
	}
	defer r.Close()
	header := r.Header()
	if len(header) != 5 || r.ColumnCount() != 5 || r.IsBound() || r.Plan() != nil {
		t.Fatalf("header %q, %d columns, bound %v, plan %v", header, r.ColumnCount(), r.IsBound(), r.Plan())
	}
	// Exact, case and spaces included, and the first of two columns of the same name.
	for name, want := range map[string]int{"Region Code": 0, "M49 Code": 2, "m49 code": -1, " M49 Code": -1, "M49 Code ": -1} {
		got, ok := header.Find(name)
		if got != want || ok != (want >= 0) {
			t.Errorf("Find(%q) = %d, %v; want %d", name, got, ok, want)
		}
	}
	if got, err := header.OrdinalBytes([]byte("M49 Code")); got != 2 || err != nil {
		t.Errorf("OrdinalBytes: %d, %v", got, err)
	}
	_, err = header.Ordinal("Population")
	var noColumn *NoColumnError
	if !errors.Is(err, ErrNoColumn) || !errors.As(err, &noColumn) || noColumn.Name != "Population" ||
		!strings.Contains(err.Error(), `"Population"`) {
		t.Errorf("a missing name: %v", err)
	}
	if _, err := header.OrdinalBytes([]byte("Population")); !errors.Is(err, ErrNoColumn) {
		t.Errorf("a missing name as bytes: %v", err)
	}

	// Before the plan, Read says so — and says so again, rather than ending the input.
	for range 2 {
		if b, err := r.Read(); b != nil || err != ErrUnbound {
			t.Fatalf("Read before Bind: %v, %v", b, err)
		}
	}
	m49, _ := header.Ordinal("M49 Code")
	name, _ := header.Ordinal("Region Name")
	plan := []Column{I32(m49, hypercast.Invariant), Text(name)}
	if err := r.Bind(plan); err != nil {
		t.Fatal(err)
	}
	if err := r.Bind(plan); err != ErrAlreadyBound {
		t.Errorf("a second Bind: %v", err)
	}
	if !r.IsBound() || len(r.Plan()) != 2 {
		t.Errorf("bound %v, plan %v", r.IsBound(), r.Plan())
	}
	b := read(t, r, 2)
	if codes := b.I32(0); codes[0] != 12 || codes[1] != 32 || b.TextString(1, 1) != "Americas" {
		t.Errorf("codes %v, names %q", codes, b.Text(1))
	}
	end(t, r)
}

func TestAPlanGivenUpFrontCannotBeBoundAgain(t *testing.T) {
	r := open(t, regions, CSV, []Column{Text(0)})
	if !r.IsBound() {
		t.Error("a reader opened with a plan is not bound")
	}
	if err := r.Bind([]Column{Text(1)}); err != ErrAlreadyBound {
		t.Errorf("Bind: %v", err)
	}
}

func TestAHeaderlessSourceIsBoundByPosition(t *testing.T) {
	r, err := NewDelimitedReaderBytesUnbound([]byte("1,a\n2,b\n"), headerless(','))
	if err != nil {
		t.Fatal(err)
	}
	// No header: the width is the first record's, and not known until it has been read.
	if r.Header() != nil || r.ColumnCount() != 0 {
		t.Fatalf("header %q, %d columns", r.Header(), r.ColumnCount())
	}
	if err := r.Bind([]Column{Text(1), I32(0, hypercast.Invariant)}); err != nil {
		t.Fatal(err)
	}
	b := read(t, r, 2)
	if r.ColumnCount() != 2 || string(b.Text(0)[1]) != "b" || b.I32(1)[1] != 2 {
		t.Errorf("%d columns, %q, %v", r.ColumnCount(), b.Text(0), b.I32(1))
	}
}

// headerless is a dialect with no header and the given separator.
func headerless(separator byte) Dialect {
	return Dialect{Separator: separator, Quoting: true}
}

func TestAPlanThatCannotBeHonouredIsRefusedAtBind(t *testing.T) {
	r, err := OpenDelimitedUnbound(repositoryFile(t, "corpus", "delimited.json"), Dialect{Separator: '\t'})
	if err != nil {
		t.Fatal(err)
	}
	defer r.Close()
	for _, plan := range [][]Column{{{}}, {Text(-1)}, {I32(0, hypercast.NumFormat{})}} {
		if err := r.Bind(plan); err == nil || err == ErrAlreadyBound || !strings.HasPrefix(err.Error(), "hypertabular: ") {
			t.Errorf("Bind(%v): %v", plan, err)
		}
		// Refused, the reader is as it was: unbound, and bindable.
		if r.IsBound() {
			t.Fatalf("a refused plan left the reader bound")
		}
	}
	if err := r.Bind([]Column{Text(0)}); err != nil {
		t.Fatal(err)
	}
	if _, err := r.Read(); err != nil {
		t.Fatal(err)
	}
}

func TestABrokenHeaderIsTheUnboundConstructorsError(t *testing.T) {
	_, err := NewDelimitedReaderBytesUnbound([]byte("a,\"b\n"), CSV)
	var failure *Failure
	if !errors.As(err, &failure) || failure.Kind != UnclosedQuote {
		t.Errorf("an unclosed quote in the header: %v", err)
	}
}

func TestARowIsItsBatchReadAcross(t *testing.T) {
	r := open(t, "id,name,note\n7,alice,\"said \"\"hi\"\"\"\n8,,x\n", CSV, []Column{I32(0, hypercast.Invariant), Text(1), Text(2)})
	b := read(t, r, 2)
	var seen []int
	for row := range b.All() {
		seen = append(seen, row.Index())
		if row.Line() != b.Line(row.Index()) || row.Batch() != b {
			t.Errorf("row %d: line %d, want %d", row.Index(), row.Line(), b.Line(row.Index()))
		}
		if id, fault := Cell[int32](row, 0); fault != nil || id != b.I32(0)[row.Index()] {
			t.Errorf("row %d: id %d, %v", row.Index(), id, fault)
		}
	}
	if len(seen) != 2 || seen[0] != 0 || seen[1] != 1 {
		t.Errorf("rows %v", seen)
	}
	// Breaking out stops the iteration.
	count := 0
	for range b.All() {
		count++
		break
	}
	if count != 1 {
		t.Errorf("%d rows after a break", count)
	}

	first, second := b.Row(0), b.Row(1)
	// An escaped quote resolved; an empty cell is the empty string and nil text.
	if first.TextString(2) != `said "hi"` || second.TextString(1) != "" || second.Text(1) != nil {
		t.Errorf("text %q, %q, %q", first.TextString(2), second.TextString(1), second.Text(1))
	}
	if second.Verdict(1).Reason() != hypercast.Empty || second.Fault(1) == nil || string(first.Raw(2)) != `said "hi"` {
		t.Errorf("verdict %v, fault %v, raw %q", second.Verdict(1), second.Fault(1), first.Raw(2))
	}
	// A string taken before another call is unchanged by it: the views are of the batch's
	// bytes, which only the next Read reuses.
	kept := first.TextString(1)
	_ = second.TextString(2)
	_ = first.Raw(2)
	if kept != "alice" {
		t.Errorf("a string view changed within its batch: %q", kept)
	}
	for _, index := range []int{-1, 2} {
		func() {
			defer func() {
				if recover() == nil {
					t.Errorf("Row(%d): no panic", index)
				}
			}()
			b.Row(index)
		}()
	}
}

func TestRowsRunAcrossBatches(t *testing.T) {
	var input bytes.Buffer
	input.WriteString("n\n")
	for n := range 10 {
		input.WriteString(string(rune('0' + n)))
		input.WriteByte('\n')
	}
	r, err := NewDelimitedReaderUnbound(iotest.OneByteReader(&input), CSV, BatchRows(3))
	if err != nil {
		t.Fatal(err)
	}
	ordinal, err := r.Header().Ordinal("n")
	if err != nil {
		t.Fatal(err)
	}
	if err := r.Bind([]Column{I32(ordinal, hypercast.Invariant)}); err != nil {
		t.Fatal(err)
	}
	var values []int32
	lastGeneration := uint64(0)
	reads := 0
	for row, err := range r.All() {
		if err != nil {
			t.Fatal(err)
		}
		if generation := row.Batch().generation; generation != lastGeneration {
			lastGeneration, reads = generation, reads+1
		}
		value, _ := Cell[int32](row, 0)
		values = append(values, value)
	}
	if len(values) != 10 || values[0] != 0 || values[9] != 9 || reads < 4 {
		t.Errorf("values %v across %d batches", values, reads)
	}
	// The end is final, and All after it yields nothing.
	for range r.All() {
		t.Error("a row after the end")
	}
	end(t, r)
}

func TestRowsEndOnTheErrorTheInputEndedOn(t *testing.T) {
	r := open(t, "a,b\n1,2\n3,4\n5\n", CSV, []Column{I32(0, hypercast.Invariant)}, BatchRows(1))
	rows := 0
	var ended error
	for row, err := range r.All() {
		if err != nil {
			ended = err
			if row != (Row{}) {
				t.Errorf("a row beside the error: %+v", row)
			}
			continue
		}
		rows++
	}
	var failure *Failure
	if rows != 2 || !errors.As(ended, &failure) || failure.Kind != ColumnCount {
		t.Errorf("%d rows, then %v", rows, ended)
	}
	// An unbound reader's All says that it is unbound.
	unbound, err := NewDelimitedReaderBytesUnbound([]byte("a\n1\n"), CSV)
	if err != nil {
		t.Fatal(err)
	}
	for _, err := range unbound.All() {
		if err != ErrUnbound {
			t.Errorf("All before Bind: %v", err)
		}
	}
}

func TestASheetIsOpenedHeaderFirst(t *testing.T) {
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	for _, open := range []func() (*Sheet, error){
		func() (*Sheet, error) { return book.SheetUnbound(0, DefaultSheetOptions) },
		func() (*Sheet, error) { return book.SheetNamedUnbound(book.Sheets()[0].Name, DefaultSheetOptions) },
	} {
		sheet, err := open()
		if err != nil {
			t.Fatal(err)
		}
		name, err := sheet.Header().Ordinal("name")
		if err != nil {
			t.Fatal(err)
		}
		if _, err := sheet.Read(); err != ErrUnbound {
			t.Errorf("Read before Bind: %v", err)
		}
		if err := sheet.Bind([]Column{Text(name)}); err != nil {
			t.Fatal(err)
		}
		var names []string
		for row, err := range sheet.All() {
			if err != nil {
				t.Fatal(err)
			}
			names = append(names, strings.Clone(row.TextString(0)))
		}
		if len(names) == 0 || names[0] != "alice" {
			t.Errorf("names %q", names)
		}
	}
	if _, err := book.SheetUnbound(99, DefaultSheetOptions); !errors.Is(err, ErrNoSheet) {
		t.Errorf("sheet 99: %v", err)
	}
	if _, err := book.SheetNamedUnbound("No such sheet", DefaultSheetOptions); !errors.Is(err, ErrNoSheet) {
		t.Errorf("a name the workbook lacks: %v", err)
	}
	if _, err := book.SheetUnbound(0, SheetOptions{}); err == nil {
		t.Error("no rows a batch: no error")
	}
	sheet, err := book.SheetUnbound(0, DefaultSheetOptions)
	if err != nil {
		t.Fatal(err)
	}
	if err := sheet.Bind([]Column{Text(-1)}); err == nil || sheet.IsBound() {
		t.Errorf("a plan that cannot be honoured: %v, bound %v", err, sheet.IsBound())
	}
}

func TestAWorkbookIsReadFromAnIOReader(t *testing.T) {
	container, err := os.ReadFile(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	book, err := NewWorkbookReader(iotest.OneByteReader(bytes.NewReader(container)))
	if err != nil || book.Format() != XLSX {
		t.Fatalf("%v, %v", book, err)
	}
	broken := errors.New("the network went away")
	if _, err := NewWorkbookReader(iotest.ErrReader(broken)); !errors.Is(err, broken) {
		t.Errorf("a failing source: %v", err)
	}
	if _, err := NewWorkbookReader(nil); err == nil {
		t.Error("no source: no error")
	}
	var failure *Failure
	if _, err := NewWorkbookReader(strings.NewReader("not a zip at all")); !errors.As(err, &failure) || failure.Kind != NotAZip {
		t.Errorf("not a zip: %v", err)
	}
}
