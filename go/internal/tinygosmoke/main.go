// Command tinygosmoke is the check CI runs in headless Chrome: this module compiled by TinyGo
// for the browser (`tinygo build -target=wasm`), its core the translated Go source in
// internal/core and HyperCast's beside it, loaded by index.html through TinyGo's own
// wasm_exec.js. It prints one PASS or FAIL line per check and DONE at the end; the forge's
// hyper-build-wasm.yml fails the run on any FAIL or a missing DONE, which is also what a
// panic part-way through looks like.
//
// Every export a read takes crosses at least once: the version, the delimited reader's, the
// transcoder, the suggester and a batch's faults (unescape by a doubled quote), and the
// workbook's and the range parser, through an XLSX written here in memory with archive/zip — deflated, so the core's
// inflate runs too, and with shared strings and a styles part, so opening it reads both. A
// browser holds bytes, not paths, so everything is read from memory, and a cell's fault and a
// torn file's error cross as well.
//
// The expected core version comes from rust/Cargo.toml at build time
// (`-ldflags "-X main.wantVersion=..."`), so the page proves the core compiled in is the one
// this commit's crate translates to, not a stale one. Stock Go builds this too, so
// `go vet ./...` covers it like any other package; it is internal, so it is nothing a
// consumer can import.
package main

import (
	"archive/zip"
	"bytes"
	"fmt"
	"io"
	"time"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	hypertabular "github.com/SkunkWerkx/HyperTabular/go"
)

// wantVersion is rust/Cargo.toml's version, set by the build's -ldflags -X.
var wantVersion string

func check(ok bool, what string) {
	if ok {
		fmt.Println("PASS", what)
	} else {
		fmt.Println("FAIL", what)
	}
}

// xlsx is a workbook of one sheet, as small as a writer may make one: a header row, a
// number, a shared string, and a date-formatted serial (style 1 is built-in format 14).
func xlsx() []byte {
	parts := []struct{ name, body string }{
		{"[Content_Types].xml", `<Types/>`},
		{"xl/workbook.xml", `<workbook xmlns:r="r"><sheets><sheet name="Orders" r:id="r1"/></sheets></workbook>`},
		{"xl/_rels/workbook.xml.rels", `<Relationships>` +
			`<Relationship Id="r1" Type="x/worksheet" Target="worksheets/sheet1.xml"/>` +
			`<Relationship Id="r2" Type="x/sharedStrings" Target="sharedStrings.xml"/>` +
			`<Relationship Id="r3" Type="x/styles" Target="styles.xml"/></Relationships>`},
		{"xl/sharedStrings.xml", `<sst><si><t>id</t></si><si><t>name</t></si><si><t>day</t></si>` +
			`<si><t>bob, jr</t></si></sst>`},
		{"xl/styles.xml", `<styleSheet><cellXfs><xf numFmtId="0"/><xf numFmtId="14"/></cellXfs></styleSheet>`},
		{"xl/worksheets/sheet1.xml", `<worksheet><sheetData>` +
			`<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c><c r="C1" t="s"><v>2</v></c></row>` +
			`<row r="2"><c r="A2"><v>7</v></c><c r="B2" t="s"><v>3</v></c><c r="C2" s="1"><v>45351</v></c></row>` +
			`</sheetData></worksheet>`},
	}
	var out bytes.Buffer
	w := zip.NewWriter(&out)
	for _, part := range parts {
		f, err := w.CreateHeader(&zip.FileHeader{Name: part.name, Method: zip.Deflate})
		if err != nil {
			panic(err)
		}
		if _, err := io.WriteString(f, part.body); err != nil {
			panic(err)
		}
	}
	if err := w.Close(); err != nil {
		panic(err)
	}
	return out.Bytes()
}

func main() {
	version := hypertabular.NativeVersion()
	fmt.Println("NativeVersion", version, "want", wantVersion)
	check(version != "" && version == wantVersion, "NativeVersion matches rust/Cargo.toml")

	csv := []byte("id,name,score,day\n1,\"say \"\"hi\"\"\",2.5,2024-02-29\n2,bob,x,2026-02-29\n")
	plan := []hypertabular.Column{
		hypertabular.I64(0, hypercast.Invariant),
		hypertabular.Text(1),
		hypertabular.F64(2, hypercast.Invariant),
		hypertabular.DateOnly(3),
	}
	reader, err := hypertabular.NewDelimitedReaderBytes(csv, hypertabular.CSV, plan)
	check(err == nil, "NewDelimitedReaderBytes")
	if err == nil {
		check(fmt.Sprint(reader.Header()) == "[id name score day]", "delimited header")
		batch, err := reader.Read()
		check(err == nil && batch.Rows() == 2, "delimited read")
		if err == nil {
			ids := batch.I64(0)
			check(ids[0] == 1 && ids[1] == 2, "I64 column")
			check(string(batch.Text(1)[0]) == `say "hi"`, "Text unescaped")
			score, fault := hypertabular.Get[float64](batch, 2, 0)
			check(fault == nil && score == 2.5, "Get[float64]")
			_, fault = hypertabular.Get[float64](batch, 2, 1)
			check(fault != nil && fault.Reason == hypercast.Malformed && string(batch.Raw(2, 1)) == "x",
				"a cell's fault and its raw text")
			day, fault := hypertabular.Get[hypercast.Date](batch, 3, 0)
			check(fault == nil && day == hypercast.Date{Year: 2024, Month: time.February, Day: 29}, "Get[Date]")
			_, fault = hypertabular.Get[hypercast.Date](batch, 3, 1)
			check(fault != nil && fault.Reason == hypercast.OutOfRange, "an impossible date is out of range")
		}
		_, err = reader.Read()
		check(err == io.EOF, "delimited end")
		check(reader.Close() == nil, "delimited close")
	}

	// A read with notes off.
	plain, err := hypertabular.NewDelimitedReaderBytes(csv, hypertabular.CSV, []hypertabular.Column{hypertabular.Text(1)},
		hypertabular.Notes(false))
	check(err == nil, "a reader with notes off")
	if err == nil {
		batch, err := plain.Read()
		check(err == nil && string(batch.Text(0)[1]) == "bob" && batch.Origin(0, 1) == hypertabular.OriginCell, "a read with notes off")
	}

	// Declared: a skipped line, a token, a short row, latin-1, and what a batch says of them.
	declared := hypertabular.CSV
	declared.Skip = 1
	declared.ShortRows = hypertabular.ShortRowsPad
	declared.Encoding = hypertabular.Latin1
	declared.Tokens = hypertabular.NewTokens("NA")
	coded, err := hypertabular.NewDelimitedReaderBytes([]byte("a title\nn\xe9,v\nx,NA\ny\nz,12q\n"), declared, []hypertabular.Column{
		hypertabular.Text(0), hypertabular.I32(1, hypercast.Invariant),
	})
	check(err == nil, "a reader that declares its dialect")
	if err == nil {
		check(fmt.Sprint(coded.Header()) == "[n\u00e9 v]", "a decoded header")
		batch, err := coded.Read()
		check(err == nil && batch.Rows() == 3, "a declared read")
		if err == nil {
			check(batch.Origin(1, 0) == hypertabular.OriginToken && batch.Origin(1, 1) == hypertabular.OriginPadded, "a cell's origin")
			check(batch.Detail(1, 2).String() == "unexpected_character", "a cell's detail")
			listed := 0
			for problem := range batch.Faults() {
				listed++
				check(problem.Row == 2 && problem.SourceColumn == 1 && problem.Offset == 2, "a batch's fault")
			}
			check(listed == 1, "a batch's faults")
		}
	}

	found, err := hypertabular.SuggestDelimitedBytes([]byte("id,name\n7,bob\n"), hypertabular.CSV, hypertabular.SuggestOptions{})
	check(err == nil && len(found.Plan) == 2 && found.Report[1].Door == hypertabular.DoorText && found.Report[1].Status == hypertabular.SuggestResolved,
		"a suggested plan")

	torn, err := hypertabular.NewDelimitedReaderBytes([]byte("a,b\n1,\"open"), hypertabular.CSV,
		[]hypertabular.Column{hypertabular.Text(0)})
	if err == nil {
		_, err = torn.Read()
	}
	_, failed := err.(*hypertabular.Failure)
	check(failed, "an unterminated quote is a *Failure")

	book, err := hypertabular.NewWorkbook(xlsx())
	check(err == nil, "NewWorkbook")
	if err == nil {
		check(book.Format() == hypertabular.XLSX, "workbook format")
		sheets := book.Sheets()
		check(len(sheets) == 1 && sheets[0].Name == "Orders", "workbook sheets")
		sheet, err := book.SheetNamed("Orders", hypertabular.DefaultSheetOptions, []hypertabular.Column{
			hypertabular.I64(0, hypercast.Invariant),
			hypertabular.Text(1),
			hypertabular.DateOnly(2),
		})
		check(err == nil, "SheetNamed")
		if err == nil {
			check(fmt.Sprint(sheet.Header()) == "[id name day]", "sheet header from shared strings")
			batch, err := sheet.Read()
			check(err == nil && batch.Rows() == 1, "sheet read")
			if err == nil {
				check(batch.I64(0)[0] == 7, "a stored number")
				check(string(batch.Text(1)[0]) == "bob, jr", "a shared string")
				day, fault := hypertabular.Get[hypercast.Date](batch, 2, 0)
				check(fault == nil && day == hypercast.Date{Year: 2024, Month: time.February, Day: 29},
					"a date-formatted serial")
			}
			_, err = sheet.Read()
			check(err == io.EOF, "sheet end")
		}

		// A sheet with notes off, and one that declares a range.
		plainSheet, err := book.Sheet(0, hypertabular.DefaultSheetOptions, []hypertabular.Column{hypertabular.Text(1)}, hypertabular.Notes(false))
		check(err == nil, "a sheet with notes off")
		if err == nil {
			batch, err := plainSheet.Read()
			check(err == nil && string(batch.Text(0)[0]) == "bob, jr", "a read of a sheet with notes off")
		}
		cut, err := hypertabular.ParseRange("B1:C2")
		check(err == nil && cut.FirstColumn() == 2 && cut.LastRow() == 2, "a range")
		_, err = hypertabular.ParseRange("c2:b1")
		check(err != nil, "what is not a range")
		options := hypertabular.DefaultSheetOptions
		options.Range = cut
		ranged, err := book.Sheet(0, options, []hypertabular.Column{hypertabular.Text(0)})
		check(err == nil, "a sheet read through a range")
		if err == nil {
			check(fmt.Sprint(ranged.Header()) == "[name day]", "a range's header")
			batch, err := ranged.Read()
			check(err == nil && string(batch.Text(0)[0]) == "bob, jr", "a range's first column")
			suggested, err := ranged.Suggest(hypertabular.SuggestOptions{})
			check(err == nil && len(suggested.Plan) == 2 && *suggested.Report[0].Name == "name", "a sheet's suggestion")
		}
	}

	_, err = hypertabular.NewWorkbook([]byte("PK\x03\x04 not a zip at all"))
	check(err != nil, "a container that is not a workbook is an error")

	fmt.Println("DONE")
}
