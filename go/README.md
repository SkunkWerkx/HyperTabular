# HyperTabular for Go

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```
go get github.com/SkunkWerkx/HyperTabular/go@v0.7.0
```

The module lives in a subdirectory of the repository, so its tags are prefixed
(`go/vX.Y.Z`).

```go
import (
	// Both import paths end in /go, so the package names have to be spelled out.
	hypercast "github.com/SkunkWerkx/HyperCast/go"
	hypertabular "github.com/SkunkWerkx/HyperTabular/go"
)

// The dialect and the plan are declared; nothing is sniffed.
plan := []hypertabular.Column{
	hypertabular.I32(0, hypercast.Invariant),
	hypertabular.Text(1),
	hypertabular.Exact(2, hypercast.Invariant),
	hypertabular.DateOnly(3),
}
reader, err := hypertabular.OpenDelimited("orders.csv", hypertabular.CSV, plan)
if err != nil {
	log.Fatal(err)
}
defer reader.Close()

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

// A workbook reads into the same batch, a sheet at a time.
book, err := hypertabular.OpenWorkbook("orders.xlsx")
if err != nil {
	log.Fatal(err)
}
sheet, err := book.SheetNamed("Orders", hypertabular.DefaultSheetOptions, plan)
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
archives — Linux, macOS and Windows on x86-64 and arm64 — so a binary carries the core and
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
passes `-tags iossimulator` by hand. The simulator on an Intel Mac and Android have no
archive and are compile errors. CI checks the selection for every platform
(`.github/scripts/check_go_archives.sh`), runs this suite in an iOS simulator through Go's
`misc/ios/go_ios_exec.go`, and links a device build.

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

```
gh attestation verify staticlib/linux_amd64/libhypertabular.a --owner SkunkWerkx
```

## License

[MIT](../LICENSE)
