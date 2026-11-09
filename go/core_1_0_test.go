package hypertabular

// The 1.0 exports, which no read takes: called through core.go's wrappers, with the shapes they read
// laid out here, and held to what the size-tagged exports read of the same input. The drift check
// wants every export called at this binding's site, and the ABI promises these stay.

import (
	"reflect"
	"slices"
	"testing"
	"unsafe"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// Sizes of the shapes only a 1.0 call reads: a ColumnSpec's three words and RawNumFormat, a
// ColumnBuffer's two addresses.
const (
	specSize   = 104
	bufferSize = 2 * 4
)

// lay1Dot0 writes a plan's ColumnSpecs and ColumnBuffers into the instance, over the block attach
// laid out, whose notes these shapes have no room for.
func lay1Dot0(t *testing.T, n *native, set *columns) (specs, buffers region) {
	t.Helper()
	if !n.attach(set) {
		t.Fatal("no room for the plan")
	}
	specs, ok := n.in.alloc(uint64(len(set.specs)) * specSize)
	if !ok {
		t.Fatal("no room for the specs")
	}
	buffers, ok = n.in.alloc(uint64(len(set.specs)) * bufferSize)
	if !ok {
		t.Fatal("no room for the buffers")
	}
	mem := n.in.mem()
	for index := range set.specs {
		spec := mem[specs.off+uint32(index)*specSize:]
		le.PutUint32(spec[0:], set.specs[index].ordinal)
		le.PutUint32(spec[4:], set.specs[index].door)
		le.PutUint32(spec[8:], set.specs[index].param)
		putFormat(spec[12:12+rawFormatSize], &set.specs[index].format)
		buffer := mem[buffers.off+uint32(index)*bufferSize:]
		le.PutUint32(buffer[0:], n.block.off+uint32(set.offsets[3*index]))
		le.PutUint32(buffer[4:], n.block.off+uint32(set.offsets[3*index+1]))
	}
	return specs, buffers
}

// A policy is the one thing 1.0 can't be told: its fill reads the low 8 flag bits, so every format is
// Western there, and a read of a Western format says the same of every cell.
func TestTheDelimited1Dot0ExportsReadWhatAReadDoes(t *testing.T) {
	input := []byte("1;a b;2.5\n12x;c;x\n1,23,456;d;4\n")
	dialect := Dialect{Separator: ';', Quoting: true, SkipBlankLines: true}
	plan := []Column{I32(0, hypercast.Invariant), Text(1), F64(2, hypercast.Invariant)}

	n := newNative()
	if code := n.init1Dot0(dialect); code != codeOK {
		t.Fatalf("init: %d", code)
	}
	set, err := newColumns(plan, 16, false, 0)
	if err != nil {
		t.Fatal(err)
	}
	specs, buffers := lay1Dot0(t, n, set)
	if !n.in.ensure(&n.arena, 4096, false) || !n.in.ensure(&n.cells, 8*64, false) {
		t.Fatal("no room for the buffers")
	}
	text, ok := n.in.alloc(uint64(len(input)))
	if !ok {
		t.Fatal("no room for the input")
	}
	text.put(n.in, input)
	n.putBuffers(region{}, 0, n.arena, 4096, n.cells, 64, region{}, 0, false)
	if code := n.fill1Dot0(text, true, specs, buffers, len(plan), 16); code != codeOK {
		t.Fatalf("fill: %d", code)
	}
	filled := decodeFilled(n.fixedBytes(fixedOut, 64))
	if filled.rows != 3 {
		t.Fatalf("%d rows", filled.rows)
	}
	set.point(n, 3)

	r, err := NewDelimitedReaderBytes(input, dialect, plan, Notes(false))
	if err != nil {
		t.Fatal(err)
	}
	batch, err := r.Read()
	if err != nil || batch.Rows() != 3 {
		t.Fatalf("%v, %v", batch, err)
	}
	ints := unsafe.Slice((*int32)(set.columns[0].values), 3)
	if want := []int32{1, 0, 0}; !slices.Equal(ints, want) {
		t.Errorf("1.0 read %v, want %v", ints, want)
	}
	if !batch.Verdicts(0)[0].OK() || batch.Verdicts(0)[2].Reason() != hypercast.Malformed {
		t.Errorf("1,23,456: a read %v", batch.Verdicts(0)[2])
	}
	if !reflect.DeepEqual(set.columns[0].verdicts[:3], batch.Verdicts(0)) || !reflect.DeepEqual(set.columns[2].verdicts[:3], batch.Verdicts(2)) {
		t.Errorf("verdicts differ: %v and %v", set.columns[0].verdicts[:3], batch.Verdicts(0))
	}
	if reals := unsafe.Slice((*float64)(set.columns[2].values), 3); !slices.Equal(reals, batch.F64(2)) {
		t.Errorf("1.0 read %v, a read %v", reals, batch.F64(2))
	}
	spans := unsafe.Slice((*rawSpan)(set.columns[1].values), 3)
	for row, span := range spans {
		if got := input[span.offset : span.offset+span.len]; string(got) != string(batch.Text(1)[row]) {
			t.Errorf("row %d: 1.0 read %q, a read %q", row, got, batch.Text(1)[row])
		}
	}
}

// Indian declared: a read takes 1,23,456, and 1.0 can't see the declaration, so it refuses it.
func TestADelimited1Dot0ExportReadsAnIndianColumnWestern(t *testing.T) {
	input := []byte("\"1,23,456\"\n")
	dialect := Dialect{Separator: ',', Quoting: true, SkipBlankLines: true}
	plan := []Column{I32(0, hypercast.Invariant.WithGrouping(hypercast.GroupingIndian))}

	n := newNative()
	if code := n.init1Dot0(dialect); code != codeOK {
		t.Fatalf("init: %d", code)
	}
	set, err := newColumns(plan, 16, false, 0)
	if err != nil {
		t.Fatal(err)
	}
	specs, buffers := lay1Dot0(t, n, set)
	if !n.in.ensure(&n.arena, 4096, false) || !n.in.ensure(&n.cells, 8*64, false) {
		t.Fatal("no room for the buffers")
	}
	text, ok := n.in.alloc(uint64(len(input)))
	if !ok {
		t.Fatal("no room for the input")
	}
	text.put(n.in, input)
	n.putBuffers(region{}, 0, n.arena, 4096, n.cells, 64, region{}, 0, false)
	if code := n.fill1Dot0(text, true, specs, buffers, len(plan), 16); code != codeOK {
		t.Fatalf("fill: %d", code)
	}
	set.point(n, 1)
	if got := set.columns[0].verdicts[0]; got.Reason() != hypercast.Malformed {
		t.Errorf("1.0: %v", got)
	}

	r, err := NewDelimitedReaderBytes(input, dialect, plan, Notes(false))
	if err != nil {
		t.Fatal(err)
	}
	batch, err := r.Read()
	if err != nil || !batch.Verdicts(0)[0].OK() || batch.I32(0)[0] != 123456 {
		t.Errorf("a read: %v, %v", batch, err)
	}
}

func TestTheWorkbook1Dot0ExportsReadWhatAReadDoes(t *testing.T) {
	book, err := OpenWorkbook(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	options := SheetOptions{HasHeader: true, SkipEmptyRows: true, BatchRows: 16}
	plan := []Column{Text(0), Text(1)}

	n, ok := newBookNative(book, true)
	if !ok {
		t.Fatal("no room for the workbook")
	}
	info := &book.Sheets()[0]
	if n.part, ok = n.in.alloc(uint64(len(info.part))); !ok {
		t.Fatal("no room for the part")
	}
	n.part.put(n.in, info.part)
	if code := n.sheet1Dot0(info, options); code != codeOK {
		t.Fatalf("sheet: %d", code)
	}
	set, err := newColumns(plan, options.BatchRows, false, batchSheet)
	if err != nil {
		t.Fatal(err)
	}
	specs, buffers := lay1Dot0(t, n, set)
	const room = 1 << 20
	const cells = 4096
	if !n.in.ensure(&n.bookWindow, room, true) || !n.in.ensure(&n.arena, room, true) || !n.in.ensure(&n.cells, 8*cells, true) || !n.in.ensure(&n.row, 16*64, true) {
		t.Fatal("no room for the buffers")
	}
	n.putBuffers(n.bookWindow, room, n.arena, room, n.cells, cells, n.row, 64, true)
	// The sheet was told it has a header, so the header comes first; told otherwise, it would be a row.
	container, length := n.containerArgs()
	n.in.enter()
	code := n.in.m.Xhypertabular_workbook_header(n.book.at(), container, length, n.at(fixedBuffers), n.at(fixedOut))
	n.in.leave()
	if code != codeOK {
		t.Fatalf("header: %d", code)
	}
	if code := n.bookFill1Dot0(specs, buffers, len(plan), options.BatchRows); code != codeOK {
		t.Fatalf("fill: %d", code)
	}
	filled := decodeFilled(n.fixedBytes(fixedOut, 64))
	rows := int(filled.rows)
	set.point(n, rows)

	sheet, err := book.Sheet(0, options, plan, Notes(false))
	if err != nil {
		t.Fatal(err)
	}
	batch, err := sheet.Read()
	if err != nil {
		t.Fatal(err)
	}
	if rows == 0 || rows != batch.Rows() {
		t.Fatalf("1.0 read %d rows, a read %d", rows, batch.Rows())
	}
	// A text cell's span says where its bytes are, the arena or a table: the same place, both ways.
	for column := range plan {
		got := unsafe.Slice((*rawSpan)(set.columns[column].values), rows)
		want := unsafe.Slice((*rawSpan)(batch.columns.columns[column].values), rows)
		if !slices.Equal(got, want) || !slices.Equal(set.columns[column].verdicts[:rows], batch.Verdicts(column)) {
			t.Errorf("column %d: 1.0 read %v, %v; a read %v, %v", column, got, set.columns[column].verdicts[:rows], want, batch.Verdicts(column))
		}
	}
}
