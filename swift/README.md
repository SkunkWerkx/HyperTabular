# HyperTabular for Swift

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers —
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
The program is an executable target's `main.swift`.

```swift
import Foundation
import HyperCast
import HyperTabular

// Open, read the header, and build the plan from the names it declares.
let reader = try DelimitedReader(contentsOfFile: "people.csv", dialect: .csv)
let header = reader.header!  // ["id", "name", "score"]
print("header: \(Array(header))")
try reader.bind([
    .i32(header.ordinal(of: "id")),  // throws NoSuchColumn("id") if it is missing
    .text(header.ordinal(of: "name")),
    .f64(header.ordinal(of: "score")),
])

while let batch = try reader.read() {
    // A column at a time, as the core wrote it…
    let ids = batch.values(0, as: Int32.self)  // UnsafeBufferPointer<Int32>
    let verdicts = batch.verdicts(2)  // a CellVerdict beside each value
    print("ids: \(Array(ids))")
    print("faults: \(verdicts.filter { !$0.isOk }.count)")

    // …or a row at a time, each cell as HyperCast's union.
    for row in batch {
        switch row.get(2, as: Double.self) {
        case .success(let score): print("\(row.string(1) ?? ""): \(score)")
        case .fault(let fault):
            print("line \(row.line): \(fault.reason) in \"\(String(decoding: row.raw(2), as: UTF8.self))\"")
        }
    }
}

// Columns known by position: the plan goes in with the opening.
let byPosition = try DelimitedReader(
    contentsOfFile: "people.csv", dialect: .csv, plan: [.i32(0), .text(1), .f64(2)])
try byPosition.forEachRow { row in print("line \(row.line): \(row.string(1) ?? "")") }

// A workbook reads into the same batch: from a file, memory, a stream or async bytes.
let book = try Workbook(contentsOfFile: "people.xlsx")
print("sheets: \(book.sheets.map(\.name))")  // ["People"]
let sheet = try book.sheet(named: "People")  // its header read; bind as above
try sheet.bind([.text(sheet.header!.ordinal(of: "name")), .decimal(sheet.header!.ordinal(of: "score"))])
try sheet.forEachRow { row in
    switch row.get(1, as: Decimal.self) {  // Foundation's Decimal
    case .success(let score): print("\(row.string(0) ?? ""): \(score)")
    case .fault(let fault): print("line \(row.line): \(fault.reason)")
    }
}
```

Asynchronous input — `URL.resourceBytes`, `FileHandle.bytes`, any `AsyncSequence` of
`UInt8` — is read as it arrives, with Task cancellation honoured between refills. Here `url`
and `bookURL` are the `URL`s of a CSV file and a workbook:

```swift
let (bytes, _) = try await URLSession.shared.bytes(from: url)
let reader = try await DelimitedReader(bytes: bytes, dialect: .csv)  // awaits the header
try reader.bind([.text(reader.header!.ordinal(of: "name"))])
while let batch = try await reader.readAsync() { print(batch.rows) }

let workbook = try await Workbook(bytes: try await URLSession.shared.bytes(from: bookURL).0)
```

A sequence hands over a byte at a time and cannot say whether another is waiting, so a
task the reader owns awaits it, up to `bufferBytes` ahead, and stops when the reader is
released. A read takes everything that has arrived and waits only when nothing has: a batch
is `batchRows` rows when that many are in, and a header or row is read once its line is in
(within 200 µs, so that a sequence ahead of the reader wakes it a batch at a time rather than
a line at a time), never held back for more. The sequence is passed `sending`, as that task
takes it. A
cancelled `readAsync()` throws `CancellationError` and loses nothing: every byte that had
arrived stays buffered, and the next read goes on from there.

## A messier file

Everything about a file is still declared, on the `Dialect` and on the `SheetOptions`, whose defaults
read as before: lines to pass over, records to discard, what a record of the wrong width is, the
encoding, and what stands for no value.

```swift
import Foundation
import HyperCast
import HyperTabular

// Windows-1252, with a title line above the table, NA for nothing and rows that stop short.
var dialect = Dialect.csv
dialect.skip = 1
dialect.shortRows = .pad
dialect.encoding = .windows1252
dialect.tokens = ["NA"]

let input = Array("Exported by hand\nname,amount\ncaf".utf8) + [0xE9] + Array(",NA\ntea\nmilk,12x4\n".utf8)
let reader = try DelimitedReader(
    bytes: input, dialect: dialect,
    plan: [
        .text(0),
        .f64(1).withTokens(["NA", "-"]),  // in place of the dialect's
    ])
let batch = try reader.read()!
for row in batch {
    let reason = row.verdict(1).reason.map { "\($0)" } ?? "ok"
    let detail = row.fault(1)?.detail.map { "\($0)" } ?? "-"
    print("\(row.string(0) ?? "") \(reason) \(row.origin(1)) \(detail)")
}
// What went wrong, and only that: a blank, a token and a padded cell aren't problems.
for problem in batch.faults() {
    print("line \(problem.line) column \(problem.sourceColumn ?? -1) \(problem.reason!) \(problem.detail!)")
}

// A plan to look at, from a sample of the text. It reads nothing until it's handed to a read.
let found = try DelimitedReader.suggest(
    bytes: Array("id,total,day\n1,\"1,234.50\",2026-01-07\n2,12.00,2026-01-08\n".utf8), dialect: .csv)
for column in found.report {
    print("\(column.name ?? "") \(column.status) \(column.door) \(column.matched) of \(column.sampled)")
}

// A sheet takes the part of it that's the table; its plan's ordinals count from B here.
var options = SheetOptions()
options.range = try SheetRange(parsing: "B2:D40")
print("\(options.range!.firstColumn) \(options.range!.lastRow)")
```

A `TabularError` of kind `.encoding` comes after every intact row before it, and its `byte` alone
counts the original bytes: a read's spans are all the decoded text's. A sheet's `range` is parsed by
the core, so `SheetRange(parsing: "A5:F200")` means what it means in every binding, with
`SheetRange(firstRow:firstColumn:lastRow:lastColumn:)` for what that can't say, such as every row from
the fifth (`SheetRange(firstRow: 5)`); both throw `RangeError`. A plan's ordinals count from the
range's first column, while `line(_:)` and a problem's `sourceColumn` stay the sheet's own. `skip` and
`headerRow`, on a `Dialect` and on `SheetOptions`, count from zero; `skipEmptyRows` and `skipBlankLines`
work on what they leave.

`notes = false`, on a `Dialect` or `SheetOptions`, drops the detail on a `Fault` and `Origin`, which then
say `.none` and `.cell`: the read costs a little less, and `faults()` can't list a formula with no
cached value. `Column.withDateOptions(_:)` hands a date door HyperCast's `DateOptions`, and on a sheet it
and `withTokens(_:)` reach only a text cell. A cell's detail is on `batch.fault(_:row:)`, `row.fault(_:)`,
`get(_:row:as:)`'s `.fault` and `faults()`; `CellVerdict.fault` has only what the verdict holds.

Declaring the locale, a number format and a date order, is the recommended route, and then nothing
needs inferring. A suggestion is a starting point for a plan to inspect, and it's never applied. Read
the report first: only a resolved column reads through its door in the plan, the rest as text. With no
format declared, each of two notations has to read the whole column: `1,234,567` and `1,234.5` read
only with `,` grouping, `1.234.567`, `1.234,5`, `2,5` and `0,125` only with `,` as the decimal, and the
plan is in whichever is left. `1,234` and `1.234` read both ways, so they come back ambiguous and read
as text until a format is declared or another cell settles them, which takes in a column of prices at
three decimals; `SuggestOptions.dateOrder` does the same for `01/02/2026`. `2,5` beside `2.5` is
unresolved, and `1,23,456`, Indian grouping, is text until the column declares Indian grouping, because
nothing says its culture. A sheet's number cell has no notation, so a column of them is a number under
any format, where text holding the same digits asks. A column of `0` and `1` is `.bool`, `007` and an
integer past `Int64` are text, and a typed date in a sheet is text, for a plan to change by hand. With
nothing to choose between, a resolved number is planned in `,` groups of three, and
`SuggestedColumn.format` names what the plan uses: a later `1,23,456` is refused.
`DelimitedReader.suggest` takes a path or bytes. A headerless file is opened twice, so no suggestion is
offered of a stream. `Sheet.suggest(_:)` answers from the sheet's first row whatever has been read of
it, and leaves it unbound and where it was.

**To keep every row of a sheet, set `skipEmptyRows` to `false`.** By default a row whose only cells are
formulas with no cached value is filtered out as empty, with nothing to say so; off, those rows are
delivered, their cells at `Origin.uncached`.

A column given no format reads `NumFormat.invariant`, which is Western: `1,234,567` is 1234567 and
`1,23,456` is `malformed`, and a format that names no policy is the same. If a column of Indian numbers
comes back refused, declare it, one column at a time, so an Indian column reads beside a Western one in
the same read:

```swift
import HyperCast
import HyperTabular

let plan: [Column] = [
    .i32(0, format: NumFormat.invariant.withGrouping(.indian)),  // 1,23,456 is 123456, and 1,234,567 is malformed
    .i32(1, format: .invariant),  // 1,234,567 is 1234567, and 1,23,456 is malformed
]
```

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `NumFormat.detect` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `.f64` reads `1,2345` as 1.2345,
`12,34` as 12.34 and `0,125` as 0.125. `NumFormat.from(locale:)` takes what the platform's Foundation says
of a locale, which is not the same everywhere: `en_IN` is Indian on macOS 27.2 and Western on Linux
(swift-foundation, Swift 6.4.2), so an Indian column says `.withGrouping(.indian)` there. A format holding the
retired grouping bit `256` (the words `351` and `863`), which HyperCast's own constructor stops with a
precondition failure and only memory written by hand can hold, is `PlanError.refused(column:)` when the
plan is bound, before any cell, naming the plan column, and in a suggest's options before it opens
anything. HyperCast's
[grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape and
the locales neither policy matches.

## Constraints

A numeric or text column can declare HyperCast's `Constraints<T>`, the OpenAPI 3.2 keywords
(`minimum`, `exclusiveMinimum`, `maximum`, `exclusiveMaximum`, `multipleOf` with a `Rounding`,
`minLength`, `maxLength`, `enum`, with `trim` and `caseSensitive` for text), through the factory of
its door. Each cell is cast first, so text that isn't the type is a `Fault` as ever, and then judged:

```swift
import HyperTabular

let reader = try DelimitedReader(
    bytes: Array("qty,sex\n12,MALE\n7,x\n".utf8), dialect: .csv,
    plan: [
        .i32(0, constraints: Constraints<Int32>(minimum: 1, maximum: 10)),
        .text(1, constraints: Constraints<String>(enum: ["Female", "Male"])),
    ])
let batch = try reader.read()!
for row in batch {
    let qty =
        switch row.checked(0, as: Int32.self) {
        case .success(let passed): "\(passed.value)"
        case .rejection(.violation(let broken)): "\(broken.value) breaks \(broken.keyword)"
        case .rejection(.fault(let fault)): "\(fault.reason)"
        }
    let sex =
        switch row.checked(1, as: String.self) {
        case .success(let passed): passed.value  // "MALE" matches the entry, and reads as "Male"
        case .rejection(let rejected): "\(rejected)"
        }
    print(qty, sex)
}
for problem in batch.faults() {
    print("line \(problem.line) column \(problem.sourceColumn!) \(problem.keyword!)")
}
```

A violation isn't a fault: the cell cast, so `fault(_:row:)` is `nil`, `CellVerdict.isViolation` says
so, the value is kept exactly as cast (never rounded), and the span is the judged text's. A cell
`multipleOf` rounded passes with `adjusted` set, and a text `enum` match reads as the entry as
declared, the caller's own string, with `enumIndex` naming it. `faults()` lists a violation with its
`keyword` and no `reason`. The constraints' type is the door's, so a column can't pair one door with
another's, and `get`, `text` and `string` stop a constrained column with a precondition failure: they
would read a violation as passed. `checked(_:row:as:)` reads an unconstrained column too, where every
cell that cast passes, unadjusted. HyperCast judges the declaration when it is built, and traps only
on one its core cannot read.

## The shape

The native core owns no memory and reads no files. `DelimitedReader` and `Sheet` allocate
the buffers — one value buffer and one verdict buffer per column, the table that locates
each cell — once, and the core fills them in one call per batch: the boundary is crossed
once per few thousand rows, not once per cell.

- **One batch type.** `read()` returns a `Batch` — `rows`, `columns`, `line(_:)`,
  `verdicts(_:)` and `values(_:as:)` for a whole column, `get(_:row:as:)` for any one cell
  as HyperCast's `Verdict` (`checked(_:row:as:)` and its `CheckedVerdict` for a column that
  declares constraints), `string`/`text` for text and `raw` for the text a cell was cast
  from — or `nil` when there are no more rows. It is a view of the reader's buffers, valid
  until the next `read()`, and the same type for delimited text and for a sheet.
- **Whole columns are the primitives.** `values(_:as:)` lends the bool, integer and
  floating-point doors as their own type, and the time door as `UInt64` nanoseconds since
  midnight. The decimal, UUID and other temporal doors are read a cell at a time with
  `get(_:row:as:)`: HyperCast presents them as Foundation's `Decimal`, `UUID`, `Date` and
  `DateComponents` and the standard library's `Duration`, none of which is laid out as the
  core writes the value (`UUID`'s layout is not part of Foundation's ABI), and HyperCast's
  Swift package has no type of its own that is.
- **Rows, too.** A `Batch` is a `RandomAccessCollection` of `Row` — `index`, `line`,
  `get(_:as:)`, `checked(_:as:)`, `verdict(_:)`, `fault(_:)`, `origin(_:)`, `text(_:)`, `string(_:)`, `raw(_:)`,
  the batch's accessors with the row fixed, allocating nothing — and `forEachRow` on a reader
  or a sheet reads every row left across batches, the one cursor here that follows the read. A row
  is valid as long as its batch: a `Row`, a slice or `reversed()` of a batch's rows, a walk of them
  or a `faults()` kept past the next `read()` stops the program with a precondition failure, rather
  than read the next batch's cells as its own. The `Batch` itself is the reader's one batch, so one
  kept past a read holds the rows just read, and `batch.lazy` with it: `[first, second]` holds one
  object twice, so copy what you keep. A batch or a row that outlives
  its reader or sheet stays readable, holding what it needs until it goes, since no read can follow.
- **The source can't call back in.** A `Source` closure runs in the middle of its reader's
  `read()`, and a `read()`, `bind(_:)` or `forEachRow` it calls on that same reader
  throws `PlanError.busy` (`The reader is in the middle of a read; its source can't call back into
  it.`). The outer read goes on unharmed, and a source that throws leaves no read in progress. A
  `read()` in the body of `forEachRow` moves the reader under the walk, which stops the program
  with a precondition failure, as any walk past the next read does. Other readers and sheets are
  there to use. A sheet, a workbook and `suggest` read memory or a path, so nothing of the
  caller's runs inside them.
- **Header first, or plan first.** Opened without a plan, a reader or sheet reads its
  header — a `Header`, a collection of `String` with `ordinal(of:)` (throwing
  `NoSuchColumn` for a name it doesn't hold, `AmbiguousColumn`, naming both positions, for one
  two columns share) and `firstIndex(of:)`, which takes the first, both exact byte for
  byte — and is bound once with
  `bind(_:)`; reading before that, or binding twice, throws `PlanError`. Once a delimited
  header has been read, a plan column at or past its width — where every record can only
  be empty — is refused with `PlanError.outOfReach(column:)`, whether the plan was given
  to the initializer or bound after. So, for a reader or a sheet, is an ordinal of 2²⁰ or
  more: no workbook format has that many columns, and one row of cell table stays under
  8 MiB.
- **The core decides how much a batch may ask for.** A `batchRows` or `bufferBytes` of 0, and a batch
  past 4 GiB (214 million rows of one `i32` column, 1.8 million of 100 text columns), throw
  ``BatchSizeError`` from the core, when a reader or a sheet opens or a plan is bound, before anything is
  allocated. A `bufferBytes` past 1 GiB starts there. A negative count is a programmer's error, and
  traps. `suggest` is never refused for the size of its own batches: the core says what fits and the
  sampler reads as many as it takes. A row that alone is past the limit throws
  ``BatchSizeError/rowTooWide(width:needed:limit:)``.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header and what
  [a messier file](#a-messier-file) needs; `SheetOptions` states a sheet's header, its range,
  whether empty rows are skipped and the batch size; the plan states each column's door and,
  for numbers, its `NumFormat`.
- **HyperCast is the judge.** `Verdict`, `Fault`, `NumFormat`, `UnixPrecision`, `DateOrder`
  and `ExcelEpoch` are HyperCast's own types. A text cell means exactly what `Cast` would
  say of the same text; a typed workbook cell is converted by the door directly.
- **A bad value is a verdict; a broken file is an error.** A cell that does not cast is a
  fault in its column and the read goes on. A record of the wrong width, input that ends
  inside a quoted cell, a workbook whose container or parts cannot be read, throws
  `TabularError`, after every intact row before it.

One very long record costs what it costs in Rust, but for memory. A release build on an Apple M6 Mac
mini (`Mac18,5`) on 2026-11-07, one run of each, reading one record from a source:

| One record | Swift | Rust |
| --- | ---: | ---: |
| 1 GiB, each read filling the room: time | 0.23 s | 0.19 s |
| 1 GiB, each read filling the room: peak resident | 2.15 GB | 1.08 GB |
| 64 MiB, 64 KiB a read | 1.36 s | 1.43 s |
| 128 MiB, 64 KiB a read | 5.42 s | 5.77 s |
| 64 MiB of Latin-1 | 0.37 s | 0.38 s |
| 128 MiB of Latin-1 | 1.42 s | 1.49 s |

Short reads and a declared encoding are quadratic in the record's length in both, since a fill looks
again from the record's start. Swift's peak is twice Rust's because the input doubles by copying, with
both copies alive for the copy.

`Tabular.isAvailable` and `Tabular.nativeVersion()` report the linked core.

## Linking and deployment

The core is linked into the executable as a static library on every platform — a SwiftPM
binary target, `HyperTabularCore.artifactbundle.zip`, attached to the GitHub Release and named
in the manifest by URL and checksum, with an archive for Linux (glibc and Swift's static musl
SDK) and Windows on x86-64 and arm64 and for macOS on both architectures — so SwiftPM
downloads it, checks it, and links the one for your target, and there is nothing to deploy
beside the executable. Swift 6.2 is the floor: it is the first release whose package manager
links a static library. HyperCast's Swift package takes its own core the same way, and the two
link side by side.

iOS, the iOS simulator and Mac Catalyst get the same archives from a second binary target,
`HyperTabularCoreApple.xcframework`: an app for those is built by Xcode, which links a
static library out of an XCFramework and does not read a static-library artifact bundle.
The manifest declares it only on a Mac, where those platforms can be built at all, and both
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

In a checkout of this repository the core is build output, never committed:
`.github/scripts/local-core.sh` builds the host's archive from the checkout's `rust/` into
`swift/HyperTabularCore.artifactbundle/` (gitignored but for its `include/`), and then
`swift test` from `swift/` links it. CI writes each leg's archives there the same way. Without
one the package still resolves, and a build stops at `import HyperTabularCore`. The root
`Package.swift` links that local bundle when it is there too, so CI's smoke tests build through
the consumer's manifest against the core the run built; a dependency's checkout never has one.

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

There is no separate package registry to attest here: SwiftPM resolves a git tag directly
against this repo, and downloads the two zips the tag's `Package.swift` names from that
release's assets, refusing either if its SHA-256 differs from the checksum written there. Each
zip verifies from the release tag alone: the release attestation, signed by this repository's
`release.yml` at the tag, names both, with the release inventory as its predicate.

```sh
gh release download v<version> --repo SkunkWerkx/HyperTabular --pattern 'HyperTabularCore*.zip'
gh attestation verify HyperTabularCore.artifactbundle.zip \
  --repo SkunkWerkx/HyperTabular --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Each zip also carries build provenance from `hyper-build-static.yml` in `SkunkWerkx/.github`,
at the preparation commit the tagged commit was staged from (the preparation run's
`stage-release` job verifies that before it writes a checksum into `Package.swift`, and the tag
run again before it signs the release attestation), so that check needs `--signer-repo` beside
`--repo`:

```sh
gh attestation verify HyperTabularCoreApple.xcframework.zip \
  --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
```

Get the signer wrong and `gh` reports a bare `verifying with issuer "sigstore.dev"`, which
reads like a bad signature but is only an identity mismatch. The root README's
[Provenance](../README.md#provenance) section has the whole picture: the inventory, which
artifacts are signed from where, and the gates every byte passes before publication.

## License

[MIT](../LICENSE)
