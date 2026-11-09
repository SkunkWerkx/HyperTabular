package hypertabular

// Replays corpus/workbook.json — the contract every binding replays, and the one the Rust
// binding replays — through this binding: each package opened from memory, from its path and
// from an io.Reader, each sheet read by index and (where the name finds it) by name, with
// the plan given and with it bound after the header, in batches of one row, of two, and of
// more than any sheet has, and with the notes off. A key this file doesn't know fails the
// decode; the one the corpus keeps for its reader is named.

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"testing/iotest"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

type workbookCase struct {
	Name string `json:"name"`
	// Note is a sentence for whoever reads the file: no replay reads it.
	Note   string `json:"note"`
	File   string `json:"file"`
	Format string `json:"format"`
	Epoch  int    `json:"epoch"`
	Sheets []struct {
		Name   string `json:"name"`
		Hidden bool   `json:"hidden"`
	} `json:"sheets"`
	// Absent for a package the core refuses to open.
	Sheet   *int `json:"sheet"`
	Options struct {
		HasHeader     bool     `json:"has_header"`
		SkipEmptyRows bool     `json:"skip_empty_rows"`
		Skip          uint32   `json:"skip"`
		HeaderRow     uint32   `json:"header_row"`
		Range         string   `json:"range"`
		Tokens        []string `json:"tokens"`
	} `json:"options"`
	Plan     []corpusColumn  `json:"plan"`
	Header   *[]string       `json:"header"`
	Rows     [][]corpusCell  `json:"rows"`
	Numbers  []int           `json:"numbers"`
	Suggest  *corpusSuggest  `json:"suggest"`
	Problems []corpusProblem `json:"problems"`
	Failure  *corpusFailure  `json:"failure"`
}

// options is the case's sheet options as this binding declares them.
func (c *workbookCase) options(t *testing.T, batchRows int) SheetOptions {
	t.Helper()
	options := SheetOptions{
		HasHeader:     c.Options.HasHeader,
		SkipEmptyRows: c.Options.SkipEmptyRows,
		BatchRows:     batchRows,
		Skip:          c.Options.Skip,
		HeaderRow:     c.Options.HeaderRow,
		Tokens:        NewTokens(c.Options.Tokens...),
	}
	if c.Options.Range != "" {
		parsed, err := ParseRange(c.Options.Range)
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		options.Range = parsed
	}
	return options
}

func workbookCorpus(t *testing.T) []workbookCase {
	t.Helper()
	path := repositoryFile(t, "corpus", "workbook.json")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var cases []workbookCase
	if err := decodeStrictly(data, &cases); err != nil {
		t.Fatalf("parsing %s: %v", path, err)
	}
	return cases
}

func TestEveryWorkbookCorpusKeyIsRead(t *testing.T) {
	junkEverywhere(t, repositoryFile(t, "corpus", "workbook.json"), func(document []byte) error {
		var cases []workbookCase
		return decodeStrictly(document, &cases)
	})
}

func TestCorpusWorkbook(t *testing.T) {
	replayWorkbookCorpus(t)
}

// Every case that says what to suggest: through this package, and a plan that then reads back
// refusing nothing it resolved. A typed number has no notation, and a text cell asks what text asks.
func TestEveryWorkbookSuggestionIsWhatTheCorpusSays(t *testing.T) {
	seen := 0
	cases := workbookCorpus(t)
	for index := range cases {
		c := &cases[index]
		if c.Suggest == nil {
			continue
		}
		seen++
		book, err := OpenWorkbook(repositoryFile(t, "corpus", filepath.FromSlash(c.File)))
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		options := c.options(t, 1024)
		sheet, err := book.SheetUnbound(*c.Sheet, options)
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		found, err := sheet.Suggest(c.Suggest.options())
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		assertSuggestion(t, c.Name, found, c.Suggest)
		read, err := book.Sheet(*c.Sheet, options, found.Plan)
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		for {
			batch, err := read.Read()
			if err == io.EOF {
				break
			}
			if err != nil {
				t.Fatalf("%s: %v", c.Name, err)
			}
			for problem := range batch.Faults() {
				if problem.Reason != hypercast.Empty {
					t.Errorf("%s: the plan refused %+v", c.Name, problem)
				}
			}
		}
	}
	if seen != 15 {
		t.Errorf("%d cases say what to suggest", seen)
	}
}

// The corpus again with every buffer starting at one element and never asked to be large up
// front: each read stops wherever the core runs out of room and resumes in a grown buffer
// that has to have kept what the old one held. A grow that dropped it would read garbage,
// and the corpus would say so.
func TestCorpusWorkbookWithBuffersThatStartWithNoRoom(t *testing.T) {
	if raceDetector {
		t.Skip("one goroutine, and three minutes under the race detector: the plain run holds it")
	}
	stingy, grown = true, [3]int{}
	defer func() { stingy = false }()
	replayWorkbookCorpus(t)
	// Not once per call: many times, mid-part, for each of the three.
	for _, times := range grown {
		if times <= 1000 {
			t.Errorf("grown %v times (window, arena, cells)", grown)
		}
	}
}

func replayWorkbookCorpus(t *testing.T) {
	t.Helper()
	corpus := workbookCorpus(t)
	if len(corpus) < 80 {
		t.Fatalf("the corpus has %d cases; expected at least 80", len(corpus))
	}
	directory := filepath.Dir(repositoryFile(t, "corpus", "workbook.json"))
	cells := 0
	for index := range corpus {
		c := &corpus[index]
		path := filepath.Join(directory, filepath.FromSlash(c.File))
		container, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		openings := []struct {
			source string
			open   func() (*Workbook, error)
		}{
			{"memory", func() (*Workbook, error) { return NewWorkbook(container) }},
			{"path", func() (*Workbook, error) { return OpenWorkbook(path) }},
			// A reader that hands over a few bytes at a time, as a network body would.
			{"a reader", func() (*Workbook, error) {
				return NewWorkbookReader(iotest.HalfReader(bytes.NewReader(container)))
			}},
		}

		// A package the core refuses: every way of opening it gives the one failure.
		if c.Sheet == nil {
			for _, opening := range openings {
				book, err := opening.open()
				if book != nil {
					t.Errorf("%s (%s): opened", c.Name, opening.source)
				}
				assertFailure(t, fmt.Sprintf("%s (%s)", c.Name, opening.source), err, c.Failure)
			}
			continue
		}

		plan := make([]Column, len(c.Plan))
		for index := range c.Plan {
			plan[index] = c.Plan[index].column(t)
		}
		sheetName := c.Sheets[*c.Sheet].Name
		cells += len(c.Rows) * len(plan)

		for _, opening := range openings {
			book, err := opening.open()
			if err != nil {
				t.Fatalf("%s (%s): %v", c.Name, opening.source, err)
			}
			if want := map[string]WorkbookFormat{"xlsx": XLSX, "ods": ODS, "numbers": Numbers}[c.Format]; book.Format() != want {
				t.Errorf("%s: format %v, want %v", c.Name, book.Format(), want)
			}
			if book.DateSystem() != hypercast.ExcelEpoch(c.Epoch) {
				t.Errorf("%s: date system %v, want %d", c.Name, book.DateSystem(), c.Epoch)
			}
			listed := book.Sheets()
			byName := false
			if len(listed) != len(c.Sheets) {
				t.Errorf("%s: %d sheets, want %d", c.Name, len(listed), len(c.Sheets))
			}
			for index := range min(len(listed), len(c.Sheets)) {
				if listed[index].Name != c.Sheets[index].Name || listed[index].Hidden != c.Sheets[index].Hidden {
					t.Errorf("%s: sheet %d is %q (hidden %v), want %q (hidden %v)", c.Name, index,
						listed[index].Name, listed[index].Hidden, c.Sheets[index].Name, c.Sheets[index].Hidden)
				}
			}
			for index := range listed {
				if listed[index].Name == sheetName {
					byName = index == *c.Sheet
					break
				}
			}

			for _, batchRows := range []int{1, 2, 1024} {
				options := c.options(t, batchRows)
				// Each of those with the plan given, and with it bound after the header.
				for _, way := range []struct{ named, unbound bool }{{false, false}, {true, false}, {false, true}, {true, true}} {
					if way.named && !byName {
						continue
					}
					label := fmt.Sprintf("%s: %s, %d rows a batch", c.Name, opening.source, batchRows)
					var sheet *Sheet
					switch {
					case way.named && way.unbound:
						label += ", by name, bound after the header"
						sheet, err = book.SheetNamedUnbound(sheetName, options)
					case way.unbound:
						label += ", bound after the header"
						sheet, err = book.SheetUnbound(*c.Sheet, options)
					case way.named:
						label += ", by name"
						sheet, err = book.SheetNamed(sheetName, options, plan)
					default:
						sheet, err = book.Sheet(*c.Sheet, options, plan)
					}
					if err != nil {
						t.Fatalf("%s: %v", label, err)
					}
					if way.unbound {
						bindSheet(t, label, sheet, plan)
					}
					replaySheet(t, label, c, sheet, batchRows)
				}
				// The notes off: the same rows, and nothing said of why.
				label := fmt.Sprintf("%s: %s, %d rows a batch, notes off", c.Name, opening.source, batchRows)
				sheet, err := book.Sheet(*c.Sheet, options, plan, Notes(false))
				if err != nil {
					t.Fatalf("%s: %v", label, err)
				}
				replaySheet(t, label, c, sheet, batchRows)
			}
		}
	}
	if cells < 12_000 {
		t.Errorf("the corpus reaches %d cells; expected at least 12,000", cells)
	}
}

// bindSheet binds a plan to a sheet opened without one, holding it to what an unbound sheet
// does first: no plan, ErrUnbound from Read, and ErrAlreadyBound from a second Bind.
func bindSheet(t *testing.T, label string, sheet *Sheet, plan []Column) {
	t.Helper()
	if sheet.IsBound() || sheet.Plan() != nil {
		t.Errorf("%s: a sheet opened without a plan says it has one", label)
	}
	if _, err := sheet.Read(); err != ErrUnbound {
		t.Errorf("%s: Read before Bind: %v", label, err)
	}
	if err := sheet.Bind(plan); err != nil {
		t.Fatalf("%s: Bind: %v", label, err)
	}
	if err := sheet.Bind(plan); err != ErrAlreadyBound || !sheet.IsBound() || len(sheet.Plan()) != len(plan) {
		t.Errorf("%s: a second Bind: %v", label, err)
	}
}

// replaySheet reads one sheet and holds it to the corpus: the header, every row's number and
// cells, and how the sheet ended.
func replaySheet(t *testing.T, label string, c *workbookCase, sheet *Sheet, batchRows int) {
	t.Helper()
	boundHeap()
	switch header := sheet.Header(); {
	case c.Header == nil:
		if header != nil {
			t.Errorf("%s: header %q, want none", label, header)
		}
	case header == nil || !reflect.DeepEqual([]string(header), *c.Header):
		t.Errorf("%s: header %q, want %q", label, header, *c.Header)
	default:
		assertLookups(t, label, header)
	}

	seen := 0
	var ended error
	var problems []corpusProblem
	for {
		b, err := sheet.Read()
		if err != nil {
			ended = err
			break
		}
		if b.Rows() < 1 || b.Rows() > batchRows {
			t.Fatalf("%s: a batch of %d rows", label, b.Rows())
		}
		problems = appendProblems(t, label, problems, b, seen)
		for row := 0; row < b.Rows(); row, seen = row+1, seen+1 {
			if seen >= len(c.Rows) {
				t.Fatalf("%s: more than the %d rows expected", label, len(c.Rows))
			}
			if len(c.Rows[seen]) != len(c.Plan) {
				t.Fatalf("%s, row %d: %d cells in the corpus for a plan of %d", label, seen, len(c.Rows[seen]), len(c.Plan))
			}
			if b.Line(row) != c.Numbers[seen] {
				t.Errorf("%s, row %d: row number %d, want %d", label, seen, b.Line(row), c.Numbers[seen])
			}
			for column := range c.Plan {
				assertCell(t, fmt.Sprintf("%s, row %d, column %d", label, seen, column), b, column, row, &c.Rows[seen][column])
			}
		}
	}
	if seen != len(c.Rows) {
		t.Errorf("%s: %d rows, want %d", label, seen, len(c.Rows))
	}
	assertProblems(t, label, problems, c.Problems, sheet.notes)
	assertFailure(t, label, ended, c.Failure)
	// A sheet that has ended stays ended: the same error, again.
	if _, again := sheet.Read(); again != ended {
		t.Errorf("%s: a second Read after the end returned %v, want the same %v", label, again, ended)
	}
}

func TestWhatIsNotThereIsAnErrorOfItsOwn(t *testing.T) {
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	plan := []Column{Text(0)}
	if _, err := book.SheetNamed("No such sheet", DefaultSheetOptions, plan); !errors.Is(err, ErrNoSheet) {
		t.Errorf("a name the workbook lacks: %v", err)
	}
	for _, index := range []int{-1, 99} {
		if _, err := book.Sheet(index, DefaultSheetOptions, plan); !errors.Is(err, ErrNoSheet) {
			t.Errorf("sheet %d: %v", index, err)
		}
	}
	if _, err := book.Sheet(0, DefaultSheetOptions, plan, BatchRows(0)); err == nil {
		t.Error("no rows a batch: no error")
	}
	if _, err := OpenWorkbook(missingPath(t, "missing.xlsx")); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("a missing file: %v", err)
	}
	var failure *Failure
	if _, err := NewWorkbook([]byte("not a zip at all")); !errors.As(err, &failure) || failure.Kind != NotAZip {
		t.Errorf("not a zip: %v", err)
	}

	// Two sheets read at once, each with its own buffers, over one workbook.
	first, err := book.Sheet(0, DefaultSheetOptions, plan)
	if err != nil {
		t.Fatal(err)
	}
	headless := DefaultSheetOptions
	headless.HasHeader = false
	second, err := book.Sheet(0, headless, plan)
	if err != nil {
		t.Fatal(err)
	}
	a, err := first.Read()
	if err != nil {
		t.Fatal(err)
	}
	rowsA := a.Rows()
	b, err := second.Read()
	if err != nil {
		t.Fatal(err)
	}
	if b.Rows() != rowsA+1 {
		t.Errorf("%d rows with the header read as one, %d without", b.Rows(), rowsA)
	}
	if _, err := first.Read(); err != io.EOF {
		t.Errorf("the end of the sheet: %v", err)
	}
	if first.Header() == nil || second.Header() != nil || len(first.Plan()) != 1 || first.Options() != DefaultSheetOptions {
		t.Errorf("header %q and %q, plan %v, options %+v", first.Header(), second.Header(), first.Plan(), first.Options())
	}
	if got := fmt.Sprint(XLSX, ODS, Numbers, WorkbookFormat(0)); got != "XLSX ODS Numbers WorkbookFormat(0)" {
		t.Errorf("formats: %s", got)
	}
}
