package hypertabular

// What the corpus can't say of the declarations a read takes and of what it gives back: the
// errors, the limits, and what's left unsaid in a case's JSON.

import (
	"errors"
	"fmt"
	"io"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strconv"
	"strings"
	"testing"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

func TestADeclarationThatIsNotOneIsAnError(t *testing.T) {
	plan := []Column{Text(0)}
	for name, dialect := range map[string]Dialect{
		"ShortRows":    func() Dialect { d := CSV; d.ShortRows = 2; return d }(),
		"LongRows":     func() Dialect { d := CSV; d.LongRows = 3; return d }(),
		"TextEncoding": func() Dialect { d := CSV; d.Encoding = 4; return d }(),
	} {
		if _, err := NewDelimitedReaderBytes([]byte("a\n1\n"), dialect, plan); err == nil || !strings.Contains(err.Error(), "undefined "+name) {
			t.Errorf("%s: %v", name, err)
		}
	}
	if _, err := NewDelimitedReaderBytes([]byte("a\n1\n"), CSV, []Column{DateOnly(0).WithDateOptions(hypercast.DateOptions{Pivot: 5})}); err == nil {
		t.Error("a pivot HyperCast refuses reached the read")
	}
	if _, err := NewDelimitedReaderBytes([]byte("a\n1\n"), CSV, []Column{DateOnly(0).WithDateOptions(hypercast.DateOptions{Shapes: 1 << 7})}); err == nil {
		t.Error("a shape HyperCast doesn't define reached the read")
	}
}

// Two lists are the same list when they hold the same tokens in order, and a dialect and a column
// stay comparable with them in.
func TestTokensAreComparable(t *testing.T) {
	if NewTokens("NA", "-") != NewTokens("NA", "-") || NewTokens("NA", "-") == NewTokens("-", "NA") || NewTokens() != (Tokens{}) {
		t.Error("lists of tokens don't compare by what they hold")
	}
	// A token's length is its own: these two would be one string if they were joined.
	if NewTokens("ab", "c") == NewTokens("a", "bc") {
		t.Error("two lists that are one string when joined are equal")
	}
	withNone, without := Text(0).WithTokens(), Text(0)
	if withNone == without {
		t.Error("a column that opts out of its dialect's tokens is one that inherits them")
	}
	if Text(0).WithTokens("NA") != Text(0).WithTokens("NA") {
		t.Error("two columns with the same tokens differ")
	}
	dialect := CSV
	dialect.Tokens = NewTokens("NA")
	if dialect == CSV {
		t.Error("a dialect with tokens is the one without")
	}
}

// A \r the fill is holding when the encoding fails ends its row, and the row comes first.
func TestAnEncodingFailureComesAfterEveryIntactRow(t *testing.T) {
	utf16 := func(text string, lone bool) []byte {
		var out []byte
		for _, unit := range text {
			out = append(out, byte(unit), 0)
		}
		if lone {
			out = append(out, 0x00, 0xD8) // a high surrogate, and nothing to pair it with
		}
		return out
	}
	dialect := CSV
	dialect.HasHeader = false
	dialect.Encoding = UTF16LE
	for _, c := range []struct {
		name   string
		text   string
		rows   []string
		record int64
		line   int
		at     int64
	}{
		{"after a \\n", "a\nb\n", []string{"a", "b"}, 2, 3, 8},
		{"after a \\r", "a\nb\r", []string{"a", "b"}, 2, 3, 8},
		{"inside a row", "a\nb", []string{"a"}, 1, 2, 6},
		{"inside a quote", "a\n\"b\r", []string{"a"}, 1, 2, 10},
	} {
		for _, buffer := range []int{1, 7, DefaultBufferBytes} {
			r, err := NewDelimitedReaderBytes(utf16(c.text, true), dialect, []Column{Text(0)}, BufferBytes(buffer))
			if err != nil {
				t.Fatalf("%s: %v", c.name, err)
			}
			var rows []string
			for row, err := range r.All() {
				if err != nil {
					var failure *Failure
					if !errors.As(err, &failure) || failure.Kind != Encoding || failure.Record != c.record || failure.Line != c.line || failure.Byte != c.at {
						t.Errorf("%s, %d bytes: %v", c.name, buffer, err)
					}
					want := fmt.Sprintf("hypertabular: text that isn't valid in its encoding at byte %d (record %d, line %d)", c.at, c.record, c.line)
					if err.Error() != want {
						t.Errorf("%s: %q", c.name, err)
					}
					break
				}
				rows = append(rows, strings.Clone(row.TextString(0)))
			}
			if strings.Join(rows, ",") != strings.Join(c.rows, ",") {
				t.Errorf("%s, %d bytes: rows %q, want %q", c.name, buffer, rows, c.rows)
			}
		}
	}
	if Encoding.String() != "encoding" {
		t.Errorf("the kind's name: %s", Encoding)
	}
}

// A source's own error isn't lost behind the decoder, and neither is one that says nothing forever.
func TestADecodedSourceFailsAsAnyOtherDoes(t *testing.T) {
	dialect := CSV
	dialect.HasHeader = false
	dialect.Encoding = Latin1
	broken := errors.New("the wire is cut")
	r, err := NewDelimitedReader(io.MultiReader(strings.NewReader("a\n"), &stuck{broken}), dialect, []Column{Text(0)})
	if err != nil {
		t.Fatal(err)
	}
	var ended error
	for _, err := range r.All() {
		ended = err
	}
	if !errors.Is(ended, broken) {
		t.Errorf("a source that fails: %v", ended)
	}
	r, err = NewDelimitedReader(&stuck{}, dialect, []Column{Text(0)})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := r.Read(); !errors.Is(err, io.ErrNoProgress) {
		t.Errorf("a source that says nothing: %v", err)
	}
}

// stuck is a source that hands over nothing, with this error or with none.
type stuck struct{ err error }

func (s *stuck) Read([]byte) (int, error) { return 0, s.err }

// Text in memory under an encoding goes through a buffer of its own, and a ceiling holds there too.
func TestARecordTooLongUnderAnEncodingIsAFailure(t *testing.T) {
	dialect := CSV
	dialect.HasHeader = false
	dialect.Encoding = Latin1
	r, err := NewDelimitedReaderBytes([]byte(strings.Repeat("x", 64)+"\n"), dialect, []Column{Text(0)}, rowCeiling(8))
	if err != nil {
		t.Fatal(err)
	}
	var failure *Failure
	if _, err := r.Read(); !errors.As(err, &failure) || failure.Kind != RowTooLong {
		t.Errorf("a record past the ceiling: %v", err)
	}
}

func TestARangeIsWhatTheCoreSaysItIs(t *testing.T) {
	r, err := ParseRange("B4:E12")
	if err != nil {
		t.Fatal(err)
	}
	if r.FirstRow() != 4 || r.FirstColumn() != 2 || r.LastRow() != 12 || r.LastColumn() != 5 {
		t.Errorf("B4:E12 is %+v", r)
	}
	made, err := NewRange(4, 2, 12, 5)
	if err != nil || made != r {
		t.Errorf("NewRange: %+v, %v", made, err)
	}
	for _, text := range []string{"", "A5", "a5:f200", "F200:A5", "A0:B2", "A1:B", " A1:B2"} {
		if _, err := ParseRange(text); !errors.Is(err, ErrRange) || !strings.Contains(err.Error(), fmt.Sprintf("%q", text)) {
			t.Errorf("ParseRange(%q): %v", text, err)
		}
	}
	open, err := NewRange(5, 0, 0, 0)
	if err != nil || open.FirstRow() != 5 || open.LastRow() != 0 {
		t.Errorf("rows from the fifth: %+v, %v", open, err)
	}
	for _, numbers := range [][4]uint32{{5, 0, 4, 0}, {0, 3, 0, 2}} {
		if _, err := NewRange(numbers[0], numbers[1], numbers[2], numbers[3]); !errors.Is(err, ErrRange) {
			t.Errorf("NewRange%v: %v", numbers, err)
		}
	}
}

func TestASheetTakesItsOptions(t *testing.T) {
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "ranges.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	cut, err := ParseRange("B4:E12")
	if err != nil {
		t.Fatal(err)
	}
	options := DefaultSheetOptions
	options.Range = cut
	sheet, err := book.Sheet(0, options, []Column{Text(0)}, BatchRows(2), BufferBytes(1), Notes(false))
	if err != nil {
		t.Fatal(err)
	}
	if sheet.notes || sheet.Options().BatchRows != 2 || sheet.Options().Range != cut {
		t.Errorf("the sheet took %+v, notes %v", sheet.Options(), sheet.notes)
	}
	batch, err := sheet.Read()
	if err != nil {
		t.Fatal(err)
	}
	if batch.Rows() != 2 {
		t.Errorf("a batch of %d rows", batch.Rows())
	}
	if _, err := book.Sheet(0, options, []Column{Text(0)}, BatchRows(0)); err == nil {
		t.Error("no rows a batch: no error")
	}
}

// Notes(false) is what a read asks the core for: no word on a refused cell, where the default has one.
func TestNotesOffAsksForNoNotes(t *testing.T) {
	input := []byte("a\n1x\n")
	plan := []Column{I32(0, hypercast.Invariant)}
	for _, on := range []bool{true, false} {
		r, err := NewDelimitedReaderBytes(input, CSV, plan, Notes(on))
		if err != nil {
			t.Fatal(err)
		}
		batch, err := r.Read()
		if err != nil {
			t.Fatal(err)
		}
		if batch.Rows() != 1 || batch.Verdicts(0)[0].Reason() != hypercast.Malformed {
			t.Fatalf("notes %v: %d rows", on, batch.Rows())
		}
		if got := batch.Detail(0, 0); (got != 0) != on {
			t.Errorf("notes %v: detail %v", on, got)
		}
	}
}

// A batch's list can be longer than a call gives back, a break ends it, and it ends when the reader
// moves on.
func TestABatchsFaultsAreAsLongAsTheyAre(t *testing.T) {
	const rows = 3*problemsHeld + 1
	input := "a,b\n" + strings.Repeat("x,q\n", rows)
	r, err := NewDelimitedReaderBytes([]byte(input), CSV, []Column{I32(0, hypercast.Invariant), Text(1), Bool(1)})
	if err != nil {
		t.Fatal(err)
	}
	batch, err := r.Read()
	if err != nil {
		t.Fatal(err)
	}
	var listed int
	for problem := range batch.Faults() {
		wantColumn := 0
		if listed%2 == 1 {
			wantColumn = 2
		}
		if problem.Row != listed/2 || problem.PlanColumn != wantColumn || problem.SourceColumn != wantColumn/2 ||
			problem.Reason != hypercast.Malformed || problem.Line != listed/2+2 || problem.Detail == 0 {
			t.Errorf("entry %d: %+v", listed, problem)
		}
		listed++
	}
	if listed != 2*rows {
		t.Errorf("%d entries, want %d", listed, 2*rows)
	}
	for range batch.Faults() {
		listed = -1
		break
	}
	if listed != -1 {
		t.Error("a second time over the list found nothing")
	}

	// Ranged over while the reader moves on, it refuses.
	list := batch.Faults()
	listed = 0
	mustPanicStale(t, func() {
		for range list {
			if listed++; listed == 1 {
				if _, err := r.Read(); err != io.EOF {
					t.Fatal(err)
				}
			}
		}
	})
	if listed != 1 {
		t.Errorf("%d entries after the reader moved on", listed)
	}
}

// mustPanicStale runs what should find its faults list past its batch.
func mustPanicStale(t *testing.T, ranged func()) {
	t.Helper()
	defer func() {
		t.Helper()
		if message, _ := recover().(string); message != staleFaults {
			t.Errorf("recovered %q, want %q", message, staleFaults)
		}
	}()
	ranged()
}

// Every door code there could be, 0 and past the last included, with a param beside it: a report and
// its plan say a door that exists and a param it declares, and only a resolved one is read through.
func TestEveryDoorTheCoreCanNameIsAColumn(t *testing.T) {
	for code := uint32(0); code <= uint32(DoorExcelSerial)+1; code++ {
		door := Door(code)
		known := door >= DoorBool && door <= DoorExcelSerial
		for _, param := range []uint32{0, 1, 2, 3, 99} {
			report := reportOf(0, rawSuggestion{door: code, param: param, status: 1, matched: 1, sampled: 1}, nil, nil)
			if !known {
				if report.Door != DoorText {
					t.Errorf("door %d: reported %v", code, report.Door)
				}
				continue
			}
			planned := planOf(0, &report)
			set, err := newColumns([]Column{planned}, 1, false, 0)
			if err != nil {
				t.Errorf("door %v, param %d: plans %+v: %v", door, param, planned, err)
				continue
			}
			valid := param >= 1 && param <= 3
			switch door {
			case DoorUnix, DoorExcelSerial, DoorDateOnlyOrdered, DoorDateTime:
				if _, _, err := (Column{door: door, param: param}).spec(); err != nil {
					valid = false
				} else {
					valid = true
				}
				if want := map[bool]Door{true: door, false: DoorText}[valid]; report.Door != want {
					t.Errorf("door %v, param %d: reported %v", door, param, report.Door)
				}
				if valid && set.specs[0].param != param {
					t.Errorf("door %v, param %d: spec param %d", door, param, set.specs[0].param)
				}
			default:
				if report.Door != door || set.specs[0].param != 0 {
					t.Errorf("door %v, param %d: reported %v with param %d", door, param, report.Door, set.specs[0].param)
				}
			}
		}
		// Ambiguous and unresolved read as text, whatever the door.
		for _, status := range []uint32{2, 3, 0, 9} {
			report := reportOf(0, rawSuggestion{door: code, param: 1 << 2, status: status}, nil, nil)
			if planned := planOf(0, &report); planned.Door() != DoorText || planned.Ordinal() != 0 {
				t.Errorf("door %d, status %d: plans %+v", code, status, planned)
			}
		}
	}
}

// The notations hypertabular_suggest names by number, as this binding declares them: each Western.
func TestAProvedNotationIsWestern(t *testing.T) {
	const flags = 95 // RawSuggestion's NOTATION_FLAGS
	for _, format := range []hypercast.NumFormat{commaGroups, commaDecimal} {
		raw, err := format.Raw()
		if err != nil {
			t.Fatal(err)
		}
		if raw.Flags != flags || uint32(format.Styles) != flags {
			t.Errorf("%+v: flags %d, want %d", format, raw.Flags, flags)
		}
	}
	// Proved by a , or not, the plan is the same: a later 1,23,456 is refused.
	for _, param := range []uint32{0, notationCommaGroups} {
		report := reportOf(0, rawSuggestion{door: uint32(DoorI32), param: param, status: 1}, nil, nil)
		if report.Format == nil || *report.Format != commaGroups || report.Param != 0 {
			t.Fatalf("param %d: %+v", param, report)
		}
		planned := planOf(0, &report)
		r, err := NewDelimitedReaderBytes([]byte("a\n\"1,234\"\n\"1,23,456\"\n"), CSV, []Column{planned})
		if err != nil {
			t.Fatal(err)
		}
		batch, err := r.Read()
		if err != nil {
			t.Fatal(err)
		}
		if batch.I32(0)[0] != 1234 || batch.Verdicts(0)[1].Reason() != hypercast.Malformed {
			t.Errorf("param %d: %v, %v", param, batch.I32(0), batch.Verdicts(0))
		}
	}
	// What only the comma decimal reads is planned in it, and a later group it can't read is refused.
	decimal := reportOf(0, rawSuggestion{door: uint32(DoorF64), param: notationCommaDecimal, status: 1}, nil, nil)
	if decimal.Format == nil || *decimal.Format != commaDecimal || decimal.Param != 0 || len(decimal.Candidates) != 0 {
		t.Fatalf("comma decimal: %+v", decimal)
	}
	r, err := NewDelimitedReaderBytes([]byte("a\n\"0,125\"\n\"1.234,5\"\n\"1,234,567\"\n"), CSV, []Column{planOf(0, &decimal)})
	if err != nil {
		t.Fatal(err)
	}
	batch, err := r.Read()
	if err != nil {
		t.Fatal(err)
	}
	if got := batch.F64(0); got[0] != 0.125 || got[1] != 1234.5 || batch.Verdicts(0)[2].Reason() != hypercast.Malformed {
		t.Errorf("comma decimal read %v, %v", got, batch.Verdicts(0))
	}
	// A date's param of 2 is its order, no notation, and it names no format.
	if date := reportOf(0, rawSuggestion{door: uint32(DoorDateOnlyOrdered), param: 2, status: 1}, nil, nil); date.Format != nil {
		t.Errorf("a date named a format: %+v", date)
	}
	// A column's declaration wins, and the report names it, whatever param says.
	for _, param := range []uint32{notationCommaGroups, notationCommaDecimal} {
		declared := hypercast.Invariant
		report := reportOf(0, rawSuggestion{door: uint32(DoorI32), param: param, status: 1}, nil, &declared)
		if *report.Format != hypercast.Invariant {
			t.Errorf("param %d: the report named %+v", param, report.Format)
		}
	}
	declared := commaDecimal
	report := reportOf(0, rawSuggestion{door: uint32(DoorI32), param: notationCommaGroups, status: 1}, nil, &declared)
	if *report.Format != commaDecimal || planOf(0, &report).Format() != commaDecimal {
		t.Errorf("the plan took %+v, the report %+v", planOf(0, &report).Format(), report.Format)
	}
	// A column that isn't a resolved number names no format.
	for _, found := range []rawSuggestion{
		{door: uint32(DoorText), status: 1},
		{door: uint32(DoorDateOnlyOrdered), param: 2, status: 1},
		{door: uint32(DoorI32), param: notationCommaGroups | notationCommaDecimal, status: 2},
		{door: uint32(DoorI32), status: 3},
	} {
		if report := reportOf(0, found, nil, nil); report.Format != nil {
			t.Errorf("%+v names %+v", found, *report.Format)
		}
	}
	// Ambiguous numbers offer both notations, and ambiguous dates, either date door, the orders.
	report = reportOf(0, rawSuggestion{door: uint32(DoorI32), param: notationCommaGroups | notationCommaDecimal, status: 2}, nil, nil)
	if len(report.Candidates) != 2 || *report.Candidates[0].Format != commaGroups || *report.Candidates[1].Format != commaDecimal {
		t.Errorf("candidates: %+v", report.Candidates)
	}
	for _, door := range []Door{DoorDateOnlyOrdered, DoorDateTime} {
		report = reportOf(0, rawSuggestion{door: uint32(door), param: 1<<2 | 1<<3, status: 2}, nil, nil)
		if len(report.Candidates) != 2 || report.Door != door || report.Param != 2 ||
			report.Candidates[0] != (Candidate{Order: hypercast.MonthDayYear}) || report.Candidates[1] != (Candidate{Order: hypercast.DayMonthYear}) {
			t.Errorf("%v: %+v", door, report)
		}
	}
}

// What a report hands out is its own: writing through it reaches neither the next suggestion nor a
// plan already made.
func TestAReportsFormatsAreItsOwn(t *testing.T) {
	text := []byte("a,b\n7,\"1,234\"\n8,2.5\n")
	first, err := SuggestDelimitedBytes(text, CSV, SuggestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if first.Report[0].Format == nil {
		t.Fatalf("%+v", first.Report[0])
	}
	first.Report[0].Format.GroupSep = "'"
	ambiguous, err := SuggestDelimitedBytes([]byte("a\n\"1,234\"\n"), CSV, SuggestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	ambiguous.Report[0].Candidates[0].Format.GroupSep = "'"
	second, err := SuggestDelimitedBytes(text, CSV, SuggestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if second.Plan[0].Format().GroupSep != "," || second.Report[1].Format.GroupSep != "," {
		t.Errorf("a write through a report reached the next suggestion: %+v", second.Plan[0].Format())
	}
	again, err := SuggestDelimitedBytes([]byte("a\n\"1,234\"\n"), CSV, SuggestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if again.Report[0].Candidates[0].Format.GroupSep != "," {
		t.Errorf("a write through a candidate reached the next suggestion")
	}
	declared := hypercast.NumFormat{DecimalSep: ",", GroupSep: "."}
	mine, err := SuggestDelimitedBytes([]byte("a\n7\n"), CSV, SuggestOptions{Format: &declared})
	if err != nil {
		t.Fatal(err)
	}
	mine.Report[0].Format.GroupSep = "'"
	if declared.GroupSep != "." || mine.Plan[0].Format().GroupSep != "." {
		t.Errorf("a write through a report reached the declared format %+v or the plan %+v", declared, mine.Plan[0].Format())
	}
}

// The list is the batch's it was asked of: asked for and then ranged over after the reader moved on,
// it refuses, and doesn't answer for the next batch or read as no more faults.
func TestFaultsAskedOfABatchStayThatBatchs(t *testing.T) {
	r, err := NewDelimitedReaderBytes([]byte("a\nx\ny\n"), CSV, []Column{I32(0, hypercast.Invariant)}, BatchRows(1))
	if err != nil {
		t.Fatal(err)
	}
	batch, err := r.Read()
	if err != nil {
		t.Fatal(err)
	}
	list := batch.Faults()
	if _, err := r.Read(); err != nil {
		t.Fatal(err)
	}
	mustPanicStale(t, func() {
		for problem := range list {
			t.Errorf("an entry of the next batch: %+v", problem)
		}
	})
	// A list that stopped early, before the reader moved on, has nothing to refuse.
	r, err = NewDelimitedReaderBytes([]byte("a\nx\ny\n"), CSV, []Column{I32(0, hypercast.Invariant)}, BatchRows(1))
	if err != nil {
		t.Fatal(err)
	}
	if batch, err = r.Read(); err != nil {
		t.Fatal(err)
	}
	for range batch.Faults() {
		if _, err := r.Read(); err != nil {
			t.Fatal(err)
		}
		break
	}
}

// A SampleRows left out is 1,000, unlike Rust's sample_rows(0): a struct can't tell it from 0, and
// options that declare only a format mustn't sample one row. One row is SampleRows: 1.
func TestASampleLeftUnsetIsAThousandRows(t *testing.T) {
	text := []byte("a\n1\n2.5\n")
	format := hypercast.Invariant
	for _, options := range []SuggestOptions{{}, {SampleRows: 0}, {Format: &format}} {
		found, err := SuggestDelimitedBytes(text, CSV, options)
		if err != nil {
			t.Fatal(err)
		}
		if got := found.Report[0]; got.Door != DoorF64 || got.Sampled != 2 || found.Truncated {
			t.Errorf("%+v: %+v, truncated %v", options, got, found.Truncated)
		}
	}
	found, err := SuggestDelimitedBytes(text, CSV, SuggestOptions{SampleRows: 1})
	if err != nil {
		t.Fatal(err)
	}
	if got := found.Report[0]; got.Door != DoorBool || got.Sampled != 1 || !found.Truncated {
		t.Errorf("one row: %+v, truncated %v", got, found.Truncated)
	}
	if limit, err := (&SuggestOptions{}).rowsWithin(1 << 20); err != nil || limit != 1000 {
		t.Errorf("unset, past a batch: %d, %v", limit, err)
	}
}

// One batch is the most a sample can be, whatever SampleRows asks: delimited text's, and a sheet's own.
func TestASampleIsOneBatchAtMost(t *testing.T) {
	for _, options := range []SuggestOptions{{}, {SampleRows: 1000}} {
		if limit, err := options.rowsWithin(5); err != nil || limit != 5 {
			t.Errorf("%+v in a batch of 5: %d, %v", options, limit, err)
		}
	}
	text := []byte("a\n" + strings.Repeat("1\n", DefaultBatchRows+1))
	found, err := SuggestDelimitedBytes(text, CSV, SuggestOptions{SampleRows: DefaultBatchRows + 1})
	if err != nil {
		t.Fatal(err)
	}
	if got := found.Report[0]; got.Sampled != DefaultBatchRows || !found.Truncated {
		t.Errorf("past a batch: %+v, truncated %v", got, found.Truncated)
	}
}

// failingAfter delivers some batches and then fails, as a source that breaks does.
type failingAfter struct {
	reader  *DelimitedReader
	deliver int
}

func (f *failingAfter) Bind([]Column) error { return nil }
func (*failingAfter) setBatchRows(int)      {}
func (*failingAfter) sizeFlags() uint32     { return 0 }

func (f *failingAfter) Read() (*Batch, error) {
	if f.deliver == 0 {
		return nil, errors.New("the source broke")
	}
	f.deliver--
	return f.reader.Read()
}

// A failure just past a whole sample only says there's more; among the sampled rows it's the caller's.
func TestAFailureJustPastTheSampleSaysTruncated(t *testing.T) {
	source := func(batchRows int) *failingAfter {
		r, err := NewDelimitedReaderBytes([]byte("a\n1\n2\n3\n4\n"), CSV, []Column{Text(0)}, BatchRows(batchRows))
		if err != nil {
			t.Fatal(err)
		}
		return &failingAfter{reader: r, deliver: 1}
	}
	s, err := sampleOf(source(2), 1, 2, false)
	if err != nil {
		t.Fatal(err)
	}
	if s.rows != 2 || !s.truncated {
		t.Errorf("past the sample: %d rows, truncated %v", s.rows, s.truncated)
	}
	if _, err := sampleOf(source(1), 1, 2, false); err == nil || err.Error() != "the source broke" {
		t.Errorf("among the sampled rows: %v", err)
	}
}

// Bytes a source returns beside an error are read first: the intact rows in them come out, and the
// error is what the read then ends on.
func TestBytesBesideASourcesErrorAreReadFirst(t *testing.T) {
	broken := errors.New("the wire is cut")
	for _, encoding := range []TextEncoding{UTF8, Latin1} {
		dialect := CSV
		dialect.Encoding = encoding
		r, err := NewDelimitedReader(&once{"a\n1\n2\n", broken}, dialect, []Column{Text(0)})
		if err != nil {
			t.Fatalf("%v: %v", encoding, err)
		}
		var rows []string
		var ended error
		for row, err := range r.All() {
			if err != nil {
				ended = err
				break
			}
			rows = append(rows, strings.Clone(row.TextString(0)))
		}
		if strings.Join(rows, ",") != "1,2" || !errors.Is(ended, broken) {
			t.Errorf("%v: rows %q, ended %v", encoding, rows, ended)
		}
	}
}

// once is a source that hands over everything and its error in one call, then says it has ended, so
// that an error not kept from that one call is lost.
type once struct {
	text string
	err  error
}

func (o *once) Read(p []byte) (int, error) {
	n := copy(p, o.text)
	o.text = o.text[n:]
	switch {
	case n == 0:
		return 0, io.EOF
	case o.text == "":
		return n, o.err
	}
	return n, nil
}

func TestSuggestTakesItsOptions(t *testing.T) {
	text := []byte("a,b\n\"1,234\",x\n\"5,678\",y\n9,z\n")
	if _, err := SuggestDelimitedBytes(text, CSV, SuggestOptions{SampleRows: -1}); err == nil || !strings.Contains(err.Error(), "SampleRows") {
		t.Errorf("a negative sample: %v", err)
	}
	bad := hypercast.NumFormat{DecimalSep: ".", GroupSep: "."}
	if _, err := SuggestDelimitedBytes(text, CSV, SuggestOptions{Format: &bad}); err == nil {
		t.Error("a format HyperCast refuses")
	}
	found, err := SuggestDelimitedBytes(text, CSV, SuggestOptions{SampleRows: 1})
	if err != nil {
		t.Fatal(err)
	}
	if !found.Truncated || found.Report[0].Sampled != 1 || *found.Report[0].Name != "a" || *found.Report[1].Name != "b" {
		t.Errorf("a sample of one: %+v", found)
	}
	// A notation for one column, ahead of another for all of them.
	all, one := hypercast.Invariant, commaDecimal
	found, err = SuggestDelimitedBytes(text, CSV, SuggestOptions{Format: &all, ColumnFormats: map[int]hypercast.NumFormat{0: one}})
	if err != nil {
		t.Fatal(err)
	}
	if found.Plan[0].Door() != DoorF64 || found.Plan[0].Format() != one {
		t.Errorf("a column's own notation: %+v, %+v", found.Report[0], found.Plan[0])
	}

	// With no header, the width is the first record's, and no column has a name.
	dialect := CSV
	dialect.HasHeader = false
	found, err = SuggestDelimitedBytes([]byte("1,x,2\n3,y,4\n"), dialect, SuggestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if len(found.Plan) != 3 || found.Report[0].Name != nil || found.Report[0].Door != DoorBool && found.Report[0].Door != DoorI32 {
		t.Errorf("no header: %+v", found)
	}
	// Nothing to read is nothing to say.
	found, err = SuggestDelimitedBytes(nil, dialect, SuggestOptions{})
	if err != nil || len(found.Plan) != 0 || len(found.Report) != 0 || found.Truncated {
		t.Errorf("nothing: %+v, %v", found, err)
	}
	// A failure among the sampled rows is the suggestion's, and just past them says only that there's
	// more.
	broken := []byte("a,b\n1,2\n3\n")
	if _, err := SuggestDelimitedBytes(broken, CSV, SuggestOptions{}); err == nil {
		t.Error("a failure among the sampled rows")
	}
	found, err = SuggestDelimitedBytes(broken, CSV, SuggestOptions{SampleRows: 1})
	if err != nil || !found.Truncated {
		t.Errorf("a failure past the sample: %+v, %v", found, err)
	}
	// A file that isn't there is the open's error, not a suggestion's.
	if _, err := SuggestDelimited(missingPath(t, "missing.csv"), CSV, SuggestOptions{}); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("a missing file: %v", err)
	}
}

// A source too wide to plan is refused before it's read through, and one with no columns takes none.
func TestASampleFitsAPlan(t *testing.T) {
	if _, err := sampleOf(&unreadable{t}, maxOrdinal+2, 1, false); err == nil || !strings.Contains(err.Error(), "2^20") {
		t.Errorf("too wide: %v", err)
	}
	sample, err := sampleOf(&unreadable{t}, 0, 1, false)
	if err != nil || sample.rows != 0 || len(sample.columns) != 0 {
		t.Errorf("no columns: %+v, %v", sample, err)
	}
}

type unreadable struct{ t *testing.T }

func (u *unreadable) Bind([]Column) error {
	u.t.Error("bound")
	return nil
}
func (*unreadable) setBatchRows(int)  {}
func (*unreadable) sizeFlags() uint32 { return 0 }

func (u *unreadable) Read() (*Batch, error) {
	u.t.Error("read")
	return nil, io.EOF
}

// A sheet's suggestion, as delimited text's: a read of its own, from the start, that leaves this one
// where it was; a header's names, or a width from the first row; and a range's ordinals.
func TestASheetSuggests(t *testing.T) {
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "ranges.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	cut, err := ParseRange("B4:E12")
	if err != nil {
		t.Fatal(err)
	}
	options := DefaultSheetOptions
	options.Range = cut
	sheet, err := book.SheetUnbound(0, options)
	if err != nil {
		t.Fatal(err)
	}
	found, err := sheet.Suggest(SuggestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if sheet.IsBound() {
		t.Error("the suggestion bound the sheet it was asked of")
	}
	names := make([]string, 0, len(found.Report))
	for index := range found.Report {
		names = append(names, *found.Report[index].Name)
	}
	if strings.Join(names, ",") != "id,name,amt,day" {
		t.Errorf("names %q", names)
	}
	// "merged" beside the ids is a cell no door takes, beside cells one does.
	if found.Plan[0].Door() != DoorText || found.Report[0].Status != SuggestUnresolved || found.Report[0].Matched != 6 {
		t.Errorf("id: %+v", found.Report[0])
	}
	if found.Plan[1].Door() != DoorText {
		t.Errorf("name: %+v", found.Report[1])
	}
	// A typed date's canonical text isn't taken as a date: it's text, resolved, by hand.
	if found.Plan[3].Door() != DoorText {
		t.Errorf("day: %+v", found.Report[3])
	}
	// The plan reads this sheet, from its start.
	if err := sheet.Bind(found.Plan); err != nil {
		t.Fatal(err)
	}
	batch, err := sheet.Read()
	if err != nil || batch.Rows() != 7 {
		t.Fatalf("the plan read %v, %v", batch, err)
	}

	// No header: the width is the first row's.
	options.HasHeader = false
	headless, err := book.SheetUnbound(0, options)
	if err != nil {
		t.Fatal(err)
	}
	found, err = headless.Suggest(SuggestOptions{SampleRows: 2})
	if err != nil {
		t.Fatal(err)
	}
	if len(found.Plan) != 4 || found.Report[0].Name != nil || !found.Truncated || found.Report[0].Sampled != 2 {
		t.Errorf("no header: %+v", found)
	}
}

// Every sheet of every package: a plan that reads what it sampled, and a suggestion that fails only
// where a read does.
func TestNoSheetsPlanRefusesACellItSampled(t *testing.T) {
	directory := filepath.Dir(repositoryFile(t, "corpus", "workbook.json"))
	files, err := filepath.Glob(filepath.Join(directory, "workbook", "*"))
	if err != nil {
		t.Fatal(err)
	}
	planned, typed, refused := 0, 0, 0
	indian := hypercast.Invariant.WithGrouping(hypercast.GroupingIndian)
	declarations := []*hypercast.NumFormat{nil, &commaGroups, &commaDecimal, &indian}
	for _, path := range files {
		if strings.HasSuffix(path, ".py") {
			continue
		}
		book, err := OpenWorkbook(path)
		if err != nil {
			continue
		}
		for at := range book.Sheets() {
			for _, declared := range declarations {
				sheet, err := book.SheetUnbound(at, DefaultSheetOptions)
				if err != nil {
					continue
				}
				found, err := sheet.Suggest(SuggestOptions{Format: declared})
				if err != nil {
					refused++
					if !sheetReadFails(t, book, at) {
						t.Errorf("%s, sheet %d: %v, where a read has no failure", path, at, err)
					}
					continue
				}
				if len(found.Plan) == 0 {
					continue
				}
				planned++
				for _, column := range found.Plan {
					if column.Door() != DoorText {
						typed++
					}
				}
				read, err := book.Sheet(at, DefaultSheetOptions, found.Plan)
				if err != nil {
					t.Errorf("%s, sheet %d: %v", path, at, err)
					continue
				}
				for rows := 0; rows < defaultSampleRows; {
					batch, err := read.Read()
					if err == io.EOF {
						break
					}
					if err != nil {
						t.Errorf("%s, sheet %d: %v", path, at, err)
						break
					}
					for problem := range batch.Faults() {
						if problem.Row+rows < defaultSampleRows && problem.Reason != hypercast.Empty && found.Plan[problem.PlanColumn].Door() != DoorText {
							t.Errorf("%s, sheet %d, %+v declared: %+v", path, at, declared, problem)
						}
					}
					rows += batch.Rows()
				}
			}
		}
	}
	if planned <= 80 || refused < 2 || typed <= 350 {
		t.Errorf("%d sheets planned, %d refused, %d typed columns", planned, refused, typed)
	}
}

// A typed number has no locale: a workbook's number column is a number whether a notation is declared
// or not, and whichever.
func TestAWorkbooksNumberColumnIsNumericUnderAnyFormat(t *testing.T) {
	for file, columns := range map[string][]string{
		"basic.xlsx":          {"amount", "ratio"},
		"basic.ods":           {"amount", "pct"},
		"excel-web-data.xlsx": {"pct", "money"},
	} {
		book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", file))
		if err != nil {
			t.Fatal(err)
		}
		for _, declared := range []*hypercast.NumFormat{nil, &commaGroups, &commaDecimal} {
			sheet, err := book.SheetUnbound(0, DefaultSheetOptions)
			if err != nil {
				t.Fatal(err)
			}
			found, err := sheet.Suggest(SuggestOptions{Format: declared})
			if err != nil {
				t.Fatal(err)
			}
			for _, name := range columns {
				at := slices.IndexFunc(found.Report, func(report SuggestedColumn) bool {
					return report.Name != nil && *report.Name == name
				})
				if at < 0 {
					t.Fatalf("%s has no %s", file, name)
				}
				if report := found.Report[at]; report.Status != SuggestResolved || !report.Door.numeric() {
					t.Errorf("%s %s, %+v declared: %+v", file, name, declared, report)
				}
			}
			// The sample's notes are its own, whatever a read of the sheet was to have.
			quiet, err := book.SheetUnbound(0, DefaultSheetOptions, Notes(false))
			if err != nil {
				t.Fatal(err)
			}
			again, err := quiet.Suggest(SuggestOptions{Format: declared})
			if err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(again.Report, found.Report) {
				t.Errorf("%s, %+v declared, notes off: %+v", file, declared, again.Report)
			}
		}
	}
}

// A declared order answers the sample's question and only that.
func TestADeclaredDateOrderSettlesADateTheSampleLeftOpen(t *testing.T) {
	open := []byte("day\n01/02/2026\n03/04/2026\n")
	ask := func(input []byte, options SuggestOptions) (SuggestedColumn, Column) {
		t.Helper()
		found, err := SuggestDelimitedBytes(input, CSV, options)
		if err != nil {
			t.Fatal(err)
		}
		return found.Report[0], found.Plan[0]
	}
	report, plan := ask(open, SuggestOptions{})
	if report.Status != SuggestAmbiguous || plan.Door() != DoorText {
		t.Errorf("nothing declared: %+v, %+v", report, plan)
	}
	day, month, year := hypercast.DayMonthYear, hypercast.MonthDayYear, hypercast.YearMonthDay
	report, plan = ask(open, SuggestOptions{DateOrder: &day})
	if report.Status != SuggestResolved || len(report.Candidates) != 0 || plan.Door() != DoorDateOnlyOrdered || report.Param != uint32(day) {
		t.Errorf("day first: %+v, %+v", report, plan)
	}
	report, plan = ask(open, SuggestOptions{DateOrder: &day, ColumnDateOrders: map[int]hypercast.DateOrder{0: month}})
	if report.Status != SuggestResolved || plan.Door() != DoorDateOnlyOrdered || report.Param != uint32(month) {
		t.Errorf("a column's own: %+v, %+v", report, plan)
	}
	report, _ = ask([]byte("day\n13/02/2026\n"), SuggestOptions{DateOrder: &month})
	if report.Status != SuggestResolved || report.Param != uint32(day) {
		t.Errorf("an order the cells rule out: %+v", report)
	}
	report, plan = ask(open, SuggestOptions{DateOrder: &year})
	if report.Status != SuggestAmbiguous || plan.Door() != DoorText {
		t.Errorf("an order out of the set: %+v, %+v", report, plan)
	}
}

func sheetReadFails(t *testing.T, book *Workbook, at int) bool {
	t.Helper()
	sheet, err := book.SheetUnbound(at, DefaultSheetOptions)
	if err != nil {
		return true
	}
	if err := sheet.Bind([]Column{Text(0)}); err != nil {
		return true
	}
	for {
		switch _, err := sheet.Read(); err {
		case nil:
		case io.EOF:
			return false
		default:
			return true
		}
	}
}

// A sheet answers from its first row whatever it has been up to, and carries on where it was.
func TestAPartReadSheetAnswersTheSameAndStaysWhereItIs(t *testing.T) {
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "libreoffice-data.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	options := DefaultSheetOptions
	options.BatchRows = 1
	unbound, err := book.SheetUnbound(0, options)
	if err != nil {
		t.Fatal(err)
	}
	fresh, err := unbound.Suggest(SuggestOptions{SampleRows: 1})
	if err != nil {
		t.Fatal(err)
	}
	if len(fresh.Plan) == 0 {
		t.Fatal("no columns")
	}
	plan := make([]Column, len(fresh.Plan))
	for ordinal := range plan {
		plan[ordinal] = Text(ordinal)
	}
	whole, err := book.Sheet(0, options, plan)
	if err != nil {
		t.Fatal(err)
	}
	part, err := book.Sheet(0, options, plan)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := whole.Read(); err != nil {
		t.Fatal(err)
	}
	second, err := whole.Read()
	if err != nil {
		t.Fatal(err)
	}
	wantLine := second.Line(0)
	if _, err := part.Read(); err != nil {
		t.Fatal(err)
	}
	answer, err := part.Suggest(SuggestOptions{SampleRows: 1})
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(answer.Plan, fresh.Plan) || len(answer.Report) != len(fresh.Report) {
		t.Errorf("the plan %+v, fresh %+v", answer.Plan, fresh.Plan)
	}
	next, err := part.Read()
	if err != nil {
		t.Fatal(err)
	}
	if next.Line(0) != wantLine {
		t.Errorf("carried on at line %d, want %d", next.Line(0), wantLine)
	}
}

// The core decides how much a batch may ask for, and every way in asks it before allocating. These
// sizes are past what a translated core's 4 GiB memory holds, so that if the check went they would
// stop there, with nothing written: proved in a 4 GiB container with the check reverted.
func TestTheCoreJudgesABatchsSize(t *testing.T) {
	input := []byte("a\n1\n")
	plan := []Column{I32(0, hypercast.Invariant)}
	const wholeMessage = "hypertabular: BatchRows asks for 5377130496 bytes, past the limit of 4294967296"
	if _, err := NewDelimitedReaderBytes(input, CSV, plan, BatchRows(1<<28)); err == nil || err.Error() != wholeMessage {
		t.Errorf("bytes: %v", err)
	}
	if _, err := NewDelimitedReader(strings.NewReader(string(input)), CSV, plan, BatchRows(1<<28)); err == nil || err.Error() != wholeMessage {
		t.Errorf("a stream: %v", err)
	}
	late, err := NewDelimitedReaderBytesUnbound(input, CSV, BatchRows(1<<28))
	if err != nil {
		t.Fatal(err)
	}
	if err := late.Bind(plan); err == nil || !strings.Contains(err.Error(), "past the limit of 4294967296") {
		t.Errorf("a late plan: %v", err)
	}
	if late.IsBound() {
		t.Error("a refused plan was taken")
	}

	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	const sheetMessage = "hypertabular: BatchRows asks for 9663676432 bytes, past the limit of 4294967296"
	if _, err := book.Sheet(0, DefaultSheetOptions, plan, BatchRows(1<<28)); err == nil || err.Error() != sheetMessage {
		t.Errorf("a sheet: %v", err)
	}
	unplanned, err := book.SheetUnbound(0, DefaultSheetOptions, BatchRows(1<<28))
	if err != nil {
		t.Fatal(err)
	}
	if err := unplanned.Bind(plan); err == nil || err.Error() != sheetMessage {
		t.Errorf("a late plan on a sheet: %v", err)
	}
	if unplanned.IsBound() {
		t.Error("a refused plan was taken")
	}

	modest, err := NewDelimitedReaderBytes(input, CSV, plan, BatchRows(1<<16))
	if err != nil {
		t.Fatal(err)
	}
	if batch, err := modest.Read(); err != nil || batch.Rows() != 1 {
		t.Errorf("a modest batch: %v, %v", batch, err)
	}
}

// A plan of no columns has no arrays, so the core passes it at any size. The size stays the core's
// to read as a count: one past 32 bits must read the rows, not wrap to none or to one a batch. A
// regression here has nothing to allocate: it reads the wrong rows.
func TestABatchSizePast32BitsReadsItsRows(t *testing.T) {
	if strconv.IntSize < 64 {
		t.Skip("an int doesn't hold this size here")
	}
	input := []byte("a\n1\n2\n")
	for _, shift := range []uint{31, 32, 33, 40} {
		for _, extra := range []int{0, 1} {
			rows := 1<<shift + extra
			reader, err := NewDelimitedReaderBytes(input, CSV, []Column{}, BatchRows(rows))
			if err != nil {
				t.Fatalf("%d: %v", rows, err)
			}
			batch, err := reader.Read()
			if err != nil || batch.Rows() != 2 {
				t.Errorf("%d rows a batch: %v, %v", rows, batch, err)
			}
			if _, err := reader.Read(); err != io.EOF {
				t.Errorf("%d rows a batch, then: %v", rows, err)
			}
		}
	}
}

// A path's sizes are judged before the file is touched, so a bad one beats a missing file.
func TestAPathsSizesAreJudgedBeforeTheFileIsOpened(t *testing.T) {
	plan := []Column{I32(0, hypercast.Invariant)}
	if _, err := OpenDelimited("no such file", CSV, plan); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("no size to refuse: %v", err)
	}
	for name, attempt := range map[string]func() (*DelimitedReader, error){
		"BatchRows must be at least 1": func() (*DelimitedReader, error) {
			return OpenDelimitedUnbound("no such file", CSV, BatchRows(0))
		},
		"BufferBytes must be at least 1": func() (*DelimitedReader, error) {
			return OpenDelimited("no such file", CSV, plan, BufferBytes(0))
		},
		"past the limit of 4294967296": func() (*DelimitedReader, error) {
			return OpenDelimited("no such file", CSV, plan, BatchRows(1<<28))
		},
	} {
		if _, err := attempt(); err == nil || !strings.Contains(err.Error(), name) {
			t.Errorf("%s: %v", name, err)
		}
	}
}

// A sampler's batch is never why a suggestion is refused: nobody set its size.
func TestASuggestionIsNotRefusedForTheSizeOfItsOwnBatch(t *testing.T) {
	const width = wideSuggestionColumns
	names := make([]string, width)
	for column := range names {
		names[column] = "c" + strconv.Itoa(column)
	}
	row := strings.TrimSuffix(strings.Repeat("1,", width), ",")
	text := []byte(strings.Join(names, ",") + "\n" + row + "\n" + row + "\n")
	wide, err := SuggestDelimitedBytes(text, CSV, SuggestOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if len(wide.Report) != width || wide.Report[width-1].Sampled != 2 || wide.Truncated {
		t.Errorf("%d columns, %d sampled, truncated %v", len(wide.Report), wide.Report[width-1].Sampled, wide.Truncated)
	}

	// At exactly the limit with no plan, and a sample past every batch.
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	options := DefaultSheetOptions
	options.BatchRows = 1 << 29
	atTheLimit, err := book.SheetUnbound(0, options)
	if err != nil {
		t.Fatal(err)
	}
	large, err := atTheLimit.Suggest(SuggestOptions{SampleRows: 1 << 30})
	if err != nil {
		t.Fatal(err)
	}
	modestSheet, err := book.SheetUnbound(0, DefaultSheetOptions)
	if err != nil {
		t.Fatal(err)
	}
	modest, err := modestSheet.Suggest(SuggestOptions{SampleRows: 4096})
	if err != nil {
		t.Fatal(err)
	}
	if words(large) != words(modest) {
		t.Errorf("%s\nand\n%s", words(large), words(modest))
	}
}

// A sample says the same in any number of batches, delimited and sheet.
func TestASampleSaysTheSameInAnyNumberOfBatches(t *testing.T) {
	text := []byte("id,price,when,note\n1,\"1,234.50\",2024-01-02,plain\n2,1.234,2024-02-03,\n3,,2024-03-04,\"a, b\"\nx,0.5,03/04/2024,\"two\nlines\"\n5,12,2024-05-06,more\n6,13,,more\n")
	for _, limit := range []int{1, 2, 3, 5, 6, 1000} {
		options := SuggestOptions{SampleRows: limit}
		whole, err := SuggestDelimitedBytes(text, CSV, options)
		if err != nil {
			t.Fatal(err)
		}
		for _, rows := range []int{1, 2, 3, 7} {
			reader, err := NewDelimitedReaderBytesUnbound(text, CSV, BatchRows(1), Notes(false))
			if err != nil {
				t.Fatal(err)
			}
			taken, err := sampleIn(reader, len(reader.Header()), limit, false, func([]Column) (int, error) { return rows, nil })
			if err != nil {
				t.Fatal(err)
			}
			found, err := taken.judge(reader.Header(), &options)
			if err != nil {
				t.Fatal(err)
			}
			if words(found) != words(whole) {
				t.Errorf("sample %d in batches of %d: %s, not %s", limit, rows, words(found), words(whole))
			}
		}
	}
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	for _, limit := range []int{1, 2, 3, 1000} {
		options := SuggestOptions{SampleRows: limit}
		opened, err := book.SheetUnbound(0, DefaultSheetOptions)
		if err != nil {
			t.Fatal(err)
		}
		whole, err := opened.Suggest(options)
		if err != nil {
			t.Fatal(err)
		}
		for _, rows := range []int{1, 2, 3} {
			sheet, err := book.SheetUnbound(0, DefaultSheetOptions)
			if err != nil {
				t.Fatal(err)
			}
			taken, err := sampleIn(sheet, len(sheet.Header()), limit, true, func([]Column) (int, error) { return rows, nil })
			if err != nil {
				t.Fatal(err)
			}
			found, err := taken.judge(sheet.Header(), &options)
			if err != nil {
				t.Fatal(err)
			}
			if words(found) != words(whole) {
				t.Errorf("sheet sample %d in batches of %d: %s, not %s", limit, rows, words(found), words(whole))
			}
		}
	}
	// A control: the sample's size does move the answer, so the comparison above can fail.
	one, _ := SuggestDelimitedBytes(text, CSV, SuggestOptions{SampleRows: 1})
	six, _ := SuggestDelimitedBytes(text, CSV, SuggestOptions{SampleRows: 6})
	if words(one) == words(six) {
		t.Errorf("the comparison can't tell these apart: %s", words(one))
	}
}

// With no header, a short and a long row, and row widths kept, across each batch boundary.
func TestARaggedHeaderlessSampleSaysTheSameInAnyNumberOfBatches(t *testing.T) {
	ragged := []byte("1,x\n2,y,long\n3\n4,z\n5,w\n")
	headerless := CSV
	headerless.HasHeader, headerless.ShortRows, headerless.LongRows = false, ShortRowsPad, LongRowsReport
	for _, limit := range []int{1, 2, 3, 4, 5, 6} {
		options := SuggestOptions{SampleRows: limit}
		whole, err := SuggestDelimitedBytes(ragged, headerless, options)
		if err != nil {
			t.Fatal(err)
		}
		if whole.Report[0].Sampled != min(limit, 5) || whole.Truncated != (limit < 5) {
			t.Errorf("sample %d: %s", limit, words(whole))
		}
		for _, rows := range []int{1, 2, 3, 4} {
			reader, err := NewDelimitedReaderBytesUnbound(ragged, headerless, BatchRows(1), Notes(false))
			if err != nil {
				t.Fatal(err)
			}
			taken, err := sampleIn(reader, 2, limit, false, func([]Column) (int, error) { return rows, nil })
			if err != nil {
				t.Fatal(err)
			}
			found, err := taken.judge(reader.Header(), &options)
			if err != nil {
				t.Fatal(err)
			}
			if words(found) != words(whole) {
				t.Errorf("sample %d in batches of %d: %s, not %s", limit, rows, words(found), words(whole))
			}
		}
	}
}

func words(found *Suggestion) string {
	var out strings.Builder
	for _, column := range found.Report {
		fmt.Fprintf(&out, "%d:%v:%v:%d/%d;", column.Ordinal, column.Door, column.Status, column.Matched, column.Sampled)
	}
	fmt.Fprintf(&out, "%v", found.Truncated)
	return out.String()
}

// One row past the limit at any batch size names its width: no public plan gets there, so this asks
// directly.
func TestARowPastTheLimitNamesItsWidth(t *testing.T) {
	spec, _, err := I32(0, hypercast.Invariant).spec()
	if err != nil {
		t.Fatal(err)
	}
	spec.ordinal = math.MaxUint32
	_, err = checkSizes([]rawSpec{spec}, 1, 1, batchSheet)
	const message = "hypertabular: a row of 4294967296 columns asks for 68719476768 bytes, past the limit of 4294967296"
	if err == nil || err.Error() != message {
		t.Errorf("%v", err)
	}
}

// An explicit zero is refused by the core, for each option, where an omitted option has its
// default. SheetOptions is a struct, which can't tell 0 from a field left out, so 0 there is the
// default; the BatchRows option can tell, and refuses it.
func TestTheCoreRefusesAZeroWhereAnOmittedSizeHasItsDefault(t *testing.T) {
	input := []byte("a\n1\n")
	plan := []Column{I32(0, hypercast.Invariant)}
	const noRows = "hypertabular: BatchRows must be at least 1"
	const noBuffer = "hypertabular: BufferBytes must be at least 1"
	if _, err := NewDelimitedReaderBytes(input, CSV, plan, BatchRows(0)); err == nil || err.Error() != noRows {
		t.Errorf("BatchRows(0): %v", err)
	}
	if _, err := NewDelimitedReader(strings.NewReader(string(input)), CSV, plan, BufferBytes(0)); err == nil || err.Error() != noBuffer {
		t.Errorf("BufferBytes(0): %v", err)
	}
	if _, err := NewDelimitedReaderBytes(input, CSV, plan, BatchRows(-1)); err == nil || !strings.Contains(err.Error(), "negative") {
		t.Errorf("BatchRows(-1): %v", err)
	}

	omitted, err := NewDelimitedReaderBytes(input, CSV, plan)
	if err != nil {
		t.Fatal(err)
	}
	if omitted.batchRows != DefaultBatchRows {
		t.Errorf("omitted, %d rows a batch", omitted.batchRows)
	}

	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := book.Sheet(0, DefaultSheetOptions, plan, BatchRows(0)); err == nil || err.Error() != noRows {
		t.Errorf("a sheet's BatchRows(0): %v", err)
	}
	if _, err := book.Sheet(0, DefaultSheetOptions, plan, BufferBytes(0)); err == nil || err.Error() != noBuffer {
		t.Errorf("a sheet's BufferBytes(0): %v", err)
	}
	options := DefaultSheetOptions
	options.BatchRows = 0
	sheet, err := book.Sheet(0, options, plan)
	if err != nil {
		t.Fatal(err)
	}
	if sheet.Options().BatchRows != DefaultBatchRows {
		t.Errorf("a struct's 0 is %d rows a batch", sheet.Options().BatchRows)
	}
}
