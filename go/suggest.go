package hypertabular

import (
	"errors"
	"fmt"
	"io"
	"math"
	"math/bits"
	"sort"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// defaultSampleRows is the rows a sample holds unless told otherwise.
const defaultSampleRows = 1000

// The notation hypertabular_suggest's param names by number: bit 1 is a format with , grouping
// digits in threes, bit 2 one with , as the decimal separator. Each is Western. A resolved numeric
// column with no format declared is planned in the second where param names it, and otherwise in
// the first, whether or not the sample held a ,.
const (
	notationCommaGroups  = 1 << 1
	notationCommaDecimal = 1 << 2
)

var (
	commaGroups  = hypercast.NumFormat{DecimalSep: ".", GroupSep: ",", Styles: hypercast.AllStyles}
	commaDecimal = hypercast.NumFormat{DecimalSep: ",", GroupSep: ".", Styles: hypercast.AllStyles}
)

// SuggestOptions is how a sample is taken and read. The zero SuggestOptions takes 1,000 rows with no
// notation declared.
type SuggestOptions struct {
	// SampleRows is the most rows the sample holds. 0 is not set, so 1,000, where Rust's
	// sample_rows(0) is 1: a Go struct can't tell a field left out from an explicit 0, and options
	// that declare only a Format mustn't sample one row. 1 asks for one row, and fewer than 0 is an
	// error. It stops at DefaultBatchRows of delimited text, and at a sheet's own BatchRows, read in
	// as many batches as the core says fit: a sample is never refused for the size of its batch.
	SampleRows int
	// Format reads every column's numbers in this notation, which settles what , means in them; nil
	// declares none.
	Format *hypercast.NumFormat
	// ColumnFormats reads the numbers of a source column, by ordinal, in a notation of its own,
	// ahead of Format.
	ColumnFormats map[int]hypercast.NumFormat
	// DateOrder reads every column's dates in this order where the sample leaves the order open, as
	// 01/02/2026 does; nil declares none. A column whose cells rule that order out keeps the order
	// they leave: it answers what the sample can't, and doesn't overrule what it can.
	DateOrder *hypercast.DateOrder
	// ColumnDateOrders reads the dates of a source column, by ordinal, in an order of its own, ahead
	// of DateOrder, and as that does.
	ColumnDateOrders map[int]hypercast.DateOrder
}

// declaredOrder is the order declared for source column ordinal, or nil.
func (o *SuggestOptions) declaredOrder(ordinal int) *hypercast.DateOrder {
	if order, ok := o.ColumnDateOrders[ordinal]; ok {
		return &order
	}
	return o.DateOrder
}

// declared is the format declared for source column ordinal, or nil.
func (o *SuggestOptions) declared(ordinal int) *hypercast.NumFormat {
	if format, ok := o.ColumnFormats[ordinal]; ok {
		return &format
	}
	return o.Format
}

// checkFormats judges every format these options declare as a plan's would be, whichever would win
// for a column and whether or not the sample reaches its ordinal. A suggest asks this before it opens
// or reads anything, so a stale format is refused on an empty source and on a broken one.
func (o *SuggestOptions) checkFormats() error {
	if o.Format != nil {
		if _, err := o.Format.Raw(); err != nil {
			return fmt.Errorf("hypertabular: the format declared for every column: %w", err)
		}
	}
	ordinals := make([]int, 0, len(o.ColumnFormats))
	for ordinal := range o.ColumnFormats {
		ordinals = append(ordinals, ordinal)
	}
	sort.Ints(ordinals)
	for _, ordinal := range ordinals {
		format := o.ColumnFormats[ordinal]
		if _, err := format.Raw(); err != nil {
			return fmt.Errorf("hypertabular: the format declared for column %d: %w", ordinal, err)
		}
	}
	return nil
}

// rowsWithin is how many rows to take, where a batch is batchRows.
func (o *SuggestOptions) rowsWithin(batchRows int) (int, error) {
	if o.SampleRows < 0 {
		return 0, fmt.Errorf("hypertabular: SampleRows must not be negative, not %d", o.SampleRows)
	}
	rows := o.SampleRows
	if rows == 0 {
		rows = defaultSampleRows
	}
	return min(rows, max(batchRows, 1)), nil
}

// SuggestStatus is how far a sample settles a column. The values are the core's codes. A later
// release may add one, so a switch over statuses needs a default arm.
type SuggestStatus uint32

const (
	// SuggestResolved says one door and one declaration take every cell, and the plan reads the
	// column through them.
	SuggestResolved SuggestStatus = 1
	// SuggestAmbiguous says every cell is read, but the sample can't say which declaration it's
	// written in: SuggestedColumn.Candidates are the choices. The plan reads the column as text
	// until the caller chooses.
	SuggestAmbiguous SuggestStatus = 2
	// SuggestUnresolved says there's nothing to go on, or a cell no door took beside cells one did,
	// or a cell that isn't text at all. The plan reads the column as text.
	SuggestUnresolved SuggestStatus = 3
)

// String names the status: "resolved", "ambiguous" or "unresolved".
func (s SuggestStatus) String() string {
	switch s {
	case SuggestResolved:
		return "resolved"
	case SuggestAmbiguous:
		return "ambiguous"
	case SuggestUnresolved:
		return "unresolved"
	default:
		return fmt.Sprintf("SuggestStatus(%d)", uint32(s))
	}
}

// Candidate is a declaration that would settle a SuggestAmbiguous column.
type Candidate struct {
	// Order is the order the date's fields come in, when Format is nil.
	Order hypercast.DateOrder
	// Format is the notation the numbers are in, or nil for a date. Declare it with
	// SuggestOptions.ColumnFormats and ask again: the door can differ under each, as 1,234 is an I32
	// under one and an F64 under the other.
	Format *hypercast.NumFormat
}

// SuggestedColumn is what a sample says of one source column.
type SuggestedColumn struct {
	// Ordinal is the zero-based source column, as a Column names it.
	Ordinal int
	// Name is the header's name for it, or nil if the source has no header.
	Name *string
	// Door is the door found: DoorText when unresolved, and the first candidate's when ambiguous.
	Door Door
	// Param is what Door declares, as the door's constructor takes it: a date's hypercast.DateOrder,
	// the first candidate's when ambiguous, a Unix precision or an Excel epoch. 0 for another door.
	Param uint32
	// Format is the notation the plan reads a resolved numeric column in: the one declared for it,
	// or else the one candidate the sample left, comma decimal where only that reads the column, and
	// otherwise , grouping in threes, whether or not the sample held a ,. Both are Western, so a later
	// 1,23,456 is refused. It's a copy, the report's own. nil for any other column.
	Format *hypercast.NumFormat
	// Candidates is what to choose between: empty unless ambiguous.
	Candidates []Candidate
	// Status is how far the sample settles it.
	Status SuggestStatus
	// Matched is the cells Door took. Resolved or ambiguous, that's all of Sampled. Unresolved, it's
	// the most that any one door but the text door took.
	Matched int
	// Sampled is the cells that aren't blank.
	Sampled int
}

// Suggestion is what SuggestDelimited, SuggestDelimitedBytes and Sheet.Suggest found: a plan to
// change or to hand to a read, and why.
type Suggestion struct {
	// Plan is a column for each source column, in order. Only a resolved column reads through its
	// door: the rest read as text, which refuses nothing and loses nothing.
	Plan []Column
	// Report is a report for each source column, in the plan's order.
	Report []SuggestedColumn
	// Truncated says the source has rows past the sample. A source with exactly the sample's rows
	// hasn't.
	Truncated bool
}

// SuggestDelimited reads a sample of the file at path through the text door and says what its
// columns look like: a plan to look at, change and hand to a read, and a report. Nothing is sniffed
// in a read, and nothing here reaches one: this is its own read of the file, opened and closed here.
// There's no form of this for a stream, which would lose the rows it sampled.
//
// The width is the header's, or the first record's, and a plan has to name the columns before that
// record can be read. So with no header the file is opened twice, once to read the first record: a
// path that can only be read once, as a FIFO's can, is not for this. The dialect is what the sample
// is read with, its tokens included, and a cell it says is empty is blank to the suggestion.
//
// Declaring the locale is the recommended route: a number format and a date order are the plan's to
// say, and with them nothing needs inferring. A suggestion is a starting point for a plan to inspect.
//
// What it says can surprise, so read the report before the plan:
//
//   - A column of 0 and 1 is bool, the first door asked, and F or No read false.
//   - 1.234 alone is ambiguous, 1.234 or 1234, and reads as text until a format is declared or a
//     value settles it, which takes in a column of prices at three decimals. Beside 2.5 it's a
//     decimal, and beside 2,5 it's 1234.
//   - 2,5, 0,125, 1.234,5 and 1.234.567 read only with , as the decimal, so they're resolved in that
//     notation, and SuggestedColumn.Format names it. 2,5 beside 2.5 is unresolved.
//   - 1,23,456, Indian grouping, reads under neither notation with none declared, because nothing
//     says its culture: alone it's text, resolved, and beside 5 unresolved. Declaring
//     hypercast.Invariant.WithGrouping(hypercast.GroupingIndian) for the column makes it a number.
//   - 007 and an integer past int64 are text, with no note saying why, so as not to lose the zeros
//     or the digits.
//   - A sheet's date column is text: Sheet.Suggest says why.
//
// Any format declared in options that HyperCast refuses is an error first, before the file is opened,
// though another replaces it or the sample never reaches its ordinal. Then what opening the file can
// return is its error, and a *Failure among the sampled rows: there's no knowing what's past it, and
// a read would stop there too. A failure past them isn't met, or only says the source was Truncated.
func SuggestDelimited(path string, dialect Dialect, options SuggestOptions) (*Suggestion, error) {
	return suggestDelimited(dialect, options, func(opts ...Option) (*DelimitedReader, error) {
		return OpenDelimitedUnbound(path, dialect, opts...)
	})
}

// SuggestDelimitedBytes is SuggestDelimited over text in memory.
func SuggestDelimitedBytes(text []byte, dialect Dialect, options SuggestOptions) (*Suggestion, error) {
	return suggestDelimited(dialect, options, func(opts ...Option) (*DelimitedReader, error) {
		return NewDelimitedReaderBytesUnbound(text, dialect, opts...)
	})
}

func suggestDelimited(dialect Dialect, options SuggestOptions, open func(...Option) (*DelimitedReader, error)) (*Suggestion, error) {
	if err := options.checkFormats(); err != nil {
		return nil, err
	}
	limit, err := options.rowsWithin(DefaultBatchRows)
	if err != nil {
		return nil, err
	}
	width := -1
	if !dialect.HasHeader {
		// A headerless source's width is its first record's, which a read says through a plan of
		// one column.
		first, err := open(BatchRows(1), Notes(false))
		if err != nil {
			return nil, err
		}
		defer first.Close()
		if err := first.Bind([]Column{Text(0)}); err != nil {
			return nil, err
		}
		if _, err := first.Read(); err != nil && err != io.EOF {
			return nil, err
		}
		width = first.ColumnCount()
	}
	reader, err := open(BatchRows(1), Notes(false))
	if err != nil {
		return nil, err
	}
	defer reader.Close()
	if width < 0 {
		width = len(reader.Header())
	}
	sample, err := sampleOf(reader, width, limit, false)
	if err != nil {
		return nil, err
	}
	return sample.judge(reader.Header(), &options)
}

// Suggest reads a sample of this sheet through the text door and says what its columns look like, as
// SuggestDelimited does. It's a read of its own, from the sheet's start with the sheet's options, and
// this one's own read is left as it was. Its plan's ordinals count from the range's first column, as
// any plan's do.
//
// A typed number is judged by its value and has no notation, whichever is declared: a column of them
// is a number under any format, and its plan reads every one, since a typed cell reads the same under
// them all. A text cell holding 1.234 keeps its question, in a column of its own or beside typed
// numbers.
//
// A typed date, date-time, time or duration cell reads as its canonical text, which the suggester
// doesn't take as a date: such a column of a sheet is text, resolved, for a plan to change by hand.
func (s *Sheet) Suggest(options SuggestOptions) (*Suggestion, error) {
	if err := options.checkFormats(); err != nil {
		return nil, err
	}
	limit, err := options.rowsWithin(s.options.BatchRows)
	if err != nil {
		return nil, err
	}
	read := s.options
	width := -1
	if !read.HasHeader {
		// A headerless sheet's width is its first row's, which a read says through a plan of one
		// column.
		firstOptions := read
		firstOptions.BatchRows = 1
		first, err := newSheet(s.book, s.info, firstOptions, nil, false, []Option{Notes(false)})
		if err != nil {
			return nil, err
		}
		if width, err = first.firstRowWidth(); err != nil {
			return nil, err
		}
	}
	read.BatchRows = 1
	// Notes on, for what the sample's second door says of a cell, in take.
	sheet, err := newSheet(s.book, s.info, read, nil, false, nil)
	if err != nil {
		return nil, err
	}
	if width < 0 {
		width = len(sheet.Header())
	}
	sample, err := sampleOf(sheet, width, limit, true)
	if err != nil {
		return nil, err
	}
	return sample.judge(sheet.Header(), &options)
}

// firstRowWidth is how many cells the sheet's next row has: 0 for no row.
func (s *Sheet) firstRowWidth() (int, error) {
	if err := s.Bind([]Column{Text(0)}); err != nil {
		return 0, err
	}
	// The instance lays the plan out at the first fill, and this is the one thing it's to ask.
	s.set.widths = true
	if _, err := s.Read(); err != nil {
		if err == io.EOF {
			return 0, nil
		}
		return 0, err
	}
	n := s.scratch.core
	return int(le.Uint32(n.widths.bytes(n.in))), nil
}

// sampled is a source a sample can be read from: a reader bound through Bind, then read.
type sampled interface {
	reading
	Bind(plan []Column) error
	setBatchRows(rows int)
	sizeFlags() uint32
}

// sample is the sample, in the shape the core takes it: each column's spans into text.
type sample struct {
	text      []byte
	columns   [][]rawSpan
	typed     [][]byte // a byte for each span of a sheet's: not 0 for a typed cell
	rows      int
	limit     int
	truncated bool
}

// sampleOf reads a source of width columns through the text door until the sample is whole and a row
// past it, or the end, says whether it's the source's last. A failure among the sampled rows is the
// caller's, and a failure just past them only says there's more.
//
// A sheet's sample binds a second, UUID, door to every column, for what it says of a cell: no door but
// a text door takes a typed cell's kind, so there it's Malformed with no detail, where a text cell that
// isn't a UUID has one. That's the read's word for which cells were typed, and it's what keeps a typed
// number out of the question of what , and . mean.
func sampleOf(source sampled, width, limit int, sheet bool) (*sample, error) {
	// One row more than the sample, so that a row past it costs no second batch to find, in as many
	// batches as the core says that takes.
	return sampleIn(source, width, limit, sheet, func(plan []Column) (int, error) {
		return sampleRows(plan, limit+1, source.sizeFlags())
	})
}

// sampleIn is sampleOf in batches of however many rows rows says.
func sampleIn(source sampled, width, limit int, sheet bool, rows func([]Column) (int, error)) (*sample, error) {
	if width > maxOrdinal+1 {
		return nil, fmt.Errorf("hypertabular: a suggestion can't plan for %d columns; an ordinal must be below 2^20 (1,048,576)", width)
	}
	s := &sample{columns: make([][]rawSpan, width), limit: limit}
	if width == 0 {
		return s, nil
	}
	plan := make([]Column, width, 2*width)
	for ordinal := range plan {
		plan[ordinal] = Text(ordinal)
	}
	if sheet {
		s.typed = make([][]byte, width)
		for ordinal := range width {
			plan = append(plan, Uuid(ordinal))
		}
	}
	batch, err := rows(plan)
	if err != nil {
		return nil, err
	}
	source.setBatchRows(batch)
	if err := source.Bind(plan); err != nil {
		return nil, err
	}
	for !s.truncated {
		batch, err := source.Read()
		switch {
		case err == io.EOF:
			return s, nil
		case err != nil && s.rows == limit:
			s.truncated = true
		case err != nil:
			return nil, err
		default:
			s.take(batch)
		}
	}
	return s, nil
}

// take takes what the sample still wants of a batch. A blank, a token and padding are all a blank
// span. A refused cell keeps its bytes, for the core's own text door to refuse.
func (s *sample) take(batch *Batch) {
	for row := range batch.Rows() {
		if s.rows == s.limit {
			s.truncated = true
			return
		}
		start := len(s.text)
		for column := range s.columns {
			verdict := batch.Verdicts(column)[row]
			switch {
			case verdict.OK():
				s.text = append(s.text, batch.Text(column)[row]...)
			case verdict.Reason() != hypercast.Empty:
				s.text = append(s.text, batch.Raw(column, row)...)
			}
			s.columns[column] = append(s.columns[column], rawSpan{offset: uint32(len(s.text))})
		}
		// A span holds a 32-bit offset. A sample that far along stops, and says it did.
		if uint64(len(s.text)) > math.MaxUint32 {
			s.text = s.text[:start]
			for column := range s.columns {
				s.columns[column] = s.columns[column][:s.rows]
			}
			s.truncated = true
			return
		}
		// Each span was written with where it ended: now it says where it starts and how long it is.
		at := uint32(start)
		for column := range s.columns {
			span := &s.columns[column][s.rows]
			span.offset, span.len = at, span.offset-at
			at += span.len
		}
		for column := range s.typed {
			var mark byte
			second := len(s.columns) + column
			if batch.Verdicts(second)[row].Reason() == hypercast.Malformed && batch.Detail(second, row) == 0 {
				mark = 1
			}
			s.typed[column] = append(s.typed[column], mark)
		}
		s.rows++
	}
}

// judge asks the core, and builds the plan and the report from its answer.
func (s *sample) judge(header Header, options *SuggestOptions) (*Suggestion, error) {
	width := len(s.columns)
	declared := make([]*hypercast.NumFormat, width)
	formats := make([]hypercast.RawNumFormat, width)
	for ordinal := range declared {
		format := options.declared(ordinal)
		if format == nil {
			continue
		}
		raw, err := format.Raw() // judged already, by checkFormats
		if err != nil {
			return nil, fmt.Errorf("hypertabular: the format declared for column %d: %w", ordinal, err)
		}
		declared[ordinal], formats[ordinal] = format, raw
	}
	cells := make([]rawSpan, 0, width*s.rows)
	for _, column := range s.columns {
		cells = append(cells, column...)
	}
	var typed []byte
	for _, column := range s.typed {
		typed = append(typed, column...)
	}
	found, ok := nativeSuggest(s.text, cells, typed, s.rows, formats)
	if !ok {
		return nil, errors.New("hypertabular: the sample does not fit the core's 4 GiB memory")
	}
	suggestion := &Suggestion{
		Plan:      make([]Column, width),
		Report:    make([]SuggestedColumn, width),
		Truncated: s.truncated,
	}
	for ordinal := range found {
		suggestion.Report[ordinal] = reportOf(ordinal, found[ordinal], header, declared[ordinal])
		settle(&suggestion.Report[ordinal], options.declaredOrder(ordinal))
		suggestion.Plan[ordinal] = planOf(ordinal, &suggestion.Report[ordinal])
	}
	return suggestion, nil
}

// reportOf is what the core said of a column, in this package's terms. Every door the core has is
// taken here, not only those it suggests today: a door it doesn't name, or a param its door doesn't
// declare, reads as text. declared is the format declared for the column, or nil.
func reportOf(ordinal int, found rawSuggestion, header Header, declared *hypercast.NumFormat) SuggestedColumn {
	report := SuggestedColumn{
		Ordinal: ordinal,
		Status:  SuggestStatus(found.status),
		Matched: int(found.matched),
		Sampled: int(found.sampled),
	}
	if ordinal < len(header) {
		report.Name = &header[ordinal]
	}
	if report.Status != SuggestResolved && report.Status != SuggestAmbiguous {
		report.Status = SuggestUnresolved
	}
	door, param := Door(found.door), found.param
	if report.Status == SuggestAmbiguous {
		// An ambiguous param is a set of candidates, bit n for code n.
		report.Candidates = candidatesOf(door, found.param)
		param = uint32(bits.TrailingZeros32(found.param))
	}
	if door.numeric() {
		param = 0
	}
	report.Door, report.Param = doorOf(door, param)
	if report.Status == SuggestResolved && report.Door.numeric() {
		format := commaGroups
		if found.param == notationCommaDecimal {
			format = commaDecimal
		}
		if declared != nil {
			format = *declared
		}
		report.Format = &format
	}
	return report
}

// settle answers a date the sample left open with the order declared for its column, if that's one
// of the candidates.
func settle(report *SuggestedColumn, order *hypercast.DateOrder) {
	if order == nil || report.Status != SuggestAmbiguous {
		return
	}
	for _, candidate := range report.Candidates {
		if candidate.Format == nil && candidate.Order == *order {
			report.Status, report.Candidates = SuggestResolved, nil
			report.Door, report.Param = DoorDateOnlyOrdered, uint32(*order)
			return
		}
	}
}

// doorOf is a door and what it declares, or the text door when the core's numbers aren't one.
func doorOf(door Door, param uint32) (Door, uint32) {
	if _, _, err := (Column{door: door, param: param, format: hypercast.Invariant}).spec(); err != nil {
		return DoorText, 0
	}
	switch door {
	case DoorUnix, DoorDateOnlyOrdered, DoorDateTime, DoorExcelSerial:
		return door, param
	default:
		return door, 0
	}
}

// candidatesOf is what an ambiguous column's param offers.
func candidatesOf(door Door, param uint32) []Candidate {
	var candidates []Candidate
	if door == DoorDateOnlyOrdered || door == DoorDateTime {
		for _, order := range []hypercast.DateOrder{hypercast.YearMonthDay, hypercast.MonthDayYear, hypercast.DayMonthYear} {
			if param&(1<<uint32(order)) != 0 {
				candidates = append(candidates, Candidate{Order: order})
			}
		}
		return candidates
	}
	if param&notationCommaGroups != 0 {
		format := commaGroups
		candidates = append(candidates, Candidate{Format: &format})
	}
	if param&notationCommaDecimal != 0 {
		format := commaDecimal
		candidates = append(candidates, Candidate{Format: &format})
	}
	return candidates
}

// planOf is the column a report plans: only a resolved column reads through its door, a numeric one
// in the format the report names.
func planOf(ordinal int, report *SuggestedColumn) Column {
	if report.Status != SuggestResolved {
		return Text(ordinal)
	}
	column := Column{ordinal: ordinal, door: report.Door, param: report.Param}
	if report.Format != nil {
		column.format = *report.Format
	}
	return column
}
