# HyperTabular for Go

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```sh
go get github.com/SkunkWerkx/HyperTabular/go@latest
```

Requires Go 1.26 or later (the `go` directive in `go.mod`); no C compiler, and `CGO_ENABLED`
makes no difference. The module lives in a subdirectory of the repository, so its tags are
prefixed (`go/v<version>`). What `go get` fetches is source, the translated core included;
HyperCast's Go module (1.1.0 or later, cgo-free the same way) and `github.com/google/uuid`
are its dependencies.

The examples read this file, saved as `people.csv`; the second row's score does not cast,
so the fault path has something to print:

```csv
id,name,score
1,alice,2.5
2,bob,oops
3,carol,4.75
```

and `people.xlsx`, a workbook with one sheet named `People` holding the same table, header
row first (open `people.csv` in a spreadsheet app, rename the sheet and save it as `.xlsx`).

```go
package main

import (
	"fmt"
	"io"
	"log"

	// Both import paths end in /go, so the package names have to be spelled out.
	hypercast "github.com/SkunkWerkx/HyperCast/go"
	hypertabular "github.com/SkunkWerkx/HyperTabular/go"
)

func main() {
	// The dialect is declared; nothing is sniffed. Open header first, look the columns up by
	// name, and bind the plan built from them.
	reader, err := hypertabular.OpenDelimitedUnbound("people.csv", hypertabular.CSV)
	if err != nil {
		log.Fatal(err)
	}
	defer reader.Close()

	header := reader.Header() // a hypertabular.Header: a []string with lookups
	fmt.Println("header:", header)
	id, err := header.Ordinal("id") // a missing name is a *NoColumnError that names it
	if err != nil {
		log.Fatal(err)
	}
	name, err := header.Ordinal("name")
	if err != nil {
		log.Fatal(err)
	}
	score, err := header.Ordinal("score")
	if err != nil {
		log.Fatal(err)
	}
	plan := []hypertabular.Column{
		hypertabular.I32(id, hypercast.Invariant),
		hypertabular.Text(name),
		hypertabular.F64(score, hypercast.Invariant),
	}
	if err := reader.Bind(plan); err != nil {
		log.Fatal(err)
	}

	for {
		batch, err := reader.Read()
		if err == io.EOF {
			break
		}
		if err != nil {
			log.Fatal(err) // a *hypertabular.Failure: the input is not rows of cells
		}
		// A column at a time: typed slices, one element per row, and a verdict beside each…
		ids, names := batch.I32(0), batch.Text(1)
		fmt.Println("ids:", ids)
		faults := 0
		for _, verdict := range batch.Verdicts(2) {
			if !verdict.OK() {
				faults++
			}
		}
		fmt.Println("faults:", faults)
		for row := range batch.Rows() {
			// …or a cell at a time, as the HyperCast door of the same name returns it.
			value, fault := hypertabular.Get[float64](batch, 2, row)
			if fault != nil {
				fmt.Printf("line %d: %v in %q\n", batch.Line(row), fault.Reason, batch.Raw(2, row))
				continue
			}
			fmt.Printf("%s: %v\n", names[row], value)
		}
	}

	// A plan known up front goes straight to the constructor, and Bind is not called. Or
	// read row by row, across batches, with Go's range-over-func iterators.
	byPosition, err := hypertabular.OpenDelimited("people.csv", hypertabular.CSV, []hypertabular.Column{
		hypertabular.I32(0, hypercast.Invariant),
		hypertabular.Text(1),
		hypertabular.F64(2, hypercast.Invariant),
	})
	if err != nil {
		log.Fatal(err)
	}
	defer byPosition.Close()
	for row, err := range byPosition.All() {
		if err != nil {
			log.Fatal(err) // how the input ended, if not at its end
		}
		name := row.TextString(1) // a view of the batch's bytes, not a copy
		fmt.Printf("line %d: %s\n", row.Line(), name)
	}

	// A workbook reads into the same batch, a sheet at a time.
	book, err := hypertabular.OpenWorkbook("people.xlsx") // or NewWorkbook(bytes), NewWorkbookReader(r)
	if err != nil {
		log.Fatal(err)
	}
	var sheets []string
	for _, info := range book.Sheets() {
		sheets = append(sheets, info.Name)
	}
	fmt.Println("sheets:", sheets)
	sheet, err := book.SheetNamedUnbound("People", hypertabular.DefaultSheetOptions)
	if err != nil {
		log.Fatal(err)
	}
	name, err = sheet.Header().Ordinal("name")
	if err != nil {
		log.Fatal(err)
	}
	score, err = sheet.Header().Ordinal("score")
	if err != nil {
		log.Fatal(err)
	}
	if err := sheet.Bind([]hypertabular.Column{
		hypertabular.Text(name),
		hypertabular.Exact(score, hypercast.Invariant),
	}); err != nil {
		log.Fatal(err)
	}
	for row, err := range sheet.All() {
		if err != nil {
			log.Fatal(err)
		}
		value, fault := hypertabular.Cell[hypercast.Decimal](row, 1)
		if fault != nil {
			fmt.Printf("line %d: %v\n", row.Line(), fault.Reason)
			continue
		}
		fmt.Printf("%s: %v\n", row.TextString(0), value)
	}
}
```

A plan known up front goes straight to the constructor — `OpenDelimited(path, dialect, plan)`,
and likewise `NewDelimitedReader` and `NewDelimitedReaderBytes` — and `Bind` is not called.

`example_test.go` holds runnable examples of the same calls, with their output checked by
`go test`.

## A messier file

Everything about a file is still declared, on the `Dialect` and on the `SheetOptions`, whose zero
values read as before: lines to pass over, records to discard, what a record of the wrong width is,
the encoding, and what stands for no value.

```go
package main

import (
	"fmt"
	"log"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
	hypertabular "github.com/SkunkWerkx/HyperTabular/go"
)

func main() {
	// Windows-1252, with a title line above the table, NA for nothing and rows that stop short.
	dialect := hypertabular.CSV
	dialect.Skip = 1
	dialect.ShortRows = hypertabular.ShortRowsPad
	dialect.Encoding = hypertabular.Windows1252
	dialect.Tokens = hypertabular.NewTokens("NA")

	input := []byte("Exported by hand\nname,amount\ncaf\xe9,NA\ntea\nmilk,12x4\n")
	reader, err := hypertabular.NewDelimitedReaderBytes(input, dialect, []hypertabular.Column{
		hypertabular.Text(0),
		hypertabular.F64(1, hypercast.Invariant).WithTokens("NA", "-"), // in place of the dialect's
	})
	if err != nil {
		log.Fatal(err)
	}
	defer reader.Close()
	batch, err := reader.Read()
	if err != nil {
		log.Fatal(err)
	}
	for row := range batch.All() {
		fmt.Println(row.TextString(0), row.Verdict(1).Reason(), row.Origin(1), row.Detail(1))
	}
	// What went wrong, and only that: a blank, a token and a padded cell aren't problems.
	for problem := range batch.Faults() {
		fmt.Println("line", problem.Line, "column", problem.SourceColumn, problem.Reason, problem.Detail)
	}

	// A plan to look at, from a sample of the text. It reads nothing until it's handed to a read.
	found, err := hypertabular.SuggestDelimitedBytes(
		[]byte("id,total,day\n1,\"1,234.50\",2026-01-07\n2,12.00,2026-01-08\n"),
		hypertabular.CSV, hypertabular.SuggestOptions{})
	if err != nil {
		log.Fatal(err)
	}
	for _, column := range found.Report {
		fmt.Println(*column.Name, column.Status, column.Door, column.Matched, "of", column.Sampled)
	}

	// A sheet takes the part of it that's the table; its plan's ordinals count from B here.
	cut, err := hypertabular.ParseRange("B2:D40")
	if err != nil {
		log.Fatal(err)
	}
	options := hypertabular.DefaultSheetOptions
	options.Range = cut
	fmt.Println(options.Range.FirstColumn(), options.Range.LastRow())
}
```

A `*Failure` of kind `Encoding` comes after every intact row before it, and its `Byte` alone counts
the original bytes: a read's spans are all the decoded text's. A sheet's `Range` is parsed by the
core, so `ParseRange("A5:F200")` means what it means in every binding, with `NewRange` for what
that can't say, such as every row from the fifth. A plan's ordinals count from the range's first
column, while `Batch.Line` and a problem's `SourceColumn` stay the sheet's own.

`Notes(false)`, for a reader or a sheet, drops `Detail` and `Origin`, which then say `0` and
`OriginCell`: the read costs a little less, and `Faults` can't list a formula with no cached value.
`Column.WithDateOptions` hands a date door HyperCast's `DateOptions`, and on a sheet it and
`WithTokens` reach only a text cell.

Declaring the locale, a number format and a date order, is the recommended route, and then nothing
needs inferring. A suggestion is a starting point for a plan to inspect, and it's never applied. Read
the report first: only a resolved column reads through its door in the plan, the rest as text. With no
format declared, each of two notations has to read the whole column: `1,234,567` and `1,234.5` read
only with `,` grouping, `1.234.567`, `1.234,5`, `2,5` and `0,125` only with `,` as the decimal, and the
plan is in whichever is left. `1,234` and `1.234` read both ways, so they come back ambiguous and read
as text until a format is declared or another cell settles them, which takes in a column of prices at
three decimals; `SuggestOptions.DateOrder` does the same for `01/02/2026`. `2,5` beside `2.5` is
unresolved, and `1,23,456`, Indian grouping, is text until the column declares Indian grouping, because
nothing says its culture.

A `SampleRows` left at 0 is not set, so 1,000, and `SampleRows: 1` asks for one row. That differs from
Rust's `sample_rows(0)`, which is 1: a Go struct can't tell a field left out from an explicit 0, and
options that declare only a `Format` mustn't sample one row.

A
sheet's number cell has no notation, so a column of them is a number under any format, where text
holding the same digits asks. A column of `0` and `1` is bool, `007` and an integer past int64 are text,
and a typed date in a sheet is text, for a plan to change by hand. With nothing to
choose between, a resolved number is planned in `,` groups of three, and
`SuggestedColumn.Format` names what the plan uses: a later `1,23,456` is refused. A headerless file is
opened twice, so no suggestion is offered of a stream.

**To keep every row of a sheet, set `SkipEmptyRows` to `false`.** By default a row whose only cells
are formulas with no cached value is filtered out as empty, with nothing to say so; off, those rows are
delivered, their cells at `OriginUncached`.

A Go plan always names its format. `hypercast.Invariant` is Western: `1,234,567` is 1234567 and
`1,23,456` is `Malformed`, and a literal that sets no policy is the same. If a column of Indian numbers comes back refused, declare it, one column at a time, so an Indian
column reads beside a Western one in the same read:

```go
indian := hypercast.Invariant.WithGrouping(hypercast.GroupingIndian)
plan := []hypertabular.Column{
	hypertabular.I32(0, indian),              // 1,23,456 is 123456, and 1,234,567 is Malformed
	hypertabular.I32(1, hypercast.Invariant), // 1,234,567 is 1234567, and 1,23,456 is Malformed
}
```

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `hypercast.Detect` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `F64` reads `1,2345` as 1.2345, `12,34`
as 12.34 and `0,125` as 0.125. A format holding the retired
grouping bit `256` (the words `351` and `863`), which only a literal or a reflective decoder can make, is
an error when the plan is bound, before any cell, and in a suggest's options before it opens anything. Go has no locale bridge, so nothing reads a locale's
name. HyperCast's [grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape and the locales neither policy matches.

## The shape

The core owns no memory and reads no files. The reader sizes the buffers — one value array
and one verdict array per column, the table that locates each cell — once, and the core
fills them in one call per batch: once per few thousand rows, not once per cell.

- **One batch type.** `Read()` returns a `*Batch` — `Rows()`, `Columns()`, `Line(row)`,
  `Verdicts(column)`, a typed slice per door (`I32`, `F64`, `Exact`, `Timestamp`,
  `DateOnly`, `Text`, …), `Fault(column, row)`, `Raw(column, row)`, `Detail(column, row)`,
  `Origin(column, row)`, `Faults()` (a list ranged over past the next `Read()` panics) and the generic
  `Get[T](batch, column, row)` (a free function, since a Go method cannot take type
  parameters) — or `io.EOF` once there are no more rows. It is a view of the reader's
  buffers, valid until the next `Read()`, and the same type for delimited text and for a
  sheet. The integer, float and bool slices, `Exact`, `Uuid`, `TimeOfDay` and `Span` are the
  column as the core wrote it; `Timestamp`, `DateOnly` and `DateTime` are converted from it
  into a slice the reader keeps, and `Text` is sliced out of the input, each the first time
  a batch's column is asked for.
- **Header first, or plan first.** `OpenDelimitedUnbound`, `NewDelimitedReaderUnbound`,
  `NewDelimitedReaderBytesUnbound`, `Workbook.SheetUnbound` and `Workbook.SheetNamedUnbound`
  read the header and leave the plan to `Bind(plan)`, once, before the first `Read()` —
  which returns `ErrUnbound` until then, and a second `Bind` `ErrAlreadyBound`. `Header()`
  is a `Header`, a `[]string` underneath, whose `Ordinal(name)` / `OrdinalBytes` find the
  one column of that exact name (case and spaces included), return a `*NoColumnError` naming
  a name it lacks (`errors.Is(err, ErrNoColumn)`), and return an `*AmbiguousColumnError` naming
  both positions for a name two columns share (`errors.Is(err, ErrAmbiguousColumn)`). `Find` /
  `FindBytes` take the first of those and answer with an `ok` instead. `ColumnCount()` is a record's width: the header's, or the first record's once
  read. Once a header has fixed that width, a plan column reading at or past it is refused —
  by `Bind`, or by a constructor given the plan as soon as the header is read — with an
  error naming the plan column, since every record has exactly the header's cells. Any plan,
  delimited or a sheet's, is refused the same way for an ordinal of 2^20 (1,048,576) or more:
  no workbook format has that many columns, and the cap keeps one row of cell table within
  8 MiB.
- **Rows, when that is the shape.** `batch.All()` is an `iter.Seq[Row]` over the batch, and
  `reader.All()` / `sheet.All()` an `iter.Seq2[Row, error]` that reads batch after batch.
  A `Row` — `Index()`, `Line()`, `Verdict(column)`, `Fault(column)`, `Detail(column)`,
  `Origin(column)`, `Text(column)`, `TextString(column)`, `Raw(column)`, and
  `Cell[T](row, column)` — is a (batch, index) pair
  and allocates nothing; like the batch, it is valid until the next read. A `batch.All()` ranged
  over past the next `Read()` or a `Close()`, and a `reader.All()` or `sheet.All()` with a `Read()`
  or a `Close()` in its loop's body, panic as a stale `Faults()` does, rather than yield the next
  batch's rows.
- **The source can't call back in.** The `io.Reader` a `DelimitedReader` reads runs in the
  middle of its `Read()`, and a `Read()`, `Bind` or `Close()` it calls on that same reader returns
  `ErrBusy` (`the reader is in the middle of a read; its source can't call back into it`),
  checked with `errors.Is`. The outer `Read()` goes on unharmed, and the reader is there to
  use, and to close, once it returns. A source that panics leaves no read in progress. Other
  readers and sheets are there to use. A sheet reads memory, so nothing of the caller's runs
  inside one of its calls.
- **Text as a string, without the copy.** `batch.TextString(column, row)` and
  `row.TextString(column)` are the cell's bytes as a `string` made with `unsafe.String`: no
  allocation, ready for `strconv`, a map lookup or a `switch`. The bytes are the reader's and
  are reused by the next `Read()`, so a string kept past it changes under you — clone what
  you keep (`strings.Clone`, or `Get[string]`, which copies).
- **Cancellation is the source's.** A read blocks only in the `io.Reader`'s own `Read`; a
  source tied to a `context.Context` — an `http.Request` made with one, a connection whose
  deadline is set — ends with the context's error, and `Read()` returns it wrapped.
  Everything after the bytes arrive is in memory and short. A loop over `All()` stops at a
  `break`.
- **The core decides how much a batch may ask for.** `BatchRows(0)`, `BufferBytes(0)` and a batch past
  4 GiB (214 million rows of one `I32` column, 1.8 million of 100 text columns) are errors from the
  core, when a reader or a sheet opens or a plan is bound, before anything is allocated. A
  `BufferBytes` past 1 GiB starts there. An option left out has its default. A `SheetOptions` is a
  struct, which can't tell 0 from a field left out, so `SheetOptions{BatchRows: 0}` is the default
  4,096; to say 0 is to say `BatchRows(0)`, which is refused. The translated core addresses 4 GiB of
  its own, so a batch just under the limit can still be refused there, and Go's own arrays for
  timestamp, date, date-time and text columns are beyond what the core counts: a text or timestamp
  column takes 24 bytes a row more, so the 178,606,080 rows of one text column the core allows hold
  3.99 GiB in the core and 3.99 GiB more in Go. A `BufferBytes(0)` is refused on a sheet too, though a
  sheet has no buffer for it to set. `Suggest` is never refused for the size of its own batches: the core
  says what fits and the sampler reads as many as it takes. A row that alone is past the limit is an
  error naming its width.
- **HyperCast is the judge.** `hypercast.Fault`, `Detail`, `NumFormat`, `DateOptions`, `Decimal`, `Date`,
  `CivilDateTime`, `Duration`, `UnixPrecision`, `DateOrder` and `ExcelEpoch` are HyperCast's
  own types. A text cell means exactly what HyperCast's door would say of the same text; a
  typed workbook cell is converted by the door directly.
- **A bad value is a verdict; a broken file is an error.** A cell that does not cast is a
  fault in its column and the read goes on, and `Batch.Faults` lists a batch's. A record of the wrong width, input that ends
  inside a quoted cell, a workbook whose container or parts cannot be read, is a
  `*hypertabular.Failure`, returned after every intact row before it.

## The core

The Rust core is built for `wasm32-wasip1`, a module that imports nothing at all, and
[wasm2go](https://github.com/ncruces/wasm2go) translates it into one Go source file,
`internal/core/core.go`, committed and compiled with the rest of the package. Every call into
it is an ordinary Go call; nothing crosses a foreign boundary, nothing is loaded at run time,
and `NativeVersion()` reports the version compiled into that file from the crate's.

- **An instance a reader.** The core keeps a reader's position in a state block it is
  handed and its buffers between calls, so every `DelimitedReader` and every `Sheet` owns an
  instance of the translated module — its code's linear memory, four 64 KiB pages to
  start — with the reader's buffers allocated in it once and grown in place. Readers share
  nothing, so readers on different goroutines run at once, and none takes a lock. A
  `Workbook` opens in an instance it drops once its sheets, shared strings and styles are
  read; each `Sheet` copies the package and the shared strings into its own, so an open
  sheet costs that memory again. Everything is garbage like any other Go memory once its
  reader is; there is nothing to close beyond a file `OpenDelimited` opened.
- **What is copied.** Input goes in: text a chunk at a time — each byte about once, the
  chunk growing until it holds a batch, and a call cut short by the chunk's end made again
  over a larger one, so a batch from memory is still full — and a workbook's package once per
  instance. Text in a declared encoding is decoded through the instance into a buffer of the
  reader's, so its `Text` slices that, even from memory. What the core writes comes out in place: on a little-endian host a column's
  values and verdicts, the cell table and the arena are views of the instance's memory, so a
  batch is still a view and `Text` still slices the input itself. A big-endian host
  (`s390x`, `ppc64`, `mips64`) decodes each field by field into buffers the reader keeps;
  the guest is little-endian whatever the host is.
- **4 GiB.** WebAssembly addresses 4 GiB, and a reader's buffers live in its instance, so a
  plan, a batch or a record too large for that is an error from the constructor, `Bind` or
  `Read` rather than a run-time fault: a 4,096-row batch of a few hundred columns is
  nowhere near it.
- **A panic is a bug, not a corruption.** wasm2go's code bounds-checks every memory access
  ([`internal/core/bounds_test.go`](internal/core/bounds_test.go) pins the checks) and keeps the call stack in Go, so a core bug, a guest stack overflow included, is a Go
  panic and never a corrupted heap; the reader it happened in refuses every call after it.
  The guest stack is 64 KiB, HyperCast's size and HyperUuid's.

wasm2go is pre-1.0 (v0.4.20 today) with one maintainer. It is pinned in a tool module of its
own, [`internal/wasm2go/`](internal/wasm2go), so it never enters your module graph, and
because its output is committed, a dead wasm2go would cost this repository its regeneration
step and cost a consumer nothing.

## Platforms

Every GOOS and GOARCH Go has, with `CGO_ENABLED=0`, because the core is Go source. CI
cross-compiles and vets the package for these 23 pairs on every pull request that touches
the Go binding, the core or the workflows, and on every release-mode run:

| GOOS | GOARCH |
| --- | --- |
| `linux` | `amd64`, `386`, `arm`, `arm64`, `riscv64`, `ppc64le`, `s390x`, `loong64`, `mips64le` |
| `windows` | `amd64`, `arm64`, `386` |
| `darwin` | `amd64`, `arm64` |
| `freebsd`, `openbsd`, `netbsd`, `illumos`, `plan9` | `amd64` |
| `aix` | `ppc64` |
| `android` | `arm64` |
| `js`, `wasip1` | `wasm` |

and runs the whole suite natively on Linux and Windows on x64 and arm64, on macOS on arm64,
on Alpine (musl x64 and arm64), in an iOS simulator, in an Android emulator, and on the three
WebAssembly hosts below. No big-endian host runs in CI; the suite runs the corpus through the
big-endian decoding on every host as well (`TestTheCorpusThroughDecodedCopies`), and
`TestDecodingForABigEndianHost` holds that decoding to big-endian order. A binary carries the
core like any package and starts without touching the filesystem.

### iOS and Mac Catalyst

`GOOS=ios GOARCH=arm64` builds the package as it builds any other. What an iOS executable
needs is Xcode's linker, because Go's `ios/arm64` port links externally; that is Go's rule
for the platform, nothing of this binding's, and it is what
[`gomobile`](https://pkg.go.dev/golang.org/x/mobile/cmd/gomobile) arranges. For the
simulator, Go's own `misc/ios/clangwrap.sh` is the `CC`:

```sh
CGO_ENABLED=1 CC="$(go env GOROOT)/misc/ios/clangwrap.sh" GOOS=ios GOARCH=arm64 go build ./...
```

The device, the simulator and Mac Catalyst build the same package; nothing here selects on a
tag. CI runs this suite in an iOS simulator through Go's `misc/ios/go_ios_exec.go`, with the
corpus staged as `testdata/corpus` since that wrapper carries nothing above the module into
the simulator, and builds it for a device. `ExampleWorkbook` is skipped there, since it opens
`../corpus` by its path.

### Android

`GOOS=android GOARCH=arm64 go build ./...` needs no NDK: Go links it internally.
`android/amd64` (the emulator) is a platform Go itself links externally, so it takes the
NDK's clang as `CC` with `CGO_ENABLED=1`, as any Go program for it does; nothing of the
core's is linked either way. CI cross-builds this whole suite for `android/amd64` and runs it
in an x86_64 emulator whose image uses 16 KB pages (`.github/scripts/android_build_suite.sh`
and `android_device_test.sh`, which run the same way against a local emulator);
`android/arm64` is built in the cross-compile sweep. `ExampleWorkbook` is skipped there, as
in the simulator.

### WebAssembly

The translated core is plain Go, so it builds for the browser and for WASI under stock Go
and TinyGo alike, with no archive and no cgo:

```sh
GOOS=js GOARCH=wasm go build -o main.wasm .          # stock Go, the browser or Node
cp "$(go env GOROOT)/lib/wasm/wasm_exec.js" .

tinygo build -target=wasm -no-debug -opt=z -o main.wasm .   # TinyGo, the browser
cp "$(tinygo env TINYGOROOT)/targets/wasm_exec.js" .
```

Use the `wasm_exec.js` of the toolchain that built the module. `GOOS=wasip1 GOARCH=wasm` and
`tinygo build -target=wasip1` do the same for a WASI runtime. The core imports nothing, so a
page loads one module. Under TinyGo, ranging over a batch's `All()` costs five allocations, the
three TinyGo makes for a range-over-func loop's closures and two for the generation it captures, and
`Faults()` four, once a loop and never a row or an entry; every other allocation count below holds there as under stock Go.

CI runs the whole suite three ways on every pull request that touches the Go binding, the
core or the workflows: under TinyGo on wasip1 under wasmtime (with the checkout mounted and
the corpus found through `HYPERTABULAR_CORPUS`, and the same binary run without the mount
as a negative control), and under stock Go on `js/wasm` under Node and on `wasip1/wasm`
under wasmtime. It also builds [`internal/tinygosmoke`](internal/tinygosmoke) with TinyGo
for the browser and runs it in headless Chrome: every export a read takes, delimited text and a
workbook written in memory, a fault with its span, and the core's version against `rust/Cargo.toml`.

## Allocations

A batch allocates nothing: every buffer is sized when the reader is built, or grown when the
core asks for room, and a `Read()` copies its input into the reader's instance, calls the
core once and hands out views. `allocs_test.go` holds it — a batch from memory and from a
stream, every column of it read, each cell's `Detail` and `Origin`, `Raw` (an escaped cell is
unescaped into a buffer the reader keeps), a walk of `Faults` however long it is, `Batch.All` and
`reader.All()` across batches — with a fault (`Fault`, or
`Get`/`Cell` on a cell that did not cast) the one allocation, HyperCast's `*Fault`.

## Building and regenerating

Nothing to build: `go test ./...` runs on the committed core. The core is regenerated only
after a change to `rust/` or to the pins:

```sh
rustup toolchain install 1.99.1 --profile minimal --target wasm32-wasip1
go generate     # in go/: builds rust/ for wasm32-wasip1 and rewrites internal/core/core.go
```

[`internal/wasm2go`](internal/wasm2go) is the generator: it builds the crate's `cargo cdylib`
with the rustc named in its `rust-toolchain.toml` (1.99.1), a 64 KiB guest stack
(`-C link-arg=-zstack-size=65536`) and wasi-libc for `memcmp`, `memcpy` and `memset` (`-l
c`), then runs the wasm2go pinned in its own `go.mod` with `-unsafe`, which replaces the memory helpers with `unsafe.Add` access behind a check of
the access's last byte (CPUs without unaligned access keep the `encoding/binary` path). The rustc is pinned as a
reproducibility constraint, not a floor: the wasm's bytes follow rustc's codegen, so one named
toolchain makes the committed file reproducible, and a bump is a deliberate
regenerate-and-commit. `HYPERTABULAR_CHECK_CORE=1 go test -count=1 -run
'TestCoreIsCurrent|TestRegenerationCarriesTheCrateVersion' .` regenerates into a temporary
file and diffs it against the committed one (without the variable the test skips when the
toolchain is missing); CI runs it on every pull request that touches the Go binding, the
core or the workflows, and release preparation regenerates the core at the new version,
since the version is a literal in the file.

## Benchmarks

`go test -run '^$' -bench 'Delimited|Workbook' -benchmem` (the workbook files come from
`corpus/generate/out/` or `HYPERTABULAR_BENCH_DIR`; `docs/workbook.md` has what each does).
Apple M6 (Mac18,5, 12 cores), macOS 27.2, Go 1.27.3, 2026-11-08: six runs of each column
interleaved in one session (the workbook benchmarks at three iterations a run), benchstat
medians with benchstat's spread; every difference from this binding is significant (p = 0.002).
Load average 2.5 to 3.1. Two cgo columns: the binding this one replaced, at commit `93e0807`
with HyperCast's cgo module v1.0.0 and this same benchmark file, linked first against its
committed `darwin_arm64` archive (the 1.0.0 core), then against this checkout's core (the same
archive built from `rust/`, its crate object alone, as the release packs it). That binding calls
1.0's fills, so both cgo columns read without notes, where this binding reads with them: the
ratio in the last column is the translated core with notes against native without, and the
notes are part of what it counts. It also checked the state block's size for equality; this core
reports the larger block `_init_with` needs, so for the third column the check was relaxed to "at
least", which is all `hypertabular_delimited_init` asks.

**Every row is a loss.** The translated core is bounds-checked Go, and nearly all of a read's
time is in it (a CPU profile puts the binding's copying and decoding at a few percent), so
the read is 1.7–2.3x slower than the native core, before the quote fix and after it. The batch
shape is unchanged — still one call per batch, still views, still no allocation a batch — but a
reader that needs the native speed should take one of the bindings that load the shared library.

| Benchmark | translated core | cgo, 1.0 core | cgo, this core | |
| --- | ---: | ---: | ---: | --- |
| Delimited, 300 000 rows × 8 columns (22.0 MB), from memory | 38.0 ms ±1% | 16.4 ms ±1% | 17.0 ms ±1% | **2.2x slower** |
| Delimited, the same, from a stream | 37.6 ms ±2% | 16.7 ms ±1% | 17.2 ms ±1% | **2.2x slower** |
| `excel-win-300k.xlsx`, open | 28.8 ms ±15% | 14.2 ms ±2% | 14.2 ms ±10% | **2.0x slower** |
| `excel-win-300k.xlsx`, read | 483 ms ±1% | 259 ms ±2% | 269 ms ±1% | **1.8x slower** |
| `libreoffice-300k.ods`, open | 380 ms ±1% | 191 ms ±2% | 191 ms ±2% | **2.0x slower** |
| `libreoffice-300k.ods`, read | 1.17 s ±1% | 586 ms ±1% | 600 ms ±1% | **1.9x slower** |
| `numbers-300k.numbers`, open | 823 ms ±1% | 466 ms ±1% | 469 ms ±1% | **1.8x slower** |
| `numbers-300k.numbers`, read | 1.14 s ±1% | 683 ms ±1% | 686 ms ±1% | **1.7x slower** |

The last column compares this binding with the cgo binding over this core. In the same session,
the cgo binding read 3.7% slower over this core than over 1.0's on delimited text from memory
(17.0 against 16.4 ms) and 0.4% to 3.9% slower on the workbook reads.

Memory a read allocates grows too, since every byte the core reads or writes now lives in a
reader's instance as well as wherever it came from: the xlsx read allocates 128 MiB against
21.7 MiB, the Numbers read 254 MiB against 38.6 MiB (the package is copied into the
workbook's instance to open it and into the sheet's to read it), and a delimited read from
memory 11.5 MiB against 1.1 MiB. The cgo binding allocates the same over either core. Allocation
counts stay a few dozen a read and none a batch.

`-unsafe` is most of what closed the gap. The same wasm translated with and without it, six
interleaved runs in one session on the same day, gives delimited from memory 34.4 ms against 61.3
ms, the xlsx read 485 ms against 700 ms, the ods read 1.16 s against 1.33 s and the Numbers read
1.13 s against 1.31 s. The ods open is level, and memory and allocations don't move.

The Go row of the "Through every binding" table in [`docs/workbook.md`](../docs/workbook.md)
is this binding, measured in the same session as the table above.

## Verifying build provenance

Go has no package registry to attest, and this module ships no binary: `go get` resolves
straight from the `go/v<version>` git tag against this repo, and what it fetches is source,
the generated core included. That file's provenance is reproducible generation, claimed
exactly: `internal/core/core.go` is the deterministic output of wasm2go over the crate at that
tag built with the pinned rustc, CI regenerates it with those pins and fails on any diff, and
anyone can repeat it with `go generate` in a checkout
([Building and regenerating](#building-and-regenerating)). No attestation is claimed for the
intermediate wasm, which is not a release asset. The root README's
[Provenance](../README.md#provenance) section has the picture for the artifacts the release
does ship: the inventory, which are signed from where, and the gates every byte passes before
publication.

## License

[MIT](../LICENSE)
