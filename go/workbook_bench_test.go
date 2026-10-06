package hypertabular

// The workbook reader over the two real-application files of corpus/README.md — Excel's
// excel-win-300k.xlsx and LibreOffice's libreoffice-300k.ods, 300 000 rows × 8 columns each —
// read whole through the plan rust/benches/workbook_benchmarks.rs reads them through, so
// that this binding's number sits beside the core's: Open is the package opened and nothing
// read; Read is opened, then the first sheet read in batches of 4096, every column's
// verdicts looked at and every text cell's bytes — the checksum every binding's benchmark
// arrives at, which says they all did the same work. ReadStrings is Read with the text
// column made into strings.
//
// The files stay out of the tree (corpus/generate/out/, sha256 in the README's table);
// HYPERTABULAR_BENCH_DIR names another directory holding them. A file that is not there is
// skipped.
// Run: go test -run '^$' -bench Workbook -benchmem

import (
	"io"
	"os"
	"path/filepath"
	"testing"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

var benchPlan = []Column{
	I64(0, hypercast.Invariant),
	F64(1, hypercast.Invariant),
	Text(2),
	DateOnly(3),
	TimeOfDay(4),
	Bool(5),
	Span(6),
	I64(7, hypercast.Invariant),
}

// benchText keeps the compiler from seeing that a string made from a cell is never used.
var benchText string

var benchFiles = []string{"excel-win-300k.xlsx", "libreoffice-300k.ods"}

func benchContainer(b *testing.B, name string) []byte {
	b.Helper()
	directory := os.Getenv("HYPERTABULAR_BENCH_DIR")
	if directory == "" {
		directory = filepath.Join("..", "corpus", "generate", "out")
	}
	container, err := os.ReadFile(filepath.Join(directory, name))
	if err != nil {
		b.Skipf("%s: %v", name, err)
	}
	return container
}

// benchRead reads the first sheet whole; strings says whether the text column is made into
// strings or looked at as the bytes it is.
func benchRead(b *testing.B, container []byte, strings bool) int {
	book, err := NewWorkbook(container)
	if err != nil {
		b.Fatal(err)
	}
	sheet, err := book.Sheet(0, DefaultSheetOptions, benchPlan)
	if err != nil {
		b.Fatal(err)
	}
	checksum := 0
	for {
		batch, err := sheet.Read()
		if err == io.EOF {
			return checksum
		}
		if err != nil {
			b.Fatal(err)
		}
		for column := range benchPlan {
			for _, verdict := range batch.Verdicts(column) {
				if verdict.OK() {
					checksum++
				}
			}
		}
		for _, text := range batch.Text(2) {
			if strings {
				benchText = string(text)
				checksum += len(benchText)
			} else {
				checksum += len(text)
			}
		}
	}
}

func BenchmarkWorkbook(b *testing.B) {
	for _, name := range benchFiles {
		b.Run(name, func(b *testing.B) {
			container := benchContainer(b, name)
			b.Logf("checksum %d", benchRead(b, container, false))
			b.Run("Open", func(b *testing.B) {
				b.SetBytes(int64(len(container)))
				for b.Loop() {
					book, err := NewWorkbook(container)
					if err != nil {
						b.Fatal(err)
					}
					_ = book.Sheets()
				}
			})
			b.Run("Read", func(b *testing.B) {
				b.SetBytes(int64(len(container)))
				for b.Loop() {
					benchRead(b, container, false)
				}
			})
			b.Run("ReadStrings", func(b *testing.B) {
				b.SetBytes(int64(len(container)))
				for b.Loop() {
					benchRead(b, container, true)
				}
			})
		})
	}
}
