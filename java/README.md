# HyperTabular for Java

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX and ODS —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) `Verdict` for every cell.

```java
import io.github.skunkwerkx.hypercast.*;
import io.github.skunkwerkx.hypertabular.*;

// Header first: open, find each column by its name, then bind the plan.
try (DelimitedReader reader = DelimitedReader.open(Path.of("orders.csv"), Dialect.CSV)) {
    Header header = reader.header();
    reader.bind(List.of(
            Column.i32(header.ordinal("id")),       // NoSuchElementException naming "id" if absent
            Column.text(header.ordinal("name")),
            Column.f64(header.ordinal("score"))));
    for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
        // A column at a time, as the core wrote it…
        MemorySegment ids = batch.values(0);          // ids.getAtIndex(ValueLayout.JAVA_INT, row)
        MemorySegment verdicts = batch.verdicts(0);   // CellVerdict.LAYOUT, one per row

        // …or a row at a time, each cell as HyperCast's union.
        for (Row row : batch) {
            String line = switch (row.get(2, Double.class)) {
                case Success<Double> score -> row.string(1) + ": " + score.value();
                case Fault<Double> fault -> "line " + row.line() + ": " + fault.reason()
                        + " in \"" + row.rawString(2) + "\"";
            };
        }
    }
}

// A plan known up front is passed to the factory instead, and a workbook reads into the same batch.
List<Column> plan = List.of(Column.i32(0), Column.text(1), Column.f64(2));
try (Workbook book = Workbook.open(Path.of("orders.xlsx"));
        Sheet sheet = book.sheet("Orders", SheetOptions.DEFAULT, plan)) {
    sheet.forEachRow(row -> { /* … */ });
}
```

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
and `Sheet` allocate the buffers — the input, one value array and one verdict array per column, the
table that locates each cell — once, as native memory, and reuses them for every batch. The
core fills them in one native call per batch: the boundary is crossed once per few thousand
rows, not once per cell.

- **One batch type.** `read()` returns a `Batch` — `rows()`, `columns()`, `line(row)`,
  `verdicts(column)`, `values(column)` for a whole primitive column, `get(column, row, type)`
  for any one cell, `text`/`string`/`chars` for text, and `raw` for the text a cell was cast
  from — or `null` when there are no more rows. It is a view of the reader's buffers, valid
  until the next `read()`, and the same type for delimited text and for a sheet.
- **Rows, when that is how the code thinks.** A `Batch` is `Iterable<Row>` (and `row(i)`
  picks one): a `Row` is the batch's accessors with the row chosen — `index()`, `line()`,
  `get(column, type)`, `verdict`, `text`, `string`, `chars`, `raw` — a view, valid as long
  as the batch. `reader.forEachRow(row -> …)` and `sheet.forEachRow(row -> …)` read every
  batch left and hand over each row; a row is valid only for its own call.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting and the header;
  `SheetOptions` states a sheet's header and whether empty rows are skipped; the plan states each column's door and, for numbers, its `NumFormat`.
- **HyperCast is the judge.** `Verdict`, `Success`, `Fault`, `CastFailure`, `NumFormat`,
  `UnixPrecision`, `DateOrder` and `ExcelEpoch` are HyperCast's own types, from its own jar
  (`io.github.skunkwerkx:hypercast`, which this one depends on). A text cell means exactly
  what `Cast` would say of the same text, a typed workbook cell is converted by the door
  directly (a stored `2.5` through the decimal door is `2.5`), and either arrives as the Java type `Cast` gives the same
  door: `Instant`, `LocalDate`, `LocalDateTime`, `LocalTime` and `Duration` at full
  nanosecond fidelity, `BigDecimal` built from the core's exact sign, magnitude and scale,
  `UUID`, and the unsigned doors widened (`u8`/`u16` to `Integer`, `u32` to `Long`, `u64`
  as the `Long` with the same bits).
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a `Fault` in its column and the read goes on. A record of the wrong width, input that
  ends inside a quoted cell, a workbook whose container or parts cannot be read, is a
  `TabularException` — carrying the failure, the
  record, the line, the byte offset and the expected and found widths — raised after every
  intact row before it.
- **Text is zero-copy.** `text` hands back a read-only `MemorySegment` view into the
  reader's own buffer — the input, or a workbook's shared strings; only a cell with `""`
  inside, or a typed workbook cell said as text, is written to an arena. `raw`
  gives the text any cell was cast from, one that did not cast included.
- **UTF-16 without a `String`.** `chars(column, row)` decodes a text cell into a char array
  the batch keeps and hands back a read-only `CharBuffer` over it (a `CharSequence`, for
  `Integer.parseInt(cs, 0, cs.length(), 10)`, a `StringBuilder`, a regex): valid until the
  next `read()`, every earlier view staying as it was, the array reused batch after batch.
  `getChars(column, row, char[], offset)` decodes into the caller's array instead and
  returns the count, or `-1` if it does not fit. Bytes that are not UTF-8 are replaced
  exactly as `string` replaces them; an empty cell is `null` and `0`, as it is `null` for
  `string`.
- **The memory is native, and confined.** The core is never handed the Java heap: a fill
  reads a whole batch, which is not the instant a `critical` downcall is for. So the
  buffers are a confined `Arena`'s — freed by `close()`, with a view handed out earlier
  refused by the runtime rather than left dangling — and a reader belongs to the thread
  that built it.

## Where the bytes come from

| Factory | The input |
| --- | --- |
| `DelimitedReader.open(Path, …)` | A file, read through its channel straight into the reader's buffer. |
| `DelimitedReader.of(InputStream, …)` | Any stream; the reader closes it. The buffer starts at `bufferBytes` and doubles when a record does not fit, up to `MAX_ROW_BYTES`. |
| `DelimitedReader.of(MemorySegment, …)` | Native memory — a mapped file, an arena allocation — read in place, of any size. |
| `DelimitedReader.of(byte[], …)` | A byte array, copied into native memory once. |

Each takes the dialect, the plan and, optionally, the rows per batch
(`DEFAULT_BATCH_ROWS`, 4096) — and a file or a stream the size its read buffer starts at
(`bufferBytes`). The header, when the dialect declares one, is read by the
factory and is `reader.header()`.

Each also comes without the plan — `open(path, dialect)`, `of(bytes, dialect)`,
`of(segment, dialect)`, `of(stream, dialect[, bufferBytes])` — which reads the header and
leaves the plan to `reader.bind(plan[, batchRows])`, once, before the first read. A read
before `bind` is an `IllegalStateException`, and not a final one: bind, and read. The header
is a `Header`, which is still the `List<String>` of names it always was, plus
`ordinal(name)` (exact, case-sensitive, untrimmed; the first of two columns with one name;
a `NoSuchElementException` naming a missing one), `findOrdinal(name)` → `OptionalInt`, and
both again for the name's UTF-8 bytes (`byte[]` or `MemorySegment`). `columnCount()` is how
many cells a record has — the header's count, or the first record's once read — as an
`OptionalInt`; `isBound()` says whether a plan is in place.

A failure of the stream or file is an `UncheckedIOException`, from the factory or from
`read()`.

A workbook is `Workbook.open(Path)` — the file mapped, not read — or `Workbook.of(byte[])`,
`Workbook.of(MemorySegment)` or `Workbook.of(InputStream)`, which reads the stream to its end
into native memory the workbook owns and closes it, as `DelimitedReader.of(InputStream, …)`
takes its stream over. It lists its `sheets()` (name, and whether hidden), its `format()`
and its `dateSystem()`; `sheet(index or name, options, plan)` starts a read with its own
buffers, so several sheets can be read at once, and `sheet(index or name, options)` reads
the header row first and leaves the plan to `sheet.bind(plan)`. A missing index is an
`IndexOutOfBoundsException`, a missing name a `NoSuchElementException`.

There is no asynchronous read. A reader is confined to the thread that built it, and in
Java a virtual thread is the idiom for a read that blocks: run the loop on one, and a
stream that blocks parks it rather than a carrier thread. `Thread.interrupt()` reaches a
file read the way it reaches any `FileChannel` (an `UncheckedIOException` wrapping
`ClosedByInterruptException`).

## The doors

One `Column` factory per door, named as `Cast` names them, and one Java type for each door,
which is what `batch.get(c, r, type)` is asked for:

| Column | `get` type |
| --- | --- |
| `Column.bool(n)` | `Boolean` |
| `Column.i8(n)` … `Column.i64(n)` | `Byte`, `Short`, `Integer`, `Long` |
| `Column.u8(n)` … `Column.u64(n)` | `Integer`, `Integer`, `Long`, `Long` |
| `Column.f32(n)`, `Column.f64(n)` | `Float`, `Double` |
| `Column.decimal(n)` | `BigDecimal` |
| `Column.uuid(n)` | `UUID` |
| `Column.timestamp(n)`, `Column.unix(n, precision)`, `Column.excelSerial(n, epoch)` | `Instant` |
| `Column.date(n)`, `Column.date(n, order)` | `LocalDate` |
| `Column.dateTime(n, order)` | `LocalDateTime` |
| `Column.time(n)` | `LocalTime` |
| `Column.duration(n)` | `Duration` |
| `Column.text(n)` | `String`; also `batch.text` → `MemorySegment` and `batch.string` → `String`, `null` for an empty cell |

Asking for another type is an `IllegalStateException`. Every numeric factory also takes a
`NumFormat`; without one the notation is `NumFormat.INVARIANT`. `batch.verdict(c, r)` and
`batch.isOk(c, r)` answer for a cell of any door.

## The native library

The jar bundles a native build for every supported platform (Linux glibc, Linux musl,
macOS, Windows × x64/arm64) under `/native/{rid}/` and picks the right one at runtime.
`Tabular.isAvailable()` and `Tabular.nativeVersion()` answer whether it resolved, and
which version answered, without the first read being what finds out.

JDK 25 is the floor, as it is for HyperCast's jar. The FFM downcalls are restricted
methods: run with `--enable-native-access=ALL-UNNAMED` (or
`--enable-native-access=io.github.skunkwerkx.hypertabular` on the module path).

## WebAssembly (GraalWasm)

The jar carries the core a second time, as `native/wasm32-wasip1/hypertabular.wasm` — the
same fourteen exports, compiled for WASI preview 1 instead of an OS — and
[GraalWasm](https://www.graalvm.org/webassembly/) runs it inside the JVM, as HyperCast's jar
does with its own core. So a platform this jar has no native build for (riscv64, ppc64le,
s390x, FreeBSD, …) still reads delimited text and workbooks, with no binary to ship and no
FFM downcall.

The seam is one level below the readers: every reader, batch and exception is one
implementation for both paths, and the wasm class mirrors each buffer a call hands the core
into the guest's memory (allocated with the module's own exported `malloc`) and what the core
wrote back out. Buffers that last between calls — the state block, a read's window, arena
and cell table, each column — keep a twin in the guest, so a batch writes only what changed
and reads back only the rows filled. The full suite, both corpus replays included, runs twice
on every build (`./gradlew test testWasm`).

**Enabling it.** GraalWasm is not a dependency of this jar — its POM lists nothing for it —
so add the two artifacts yourself (`wasm` is a POM-type dependency that fans out into the
Truffle runtime):

```kotlin
dependencies {
    implementation("io.github.skunkwerkx:hypertabular:<version>")
    implementation("org.graalvm.polyglot:polyglot:25.4.4.1.1")
    runtimeOnly("org.graalvm.polyglot:wasm:25.4.4.1.1")
}
```

Then set `-Dhypertabular.backend=wasm` to force it, or do nothing: unset, the native library
is used when the jar has one for the running platform and it loads, and the module
otherwise; if the module cannot start either, the native failure is thrown with the wasm
one suppressed on it. `-Dhypertabular.backend=native` forces FFM. `Tabular.backend()` reports
`"native"` or `"wasm"`. Selecting wasm without GraalWasm on the classpath makes
`isAvailable()` `false` and every read throw a message naming the two artifacts. Run with
`--enable-native-access` either way: the wasm path reads the addresses inside the buffers
block the readers build.

**What it costs**, the 300 000-row Excel workbook (`corpus/generate/out/excel-win-300k.xlsx`,
eight columns, the JMH suite's `read` loop, checksum 5285882 on every path), best of five on
one linux-x64 box:

| Path | GraalVM CE 25.4 | Temurin 25 |
| --- | ---: | ---: |
| FFM downcall | 721 ms | 740 ms |
| GraalWasm | 1.88 s (JIT; the first read 8.0 s, while Truffle compiles the guest) | 148 s (one read; no JIT) |

Under GraalVM's JIT the module costs about 2.6x the native read. On a stock OpenJDK GraalWasm
has no JIT and interprets the module, which is correct but two hundred times slower: the
wasm path is for a platform with no native build, run on a GraalVM JDK, not a speed option.
Keep `org.graalvm.polyglot:polyglot` and `:wasm` at the same release as the GraalVM JDK.

## Building

```sh
cd ../rust; cargo cdylib      # the native library, staged from rust/target/release
cd ../java
./gradlew test javadoc        # the suite — the conformance corpus included — and the doc gate
cd ../rust; cargo wasm-module # the wasm32-wasip1 module, staged from rust/target/wasm32-wasip1/release
cd ../java; ./gradlew testWasm  # the same suite through GraalWasm
./gradlew :aot-smoke-test:run # every native entry point, crossed once, on a plain JVM
```

`./gradlew :aot-smoke-test:nativeRun` under a GraalVM `JAVA_HOME` runs the same program as
a Native Image binary; the downcall signatures and the resource glob it needs ship inside
the jar (`META-INF/native-image/`), so a consumer's own image needs no configuration.
