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
	plan := []hypertabular.Column{hypertabular.Text(0), hypertabular.Text(1)}
	sheet, err := book.Sheet(0, hypertabular.DefaultSheetOptions, plan)
	if err != nil {
		log.Fatal(err)
	}
	fmt.Println(sheet.Header()[:2])
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
