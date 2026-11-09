# HyperTabular for Java

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and Numbers —
read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) `Verdict` for every cell, calling the
native `libhypertabular` core in-process: through JNI on JDK 21 to 24, and through
`java.lang.foreign` (FFM) downcalls on JDK 25 and later, from one multi-release jar. The API,
the verdicts, the exceptions and their messages are the same on both. JDK 21 is the floor,
as it is for HyperCast's jar.

```kotlin
implementation("io.github.skunkwerkx:hypertabular:<version>")
```

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
Run the program with `--enable-native-access=ALL-UNNAMED` ([below](#the-native-library)). It
names `MemorySegment`, which on JDK 21 means compiling and running it with `--enable-preview`
([below](#memorysegment-on-jdk-21)); the row and cell accessors alone need nothing.

```java
import io.github.skunkwerkx.hypercast.*;
import io.github.skunkwerkx.hypertabular.*;
import java.lang.foreign.*;
import java.math.BigDecimal;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;

public class Main {
    public static void main(String[] args) {
        // Header first: open, find each column by its name, then bind the plan.
        try (DelimitedReader reader = DelimitedReader.open(Path.of("people.csv"), Dialect.CSV)) {
            Header header = reader.header();
            System.out.println("header: " + header);
            reader.bind(List.of(
                    Column.i32(header.ordinal("id")), // NoSuchElementException naming "id" if absent
                    Column.text(header.ordinal("name")),
                    Column.f64(header.ordinal("score"))));
            for (Batch batch = reader.read(); batch != null; batch = reader.read()) {
                // A column at a time, as the core wrote it…
                MemorySegment ids = batch.values(0); // JAVA_INT, one per row
                MemorySegment verdicts = batch.verdicts(2); // CellVerdict.LAYOUT, one per row
                System.out.println("ids: " + Arrays.toString(ids.toArray(ValueLayout.JAVA_INT)));
                long faults = verdicts.elements(CellVerdict.LAYOUT)
                        .filter(verdict -> verdict.get(ValueLayout.JAVA_INT, 8) != 0) // the reason, 0 for ok
                        .count();
                System.out.println("faults: " + faults);

                // …or a row at a time, each cell as HyperCast's union.
                for (Row row : batch) {
                    String line = switch (row.get(2, Double.class)) {
                        case Success<Double> score -> row.string(1) + ": " + score.value();
                        case Fault<Double> fault ->
                            "line " + row.line() + ": " + fault.reason() + " in \"" + row.rawString(2) + "\"";
                    };
                    System.out.println(line);
                }
            }
        }

        // A plan known up front is passed to the factory instead, and bound before the header is read.
        List<Column> plan = List.of(Column.i32(0), Column.text(1), Column.f64(2));
        try (DelimitedReader reader = DelimitedReader.open(Path.of("people.csv"), Dialect.CSV, plan)) {
            reader.forEachRow(row -> System.out.println("line " + row.line() + ": " + row.string(1)));
        }

        // A workbook reads into the same batch.
        try (Workbook book = Workbook.open(Path.of("people.xlsx"));
                Sheet sheet = book.sheet("People", SheetOptions.DEFAULT)) {
            System.out.println(
                    "sheets: " + book.sheets().stream().map(SheetInfo::name).toList());
            Header header = sheet.header();
            sheet.bind(List.of(Column.text(header.ordinal("name")), Column.decimal(header.ordinal("score"))));
            sheet.forEachRow(row -> {
                String line = switch (row.get(1, BigDecimal.class)) {
                    case Success<BigDecimal> score -> row.string(0) + ": " + score.value();
                    case Fault<BigDecimal> fault -> "line " + row.line() + ": " + fault.reason();
                };
                System.out.println(line);
            });
        }
    }
}
```

## A messier file

Everything about a file is still declared, on the `Dialect` and on the `SheetOptions`, whose defaults
read as before: lines to pass over, records to discard, what a record of the wrong width is, the
encoding, and what stands for no value.

```java
import io.github.skunkwerkx.hypercast.*;
import io.github.skunkwerkx.hypertabular.*;
import java.io.ByteArrayOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.List;

public class Messier {
    public static void main(String[] args) {
        // Windows-1252, with a title line above the table, NA for nothing and rows that stop short.
        Dialect dialect = Dialect.CSV
                .withSkip(1)
                .withShortRows(ShortRows.PAD)
                .withEncoding(Encoding.WINDOWS_1252)
                .withTokens("NA");

        ByteArrayOutputStream input = new ByteArrayOutputStream();
        input.writeBytes("Exported by hand\nname,amount\ncaf".getBytes(StandardCharsets.UTF_8));
        input.write(0xE9);
        input.writeBytes(",NA\ntea\nmilk,12x4\n".getBytes(StandardCharsets.UTF_8));
        try (DelimitedReader reader = DelimitedReader.of(input.toByteArray(), dialect, List.of(
                Column.text(0),
                Column.f64(1).withTokens("NA", "-")))) { // in place of the dialect's
            Batch batch = reader.read();
            for (Row row : batch) {
                System.out.println(row.string(0) + " " + row.verdict(1).reason() + " " + row.origin(1) + " " + row.detail(1));
            }
            // What went wrong, and only that: a blank, a token and a padded cell aren't problems.
            for (Problem problem : batch.faults()) {
                System.out.println("line " + problem.line() + " column " + problem.sourceColumn().getAsInt() + " "
                        + problem.reason() + " " + problem.detail());
            }
        }

        // A plan to look at, from a sample of the text. It reads nothing until it's handed to a read.
        Suggestion found = DelimitedReader.suggest(
                "id,total,day\n1,\"1,234.50\",2026-01-07\n2,12.00,2026-01-08\n".getBytes(StandardCharsets.UTF_8),
                Dialect.CSV);
        for (SuggestedColumn column : found.report()) {
            System.out.println(column.name() + " " + column.status() + " " + column.door() + " " + column.matched()
                    + " of " + column.sampled());
        }

        // A sheet takes the part of it that's the table; its plan's ordinals count from B here.
        SheetOptions options = SheetOptions.DEFAULT.withRange(Range.parse("B2:D40"));
        System.out.println(options.range().firstColumn() + " " + options.range().lastRow());
    }
}
```

A `TabularException` of `TabularFailure.ENCODING` comes after every intact row before it, and its
`byteOffset()` alone counts the original bytes: a read's spans are all the decoded text's. A sheet's
`range` is parsed by the core, so `Range.parse("A5:F200")` means what it means in every binding, with
`Range.tryParse` for an `Optional` in place of its `IllegalArgumentException`, and
`new Range(firstRow, firstColumn, lastRow, lastColumn)` for what that can't say, such as every row from
the fifth (`new Range(5, 0, 0, 0)`). A plan's ordinals count from the range's first column, while
`Batch.line` and a problem's `sourceColumn` stay the sheet's own. `skip` and `headerRow`, on a `Dialect`
and on `SheetOptions`, count from zero; `skipEmptyRows` and `skipBlankLines` work on what they leave.

`withNotes(false)`, on a `Dialect` or `SheetOptions`, drops the detail on a fault and the origin, which
then say `null` and `Origin.CELL`: the read costs a little less, and `faults()` can't list a formula
with no cached value. `Column.withDateOptions` hands a date door HyperCast's `DateOptions`, with its
`MonthNames` if it has them, and on a sheet it and `withTokens` reach only a text cell. A cell's detail
is on `batch.detail(column, row)`, `row.detail(column)` and `faults()`, `null` where there is none, and
a `Fault` from `get` has only what the verdict holds. `faults()` belongs to its batch: walking it after
the next `read()` throws `ConcurrentModificationException`.

Declaring the locale, a number format and a date order, is the recommended route, and then nothing
needs inferring. A suggestion is a starting point for a plan to inspect, and it's never applied. Read
the report first: only a resolved column reads through its door in the plan, the rest as text. With no
format declared, each of two notations has to read the whole column: `1,234,567` and `1,234.5` read
only with `,` grouping, `1.234.567`, `1.234,5`, `2,5` and `0,125` only with `,` as the decimal, and the
plan is in whichever is left. `1,234` and `1.234` read both ways, so they come back ambiguous and read
as text until a format is declared or another cell settles them, which takes in a column of prices at
three decimals; `SuggestOptions.withDateOrder` does the same for `01/02/2026`. `2,5` beside `2.5` is
unresolved, and `1,23,456`, Indian grouping, is text until the column declares Indian grouping, because
nothing says its culture. A sheet's number cell has no notation, so a column of them is a number under
any format, where text holding the same digits asks. A column of `0` and `1` is `bool`, `007` and an
integer past `long` are text, and a typed date in a sheet is text, for a plan to change by hand. With
nothing to choose between, a resolved number is planned in `,` groups of three, and
`SuggestedColumn.format()` names what the plan uses: a later `1,23,456` is refused.
`DelimitedReader.suggest` takes a path, a `byte[]` or a `MemorySegment`. A headerless file is opened
twice, so no suggestion is offered of a stream. `sampleRows` of `0` is one row, and a negative one is an
`IllegalArgumentException` at `withSampleRows`. `Sheet.suggest` answers from the sheet's first row
whatever has been read of it, and leaves it unbound and where it was. A failure past a whole sample only
sets `truncated()`; one among the sampled rows is thrown, and so is a thread interrupt wherever it comes:
it is the caller asking to stop, not a failure of the file.

**To keep every row of a sheet, use `withEmptyRowsSkipped(false)`.** By default a row whose only cells
are formulas with no cached value is filtered out as empty, with nothing to say so; off, those rows are
delivered, their cells at `Origin.UNCACHED`.

A column given no format reads `NumFormat.INVARIANT`, which is Western: `1,234,567` is 1234567 and
`1,23,456` is `MALFORMED`, and a format that names no policy is the same. If a column of Indian numbers
comes back refused, declare it, one column at a time, so an Indian column reads beside a Western one in
the same read:

```java
NumFormat indian = NumFormat.INVARIANT.withGrouping(Grouping.INDIAN);
List<Column> plan = List.of(
        Column.i32(0, indian),              // 1,23,456 is 123456, and 1,234,567 is MALFORMED
        Column.i32(1, NumFormat.INVARIANT)); // 1,234,567 is 1234567, and 1,23,456 is MALFORMED
```

Neither policy reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `NumFormat.DETECT` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `Column.f64` reads `1,2345` as
1.2345, `12,34` as 12.34 and `0,125` as 0.125. A `NumFormat` can't hold the retired grouping bit `256`
(the words `351` and `863`): its constructor refuses it, so no plan or suggestion here has one to
refuse. HyperCast's
[grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have every shape
and the locales neither policy matches.

## The shape

The native core (`libhypertabular`) owns no memory and reads no files. `DelimitedReader`
and `Sheet` allocate the buffers — the input, one value array and one verdict array per column, the
table that locates each cell — once, as native memory, and reuse them for every batch. The
core fills them in one native call per batch: the boundary is crossed once per few thousand
rows, not once per cell.

- **One batch type.** `read()` returns a `Batch` — `rows()`, `columns()`, `line(row)`,
  `verdicts(column)`, `values(column)` for a whole column, `get(column, row, type)` for any
  one cell, `detail` and `origin` for what a cell's verdict says beside itself, `faults()` for
  what went wrong in the batch, `text`/`string`/`chars` for text, and `raw` for the text a cell
  was cast from —
  or `null` when there are no more rows. It is a view of the reader's buffers, valid until
  the next `read()`, and the same type for delimited text and for a sheet.
- **Whole columns as the core wrote them.** `values(column)` is a read-only `MemorySegment`
  over any column but a text one: the primitive doors at their own width, read with the
  matching `ValueLayout`, and every other door in the layout HyperCast's
  `io.github.skunkwerkx.hypercast.interop.NativeValues` reads, a `*_BYTES` stride a row —
  `NativeValues.decimal(values, row * NativeValues.DECIMAL_BYTES)` is the `BigDecimal` that
  `get` would make, and likewise `uuid`, `instant`, `date`, `civil`, `time` and `duration`.
- **Rows, when that is how the code thinks.** A `Batch` is `Iterable<Row>` (and `row(i)`
  picks one): a `Row` is the batch's accessors with the row chosen — `index()`, `line()`,
  `get(column, type)`, `verdict`, `detail`, `origin`, `text`, `string`, `chars`, `raw` — a view, valid as long
  as the batch, and an iterator goes on no further than that: past the next `read()` it throws
  `ConcurrentModificationException`, never answering for the next batch. `reader.forEachRow(row -> …)` and `sheet.forEachRow(row -> …)` read every
  batch left and hand over each row; a row is valid only for its own call.
- **The core decides how much a batch may ask for.** A `batchRows` or `bufferBytes` of 0, and a batch
  past 4 GiB (214 million rows of one `i32` column, 1.8 million of 100 text columns), are an
  `IllegalArgumentException` from the core, when a reader or a sheet opens or a plan is bound, before
  anything is allocated. A `bufferBytes` past 1 GiB starts there. An arena writes zeros over what it
  hands out, so a batch near the limit holds that memory from the moment it's bound. `suggest` is never
  refused for the size of its own batches: the core says what fits and the sampler reads as many as it
  takes. A row that alone is past the limit is an `IllegalArgumentException` naming its width.
- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header and the
  encoding; `SheetOptions` states a sheet's header, its range and whether empty rows are skipped;
  the plan states each column's door and, for numbers, its `NumFormat`. `suggest` reads a sample
  to propose a plan, only when asked, and applies nothing.
- **HyperCast is the judge.** `Verdict`, `Success`, `Fault`, `CastFailure`, `FaultDetail`,
  `NumFormat`, `DateOptions`, `MonthNames`, `UnixPrecision`, `DateOrder` and `ExcelEpoch` are HyperCast's own types, from its own jar
  (`io.github.skunkwerkx:hypercast`, which this one depends on). A text cell means exactly
  what `Cast` would say of the same text, a typed workbook cell is converted by the door
  directly (a stored `2.5` through the decimal door is `2.5`), and either arrives as the Java type `Cast` gives the same
  door: `Instant`, `LocalDate`, `LocalDateTime`, `LocalTime` and `Duration` at full
  nanosecond fidelity, `BigDecimal` built from the core's exact sign, magnitude and scale,
  `UUID`, and the unsigned doors widened (`u8`/`u16` to `Integer`, `u32` to `Long`, `u64`
  as the `Long` with the same bits).
- **A bad value is a verdict; a broken file is an exception.** A cell that does not cast
  is a `Fault` in its column and the read goes on, and `batch.faults()` lists a batch's. A record of the wrong width, input that
  ends inside a quoted cell, a workbook whose container or parts cannot be read, is a
  `TabularException` — carrying the failure, the
  record, the line, the byte offset and the expected and found widths — raised after every
  intact row before it.
- **Text is zero-copy.** `text` hands back a read-only `MemorySegment` view into the
  reader's own buffer — the input, or a workbook's shared strings; only a cell with `""`
  inside, or a typed workbook cell said as text, is written to an arena. Text in a declared
  encoding is decoded into a buffer of the reader's, so its views are into that, even from
  memory. `raw` gives the text any cell was cast from, one that did not cast included. A cell
  whose bytes aren't UTF-8 is `MALFORMED` at the first bad sequence, with `text` and `string`
  `null`, and `raw` still has its bytes.
- **UTF-16 without a `String`.** `chars(column, row)` decodes a text cell into a char array
  the batch keeps and hands back a read-only `CharBuffer` over it (a `CharSequence`, for
  `Integer.parseInt(cs, 0, cs.length(), 10)`, a `StringBuilder`, a regex): valid until the
  next `read()`, every earlier view staying as it was, the array reused batch after batch.
  `getChars(column, row, char[], offset)` decodes into the caller's array instead and
  returns the count, or `-1` if it does not fit. A cell with no text, empty or not UTF-8, is
  `null` and `0`, as it is `null` for `string`.
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
| `DelimitedReader.of(MemorySegment, …)` | Native memory — a mapped file, an arena allocation — read in place, of any size. On JDK 21 to 24 a shared arena's segment is copied first (below). |
| `DelimitedReader.of(byte[], …)` | A byte array, copied into native memory once. |

Each takes the dialect, the plan and, optionally, the rows per batch
(`DEFAULT_BATCH_ROWS`, 4096) — and a file or a stream the size its read buffer starts at
(`bufferBytes`). The header, when the dialect declares one, is read by the
factory and is `reader.header()`.

Each also comes without the plan — `open(path, dialect)`, `of(bytes, dialect)`,
`of(segment, dialect)`, `of(stream, dialect[, bufferBytes])` — which reads the header and
leaves the plan to `reader.bind(plan[, batchRows])`, once, before the first read. A read
before `bind` is an `IllegalStateException`, and not a final one: bind, and read. The header
is a `Header`, the `List<String>` of names, with
`ordinal(name)` (exact, case-sensitive, untrimmed; a `NoSuchElementException` naming a missing
one, and its subclass `AmbiguousColumnException`, naming both positions, when two columns share the
name), `findOrdinal(name)` → `OptionalInt`, which takes the first of those, and
both again for the name's UTF-8 bytes (`byte[]` or `MemorySegment`). `columnCount()` is how
many cells a record has — the header's count, or the first record's once read — as an
`OptionalInt`; `isBound()` says whether a plan is in place.

Once a header has fixed the width every record has, a plan column reading at or past it
could only ever be empty: the factory or `bind` refuses it with an `IllegalArgumentException`
naming the plan column. Without a header, such a column reads as `EMPTY`, and a plan reaching
past 256 source columns is read in batches shorter than `batchRows`, so that the cell table
stays near 8 MiB. An ordinal must be below 2^20 (1,048,576), for a delimited reader and a sheet
alike: no workbook format has that many columns (XLSX and ODS stop at 16,384, Numbers at
1,000), and at the cap one row of cell table is at most 8 MiB.

A failure of the stream or file is an `UncheckedIOException`, from the factory or from
`read()`. A stream is read once a call, so the rows it has finished come back however long it then
blocks. On JDK 21 a reader's or a workbook's buffers count against
`-XX:MaxDirectMemorySize`, which is the heap size unless it's said, so a record or a workbook too big
for that is an `OutOfMemoryError` there, short of `MAX_ROW_BYTES`: raise the flag.

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
file read the way it reaches any `FileChannel`: the channel closes, that read throws an
`UncheckedIOException` wrapping `ClosedByInterruptException`, and every later read an
`UncheckedIOException` wrapping an `InterruptedIOException` that says the interrupt ended it. The
rows before it have all been delivered.

A stream's code runs in the middle of a read. A call from it back into its reader — `read()`,
`bind`, `forEachRow` — throws `IllegalStateException` ("The reader is in the middle of a read;
its stream can't call back into it."), and the read it came from goes on whole. A `close()` from
there is done as soon as the stream returns, and that read ends with an `IllegalStateException`
("The reader was closed in the middle of a read."). It hands out no batch, and every later call
throws "The reader is closed." What the stream throws reaches the caller and isn't final: the
next read goes on from where the reader was.

A segment handed to `DelimitedReader.of(MemorySegment, …)` or `Workbook.of(MemorySegment)` is read
in place, and has to stay alive and unchanged while the reader or workbook is open. On JDK 25 each
native call holds the segment's arena open, so a close from another thread in the middle of one is
refused. The JNI path, on JDK 21 to 24, can't, so it copies a segment another thread could close —
a shared arena's — into the reader's own memory when the reader or workbook is opened. A confined
arena's, an automatic arena's and the global arena's are still read in place. The copy holds a
second copy of the input in native memory for the reader's life, counted against
`-XX:MaxDirectMemorySize` on JDK 21 like the reader's other buffers. Its time, measured 2026-11-08 on
an Apple M6 (32 GB) with Temurin 21.0.13, opening 93.1 MiB of `id,name,score` rows (4,000,000, read as
`i64`, text and `f64`, 4,096 a batch), median of 7 runs after a warm-up:

| JDK 21, JNI | Open | Open and read every batch |
| --- | ---: | ---: |
| Confined arena, read in place | 0.2 ms | 80.5 ms |
| Shared arena, copied | 2.4 ms | 83.1 ms |

So a shared arena's input costs 2.2 ms more to open and 2.6 ms (3%) more over the whole read here.
On JDK 25 (FFM) the two read the same: 79.1 and 78.7 ms.

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
`NumFormat`; without one the notation is `NumFormat.INVARIANT`, Western. `batch.verdict(c, r)` and
`batch.isOk(c, r)` answer for a cell of any door.

## The native library

The jar bundles a native build for every supported platform (Linux glibc, Linux musl,
macOS, Windows × x64/arm64) under `/native/{rid}/` — the core, `libhypertabular`, and the JNI
library built from it, `libhypertabular_jni` — and picks the right one at runtime.
`Tabular.isAvailable()` and `Tabular.nativeVersion()` answer whether it resolved, and
which version answered, without the first read being what finds out.

Which library loads is the JDK's choice, made by the multi-release jar: JDK 25 and later read
`META-INF/versions/25` and call the core through FFM downcalls; JDK 21 to 24 read the base and
load the JNI library, which holds the core inside it and hands each call the addresses of the
same native buffers. JDK 21 is the floor, as it is for HyperCast's jar: Temurin builds no JDK 17
for Windows arm64 or Alpine (musl) arm64, so a 17 floor could not be tested on every platform
this jar ships. JDK 22 to 24 take the JNI path; they are past end of life and not tested.

HyperCast's own jar (the dependency every verdict type comes from) loads a library of its own
only when one of its doors is called; it and this jar's share a process on either path, each
with its own copy of HyperCast's core.

### Native access

Loading native code is a restricted operation. On JDK 21 to 23 nothing is printed without the
grant; from 24 a JVM without it prints a warning naming the method that loads native code —
`java.lang.System::load` on 24, where the JNI path loads the library, and
`java.lang.foreign.SymbolLookup::libraryLookup` on 25 and later. Grant it where the JVM is
launched: `--enable-native-access=ALL-UNNAMED` on the class path, or on the module path
`--enable-native-access=io.github.skunkwerkx.hypertabular,io.github.skunkwerkx.hypercast` (on
25 HyperCast's `NativePlatform` opens this jar's library, so both modules are named).

### Shaded and fat jars

Keep `Multi-Release: true` (maven-shade's `ManifestResourceTransformer`, Shadow's
`manifest`). Without it JDK 25 reads only the base and runs the JNI path: correct, and its
warning names `System::load`.

### `MemorySegment` on JDK 21

`java.lang.foreign` is a preview API in JDK 21, so `javac --release 21` refuses code that
names `MemorySegment` or `ValueLayout` unless it is compiled with `--enable-preview` (and run
with it). The jar itself needs neither flag: the readers' buffers are native memory from
`java.lang.foreign`'s memory API, which JDK 21 runs from ordinary class files. Code on 21 that
reads through `get`, `verdict`, `string`, `chars` and `Row` and never names a segment compiles
and runs as on any other JDK.

## Building

The jar carries two native libraries per platform, so build both before Gradle runs: `cargo
cdylib` in `rust/`, then `cargo build --release --locked --lib` in `java/jni/`. Gradle stages
them into the jar and refuses a stale one: the build fails when either library is older than a
source cargo read for it (its dep-info), or when one is built without the other, and the
message gives the rebuild commands.

Gradle, Spotless, javadoc and publishing run on JDK 25. `./gradlew test` runs the suite — the
conformance corpus included — against the built jar on JDK 25 (the FFM path); `testFloor` runs
the same suite on JDK 21 (the JNI path end to end, under `-Xcheck:jni`), `checkSuiteParity`
fails unless both ran the same tests with none skipped (but the one that reads a record at
`MAX_ROW_BYTES`, which skips itself out loud on a host with under 6 GiB of memory), and `checkFloorApi` fails if the base
references any JDK class, method or field that JDK 21 lacks: the base compiles
`-source/-target 21` against JDK 25's API rather than `--release 21`, because `--release 21`
refuses `java.lang.foreign` as preview. `gradle.properties` turns toolchain auto-download off,
so pass the JDK 21 explicitly:

```sh
cd ../rust; cargo cdylib                              # the core
cd ../java/jni; cargo build --release --locked --lib  # the JNI library
cd ..
./gradlew test testFloor checkSuiteParity javadoc \
  -Dorg.gradle.java.installations.paths=<path to a JDK 21> \
  -Dorg.gradle.java.installations.auto-detect=false
./gradlew :aot-smoke-test:run :aot-smoke-test:runFloor  # every native entry point, on 25 and on 21
```

The JNI library's own checks run in `java/jni/`: `cargo fmt --check`, `cargo clippy --release
--locked --all-targets -- -D warnings`, `cargo test --locked` (which also holds the table to
`Native.java`'s declarations and every entry point to the no-panic attribute), and the proof
that no native can panic, which needs GNU ld (Linux, or a Linux container):

```sh
CARGO_PROFILE_RELEASE_PANIC=unwind cargo build --release --locked --example no_panic --features no-panic
```

`./gradlew :aot-smoke-test:nativeRun` under a GraalVM `JAVA_HOME` runs the smoke program as
a Native Image binary; the downcall signatures and the resource globs it needs (the core
library's three names, never the JNI library's) ship inside the jar
(`META-INF/native-image/`), so a consumer's own image needs no configuration. Native Image is
built on GraalVM for JDK 25 and always takes the FFM path, and the smoke program fails if the
image embeds the JNI library. `./gradlew :benchmarks:jmh -PjmhJdk=21` forks the benchmarks on
the floor.

## Verifying provenance

The published jar verifies from the release tag alone, and so do the sources and javadoc jars,
the `.pom` and their signatures: the release attestation, signed by this repository's
`release.yml` at the tag, names every file of the Central bundle among everything the release
ships, with the release inventory as its predicate.

```sh
curl -LO https://repo1.maven.org/maven2/io/github/skunkwerkx/hypertabular/<version>/hypertabular-<version>.jar
gh attestation verify hypertabular-<version>.jar --repo SkunkWerkx/HyperTabular \
  --source-ref refs/tags/v<version> --predicate-type https://github.com/SkunkWerkx/.github/predicates/release/v1
```

Its build provenance is the second check. The bundle is assembled by `hyper-pack-maven.yml`,
a reusable workflow that lives in `SkunkWerkx/.github`, and that is the identity Fulcio
records as the signer, so `--repo` alone is not enough there: add `--signer-repo`, or use
`--owner` in place of both. The native libraries inside the jar
carry their own, from the Forge's build workflows at the preparation commit on master, and the
pack job verifies each of them before it goes into the jar.

```sh
gh attestation verify hypertabular-<version>.jar \
  --repo SkunkWerkx/HyperTabular --signer-repo SkunkWerkx/.github
# or: gh attestation verify hypertabular-<version>.jar --owner SkunkWerkx
```

Get the signer wrong and `gh` reports a bare `verifying with issuer "sigstore.dev"`, which
reads like a bad signature but is only an identity mismatch. The root README's
[Provenance](../README.md#provenance) section has the whole picture: the inventory, which
artifacts are signed from where, and the gates every byte passes before publication.
