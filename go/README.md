# HyperTabular for Go

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```sh
go get github.com/SkunkWerkx/HyperTabular/go@latest
```

The module lives in a subdirectory of the repository, so its tags are prefixed
(`go/vX.Y.Z`).

```go
import (
	// Both import paths end in /go, so the package names have to be spelled out.
	hypercast "github.com/SkunkWerkx/HyperCast/go"
	hypertabular "github.com/SkunkWerkx/HyperTabular/go"
)

// The dialect is declared; nothing is sniffed. Open header first, look the columns up by
// name, and bind the plan built from them.
reader, err := hypertabular.OpenDelimitedUnbound("orders.csv", hypertabular.CSV)
if err != nil {
	log.Fatal(err)
}
defer reader.Close()

header := reader.Header() // a hypertabular.Header: a []string with lookups
id, err := header.Ordinal("id") // a missing name is a *NoColumnError that names it
if err != nil {
	log.Fatal(err)
}
customer, _ := header.Ordinal("customer")
total, _ := header.Ordinal("total")
plan := []hypertabular.Column{
	hypertabular.I32(id, hypercast.Invariant),
	hypertabular.Text(customer),
	hypertabular.Exact(total, hypercast.Invariant),
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
	ids, customers := batch.I32(0), batch.Text(1)
	for row := 0; row < batch.Rows(); row++ {
		// …or a cell at a time, as the HyperCast door of the same name returns it.
		total, fault := hypertabular.Get[hypercast.Decimal](batch, 2, row)
		if fault != nil {
			fmt.Printf("%d %s: %q is %v\n", ids[row], customers[row], batch.Raw(2, row), fault.Reason)
			continue
		}
		fmt.Println(ids[row], string(customers[row]), total)
	}
}
```

A plan known up front goes straight to the constructor — `OpenDelimited(path, dialect, plan)`,
and likewise `NewDelimitedReader` and `NewDelimitedReaderBytes` — and `Bind` is not called.

Or read row by row, across batches, with Go's range-over-func iterators:

```go
for row, err := range reader.All() {
	if err != nil {
		log.Fatal(err) // how the input ended, if not at its end
	}
	id, fault := hypertabular.Cell[int32](row, 0)
	name := row.TextString(1) // a view of the batch's bytes, not a copy
	…
}
```

A workbook reads into the same batch, a sheet at a time:

```go
book, err := hypertabular.OpenWorkbook("orders.xlsx") // or NewWorkbook(bytes), NewWorkbookReader(r)
if err != nil {
	log.Fatal(err)
}
sheet, err := book.SheetNamedUnbound("Orders", hypertabular.DefaultSheetOptions)
if err != nil {
	log.Fatal(err)
}
id, err = sheet.Header().Ordinal("id")
…
err = sheet.Bind(plan)
```

`example_test.go` holds these as runnable examples, with their output checked by `go test`.

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
  sheet.
- **Header first, or plan first.** `OpenDelimitedUnbound`, `NewDelimitedReaderUnbound`,
  `NewDelimitedReaderBytesUnbound`, `Workbook.SheetUnbound` and `Workbook.SheetNamedUnbound`
  read the header and leave the plan to `Bind(plan)`, once, before the first `Read()` —
  which returns `ErrUnbound` until then, and a second `Bind` `ErrAlreadyBound`. `Header()`
  is a `Header`, a `[]string` underneath, whose `Ordinal(name)` / `OrdinalBytes` find the
  first column of that exact name (case and spaces included) or return a `*NoColumnError`
  naming it (`errors.Is(err, ErrNoColumn)`), and whose `Find` / `FindBytes` answer with an
  `ok` instead. `ColumnCount()` is a record's width: the header's, or the first record's once
  read.
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

Go has no package registry to attest; `go get` resolves straight from the `go/vX.Y.Z` tag.
Each committed archive was attested when the forge built it and verified again before it was
committed, and can be checked against the build that made it:

```sh
gh attestation verify staticlib/linux_amd64/libhypertabular.a --owner SkunkWerkx
```

## License

[MIT](../LICENSE)
