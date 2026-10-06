package hypertabular

// Replays corpus/workbook.json — the contract every binding replays, and the one the Rust
// binding replays — through this binding: each package opened from memory and from its path,
// each sheet read by index and (where the name finds it) by name, in batches of one row, of
// two, and of more than any sheet has.

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"testing"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

type workbookCase struct {
	Name   string `json:"name"`
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
		HasHeader     bool `json:"has_header"`
		SkipEmptyRows bool `json:"skip_empty_rows"`
	} `json:"options"`
	Plan    []corpusColumn `json:"plan"`
	Header  *[]string      `json:"header"`
	Rows    [][]corpusCell `json:"rows"`
	Numbers []int          `json:"numbers"`
	Failure *corpusFailure `json:"failure"`
}

func workbookCorpus(t *testing.T) []workbookCase {
	t.Helper()
	path := repositoryFile(t, "corpus", "workbook.json")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var cases []workbookCase
	if err := json.Unmarshal(data, &cases); err != nil {
		t.Fatalf("parsing %s: %v", path, err)
	}
	return cases
}

func TestWorkbookCorpus(t *testing.T) {
	replayWorkbookCorpus(t)
}

// The corpus again with every buffer starting at one element and never asked to be large up
// front: each read stops wherever the core runs out of room and resumes in a grown buffer
// that has to have kept what the old one held. A grow that dropped it would read garbage,
// and the corpus would say so.
func TestWorkbookCorpusWithBuffersThatStartWithNoRoom(t *testing.T) {
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
			if want := map[string]WorkbookFormat{"xlsx": XLSX, "ods": ODS}[c.Format]; book.Format() != want {
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
				options := SheetOptions{HasHeader: c.Options.HasHeader, SkipEmptyRows: c.Options.SkipEmptyRows, BatchRows: batchRows}
				for _, named := range []bool{false, true} {
					if named && !byName {
						continue
					}
					label := fmt.Sprintf("%s: %s, %d rows a batch", c.Name, opening.source, batchRows)
					var sheet *Sheet
					if named {
						label += ", by name"
						sheet, err = book.SheetNamed(sheetName, options, plan)
					} else {
						sheet, err = book.Sheet(*c.Sheet, options, plan)
					}
					if err != nil {
						t.Fatalf("%s: %v", label, err)
					}
					replaySheet(t, label, c, sheet, batchRows)
				}
			}
		}
	}
	if cells < 12_000 {
		t.Errorf("the corpus reaches %d cells; expected at least 12,000", cells)
	}
}

// replaySheet reads one sheet and holds it to the corpus: the header, every row's number and
// cells, and how the sheet ended.
func replaySheet(t *testing.T, label string, c *workbookCase, sheet *Sheet, batchRows int) {
	t.Helper()
	switch header := sheet.Header(); {
	case c.Header == nil:
		if header != nil {
			t.Errorf("%s: header %q, want none", label, header)
		}
	case header == nil || !reflect.DeepEqual(header, *c.Header):
		t.Errorf("%s: header %q, want %q", label, header, *c.Header)
	}

	seen := 0
	var ended error
	for {
		b, err := sheet.Read()
		if err != nil {
			ended = err
			break
		}
		if b.Rows() < 1 || b.Rows() > batchRows {
			t.Fatalf("%s: a batch of %d rows", label, b.Rows())
		}
		for row := 0; row < b.Rows(); row, seen = row+1, seen+1 {
			if seen >= len(c.Rows) {
				t.Fatalf("%s: more than the %d rows expected", label, len(c.Rows))
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
	if _, err := book.Sheet(0, SheetOptions{}, plan); err == nil {
		t.Error("no rows a batch: no error")
	}
	if _, err := OpenWorkbook(filepath.Join(t.TempDir(), "missing.xlsx")); !errors.Is(err, os.ErrNotExist) {
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
	if got := fmt.Sprint(XLSX, ODS, WorkbookFormat(0)); got != "XLSX ODS WorkbookFormat(0)" {
		t.Errorf("formats: %s", got)
	}
}
