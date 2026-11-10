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
- **A column's number format meets only a text cell.** A typed number has no notation, so it's
  the same under any format; a text cell holding `1,23,456` is read by the format that column
  declares, Western unless it declares Indian (`corpus/workbook/grouping.*`). A retired grouping
  bit in a plan is refused at the bind, before any cell, as in a delimited read.
- **Text that isn't valid UTF-8** is judged as a delimited file's is. A workbook declares
  its own encoding and the core reads no other, so a part's bytes that aren't UTF-8, in a
  shared, inline or Numbers string alike, are a `Malformed` verdict on a cell read through the
  text door, and `FailureKind::Encoding` in a header name, with the part and sheet row where
  a workbook's failures put them. Character references can't produce it: an unusable one
  stays as written. A text cell is asked about for itself, since its bytes aren't one region
  of the part, with one exception: an XLSX shared string is asked about once, when the strings
  are loaded, and the answer travels in its own table entry (the top bit of its length, which
  no string is long enough to need). A cell naming a string that passed is handed its bytes
  without a second look; one naming a string that didn't is asked again, so its fault and span
  are that cell's own. The answer is the workbook's, in the table its caller holds: two
  workbooks open at once share nothing. An inline string, an ODS cell and a Numbers string are
  still asked about each time.
- **Formulas are never evaluated.** `<f>` is skipped; the cached value is the value. A
  formula with no cached value reads as an empty cell, which `_fill_with` marks: "Cells" under
  "Reading part of a sheet" says what counts as one in each format.
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
  value (no Numbers cell is known to lack one: "Cells" says what was checked), and the cells a
  merged cell covers are empty. Rows are the tile's rows: a row the
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
   the batch's arena; clear, in the shared strings, which are never copied. `sheet_with` and
   `fill_with` are the same two calls with what's next, "Reading part of a sheet".

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

## Reading part of a sheet

`hypertabular_workbook_sheet_with` takes `RawSheetOptions`: `sheet`'s two switches, `skip`,
`header_row` and a range. `hypertabular_workbook_fill_with` takes `fill`'s calls in the size-tagged
shapes delimited text has, `ColumnSpecWith`, `ColumnBufferWith` and `BuffersWith`, for tokens, date
options, notes and row widths. Either works without the other, and every size is read before what
it describes. A refusal of what's declared writes nothing. A column declares nothing more and its
values and verdicts are the ones `fill` gives; every corpus package is read both ways to hold that.

**The order.** A sheet's rows pass through a range, then `skip`, then the empty-row filter and
`header_row`, then the header, and the first row left is data.

- The range keeps rows and columns: `A5:F200` is `5,1,200,6` as `hypertabular_range_parse` reads it,
  1-based and included, in capitals, `XFD` at the widest. In `RawSheetOptions` a `0` means no limit
  on that side, which `A5:F200` has no way to write. Past the sheet's end a range adds nothing: no
  row is invented after the last one stored, however large `last_row` is.
- `skip` counts rows of the sheet inside the range, empty ones and the rows of a gap in the
  numbering included.
- `header_row` counts what the filter leaves: with empty rows skipped, an empty row doesn't count.
  So a sheet with a blank line above its title takes the same `header_row` whichever spelling of
  the blank the file used, and `header_row` means what it means in delimited text, whose skipped
  blank lines don't count either. It's the one place the order isn't simply the list above: the
  filter runs during the counting, not after it.
- A plan's ordinals count from the range's first column. A row's number and a failure's position
  stay the sheet's own: the row called `5` in the file is `5` in the batch, whatever was passed
  over.

**What isn't read.** A row before the range or wholly inside `skip` is stepped past unread, and so
is a cell outside the range's columns, so a shared string that isn't there means nothing there. Each
is still tokenized as XML, or for Numbers stepped over by its offsets. A row `header_row` discards
is read, since the filter has to see it, and a broken one fails the read. The read ends on reaching
`last_row`: no row after it is looked at, so a failure in one isn't met, exactly as it isn't when a
caller stops reading there. A failure that a row before `last_row` reaches is reported after the
intact rows, as ever.

Opening a workbook comes before any range, and reads what it reads whatever one says. So a break in
what opening reads is met there: an ODS's `content.xml` is read through to list its tables, so its
XML breaking anywhere fails the open, and a Numbers document's parts are walked to index their objects,
so a chunk that isn't one fails it too. What a range saves in those formats is a failure in a cell:
a Numbers cell naming a string the list doesn't have, past the range, isn't met. An XLSX sheet's part is
read only when that sheet is, so a range saves its XML as well.

**A repeat is counted, never walked.** ODS's `number-rows-repeated` can say four billion in a few
bytes, as can a gap in XLSX row numbers. Every limit above is arithmetic on the row's first number
and how many rows it stands for, so a row cut by a range's edge, or by `skip`, costs what one row
costs, and a repeated cell likewise in the range's columns.

**Cells.** In a `fill_with`, a cell's note says what's known beyond its verdict:

- A text cell whose exact bytes are a declared token is `Empty`, at origin `token`. A column's own
  list replaces the sheet's. Only text cells can be one: a number whose text would match is a number,
  a boolean a boolean, an error cell its text. An empty text cell matches `""`, and a cell with
  nothing in it matches nothing.
- A cell the row doesn't have is `Empty` at origin `padded`: an ordinal past the row's last occupied
  column inside the range, whatever the plan reads, or any ordinal of a row with nothing in it. A hole
  inside the row is a cell, `Empty` with detail `blank`, like a merged cell's covered part when
  something follows it; a covered cell at the range's trailing edge doesn't extend the row, so it's
  `padded`.
- A formula with no cached result is `Empty` at origin `uncached`, with no detail, and no token
  matches it. In XLSX that's `<f>` with no `<v>`, or an empty `<v>` in a cell not declared a string:
  openpyxl writes one spelling or the other by which XML writer it has, and Numbers' export writes the
  first for every formula that errs and for `=""`. In ODS it's `table:formula` with a value type of
  float, percentage, currency, boolean, date or time and no value of it, whatever `text:p` shows, or
  with no value type and no `text:p`. What's left of a formula that looks empty has a result of no
  text: XLSX's `t="str"` with an empty `<v>`, which is how Excel, Google and LibreOffice write `=""`,
  ODS's `office:value-type="string"`, whose value is its text, and ODS's `<text:p/>` with no type,
  LibreOffice's and Google's `=""`. Those read as they always did, `Empty` at origin `cell` or `padded`.
  An ODS value of spaces isn't one: it stays the text it always read as. An uncached formula makes a
  row as wide as it reaches. A `_fill` says plain `Empty` throughout.
- **To keep every row, set `skip_empty_rows=false`.** **The default read can lose those rows, and says nothing.** The filter doesn't count a formula with
  nothing spelled where a value goes, so a row whose only cells are such formulas is an empty row, and
  `skip_empty_rows` leaves it out. Which it is depends on how the writer spells it: XLSX's `<f>` alone
  or with `<v />`, and ODS's `table:formula` with no type, count as empty; `<v></v>`, and a typed ODS
  cell, hold a cell. So one openpyxl script gives different rows by whether lxml is installed, and a
  table of nothing but formulas from such a writer can read as its header and no rows. Turning the
  filter off delivers those rows, every cell marked. 1.0 read them this way, and `_fill` and
  `_fill_with` deliver the same rows. `hypertabular_faults` and `Batch::faults` list a batch's cells,
  so they can't name a row the filter left out: they list an uncached formula in a row that holds
  something else, and in every row once the filter is off, given notes.
- **Numbers has no vector for that, because no such cell has been found.** The storage could spell
  one: a formula's identifier is a flag apart from a cell's type (`0x200`, in `numbers.rs`), so the
  form to look for is flag `0x200` with no value stored, for any type. An error cell, which stores no
  value, is its own result. In `numbers-data.numbers`, the only file here that Numbers wrote with
  formulas in it, all nine flagged cells have a result: a number, two dates, two strings and a boolean
  with their values, and three error cells. `=""` is a text cell naming a string. None of the 19 cells
  of no type there, or the 3,000 in `numbers-300k.numbers`, carries the flag. This isn't a claim that
  Numbers can't write one. If it turns out to, it reads today as a blank, or as `false` for a boolean,
  and it would join with a vector.
- A text cell's fault carries HyperCast's detail from its detailed door. So does a typed cell's,
  from the door's twin, `i32_from_f64` and so on, or `excel_serial` for a stored date under its
  declared epoch. A cell of a kind the door has no twin for, a boolean through an integer door, is
  `Malformed` with no detail, since no door was asked.
- Date options and month names reach text cells at the date doors. A stored date has been parsed
  already, so they don't touch it.
- A row's width, in `row_widths`, runs to the last occupied column inside the range, whatever the
  plan reads, and is `0` for an empty row. Only a writer that names a cell twice can make it say more:
  a cell later named again as empty still counts, and reads as a hole, since the row has no memory to
  work back through columns it kept no slot for.

A row-wise fill has nowhere to keep what judging a column found, so a column's notation and date
options are judged again per text cell at their doors. Both were judged when the fill began, so this
never changes an answer, only what it costs.

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

How much a sheet's batch may ask for is the core's answer, before any of that is allocated
(`hypertabular_batch_check`; `docs/design.md`, "Errors"). A sheet's cell table is a whole batch, so it
counts `batch_rows` × (plan columns + 1) entries, and a sheet with no plan stops at 2^29 rows. The row
slots count too, one per column up to the furthest the plan names. A suggestion reads its sample in as
many batches as the core says fit, so a sheet is never refused for the size of the batch it picked.

A sheet's sample is read through twice its columns: a text door for each, and a UUID door beside it,
whose verdict says which cells were typed. That second door costs about 0.45 ms on a 1,000-row
sample of the eight-column files here, about a fifth of a whole `Sheet::suggest` (2.2 ms on the
xlsx, 2.6 ms on the ods, 2.0 ms on the Numbers file): the sample's read through both is 1.73, 2.17
and 1.52 ms, against 1.26, 1.73 and 1.09 ms through the text doors alone. The notes a sample turns
on cost under 0.5%. Apple M6, Rust 1.99.1, 2026-11-08, the median of 41 runs of each, interleaved,
on a workbook already open; a 4,096-row sample costs four times as much, in the same proportion.

## What holds it to the truth

- `corpus/workbook.json` — 398 cases over the 41 packages in `corpus/workbook/`: every
  sheet, with and without a header, empty rows skipped and delivered, every door over
  every column. The cases for the ten synthetic XLSX and ODS packages were written by the
  std workbook reader this crate had before the core could read a workbook — a reader that
  owed the core nothing — with the core required to agree; that reader has been
  deleted. The cases for the fifteen files Excel, LibreOffice, Google Sheets and Numbers
  wrote, and for the two generated Numbers packages, were written by the core and read
  against the application that wrote each file, by eye. The cases for `ranges.*`, `uncached.*` and `dates.*`,
  eight packages built in `tests/support/packages.rs`, were written by the core and are held to tables
  stated by hand in `tests/kernel_workbook_rules.rs` and, for `dates.*`, `tests/corpus_workbook.rs`. **Nothing
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
  do, but for white space after a leading sign, which HyperCast reads and Rust refuses,
  checked over hundreds of thousands of values.
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
`_sheet_with`, `_header`, `_fill`, `_fill_with`, `hypertabular_range_parse`,
`hypertabular_faults`, `hypertabular_suggest` and `hypertabular_batch_check`, over the `#[repr(C)]` shapes in `kernel/abi.rs`.
A workbook passes `hypertabular_faults` `long_rows` `0`, having no such policy.

## Figures

Linux x64, 300 000 rows × 8 columns, 100.8 MB of sheet XML in a 15.1 MB deflated package,
one plan, batches of 4096: 604 ms through the core (167 MB/s of sheet XML, 0.50 M rows/s;
`tests/kernel_workbook.rs`, `throughput`). Through the Rust binding the read costs what it
costs through the core called directly (630 ms against 627 ms, a later run of the same test).
Those are receipts from before 1.0, not run again since.

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
says they did the same work. Apple M6 (Mac18,5, 32 GB), macOS 27.2, 2026-11-08, one session, one
job at a time (load average 1.6 to 3.1): one build of the core as this release ships it for every
binding, with HyperCast 1.1.0; Rust 1.99.1, .NET 11 RC 3 (BenchmarkDotNet 0.16 preview 2,
each benchmark in its own process), JDK 25.0.5 (Temurin, the FFM path), Go 1.27.3, Swift 6.4,
CPython 3.15.2, Ruby 4.0.9, PHP 8.5.12 with no `php.ini`. Rust's row is the median of three
interleaved runs' Criterion medians, Go's benchstat's median of six; otherwise each harness's own
figure: its mean (JMH, BenchmarkDotNet, pyperf, benchmark-ips), median (package-benchmark) or mode
(phpbench). The last row is numbers-parser 4.22.2 on CPython 3.14.10, the Python reader every other
reader of `.numbers` is held against, doing the Python harness's **values** work on the same file
(`python/bench_numbers_parser.py`, best of five); it decodes the whole document at open, so its
read is its open.

| Binding (harness) | xlsx open | xlsx read | ods open | ods read | numbers open | numbers read |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Rust (Criterion) | 13.7 ms | 264 ms | 189 ms | 595 ms | 470 ms | 683 ms |
| C# (BenchmarkDotNet) | 14.3 ms | 267 ms | 191 ms | 602 ms | 474 ms | 688 ms |
| Java (JMH), FFM on JDK 25 | 14.6 ms | 269 ms | 195 ms | 602 ms | 473 ms | 690 ms |
| Go (`go test -bench`), the translated core | 28.8 ms | 483 ms | 380 ms | 1.17 s | 823 ms | 1.14 s |
| Swift (package-benchmark) | 15 ms | 266 ms | 188 ms | 592 ms | 464 ms | 679 ms |
| Python (pyperf) | 14.8 ms | 277 ms | 190 ms | 610 ms | 460 ms | 682 ms |
| Ruby (benchmark-ips), Magnus | 13.6 ms | 458 ms | 188 ms | 787 ms | 466 ms | 875 ms |
| Ruby (benchmark-ips), Fiddle | 14.2 ms | 704 ms | 193 ms | 1.02 s | 480 ms | 887 ms |
| PHP (phpbench), the extension | 13.9 ms | 362 ms | 188 ms | 707 ms | 477 ms | 788 ms |
| numbers-parser 4.22.2 (Python) | | | | | 21.4 s | 22.2 s |

The table is the 2026-10-24 one taken again on another machine: that one was Linux x64 (WSL 2) on an
Intel Core i9-11900H, with the core at `00a511d`, and its rows aren't comparable with these.

- **Go's row is the translated core,** and every cell of it is a loss of 1.7–2.3x against the
  native core. Measured in the same session against the cgo binding it replaced, over 1.0's core
  and over this one: the xlsx read 483 ms against 259 and 269 ms, the ods read 1.17 s against 586
  and 600 ms, the Numbers read 1.14 s against 683 and 686 ms, with several times the memory.
  [`go/README.md`](../go/README.md#benchmarks) has every row.
- **Ruby's Fiddle path is the slower one now** on the files with more cells to decode: 704 ms
  against Magnus's 458 ms on the xlsx read and 1.02 s against 787 ms on the ods read, level on the
  Numbers read (887 against 875 ms). A second run of both, Fiddle first, gave 704 and 458 ms, and
  1.00 s and 794 ms. Where it goes isn't measured.
- **The release's own cost, against 1.0, through the bindings that changed how they read**, measured
  on this machine on 2026-11-07 with a plain timing loop, not these harnesses. These predate the
  changes of 2026-11-08 to Ruby's read path (a guard against calls back in, and an exception in a
  fetch made final) and to Python's handling of interrupts, and haven't been run again since: PHP's extension, which
  now reads through the Rust API where 1.0's drove the core directly, reads an xlsx sheet of 500k
  rows and 3 columns in 152.3 ms against 143.3 ms, an 87 MiB delimited file of 2M rows and 6 columns
  in 67.4 ms against 63.7 ms (from a string, 63.2 ms against 64.4 ms), and a file of 2M rows whose 6M
  cells are all bad in 118.1 ms against 106.0 ms. Python's extension, moved the same way: the xlsx
  sheet in 107.0 ms against 101.2 ms, a 97 MiB delimited file in 83.9 ms against 78.6 ms and from
  `bytes` in 75.2 ms against 73.9 ms, and the all-bad file in 98.7 ms against 87.4 ms. Ruby, with
  the size-tagged fills and the new reader surface, Magnus then Fiddle: the xlsx sheet in 146.7 and
  146.4 ms against 139.4 and 140.4 ms, a 92 MiB delimited file in 54.8 and 57.1 ms against 51.3 and
  53.1 ms, and the all-bad file, with every verdict, in 4.57 and 4.74 s against 4.08 and 4.09 s.
  The core's own, file by file, is under "What this release costs the core" below.
- TypeScript, R and C++ have no benchmark harness yet.

C#, Java and Swift hand out views of the reader's buffers, so their read is the core's. Go's
does too, through the translated core, which is what its row measures.
The build is layout-sensitive: Criterion's binary has read the ODS file about 10% slower
than the same read in a plain release program, and compiling with every function and branch
target aligned
(`-C llvm-args=-align-all-functions=6 -C llvm-args=-align-all-nofallthru-blocks=5`) brings
the two within 2% of each other. A difference of under about 10% between two rows of this
table is not a finding until it survives runs alternated in one sitting.
Python's read makes the text column's `str`s and counts faults from
`fault_count`; its **values** scope, which makes every column's Python objects — `date`,
`time` and `timedelta` included — is 317 ms, 644 ms and 718 ms. Ruby and PHP have no way to
ask whether a cell cast short of decoding its column, so their row is the **values** scope:
every column made into the gem's or package's carriers (Date, Rational, DateTimeImmutable,
HyperCast's Duration), the cells that cast counted from them. Their **verdicts** scope —
a HyperCast `Success` or `Fault` built for every cell, what a caller who matches each cell
pays — is 829 ms, 1.16 s and 1.25 s in Ruby through Magnus and 1.12 s, 1.39 s and 1.27 s through
Fiddle, and 409 ms, 740 ms and 840 ms in PHP. The crossing is once per batch, so the Magnus
extension was chosen for platform gems with no shared library to find, and ruby.wasm, not speed;
it is the faster path today all the same, as the table says.

ODS's **open** is most of a second because a package has no listing of its sheets outside
`content.xml`, so opening one reads the whole part; the sheet's read then reads it again.
XLSX lists its sheets in a small part of their own.

Numbers' **open** is most of its read: opening a package walks the document's objects to
list its tables and loads every list a cell indexes — 2.1 MB of string list in this file —
so the read that follows has only the tiles left, about 210 ms of the core's 683 ms. Where the
open's time went, sampled, is under "Parked"; it is the next candidate. Every binding but Go is
within 10% of the core on the opens and on the reads that look at verdicts; Ruby's and PHP's reads
decode every value. numbers-parser takes 32 times the core's time for the same file.

Where a read's time goes, profiled (samply) on the core at `00a511d`, 2026-10-24: the XML —
tokenizing the parts and assembling rows. Inflate is about a sixth of a read, and no other
line is more than a few percent.

PHP's numbers are taken with no `php.ini` (`runner.php_disable_ini` in `phpbench.json`): a development install with
Xdebug loaded runs the same benchmark at more than twice the time.

To run them: `cargo bench --bench workbook_benchmarks` (rust/), `dotnet run -c Release
--project HyperTabular.Benchmarks` (csharp/), `./gradlew :benchmarks:jmh` (java/),
`go test -run '^$' -bench Workbook -benchmem` (go/), `swift package benchmark`
(swift/Benchmarks/), `python bench_workbook.py --fast` (python/), `ruby
benchmark/workbook_benchmark.rb` (ruby/), `composer bench` (php/, after `cargo php-ext` in
rust/); `python bench_numbers_parser.py` (python/, `pip install numbers-parser` first) is the
numbers-parser row. Each reads the files from `corpus/generate/out/`, or from
`HYPERTABULAR_BENCH_DIR`. For a table taken on one core, start from
`.github/scripts/local-clean.sh`, which removes every library a binding stages and a later run
would load in preference to `rust/target/release/` (a stale one reads as a core that does not
know the newest format), then build each binding's core from the checkout as its README says:
`cargo cdylib` in rust/ for the bindings that load the shared library (and `cargo build
--release --locked --lib` in java/jni/ beside it for Java),
`.github/scripts/local-core.sh` for Swift, `cargo php-ext` for PHP, and `go generate` in go/
after any change to rust/, since Go's core is the committed translation.

### What validating text costs

Apple M6, Rust 1.99, 2026-11-06: the same tree with and without the text door's validation,
interleaved, the median of three runs of Criterion's median, opening and reading each file.
The three real files hold one column of text in eight. `text_heavy` is generated, 100 000
rows of eight shared strings through the text door, half of the table multi-byte. The earlier
tables predate validation.

| File | Without | With | Change |
| --- | ---: | ---: | ---: |
| `text_heavy` (read only) | 48.0 ms | 57.3 ms | +19.3% |
| `excel-win-300k.xlsx` | 251.0 ms | 254.2 ms | +1.3% |
| `libreoffice-300k.ods` | 574.2 ms | 580.4 ms | +1.1% |
| `numbers-300k.numbers` | 662.1 ms | 665.2 ms | +0.5% |

### What this release costs the core

Apple M6, Rust 1.99.1, 2026-11-09, one session, built as "Numbers" in
[`delimited.md`](delimited.md#numbers) says: **1.0** is tag `v1.0.0`'s `rust/` over HyperCast 1.0.0,
**now** this release over HyperCast 1.1 (simdutf8 in its text door), and the third build, 1.0's
`rust/` over HyperCast 1.1, splits each change into **HyperCast** (that build against 1.0) and
**HyperTabular** (now against that build). The same benchmark file on all three, five runs of each
interleaved with the order rotated, the median of each build's Criterion medians; load average 1.5
to 1.9. Every file reads to the same checksum on all three.

| File | 1.0 | Now | Change | HyperCast | HyperTabular |
| --- | ---: | ---: | ---: | ---: | ---: |
| `text_heavy` (read only) | 48.5 ms | 50.5 ms | +4.1% | -1.2% | +5.4% |
| `excel-win-300k.xlsx`, open | 13.6 ms | 13.6 ms | +0.1% | +0.4% | -0.3% |
| `excel-win-300k.xlsx`, read | 252.3 ms | 257.8 ms | +2.2% | -0.5% | +2.7% |
| `libreoffice-300k.ods`, open | 184.3 ms | 185.5 ms | +0.7% | +0.4% | +0.3% |
| `libreoffice-300k.ods`, read | 577.6 ms | 585.9 ms | +1.4% | 0.0% | +1.4% |
| `numbers-300k.numbers`, open | 457.5 ms | 460.7 ms | +0.7% | +0.2% | +0.5% |
| `numbers-300k.numbers`, read | 667.9 ms | 670.5 ms | +0.4% | 0.0% | +0.3% |

The losses are HyperTabular's. `text_heavy`'s cells are shared strings of 19 to 36 bytes: its
2 000 strings are asked about once each when they are loaded, and its 800 000 cells are handed
out on that answer. Asking every cell, as this tree did until 2026-11-09, read it in 58.1 ms
(+17.8% against 1.0, an earlier session the same evening, where this tree read it in 50.1 ms).
The xlsx read's 2% is notes and the sheet options, with the check on its one text column now
made once a string; the ods read's 1.4% is the same work with every text cell still asked about.
The Numbers rows are inside the session's noise (their spread was 2.0 to 2.4%); nothing in an
open reaches a door.

## Parked

- **Formula-only rows: an explicit policy change to come.** A row whose only cells are formulas with
  no cached value is filtered by default, for now, and `skip_empty_rows=false` is the lossless route
  ("Cells"). Changing that default is a decision for a version of its own, printed as what it is, with
  no quiet rewrite of the corpus.
- **Native date and date-time inference.** A workbook's date column suggests text, because a date cell's
  text has no zone and no door takes it. A suggester that asks the typed cell has to do so without
  discarding times or inventing zones.
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
- **Numbers' open.** Most of a `.numbers` read (470 ms of the table's 683 ms). Where its
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
