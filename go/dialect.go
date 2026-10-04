package hypertabular

// Dialect is how the text is delimited, declared by the caller. Nothing is sniffed: the
// separator is stated, quoting is stated, the header is stated — the same stance
// HyperCast's NumFormat takes for numeric notation.
//
// The zero Dialect is not a dialect (it has no separator). Start from CSV, TSV or PSV and
// change what differs.
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
