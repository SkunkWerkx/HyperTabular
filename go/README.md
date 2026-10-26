# HyperTabular for Go

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```sh
go get github.com/SkunkWerkx/HyperTabular/go@latest
```

The module lives in a subdirectory of the repository, so its tags are prefixed
(`go/v<version>`).

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

## The shape

The native core owns no memory and reads no files. The reader allocates the buffers — one
value slice and one verdict slice per column, the table that locates each cell — once, and
the core fills them in one cgo call per batch: the boundary is crossed once per few thousand
rows, not once per cell.

- **One batch type.** `Read()` returns a `*Batch` — `Rows()`, `Columns()`, `Line(row)`,
  `Verdicts(column)`, a typed slice per door (`I32`, `F64`, `Exact`, `Timestamp`,
  `DateOnly`, `Text`, …), `Fault(column, row)`, `Raw(column, row)` and the generic
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
  first column of that exact name (case and spaces included) or return a `*NoColumnError`
  naming it (`errors.Is(err, ErrNoColumn)`), and whose `Find` / `FindBytes` answer with an
  `ok` instead. `ColumnCount()` is a record's width: the header's, or the first record's once
  read. Once a header has fixed that width, a plan column reading at or past it is refused —
  by `Bind`, or by a constructor given the plan as soon as the header is read — with an
  error naming the plan column, since every record has exactly the header's cells. Any plan,
  delimited or a sheet's, is refused the same way for an ordinal of 2^20 (1,048,576) or more:
  no workbook format has that many columns, and the cap keeps one row of cell table within
  8 MiB.
- **Rows, when that is the shape.** `batch.All()` is an `iter.Seq[Row]` over the batch, and
  `reader.All()` / `sheet.All()` an `iter.Seq2[Row, error]` that reads batch after batch.
  A `Row` — `Index()`, `Line()`, `Verdict(column)`, `Fault(column)`, `Text(column)`,
  `TextString(column)`, `Raw(column)`, and `Cell[T](row, column)` — is a (batch, index) pair
  and allocates nothing; like the batch, it is valid until the next read.
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
- **HyperCast is the judge.** `hypercast.Fault`, `NumFormat`, `Decimal`, `Date`,
  `CivilDateTime`, `Duration`, `UnixPrecision`, `DateOrder` and `ExcelEpoch` are HyperCast's
  own types. A text cell means exactly what HyperCast's door would say of the same text; a
  typed workbook cell is converted by the door directly.
- **A bad value is a verdict; a broken file is an error.** A cell that does not cast is a
  fault in its column and the read goes on. A record of the wrong width, input that ends
  inside a quoted cell, a workbook whose container or parts cannot be read, is a
  `*hypertabular.Failure`, returned after every intact row before it.

## Linking

The core is linked in through cgo from the module's own `staticlib/{GOOS}_{GOARCH}`
archives — Linux, macOS, Windows and [Android](#android) on x86-64 and arm64, and
[iOS and Mac Catalyst](#ios-and-mac-catalyst) — so a binary carries the core and
loads nothing. Building takes a C compiler; `CGO_ENABLED=0` or any other target is a
compile error rather than a binary that fails at run time. HyperCast's Go module links its
own core the same way, and the two archives link side by side.

### iOS and Mac Catalyst

Go builds for all three as `GOOS=ios`, with cgo and the platform's own clang. A Mach-O
object says which platform it was built for and the linker refuses a mismatch, so each has
its own archive, and since the three are one `GOOS`/`GOARCH` pair to Go, build tags choose
between them:

| Building for | Tags | Archive |
| --- | --- | --- |
| An iOS device | none | `staticlib/ios_arm64` |
| The iOS simulator, Apple silicon | `iossimulator` | `staticlib/iossimulator_arm64` |
| Mac Catalyst | `maccatalyst` | `staticlib/maccatalyst_arm64`, `staticlib/maccatalyst_amd64` |

`gomobile` sets `maccatalyst` itself; nothing sets `iossimulator`, so a simulator build
passes `-tags iossimulator` by hand. The simulator on an Intel Mac has no archive and is a
compile error. CI checks the selection for every platform
(`.github/scripts/check_go_archives.sh`), runs this suite in an iOS simulator through Go's
`misc/ios/go_ios_exec.go`, and links a device build.

### Android

`GOOS=android` with cgo, the NDK's clang as `CC`, as `gomobile` arranges and as any cgo
package on Android needs:

```shell
CC=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang \
  GOOS=android GOARCH=arm64 CGO_ENABLED=1 go build ./...
```

(`x86_64-linux-android21-clang` and `GOARCH=amd64` for the emulator.) `GOOS=android` also
satisfies Go's `linux` constraint, so the Linux link lines exclude it and it takes its own
archives, `staticlib/android_arm64` and `staticlib/android_amd64`, built for Android against
Bionic (API 21+). HyperCast's Go module does the same for its own core, and the two link
side by side. Page alignment is the final link's: NDK r28 and later align to the 16 KB pages
Android 15 devices may use by default, and an older NDK needs
`-extldflags=-Wl,-z,max-page-size=16384`.

CI checks that both select their own archive (`.github/scripts/check_go_archives.sh`),
cross-compiles this whole suite for `android/amd64`, and runs it in an x86_64 emulator whose
image uses 16 KB pages (`.github/scripts/android_build_suite.sh` and
`android_device_test.sh`, which run the same way against a local emulator); `android/arm64`
is linked. `ExampleWorkbook` is skipped there, since it opens `../corpus` by its path and
nothing is above the directory the suite is pushed to.

### In the browser (TinyGo)

[TinyGo](https://tinygo.org) 0.42 or later compiles this module to WebAssembly with the core
linked in: TinyGo links with `wasm-ld` and has cgo, so `backend_tinygo.go` names
`staticlib/wasm/libhypertabular.a` — the core built for `wasm32-wasip1` — on its link line.
HyperCast's module does the same with its own archive, and the two link side by side.

```sh
tinygo build -target=wasm -no-debug -opt=z -o main.wasm .
cp "$(tinygo env TINYGOROOT)/targets/wasm_exec.js" .
```

Use TinyGo's own `wasm_exec.js`, not stock Go's. The core imports nothing, so the page loads
one module. `-target=wasip1` works the same way under a WASI runtime. CI builds
[`internal/tinygosmoke`](internal/tinygosmoke) this way on every pull request and runs it in
headless Chrome: every export, delimited text and a workbook written in memory, a fault
with its span, and the core's version against `rust/Cargo.toml`. Stock Go compiled to
WebAssembly (`GOOS=wasip1`, `GOOS=js`) links Go code only, so it is a compile error.

## Verifying build provenance

Go has no package registry to attest: `go get` resolves straight from the `go/v<version>` git
tag against this repo, and what it fetches is the tree at that tag, the static libraries under
`go/staticlib/` included. Each of them verifies from the release tag alone: the release
attestation, signed by this repository's `release.yml` at the root tag, names every one of
them, with the release inventory as its predicate. The same file sits in the module cache at
`$(go env GOMODCACHE)/github.com/!skunk!werkx/!hyper!tabular/go@v<version>/staticlib/`.

```sh
gh attestation verify go/staticlib/linux_amd64/libhypertabular.a --repo SkunkWerkx/HyperTabular \
  --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Each archive also carries build provenance from `hyper-build-static.yml` in
`SkunkWerkx/.github`, at the preparation commit the tagged commit was staged from
(`stage-native-binaries.yml` verifies that before committing a byte, and the staging commit's
`Release-Source-SHA` and `Release-CI-Run-ID` trailers name the commit and the run), so that
check needs `--signer-repo` beside `--repo`:

```sh
gh attestation verify go/staticlib/linux_amd64/libhypertabular.a \
  --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
```

Get the signer wrong and `gh` reports a bare `verifying with issuer "sigstore.dev"`, which
reads like a bad signature but is only an identity mismatch. The root README's
[Provenance](../README.md#provenance) section has the whole picture: the inventory, which
artifacts are signed from where, and the gates every byte passes before publication.

## License

[MIT](../LICENSE)
