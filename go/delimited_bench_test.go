package hypertabular

// The delimited reader over 300 000 generated rows of the workbook benchmark's eight columns
// (i64, f64, text, date, time, bool, duration, i64), as CSV, read whole through the same plan:
// Memory is NewDelimitedReaderBytes over the text, Stream is NewDelimitedReader over a
// bytes.Reader with the default buffer; each reads in batches of 4096, looks at every
// column's verdicts and every text cell's bytes, and reports the checksum that says the two
// did the same work. The text is made in memory, so nothing is read from disk.
// Run: go test -run '^$' -bench Delimited -benchmem

import (
	"bytes"
	"fmt"
	"io"
	"sync"
	"testing"
)

var delimitedBenchText = sync.OnceValue(func() []byte {
	var text bytes.Buffer
	text.WriteString("id,amount,name,day,at,flag,elapsed,ref\n")
	for row := range 300_000 {
		fmt.Fprintf(&text, "%d,%d.%02d,name %d,2024-%02d-%02d,%02d:%02d:%02d,%t,PT%dH%dM,%d\n",
			row, row%100_000, row%100, row, row%12+1, row%28+1, row%24, row%60, (row*7)%60, row%3 == 0, row%48, row%60, row*31)
	}
	return text.Bytes()
})

func delimitedBenchRead(b *testing.B, r *DelimitedReader) int {
	checksum := 0
	for {
		batch, err := r.Read()
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
			checksum += len(text)
		}
	}
}

func BenchmarkDelimited(b *testing.B) {
	text := delimitedBenchText()
	sources := []struct {
		name string
		open func() (*DelimitedReader, error)
	}{
		{"Memory", func() (*DelimitedReader, error) { return NewDelimitedReaderBytes(text, CSV, benchPlan) }},
		{"Stream", func() (*DelimitedReader, error) { return NewDelimitedReader(bytes.NewReader(text), CSV, benchPlan) }},
	}
	for _, source := range sources {
		b.Run(source.name, func(b *testing.B) {
			b.SetBytes(int64(len(text)))
			checksum := 0
			for b.Loop() {
				r, err := source.open()
				if err != nil {
					b.Fatal(err)
				}
				checksum = delimitedBenchRead(b, r)
			}
			b.Logf("checksum %d", checksum)
		})
	}
}
