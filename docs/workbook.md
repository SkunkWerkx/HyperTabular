# Workbooks — design notes

The shared architecture — the core and what holds it, doors and columns, the cast matrix,
verdicts, the batch, the C ABI, and the prior art — is recorded once in
[`design.md`](design.md) and [`prior-art.md`](prior-art.md). This file holds what is
specific to XLSX and ODS.

Two layers read a workbook. The core (`rust/src/kernel/workbook`) reads the container's
bytes — zip, inflate, XML, cells, casts — into memory its caller owns, and is what
`libhypertabular` exports. `Workbook` and `Sheet` (`rust/src/workbook.rs`) are the Rust
binding over it — the same thing each language's binding is: they hold the container,
own the buffers, grow one when the core asks, and lend out what the core wrote.

## What is fixed here, and why

- **Format detection is by container, not extension.** A zip with `[Content_Types].xml`
  or `xl/workbook.xml` is XLSX; one whose `mimetype` entry says
  `application/vnd.oasis.opendocument.spreadsheet` is ODS; anything else is
  `FailureKind::NotAWorkbook`. Nothing about the *data* is sniffed.
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
   every later call.
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
token at a time for this: a token is looked at, acted on, and only then stepped past.

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
- Lookups that a hostile file could make quadratic — a sheet's part by its relationship
  id, a cell format's number format by its id — are a sort and a search in the caller's
  scratch, not a walk.

## Allocation story

The core allocates nothing: `tests/kernel_workbook_allocation_free.rs` watches four
workbooks go through it end to end with a counting allocator, and the shipped library
imports no allocator. The binding allocates: the container (unless the caller holds it),
the shared strings and format kinds once per workbook, and per sheet a state block, a
window, an arena, a cell table and the column arrays — sized by the first batches, after
which a read allocates nothing (`tests/reader_allocation_free.rs`).

## What holds it to the truth

- `corpus/workbook.json` — 198 cases over the 23 packages in `corpus/workbook/`: every
  sheet, with and without a header, empty rows skipped and delivered, every door over
  every column. The cases for the ten synthetic packages were written by the std workbook
  reader this crate had before the core could read a workbook — a reader that owed the
  core nothing — with the core required to agree; that reader has since been deleted. The
  cases for the thirteen files Excel, LibreOffice and Google Sheets wrote were written by
  the core and read against the application that wrote each file, by eye. **Nothing
  independent of the core reads a workbook here any more**: the corpus is the oracle's frozen word, and
  `tests/corpus_workbook.rs` holds the core to it. Every binding replays the same file.
- The core against itself (`tests/kernel_workbook.rs`): generated packages that reach for
  every shape a cell, a row and a part can take, read with buffers that start empty and
  grow only as asked, and with room to spare — the two must agree.
- Hostile input: truncation, flipped bits, noise, a garbage directory, sizes that lie, a
  deflate bomb. Each comes back as a code.
- Numbers: the core says and reads a double exactly as Rust's own formatter and parser do,
  checked over hundreds of thousands of values.
- What is still owed: a file Apple Numbers wrote. `corpus/README.md` has the list.

## Exports

`hypertabular_workbook_state_size`, `_open`, `_sheets`, `_strings`, `_styles`, `_sheet`,
`_header`, `_fill`, over the `#[repr(C)]` shapes in `kernel/abi.rs`.

## Numbers

Linux x64, 300 000 rows × 8 columns, 100.8 MB of sheet XML in a 15.1 MB deflated package,
one plan, batches of 4096: 604 ms through the core (167 MB/s of sheet XML, 0.50 M rows/s).
The std reader it replaced, inflating with zlib-rs, read the same file in 505 ms
(`tests/kernel_workbook.rs`, `throughput`, measured before that reader was deleted).
Through the Rust binding the read costs what it costs through the core called directly
(630 ms against 627 ms, a later run of the same test). The
difference was long guessed to be the inflate, since the core's own runs at about two
thirds of zlib-rs's speed (`design.md`); measured since (below), it is not.

### Through every binding

The two real-application files of `corpus/README.md` — Excel's `excel-win-300k.xlsx`
(111 MB of sheet XML and 7.7 MB of shared strings in a 16.7 MB package) and LibreOffice's
`libreoffice-300k.ods` (360 MB of `content.xml` in 13.3 MB) — read whole by each binding's
own benchmark harness through one plan: `i64`, `f64`, text, date, time, bool, duration,
`i64` (that last column holds 3 000 `#N/A` error cells, which fault). **open** is the package
opened and nothing read; **read** is opened, then the first sheet read in batches of 4096,
every column's verdicts looked at and every text cell's bytes. Every harness prints the
same checksum (5 285 882: the cells that cast, plus the text column's bytes), which is what
says they did the same work. Linux x64 (WSL 2), i9-11900H, 2026-10-07: the 0.7.0 core and
HyperCast 0.7.0; Rust 1.99, .NET 11 RC 1, JDK 25.0.4 (Temurin), Go 1.27.1, Swift 6.3.3,
CPython 3.14.8, Ruby 4.0.7, PHP 8.5.10 with no `php.ini`. Best of the harness's own runs
where it reports them (Go, Ruby), its mean or median otherwise.

| Binding (harness) | xlsx open | xlsx read | ods open | ods read |
| --- | ---: | ---: | ---: | ---: |
| Rust (Criterion) | 34 ms | 601 ms | 337 ms | 1.20 s |
| C# (BenchmarkDotNet) | 32 ms | 591 ms | 334 ms | 1.21 s |
| Java (JMH) | 41 ms | 600 ms | 346 ms | 1.23 s |
| Go (`go test -bench`) | 30 ms | 590 ms | 328 ms | 1.20 s |
| Swift (package-benchmark) | 37 ms | 617 ms | 336 ms | 1.20 s |
| Python (pyperf) | 39 ms | 630 ms | 344 ms | 1.24 s |
| Ruby (benchmark-ips), Magnus | 30 ms | 1.01 s | 330 ms | 1.61 s |
| Ruby (benchmark-ips), Fiddle | 36 ms | 1.04 s | 343 ms | 1.65 s |
| PHP (phpbench) | 44 ms | 776 ms | 346 ms | 1.34 s |

C#, Java, Go and Swift hand out views of the reader's buffers, so their read is the core's.
The build is layout-sensitive: when this table was first taken, Criterion's binary read the
ODS file about 10% slower than the same read in a plain release program, and compiling with
every function and branch target aligned
(`-C llvm-args=-align-all-functions=6 -C llvm-args=-align-all-nofallthru-blocks=5`) brought
the two within 2% of each other. A difference of under about 10% between two rows of this
table is not a finding until it survives runs alternated in one sitting.
Python's read makes the text column's `str`s and counts faults from
`fault_count`; its **values** scope, which makes every column's Python objects — `date`,
`time` and `timedelta` included — is 717 ms and 1.34 s. Ruby and PHP have no way to ask
whether a cell cast short of decoding its column, so their row is the **values** scope:
every column made into the gem's or package's carriers (Date, Rational, DateTimeImmutable,
HyperCast's Duration), the cells that cast counted from them. Their **verdicts** scope —
a HyperCast `Success` or `Fault` built for every cell, what a caller who matches each cell
pays — is 1.92 s and 2.48 s in Ruby (a Ruby `Data` instance costs about 0.45 µs to make,
however it is made) and 929 ms and 1.50 s in PHP. Ruby's two backends are within a few
percent of each other: the crossing was already once per batch, so the Magnus extension
buys platform gems with no shared library to find, and ruby.wasm, not speed.

ODS's **open** is most of a second because a package has no listing of its sheets outside
`content.xml`, so opening one reads the whole part; the sheet's read then reads it again.
XLSX lists its sheets in a small part of their own.

Where a read's time went (Rust, best of three, measured before the tokenizer fixes that
brought the table above to its numbers): the core's inflate alone takes 117 ms over
the xlsx sheet's 111 MB and 93 ms over the ODS part's 360 MB — it is not the cost. Casting
is not either: with an empty plan the xlsx read is 689 ms and with the full plan 705 ms,
the ODS read 1.66 s and 1.72 s. The rest is the XML — tokenizing the parts and assembling
rows — about 570 ms for the xlsx sheet (195 MB/s) and, for ODS, about 380 ms of the open's
scan and 1.1 s of the read (330 MB/s). That is where the reader is fast or slow. The text
door is the one plan that costs more: all eight columns through it is 1.03 s and 2.01 s,
every typed cell said as canonical text.

Profiled (samply, the ODS read), the XML was most of it in two places. `tag()` found a tag's
`>` a byte at a time, minding quotes, over ODS's long cell tags; it now jumps from one `>`
or quote to the next eight bytes at a time, as `find_byte` finds one byte. And every
attribute lookup walked the tag's attributes from the start — four or five a cell for ODS,
three (`r`, `s`, `t`) for XLSX; each cell's are now gathered in one pass, each the first of
its name as the lookups found it. A third, smaller: the nine-byte `<![CDATA[` comparison,
a `memcmp` call, ran for every ordinary tag until `<!` was asked first. Best of several
runs, Rust, before and after the three: XLSX read 710 → 580 ms, ODS read 1.88 → 1.19 s, ODS
open 530 → 320 ms. The table above was taken after them, every binding on a library rebuilt
from the same core. What is left is spread
thin: inflate is about a sixth of a read, and no other line is more than a few percent.

What these numbers changed when they were first taken: Ruby and PHP each ran a scan over
every span of a batch, in the host language, to find how much of the arena to copy, where
the core's own `arena_used` already says (Ruby's read went from 1.05 s to 0.68 s before any
values were made); and PHP decoded its record doors with an `unpack()` a cell, which is now
one a column (0.28 s to 0.14 s of decoding for the xlsx file). PHP's numbers are taken with
no `php.ini` (`runner.php_disable_ini` in `phpbench.json`): a development install with
Xdebug loaded runs the same benchmark at more than twice the time.

To run them: `cargo bench --bench workbook_benchmarks` (rust/), `dotnet run -c Release
--project HyperTabular.Benchmarks` (csharp/), `./gradlew :benchmarks:jmh` (java/),
`go test -run '^$' -bench Workbook -benchmem` (go/), `swift package benchmark`
(swift/Benchmarks/), `python bench_workbook.py --fast` (python/), `ruby
benchmark/workbook_benchmark.rb` (ruby/), `vendor/bin/phpbench run --report=aggregate` (php/).
Each reads the files from `corpus/generate/out/`, or from `HYPERTABULAR_BENCH_DIR`.

## Parked

- **XLS (BIFF8).** The read-only record set and the CONTINUE-aware string cursor are
  documented in the prior-art record; it waits on a decision that has not been taken.
- **XLSB.** Not requested; the record ids are in the prior-art record if it ever is.
- **Lazy shared strings** (Sylvan's forward cursor) if a workbook with a huge table and a
  small read ever makes the preload the wrong trade.
- **Cell-level styles beyond the number-format kind**, hidden rows/columns, merged
  ranges: not consulted; this reader delivers values.
