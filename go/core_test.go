package hypertabular

// Portable tests of the translated-core backend (core.go): no build tag and no os/exec, so
// they run under stock Go on every port and under TinyGo.

import (
	"bytes"
	"encoding/binary"
	"fmt"
	"io"
	"os"
	"reflect"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	"github.com/SkunkWerkx/HyperTabular/go/internal/core"
	"github.com/google/uuid"
)

// The fixed block and every region start where the module's own memory ends.
func TestTheModuleIsItsOwnTwoPages(t *testing.T) {
	if got := len(*core.New().Xmemory().Slice()); got != 2*pageSize {
		t.Fatalf("a fresh module holds %d bytes, want two pages", got)
	}
	n := newNative()
	if n.fixed.off != 2*pageSize || len(n.in.mem()) != 3*pageSize {
		t.Fatalf("the fixed block at %d, in %d bytes", n.fixed.off, len(n.in.mem()))
	}
}

// Every layout is the fields of the Go type a batch views the door's values as, padding a
// run of single bytes: what decodes a value on a big-endian host is held on any host.
func TestEveryLayoutIsTheViewedType(t *testing.T) {
	viewed := map[Door]reflect.Type{
		DoorBool: reflect.TypeFor[bool](), DoorI8: reflect.TypeFor[int8](), DoorI16: reflect.TypeFor[int16](),
		DoorI32: reflect.TypeFor[int32](), DoorI64: reflect.TypeFor[int64](), DoorU8: reflect.TypeFor[uint8](),
		DoorU16: reflect.TypeFor[uint16](), DoorU32: reflect.TypeFor[uint32](), DoorU64: reflect.TypeFor[uint64](),
		DoorF32: reflect.TypeFor[float32](), DoorF64: reflect.TypeFor[float64](), DoorUuid: reflect.TypeFor[uuid.UUID](),
		DoorTimestamp: reflect.TypeFor[hypercast.RawTimestamp](), DoorUnix: reflect.TypeFor[hypercast.RawTimestamp](),
		DoorExcelSerial: reflect.TypeFor[hypercast.RawTimestamp](), DoorDateOnly: reflect.TypeFor[hypercast.RawDate](),
		DoorDateOnlyOrdered: reflect.TypeFor[hypercast.RawDate](), DoorTimeOfDay: reflect.TypeFor[time.Duration](),
		DoorSpan: reflect.TypeFor[hypercast.Duration](), DoorText: reflect.TypeFor[rawSpan](),
		DoorExact: reflect.TypeFor[hypercast.Decimal](), DoorDateTime: reflect.TypeFor[hypercast.RawCivil](),
	}
	for door := DoorBool; door <= DoorExcelSerial; door++ {
		typ := viewed[door]
		if want := fieldsOf(typ); !slices.Equal(door.layout(), want) || typ.Size() != door.valueSize() {
			t.Errorf("%v: layout %v of %d bytes, the viewed %v is %v of %d", door, door.layout(), door.valueSize(), typ, want, typ.Size())
		}
	}
	if want := fieldsOf(reflect.TypeFor[CellVerdict]()); !slices.Equal(fieldsVerdict, want) {
		t.Errorf("verdict: layout %v, CellVerdict is %v", fieldsVerdict, want)
	}
}

// fieldsOf is typ's fields in order by width, arrays of bytes and padding as single bytes.
func fieldsOf(typ reflect.Type) []uint8 {
	switch typ.Kind() {
	case reflect.Struct:
		var fields []uint8
		at := uintptr(0)
		for index := range typ.NumField() {
			field := typ.Field(index)
			for ; at < field.Offset; at++ {
				fields = append(fields, 1)
			}
			fields = append(fields, fieldsOf(field.Type)...)
			at = field.Offset + field.Type.Size()
		}
		for ; at < typ.Size(); at++ {
			fields = append(fields, 1)
		}
		return fields
	case reflect.Array:
		var fields []uint8
		for range typ.Len() {
			fields = append(fields, fieldsOf(typ.Elem())...)
		}
		return fields
	default:
		return []uint8{uint8(typ.Size())}
	}
}

// What a big-endian host does, held on any host: every field of every layout is read in the
// guest's order and written in the host's, padding and single bytes as they are.
func TestDecodingForABigEndianHost(t *testing.T) {
	layouts := map[string][]uint8{
		"verdict": fieldsVerdict, "span": fieldsSpan, "decimal": fieldsDecimal, "civil": fieldsCivil,
		"seconds": fieldsSeconds, "duration": fieldsDuration, "date": fieldsDate, "uuid": fieldsUuid, "1": fields1, "2": fields2, "4": fields4, "8": fields8,
	}
	for name, fields := range layouts {
		var guest, want []byte
		value := uint64(0x0102030405060708)
		for range 3 { // three values
			for _, width := range fields {
				value += 0x1111111111111111
				g, w := make([]byte, width), make([]byte, width)
				switch width {
				case 1:
					g[0], w[0] = byte(value), byte(value)
				case 2:
					binary.LittleEndian.PutUint16(g, uint16(value))
					binary.BigEndian.PutUint16(w, uint16(value))
				case 4:
					binary.LittleEndian.PutUint32(g, uint32(value))
					binary.BigEndian.PutUint32(w, uint32(value))
				case 8:
					binary.LittleEndian.PutUint64(g, value)
					binary.BigEndian.PutUint64(w, value)
				}
				guest, want = append(guest, g...), append(want, w...)
			}
		}
		got := make([]byte, len(guest))
		decodeFields(got, guest, fields, binary.BigEndian)
		if !bytes.Equal(got, want) {
			t.Errorf("%s: decoded % x, want % x", name, got, want)
		}
	}
}

// Both corpora again with every view a decoded copy, as on a big-endian host or a guest
// memory that is not 8-aligned.
func TestTheCorpusThroughDecodedCopies(t *testing.T) {
	if raceDetector {
		t.Skip("one goroutine, and a minute and a half under the race detector: the plain run holds it")
	}
	forceDecode = true
	defer func() { forceDecode = false }()
	TestCorpusDelimited(t)
	replayWorkbookCorpus(t)
	// And the copies are what was handed out.
	r := open(t, "a,b\n1,2\n", Dialect{Separator: ',', HasHeader: true}, []Column{I32(0, hypercast.Invariant), Text(1)})
	b := read(t, r, 1)
	if r.core.cellsBuf == nil || r.set.block == nil || b.I32(0)[0] != 1 || string(b.Text(1)[0]) != "2" {
		t.Fatal("the decoded path was not taken")
	}
}

// rowsOf is input of n records of an id and a name, the name padded to width bytes.
func rowsOf(n, width int) []byte {
	var input bytes.Buffer
	for row := range n {
		name := fmt.Sprintf("name %d", row)
		fmt.Fprintf(&input, "%d,%s%s\n", row, name, strings.Repeat("x", max(0, width-len(name))))
	}
	return input.Bytes()
}

// readAll reads r to its end, checking every row against rowsOf, and returns the batch sizes.
func readAll(t *testing.T, r *DelimitedReader) []int {
	t.Helper()
	var sizes []int
	seen := 0
	for {
		b, err := r.Read()
		if err == io.EOF {
			return sizes
		}
		if err != nil {
			t.Fatal(err)
		}
		ids, names := b.I64(0), b.Text(1)
		for row := range b.Rows() {
			if ids[row] != int64(seen+row) || !strings.HasPrefix(string(names[row]), fmt.Sprintf("name %d", seen+row)) {
				t.Fatalf("row %d: %d, %.20q", seen+row, ids[row], names[row])
			}
		}
		seen += b.Rows()
		sizes = append(sizes, b.Rows())
	}
}

var (
	idAndName = []Column{I64(0, hypercast.Invariant), Text(1)}
	noHeader  = Dialect{Separator: ',', Quoting: true}
)

// Text in memory is handed to the core a chunk at a time, and every byte of it is copied in
// about once: copying the rest of the text for every batch would make a read quadratic.
func TestTextInMemoryIsCopiedInAboutOnce(t *testing.T) {
	input := rowsOf(100_000, 20) // about 3 MB, in batches of 64 rows: about 1,500 batches
	r, err := NewDelimitedReaderBytes(input, noHeader, idAndName, BatchRows(64))
	if err != nil {
		t.Fatal(err)
	}
	readAll(t, r)
	if limit := uint64(len(input))*2 + 4*chunkMin; r.core.copied > limit {
		t.Errorf("%d bytes of text copied in %d bytes, past %d", len(input), r.core.copied, limit)
	}
}

// A batch from memory is full until the input runs out, whatever the chunk: a call cut short
// by the chunk's end is made again over a larger one.
func TestABatchFromMemoryIsFull(t *testing.T) {
	const batchRows = 4096
	input := rowsOf(50_000, 60) // a batch is about 280 KB, several chunks
	r, err := NewDelimitedReaderBytes(input, noHeader, idAndName, BatchRows(batchRows))
	if err != nil {
		t.Fatal(err)
	}
	sizes := readAll(t, r)
	for index, size := range sizes[:len(sizes)-1] {
		if size != batchRows {
			t.Fatalf("batch %d has %d rows, want %d: %v", index, size, batchRows, sizes)
		}
	}
	if total := (len(sizes)-1)*batchRows + sizes[len(sizes)-1]; total != 50_000 {
		t.Fatalf("%d rows", total)
	}
}

// A record longer than any chunk so far is read whole, from memory and from a stream.
func TestARecordLongerThanTheChunk(t *testing.T) {
	var text bytes.Buffer
	text.Write(rowsOf(2, 10))
	fmt.Fprintf(&text, "2,name 2%s\n3,name 3%s\n4,name 4\n", strings.Repeat("x", 5*chunkMin), strings.Repeat("y", 7*chunkMin))
	for _, build := range []func() (*DelimitedReader, error){
		func() (*DelimitedReader, error) {
			return NewDelimitedReaderBytes(text.Bytes(), noHeader, idAndName, BatchRows(16))
		},
		func() (*DelimitedReader, error) {
			return NewDelimitedReader(bytes.NewReader(text.Bytes()), noHeader, idAndName, BatchRows(16), BufferBytes(100))
		},
	} {
		r, err := build()
		if err != nil {
			t.Fatal(err)
		}
		total := 0
		for _, size := range readAll(t, r) {
			total += size
		}
		if total != 5 {
			t.Fatalf("%d rows, want 5", total)
		}
	}
}

// Every reader has an instance of its own: readers on separate goroutines run at once and
// share nothing (the race detector runs this too).
func TestReadersOnGoroutinesShareNothing(t *testing.T) {
	input := rowsOf(20_000, 12)
	var wg sync.WaitGroup
	errs := make(chan error, 8)
	for worker := range 8 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			r, err := NewDelimitedReaderBytes(input, noHeader, idAndName, BatchRows(100+worker))
			if err != nil {
				errs <- err
				return
			}
			seen := 0
			for {
				b, err := r.Read()
				if err == io.EOF {
					break
				}
				if err != nil {
					errs <- err
					return
				}
				for row, id := range b.I64(0) {
					if id != int64(seen+row) {
						errs <- fmt.Errorf("reader %d, row %d: %d", worker, seen+row, id)
						return
					}
				}
				seen += b.Rows()
			}
			if seen != 20_000 {
				errs <- fmt.Errorf("reader %d: %d rows", worker, seen)
			}
		}()
	}
	wg.Wait()
	close(errs)
	for err := range errs {
		t.Error(err)
	}
}

// Buffers the core's memory cannot hold are an error, made before anything that large is
// allocated: WebAssembly addresses 4 GiB.
func TestBuffersPastTheCoresMemoryAreAnError(t *testing.T) {
	const rows = 1 << 26 // 16 bytes a value and 12 a verdict: 1.75 GiB a column
	plan := make([]Column, 8)
	for index := range plan {
		plan[index] = Exact(0, hypercast.Invariant)
	}
	_, err := NewDelimitedReaderBytes([]byte("1\n"), CSV, plan, BatchRows(rows))
	if err == nil || (strconv.IntSize == 64 && !strings.Contains(err.Error(), "4 GiB")) {
		t.Errorf("a plan of 14 GiB: %v", err)
	}

	container, err := os.ReadFile(repositoryFile(t, "corpus", "workbook", "basic.xlsx"))
	if err != nil {
		t.Fatal(err)
	}
	book, err := NewWorkbook(container)
	if err != nil {
		t.Fatal(err)
	}
	// The cell table of a batch: 9 entries a row, 8 bytes each, 4.5 GiB.
	options := DefaultSheetOptions
	options.BatchRows = rows
	sheet, err := book.Sheet(0, options, plan)
	if err == nil {
		_, err = sheet.Read()
	}
	if err == nil || (strconv.IntSize == 64 && !strings.Contains(err.Error(), "4 GiB")) {
		t.Errorf("a cell table of 4.5 GiB: %v", err)
	}

	// The same table met by the first Read of a sheet bound after its header: the header's
	// rows are not this call's, so no batch comes back beside the error.
	unbound, err := book.SheetUnbound(0, options)
	if err != nil {
		t.Fatal(err)
	}
	if err := unbound.Bind(plan); err != nil {
		t.Fatal(err)
	}
	batch, err := unbound.Read()
	if batch != nil || err == nil || (strconv.IntSize == 64 && !strings.Contains(err.Error(), "4 GiB")) {
		t.Errorf("a cell table of 4.5 GiB after a header: a batch %t, %v", batch != nil, err)
	}
}

// A call that panicked inside the core left its instance mid-call; the reader refuses to go
// on rather than run on it.
func TestAReaderTheCorePanickedInIsRetired(t *testing.T) {
	r := open(t, "1\n2\n", CSV, []Column{I32(0, hypercast.Invariant)}, BatchRows(1))
	read(t, r, 1)
	r.core.in.busy = true // what a panic part-way through a call leaves
	defer func() {
		message, _ := recover().(string)
		if !strings.Contains(message, "panicked") {
			t.Errorf("recovered %q", message)
		}
	}()
	_, _ = r.Read()
	t.Error("Read ran on the instance")
}
