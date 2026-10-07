// Command tinygosmoke is the check CI runs in headless Chrome: this module compiled by TinyGo
// for the browser (`tinygo build -target=wasm`), with the core linked in from
// staticlib/wasm and HyperCast's beside it, loaded by index.html through TinyGo's own
// wasm_exec.js. It prints one PASS or FAIL line per check and DONE at the end; the forge's
// hyper-build-wasm.yml fails the run on any FAIL or a missing DONE, which is also what a
// trap part-way through looks like.
//
// Every export crosses at least once: the version, the delimited reader's five (unescape by
// a doubled quote), and the workbook's eight, through an XLSX written here in memory with
// archive/zip — deflated, so the core's inflate runs too, and with shared strings and a
// styles part, so opening it reads both. A browser holds bytes, not paths, so everything is
// read from memory, and a cell's fault and a torn file's error cross as well.
//
// The expected core version comes from rust/Cargo.toml at build time
// (`-ldflags "-X main.wantVersion=..."`), so the page proves the archive that was linked is
// the one this commit's core builds, not a stale one. Stock Go builds this too, natively
// under cgo, so `go vet ./...` covers it like any other package; it is internal, so it is
// nothing a consumer can import.
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
	}

	_, err = hypertabular.NewWorkbook([]byte("PK\x03\x04 not a zip at all"))
	check(err != nil, "a container that is not a workbook is an error")

	fmt.Println("DONE")
}
