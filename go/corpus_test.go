package hypertabular

// Replays the shared conformance corpus (corpus/delimited.json at the repository root) — the
// same file the Rust and C# bindings replay — through this binding: from memory, from an
// io.Reader read through buffers too small for a record, from a source that hands over one
// byte at a time, and from a file, in batches of one row, of two and of many — each of the
// three also opened header first, its plan bound after, and with the notes off. How the input
// is cut up, and when the plan is given, is the binding's business and must not change the
// answer. A key this file doesn't know fails the decode, so that one added to the corpus can't be
// passed over.

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"math/big"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"testing"
	"testing/iotest"
	"time"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	"github.com/google/uuid"
)

type corpusCase struct {
	Name     string `json:"name"`
	Input    string `json:"input"`
	InputHex string `json:"input_hex"`
	Dialect  struct {
		Separator      string   `json:"separator"`
		Quoting        bool     `json:"quoting"`
		HasHeader      bool     `json:"has_header"`
		SkipBlankLines bool     `json:"skip_blank_lines"`
		Skip           uint32   `json:"skip"`
		HeaderRow      uint32   `json:"header_row"`
		ShortRows      string   `json:"short_rows"`
		LongRows       string   `json:"long_rows"`
		Encoding       string   `json:"encoding"`
		Tokens         []string `json:"tokens"`
	} `json:"dialect"`
	Plan []corpusColumn `json:"plan"`
	// null when the dialect declares no header, [] for an input with no record.
	Header   *[]string       `json:"header"`
	Rows     [][]corpusCell  `json:"rows"`
	Problems []corpusProblem `json:"problems"`
	Suggest  *corpusSuggest  `json:"suggest"`
	Failure  *corpusFailure  `json:"failure"`
}

// bytes is the case's input, the original bytes when they aren't UTF-8.
func (c *corpusCase) bytes(t *testing.T) []byte {
	t.Helper()
	if c.InputHex != "" {
		return unhex(t, c.InputHex)
	}
	return []byte(c.Input)
}

func unhex(t *testing.T, text string) []byte {
	t.Helper()
	data, err := hex.DecodeString(text)
	if err != nil {
		t.Fatalf("%q: %v", text, err)
	}
	return data
}

// dialect is the case's dialect as this binding declares it.
func (c *corpusCase) dialect(t *testing.T) Dialect {
	t.Helper()
	dialect := Dialect{
		Separator:      c.Dialect.Separator[0],
		Quoting:        c.Dialect.Quoting,
		HasHeader:      c.Dialect.HasHeader,
		SkipBlankLines: c.Dialect.SkipBlankLines,
		Skip:           c.Dialect.Skip,
		HeaderRow:      c.Dialect.HeaderRow,
		Tokens:         NewTokens(c.Dialect.Tokens...),
	}
	switch c.Dialect.ShortRows {
	case "", "reject":
	case "pad":
		dialect.ShortRows = ShortRowsPad
	default:
		t.Fatalf("%s: short_rows %q", c.Name, c.Dialect.ShortRows)
	}
	switch c.Dialect.LongRows {
	case "", "reject":
	case "report":
		dialect.LongRows = LongRowsReport
	case "truncate":
		dialect.LongRows = LongRowsTruncate
	default:
		t.Fatalf("%s: long_rows %q", c.Name, c.Dialect.LongRows)
	}
	switch c.Dialect.Encoding {
	case "":
	case "windows-1252":
		dialect.Encoding = Windows1252
	case "latin-1":
		dialect.Encoding = Latin1
	case "utf-16le":
		dialect.Encoding = UTF16LE
	default:
		t.Fatalf("%s: encoding %q", c.Name, c.Dialect.Encoding)
	}
	return dialect
}

// corpusProblem is an entry of a batch's faults list, in the corpus's shape.
type corpusProblem struct {
	Row          int     `json:"row"`
	Line         int     `json:"line"`
	PlanColumn   *int    `json:"plan_column"`
	SourceColumn *int    `json:"source_column"`
	Reason       *string `json:"reason"`
	Fault        []int   `json:"fault"`
	Detail       *string `json:"detail"`
	Origin       *string `json:"origin"`
	Expected     *int    `json:"expected"`
	Found        *int    `json:"found"`
	Keyword      *string `json:"keyword"`
}

// corpusSuggest is what hypertabular_suggest says of a case's input.
type corpusSuggest struct {
	Columns    []corpusSuggested `json:"columns"`
	Truncated  bool              `json:"truncated"`
	SampleRows *int              `json:"sample_rows"`
	Format     *corpusFormat     `json:"format"`
}

type corpusSuggested struct {
	Door    string `json:"door"`
	Param   uint32 `json:"param"`
	Status  string `json:"status"`
	Matched int    `json:"matched"`
	Sampled int    `json:"sampled"`
}

type corpusFormat struct {
	DecimalSep string `json:"decimal_sep"`
	GroupSep   string `json:"group_sep"`
	Flags      uint32 `json:"flags"`
	Currency   string `json:"currency"` // absent ⇒ none declared
}

func (f *corpusFormat) format() hypercast.NumFormat {
	return hypercast.NumFormat{
		DecimalSep: f.DecimalSep,
		GroupSep:   f.GroupSep,
		Styles:     hypercast.NumStyles(f.Flags),
		Currency:   f.Currency,
	}
}

type corpusColumn struct {
	Door      string        `json:"door"`
	Ordinal   int           `json:"ordinal"`
	Precision uint32        `json:"precision"`
	Order     uint32        `json:"order"`
	Epoch     uint32        `json:"epoch"`
	Format    *corpusFormat `json:"format"`
	// Absent inherits the dialect's or the sheet's, [] declares none.
	Tokens  *[]string `json:"tokens"`
	Options *struct {
		Flags  []string `json:"flags"`
		Pivot  uint32   `json:"pivot"`
		Months [][2]any `json:"months"`
	} `json:"options"`
	Constraints *corpusConstraints `json:"constraints"`
}

// corpusConstraints is a plan entry's constraints: every bound, step and numeric entry a decimal's
// text.
type corpusConstraints struct {
	Minimum          *string   `json:"minimum"`
	ExclusiveMinimum *string   `json:"exclusiveMinimum"`
	Maximum          *string   `json:"maximum"`
	ExclusiveMaximum *string   `json:"exclusiveMaximum"`
	MultipleOf       *string   `json:"multipleOf"`
	Rounding         *string   `json:"rounding"`
	MinLength        *int      `json:"minLength"`
	MaxLength        *int      `json:"maxLength"`
	Enum             *[]string `json:"enum"`
	Trim             bool      `json:"trim"`
	CaseSensitive    bool      `json:"caseSensitive"`
}

// A cell is a verdict in HyperCast's corpus shape, with `text` for the text door, and — for
// a fault — its span and `raw`, the cell's own text.
type corpusCell struct {
	Expect     string          `json:"expect"`
	Value      json.RawMessage `json:"value"`
	Text       *string         `json:"text"`
	Fault      []int           `json:"fault"`
	Raw        *string         `json:"raw"`
	RawHex     *string         `json:"raw_hex"`
	Detail     *string         `json:"detail"`
	Origin     *string         `json:"origin"`
	Seconds    int64           `json:"seconds"`
	Nanos      int64           `json:"nanos"`
	Year       int             `json:"year"`
	Month      int             `json:"month"`
	Day        int             `json:"day"`
	NanosOfDay uint64          `json:"nanos_of_day"`
	Magnitude  string          `json:"magnitude"`
	Scale      uint8           `json:"scale"`
	Negative   bool            `json:"negative"`
	// A constrained column's: the keyword a violation broke, whether rounding adjusted a value, the
	// text enum entry it matched, and a float's exact bits.
	Keyword   *string `json:"keyword"`
	Adjusted  *bool   `json:"adjusted"`
	EnumIndex *int    `json:"enum_index"`
	Bits      *string `json:"bits"`
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
// directory the tests read. It also looks under testdata/, where CI stages the corpus for the
// iOS simulator: Go's go_ios_exec carries the module's files and testdata directories into
// the app, and nothing above the module, so the repository's corpus/ is not there to walk
// up to. On Android (.github/scripts/android_device_test.sh) the corpus is pushed beside the
// test binary, in the directory it runs in, which is the first place looked. A WASI runtime
// sees only what is mounted, so the corpus is wherever HYPERTABULAR_CORPUS names when it is
// set (the forge's wasm job mounts the checkout and points there).
func repositoryFile(t *testing.T, elem ...string) string {
	t.Helper()
	if dir := os.Getenv("HYPERTABULAR_CORPUS"); dir != "" && len(elem) > 0 && elem[0] == "corpus" {
		candidate := filepath.Join(append([]string{dir}, elem[1:]...)...)
		if _, err := os.Stat(candidate); err != nil {
			t.Fatalf("%s not found at %s: %v", filepath.Join(elem...), candidate, err)
		}
		return candidate
	}
	dir, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	for {
		for _, base := range []string{dir, filepath.Join(dir, "testdata")} {
			candidate := filepath.Join(append([]string{base}, elem...)...)
			if _, err := os.Stat(candidate); err == nil {
				return candidate
			}
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
	if err := decodeStrictly(data, &cases); err != nil {
		t.Fatalf("parsing %s: %v", path, err)
	}
	return cases
}

// decodeStrictly fails on a key the destination doesn't have, at any level.
func decodeStrictly(data []byte, into any) error {
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.DisallowUnknownFields()
	return decoder.Decode(into)
}

// junkEverywhere is the corpus's decode, once for every place in it that holds an object: a
// made-up key put there has to fail it. decode says whether a document decodes.
func junkEverywhere(t *testing.T, path string, decode func([]byte) error) {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var cases []any
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatal(err)
	}
	places := map[string]bool{}
	for _, one := range cases {
		var object func(place string, value any)
		object = func(place string, value any) {
			switch value := value.(type) {
			case []any:
				for _, item := range value {
					object(place+"/[]", item)
				}
			case map[string]any:
				for key, inner := range value {
					object(place+"/"+key, inner)
				}
				if places[place] {
					return
				}
				places[place] = true
				value["no such key"] = true
				document, err := json.Marshal([]any{one})
				delete(value, "no such key")
				if err != nil {
					t.Fatal(err)
				}
				if decode(document) == nil {
					t.Errorf("%s: a key this replay doesn't know was passed over at %q", path, place)
				}
			}
		}
		object("", one)
	}
	if len(places) < 8 {
		t.Errorf("%s: only %d places hold an object: %v", path, len(places), places)
	}
}

func TestEveryDelimitedCorpusKeyIsRead(t *testing.T) {
	junkEverywhere(t, repositoryFile(t, "corpus", "delimited.json"), func(document []byte) error {
		var cases []corpusCase
		return decodeStrictly(document, &cases)
	})
}

// format is the entry's format. No format means hypercast.Invariant, flags 95 (corpus/README.md).
func (c *corpusColumn) format() hypercast.NumFormat {
	if c.Format == nil {
		return hypercast.Invariant
	}
	return c.Format.format()
}

// dateOptions is the entry's date options, in HyperCast's own type.
func (c *corpusColumn) dateOptions(t *testing.T) hypercast.DateOptions {
	t.Helper()
	var options hypercast.DateOptions
	options.Pivot = int(c.Options.Pivot)
	for _, flag := range c.Options.Flags {
		switch flag {
		case "compact_date":
			options.Shapes |= hypercast.CompactDate
		case "space_separator":
			options.Shapes |= hypercast.SpaceSeparator
		case "hour_offset":
			options.Shapes |= hypercast.HourOffset
		default:
			t.Fatalf("date flag %q", flag)
		}
	}
	if len(c.Options.Months) > 0 {
		names := make([]hypercast.MonthName, 0, len(c.Options.Months))
		for _, entry := range c.Options.Months {
			names = append(names, hypercast.MonthName{Month: time.Month(entry[0].(float64)), Name: entry[1].(string)})
		}
		months, err := hypercast.NewMonthNames(names...)
		if err != nil {
			t.Fatal(err)
		}
		options.Months = months
	}
	return options
}

// column is the plan entry as this binding declares it, with its tokens and date options.
func (c *corpusColumn) column(t *testing.T) Column {
	t.Helper()
	var column Column
	if c.Constraints != nil {
		column = c.checked(t)
	} else {
		column = c.door(t)
	}
	if c.Tokens != nil {
		column = column.WithTokens(*c.Tokens...)
	}
	if c.Options != nil {
		column = column.WithDateOptions(c.dateOptions(t))
	}
	return column
}

// checked is a plan entry that declares constraints, through the constructor of its door.
func (c *corpusColumn) checked(t *testing.T) Column {
	t.Helper()
	d := c.Constraints
	var rules []hypercast.Rule
	for _, bound := range []struct {
		rule  func(string) hypercast.Rule
		value *string
	}{
		{hypercast.Minimum[string], d.Minimum}, {hypercast.ExclusiveMinimum[string], d.ExclusiveMinimum},
		{hypercast.Maximum[string], d.Maximum}, {hypercast.ExclusiveMaximum[string], d.ExclusiveMaximum},
		{hypercast.MultipleOf[string], d.MultipleOf},
	} {
		if bound.value != nil {
			rules = append(rules, bound.rule(*bound.value))
		}
	}
	if d.Rounding != nil {
		rounding, known := map[string]hypercast.Rounding{
			"strict": hypercast.Strict, "midpoint": hypercast.Midpoint, "floor": hypercast.Floor, "ceiling": hypercast.Ceiling,
		}[*d.Rounding]
		if !known {
			t.Fatalf("unknown rounding %q", *d.Rounding)
		}
		rules = append(rules, hypercast.WithRounding(rounding))
	}
	if d.MinLength != nil {
		rules = append(rules, hypercast.MinLength(*d.MinLength))
	}
	if d.MaxLength != nil {
		rules = append(rules, hypercast.MaxLength(*d.MaxLength))
	}
	if d.Trim {
		rules = append(rules, hypercast.Trim())
	}
	if d.CaseSensitive {
		rules = append(rules, hypercast.CaseSensitive())
	}
	if d.Enum != nil {
		rules = append(rules, hypercast.Enum(*d.Enum...))
	}
	format := c.format()
	switch c.Door {
	case "i8":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[int8](rules...))
	case "i16":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[int16](rules...))
	case "i32":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[int32](rules...))
	case "i64":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[int64](rules...))
	case "u8":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[uint8](rules...))
	case "u16":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[uint16](rules...))
	case "u32":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[uint32](rules...))
	case "u64":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[uint64](rules...))
	case "f32":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[float32](rules...))
	case "f64":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[float64](rules...))
	case "decimal":
		return Checked(c.Ordinal, format, hypercast.MustConstraints[hypercast.Decimal](rules...))
	case "text":
		return CheckedText(c.Ordinal, hypercast.MustConstraints[string](rules...))
	}
	t.Fatalf("no checked door %q", c.Door)
	return Column{}
}

// door is the plan entry's door: named as HyperCast's corpus names its types, with what the door
// declares beside it.
func (c *corpusColumn) door(t *testing.T) Column {
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

// judgement is a checked cell with its type set aside, so that one comparison serves every door.
type judgement struct {
	value     any
	adjusted  bool
	enumIndex int
	fault     *hypercast.Fault
	violation *hypercast.Keyword
	offset    int
	length    int
}

// judgedAs is the cell through GetChecked, with CellChecked held to it.
func judgedAs[T hypercast.CheckedTarget](t *testing.T, label string, b *Batch, column, row int) judgement {
	t.Helper()
	passed, rejection := GetChecked[T](b, column, row)
	viaRow, rowRejection := CellChecked[T](b.Row(row), column)
	if !reflect.DeepEqual(passed, viaRow) || !reflect.DeepEqual(rejection, rowRejection) {
		t.Errorf("%s: CellChecked %v, %v; GetChecked %v, %v", label, viaRow, rowRejection, passed, rejection)
	}
	j := judgement{value: passed.Value, adjusted: passed.Adjusted, enumIndex: passed.EnumIndex}
	switch r := rejection.(type) {
	case nil:
	case *hypercast.Fault:
		j.fault = r
	case *hypercast.Violation[T]:
		j.violation, j.value, j.offset, j.length = &r.Keyword, r.Value, r.Offset, r.Length
	default:
		t.Fatalf("%s: a rejection of %T", label, rejection)
	}
	if rejection != nil && (passed.Adjusted || passed.EnumIndex != -1) {
		t.Errorf("%s: a rejection beside %+v", label, passed)
	}
	return j
}

// judgedCell is the cell through GetChecked, as the type its door presents.
func judgedCell(t *testing.T, label string, b *Batch, column, row int) judgement {
	t.Helper()
	switch door := b.Columns()[column].Door(); door {
	case DoorI8:
		return judgedAs[int8](t, label, b, column, row)
	case DoorI16:
		return judgedAs[int16](t, label, b, column, row)
	case DoorI32:
		return judgedAs[int32](t, label, b, column, row)
	case DoorI64:
		return judgedAs[int64](t, label, b, column, row)
	case DoorU8:
		return judgedAs[uint8](t, label, b, column, row)
	case DoorU16:
		return judgedAs[uint16](t, label, b, column, row)
	case DoorU32:
		return judgedAs[uint32](t, label, b, column, row)
	case DoorU64:
		return judgedAs[uint64](t, label, b, column, row)
	case DoorF32:
		return judgedAs[float32](t, label, b, column, row)
	case DoorF64:
		return judgedAs[float64](t, label, b, column, row)
	case DoorExact:
		return judgedAs[hypercast.Decimal](t, label, b, column, row)
	case DoorText:
		return judgedAs[string](t, label, b, column, row)
	default:
		t.Fatalf("%s: no checked door %v", label, door)
		return judgement{}
	}
}

// assertJudgedValue holds a checked value to the corpus: a float by its exact bits, anything else as
// want says it, a cell that cast.
func assertJudgedValue(t *testing.T, label string, door Door, value any, expected *corpusCell) {
	t.Helper()
	var bits string
	switch v := value.(type) {
	case float32:
		bits = fmt.Sprintf("0x%08x", math.Float32bits(v))
	case float64:
		bits = fmt.Sprintf("0x%016x", math.Float64bits(v))
	case string:
		value = []byte(v)
	}
	if expected.Bits != nil {
		if bits != *expected.Bits {
			t.Errorf("%s: bits %s, want %s", label, bits, *expected.Bits)
		}
		return
	}
	cast := *expected
	cast.Expect = "ok"
	if expectation := want(t, label, door, &cast); !reflect.DeepEqual(value, expectation) {
		t.Errorf("%s: %v (%T), want %v (%T)", label, value, value, expectation, expectation)
	}
}

// assertChecked holds a constrained column's cell to what the corpus says of it, through the
// checked getters: a value with its adjusted flag and enum match, a fault, or a violation with its
// keyword and the value as cast. The plain getters refuse the column.
func assertChecked(t *testing.T, label string, b *Batch, column, row int, expected *corpusCell) {
	t.Helper()
	door := b.Columns()[column].Door()
	verdict := b.Verdicts(column)[row]
	if verdict.Violation() != (expected.Expect == "violation") {
		t.Errorf("%s: a violation %v, want %s", label, verdict.Violation(), expected.Expect)
	}
	if !panics(func() { viaGet(b, column, row) }) {
		t.Errorf("%s: Get read a constrained column", label)
	}
	j := judgedCell(t, label, b, column, row)
	switch {
	case expected.Expect == "ok":
		if j.fault != nil || j.violation != nil || !verdict.OK() {
			t.Errorf("%s: expected a value, got %+v", label, j)
			return
		}
		if expected.Adjusted == nil || j.adjusted != *expected.Adjusted {
			t.Errorf("%s: adjusted %v, want %v", label, j.adjusted, shown(expected.Adjusted))
		}
		wantIndex := -1
		if expected.EnumIndex != nil {
			wantIndex = *expected.EnumIndex
		}
		if j.enumIndex != wantIndex {
			t.Errorf("%s: enum index %d, want %d", label, j.enumIndex, wantIndex)
		}
		assertJudgedValue(t, label, door, j.value, expected)
	case expected.Expect == "violation":
		if j.violation == nil || expected.Keyword == nil || j.violation.String() != *expected.Keyword {
			t.Errorf("%s: violation %v, want %v", label, j.violation, shown(expected.Keyword))
			return
		}
		if b.Fault(column, row) != nil || verdict.Reason() != 0 {
			t.Errorf("%s: a violation with a fault", label)
		}
		if j.offset != expected.Fault[0] || j.length != expected.Fault[1] ||
			verdict.Offset() != j.offset || verdict.Length() != j.length {
			t.Errorf("%s: violation span (%d,%d), want %v", label, j.offset, j.length, expected.Fault)
		}
		if raw := string(b.Raw(column, row)); expected.Raw == nil || raw != *expected.Raw {
			t.Errorf("%s: raw text %q, want %v", label, raw, expected.Raw)
		}
		assertJudgedValue(t, label, door, j.value, expected)
	default:
		reason, known := expectedReason[expected.Expect]
		if !known {
			t.Fatalf("%s: unknown expectation %q", label, expected.Expect)
		}
		if j.fault == nil || j.fault.Reason != reason || !reflect.DeepEqual(j.fault, b.Fault(column, row)) {
			t.Errorf("%s: fault %v, want %s", label, j.fault, expected.Expect)
			return
		}
		if expected.Fault != nil {
			if j.fault.Offset != expected.Fault[0] || j.fault.Length != expected.Fault[1] {
				t.Errorf("%s: fault span (%d,%d), want %v", label, j.fault.Offset, j.fault.Length, expected.Fault)
			}
			switch raw := b.Raw(column, row); {
			case expected.RawHex != nil:
				if !bytes.Equal(raw, unhex(t, *expected.RawHex)) {
					t.Errorf("%s: raw bytes %x, want %s", label, raw, *expected.RawHex)
				}
			case expected.Raw == nil || string(raw) != *expected.Raw:
				t.Errorf("%s: raw text %q, want %v", label, raw, expected.Raw)
			}
		}
	}
}

// panics reports whether f panics.
func panics(f func()) (panicked bool) {
	defer func() { panicked = recover() != nil }()
	f()
	return false
}

// assertCell holds one cell of the batch in hand to what the corpus says of it.
func assertCell(t *testing.T, label string, b *Batch, column, row int, expected *corpusCell) {
	t.Helper()
	if b.Columns()[column].IsChecked() {
		assertChecked(t, label, b, column, row, expected)
		return
	}
	door := b.Columns()[column].Door()
	verdict := b.Verdicts(column)[row]
	fault := b.Fault(column, row)
	if verdict.Violation() {
		t.Errorf("%s: a violation in a column that declares no constraints", label)
	}

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
			switch raw := b.Raw(column, row); {
			case expected.RawHex != nil:
				if !bytes.Equal(raw, unhex(t, *expected.RawHex)) {
					t.Errorf("%s: raw bytes %x, want %s", label, raw, *expected.RawHex)
				}
			case expected.Raw == nil || string(raw) != *expected.Raw:
				t.Errorf("%s: raw text %q, want %v", label, raw, expected.Raw)
			}
		}
	}

	// The note beside the verdict: with notes off there's none, and it says so.
	wantDetail, wantOrigin := "none", "cell"
	if b.columns.notes {
		if expected.Detail != nil {
			wantDetail = *expected.Detail
		}
		if expected.Origin != nil {
			wantOrigin = *expected.Origin
		}
	}
	if detail := b.Detail(column, row).String(); detail != wantDetail {
		t.Errorf("%s: detail %q, want %q", label, detail, wantDetail)
	}
	if origin := b.Origin(column, row).String(); origin != wantOrigin {
		t.Errorf("%s: origin %q, want %q", label, origin, wantOrigin)
	}

	if value, expectation := got(b, column, row), want(t, label, door, expected); !reflect.DeepEqual(value, expectation) {
		t.Errorf("%s: %v (%T), want %v (%T)", label, value, value, expectation, expectation)
	}
	// Get says what the column accessor and the verdict say together.
	assertRowView(t, label, b, column, row)
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

// assertRowView holds the row view of a cell to the batch's own word on it: each Row member
// is the Batch call it stands for.
func assertRowView(t *testing.T, label string, b *Batch, column, row int) {
	t.Helper()
	view := b.Row(row)
	if view.Index() != row || view.Batch() != b || view.Line() != b.Line(row) {
		t.Errorf("%s: row %d of line %d, want row %d of line %d", label, view.Index(), view.Line(), row, b.Line(row))
	}
	if view.Verdict(column) != b.Verdicts(column)[row] || !reflect.DeepEqual(view.Fault(column), b.Fault(column, row)) {
		t.Errorf("%s: the row's verdict %v, the batch's %v", label, view.Verdict(column), b.Verdicts(column)[row])
	}
	if raw := string(view.Raw(column)); raw != string(b.Raw(column, row)) {
		t.Errorf("%s: the row's raw text %q, the batch's %q", label, raw, b.Raw(column, row))
	}
	if view.Detail(column) != b.Detail(column, row) || view.Origin(column) != b.Origin(column, row) {
		t.Errorf("%s: the row's note %v %v, the batch's %v %v", label, view.Detail(column), view.Origin(column), b.Detail(column, row), b.Origin(column, row))
	}
	if b.Columns()[column].Door() != DoorText {
		return
	}
	text := b.Text(column)[row]
	if !bytes.Equal(view.Text(column), text) || (text == nil) != (view.Text(column) == nil) {
		t.Errorf("%s: the row's text %q, the batch's %q", label, view.Text(column), text)
	}
	if got, viewed := b.TextString(column, row), view.TextString(column); got != string(text) || viewed != got {
		t.Errorf("%s: text as a string %q and %q, want %q", label, got, viewed, text)
	}
	value, fault := Cell[[]byte](view, column)
	if want, wantFault := Get[[]byte](b, column, row); !bytes.Equal(value, want) || !reflect.DeepEqual(fault, wantFault) {
		t.Errorf("%s: Cell %q, %v; Get %q, %v", label, value, fault, want, wantFault)
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
	if failure.Expected != expected.Expected || failure.Found != expected.Found {
		t.Errorf("%s: failure %+v, want %+v", label, *failure, *expected)
	}
}

// replay reads one case through a reader built by open and holds it to the corpus: the
// header, every cell, and how the input ended.
func replay(t *testing.T, label string, c *corpusCase, open func(Dialect, []Column) (*DelimitedReader, error)) {
	t.Helper()
	boundHeap()
	dialect := c.dialect(t)
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
	case header == nil || !reflect.DeepEqual([]string(header), *c.Header):
		t.Errorf("%s: header %q, want %q", label, header, *c.Header)
	default:
		assertLookups(t, label, header)
	}
	if !reflect.DeepEqual(r.Plan(), plan) {
		t.Errorf("%s: plan %v, want %v", label, r.Plan(), plan)
	}

	seen := 0
	var ended error
	var problems []corpusProblem
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
		problems = appendProblems(t, label, problems, b, seen)
		for row := 0; row < b.Rows(); row, seen = row+1, seen+1 {
			if seen >= len(c.Rows) {
				t.Fatalf("%s: more than the %d rows expected", label, len(c.Rows))
			}
			if len(c.Rows[seen]) != len(plan) {
				t.Fatalf("%s, row %d: %d cells in the corpus for a plan of %d", label, seen, len(c.Rows[seen]), len(plan))
			}
			for column := range plan {
				assertCell(t, fmt.Sprintf("%s, row %d, column %d", label, seen, column), b, column, row, &c.Rows[seen][column])
			}
		}
	}
	if seen != len(c.Rows) {
		t.Errorf("%s: %d rows, want %d", label, seen, len(c.Rows))
	}
	assertProblems(t, label, problems, c.Problems, r.notes)
	assertFailure(t, label, ended, c.Failure)
	// However the input ended is final: the same error, again.
	if _, again := r.Read(); again != ended {
		t.Errorf("%s: a second Read after the end returned %v, want the same %v", label, again, ended)
	}
}

// appendProblems is the list so far and this batch's Faults, in the corpus's shape: rows counted
// from base, and each entry either a cell's or a row's.
func appendProblems(t *testing.T, label string, problems []corpusProblem, b *Batch, base int) []corpusProblem {
	t.Helper()
	for problem := range b.Faults() {
		entry := corpusProblem{Row: base + problem.Row, Line: problem.Line}
		if problem.Line != b.Line(problem.Row) {
			t.Errorf("%s: a problem on line %d of a row on line %d", label, problem.Line, b.Line(problem.Row))
		}
		// A violation names its keyword and has no reason; anything else has a reason and no keyword.
		if (problem.Keyword != 0) == (problem.Reason != 0) && problem.PlanColumn >= 0 {
			t.Errorf("%s: a reason %v beside a keyword %v", label, problem.Reason, problem.Keyword)
		}
		if problem.PlanColumn >= 0 {
			plan, source := problem.PlanColumn, problem.SourceColumn
			reason := map[hypercast.CastFailure]string{hypercast.Empty: "empty", hypercast.Malformed: "malformed", hypercast.OutOfRange: "out_of_range"}[problem.Reason]
			entry.PlanColumn, entry.SourceColumn, entry.Reason = &plan, &source, &reason
			if problem.Keyword != 0 {
				reason, keyword := "violation", problem.Keyword.String()
				entry.Reason, entry.Keyword = &reason, &keyword
			}
			if problem.Reason != hypercast.Empty {
				entry.Fault = []int{problem.Offset, problem.Length}
			} else if problem.Offset != 0 || problem.Length != 0 {
				t.Errorf("%s: an uncached formula's span: %+v", label, problem)
			}
			if problem.Expected != 0 || problem.Found != 0 {
				t.Errorf("%s: a cell with a row's widths: %+v", label, problem)
			}
		} else {
			if problem.SourceColumn != -1 || problem.Reason != 0 || problem.Offset != 0 || problem.Length != 0 {
				t.Errorf("%s: a row that's half a cell: %+v", label, problem)
			}
			expected, found := problem.Expected, problem.Found
			entry.Expected, entry.Found = &expected, &found
		}
		if problem.Detail != 0 {
			detail := problem.Detail.String()
			entry.Detail = &detail
		}
		if problem.Origin != OriginCell {
			origin := problem.Origin.String()
			entry.Origin = &origin
		}
		problems = append(problems, entry)
	}
	return problems
}

// assertProblems holds what Faults listed to what the corpus says. With notes off the list has no
// detail or origin to give, and no formula with no cached value.
func assertProblems(t *testing.T, label string, got, expected []corpusProblem, notes bool) {
	t.Helper()
	if !notes {
		kept := make([]corpusProblem, 0, len(expected))
		for _, entry := range expected {
			if entry.Origin != nil && *entry.Origin == "uncached" {
				continue
			}
			entry.Detail, entry.Origin = nil, nil
			kept = append(kept, entry)
		}
		expected = kept
	}
	if len(got) == 0 && len(expected) == 0 {
		return
	}
	if !reflect.DeepEqual(got, expected) {
		show := func(list []corpusProblem) string {
			text, _ := json.Marshal(list)
			return string(text)
		}
		t.Errorf("%s: faults %s, want %s", label, show(got), show(expected))
	}
}

// assertLookups holds a header's lookups to its names: every name is found where it first
// is, as a string and as bytes; the strict lookups refuse a name two columns share, naming the
// two; and a name that is not there is an error that names it.
func assertLookups(t *testing.T, label string, header Header) {
	t.Helper()
	for _, name := range header {
		first := slices.Index(header, name)
		if found, ok := header.Find(name); found != first || !ok {
			t.Errorf("%s: Find(%q) = %d, %v; want %d", label, name, found, ok, first)
		}
		if found, ok := header.FindBytes([]byte(name)); found != first || !ok {
			t.Errorf("%s: FindBytes(%q) = %d, %v; want %d", label, name, found, ok, first)
		}
		assertOrdinal := func(which string, found int, err error) {
			t.Helper()
			second := slices.Index(header[first+1:], name)
			if second < 0 {
				if found != first || err != nil {
					t.Errorf("%s: %s(%q) = %d, %v; want %d", label, which, name, found, err, first)
				}
				return
			}
			var ambiguous *AmbiguousColumnError
			if !errors.Is(err, ErrAmbiguousColumn) || !errors.As(err, &ambiguous) ||
				ambiguous.Name != name || ambiguous.First != first || ambiguous.Second != first+1+second || found != -1 {
				t.Errorf("%s: %s(%q) = %d, %v; want it to name %d and %d", label, which, name, found, err, first, first+1+second)
			}
		}
		found, err := header.Ordinal(name)
		assertOrdinal("Ordinal", found, err)
		found, err = header.OrdinalBytes([]byte(name))
		assertOrdinal("OrdinalBytes", found, err)
	}
	missing := strings.Join(header, "|") + "?"
	var noColumn *NoColumnError
	if _, err := header.Ordinal(missing); !errors.Is(err, ErrNoColumn) || !errors.As(err, &noColumn) || noColumn.Name != missing {
		t.Errorf("%s: Ordinal of a missing name: %v", label, err)
	}
}

// bound is a reader opened header first, its plan bound after.
func bound(r *DelimitedReader, err error, plan []Column) (_ *DelimitedReader, failed error) {
	if err != nil {
		return nil, err
	}
	defer func() {
		if failed != nil {
			_ = r.Close()
		}
	}()
	if r.IsBound() || r.Plan() != nil {
		return nil, errors.New("a reader opened without a plan says it has one")
	}
	if _, err := r.Read(); err != ErrUnbound {
		return nil, fmt.Errorf("Read before Bind: %v", err)
	}
	if err := r.Bind(plan); err != nil {
		return nil, err
	}
	if err := r.Bind(plan); err != ErrAlreadyBound {
		return nil, fmt.Errorf("a second Bind: %v", err)
	}
	return r, nil
}

// tempDir is t.TempDir, except under WASI with no temporary directory mounted (the forge's
// TinyGo run mounts only the checkout): there it reports false, and the caller leaves out
// what needs a file of its own.
func tempDir(t *testing.T) (string, bool) {
	t.Helper()
	if runtime.GOOS == "wasip1" {
		if _, err := os.Stat(os.TempDir()); err != nil {
			return "", false
		}
	}
	return t.TempDir(), true
}

// missingPath is a path to a file that is not there.
func missingPath(t *testing.T, name string) string {
	t.Helper()
	if dir, ok := tempDir(t); ok {
		return filepath.Join(dir, name)
	}
	return filepath.Join(string(filepath.Separator), "no such directory", name)
}

func TestCorpusDelimited(t *testing.T) {
	corpus := delimitedCorpus(t)
	if len(corpus) < 30 {
		t.Fatalf("the corpus has %d cases; expected at least 30", len(corpus))
	}
	directory, files := tempDir(t)
	doors := map[Door]bool{}
	for index := range corpus {
		c := &corpus[index]
		input := c.bytes(t)
		for _, planned := range c.Plan {
			doors[planned.column(t).Door()] = true
		}
		path := filepath.Join(directory, fmt.Sprintf("case-%d.csv", index))
		if files {
			if err := os.WriteFile(path, input, 0o600); err != nil {
				t.Fatal(err)
			}
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
			// (Under an encoding memory is decoded through a buffer, which this would cap.)
			if !bytes.Contains(input, []byte(`"`)) && c.Dialect.Encoding == "" {
				replay(t, fmt.Sprintf("%s (memory in %d-byte windows, %d rows a batch)", c.Name, window, batchRows), c,
					func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
						return NewDelimitedReaderBytes(input, dialect, plan, BatchRows(batchRows), rowCeiling(window))
					})
			}
			if files {
				replay(t, fmt.Sprintf("%s (file, %d rows a batch)", c.Name, batchRows), c,
					func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
						return OpenDelimited(path, dialect, plan, BatchRows(batchRows), BufferBytes(32))
					})
			}
			// The notes off: the same rows, and nothing said of why.
			replay(t, fmt.Sprintf("%s (memory, notes off, %d rows a batch)", c.Name, batchRows), c,
				func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
					return NewDelimitedReaderBytes(input, dialect, plan, BatchRows(batchRows), Notes(false))
				})
			// Header first, the plan bound after: the same reads, whatever the source.
			replay(t, fmt.Sprintf("%s (memory, bound after the header, %d rows a batch)", c.Name, batchRows), c,
				func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
					r, err := NewDelimitedReaderBytesUnbound(input, dialect, BatchRows(batchRows))
					return bound(r, err, plan)
				})
			replay(t, fmt.Sprintf("%s (stream through 5 bytes, bound after the header, %d rows a batch)", c.Name, batchRows), c,
				func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
					r, err := NewDelimitedReaderUnbound(bytes.NewReader(input), dialect, BatchRows(batchRows), BufferBytes(5))
					return bound(r, err, plan)
				})
			if files {
				replay(t, fmt.Sprintf("%s (file, bound after the header, %d rows a batch)", c.Name, batchRows), c,
					func(dialect Dialect, plan []Column) (*DelimitedReader, error) {
						r, err := OpenDelimitedUnbound(path, dialect, BatchRows(batchRows), BufferBytes(7))
						return bound(r, err, plan)
					})
			}
		}
	}
	// The corpus reaches every door this binding has a constructor for.
	for door := DoorBool; door <= DoorExcelSerial; door++ {
		if !doors[door] {
			t.Errorf("no corpus case casts through %v", door)
		}
	}
}

// corpusDoors is a door as the corpus names it, and corpusColumn.door the other way.
var corpusDoors = map[Door]string{
	DoorBool: "bool", DoorI8: "i8", DoorI16: "i16", DoorI32: "i32", DoorI64: "i64",
	DoorU8: "u8", DoorU16: "u16", DoorU32: "u32", DoorU64: "u64", DoorF32: "f32", DoorF64: "f64",
	DoorExact: "decimal", DoorUuid: "uuid", DoorTimestamp: "timestamp", DoorUnix: "unix",
	DoorExcelSerial: "excel_serial", DoorDateOnly: "date", DoorDateOnlyOrdered: "date_ordered",
	DoorDateTime: "datetime", DoorTimeOfDay: "time", DoorSpan: "duration", DoorText: "text",
}

// assertSuggestion holds a suggestion to what the corpus says of the columns: the param is put back
// together from the report, as what RawSuggestion held.
func assertSuggestion(t *testing.T, label string, found *Suggestion, expected *corpusSuggest) {
	t.Helper()
	if found.Truncated != expected.Truncated {
		t.Errorf("%s: truncated %v, want %v", label, found.Truncated, expected.Truncated)
	}
	got := make([]corpusSuggested, 0, len(found.Report))
	masked := make([]corpusSuggested, 0, len(found.Report))
	for index := range found.Report {
		report := &found.Report[index]
		param := report.Param
		if len(report.Candidates) > 0 {
			param = 0
			for _, candidate := range report.Candidates {
				switch {
				case candidate.Format == nil:
					param |= 1 << uint32(candidate.Order)
				case candidate.Format.DecimalSep == ".":
					param |= notationCommaGroups
				default:
					param |= notationCommaDecimal
				}
			}
		}
		want := expected.Columns[index]
		numeric := report.Status == SuggestResolved && report.Door.numeric()
		if numeric {
			// The plan is in the one notation the param left, and comma groups is also what's used
			// with no separator to prove it. The export's param is held to below.
			planned := commaGroups
			if want.Param == notationCommaDecimal {
				planned = commaDecimal
			}
			want.Param = 0
			if expected.Format != nil {
				planned = expected.Format.format()
			}
			if report.Format == nil || *report.Format != planned || found.Plan[index].Format() != planned {
				t.Errorf("%s: column %d is planned in %+v, reported %+v, want %+v", label, index, found.Plan[index].Format(), report.Format, planned)
			}
		} else if report.Format != nil {
			t.Errorf("%s: column %d, not a resolved number, reports the format %+v", label, index, *report.Format)
		}
		masked = append(masked, want)
		got = append(got, corpusSuggested{corpusDoors[report.Door], param, report.Status.String(), report.Matched, report.Sampled})
		if report.Ordinal != index {
			t.Errorf("%s: column %d reports ordinal %d", label, index, report.Ordinal)
		}
		if planned := found.Plan[index]; planned.Ordinal() != index ||
			(report.Status == SuggestResolved) != (planned.Door() == report.Door) && report.Door != DoorText {
			t.Errorf("%s: column %d plans %v from %+v", label, index, planned, *report)
		}
	}
	if !reflect.DeepEqual(got, masked) {
		t.Errorf("%s: columns %+v, want %+v", label, got, masked)
	}
}

// suggestOptions is what the corpus declares for a case's suggestion.
func (c *corpusSuggest) options() SuggestOptions {
	var options SuggestOptions
	if c.SampleRows != nil {
		options.SampleRows = *c.SampleRows
	}
	if c.Format != nil {
		format := c.Format.format()
		options.Format = &format
	}
	return options
}

// Every case that says what to suggest: from memory and from a file; through the export, over the
// cells the case's own rows say; and a plan that reads back without refusing what it resolved.
func TestEverySuggestionIsWhatTheCorpusSays(t *testing.T) {
	directory, files := tempDir(t)
	seen := 0
	for index, c := range delimitedCorpus(t) {
		if c.Suggest == nil {
			continue
		}
		seen++
		input, dialect, options := c.bytes(t), c.dialect(t), c.Suggest.options()
		found, err := SuggestDelimitedBytes(input, dialect, options)
		if err != nil {
			t.Errorf("%s: %v", c.Name, err)
			continue
		}
		assertSuggestion(t, c.Name+" (memory)", found, c.Suggest)
		if files {
			path := filepath.Join(directory, fmt.Sprintf("suggest-%d.txt", index))
			if err := os.WriteFile(path, input, 0o600); err != nil {
				t.Fatal(err)
			}
			fromFile, err := SuggestDelimited(path, dialect, options)
			if err != nil {
				t.Errorf("%s: %v", c.Name, err)
			} else {
				assertSuggestion(t, c.Name+" (file)", fromFile, c.Suggest)
			}
		}

		// The export, over the case's own rows: a cell with no text in the case is blank here, as a
		// read's empty verdict is.
		sample := len(c.Rows)
		if c.Suggest.SampleRows != nil {
			sample = *c.Suggest.SampleRows
		}
		var text []byte
		var cells []rawSpan
		for column := range c.Plan {
			for _, row := range c.Rows[:sample] {
				var cell []byte
				switch {
				case row[column].RawHex != nil:
					cell = unhex(t, *row[column].RawHex)
				case row[column].Text != nil:
					cell = []byte(*row[column].Text)
				}
				cells = append(cells, rawSpan{offset: uint32(len(text)), len: uint32(len(cell))})
				text = append(text, cell...)
			}
		}
		formats := make([]hypercast.RawNumFormat, len(c.Plan))
		if options.Format != nil {
			format, err := options.Format.Raw()
			if err != nil {
				t.Fatal(err)
			}
			for column := range formats {
				formats[column] = format
			}
		}
		direct, ok := nativeSuggest(text, cells, nil, sample, formats)
		if !ok {
			t.Fatalf("%s: no room", c.Name)
		}
		for column, want := range c.Suggest.Columns {
			got := direct[column]
			status := map[uint32]string{1: "resolved", 2: "ambiguous", 3: "unresolved"}[got.status]
			if corpusDoors[Door(got.door)] != want.Door || got.param != want.Param || status != want.Status ||
				int(got.matched) != want.Matched || int(got.sampled) != want.Sampled {
				t.Errorf("%s: the export says %+v of column %d, want %+v", c.Name, got, column, want)
			}
		}

		// What's resolved takes every cell it saw, and refuses none.
		reader, err := NewDelimitedReaderBytes(input, dialect, found.Plan)
		if err != nil {
			t.Errorf("%s: read back: %v", c.Name, err)
			continue
		}
		read := 0
		for read < sample {
			batch, err := reader.Read()
			if err != nil {
				t.Errorf("%s: read back: %v", c.Name, err)
				break
			}
			for problem := range batch.Faults() {
				report := &found.Report[problem.PlanColumn]
				if report.Status == SuggestResolved && report.Door != DoorText && problem.Row+read < sample {
					t.Errorf("%s: read back refused %+v", c.Name, problem)
				}
			}
			read += batch.Rows()
		}
		_ = reader.Close()
	}
	if seen < 12 {
		t.Errorf("%d cases say what to suggest", seen)
	}
}

// Every case the corpus has, whether it says what to suggest or not: a suggestion fails where a
// read of the case fails, and nowhere else.
func TestSuggestFailsOnlyWhereAReadOfTheCaseFails(t *testing.T) {
	planned, failed := 0, 0
	for _, c := range delimitedCorpus(t) {
		if _, err := SuggestDelimitedBytes(c.bytes(t), c.dialect(t), SuggestOptions{}); err == nil {
			planned++
		} else {
			failed++
			if c.Failure == nil {
				t.Errorf("%s: %v", c.Name, err)
			}
		}
	}
	if planned <= 80 || failed == 0 {
		t.Errorf("%d planned, %d failed", planned, failed)
	}
}

// shown is what a corpus field the case may leave out says: its value, or nil.
func shown[T any](field *T) any {
	if field == nil {
		return nil
	}
	return *field
}
