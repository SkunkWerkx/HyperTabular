package hypertabular

// Replays the shared conformance corpus (corpus/delimited.json at the repository root) — the
// same file the Rust and C# bindings replay — through this binding: from memory, from an
// io.Reader read through buffers too small for a record, from a source that hands over one
// byte at a time, and from a file, in batches of one row, of two and of many. How the input
// is cut up is the binding's business and must not change the answer.

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"math/big"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"testing"
	"testing/iotest"
	"time"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	"github.com/google/uuid"
)

type corpusCase struct {
	Name    string `json:"name"`
	Input   string `json:"input"`
	Dialect struct {
		Separator      string `json:"separator"`
		Quoting        bool   `json:"quoting"`
		HasHeader      bool   `json:"has_header"`
		SkipBlankLines bool   `json:"skip_blank_lines"`
	} `json:"dialect"`
	Plan []corpusColumn `json:"plan"`
	// null when the dialect declares no header, [] for an input with no record.
	Header  *[]string      `json:"header"`
	Rows    [][]corpusCell `json:"rows"`
	Failure *corpusFailure `json:"failure"`
}

type corpusColumn struct {
	Door      string `json:"door"`
	Ordinal   int    `json:"ordinal"`
	Precision uint32 `json:"precision"`
	Order     uint32 `json:"order"`
	Epoch     uint32 `json:"epoch"`
	Format    *struct {
		DecimalSep string `json:"decimal_sep"`
		GroupSep   string `json:"group_sep"`
		Flags      uint32 `json:"flags"`
		Currency   string `json:"currency"` // absent ⇒ none declared
	} `json:"format"`
}

// A cell is a verdict in HyperCast's corpus shape, with `text` for the text door, and — for
// a fault — its span and `raw`, the cell's own text.
type corpusCell struct {
	Expect     string          `json:"expect"`
	Value      json.RawMessage `json:"value"`
	Text       *string         `json:"text"`
	Fault      []int           `json:"fault"`
	Raw        *string         `json:"raw"`
	Seconds    int64           `json:"seconds"`
	Nanos      int64           `json:"nanos"`
	Year       int             `json:"year"`
	Month      int             `json:"month"`
	Day        int             `json:"day"`
	NanosOfDay uint64          `json:"nanos_of_day"`
	Magnitude  string          `json:"magnitude"`
	Scale      uint8           `json:"scale"`
	Negative   bool            `json:"negative"`
}

type corpusFailure struct {
	Kind     string `json:"kind"`
	Record   int64  `json:"record"`
	Line     int    `json:"line"`
	Byte     int64  `json:"byte"`
	Expected int    `json:"expected"`
	Found    int    `json:"found"`
}

// repositoryFile finds a file by its path from the repository root, walking up from the
// test's directory — the corpus and rust/Cargo.toml are the two things outside this
// directory the tests read.
func repositoryFile(t *testing.T, elem ...string) string {
	t.Helper()
	dir, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	for {
		candidate := filepath.Join(append([]string{dir}, elem...)...)
		if _, err := os.Stat(candidate); err == nil {
			return candidate
		}
		// The root is its own parent on every OS — "/" here, "C:\" on Windows.
		parent := filepath.Dir(dir)
		if parent == dir {
			t.Fatalf("%s not found above the test directory", filepath.Join(elem...))
		}
		dir = parent
	}
}

func delimitedCorpus(t *testing.T) []corpusCase {
	t.Helper()
	path := repositoryFile(t, "corpus", "delimited.json")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var cases []corpusCase
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatalf("parsing %s: %v", path, err)
	}
	return cases
}

func (c *corpusColumn) format() hypercast.NumFormat {
	if c.Format == nil {
		return hypercast.Invariant
	}
	return hypercast.NumFormat{
		DecimalSep: []rune(c.Format.DecimalSep)[0],
		GroupSep:   []rune(c.Format.GroupSep)[0],
		Styles:     hypercast.NumStyles(c.Format.Flags),
		Currency:   c.Format.Currency,
	}
}

// column is the plan entry as this binding declares it: the door named as HyperCast's
// corpus names its types, with what the door declares beside it.
func (c *corpusColumn) column(t *testing.T) Column {
	t.Helper()
	switch c.Door {
	case "bool":
		return Bool(c.Ordinal)
	case "i8":
		return I8(c.Ordinal, c.format())
	case "i16":
		return I16(c.Ordinal, c.format())
	case "i32":
		return I32(c.Ordinal, c.format())
	case "i64":
		return I64(c.Ordinal, c.format())
	case "u8":
		return U8(c.Ordinal, c.format())
	case "u16":
		return U16(c.Ordinal, c.format())
	case "u32":
		return U32(c.Ordinal, c.format())
	case "u64":
		return U64(c.Ordinal, c.format())
	case "f32":
		return F32(c.Ordinal, c.format())
	case "f64":
		return F64(c.Ordinal, c.format())
	case "decimal":
		return Exact(c.Ordinal, c.format())
	case "uuid":
		return Uuid(c.Ordinal)
	case "timestamp":
		return Timestamp(c.Ordinal)
	case "unix":
		return Unix(c.Ordinal, hypercast.UnixPrecision(c.Precision))
	case "excel_serial":
		return ExcelSerial(c.Ordinal, hypercast.ExcelEpoch(c.Epoch))
	case "date":
		return DateOnly(c.Ordinal)
	case "date_ordered":
		return DateOnlyOrdered(c.Ordinal, hypercast.DateOrder(c.Order))
	case "datetime":
		return DateTime(c.Ordinal, hypercast.DateOrder(c.Order))
	case "time":
		return TimeOfDay(c.Ordinal)
	case "duration":
		return Span(c.Ordinal)
	case "text":
		return Text(c.Ordinal)
	default:
		t.Fatalf("unknown door %q", c.Door)
		return Column{}
	}
}

var expectedReason = map[string]hypercast.CastFailure{
	"empty":        hypercast.Empty,
	"malformed":    hypercast.Malformed,
	"out_of_range": hypercast.OutOfRange,
}

// got is the cell at (column, row) of the batch in hand, read through the accessor its
// door has, boxed.
func got(b *Batch, column, row int) any {
	switch door := b.Columns()[column].Door(); door {
	case DoorBool:
		return b.Bool(column)[row]
	case DoorI8:
		return b.I8(column)[row]
	case DoorI16:
		return b.I16(column)[row]
	case DoorI32:
		return b.I32(column)[row]
	case DoorI64:
		return b.I64(column)[row]
	case DoorU8:
		return b.U8(column)[row]
	case DoorU16:
		return b.U16(column)[row]
	case DoorU32:
		return b.U32(column)[row]
	case DoorU64:
		return b.U64(column)[row]
	case DoorF32:
		return b.F32(column)[row]
	case DoorF64:
		return b.F64(column)[row]
	case DoorExact:
		return b.Exact(column)[row]
	case DoorUuid:
		return b.Uuid(column)[row]
	case DoorTimestamp, DoorUnix, DoorExcelSerial:
		return b.Timestamp(column)[row]
	case DoorDateOnly, DoorDateOnlyOrdered:
		return b.DateOnly(column)[row]
	case DoorDateTime:
		return b.DateTime(column)[row]
	case DoorTimeOfDay:
		return b.TimeOfDay(column)[row]
	case DoorSpan:
		return b.Span(column)[row]
	case DoorText:
		return b.Text(column)[row]
	default:
		panic(fmt.Sprintf("no accessor for %v", door))
	}
}

// viaGet is the cell at (column, row) read through Get, as the type its door presents.
func viaGet(b *Batch, column, row int) (any, *hypercast.Fault) {
	switch door := b.Columns()[column].Door(); door {
	case DoorBool:
		return boxed(Get[bool](b, column, row))
	case DoorI8:
		return boxed(Get[int8](b, column, row))
	case DoorI16:
		return boxed(Get[int16](b, column, row))
	case DoorI32:
		return boxed(Get[int32](b, column, row))
	case DoorI64:
		return boxed(Get[int64](b, column, row))
	case DoorU8:
		return boxed(Get[uint8](b, column, row))
	case DoorU16:
		return boxed(Get[uint16](b, column, row))
	case DoorU32:
		return boxed(Get[uint32](b, column, row))
	case DoorU64:
		return boxed(Get[uint64](b, column, row))
	case DoorF32:
		return boxed(Get[float32](b, column, row))
	case DoorF64:
		return boxed(Get[float64](b, column, row))
	case DoorExact:
		return boxed(Get[hypercast.Decimal](b, column, row))
	case DoorUuid:
		return boxed(Get[uuid.UUID](b, column, row))
	case DoorTimestamp, DoorUnix, DoorExcelSerial:
		return boxed(Get[time.Time](b, column, row))
	case DoorDateOnly, DoorDateOnlyOrdered:
		return boxed(Get[hypercast.Date](b, column, row))
	case DoorDateTime:
		return boxed(Get[hypercast.CivilDateTime](b, column, row))
	case DoorTimeOfDay:
		return boxed(Get[time.Duration](b, column, row))
	case DoorSpan:
		return boxed(Get[hypercast.Duration](b, column, row))
	case DoorText:
		return boxed(Get[[]byte](b, column, row))
	default:
		panic(fmt.Sprintf("no Get for %v", door))
	}
}

func boxed[T any](value T, fault *hypercast.Fault) (any, *hypercast.Fault) { return value, fault }

// want is what the corpus says the cell holds, as the Go value the door presents it as —
// the type HyperCast's own Go door returns. A cell that did not cast holds the zero value.
func want(t *testing.T, label string, door Door, cell *corpusCell) any {
	t.Helper()
	ok := cell.Expect == "ok"
	signed := func() int64 {
		if !ok {
			return 0
		}
		value, err := strconv.ParseInt(string(cell.Value), 10, 64)
		if err != nil {
			t.Fatalf("%s: bad expected value: %v", label, err)
		}
		return value
	}
	unsigned := func() uint64 {
		if !ok {
			return 0
		}
		value, err := strconv.ParseUint(string(cell.Value), 10, 64)
		if err != nil {
			t.Fatalf("%s: bad expected value: %v", label, err)
		}
		return value
	}
	real := func() float64 {
		if !ok {
			return 0
		}
		value, err := strconv.ParseFloat(string(cell.Value), 64)
		if err != nil {
			t.Fatalf("%s: bad expected value: %v", label, err)
		}
		return value
	}
	date := func() hypercast.Date {
		if !ok {
			return hypercast.Date{}
		}
		return hypercast.Date{Year: cell.Year, Month: time.Month(cell.Month), Day: cell.Day}
	}
	switch door {
	case DoorBool:
		return ok && string(cell.Value) == "true"
	case DoorI8:
		return int8(signed())
	case DoorI16:
		return int16(signed())
	case DoorI32:
		return int32(signed())
	case DoorI64:
		return signed()
	case DoorU8:
		return uint8(unsigned())
	case DoorU16:
		return uint16(unsigned())
	case DoorU32:
		return uint32(unsigned())
	case DoorU64:
		return unsigned()
	case DoorF32:
		return float32(real())
	case DoorF64:
		return real()
	case DoorExact:
		if !ok {
			return hypercast.Decimal{}
		}
		// The raw triple is the contract.
		magnitude, parsed := new(big.Int).SetString(cell.Magnitude, 10)
		if !parsed || magnitude.Sign() < 0 || magnitude.BitLen() > 96 {
			t.Fatalf("%s: bad expected magnitude %q", label, cell.Magnitude)
		}
		lo := new(big.Int).And(magnitude, new(big.Int).SetUint64(math.MaxUint64))
		hi := new(big.Int).Rsh(magnitude, 64)
		return hypercast.Decimal{Lo: lo.Uint64(), Hi: uint32(hi.Uint64()), Scale: cell.Scale, Negative: cell.Negative}
	case DoorUuid:
		if !ok {
			return uuid.UUID{}
		}
		var text string
		if err := json.Unmarshal(cell.Value, &text); err != nil {
			t.Fatalf("%s: bad expected value: %v", label, err)
		}
		value, err := uuid.Parse(text)
		if err != nil {
			t.Fatalf("%s: bad expected value: %v", label, err)
		}
		return value
	case DoorTimestamp, DoorUnix, DoorExcelSerial:
		if !ok {
			return time.Time{}
		}
		return time.Unix(cell.Seconds, cell.Nanos).UTC()
	case DoorDateOnly, DoorDateOnlyOrdered:
		return date()
	case DoorDateTime:
		if !ok {
			return hypercast.CivilDateTime{}
		}
		return hypercast.CivilDateTime{Date: date(), TimeOfDay: time.Duration(cell.NanosOfDay)}
	case DoorTimeOfDay:
		if !ok {
			return time.Duration(0)
		}
		return time.Duration(cell.Nanos)
	case DoorSpan:
		if !ok {
			return hypercast.Duration{}
		}
		if cell.Nanos < math.MinInt32 || cell.Nanos > math.MaxInt32 {
			t.Fatalf("%s: corpus nanos out of i32", label)
		}
		return hypercast.Duration{Seconds: cell.Seconds, Nanos: int32(cell.Nanos)}
	case DoorText:
		if !ok {
			return []byte(nil)
		}
		if cell.Text == nil {
			t.Fatalf("%s: an ok text cell with no text", label)
		}
		return []byte(*cell.Text)
	default:
		t.Fatalf("%s: no expectation for %v", label, door)
		return nil
	}
}

// assertCell holds one cell of the batch in hand to what the corpus says of it.
func assertCell(t *testing.T, label string, b *Batch, column, row int, expected *corpusCell) {
	t.Helper()
	door := b.Columns()[column].Door()
	verdict := b.Verdicts(column)[row]
	fault := b.Fault(column, row)

	if expected.Expect == "ok" {
		if !verdict.OK() || fault != nil {
			t.Errorf("%s: expected a value, got %v", label, fault)
			return
		}
	} else {
		reason, known := expectedReason[expected.Expect]
		if !known {
			t.Fatalf("%s: unknown expectation %q", label, expected.Expect)
		}
		if verdict.OK() || fault == nil {
			t.Errorf("%s: expected %s, got the value %v", label, expected.Expect, got(b, column, row))
			return
		}
		if verdict.Reason() != reason || fault.Reason != reason {
			t.Errorf("%s: %v, want %v", label, fault.Reason, reason)
		}
		if fault.Offset != verdict.Offset() || fault.Length != verdict.Length() {
			t.Errorf("%s: the fault %v and the verdict (%d,%d) disagree", label, fault, verdict.Offset(), verdict.Length())
		}
		if expected.Fault != nil {
			if fault.Offset != expected.Fault[0] || fault.Length != expected.Fault[1] {
				t.Errorf("%s: fault span (%d,%d), want (%d,%d)", label, fault.Offset, fault.Length, expected.Fault[0], expected.Fault[1])
			}
			// The cell's own text is still to hand, for the diagnostic a fault deserves.
			if raw := string(b.Raw(column, row)); expected.Raw == nil || raw != *expected.Raw {
				t.Errorf("%s: raw text %q, want %v", label, raw, expected.Raw)
			}
		}
	}

	if value, expectation := got(b, column, row), want(t, label, door, expected); !reflect.DeepEqual(value, expectation) {
		t.Errorf("%s: %v (%T), want %v (%T)", label, value, value, expectation, expectation)
	}
	// Get says what the column accessor and the verdict say together.
	if value, getFault := viaGet(b, column, row); !reflect.DeepEqual(getFault, fault) {
		t.Errorf("%s: Get's fault %v, want %v", label, getFault, fault)
	} else if expectation := want(t, label, door, expected); !reflect.DeepEqual(value, expectation) {
		t.Errorf("%s: Get %v (%T), want %v (%T)", label, value, value, expectation, expectation)
	}
	if door == DoorText && expected.Expect == "ok" {
		// A text cell is its own raw text, escaped quotes resolved either way.
		if raw := string(b.Raw(column, row)); raw != *expected.Text {
			t.Errorf("%s: raw text %q, want %q", label, raw, *expected.Text)
		}
	}
}

func assertFailure(t *testing.T, label string, actual error, expected *corpusFailure) {
	t.Helper()
	if expected == nil {
		if actual != io.EOF {
			t.Errorf("%s: ended with %v, want io.EOF", label, actual)
		}
		return
	}
	var failure *Failure
	if !errors.As(actual, &failure) {
		t.Errorf("%s: ended with %v, want a %s failure", label, actual, expected.Kind)
		return
	}
	// The corpus names a kind as the core does: its name, in snake case.
	kind := failure.Kind
	if name := strings.ReplaceAll(kind.String(), " ", "_"); name != expected.Kind {
		t.Errorf("%s: a %s failure, want %s", label, name, expected.Kind)
	}
	if failure.Record != expected.Record || failure.Line != expected.Line || failure.Byte != expected.Byte {
		t.Errorf("%s: failure %+v, want %+v", label, *failure, *expected)
	}
	if kind == ColumnCount && (failure.Expected != expected.Expected || failure.Found != expected.Found) {
		t.Errorf("%s: failure %+v, want %+v", label, *failure, *expected)
	}
}

// replay reads one case through a reader built by open and holds it to the corpus: the
// header, every cell, and how the input ended.
func replay(t *testing.T, label string, c *corpusCase, open func(Dialect, []Column) (*DelimitedReader, error)) {
	t.Helper()
	dialect := Dialect{
		Separator:      c.Dialect.Separator[0],
		Quoting:        c.Dialect.Quoting,
		HasHeader:      c.Dialect.HasHeader,
		SkipBlankLines: c.Dialect.SkipBlankLines,
	}
	plan := make([]Column, len(c.Plan))
	for index := range c.Plan {
		plan[index] = c.Plan[index].column(t)
	}

	r, err := open(dialect, plan)
	if err != nil {
		// A header that is structurally broken is the constructor's error: the failure, and
		// no rows before it.
		if len(c.Rows) != 0 {
			t.Errorf("%s: could not be opened: %v", label, err)
			return
		}
		assertFailure(t, label, err, c.Failure)
		return
	}
	defer r.Close()

	switch header := r.Header(); {
	case c.Header == nil:
		if header != nil {
			t.Errorf("%s: header %q, want none", label, header)
		}
	case header == nil || !reflect.DeepEqual(header, *c.Header):
		t.Errorf("%s: header %q, want %q", label, header, *c.Header)
	}
	if !reflect.DeepEqual(r.Plan(), plan) {
		t.Errorf("%s: plan %v, want %v", label, r.Plan(), plan)
	}

	seen := 0
	var ended error
	for {
		b, err := r.Read()
		if err != nil {
			ended = err
			if b != nil {
				t.Errorf("%s: a batch beside the error %v", label, err)
			}
			break
		}
		if b.Rows() == 0 {
			t.Fatalf("%s: Read returned an empty batch and no error", label)
		}
		for row := 0; row < b.Rows(); row, seen = row+1, seen+1 {
			if seen >= len(c.Rows) {
				t.Fatalf("%s: more than the %d rows expected", label, len(c.Rows))
			}
			for column := range plan {
				assertCell(t, fmt.Sprintf("%s, row %d, column %d", label, seen, column), b, column, row, &c.Rows[seen][column])
			}
		}
	}
	if seen != len(c.Rows) {
		t.Errorf("%s: %d rows, want %d", label, seen, len(c.Rows))
	}
	assertFailure(t, label, ended, c.Failure)
	// However the input ended is final: the same error, again.
	if _, again := r.Read(); again != ended {
		t.Errorf("%s: a second Read after the end returned %v, want the same %v", label, again, ended)
	}
}

func TestDelimitedCorpus(t *testing.T) {
	corpus := delimitedCorpus(t)
	if len(corpus) < 30 {
		t.Fatalf("the corpus has %d cases; expected at least 30", len(corpus))
	}
	directory := t.TempDir()
	doors := map[Door]bool{}
	for index := range corpus {
		c := &corpus[index]
		input := []byte(c.Input)
		for _, planned := range c.Plan {
			doors[planned.column(t).Door()] = true
		}
		path := filepath.Join(directory, fmt.Sprintf("case-%d.csv", index))
		if err := os.WriteFile(path, input, 0o600); err != nil {
			t.Fatal(err)
		}

		for _, batchRows := range []int{1, 2, 1024} {
			replay(t, fmt.Sprintf("%s (memory, %d rows a batch)", c.Name, batchRows), c,
				func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
					return NewDelimitedReaderBytes(input, dialect, plan, BatchRows(batchRows))
				})
			for _, bufferBytes := range []int{1, 5, 64, DefaultBufferBytes} {
				replay(t, fmt.Sprintf("%s (stream through %d bytes, %d rows a batch)", c.Name, bufferBytes, batchRows), c,
					func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
						return NewDelimitedReader(bytes.NewReader(input), dialect, plan, BatchRows(batchRows), BufferBytes(bufferBytes))
					})
			}
			// A source that hands over one byte a call, and one that reports the end of the
			// input together with its last bytes rather than after them.
			replay(t, fmt.Sprintf("%s (one byte a read, %d rows a batch)", c.Name, batchRows), c,
				func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
					return NewDelimitedReader(iotest.OneByteReader(bytes.NewReader(input)), dialect, plan, BatchRows(batchRows), BufferBytes(7))
				})
			replay(t, fmt.Sprintf("%s (end of input beside the last bytes, %d rows a batch)", c.Name, batchRows), c,
				func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
					return NewDelimitedReader(iotest.DataErrReader(bytes.NewReader(input)), dialect, plan, BatchRows(batchRows), BufferBytes(16))
				})
			// The text in memory, handed to the core a window at a time: every window but
			// the last is cut wherever the ceiling falls. Sized so each record still fits.
			window := 1
			for _, line := range bytes.SplitAfter(input, []byte("\n")) {
				window = max(window, len(line))
			}
			if !bytes.Contains(input, []byte(`"`)) {
				replay(t, fmt.Sprintf("%s (memory in %d-byte windows, %d rows a batch)", c.Name, window, batchRows), c,
					func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
						return NewDelimitedReaderBytes(input, dialect, plan, BatchRows(batchRows), rowCeiling(window))
					})
			}
			replay(t, fmt.Sprintf("%s (file, %d rows a batch)", c.Name, batchRows), c,
				func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
					return OpenDelimited(path, dialect, plan, BatchRows(batchRows), BufferBytes(32))
				})
		}
	}
	// The corpus reaches every door this binding has a constructor for.
	for door := DoorBool; door <= DoorExcelSerial; door++ {
		if !doors[door] {
			t.Errorf("no corpus case casts through %v", door)
		}
	}
}
