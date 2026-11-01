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

## The shape

The core owns no memory and reads no files. The reader sizes the buffers — one value array
and one verdict array per column, the table that locates each cell — once, and the core
fills them in one call per batch: once per few thousand rows, not once per cell.

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

## The core

The Rust core is built for `wasm32-wasip1`, a module that imports nothing at all, and
[wasm2go](https://github.com/ncruces/wasm2go) translates it into one Go source file,
`internal/core/core.go`, committed and compiled with the rest of the package. Every call into
it is an ordinary Go call; nothing crosses a foreign boundary, nothing is loaded at run time,
and `NativeVersion()` reports the version compiled into that file from the crate's.

- **An instance a reader.** The core keeps a reader's position in a state block it is
  handed and its buffers between calls, so every `DelimitedReader` and every `Sheet` owns an
  instance of the translated module — its code's linear memory, three 64 KiB pages to
  start — with the reader's buffers allocated in it once and grown in place. Readers share
  nothing, so readers on different goroutines run at once, and none takes a lock. A
  `Workbook` opens in an instance it drops once its sheets, shared strings and styles are
  read; each `Sheet` copies the package and the shared strings into its own, so an open
  sheet costs that memory again. Everything is garbage like any other Go memory once its
  reader is; there is nothing to close beyond a file `OpenDelimited` opened.
- **What is copied.** Input goes in: text a chunk at a time — each byte about once, the
  chunk growing until it holds a batch, and a call cut short by the chunk's end made again
  over a larger one, so a batch from memory is still full — and a workbook's package once per
  instance. What the core writes comes out in place: on a little-endian host a column's
  values and verdicts, the cell table and the arena are views of the instance's memory, so a
  batch is still a view and `Text` still slices the input itself. A big-endian host
  (`s390x`, `ppc64`, `mips64`) decodes each field by field into buffers the reader keeps;
  the guest is little-endian whatever the host is.
- **4 GiB.** WebAssembly addresses 4 GiB, and a reader's buffers live in its instance, so a
  plan, a batch or a record too large for that is an error from the constructor, `Bind` or
  `Read` rather than a run-time fault: a 4,096-row batch of a few hundred columns is
  nowhere near it.
- **A panic is a bug, not a corruption.** wasm2go's code bounds-checks every memory access
  and keeps the call stack in Go, so a core bug, a guest stack overflow included, is a Go
  panic and never a corrupted heap; the reader it happened in refuses every call after it.
  The guest stack is 64 KiB, HyperCast's size and HyperUuid's.

wasm2go is pre-1.0 (v0.4.19 today) with one maintainer. It is pinned in a tool module of its
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
page loads one module. Under TinyGo, ranging over a batch's `All()` costs the three
allocations TinyGo makes for a range-over-func loop's closures, once a loop and never a row;
every other allocation count below holds there as under stock Go.

CI runs the whole suite three ways on every pull request that touches the Go binding, the
core or the workflows: under TinyGo on wasip1 under wasmtime (with the checkout mounted and
the corpus found through `HYPERTABULAR_CORPUS`, and the same binary run without the mount
as a negative control), and under stock Go on `js/wasm` under Node and on `wasip1/wasm`
under wasmtime. It also builds [`internal/tinygosmoke`](internal/tinygosmoke) with TinyGo
for the browser and runs it in headless Chrome: every export, delimited text and a workbook
written in memory, a fault with its span, and the core's version against `rust/Cargo.toml`.

## Allocations

A batch allocates nothing: every buffer is sized when the reader is built, or grown when the
core asks for room, and a `Read()` copies its input into the reader's instance, calls the
core once and hands out views. `allocs_test.go` holds it — a batch from memory and from a
stream, every column of it read, `Raw` (an escaped cell is unescaped into a buffer the
reader keeps), `Batch.All` and `reader.All()` across batches — with a fault (`Fault`, or
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
c`), then runs the wasm2go pinned in its own `go.mod`. The rustc is pinned as a
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
Apple M6 (Mac18,5, 12 cores), macOS 27.2, Go 1.27.3, 2026-11-01: six runs of each binding
interleaved in one session (the workbook benchmarks at three iterations a run), benchstat
medians with benchstat's spread; every difference below is significant (p = 0.002). The cgo
column is the binding this one replaced, at commit `93e0807` with its committed
`darwin_arm64` archive (the 1.0.0 core) and HyperCast's cgo module v1.0.0, run with this
same benchmark file. Other work shared the machine during the session.

**Every row is a loss.** The translated core is bounds-checked Go, and nearly all of a read's
time is in it (a CPU profile puts the binding's copying and decoding at a few percent), so
the read is 2–3.7x slower than the native core was. The batch shape is unchanged — still
one call per batch, still views, still no allocation a batch — but a reader that needs the
native speed should take one of the bindings that load the shared library.

| Benchmark | translated core | cgo binding | |
| --- | ---: | ---: | --- |
| Delimited, 300 000 rows × 8 columns (22.0 MB), from memory | 64.3 ms ±10% | 17.5 ms ±9% | **3.7x slower** |
| Delimited, the same, from a stream | 64.3 ms ±5% | 17.8 ms ±6% | **3.6x slower** |
| `excel-win-300k.xlsx`, open | 42.1 ms ±10% | 15.3 ms ±18% | **2.8x slower** |
| `excel-win-300k.xlsx`, read | 736 ms ±10% | 274 ms ±11% | **2.7x slower** |
| `libreoffice-300k.ods`, open | 384 ms ±10% | 201 ms ±6% | **1.9x slower** |
| `libreoffice-300k.ods`, read | 1.40 s ±8% | 627 ms ±6% | **2.2x slower** |
| `numbers-300k.numbers`, open | 991 ms ±9% | 505 ms ±10% | **2.0x slower** |
| `numbers-300k.numbers`, read | 1.43 s ±8% | 722 ms ±7% | **2.0x slower** |

Memory a read allocates grows too, since every byte the core reads or writes now lives in a
reader's instance as well as wherever it came from: the xlsx read allocates 126 MiB against
21.7 MiB, the Numbers read 253 MiB against 38.6 MiB (the package is copied into the
workbook's instance to open it and into the sheet's to read it), and a delimited read from
memory 7.9 MiB against 1.1 MiB. Allocation counts are unchanged, a few dozen a read and none a
batch.

The Go row of the "Through every binding" table in [`docs/workbook.md`](../docs/workbook.md)
**predates the translated core**: it measured the cgo binding on the Linux i9-11900H on
2026-10-24.

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
