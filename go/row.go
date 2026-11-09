package hypertabular

import (
	"io"
	"iter"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// TextString is the Text cell at (column, row) as a string — without a copy. The string is
// a view of the same bytes Text hands out, made with unsafe.String: nothing is allocated,
// which is what lets a cell go straight to strconv, a map lookup or a switch with no garbage
// per cell. The empty string is a cell with no bytes, which is the one way text fails.
//
// The bytes behind the string belong to the reader, not to the string: they are reused by
// the next Read, and a string kept past it says whatever the reader has put there since —
// which a Go string never otherwise does. Use it within the batch, and copy what is kept
// (strings.Clone, or Get[string], which allocates its own). Under NewDelimitedReaderBytes the
// bytes are the caller's own, and must not change while the string is in use.
//
// Asking a column whose door is not DoorText panics, as Text does.
func (b *Batch) TextString(column, row int) string {
	text := b.Text(column)[row]
	return unsafe.String(unsafe.SliceData(text), len(text))
}

// Row is one row of a Batch: the batch read across rather than down, for code that builds a
// value from each record's columns. It is a (batch, index) pair, so it costs nothing to make
// or to copy, and — like everything a batch hands out — it is valid until the reader's next
// Read; do not keep it past that.
type Row struct {
	batch *Batch
	index int
}

// Row is the row at index of the batch in hand. An index outside the batch panics.
func (b *Batch) Row(index int) Row {
	b.checkRow(index)
	return Row{batch: b, index: index}
}

// All is the batch's rows, in order:
//
//	for row := range batch.All() { … }
//
// The rows are the batch's in hand when All is called. Ranged over after the reader has read
// again or been closed — a Read or a Close in the loop's body included — it panics, as Faults
// does: it can't answer for the next batch.
func (b *Batch) All() iter.Seq[Row] {
	generation := b.generation
	return func(yield func(Row) bool) {
		if b.generation != generation {
			panic(staleRows)
		}
		for index := 0; index < b.rows; index++ {
			if !yield(Row{batch: b, index: index}) {
				return
			}
			if b.generation != generation {
				panic(staleRows)
			}
		}
	}
}

// staleRows is what a row iterator says when it's ranged over past its batch.
const staleRows = "hypertabular: the batch these rows belong to is over: the reader has read on, or was closed"

// Batch is the batch the row is in.
func (r Row) Batch() *Batch { return r.batch }

// Index is the row's place in its batch, from zero.
func (r Row) Index() int { return r.index }

// Line is where the row came from, as Batch.Line says: for delimited text the 1-based line
// its record starts on, for a sheet its 1-based row number.
func (r Row) Line() int { return r.batch.Line(r.index) }

// Verdict is the verdict of the row's cell in column: Batch.Verdicts(column)[r.Index()].
func (r Row) Verdict(column int) CellVerdict { return r.batch.Verdicts(column)[r.index] }

// Fault is the verdict of the row's cell in column as HyperCast's own union, as Batch.Fault
// says.
func (r Row) Fault(column int) *hypercast.Fault { return r.batch.Fault(column, r.index) }

// Detail is what failed in the row's cell in column, as Batch.Detail says.
func (r Row) Detail(column int) hypercast.Detail { return r.batch.Detail(column, r.index) }

// Origin is where the verdict of the row's cell in column came from, as Batch.Origin says.
func (r Row) Origin(column int) Origin { return r.batch.Origin(column, r.index) }

// Text is the row's cell in a Text column, as Batch.Text says: nil for a cell with no bytes.
func (r Row) Text(column int) []byte { return r.batch.Text(column)[r.index] }

// TextString is the row's cell in a Text column as a string with no copy, as
// Batch.TextString says — valid only until the reader's next Read.
func (r Row) TextString(column int) string { return r.batch.TextString(column, r.index) }

// Raw is the text the row's cell in column was cast from, as Batch.Raw says.
func (r Row) Raw(column int) []byte { return r.batch.Raw(column, r.index) }

// Cell is the row's cell in column as HyperCast judged it, as Get says of the same cell:
// Get(row.Batch(), column, row.Index()).
//
//	id, fault := hypertabular.Cell[int32](row, 0)
//
// Cell is a function rather than a method because Go methods cannot take type parameters.
func Cell[T Value](row Row, column int) (T, *hypercast.Fault) {
	return Get[T](row.batch, column, row.index)
}

// reading is what DelimitedReader and Sheet have in common for rows: a Read.
type reading interface {
	Read() (*Batch, error)
}

// rows reads batch after batch and yields their rows, then the error the reading ended on
// unless that is io.EOF.
func rows(reader reading) iter.Seq2[Row, error] {
	return func(yield func(Row, error) bool) {
		for {
			batch, err := reader.Read()
			if err == io.EOF {
				return
			}
			if err != nil {
				yield(Row{}, err)
				return
			}
			generation := batch.generation
			for index := 0; index < batch.rows; index++ {
				if !yield(Row{batch: batch, index: index}, nil) {
					return
				}
				// A Read or a Close in the loop's body has moved the reader: what is left of this
				// batch is gone, and the next is the body's, so neither is yielded.
				if batch.generation != generation {
					panic(staleRows)
				}
			}
		}
	}
}

// All reads the rest of the input, batch by batch, and yields every row of it in order —
// each with a nil error — and then, if the input ended on anything but io.EOF, a zero Row
// and that error:
//
//	for row, err := range reader.All() {
//		if err != nil {
//			return err
//		}
//		…
//	}
//
// Each row is valid for its own iteration only: the batch it is in is read over when the
// next is. Breaking out of the loop stops the reading there; a later All goes on with the
// next batch, so what was left of the batch in hand is not yielded again. A Read or a Close in
// the loop's body panics the loop, as Batch.All says: the reader is All's to move while it ranges.
func (r *DelimitedReader) All() iter.Seq2[Row, error] { return rows(r) }

// All reads the rest of the sheet, batch by batch, and yields every row of it in order, as
// DelimitedReader.All does.
func (s *Sheet) All() iter.Seq2[Row, error] { return rows(s) }
