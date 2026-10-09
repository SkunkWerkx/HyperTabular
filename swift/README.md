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

HyperCast comes along as HyperTabular's dependency, so `import HyperCast` works without
listing it. To name its product in your own target as well, add HyperCast to your package's
`dependencies` too; SwiftPM only resolves a `.product(name:package:)` for a package listed
there.

```swift
import HyperCast
import HyperTabular

// Open, read the header, and build the plan from the names it declares.
let reader = try DelimitedReader(contentsOfFile: "orders.csv", dialect: .csv)
let header = reader.header!                      // ["id", "name", "score"]
try reader.bind([
    .i32(header.ordinal(of: "id")),              // throws NoSuchColumn("id") if it is missing
    .text(header.ordinal(of: "name")),
    .f64(header.ordinal(of: "score")),
])

while let batch = try reader.read() {
    // A column at a time, as the core wrote it…
    let ids = batch.values(0, as: Int32.self)    // UnsafeBufferPointer<Int32>
    let verdicts = batch.verdicts(2)             // a CellVerdict beside each value

    // …or a row at a time, each cell as HyperCast's union.
    for row in batch {
        switch row.get(2, as: Double.self) {
        case .success(let score): print(row.string(1) ?? "", score)
        case .fault(let fault): print("line \(row.line): \(fault.reason)")
        }
    }
}

// Columns known by position: the plan goes in with the opening.
let byPosition = try DelimitedReader(
    contentsOfFile: "orders.csv", dialect: .csv, plan: [.i32(0), .text(1), .f64(2)])
try byPosition.forEachRow { row in print(row.line, row.string(1) ?? "") }

// A workbook reads into the same batch: from a file, memory, a stream or async bytes.
let book = try Workbook(contentsOfFile: "orders.xlsx")
print(book.sheets.map(\.name))                   // ["Orders"]
let sheet = try book.sheet(named: "Orders")      // its header read; bind as above
try sheet.bind([.i32(sheet.header!.ordinal(of: "id"))])
while let batch = try sheet.read() { /* … */ }
```

Asynchronous input — `URL.resourceBytes`, `FileHandle.bytes`, any `AsyncSequence` of
`UInt8` — is read as it arrives, with Task cancellation honoured between refills:

```swift
let (bytes, _) = try await URLSession.shared.bytes(from: url)
let reader = try await DelimitedReader(bytes: bytes, dialect: .csv)   // awaits the header
try reader.bind([.text(reader.header!.ordinal(of: "name"))])
while let batch = try await reader.readAsync() { /* … */ }

let workbook = try await Workbook(bytes: try await URLSession.shared.bytes(from: bookURL).0)
```

A cancelled `readAsync()` throws `CancellationError` and loses nothing: every byte that had
arrived stays buffered, and the next read goes on from there.

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
- **Rows, too.** A `Batch` is a `RandomAccessCollection` of `Row` — `index`, `line`,
  `get(_:as:)`, `verdict(_:)`, `text(_:)`, `string(_:)`, `raw(_:)`, the batch's accessors
  with the row fixed, allocating nothing — and `forEachRow` on a reader or a sheet reads
  every row left across batches. A row is valid as long as its batch.
- **Header first, or plan first.** Opened without a plan, a reader or sheet reads its
  header — a `Header`, a collection of `String` with `ordinal(of:)` (throwing
  `NoSuchColumn`) and `firstIndex(of:)`, both exact byte for byte — and is bound once with
  `bind(_:)`; reading before that, or binding twice, throws `PlanError`.
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

Android uses the same artifact bundle: it carries the core for `aarch64-unknown-linux-android`
and `x86_64-unknown-linux-android`, and a package built with the
[Swift SDK for Android](https://www.swift.org/documentation/articles/swift-sdk-for-android-getting-started.html)
(`swift build --swift-sdk aarch64-unknown-linux-android28`; Swift 6.3 or later, API 28 or
later) links it like any other triple, beside HyperCast's own. Page alignment is the final
link's, which the SDK does with the NDK's linker; NDK r28 and later align to the 16 KB pages
Android 15 devices may use by default. CI cross-builds this suite with the SDK for x86_64 and
runs it in an emulator whose image uses 16 KB pages, through
`.github/scripts/android_build_suite.sh` and `android_device_test.sh`, which run the same way
against a local emulator; the aarch64 build is linked. The Swift runtime on Android is shared
libraries, which an app packages the way the SDK's documentation describes; the core adds
nothing to them.

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

```sh
gh attestation verify HyperTabularCore.artifactbundle/x86_64-unknown-linux-gnu/libhypertabular.a --owner SkunkWerkx
```

## License

[MIT](../LICENSE)
