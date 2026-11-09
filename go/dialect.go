package hypertabular

import (
	"encoding/binary"
	"fmt"
	"math"
)

// ShortRows is what a record with fewer cells than the width is. The values are the core's codes.
type ShortRows uint32

const (
	// ShortRowsReject makes it a ColumnCount failure: the default.
	ShortRowsReject ShortRows = 0
	// ShortRowsPad makes it a row whose missing cells are Empty, at OriginPadded.
	ShortRowsPad ShortRows = 1
)

// LongRows is what a record with more cells than the width is. The values are the core's codes.
type LongRows uint32

const (
	// LongRowsReject makes it a ColumnCount failure: the default.
	LongRowsReject LongRows = 0
	// LongRowsReport cuts it to the width, and Batch.Faults lists it with the width it had.
	LongRowsReport LongRows = 1
	// LongRowsTruncate cuts it to the width, and nothing says so.
	LongRowsTruncate LongRows = 2
)

// TextEncoding is what a file's bytes are, declared and never sniffed.
//
// Anything but UTF8 is decoded ahead of the read, so that everything a batch says, its spans
// included, is about the decoded text. A unit that has no character is an Encoding *Failure, after
// every intact row before it, whose Byte alone counts the original bytes: nothing is ever replaced.
// A workbook declares its own encoding, so this is delimited text's alone. The values are the
// core's codes.
type TextEncoding uint32

const (
	// UTF8 is read without a copy: the default. A cell that isn't valid is that cell's Malformed
	// verdict.
	UTF8 TextEncoding = 0
	// Windows1252 is decoded as WHATWG has it: the five bytes the code page leaves undefined are
	// their C1 controls. EF BB BF is three characters here, not a byte-order mark.
	Windows1252 TextEncoding = 1
	// Latin1 is ISO-8859-1: every byte the code point of its value.
	Latin1 TextEncoding = 2
	// UTF16LE is UTF-16, little-endian, with or without a byte-order mark. A lone surrogate, or
	// half a code unit at the end of the input, is the failure.
	UTF16LE TextEncoding = 3
)

// Tokens is a list of what stands for no value: a cell whose exact, unquoted bytes are one of them
// is Empty, whatever its door, and Batch.Origin says OriginToken. A quoted cell never is one. The
// zero Tokens is none. Comparable: two lists are == when they hold the same tokens in the same order.
type Tokens struct {
	// packed is the list as the core reads it: each token a little-endian 32-bit length, then its
	// bytes.
	packed string
}

// NewTokens is the list of these tokens.
func NewTokens(tokens ...string) Tokens {
	var packed []byte
	for _, token := range tokens {
		// A token too long for the length word can't be in a cell, whose offsets are 31 bits.
		if uint64(len(token)) <= math.MaxUint32 {
			packed = binary.LittleEndian.AppendUint32(packed, uint32(len(token)))
			packed = append(packed, token...)
		}
	}
	return Tokens{packed: string(packed)}
}

// Dialect is how the text is delimited, declared by the caller. Nothing is sniffed: the
// separator is stated, quoting is stated, the header is stated — the same stance
// HyperCast's NumFormat takes for numeric notation.
//
// The zero Dialect is not a dialect (it has no separator). Start from CSV, TSV or PSV and
// change what differs. Build one with field names: a later release may add a field.
type Dialect struct {
	// Separator is the single-byte separator: tab, or any printable ASCII byte except '"'.
	Separator byte
	// Quoting says whether '"' quotes cells (RFC 4180, "" for a literal quote). Off, a quote
	// is an ordinary byte.
	Quoting bool
	// HasHeader says whether the first record is a header, exposed through
	// DelimitedReader.Header and never delivered as a row.
	HasHeader bool
	// SkipBlankLines says whether a completely empty line is skipped rather than read as a
	// one-cell row.
	SkipBlankLines bool
	// Skip is the physical lines to pass over after a byte-order mark, before anything is read of
	// them: a quote in one means nothing.
	Skip uint32
	// HeaderRow is the records to discard after those, ahead of the header if there is one. A
	// blank line this dialect skips isn't a record.
	HeaderRow uint32
	// ShortRows is what a record with fewer cells than the width is.
	ShortRows ShortRows
	// LongRows is what a record with more cells than the width is.
	LongRows LongRows
	// Encoding is what the input's bytes are.
	Encoding TextEncoding
	// Tokens is what stands for no value in every column without tokens of its own
	// (Column.WithTokens).
	Tokens Tokens
}

// The declared dialects most files are written in.
var (
	// CSV is comma-separated, quoted, with a header, blank lines skipped.
	CSV = Dialect{Separator: ',', Quoting: true, HasHeader: true, SkipBlankLines: true}
	// TSV is tab-separated, otherwise as CSV.
	TSV = Dialect{Separator: '\t', Quoting: true, HasHeader: true, SkipBlankLines: true}
	// PSV is pipe-separated, otherwise as CSV.
	PSV = Dialect{Separator: '|', Quoting: true, HasHeader: true, SkipBlankLines: true}
)

// check refuses what the core would: a policy or an encoding that isn't one. The separator is the
// core's to judge, when the state is made.
func (d Dialect) check() error {
	switch {
	case d.ShortRows > ShortRowsPad:
		return fmt.Errorf("hypertabular: undefined ShortRows %d", uint32(d.ShortRows))
	case d.LongRows > LongRowsTruncate:
		return fmt.Errorf("hypertabular: undefined LongRows %d", uint32(d.LongRows))
	case d.Encoding > UTF16LE:
		return fmt.Errorf("hypertabular: undefined TextEncoding %d", uint32(d.Encoding))
	}
	return nil
}
