# Workbooks — design notes

The shared architecture — the core and what holds it, doors and columns, the cast matrix,
verdicts, the batch, the C ABI, and the prior art — is recorded once in
[`design.md`](design.md) and [`prior-art.md`](prior-art.md). This file holds what is
specific to XLSX, ODS and Numbers.

Two layers read a workbook. The core (`rust/src/kernel/workbook`) reads the container's
bytes — zip, inflate, XML or Apple's IWA objects, cells, casts — into memory its caller owns, and is what
`libhypertabular` exports. `Workbook` and `Sheet` (`rust/src/workbook.rs`) are the Rust
binding over it — the same thing each language's binding is: they hold the container,
own the buffers, grow one when the core asks, and lend out what the core wrote.

## What is fixed here, and why

- **Format detection is by container, not extension.** A zip with `[Content_Types].xml`
  or `xl/workbook.xml` is XLSX; one whose `mimetype` entry says
  `application/vnd.oasis.opendocument.spreadsheet` is ODS; one with neither whose
  `Index/Document.iwa` holds an object 1 that is a `TN.DocumentArchive` naming
  `TN.SheetArchive`s is Numbers; anything else is `FailureKind::NotAWorkbook`. Nothing
  about the *data* is sniffed.
- **Cells are read as what the file stores, then cast.** A numeric `<v>` is a number;
  under a temporal number format it is a wall clock, a time of day or a span, by the
  serial rules; `t="b"` is a boolean; `t="e"` an error; `t="d"` an ISO wall clock; shared,
  inline and formula strings are text. A number the serial rules refuse (negative,
  serial 60, past 9999) stays a number, so that the door reports the fault and not this
  layer. Text goes through the HyperCast door; a typed cell is converted directly —
  `design.md` has the matrix.
- **Formulas are never evaluated.** `<f>` is skipped; the cached value is the value. A
  formula with no cached value is an empty cell.
- **Date-kind classification** (`kernel/workbook/styles.rs`): built-in ids per ISO 29500
  §18.8.30 → date, time, elapsed (46, and Thai 79) or text (49); the ids that are times in
  Chinese and dates in Japanese or Korean (34, 35, 52, 53, 55, 56) → time. Custom codes:
  first `;` section, `"…"` and `\`/`_`/`*` escapes skipped, `[h]`/`[m]`/`[s]` ⇒ elapsed,
  any other `[…]` ignored, `AM/PM` skipped, then `y`, `d` or a month `m` ⇒ date, `h`, `s`
  or a minutes `m` ⇒ time (`m` is minutes after `h` or before `s`). A date format in the
  1904 system has a date from serial `0` (1904-01-01) up; under one day anything else is a
  time of day. A custom id declared twice means what the latest declaration before the
  cell format says.
- **Rows.** `<row r>` gaps and empty `<row>`s are empty rows: skipped by default
  (`skip_empty_rows`), delivered with their sheet row numbers otherwise. Missing `r` on a
  row or cell means "the next one", per ISO 29500. `dimension` is not consulted (it is
  advisory and often stale). Every row of a batch carries its number (`Batch::line`).
- **Header** (`has_header`) is the first delivered row, each cell said the way the text
  door says it (a typed cell as its canonical text; an empty cell as an empty name), up
  to the last column that holds a cell.
- **Sheets are independent.** Each `Sheet` has its own state block and buffers, and
  borrows the workbook's container and tables, so several can be read at once. Hidden
  and very-hidden sheets are listed with their state and read like any other; chart,
  dialog, and macro sheets are not listed (no cells).
- **ODS typing** comes from `office:value-type` and its paired value attribute —
  `office:value`, `office:date-value`, `office:time-value`, `office:boolean-value`,
  `office:string-value` — never from styles. `calcext:value-type="error"` wins over
  `office:value-type="string"` (that is how LibreOffice writes error cells). String cells
  join `text:p` paragraphs with `\n` and expand `text:s`/`text:tab`/`text:line-break`;
  annotations and drawings are skipped. `number-columns-repeated` on an empty cell just
  advances the column; on a value it fills the columns (capped at ODF's 16 384);
  `number-rows-repeated` delivers the row that many times — a repeated *empty* row is one
  parse and (by default) zero deliveries, so LibreOffice's million-row padding costs
  nothing. ODS sheet hidden state lives in styles this reader does not read; it is always
  reported as not hidden.
- **Numbers** (`kernel/workbook/numbers.rs`, over `iwa.rs`, `snappy.rs` and `proto.rs`) is
  Apple's IWA packaging: `.iwa` parts of Snappy-compressed chunks whose decoded stream is a
  sequence of Protocol Buffers objects — the document, its sheets, each sheet's tables,
  each table's model, data lists and tiles. A sheet here is one Numbers *table*, which is
  what has rows and columns and what Numbers itself makes a worksheet of when it exports
  to Excel: it is listed under its sheet's name, or `<sheet> - <table>` when the sheet
  holds more than one, hidden when the sheet is. Cells are the v5 storage Numbers has
  written since 2018 (an older file is `FailureKind::Iwa` with `found` 4 and its version
  in `expected`): a number is the 128-bit decimal said as text and read through the one number
  grammar, or the double a currency cell carries; a date is seconds since 2001-01-01, a
  wall clock, or a time of day when its format shows no date (`suppress_date_format`, or
  a pattern with no date letters — a custom format's pattern is looked up in the
  document's custom-format list by the uid the cell's format names); a duration is a
  span; a boolean a boolean; a text or
  rich-text cell is a span into the table's string or rich-text list, loaded by `strings`
  with the runs of a rich string joined; an error cell is `#VALUE!`, because the file
  records which formula failed, not which error it made; a formula's cached value is the
  value, and the cells a merged cell covers are empty. Rows are the tile's rows: a row the
  tile leaves out is an empty row, delivered with its number when empty rows are, and a
  tile holds 256 rows unless the table's storage says otherwise. Width is capped at
  Excel's 16 384 columns.

## The core's calls

The container is bytes the caller holds — a file it read or mapped — handed over on every
call, with every buffer the call works in (`Buffers`, `kernel/abi.rs`) and a state block
(`hypertabular_workbook_state_size()` bytes, about 27 KiB: the inflate tables, the read
position in the part being read, where the workbook's parts are, and how far the call in
progress has got). The state is offsets and counters and never a pointer.

1. `open` — is it a workbook, which kind, which date system, how large its shared strings
   are.
2. `sheets` — each sheet's name, whether it is hidden, and what `sheet` is given to
   position on it.
3. `strings` and `styles` (XLSX) — the shared strings, unescaped, rich-text runs joined
   and phonetic runs left out, with a span for each; and one number-format kind per cell
   format. Loaded once into buffers of the caller's, which it hands back, read-only, to
   every later call. For Numbers, `strings` loads every table's lists — its strings, its
   rich text with the runs joined, which of its date formats show no date, and where each
   of its tiles is — as sorted records a cell finds by search, and `styles` loads nothing;
   `open`'s `strings_bytes` is what the data-list parts decode to.
4. `sheet` — position a state block on one sheet, with its options. Then `header`, and
   `fill`: up to `max_rows` rows cast through the caller's plan into the caller's
   columns, in the layout a delimited batch has — the same `ColumnSpec`, `ColumnBuffer`,
   `CellVerdict` and `Filled`, the same value per door — so that what reads one batch
   reads the other. A text value says where its bytes are by its span's flag: set, in
   the batch's arena; clear, in the shared strings, which are never copied.

**Growing a buffer.** A part is a compressed stream, and a read of one cannot be rolled
back to where the call began the way a read of delimited text can. So when a call returns
`ERR_WINDOW`, `ERR_ARENA` or `ERR_CELLS`, nothing is undone: the state holds exactly where
the read stopped. The caller makes the buffer the code names at least as large as the
result's `needed`, *keeping what it held*, and makes the same call again; the read goes on
from the token it stopped at. (`open` alone starts over.) Each row reader is written a
token at a time for this: a token is looked at, acted on, and only then stepped past. A
Numbers listing or loading starts over instead — nothing it wrote was read before it was
done, and the chunks already decoded stay in the window — while `fill` resumes at the cell
it stopped at.

## Container and streaming

- `zip.rs`: the central directory read where it lies — the end record found from the
  tail, zip64 honoured, entries stored or deflated only, encryption and other methods
  refused. No list of entries is kept; a part is found by walking the directory again.
  Local headers are read only for the data offset. Names are matched
  ASCII-case-insensitively with any leading `/` ignored (OPC's rules; third-party writers
  root parts inconsistently). Every offset and length is believed only as far as the
  container's own length allows.
- `inflate.rs` (`kernel/`): the core's own inflate — see `design.md`. A deflated part is
  inflated into a *window* of the caller's, which doubles as the inflate dictionary: at
  least 64 KiB, slid as the read moves on, and grown only when one token (a very long
  string, say) will not fit it. A deflate bomb is therefore only as large as the window
  the caller is willing to give. A stored part needs no window: it is tokenized where it
  lies.
- `xml.rs`: one pull tokenizer for every part, with no state of its own — it is asked for
  the token at a position and answers with offsets, or says the token runs past what the
  buffer holds. Start/empty/end tags with lazy attributes, text, CDATA; comments, PIs,
  doctypes stepped over; namespace prefixes ignored. Tag scanning is quote-aware; text
  scanning is an eight-bytes-at-a-time search. Entities are decoded only when a `&` is
  present.
- `iwa.rs`: Apple's IWA packaging. Every `.iwa` part is chunks — a zero byte, a three-byte
  length, a Snappy block — whose decoded stream is varint-prefixed `TSP.ArchiveInfo`
  headers each followed by its messages. The loader finds an object by its identifier and
  hands its first message over as bytes, keeping in the window only the chunks that hold
  the object asked for last (a tile's while a tile is read, never a whole part) and in the
  state block where they came from. A deflated part is inflated whole into the window's
  far end first. `snappy.rs` decodes a block, every offset and length believed only as far
  as the block and the buffer allow; `proto.rs` walks a message's fields in place.
- Lookups that a hostile file could make quadratic — a sheet's part by its relationship
  id, a cell format's number format by its id, a Numbers object by its identifier, a
  Numbers cell's string by its key — are a sort and a search in the caller's scratch, not
  a walk.

## Opening, and the plan

- A workbook is read from its end — a zip's directory is there — so a stream
  (`Workbook::from_reader`, and `from_async_reader` behind the `async` feature) is read
  whole into memory the workbook owns before the core is handed it.
- A sheet opens with a plan, or without one (`Workbook::sheet_unbound`) to read its header
  first and `bind` the plan after. The core holds the header row's cells in the caller's
  row slots as well as naming them, because a row an ODS sheet repeats is delivered again
  from those slots; so a sheet without a plan gives the header call a slot for every name
  (the slots grow with the cell table), and binding only ever grows them. Every binding
  does the same, and `generated-a.ods` in the corpus is the case that holds them to it.
- A plan's ordinals are capped as a delimited plan's are: an ordinal must be below 2^20
  (1,048,576), more columns than any of the three formats has, so a sheet's row slots are
  at most 16 MiB. A sheet's header does not fix the width of the rows below it — a row may
  hold cells past the header's last name — so a sheet plan may read past it.

## Allocation story

The core allocates nothing: `tests/kernel_workbook_allocation_free.rs` watches five
workbooks go through it end to end with a counting allocator, and the shipped library
imports no allocator. The binding allocates: the container (unless the caller holds it),
the shared strings and format kinds once per workbook, and per sheet a state block, a
window, an arena, a cell table and the column arrays — sized by the first batches, after
which a read allocates nothing (`tests/reader_allocation_free.rs`).

## What holds it to the truth

- `corpus/workbook.json` — 246 cases over the 27 packages in `corpus/workbook/`: every
  sheet, with and without a header, empty rows skipped and delivered, every door over
  every column. The cases for the ten synthetic XLSX and ODS packages were written by the
  std workbook reader this crate had before the core could read a workbook — a reader that
  owed the core nothing — with the core required to agree; that reader has been
  deleted. The cases for the fifteen files Excel, LibreOffice, Google Sheets and Numbers
  wrote, and for the two generated Numbers packages, were written by the core and read
  against the application that wrote each file, by eye. **Nothing
  independent of the core reads a workbook here**: the corpus is the oracle's frozen word, and
  `tests/corpus_workbook.rs` holds the core to it. Every binding replays the same file.
- The core against itself (`tests/kernel_workbook.rs`): generated packages that reach for
  every shape a cell, a row and a part can take, read with buffers that start empty and
  grow only as asked, and with room to spare — the two must agree.
- Hostile input: truncation, flipped bits, noise, a garbage directory, sizes that lie, a
  deflate bomb. Each comes back as a code. A part cut between two rows — by the file, or
  by a directory that declares it that long — delivers the rows before the cut and then
  its failure, never a shorter sheet; and the size a directory declares for the string
  table is held to what the part's bytes can inflate to before a caller sizes an arena by
  it.
- Doubles: the core says and reads a double exactly as Rust's own formatter and parser
  do, checked over hundreds of thousands of values.
- Numbers: generated packages in every layout the reader has to be indifferent to —
  deflated or stored parts, chunks of a few bytes or one, tiles of seven rows or 256,
  narrow or wide offsets, a segmented string list, rows left out — read stingy against
  roomy; a table of every kind of cell read back as written through the Rust binding;
  and each way a package can be wrong named by its failure (a storage version the core
  does not read, an object a reference names that is not there, a chunk that is not one,
  a string a cell names that the list does not have, an object 1 that is not a document).
  `numbers-data.numbers`, which Numbers itself wrote, and `numbers-data.xlsx`, its Excel
  export, are read with the rest of `corpus/workbook/`; `corpus/README.md` lists the twelve
  cells where the export says something else.
- Owed: whether a Keynote or Pages package is refused at `open` or lists no sheets has not
  been checked against a file; neither has a `.numbers` file Numbers wrote before 2018.

## Exports

`hypertabular_workbook_state_size`, `_open`, `_sheets`, `_strings`, `_styles`, `_sheet`,
`_header`, `_fill`, over the `#[repr(C)]` shapes in `kernel/abi.rs`.

## Figures

Linux x64, 300 000 rows × 8 columns, 100.8 MB of sheet XML in a 15.1 MB deflated package,
one plan, batches of 4096: 604 ms through the core (167 MB/s of sheet XML, 0.50 M rows/s;
`tests/kernel_workbook.rs`, `throughput`). Through the Rust binding the read costs what it
costs through the core called directly (630 ms against 627 ms, a later run of the same test).

### Through every binding

The three real-application files of `corpus/README.md` — Excel's `excel-win-300k.xlsx`
(111 MB of sheet XML and 7.7 MB of shared strings in a 16.7 MB package), LibreOffice's
`libreoffice-300k.ods` (360 MB of `content.xml` in 13.3 MB) and Numbers'
`numbers-300k.numbers` (29.4 MB of Snappy-compressed IWA parts, stored in a 29.6 MB
package) — read whole by each binding's own benchmark harness through one plan: `i64`,
`f64`, text, date, time, bool, duration, `i64` (that last column holds 3 000 `#N/A` error
cells, which fault; Numbers has no `NA()`, so its file has 3 000 empty cells there, which
fault the same). **open** is the package
opened and nothing read; **read** is opened, then the first sheet read in batches of 4096,
every column's verdicts looked at and every text cell's bytes. Every harness prints the
same checksum (5 285 882: the cells that cast, plus the text column's bytes), which is what
says they did the same work. Linux x64 (WSL 2), i9-11900H, 2026-10-24, one sitting: the
core at `00a511d` (0.9.0, unreleased), built from the checkout for every binding, and
HyperCast 0.9.0; Rust 1.99.1, .NET 11 RC 2, JDK 25.0.4.1 (Temurin), Go 1.27.3, Swift 6.3.3,
CPython 3.15.0, Ruby 4.0.7, PHP 8.5.11 with no `php.ini`. Best of the harness's own runs
where it reports them (Go, five runs); otherwise its mean (JMH, BenchmarkDotNet, pyperf,
benchmark-ips), median (Criterion, package-benchmark) or mode (phpbench). The last row is
numbers-parser 4.22.0, the Python reader every other reader of `.numbers` is held against,
doing the Python harness's **values** work on the same file (`python/bench_numbers_parser.py`,
best of five); it decodes the whole document at open, so its read is its open.

| Binding (harness) | xlsx open | xlsx read | ods open | ods read | numbers open | numbers read |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust (Criterion) | 32 ms | 565 ms | 322 ms | 1.19 s | 820 ms | 1.16 s |
| C# (BenchmarkDotNet) | 31 ms | 586 ms | 332 ms | 1.23 s | 873 ms | 1.22 s |
| Java (JMH) | 39 ms | 606 ms | 343 ms | 1.25 s | 884 ms | 1.25 s |
| Go (`go test -bench`) | 30 ms | 593 ms | 329 ms | 1.20 s | 842 ms | 1.21 s |
| Swift (package-benchmark) | 31 ms | 617 ms | 341 ms | 1.24 s | 942 ms | 1.32 s |
| Python (pyperf) | 39 ms | 625 ms | 337 ms | 1.24 s | 845 ms | 1.22 s |
| Ruby (benchmark-ips), Magnus | 29 ms | 1.00 s | 341 ms | 1.64 s | 827 ms | 1.62 s |
| Ruby (benchmark-ips), Fiddle | 34 ms | 995 ms | 343 ms | 1.68 s | 885 ms | 1.69 s |
| PHP (phpbench) | 41 ms | 775 ms | 345 ms | 1.42 s | 890 ms | 1.44 s |
| numbers-parser 4.22.0 (Python) | | | | | 66.8 s | 68.1 s |

C#, Java, Go and Swift hand out views of the reader's buffers, so their read is the core's.
The build is layout-sensitive: Criterion's binary has read the ODS file about 10% slower
than the same read in a plain release program, and compiling with every function and branch
target aligned
(`-C llvm-args=-align-all-functions=6 -C llvm-args=-align-all-nofallthru-blocks=5`) brings
the two within 2% of each other. A difference of under about 10% between two rows of this
table is not a finding until it survives runs alternated in one sitting.
Python's read makes the text column's `str`s and counts faults from
`fault_count`; its **values** scope, which makes every column's Python objects — `date`,
`time` and `timedelta` included — is 698 ms, 1.35 s and 1.29 s. Ruby and PHP have no way to
ask whether a cell cast short of decoding its column, so their row is the **values** scope:
every column made into the gem's or package's carriers (Date, Rational, DateTimeImmutable,
HyperCast's Duration), the cells that cast counted from them. Their **verdicts** scope —
a HyperCast `Success` or `Fault` built for every cell, what a caller who matches each cell
pays — is 1.95 s, 2.54 s and 2.55 s in Ruby (a Ruby `Data` instance costs about 0.45 µs to
make, however it is made) and 926 ms, 1.55 s and 1.60 s in PHP. Ruby's two backends are
within a few percent of each other: the crossing is once per batch, so the Magnus
extension buys platform gems with no shared library to find, and ruby.wasm, not speed.

ODS's **open** is most of a second because a package has no listing of its sheets outside
`content.xml`, so opening one reads the whole part; the sheet's read then reads it again.
XLSX lists its sheets in a small part of their own.

Numbers' **open** is most of its read: opening a package walks the document's objects to
list its tables and loads every list a cell indexes — 2.1 MB of string list in this file —
so the read that follows has only the tiles left, about 340 ms of the 1.16 s. Where the
open's time went, sampled, is under "Parked"; it is the next candidate. Swift's Numbers open,
942 ms against the core's 820, is the one cell of the table more than 10% from its column in
this sitting, and has not been run alternated. On an Apple M-series laptop, 2026-10-24, the
core read the file in 0.60 s (0.43 s of it the open) against numbers-parser's 21 s: not table
numbers, the machine differs, but the same ratio.

Where a read's time goes, profiled (samply) on the core the table was taken on: the XML —
tokenizing the parts and assembling rows. Inflate is about a sixth of a read, and no other
line is more than a few percent.

PHP's numbers are taken with no `php.ini` (`runner.php_disable_ini` in `phpbench.json`): a development install with
Xdebug loaded runs the same benchmark at more than twice the time.

To run them: `cargo bench --bench workbook_benchmarks` (rust/), `dotnet run -c Release
--project HyperTabular.Benchmarks` (csharp/), `./gradlew :benchmarks:jmh` (java/),
`go test -run '^$' -bench Workbook -benchmem` (go/), `swift package benchmark`
(swift/Benchmarks/), `python bench_workbook.py --fast` (python/), `ruby
benchmark/workbook_benchmark.rb` (ruby/), `vendor/bin/phpbench run --report=aggregate` (php/);
`python bench_numbers_parser.py` (python/, `pip install numbers-parser` first) is the
numbers-parser row. Each reads the files from `corpus/generate/out/`, or from
`HYPERTABULAR_BENCH_DIR`. For a table taken on one core, build it from the checkout for the
three bindings that otherwise link or load a committed binary
(`.github/scripts/local-core.sh`, then Go with `-tags hypertabular_local`, Swift with
`HYPERTABULAR_LOCAL_CORE=1` and PHP with `HYPERTABULAR_NATIVE_LIBRARY` naming
`rust/target/release/libhypertabular.so`), and clear any library staged earlier under
`csharp/HyperTabular/runtimes/`, `java/src/main/resources/native/`, `ruby/lib/hypertabular/native/`
or `ruby/lib/hypertabular/<minor>/` (the Magnus extension `rake native:dev` stages): those are
git-ignored, loaded in preference to `rust/target/release/`, and a stale one reads as a core
that does not know the newest format.

## Parked

- **XLS (BIFF8).** The read-only record set and the CONTINUE-aware string cursor are
  documented in the prior-art record; it waits on a decision that has not been taken.
- **XLSB.** Not requested; the record ids are in the prior-art record if it ever is.
- **Lazy shared strings** (Sylvan's forward cursor) if a workbook with a huge table and a
  small read ever makes the preload the wrong trade.
- **Cell-level styles beyond the number-format kind**, hidden rows/columns, merged
  ranges: not consulted; this reader delivers values.
- **Numbers' error kinds.** An error cell names the formula that failed and nothing of
  what error it made, in anything this core reads, so every one is `#VALUE!`. If a field
  that does carry the kind turns up, it goes here.
- **Numbers' open.** Most of a `.numbers` read (820 ms of the table's 1.16 s). Where its
  time went, sampled on an Apple M-series laptop, 2026-10-24: about a third decoding chunk
  prefixes again while the index walks a part's object headers (`iwa::ensure` decodes the
  chunk in hand from its start each time more of it is needed, quadratic in the objects a
  chunk holds, and the calculation-engine part holds thousands), a sixth the walk over the
  string list's entries, the rest spread over the heap sort and the zip directory.
- **Performance candidates** left after the 2026-10-05 tokenizer pass (SWAR tag scan,
  single-pass attributes, `<!` before `<![CDATA[`): inflate, about a sixth of a read, which
  must stay under the no-panic proof; the bounds checks in `Attrs::next`, `trimmed` and
  `slice`; and ODS open's full-part scan. A/B with two saved binaries, alternated, best-of;
  the build is layout-sensitive and the machine noise is 3 to 5%.
