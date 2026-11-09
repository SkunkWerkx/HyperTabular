package hypertabular

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// The bytes every test here reads: each column holds a shape of each policy, so a plan can read them
// both ways in one read.
const groupingInput = "indian,western\n\"1,23,456\",\"1,23,456\"\n\"1,234,567\",\"1,234,567\"\n"

var indian = hypercast.Invariant.WithGrouping(hypercast.GroupingIndian)

// A stale format: 351 and 863 hold the retired bit 8, and only a literal or a reflective decoder can
// make one, since NewNumFormat and WithStyles panic on it. 95 and 607 are the same format without
// that bit, Western and Indian, the control that a call refuses for the word and nothing else.
func worded(styles uint32) hypercast.NumFormat {
	return hypercast.NumFormat{DecimalSep: ".", GroupSep: ",", Styles: hypercast.NumStyles(styles)}
}

var stalePolicies = []struct {
	styles  uint32
	refused bool
}{{351, true}, {863, true}, {95, false}, {607, false}}

// holds fails unless err says what the row wants.
func holds(t *testing.T, what string, styles uint32, refused bool, err error) {
	t.Helper()
	if refused != (err != nil) {
		t.Errorf("%s, %d: refused %v, error %v", what, styles, refused, err)
	}
}

func TestAnIndianColumnReadsBesideAWesternOne(t *testing.T) {
	// Twice: named, and with no policy said, which is Western.
	for _, western := range []hypercast.NumFormat{hypercast.Invariant, {DecimalSep: ".", GroupSep: ",", Styles: hypercast.AllStyles}} {
		r := open(t, groupingInput, CSV, []Column{I32(0, indian), I32(1, western)})
		batch, err := r.Read()
		if err != nil || batch.Rows() != 2 {
			t.Fatalf("%v, %v", batch, err)
		}
		if got := batch.Verdicts(0); !got[0].OK() || batch.I32(0)[0] != 123456 || got[1].Reason() != hypercast.Malformed {
			t.Errorf("Indian: %v %v", batch.I32(0), got)
		}
		if got := batch.Verdicts(1); got[0].Reason() != hypercast.Malformed || !got[1].OK() || batch.I32(1)[1] != 1234567 {
			t.Errorf("Western: %v %v", batch.I32(1), got)
		}
	}
}

func TestAnIndianSheetColumnReadsBesideAWesternOne(t *testing.T) {
	for _, file := range []string{"grouping.xlsx", "grouping.ods", "grouping.numbers"} {
		book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", file))
		if err != nil {
			t.Fatal(err)
		}
		sheet, err := book.Sheet(0, DefaultSheetOptions, []Column{I32(2, indian), I32(2, hypercast.Invariant)})
		if err != nil {
			t.Fatal(err)
		}
		batch, err := sheet.Read()
		if err != nil || batch.Rows() != 3 {
			t.Fatalf("%s: %v, %v", file, batch, err)
		}
		if got := batch.Verdicts(0); !got[0].OK() || batch.I32(0)[0] != 123456 || got[1].Reason() != hypercast.Malformed || batch.I32(0)[2] != 1234 {
			t.Errorf("%s, Indian: %v %v", file, batch.I32(0), got)
		}
		if got := batch.Verdicts(1); got[0].Reason() != hypercast.Malformed || !got[1].OK() || batch.I32(1)[1] != 1234567 || batch.I32(1)[2] != 1234 {
			t.Errorf("%s, Western: %v %v", file, batch.I32(1), got)
		}
	}
}

func TestAStalePolicyIsRefusedAtEveryDelimitedEntryPoint(t *testing.T) {
	directory, ok := tempDir(t)
	if !ok {
		t.Skip("no temporary directory to write a file in")
	}
	path := filepath.Join(directory, "grouping.csv")
	if err := os.WriteFile(path, []byte(groupingInput), 0o600); err != nil {
		t.Fatal(err)
	}
	for _, row := range stalePolicies {
		plan := []Column{I32(0, worded(row.styles))}
		_, err := NewDelimitedReaderBytes([]byte(groupingInput), CSV, plan)
		holds(t, "bytes", row.styles, row.refused, err)
		_, err = NewDelimitedReader(strings.NewReader(groupingInput), CSV, plan)
		holds(t, "reader", row.styles, row.refused, err)
		_, err = OpenDelimited(path, CSV, plan)
		holds(t, "path", row.styles, row.refused, err)

		for name, opener := range map[string]func() (*DelimitedReader, error){
			"bytes, unbound": func() (*DelimitedReader, error) {
				return NewDelimitedReaderBytesUnbound([]byte(groupingInput), CSV)
			},
			"reader, unbound": func() (*DelimitedReader, error) {
				return NewDelimitedReaderUnbound(strings.NewReader(groupingInput), CSV)
			},
			"path, unbound": func() (*DelimitedReader, error) {
				return OpenDelimitedUnbound(path, CSV)
			},
		} {
			r, err := opener()
			if err != nil {
				t.Fatal(err)
			}
			err = r.Bind(plan)
			holds(t, name, row.styles, row.refused, err)
			if row.refused {
				if _, err := r.Read(); !errors.Is(err, ErrUnbound) {
					t.Errorf("%s, %d: a read after the refusal: %v", name, row.styles, err)
				}
			}
			_ = r.Close()
		}

		_, err = SuggestDelimitedBytes([]byte(groupingInput), CSV, SuggestOptions{Format: &plan[0].format})
		holds(t, "suggest bytes", row.styles, row.refused, err)
		_, err = SuggestDelimited(path, CSV, SuggestOptions{ColumnFormats: map[int]hypercast.NumFormat{1: worded(row.styles)}})
		holds(t, "suggest path", row.styles, row.refused, err)
	}
}

func TestAStalePolicyIsRefusedAtEverySheetEntryPoint(t *testing.T) {
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "grouping.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	for _, row := range stalePolicies {
		format := worded(row.styles)
		plan := []Column{I32(0, format)}
		_, err := book.Sheet(0, DefaultSheetOptions, plan)
		holds(t, "sheet", row.styles, row.refused, err)

		sheet, err := book.SheetUnbound(0, DefaultSheetOptions)
		if err != nil {
			t.Fatal(err)
		}
		_, err = sheet.Suggest(SuggestOptions{Format: &format})
		holds(t, "suggest", row.styles, row.refused, err)
		err = sheet.Bind(plan)
		holds(t, "bind", row.styles, row.refused, err)
		if row.refused {
			if _, err := sheet.Read(); !errors.Is(err, ErrUnbound) {
				t.Errorf("sheet, %d: a read after the refusal: %v", row.styles, err)
			}
		}
	}
}

// A suggest judges every format it's given before it opens anything: one never met, one replaced, one
// past the sample, an empty source and a broken one. A word without the retired bit is the control,
// which gets as far as the source does.
func TestAStaleFormatIsRefusedBeforeASuggestOpensAnything(t *testing.T) {
	headerless := CSV
	headerless.HasHeader = false
	for _, row := range stalePolicies {
		format := worded(row.styles)
		_, refusedWith := format.Raw()
		if row.refused != (refusedWith != nil) {
			t.Fatalf("%d: %v", row.styles, refusedWith)
		}
		// refusal fails unless err is the format's and not the source's: it holds HyperCast's word.
		refusal := func(what string, err error) {
			t.Helper()
			var failure *Failure
			if err == nil || errors.As(err, &failure) || !strings.Contains(err.Error(), refusedWith.Error()) {
				t.Errorf("%s, %d: %v", what, row.styles, err)
			}
		}
		every := SuggestOptions{Format: &format}
		past := SuggestOptions{ColumnFormats: map[int]hypercast.NumFormat{99: format}}
		replaced := SuggestOptions{Format: &format, ColumnFormats: map[int]hypercast.NumFormat{0: indian}}
		for _, ask := range []struct {
			what    string
			text    string
			dialect Dialect
			options SuggestOptions
		}{
			{"a sample", "a\n1\n", CSV, every},
			{"empty", "", CSV, every},
			{"empty, headerless", "", headerless, every},
			{"past the sample", "a\n1\n", CSV, past},
			{"replaced for a column", "a\n1\n", CSV, replaced},
		} {
			_, err := SuggestDelimitedBytes([]byte(ask.text), ask.dialect, ask.options)
			if row.refused {
				refusal(ask.what, err)
			} else if err != nil {
				t.Errorf("%s, %d: %v", ask.what, row.styles, err)
			}
		}

		// A broken source says so only of a format that's sound.
		for _, broken := range []struct {
			text    string
			dialect Dialect
		}{{"a\n\"1\n", CSV}, {"\"1\n", headerless}} {
			_, err := SuggestDelimitedBytes([]byte(broken.text), broken.dialect, every)
			var failure *Failure
			if row.refused {
				refusal("a broken source", err)
			} else if !errors.As(err, &failure) {
				t.Errorf("a broken source, %d: %v", row.styles, err)
			}
		}
		_, err := SuggestDelimited(filepath.Join(os.TempDir(), "hypertabular-no-such-file.csv"), CSV, every)
		if row.refused {
			refusal("no such file", err)
		} else if err == nil {
			t.Errorf("no such file, %d: read", row.styles)
		}

		// A sheet: the same, and one whose sample would meet its broken part.
		book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "grouping.xlsx"))
		if err != nil {
			t.Fatal(err)
		}
		sheet, err := book.SheetUnbound(0, DefaultSheetOptions)
		if err != nil {
			t.Fatal(err)
		}
		for what, options := range map[string]SuggestOptions{"every": every, "past": past, "replaced": replaced} {
			_, err := sheet.Suggest(options)
			if row.refused {
				refusal("a sheet, "+what, err)
			} else if err != nil {
				t.Errorf("a sheet, %s, %d: %v", what, row.styles, err)
			}
		}
		book, err = OpenWorkbook(repositoryFile(t, "corpus", "workbook", "broken.xlsx"))
		if err != nil {
			t.Fatal(err)
		}
		sheet, err = book.SheetUnbound(0, DefaultSheetOptions)
		if err != nil {
			t.Fatal(err)
		}
		_, err = sheet.Suggest(every)
		var failure *Failure
		if row.refused {
			refusal("a broken sheet", err)
		} else if !errors.As(err, &failure) {
			t.Errorf("a broken sheet, %d: %v", row.styles, err)
		}
	}
}

// What the delimiter rule says stays: nothing says 1,23,456 is Indian. Declared, it's a number.
func TestAnIndianShapeHasNoCultureUntilOneIsDeclared(t *testing.T) {
	ask := func(text string, options SuggestOptions) *SuggestedColumn {
		t.Helper()
		found, err := SuggestDelimitedBytes([]byte(text), CSV, options)
		if err != nil {
			t.Fatal(err)
		}
		return &found.Report[0]
	}
	if got := ask("a\n\"1,23,456\"\n", SuggestOptions{}); got.Door != DoorText || got.Status != SuggestResolved {
		t.Errorf("alone: %+v", got)
	}
	if got := ask("a\n\"1,23,456\"\n5\n", SuggestOptions{}); got.Door != DoorText || got.Status != SuggestUnresolved {
		t.Errorf("beside 5: %+v", got)
	}
	got := ask("a\n\"1,234,567\"\n", SuggestOptions{})
	if raw, err := got.Format.Raw(); err != nil || raw.Flags != 95 {
		t.Errorf("a suggested plan's flags: %+v, %v", raw, err)
	}

	declared := SuggestOptions{ColumnFormats: map[int]hypercast.NumFormat{0: indian}}
	got = ask("a\n\"1,23,456\"\n", declared)
	if got.Door != DoorI32 || got.Status != SuggestResolved || got.Format == nil || *got.Format != indian {
		t.Errorf("Indian declared: %+v", got)
	}
	if got := ask("a\n\"1,234,567\"\n", declared); got.Door != DoorText || got.Format != nil {
		t.Errorf("a Western shape, Indian declared: %+v", got)
	}
}
