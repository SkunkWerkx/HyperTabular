# HyperTabular for Swift

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell.

```swift
// Package.swift
.package(url: "https://github.com/SkunkWerkx/HyperTabular", from: "<version>"),
// and in the target:
.product(name: "HyperTabular", package: "HyperTabular"),
```

```swift
import HyperCast
import HyperTabular

let reader = try DelimitedReader(
    contentsOfFile: "orders.csv", dialect: .csv, plan: [.i32(0), .text(1), .f64(2)])
print(reader.header ?? [])                       // ["id", "name", "score"]

while let batch = try reader.read() {
    // A column at a time, as the core wrote it…
    let ids = batch.values(0, as: Int32.self)    // UnsafeBufferPointer<Int32>
    let verdicts = batch.verdicts(2)             // a CellVerdict beside each value

    // …or a cell at a time, as HyperCast's union.
    for row in 0..<batch.rows {
        switch batch.get(2, row: row, as: Double.self) {
        case .success(let score): print(batch.string(1, row: row) ?? "", score)
        case .fault(let fault): print("line \(batch.line(row)): \(fault.reason)")
        }
    }
}

// A workbook reads into the same batch.
let book = try Workbook(contentsOfFile: "orders.xlsx")
print(book.sheets)                               // [SheetInfo(name: "Orders", hidden: false)]
let sheet = try book.sheet(named: "Orders", plan: [.i32(0), .text(1), .f64(2)])
while let batch = try sheet.read() { /* … */ }
```

## The shape

The native core owns no memory and reads no files. `DelimitedReader` and `Sheet` allocate
the buffers — one value buffer and one verdict buffer per column, the table that locates
each cell — once, and the core fills them in one call per batch: the boundary is crossed
once per few thousand rows, not once per cell.

- **One batch type.** `read()` returns a `Batch` — `rows`, `columns`, `line(_:)`,
  `verdicts(_:)` and `values(_:as:)` for a whole column, `get(_:row:as:)` for any one cell
  as HyperCast's `Verdict`, `string`/`text` for text and `raw` for the text a cell was cast
  from — or `nil` when there are no more rows. It is a view of the reader's buffers, valid
  until the next `read()`, and the same type for delimited text and for a sheet.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  `SheetOptions` states a sheet's header, whether empty rows are skipped and the batch size;
  the plan states each column's door and, for numbers, its `NumFormat`.
- **HyperCast is the judge.** `Verdict`, `Fault`, `NumFormat`, `UnixPrecision`, `DateOrder`
  and `ExcelEpoch` are HyperCast's own types. A text cell means exactly what `Cast` would
  say of the same text; a typed workbook cell is converted by the door directly.
- **A bad value is a verdict; a broken file is an error.** A cell that does not cast is a
  fault in its column and the read goes on. A record of the wrong width, input that ends
  inside a quoted cell, a workbook whose container or parts cannot be read, throws
  `TabularError`, after every intact row before it.

`Tabular.isAvailable` and `Tabular.nativeVersion()` report the linked core.

## Linking and deployment

The core is linked into the executable as a static library on every platform — a SwiftPM
binary target, `HyperTabularCore.artifactbundle`, with an archive for Linux (glibc and
Swift's static musl SDK) and Windows on x86-64 and arm64 and for macOS on both
architectures — so there is nothing to deploy beside the executable. Swift 6.2 is the floor:
it is the first release whose package manager links a static library. HyperCast's Swift
package links its own core the same way, and the two link side by side.

iOS, the iOS simulator and Mac Catalyst get the same archives from a second binary target,
`HyperTabularCoreApple.xcframework`: an app for those is built by Xcode, which links a
static library out of an XCFramework and does not read a static-library artifact bundle.
The manifest declares it only on a Mac, and only when the XCFramework is in the tree; both
targets define the one `HyperTabularCore` module the binding imports. The floors are iOS 16
and Mac Catalyst 16. CI's `test-apple-mobile` job runs the suite on an iOS simulator and as
a Mac Catalyst process with `xcodebuild test`, and builds the package for an iOS device.

### WebAssembly

The binding compiles to WebAssembly from Swift 6.2 with swift.org's WebAssembly SDK:

```sh
swift sdk install <the Wasm SDK URL and checksum from swift.org/install>
swift build --swift-sdk swift-6.4.0-RELEASE_wasm     # `swift sdk list` names yours
```

The core is linked in from the artifact bundle's `wasm32-unknown-wasip1` archive, so there
is no module to load and no engine to embed, and the core asks nothing of the host. The
result is a plain `wasm32-wasip1` command module, which a browser runs through a WASI shim
such as [`@bjorn3/browser_wasi_shim`](https://github.com/bjorn3/browser_wasi_shim). CI runs
the suite under WasmKit on Swift 6.4, a smoke executable on 6.2, and the same smoke
executable in headless Chrome.

## Verifying provenance

SwiftPM resolves from the git tag, so the archives committed in the bundle are what a
consumer links. Each was attested when the forge built it and verified again before it was
committed:

```
gh attestation verify HyperTabularCore.artifactbundle/x86_64-unknown-linux-gnu/libhypertabular.a --owner SkunkWerkx
```

## License

[MIT](../LICENSE)
