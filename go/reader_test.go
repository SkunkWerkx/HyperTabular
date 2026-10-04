package hypertabular

// Binding-level behavior the corpus can't express: the plan as a projection, text handed out
// without copying, the raw text of a cell, the errors a caller's own mistakes get, how a
// broken input and a failing source end a read, and the paths the reader takes only for
// unusual shapes — a plan wider than the shim's stack table, a header wider than the first
// name table, an escaped cell larger than the arena, a record larger than the row ceiling.

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"
	"testing/iotest"
	"time"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// rowCeiling lowers the row ceiling, which is otherwise MaxRowBytes — a gibibyte, and not
// something a test can reach.
func rowCeiling(bytes int) Option {
	return func(o *options) { o.maxRowBytes = bytes }
}

func open(t *testing.T, input string, dialect Dialect, plan []Column, opts ...Option) *DelimitedReader {
	t.Helper()
	r, err := NewDelimitedReaderBytes([]byte(input), dialect, plan, opts...)
	if err != nil {
		t.Fatal(err)
	}
	return r
}

func read(t *testing.T, r *DelimitedReader, rows int) {
	t.Helper()
	if n, err := r.Read(); err != nil || n != rows {
		t.Fatalf("Read returned %d, %v; want %d rows", n, err, rows)
	}
}

func end(t *testing.T, r *DelimitedReader) {
	t.Helper()
	if n, err := r.Read(); err != io.EOF || n != 0 {
		t.Fatalf("Read returned %d, %v; want the end of the input", n, err)
	}
}

// The version the linked core reports is the crate's own: the archive on the link line is
// the one this checkout builds.
func TestNativeVersionIsTheCratesVersion(t *testing.T) {
	manifest, err := os.ReadFile(repositoryFile(t, "rust", "Cargo.toml"))
	if err != nil {
		t.Fatal(err)
	}
	match := regexp.MustCompile(`(?m)^version = "([^"]+)"`).FindSubmatch(manifest)
	if match == nil {
		t.Fatal("no version in rust/Cargo.toml")
	}
	if got := NativeVersion(); got != string(match[1]) {
		t.Errorf("NativeVersion() = %q; rust/Cargo.toml says %q", got, match[1])
	}
}

func TestAPlanIsAProjection(t *testing.T) {
	// Five source columns read into four, out of order, one of them twice, and one that no
	// record has.
	r := open(t, "id,name,score,when,flag\n7,alice,2.5,2026-01-07,yes\n8,bob,x,2026-02-30,no\n", CSV, []Column{
		DateOnly(3),
		Text(0),
		I32(0, hypercast.Invariant),
		F64(2, hypercast.Invariant),
		Text(9),
	})
	if header := r.Header(); len(header) != 5 || header[4] != "flag" {
		t.Fatalf("header %q", header)
	}
	read(t, r, 2)
	if dates := r.DateOnly(0); dates[0] != (hypercast.Date{Year: 2026, Month: time.January, Day: 7}) || dates[1] != (hypercast.Date{}) {
		t.Errorf("dates %v", dates)
	}
	if fault := r.Fault(0, 1); fault == nil || fault.Reason != hypercast.OutOfRange {
		t.Errorf("2026-02-30: %v", fault)
	}
	if text, ids := r.Text(1), r.I32(2); string(text[0]) != "7" || string(text[1]) != "8" || ids[0] != 7 || ids[1] != 8 {
		t.Errorf("one source column through two doors: %q, %v", text, ids)
	}
	if scores := r.F64(3); scores[0] != 2.5 || scores[1] != 0 || r.Verdicts(3)[1].Reason() != hypercast.Malformed {
		t.Errorf("scores %v, %v", scores, r.Verdicts(3))
	}
	// A source column past the record's last cell reads as empty.
	if missing := r.Text(4); missing[0] != nil || missing[1] != nil || r.Verdicts(4)[0].Reason() != hypercast.Empty {
		t.Errorf("a column no record has: %q, %v", missing, r.Verdicts(4))
	}
	if string(r.Raw(4, 0)) != "" {
		t.Errorf("a column no record has: raw %q", r.Raw(4, 0))
	}
	if r.Records() != 3 {
		t.Errorf("Records() = %d, want 3", r.Records())
	}
	end(t, r)
}

// Every door, on the same text HyperCast's own Go door is asked about directly: the value
// and the fault have to be the ones it gives. Both cores are in this binary — this
// module's archive and the one the hypercast package links — and this is them agreeing.
func TestEveryDoorSaysWhatHyperCastSays(t *testing.T) {
	euro := hypercast.NumFormat{DecimalSep: ',', GroupSep: '.', Styles: hypercast.AllStyles, Currency: "€"}
	type door struct {
		column Column
		direct func(text string) (any, *hypercast.Fault)
		read   func(r *DelimitedReader) any
		texts  []string
	}
	doors := []door{
		{Bool(0), func(s string) (any, *hypercast.Fault) { v, f := hypercast.Bool(s); return v, f },
			func(r *DelimitedReader) any { return r.Bool(0)[0] }, []string{"yes", "OFF", "maybe"}},
		{I8(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.I8(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.I8(0)[0] }, []string{"-128", "0xFF", "128"}},
		{I16(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.I16(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.I16(0)[0] }, []string{"(1234)", "40000"}},
		{I32(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.I32(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.I32(0)[0] }, []string{"1e3", "  12x4"}},
		{I64(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.I64(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.I64(0)[0] }, []string{"-9223372036854775808", "9223372036854775808"}},
		{U8(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.U8(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.U8(0)[0] }, []string{"255", "256", "-1"}},
		{U16(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.U16(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.U16(0)[0] }, []string{"0xFFFF", "65536"}},
		{U32(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.U32(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.U32(0)[0] }, []string{"4294967295", "4294967296"}},
		{U64(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.U64(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.U64(0)[0] }, []string{"18446744073709551615", "1.5"}},
		{F32(0, hypercast.Invariant), func(s string) (any, *hypercast.Fault) { v, f := hypercast.F32(s, hypercast.Invariant); return v, f },
			func(r *DelimitedReader) any { return r.F32(0)[0] }, []string{"1.5e3", "1e40", "NaN"}},
		{F64(0, euro), func(s string) (any, *hypercast.Fault) { v, f := hypercast.F64(s, euro); return v, f },
			func(r *DelimitedReader) any { return r.F64(0)[0] }, []string{"€ 1.234,50", "50%", "1,2,3"}},
		{Exact(0, euro), func(s string) (any, *hypercast.Fault) { v, f := hypercast.Exact(s, euro); return v, f },
			func(r *DelimitedReader) any { return r.Exact(0)[0] }, []string{"(1.234,50 €)", "-0,025", "1e40"}},
		{Uuid(0), func(s string) (any, *hypercast.Fault) { v, f := hypercast.Uuid(s); return v, f },
			func(r *DelimitedReader) any { return r.Uuid(0)[0] }, []string{"urn:uuid:01020304-0506-0708-090a-0b0c0d0e0f10", "0102"}},
		{Timestamp(0), func(s string) (any, *hypercast.Fault) { v, f := hypercast.Timestamp(s); return v, f },
			func(r *DelimitedReader) any { return r.Timestamp(0)[0] }, []string{"2026-01-02T15:04:05.123456789+05:00", "2026-01-02 15:04:05"}},
		{Unix(0, hypercast.Milliseconds), func(s string) (any, *hypercast.Fault) { v, f := hypercast.Unix(s, hypercast.Milliseconds); return v, f },
			func(r *DelimitedReader) any { return r.Timestamp(0)[0] }, []string{"1700000000123", "-1", "soon"}},
		{ExcelSerial(0, hypercast.Excel1900), func(s string) (any, *hypercast.Fault) {
			v, f := hypercast.ExcelSerial(s, hypercast.Excel1900)
			return v, f
		},
			func(r *DelimitedReader) any { return r.Timestamp(0)[0] }, []string{"45292.75", "60"}},
		{DateOnly(0), func(s string) (any, *hypercast.Fault) { v, f := hypercast.DateOnly(s); return v, f },
			func(r *DelimitedReader) any { return r.DateOnly(0)[0] }, []string{"2026-01-07", "1/7/2026"}},
		{DateOnlyOrdered(0, hypercast.DayMonthYear), func(s string) (any, *hypercast.Fault) {
			v, f := hypercast.DateOnlyOrdered(s, hypercast.DayMonthYear)
			return v, f
		}, func(r *DelimitedReader) any { return r.DateOnly(0)[0] }, []string{"1/7/2026", "31.12.1999", "13/13/2026"}},
		{DateTime(0, hypercast.MonthDayYear), func(s string) (any, *hypercast.Fault) {
			v, f := hypercast.DateTime(s, hypercast.MonthDayYear)
			return v, f
		}, func(r *DelimitedReader) any { return r.DateTime(0)[0] }, []string{"1/7/2026 3:04 PM", "1/7/2026 25:00"}},
		{TimeOfDay(0), func(s string) (any, *hypercast.Fault) { v, f := hypercast.TimeOfDay(s); return v, f },
			func(r *DelimitedReader) any { return r.TimeOfDay(0)[0] }, []string{"15:04:05.123456789", "24:00:01"}},
		{Span(0), func(s string) (any, *hypercast.Fault) { v, f := hypercast.Span(s); return v, f },
			func(r *DelimitedReader) any { return r.Span(0)[0] }, []string{"PT1H30M15.5S", "-1.5s", "a while"}},
	}
	if len(doors) != 21 {
		t.Fatalf("%d doors here; every door but Text has a HyperCast door to agree with, and that is 21", len(doors))
	}
	dialect := Dialect{Separator: '\t'}
	for _, door := range doors {
		for _, text := range door.texts {
			r := open(t, text, dialect, []Column{door.column})
			read(t, r, 1)
			value, fault := door.direct(text)
			if got := door.read(r); got != value {
				t.Errorf("%v %q: %v, HyperCast says %v", door.column.Door(), text, got, value)
			}
			switch got := r.Fault(0, 0); {
			case fault == nil && got != nil, fault != nil && (got == nil || *got != *fault):
				t.Errorf("%v %q: fault %v, HyperCast says %v", door.column.Door(), text, got, fault)
			}
			if raw := string(r.Raw(0, 0)); raw != text {
				t.Errorf("%v %q: raw text %q", door.column.Door(), text, raw)
			}
		}
	}
}

// within reports whether part is a slice of whole's own memory.
func within(part, whole []byte) bool {
	if len(part) == 0 || len(whole) == 0 {
		return false
	}
	first, base := uintptr(unsafe.Pointer(unsafe.SliceData(part))), uintptr(unsafe.Pointer(unsafe.SliceData(whole)))
	return first >= base && first+uintptr(len(part)) <= base+uintptr(len(whole))
}

func TestTextIsNotCopied(t *testing.T) {
	input := []byte("plain,\"quoted, with a comma\",\"an \"\"escaped\"\" one\",\n")
	r, err := NewDelimitedReaderBytes(input, Dialect{Separator: ',', Quoting: true}, []Column{Text(0), Text(1), Text(2), Text(3)})
	if err != nil {
		t.Fatal(err)
	}
	read(t, r, 1)
	plain, quoted, escaped, empty := r.Text(0)[0], r.Text(1)[0], r.Text(2)[0], r.Text(3)[0]
	if string(plain) != "plain" || !within(plain, input) {
		t.Errorf("a plain cell: %q, a slice of the input: %v", plain, within(plain, input))
	}
	if string(quoted) != "quoted, with a comma" || !within(quoted, input) {
		t.Errorf("a quoted cell: %q, a slice of the input: %v", quoted, within(quoted, input))
	}
	// Only a cell with "" inside is anywhere but in the input: unescaped, in the arena.
	if string(escaped) != `an "escaped" one` || within(escaped, input) {
		t.Errorf("an escaped cell: %q, a slice of the input: %v", escaped, within(escaped, input))
	}
	if empty != nil || r.Verdicts(3)[0].Reason() != hypercast.Empty {
		t.Errorf("an empty cell: %q, %v", empty, r.Verdicts(3)[0])
	}
	// A cell's capacity stops at its own end: appending to one cannot write into the input.
	if cap(plain) != len(plain) {
		t.Errorf("a cell's capacity is %d, its length %d", cap(plain), len(plain))
	}
	// The same slices again for the same batch, not a second conversion.
	if again := r.Text(0); unsafe.SliceData(again) != unsafe.SliceData(r.Text(0)) || unsafe.SliceData(again[0]) != unsafe.SliceData(plain) {
		t.Error("a second call for the same batch handed out different slices")
	}
}

func TestRawIsTheTextACellWasCastFrom(t *testing.T) {
	input := []byte("n\n12x4\n\"1,\"\"2\"\"\"\n  7 \n")
	r, err := NewDelimitedReaderBytes(input, CSV, []Column{I32(0, hypercast.Invariant)})
	if err != nil {
		t.Fatal(err)
	}
	read(t, r, 3)
	// A cell that did not cast: its text, in place, and the span the fault indexes it by.
	raw, fault := r.Raw(0, 0), r.Fault(0, 0)
	if string(raw) != "12x4" || !within(raw, input) || fault == nil || string(raw[fault.Offset:fault.Offset+fault.Length]) != "x" {
		t.Errorf("raw %q, fault %v", raw, fault)
	}
	// A cell with an escaped quote in it: unescaped, as the core cast it.
	if raw := r.Raw(0, 1); string(raw) != `1,"2"` || within(raw, input) {
		t.Errorf("raw %q", raw)
	}
	// A cell that cast: untrimmed.
	if raw := r.Raw(0, 2); string(raw) != "  7 " || r.I32(0)[2] != 7 {
		t.Errorf("raw %q, value %d", raw, r.I32(0)[2])
	}
	for _, row := range []int{-1, 3} {
		func() {
			defer func() {
				if recover() == nil {
					t.Errorf("Raw(0, %d) did not panic", row)
				}
			}()
			r.Raw(0, row)
		}()
	}
}

func TestAskingAColumnForAnotherDoorsTypePanics(t *testing.T) {
	r := open(t, "1\n", Dialect{Separator: ','}, []Column{I32(0, hypercast.Invariant), Unix(0, hypercast.Seconds)})
	read(t, r, 1)
	for name, call := range map[string]func(){
		"I64 on an I32 column":      func() { r.I64(0) },
		"Text on an I32 column":     func() { r.Text(0) },
		"DateOnly on a Unix column": func() { r.DateOnly(1) },
		"a column the plan lacks":   func() { r.I32(2) },
	} {
		func() {
			defer func() {
				if recover() == nil {
					t.Errorf("%s did not panic", name)
				}
			}()
			call()
		}()
	}
	// The three instant doors share an accessor.
	if instants := r.Timestamp(1); !instants[0].Equal(time.Unix(1, 0)) {
		t.Errorf("Timestamp on a Unix column: %v", instants)
	}
}

func TestACallersMistakeIsTheConstructorsError(t *testing.T) {
	for name, build := range map[string]func() (*DelimitedReader, error){
		"the zero Column": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{{}})
		},
		"a negative ordinal": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{Text(-1)})
		},
		"the zero NumFormat": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{I32(0, hypercast.NumFormat{})})
		},
		"equal separators": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{F64(0, hypercast.NumFormat{DecimalSep: '.', GroupSep: '.'})})
		},
		"a separator that is no scalar value": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{F64(0, hypercast.NumFormat{DecimalSep: '.', GroupSep: 0xD800})})
		},
		"a currency symbol with a digit in it": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{Exact(0, hypercast.NumFormat{DecimalSep: '.', GroupSep: ',', Currency: "A1"})})
		},
		"a currency symbol too long": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{Exact(0, hypercast.NumFormat{DecimalSep: '.', GroupSep: ',', Currency: strings.Repeat("€", 6)})})
		},
		"an undefined precision": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{Unix(0, 9)})
		},
		"an undefined order": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{DateTime(0, 0)})
		},
		"an undefined epoch": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, []Column{ExcelSerial(0, 3)})
		},
		"the zero Dialect": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, Dialect{}, nil)
		},
		"a quote for a separator": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, Dialect{Separator: '"'}, nil)
		},
		"no rows a batch": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(nil, CSV, nil, BatchRows(0))
		},
		"no buffer": func() (*DelimitedReader, error) {
			return NewDelimitedReader(strings.NewReader(""), CSV, nil, BufferBytes(-1))
		},
		"no source": func() (*DelimitedReader, error) {
			return NewDelimitedReader(nil, CSV, nil)
		},
	} {
		if r, err := build(); err == nil || r != nil {
			t.Errorf("%s: no error", name)
		} else if !strings.HasPrefix(err.Error(), "hypertabular: ") {
			t.Errorf("%s: %v", name, err)
		}
	}
}

func TestAStructuralFailureComesAfterTheIntactRows(t *testing.T) {
	r, err := NewDelimitedReader(strings.NewReader("a,b\n1,2\n3,4\n5\n6,7\n"), CSV, []Column{I32(0, hypercast.Invariant)}, BatchRows(16))
	if err != nil {
		t.Fatal(err)
	}
	read(t, r, 2)
	if ids := r.I32(0); ids[0] != 1 || ids[1] != 3 {
		t.Errorf("the rows before the break: %v", ids)
	}
	_, err = r.Read()
	var failure *Failure
	if !errors.As(err, &failure) {
		t.Fatalf("Read returned %v, want a *Failure", err)
	}
	if *failure != (Failure{Kind: ColumnCount, Record: 3, Line: 4, Byte: 12, Expected: 2, Found: 1}) {
		t.Errorf("failure %+v", *failure)
	}
	if got, want := failure.Error(), "hypertabular: record 3 (line 4, byte 12) has 1 cells; the first record had 2"; got != want {
		t.Errorf("Error() = %q, want %q", got, want)
	}
	if r.Rows() != 0 || len(r.I32(0)) != 0 {
		t.Errorf("%d rows in hand beside the failure", r.Rows())
	}
	// Final: the same failure, and never the rows behind it.
	if _, again := r.Read(); again != err {
		t.Errorf("a second Read returned %v", again)
	}

	_, err = NewDelimitedReaderBytes([]byte("a,\"b\nc"), CSV, nil)
	if !errors.As(err, &failure) || failure.Kind != UnclosedQuote || failure.Record != 0 || failure.Line != 1 || failure.Byte != 0 {
		t.Errorf("a header that never closes its quote: %v", err)
	} else if !strings.Contains(failure.Error(), "ended inside a quoted cell") {
		t.Errorf("Error() = %q", failure.Error())
	}
}

func TestARecordPastTheRowCeilingIsAFailure(t *testing.T) {
	const input = "1,2\n3,4\n" + "5,66666666666666666666666666666666666666\n" + "7,8\n"
	want := Failure{Kind: RowTooLong, Record: 2, Line: 3, Byte: 8}
	for name, build := range map[string]func() (*DelimitedReader, error){
		"memory": func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes([]byte(input), Dialect{Separator: ','}, []Column{I32(0, hypercast.Invariant)}, rowCeiling(16))
		},
		"stream": func() (*DelimitedReader, error) {
			return NewDelimitedReader(strings.NewReader(input), Dialect{Separator: ','}, []Column{I32(0, hypercast.Invariant)}, rowCeiling(16), BufferBytes(4))
		},
	} {
		r, err := build()
		if err != nil {
			t.Fatal(err)
		}
		seen := 0
		for {
			rows, err := r.Read()
			if err != nil {
				var failure *Failure
				if !errors.As(err, &failure) || *failure != want {
					t.Errorf("%s: ended with %v, want %+v", name, err, want)
				} else if !strings.Contains(failure.Error(), "row ceiling") {
					t.Errorf("%s: Error() = %q", name, failure.Error())
				}
				if _, again := r.Read(); again != err {
					t.Errorf("%s: a second Read returned %v", name, again)
				}
				break
			}
			seen += rows
		}
		if seen != 2 {
			t.Errorf("%s: %d rows before the failure, want 2", name, seen)
		}
	}
}

// failing reads what it was given, then fails.
type failing struct {
	text string
	err  error
}

func (f *failing) Read(p []byte) (int, error) {
	if f.text == "" {
		return 0, f.err
	}
	n := copy(p, f.text)
	f.text = f.text[n:]
	return n, nil
}

func TestASourcesErrorEndsTheRead(t *testing.T) {
	broken := errors.New("the wire went quiet")
	r, err := NewDelimitedReader(&failing{"a\n1\n2\n3", broken}, CSV, []Column{I32(0, hypercast.Invariant)})
	if err != nil {
		t.Fatal(err)
	}
	// The whole rows that arrived are delivered; the unfinished one never is.
	read(t, r, 2)
	if _, err := r.Read(); !errors.Is(err, broken) {
		t.Fatalf("Read returned %v", err)
	}
	if _, err := r.Read(); !errors.Is(err, broken) {
		t.Errorf("a second Read returned %v", err)
	}

	// Before the header is complete, it is the constructor's error.
	if _, err := NewDelimitedReader(&failing{"a,b", broken}, CSV, nil); !errors.Is(err, broken) {
		t.Errorf("the constructor returned %v", err)
	}
	if _, err := NewDelimitedReader(iotest.ErrReader(broken), CSV, nil); !errors.Is(err, broken) {
		t.Errorf("the constructor returned %v", err)
	}

	// A source that never makes progress is stopped, not waited on for ever.
	if _, err := NewDelimitedReader(&failing{"", nil}, CSV, nil); !errors.Is(err, io.ErrNoProgress) {
		t.Errorf("the constructor returned %v", err)
	}
}

func TestOpenDelimitedOwnsItsFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "orders.tsv")
	if err := os.WriteFile(path, []byte("id\tname\n1\talice\n2\tbob\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	r, err := OpenDelimited(path, TSV, []Column{I32(0, hypercast.Invariant), Text(1)})
	if err != nil {
		t.Fatal(err)
	}
	read(t, r, 2)
	names := r.Text(1)
	if err := r.Close(); err != nil {
		t.Fatal(err)
	}
	// Closed: no batch in hand, no more reads, and closing again is nothing.
	if r.Rows() != 0 || len(r.I32(0)) != 0 {
		t.Errorf("%d rows in hand after Close", r.Rows())
	}
	if _, err := r.Read(); !errors.Is(err, os.ErrClosed) {
		t.Errorf("Read after Close returned %v", err)
	}
	if err := r.Close(); err != nil {
		t.Errorf("a second Close returned %v", err)
	}
	// What was handed out before is still ordinary Go memory: stale, never dangling.
	runtime.GC()
	if string(names[0]) != "alice" || string(names[1]) != "bob" {
		t.Errorf("the slices kept past Close: %q", names)
	}

	if _, err := OpenDelimited(filepath.Join(t.TempDir(), "missing.csv"), CSV, nil); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("a missing file: %v", err)
	}
	// A plan the constructor refuses does not leave the file open behind it (on Windows, an
	// open file could not be removed).
	if _, err := OpenDelimited(path, CSV, []Column{{}}); err == nil {
		t.Error("the zero Column: no error")
	}
	if err := os.Remove(path); err != nil {
		t.Errorf("removing the file after a refused plan: %v", err)
	}
}

// A reader over an io.Reader does not close it — the caller opened it.
type closeCounter struct {
	io.Reader
	closed int
}

func (c *closeCounter) Close() error { c.closed++; return nil }

func TestAReaderDoesNotCloseASourceItWasHanded(t *testing.T) {
	source := &closeCounter{Reader: strings.NewReader("a\n1\n")}
	r, err := NewDelimitedReader(source, CSV, []Column{Text(0)})
	if err != nil {
		t.Fatal(err)
	}
	read(t, r, 1)
	if err := r.Close(); err != nil || source.closed != 0 {
		t.Errorf("Close returned %v and closed the source %d times", err, source.closed)
	}
}

func TestAnEmptyPlanCountsRows(t *testing.T) {
	r := open(t, "a,b\n1,2\n\n3,4\n5,6\n", CSV, nil, BatchRows(2))
	if r.Columns() != 0 {
		t.Fatalf("%d columns", r.Columns())
	}
	read(t, r, 2)
	read(t, r, 1)
	end(t, r)
	// The header, three rows and a skipped blank line.
	if r.Records() != 5 {
		t.Errorf("Records() = %d, want 5", r.Records())
	}
}

// A plan wider than the fill shim's stack table takes its other path: the column table on
// the C heap for the length of the call.
func TestAPlanWiderThanTheShimsStackTable(t *testing.T) {
	const width, rows = 150, 40
	var input strings.Builder
	for row := 0; row < rows; row++ {
		for column := 0; column < width; column++ {
			if column > 0 {
				input.WriteByte(',')
			}
			fmt.Fprintf(&input, "%d", row*1000+column)
		}
		input.WriteByte('\n')
	}
	plan := make([]Column, 0, 2*width)
	for column := 0; column < width; column++ {
		plan = append(plan, I64(column, hypercast.Invariant), Text(column))
	}
	r, err := NewDelimitedReader(strings.NewReader(input.String()), Dialect{Separator: ','}, plan, BatchRows(16), BufferBytes(512))
	if err != nil {
		t.Fatal(err)
	}
	seen := 0
	for {
		n, err := r.Read()
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatal(err)
		}
		for column := 0; column < width; column++ {
			values, texts := r.I64(2*column), r.Text(2*column+1)
			for row := 0; row < n; row++ {
				want := int64((seen+row)*1000 + column)
				if values[row] != want || string(texts[row]) != fmt.Sprint(want) || !r.Verdicts(2 * column)[row].OK() {
					t.Fatalf("row %d, column %d: %d and %q, want %d", seen+row, column, values[row], texts[row], want)
				}
			}
		}
		seen += n
		runtime.GC()
	}
	if seen != rows {
		t.Errorf("%d rows, want %d", seen, rows)
	}
}

func TestAHeaderWiderThanTheFirstNameTable(t *testing.T) {
	names := make([]string, 200)
	for index := range names {
		names[index] = fmt.Sprintf("column %d", index)
	}
	// One name with an escaped quote in it, longer than the arena starts out.
	long := strings.Repeat("long ", 2000)
	names[100] = long + `"quoted"`
	quoted := make([]string, len(names))
	copy(quoted, names)
	quoted[100] = `"` + long + `""quoted"""`
	input := strings.Join(quoted, ",") + "\n" + strings.Repeat(",", len(names)-1) + "\n"
	for _, bufferBytes := range []int{1, 100, DefaultBufferBytes} {
		r, err := NewDelimitedReader(strings.NewReader(input), CSV, []Column{Text(199)}, BufferBytes(bufferBytes))
		if err != nil {
			t.Fatal(err)
		}
		header := r.Header()
		if len(header) != len(names) {
			t.Fatalf("%d names, want %d", len(header), len(names))
		}
		for index := range names {
			if header[index] != names[index] {
				t.Fatalf("name %d: %q, want %q", index, header[index], names[index])
			}
		}
		read(t, r, 1)
		end(t, r)
	}
}

func TestAnEscapedCellLargerThanTheArena(t *testing.T) {
	body := strings.Repeat(`He said ""no"". `, 2000) // 32,000 bytes escaped, 28,000 unescaped
	want := strings.ReplaceAll(body, `""`, `"`)
	input := "small,\"" + body + "\"\n\"a\"\"b\",\"" + body + body + "\"\n"
	for _, batchRows := range []int{1, 16} {
		r := open(t, input, Dialect{Separator: ',', Quoting: true}, []Column{Text(0), Text(1)}, BatchRows(batchRows))
		var first, second [][]string
		for {
			n, err := r.Read()
			if err == io.EOF {
				break
			}
			if err != nil {
				t.Fatal(err)
			}
			for row := 0; row < n; row++ {
				first = append(first, []string{string(r.Text(0)[row]), string(r.Raw(0, row))})
				second = append(second, []string{string(r.Text(1)[row]), string(r.Raw(1, row))})
			}
		}
		if len(first) != 2 || first[0][0] != "small" || first[1][0] != `a"b` || first[1][1] != `a"b` {
			t.Errorf("%d rows a batch: the first column: %q", batchRows, first)
		}
		if len(second) != 2 || second[0][0] != want || second[0][1] != want || second[1][0] != want+want || second[1][1] != want+want {
			t.Errorf("%d rows a batch: the escaped cells did not come back whole", batchRows)
		}
	}
}

// The reader's buffers are Go memory the core writes through pointers the collector does
// not see being used. Reading with the collector running between and during batches — and
// with garbage being made beside it — is what would show a buffer moved or freed.
func TestReadingWhileTheCollectorRuns(t *testing.T) {
	const rows = 20000
	var input bytes.Buffer
	input.WriteString("id,name,when,amount\n")
	for row := 0; row < rows; row++ {
		fmt.Fprintf(&input, "%d,\"name \"\"%d\"\"\",2026-01-02T15:04:05.%09dZ,%d.25\n", row, row, row, row)
	}
	stop := make(chan struct{})
	done := make(chan struct{})
	go func() {
		defer close(done)
		var garbage [][]byte
		for {
			select {
			case <-stop:
				return
			default:
				garbage = append(garbage[:0], make([]byte, 1<<16))
				runtime.GC()
			}
		}
	}()
	plan := []Column{I32(0, hypercast.Invariant), Text(1), Timestamp(2), Exact(3, hypercast.Invariant)}
	for _, build := range []func() (*DelimitedReader, error){
		func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(input.Bytes(), CSV, plan, BatchRows(64))
		},
		func() (*DelimitedReader, error) {
			return NewDelimitedReader(bytes.NewReader(input.Bytes()), CSV, plan, BatchRows(64), BufferBytes(1024))
		},
	} {
		r, err := build()
		if err != nil {
			t.Fatal(err)
		}
		seen := 0
		for {
			n, err := r.Read()
			if err == io.EOF {
				break
			}
			if err != nil {
				t.Fatal(err)
			}
			ids, names, instants, amounts := r.I32(0), r.Text(1), r.Timestamp(2), r.Exact(3)
			for row := 0; row < n; row++ {
				id := seen + row
				if int(ids[row]) != id || string(names[row]) != fmt.Sprintf(`name "%d"`, id) ||
					instants[row].Nanosecond() != id || amounts[row].String() != fmt.Sprintf("%d.25", id) {
					t.Fatalf("row %d: %d, %q, %v, %v", id, ids[row], names[row], instants[row], amounts[row])
				}
			}
			seen += n
		}
		if seen != rows {
			t.Errorf("%d rows, want %d", seen, rows)
		}
	}
	close(stop)
	<-done
}

func TestTheNamesOfThings(t *testing.T) {
	if got := fmt.Sprint(DoorI32, DoorDateOnlyOrdered, DoorText, Door(0), Door(23)); got != "I32 DateOnlyOrdered Text Door(0) Door(23)" {
		t.Errorf("doors: %s", got)
	}
	if got := fmt.Sprint(UnclosedQuote, ColumnCount, RowTooLong, FailureKind(0)); got != "unclosed quote column count row too long FailureKind(0)" {
		t.Errorf("failure kinds: %s", got)
	}
	column := F64(3, hypercast.Detect)
	if column.Ordinal() != 3 || column.Door() != DoorF64 || column.Format() != hypercast.Detect {
		t.Errorf("column %+v", column)
	}
	if CSV.Separator != ',' || TSV.Separator != '\t' || PSV.Separator != '|' || !CSV.Quoting || !CSV.HasHeader || !CSV.SkipBlankLines {
		t.Errorf("dialects %+v %+v %+v", CSV, TSV, PSV)
	}
}
