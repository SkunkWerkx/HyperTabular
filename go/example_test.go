package hypertabular_test

import (
	"errors"
	"fmt"
	"io"
	"log"
	"strings"

	// Both import paths end in /go, so the package names have to be spelled out.
	hypercast "github.com/SkunkWerkx/HyperCast/go"
	hypertabular "github.com/SkunkWerkx/HyperTabular/go"
)

func ExampleDelimitedReader() {
	const orders = "id,customer,total,placed\n" +
		"1,alice,\"1,234.50\",2026-01-07\n" +
		"2,\"bob \"\"the builder\"\"\",12x4,2026-02-30\n"

	// The dialect and the plan are declared; nothing is sniffed.
	plan := []hypertabular.Column{
		hypertabular.I32(0, hypercast.Invariant),
		hypertabular.Text(1),
		hypertabular.Exact(2, hypercast.Invariant),
		hypertabular.DateOnly(3),
	}
	reader, err := hypertabular.NewDelimitedReader(strings.NewReader(orders), hypertabular.CSV, plan)
	if err != nil {
		log.Fatal(err)
	}
	defer reader.Close()
	fmt.Println(reader.Header())

	for {
		batch, err := reader.Read()
		if err == io.EOF {
			break
		}
		if err != nil {
			log.Fatal(err)
		}
		// A column at a time: typed slices, one element per row, and a verdict beside each…
		ids, customers, placed := batch.I32(0), batch.Text(1), batch.DateOnly(3)
		for row := 0; row < batch.Rows(); row++ {
			fmt.Printf("%d %s:", ids[row], customers[row])
			// …or a cell at a time, as the HyperCast door of the same name returns it.
			if total, fault := hypertabular.Get[hypercast.Decimal](batch, 2, row); fault != nil {
				// A value that did not cast is a verdict, and its text is still to hand.
				fmt.Printf(" total %q is %v at byte %d;", batch.Raw(2, row), fault.Reason, fault.Offset)
			} else {
				fmt.Printf(" total %v;", total)
			}
			if verdict := batch.Verdicts(3)[row]; !verdict.OK() {
				fmt.Printf(" placed %q is %v\n", batch.Raw(3, row), verdict.Reason())
			} else {
				fmt.Printf(" placed %d-%02d-%02d\n", placed[row].Year, placed[row].Month, placed[row].Day)
			}
		}
	}
	// Output:
	// [id customer total placed]
	// 1 alice: total 1234.5; placed 2026-01-07
	// 2 bob "the builder": total "12x4" is malformed at byte 2; placed "2026-02-30" is out of range
}

func ExampleDelimitedReader_Bind() {
	// The publisher controls the column order; the names are the contract. Open without a
	// plan, look the names up in the header, and bind the plan built from them.
	const countries = "Country,M49 Code,ISO-alpha2 Code\n" +
		"Algeria,012,DZ\n" +
		"Argentina,032,AR\n"
	reader, err := hypertabular.NewDelimitedReaderUnbound(strings.NewReader(countries), hypertabular.CSV)
	if err != nil {
		log.Fatal(err)
	}
	defer reader.Close()

	header := reader.Header()
	m49, err := header.Ordinal("M49 Code")
	if err != nil {
		log.Fatal(err)
	}
	iso, err := header.Ordinal("ISO-alpha2 Code")
	if err != nil {
		log.Fatal(err)
	}
	if err := reader.Bind([]hypertabular.Column{
		hypertabular.U16(m49, hypercast.Invariant),
		hypertabular.Text(iso),
	}); err != nil {
		log.Fatal(err)
	}
	// A name the header lacks is an error that says which.
	_, err = header.Ordinal("Region Code")
	fmt.Println(err, errors.Is(err, hypertabular.ErrNoColumn))

	// Row by row, across batches: each row is valid for its own iteration.
	for row, err := range reader.All() {
		if err != nil {
			log.Fatal(err)
		}
		code, _ := hypertabular.Cell[uint16](row, 0)
		// TextString is a view, not a copy: kept past the row, it would need strings.Clone.
		fmt.Println(row.Line(), row.TextString(1), code)
	}
	// Output:
	// hypertabular: the header has no column named "Region Code" true
	// 2 DZ 12
	// 3 AR 32
}

func ExampleFailure() {
	// A record of the wrong width is not a cell's verdict: the input is not rows of cells.
	// It is returned after every intact row before it.
	const broken = "a,b\n1,2\n3\n4,5\n"
	plan := []hypertabular.Column{hypertabular.I32(0, hypercast.Invariant)}
	reader, err := hypertabular.NewDelimitedReaderBytes([]byte(broken), hypertabular.CSV, plan)
	if err != nil {
		log.Fatal(err)
	}
	for {
		batch, err := reader.Read()
		if err != nil {
			var failure *hypertabular.Failure
			if errors.As(err, &failure) {
				fmt.Println(failure.Kind, "at line", failure.Line, "— expected", failure.Expected, "cells, found", failure.Found)
				fmt.Println(failure)
			}
			break
		}
		fmt.Println(batch.Rows(), "row:", batch.I32(0))
	}
	// Output:
	// 1 row: [1]
	// column count at line 3 — expected 2 cells, found 1
	// hypertabular: record 2 (line 3, byte 8) has 1 cells; the first record had 2
}

func ExampleWorkbook() {
	// A workbook reads into the same batch, a sheet at a time.
	book, err := hypertabular.OpenWorkbook("../corpus/workbook/basic.xlsx")
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(book.Format(), book.Sheets()[0].Name)
	// Header first: the sheet is opened, its header read, and the plan bound by name.
	sheet, err := book.SheetUnbound(0, hypertabular.DefaultSheetOptions)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(sheet.Header()[:2])
	id, _ := sheet.Header().Ordinal("id")
	named, _ := sheet.Header().Ordinal("name")
	if err := sheet.Bind([]hypertabular.Column{hypertabular.Text(id), hypertabular.Text(named)}); err != nil {
		log.Fatal(err)
	}
	batch, err := sheet.Read()
	if err != nil {
		log.Fatal(err)
	}
	name, _ := hypertabular.Get[string](batch, 1, 0)
	fmt.Println("row", batch.Line(0), name)
	// Output:
	// XLSX Data
	// [id name]
	// row 2 alice
}

func ExampleNativeVersion() {
	// The linked core's own version, read from the core.
	fmt.Println(hypertabular.NativeVersion() != "")
	// Output: true
}
