# Corpus

Three things live here: the two conformance contracts every binding replays — one for
delimited text, one for workbooks — and the files real applications wrote.

## `delimited.json` — the contract for delimited text

Every binding reads each case and must produce exactly what it says: the header, every
row's cells through the case's plan, and the structural failure if there is one. The Rust
binding replays it in `rust/tests/corpus_delimited.rs`.

It is written by `cargo run --example corpus` (in `rust/`), from cases that state two
things by hand: the input, and the text that must reach each cell, tokens marked. What each plan
column makes of that text is not this layer's to decide — HyperCast is the judge — so a cell's
verdict is whatever HyperCast's own door says of the hand-written text, asked directly.

`delimited.json` is 123 cases. `Western grouping refuses an Indian group`, `no format is Western
grouping` and `Indian grouping reads an Indian group` read the same bytes under flags `95`, no format and
flags `607`, so a binding that loses a format's grouping fails one of them, and so does one that takes no
format for anything else. `an Indian column beside a Western one` is one read of two policies, each
refusing what the other reads, and `no format beside an Indian column` the same with one left undeclared.
`both groupings read these shapes`, `neither grouping reads these shapes`, `one policy reads each of these
shapes` and `space groups: neither reads a misplaced one` pin the rest, `1,2,345` among them: no policy
reads a shape into a wrong value. Separator detection isn't a policy: a format that declares it infers
`,` and `.` from each cell and reads `12,34` as 12.34, which no vector here declares. A plan a binding refuses isn't a case: a format holding the retired bit
`256` (flags `351`, `863`) is refused by each binding's own tests, when it's bound.

A plan entry with no `format` means flags `95`: `.` decimal, `,` groups, every lenience, Western grouping.
That's `NumFormat::INVARIANT` in Rust and every binding's own `invariant`, what the C ABI's all-zero format
reads, and what a column given no format reads in a binding that has one. Flags `607` are Indian, declared.
Grouping's shapes and flags are in HyperCast's
[docs/grouping.md](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md).

A case:

```json
{
  "name": "a cell that does not cast says where",
  "input": "a,b,c\n12x4,256,x\n",
  "dialect": { "separator": ",", "quoting": true, "skip_blank_lines": true, "has_header": true },
  "plan": [ { "door": "i32", "ordinal": 0 }, { "door": "u8", "ordinal": 1 } ],
  "header": ["a", "b", "c"],
  "rows": [ [ { "expect": "malformed", "fault": [2, 1], "raw": "12x4" },
              { "expect": "out_of_range", "fault": [0, 3], "raw": "256" } ] ],
  "failure": { "kind": "column_count", "record": 2, "line": 3, "byte": 8, "expected": 2, "found": 1 }
}
```

- `input` is UTF-8 text; a byte-order mark is written `\uFEFF`. An input that isn't UTF-8
  has `input_hex` in its place, two lowercase hex digits a byte.
- A plan entry names its door as HyperCast's corpus names its types, with what the door
  declares beside it (`precision` for `unix`, `order` for `date_ordered` and `datetime`,
  `epoch` for `excel_serial`, numbered as HyperCast numbers them) and a `format` in
  HyperCast's shape where the numeric notation is not the invariant one.
- A dialect may add what a read passes over and how it takes a record of the wrong width, each
  absent when it is the default: `skip`, physical lines after a byte-order mark; `header_row`,
  records discarded after those; `short_rows`, `reject` or `pad`; `long_rows`, `reject`, `report`
  or `truncate`; `tokens`, a list of strings that stand for no value; and `encoding`,
  `windows-1252`, `latin-1` or `utf-16le`, whose input is always `input_hex`, the original bytes,
  and whose header, cells and spans are all the decoded text.
- A plan entry may add `tokens`, which replace the dialect's (an empty list means none), and
  `options` for the date doors, in HyperCast's corpus shape: `flags` by name (`compact_date`,
  `space_separator`, `hour_offset`), `pivot`, and `months`, a list of `[month, name]`.
- `header` is `null` when the dialect declares none, and `[]` for an input with no record.
- A cell is a verdict in HyperCast's corpus shape — `{"expect": "ok", …the value…}`, with
  `text` for the text door — or `{"expect": "empty"}`, or a fault with its span and `raw`,
  the cell's own text, which a binding has to be able to give back. A cell whose bytes
  aren't UTF-8 has `raw_hex` in place of `raw`, in `input_hex`'s digits. A cell the door refused
  adds `detail`, HyperCast's name for what failed. A cell no door was asked about adds `origin`,
  and has no `detail`: `token` for a declared token, `padded` for a cell its row doesn't have
  (and, in `workbook.json`, `uncached` for a formula with no result); absent, the verdict is the
  door's.
- `problems` is what the rows' faults list says, by row and then by column, and `[]` when it says
  nothing: a cell a door refused or a formula with no cached result as
  `{"row", "line", "plan_column", "source_column", "reason"}`, with the cell's own `fault`, `detail`
  and `origin` where it has them; and a long row under `report` as
  `{"row", "line", "expected", "found"}`, ahead of the row's cells. `row` indexes `rows`, so it counts
  across the read, whatever the batch size. It follows from `rows` and from the lines and widths the
  writer has by hand, not from any reader's list.
- `suggest`, on the cases it's written for, is what `hypertabular_suggest` says of the input, worked
  out by hand from the rule in `docs/design.md`: `columns`, each `{"door", "param", "status",
  "matched", "sampled"}` in plan order; `truncated`, whether rows lie past the sample; and, when the
  case asks for them, `sample_rows` (otherwise 1,000) and `format`, declared for every column. `door`
  names a door as a plan entry does, `param` is the number `RawSuggestion` holds (on a resolved numeric
  column, `2` where a `,` proved that notation and `4` where only the comma decimal reads it), and `status` is
  `resolved`, `ambiguous` or `unresolved`. A case's plan is every cell's text, which is what a sample
  is. The `sample_rows` rows of `rows`, each cell's `text` or `raw_hex` or else a blank, are the sample
  to pass the export, and a case that passes them through a binding's own suggestion has to land on
  the same answer. On a workbook case the sample is the sheet's, so its suggestion is asked through a
  binding's own sheet, and the plan it gives has to read back refusing nothing.
- `failure`, when present, ends the input after the rows listed; its positions count the
  header, skipped blank lines and a byte-order mark. Under a declared `encoding`, `record` and
  `line` read the same, an `encoding` failure's `byte` is its unit's offset in the original input,
  and any other failure's `byte` counts the decoded text.

How the input is cut into chunks is the binding's business and must not change the answer;
a binding should replay each case more than one way.

A replay fails on a key it doesn't know, at any level, so that a key added here can't be passed
over: every binding's replay does, Rust's in `every_corpus_key_is_read`, so a new key is a change to
each of them. Both files hold to this, and the one key in either that no replay reads is a workbook
case's `note`, a sentence for whoever reads the file.

## `workbook.json` and `workbook/` — the contract for workbooks

`workbook/` holds twenty-six small packages, and beside them the fifteen files real
applications wrote (the table at the bottom lists them). The twenty-six: five fixtures
(`basic.xlsx`, `basic-1904.xlsx`, `multisheet.xlsx`, `rich.xlsx` from openpyxl and
`basic.ods` from the ODF schema by hand — `make_fixtures.py` writes them) and fifteen
generated ones — two XLSX, two ODS and two Numbers, one of each deflated and one stored,
the XLSX pair one in each date system; `broken.xlsx`, whose sheet names a shared
string that is not there; and `ranges.xlsx`, `ranges.ods` and `ranges.numbers`, one table in each
format, with a title above it, stray cells beside it, a footer below it and every shape a range can
cut; and `uncached.xlsx` and `uncached.ods`, every spelling of a formula with no result beside what
only looks like one. Those two are assembled by hand, in `tests/support/packages.rs`, not written by
an application: openpyxl spells two of the XLSX forms and Numbers' export a third, as the comments
there say, and LibreOffice 25.2, handed the ODS forms, wrote a result for every one, so of them its
writer spells only `<text:p/>`, its `=""`. Like the other generated packages they are minimal, so no
application opens them as they stand. No Numbers package has these forms, since none has been found
in a file Numbers wrote: `docs/workbook.md`, "Cells", says what was checked. And `dates.xlsx`,
`dates.ods` and `dates.numbers`, one table in each format, of text cells only a declared field order
can read (`01/02/2026`, `13/02/2026`, `02/13/2026`, each also with a time) or declared month names
(`7-Okt-2026`), above a stored date, which has no order to declare. And `suggest.xlsx`, `suggest.ods`
and `suggest.numbers`, one table in each format, in which number cells and text cells holding the same
digits sit in columns of their own and side by side, for what a suggester has to tell apart. And
`grouping.xlsx`, `grouping.ods` and `grouping.numbers`, one table in each format, of text cells in Indian and
Western grouping above a typed number, for a column's policy to reach a text cell and leave a typed one alone.
Between them the
generated packages hold every kind of cell each format has: shared, inline and rich strings, numbers under date, time and elapsed
formats, booleans, errors, ISO dates, CDATA, prefixed elements, sparse rows, empty rows in
both spellings, gaps in the row numbers, ODS's repeated rows and columns, nested tables,
annotations and paragraphs, and Numbers' decimals, currencies, rich text, dates and times
of day, durations, error and merged cells, in layouts Numbers itself never writes small:
chunks of a few bytes, tiles of seven rows, wide cell offsets, a segmented string list,
rows left out of their tile.

`workbook.json` is 398 cases over those 41 packages — every sheet of every package, read
several ways, the two encrypted files refused, and `ranges.*`, `uncached.*`, `dates.*`, `suggest.*` and
`grouping.*` read by hand-listed cases besides — 2,010 rows and 66,351 cells. What the `dates.*`, `suggest.*`
and `grouping.*` cases read is held to tables worked out by hand in `rust/tests/corpus_workbook.rs`. A case:

```json
{
  "name": "generated-a.xlsx, sheet 0 (\"Data\"), a header, empty rows skipped",
  "file": "workbook/generated-a.xlsx",
  "format": "xlsx",
  "epoch": 1,
  "sheets": [{"hidden": false, "name": "Data"}, {"hidden": false, "name": "More & more"}, {"hidden": true, "name": "Hidden"}],
  "sheet": 0,
  "options": {"has_header": true, "skip_empty_rows": true},
  "plan": [{"door": "bool", "ordinal": 0}, {"door": "i8", "ordinal": 0}],
  "header": ["", "7000000000000", "#NUM!", "#WAT"],
  "numbers": [4, 5, 6],
  "rows": [
    [{"expect": "ok", "value": true}, {"expect": "ok", "value": 1}]
  ],
  "failure": {"kind": "shared_string", "record": 6, "line": 3, "byte": 322, "expected": 7, "found": 99}
}
```

- `file` is relative to this directory. `format` is `xlsx`, `ods` or `numbers`; `epoch` is the
  workbook's date system as HyperCast numbers `ExcelEpoch` (`1` 1900, `2` 1904).
- `sheets` is the workbook's whole listing; `sheet` is the index of the one this case
  reads. A binding should open it by index and, where the name is unique, by name.
- `plan`, `header`, `rows` and a cell are as in `delimited.json`. With the default
  options the plan is every door over every column (ten columns for the synthetic
  fixtures, eighteen for the real writers' files, four for the generated packages, with
  what a door declares varied by column); with the other three the options are the point,
  and the plan is the text door over every column.
- `options` may add what `delimited.json`'s dialect has, each absent when it's the default: `skip`,
  `header_row`, `tokens`, and `range`, written like `B4:E12` for a binding to parse through
  `hypertabular_range_parse`. A plan entry's `ordinal` then counts from the range's first column,
  and `numbers` stay the sheet's own. The `ranges.*` cases that set any are the place to find them.
- A cell's `detail` and `origin` are as in `delimited.json`, the typed twins' details included.
  Tokens match text cells alone, `padded` means an ordinal past the last cell the row holds, and
  `uncached` a formula with no cached result, which has no `detail` either.
- `numbers` is each row's number in the sheet, parallel to `rows`.
- `problems` is as in `delimited.json`, without long rows, which a sheet has no policy for. Its
  `line` is the row's number, and its `source_column` is the sheet's own, `A` being `0`, wherever a
  range starts. The writer works it out from `rows` and `numbers`, so the core's list has to agree
  with the core's cells. A row the empty-row filter leaves out isn't in `rows`, so isn't in here.
- A typed cell that fails its door has the cell said as text for its `raw` and a fault
  that spans all of it; a typed cell through the text door is that same text.
- `failure`, when present (the one shown is `broken.xlsx`'s; the only others are the two
  encrypted files', cases of nothing but `name`, `file` and the failure), ends the sheet
  after the rows listed: `kind` is one of
  `not_a_zip`, `container`, `encrypted`, `method`, `missing_part`, `xml`, `deflate`,
  `not_a_workbook`, `shared_string`, `too_large`, `iwa`; `record` is the part of the
  package being read (`6` a worksheet, `10` a Numbers part), `line` the sheet row, `byte`
  the offset in the part's inflated bytes (for a Numbers part, in its decoded stream).
- `basic.ods` is not read with empty rows delivered: a real ODS sheet ends in a million
  empty rows, and they would be most of the file.

**Where the file's authority comes from.** It was written by the std workbook reader this
crate had before its core could read a workbook — a second implementation, which owed the
core nothing — and the core was required to agree with it, cell for cell, before the file
was written. That reader has been deleted. Nothing independent of the core reads a
workbook in this repository, so the file is that reader's frozen word:
`cargo run --example corpus` writes it from the core, through the Rust binding, and
`rust/tests/corpus_workbook.rs` fails if what it writes is not what is committed. A diff
in `workbook.json` is a change in what the core reads, and has to be explained, not
regenerated away. The one thing in it that was never the oracle's is a failure's fields,
which that reader reported only as a message; the case says so in its `note`.

`cargo run --example corpus -- --packages` rewrites the generated packages. Their
deflated bytes depend on the deflate the build links, which is why they are committed and
not rebuilt.

## The files real applications wrote

The packages above came from openpyxl, a hand-written ODF schema and a generator. This
directory is also for the files real writers produce — Excel, LibreOffice, Google Sheets, Apple
Numbers — because "the reader handles what the spec allows" and "the reader handles what
Excel actually writes" are different claims, and every one below is the first real proof of
a code path or a design claim. Once a file is here, it joins `workbook.json` the way the
fixtures did — with one difference that has to be said: its expected output will be
written by the core, with no second reader left to check it against, so each such file
is to be read against the application that wrote it, by eye, before its case is
committed.

## Ground rules

- Files land in `corpus/workbook/`, beside the packages already there, named
  `<writer>-<what>.<ext>` exactly as the table at the bottom lists them.
- Record the writer's version beside each file in that table (Excel: File → Account →
  About Excel; LibreOffice: Help → About). A fixture nobody can attribute is a fixture
  nobody can regenerate.
- Save to a local path, not a OneDrive/AutoSave folder, so the desktop application is the
  only writer of the bytes.
- The tree carries fixtures, not benchmark inputs: the three 300 000-row files (16.7 MB,
  13.3 MB and 29.6 MB) live under `corpus/generate/out/`, which is ignored, and the table
  records their sha256 instead.

## The dataset

One small sheet, reused by every writer so the outputs compare. Sheet name **`Data`**
(add a second sheet named **`Ünïcödé sheet`** with a single cell `1` in A1). Row 1 is the
header; rows 2–4 are values. Type values, don't paste from another spreadsheet.

`corpus/dataset.tsv` holds the same values, rows 1 to 10, to paste as text into A1 instead of
typing them: pasted text is read the way typing is. Three things a paste cannot do, to be
done before or after it:

- format column F as **Text** *before* pasting, or its three cells arrive as a number, a
  date and a boolean;
- retype E3 (`'123`) by hand — pasted, the apostrophe is a character in the cell, not the
  prefix — and bold the word in E2;
- apply every number format in the tables below, and the merge of B10:C10; the file
  carries values only.

| Col | Header | Row 2 | Row 3 | Row 4 | Notes |
| --- | --- | --- | --- | --- | --- |
| A | `id` | `1` | `2` | `3` | integers |
| B | `amount` | `2.5` | `-7` | `1E+15` | doubles; row 4 typed as `1000000000000000` |
| C | `name` | `alice` | `bob, jr` | *(empty)* | plain shared strings |
| D | `note` | `  padded  ` | `x <&> "y"` | `日本語 é 🙂` | leading/trailing spaces on row 2 (Excel keeps them) |
| E | `rich` | `plain **bold** tail` | `'123` | `=""` | row 2: select the word in the formula bar and bold it; row 3: leading apostrophe; row 4: formula returning empty text |
| F | `astext` | `123` | `2024-01-31` | `TRUE` | format the column as **Text** *before* typing, so all three are strings |
| G | `when` | `2024-01-31 10:30:00` | `1900-02-29` | `9999-12-31 23:59:59` | date-time format; row 3 is the phantom — Excel accepts it and stores serial 60 |
| H | `day` | `2023-03-15` | `1900-02-28` | `1900-03-01` | date format (serials 44,999 / 59 / 61) |
| I | `at` | `15:04:05` | `0:00:00` | `23:59:59.5` | time format `hh:mm:ss.0` |
| J | `bigtime` | `1.5` | `0.5` | `2.25` | time format `hh:mm` — row 2 and 4 are ≥ 1 so they are wall clocks, row 3 is a clock |
| K | `elapsed` | `1.5` | `0.001` | `100` | `[h]:mm:ss` on row 2, `[mm]:ss` on row 3, `[s]` on row 4 |
| L | `badserial` | `-1` | `60` | `2958466` | **date** format on all three: negative, phantom, past 9999-12-31 — must stay numbers |
| M | `flag` | `TRUE` | `FALSE` | `=TRUE()` | booleans, the last one a formula with a cached boolean |
| N | `pct` | `25%` | `0.1%` | `100%` | percentage format |
| O | `money` | `$-7.00` | `$1,234.56` | `€3` | currency formats |
| P | `err` | `=1/0` | `=NA()` | `=VALUE("x")` | cached errors `#DIV/0!`, `#N/A`, `#VALUE!` |
| Q | `calc` | `=A2*2` | `=C3&"!"` | `=DATE(2024,1,31)` | formulas with a cached number, a cached string, a cached date (date format on row 4) |

Then the custom number formats, each on its own cell in row 6, value first, format second
(Format Cells → Number → Custom → type the code exactly):

| Cell | Value | Format code | Must classify as |
| --- | --- | --- | --- |
| A6 | `5` | `#,##0 "h"` | number — the `h` is quoted |
| B6 | `5` | `h "hours"` | date/time — the `h` outside the quotes wins |
| C6 | `45000.5` | `[$-409]m/d/yy h:mm AM/PM;@` | date/time |
| D6 | `-3.5` | `[Red]0.00;[Blue]-0.00` | number |
| E6 | `-3.5` | `0.00_);\(0.00\)` | number |
| F6 | `45000` | `yyyy\-mm\-dd` | date |
| G6 | `45000` | `"Year "yyyy` | date |
| H6 | `45000` | `dddd, mmmm d, yyyy` | date |
| I6 | `0.5` | `mm:ss.0` | time |
| J6 | `1.5` | `[hh]:mm` | elapsed |
| K6 | `45000` | Format Cells → Date → **Locale: Japanese** → pick an era format (`平成…`) | date — this is the built-in id 27–36 range no synthetic fixture has |
| L6 | `45000` | Format Cells → Date → Locale: Chinese (Simplified) → pick one | date — ids 50–58 |

Leave row 5 empty (a gap), and put `Z8` in cell Z8 with A8 = `8` (a sparse row). Merge
B10:C10 with `merged` in it (merges are not consulted; the file should carry one anyway).

## Excel for Windows

**1. `excel-win-data.xlsx`** — the dataset above, File → Save As → *Excel Workbook
(\*.xlsx)*. Calculation must be automatic (Formulas → Calculation Options) so every
formula carries its cached value. Proves: `t="e"`, `t="str"`, `t="b"` with `<f>`, rich-text
runs, `quotePrefix` strings, `xml:space="preserve"`, the phantom and the rejected serials,
the elapsed and locale-specific built-in formats, and the custom-format classifier on real
`styles.xml`.

**2. `excel-win-strict.xlsx`** — the same workbook, File → Save As → file type *Strict Open
XML Spreadsheet (\*.xlsx)*. Excel writes ISO 8601 dates with `t="d"` and the
`purl.oclc.org` namespaces. Proves the `t="d"` path, and that the relationship matching, which
keys on a type's last segment, survives strict URIs.

**3. `excel-win-1904.xlsx`** — a **new blank** workbook, and *before typing anything*: File
→ Options → Advanced → *When calculating this workbook* → check **Use 1904 date system**
(turning it on after dates exist shifts them by 1,462 days). Then type, date format on all:

| Cell | Value | Serial in 1904 |
| --- | --- | --- |
| A1 | `1904-01-01` | 0 |
| A2 | `1904-02-29` | 59 — a real leap day, no phantom in this system |
| A3 | `1904-03-01` | 60 |
| A4 | `2024-01-31 10:30:00` | 43,860.4375 |
| A5 | `9999-12-31` | 2,957,003 |

Save as .xlsx. Proves `workbookPr/@date1904` from the writer that invented it.

**4. `excel-win-sheets.xlsm`** — a workbook with every sheet kind:
- a normal sheet `Visible` with `1` in A1;
- select A1:A3 holding `1 2 3`, Insert → a column chart, then Chart Design → Move Chart →
  *New sheet* → `Chart1` — a **chart sheet**;
- a sheet `Hidden`: right-click its tab → **Hide**;
- a sheet `Very`: press Alt+F11, select the sheet in the Project pane, press F4, set
  **Visible** to `2 - xlSheetVeryHidden`;
- right-click a tab → **Insert…** → *MS Excel 4.0 Macro* — a **macro sheet**;
- right-click a tab → **Insert…** → *MS Excel 5.0 Dialog* — a **dialog sheet**;
- File → Save As → *Excel Macro-Enabled Workbook (\*.xlsm)*.
Proves: `state="hidden"` / `"veryHidden"` from Excel, and that chart, dialog and macro
sheets are filtered out of the listing on their real relationship types.

**5. `excel-win-encrypted.xlsx`** — the dataset workbook, File → Info → Protect Workbook →
**Encrypt with Password** → `hypercast`, save. Excel writes an OLE container, not a zip,
with the package in a stream named `EncryptedPackage`. Proves the refusal as `encrypted`
on the real thing; an OLE container without that stream (a legacy `.xls`) is
`not_a_workbook`. Also **6. `excel-win-protected.xlsx`**: the same
menu → **Protect Workbook Structure** → `hypercast`. That one is still a zip with a
`<workbookProtection>` element and must read normally.

**7. `excel-win-300k.xlsx`** (stays out of the tree, sha256 in the table) — the
real-file benchmark row. New workbook; click the Name Box, type `A2:H300000`, Enter; type
`=ROW()` and press **Ctrl+Enter** to fill the whole selection. Then per column, select the
column body and Ctrl+Enter a different formula:

| Col | Formula | Then format as |
| --- | --- | --- |
| A | `=ROW()` | General |
| B | `=ROW()/7` | `0.000000` |
| C | `="name"&ROW()` | General |
| D | `=DATE(2020,1,1)+ROW()` | `yyyy-mm-dd` |
| E | `=MOD(ROW(),1440)/1440` | `hh:mm` |
| F | `=ISEVEN(ROW())` | General |
| G | `=ROW()/24` | `[h]:mm:ss` |
| H | `=IF(MOD(ROW(),100)=0,NA(),ROW())` | General |

Select all, Copy, Paste Special → **Values** (so the file holds typed cells, including
3,000 real `#N/A` error cells, not formulas), add a header row, save. Excel takes a
minute; the file is 16.7 MB, 111 MB of sheet XML inside.

**Not from Excel: zip64.** Excel only writes zip64 structures past 4 GiB or 65,535
entries. `excel-win-zip64.xlsx` is `excel-win-data.xlsx` re-wrapped with Python's `zipfile`
forcing zip64 — a container test, and the parts inside are Excel's own.

## LibreOffice Calc (Windows build is fine)

**8. `libreoffice-data.ods`** — the dataset, with these Calc specifics:
- a tab inside a cell: type `a<Tab>b` in Notepad, copy, paste into D5 (Calc keeps the
  `text:tab`); a line break: Ctrl+Enter inside the cell (`text:line-break`); three spaces
  between words (`text:s text:c="3"`);
- a comment on A2: Insert → Comment (`office:annotation`, which must be skipped);
- errors `=1/0` and `=NA()` (LibreOffice writes `office:value-type="string"` *and*
  `calcext:value-type="error"`; the design says the second wins);
- a duration: type `25:30:00`, format `[HH]:MM:SS` (`office:time-value="PT25H30M00S"`);
- hide a sheet: right-click tab → Hide Sheet;
- File → Save As → *ODF Spreadsheet (.ods)*, and choose **Keep current format** if asked.
It holds every ODS code path to a file LibreOffice wrote; the fixtures' ODS is
hand-written XML. Its million-row padding rows come free.

**9. `libreoffice-data.xlsx`** — the same workbook, File → Save As → *Excel 2007–365
(.xlsx)*. A second real `styles.xml` and a second set of relationship targets.

**10. `libreoffice-encrypted.ods`** — File → Save As, tick **Save with password**, password
`hypercast`. The manifest gains `encryption-data`; the reader must refuse it by name.

**11. `libreoffice-300k.ods`** (out of tree, sha256) — optional, for the ODS benchmark
row. Name Box `A2:H300000`, formula, **Alt+Enter** fills the selection in Calc; then
Data → Calculate → *Formula to Value*; save as .ods.

## Google Sheets

**12. `google-data.xlsx` and `google-data.ods`** — File → Import → upload
`excel-win-data.xlsx` (Replace spreadsheet), then File → Download → *Microsoft Excel
(.xlsx)* and again *OpenDocument (.ods)*. Google's writer roots parts and writes
relationship targets its own way; these two files are what the design's "third-party
writers root parts inconsistently" line is checked against.

## Apple Numbers

**13. `numbers-data.numbers`** — the dataset, typed into Numbers on a Mac and saved in
its own format (every other reader of `.numbers` is a reverse engineering of this file
format; this is the file the core's IWA reader is held to). Numbers specifics:

- A new blank spreadsheet with one table per sheet, each table's name hidden (Format →
  Table → Table Name): the core lists a sheet by its name, and the export writes a shown
  table name as a merged title row above the data. Turn off Auto-Correction (Settings →
  Auto-Correction: spelling, capitalisation, smart quotes and dashes, link detection)
  before typing, or `x <&> "y"` and `bob, jr` arrive changed.
- Numbers has no 1900 date system and refuses the phantom `1900-02-29` (G3): it keeps
  reverting the cell to *Automatic* and stores the text, which is the right answer — the
  core reads that cell as text, as it does F3.
- Format column F as Text before typing, as for every other writer. B4 typed as
  `1000000000000000` is stored as text by Numbers unless typed `1E+15`.
- Numbers has no serial behind a date, so J, K and L hold what the serials mean: J
  date-times, K durations (a duration is a cell type; `36:00` typed is minutes and
  seconds), L plain numbers, since Numbers refuses the dates that `-1`, `60` and
  `2958466` would be. A time has no fractional second: `23:59:59.5` typed becomes a
  duration, and I4 is `=TIME(23,59,59.5)`, which Numbers evaluates to `23:59:59`. A time
  typed alone is stored on the day it was typed, under a format that shows no date, and
  the core reads it as a time of day — as it reads J3 and B6, whose custom format
  `h' hours'` hides a date too.
- Row 6 is typed as what each serial means (a number, dates, a time, a duration) under the
  formats the table above asks for, built with Format → Cell → Data Format → Create Custom
  Format; Numbers has no format codes. A custom format lives in the document's format list,
  and that pattern is what tells the core a date (B6) from a time (I6). E6's parentheses
  are the negative-number style, the unlabeled pop-up beside Decimals in the Number format,
  not Currency. Numbers has no `NA()`; P3 is `=MATCH("zz",C2:C4,0)`. Every Numbers error
  reads `#VALUE!`: the file records which formula failed, not which error it made.
- The second sheet `Ünïcödé sheet` with `1` in A1; the third, `Hidden`, hidden from the
  sheet tab's menu (Numbers 26 can hide a sheet).

**14. `numbers-data.xlsx`** — the same document, File → Export To → Excel, one worksheet per
table, *Include a summary worksheet* unchecked. Numbers is the writer most often cited for
oddly rooted parts. Its export says something else than the `.numbers` file in twelve cells
of the `Data` sheet, every one of them the writer's doing:

- Serials are counted from 1899-12-30, so a date before 1900-03-01 lands a day late: J2, J4
  and B6 are one day after their `.numbers` values, and H3, `1900-02-28`, is written as
  serial 60 — Excel's phantom day, which the core refuses as a date and reads as the
  number `60`.
- A time of day is written with the day it was typed on, under `h:mm:ss`, so the core reads
  I2, I3, J3 and I6 as wall clocks on 2026-10-24, and I4 on 1904-01-01, where Numbers'
  `TIME()` roots its result.
- An error cell is written as its formula with no cached value, so P2 to P4 read as empty.
  Excel recalculates them on open; the core never evaluates.
- The export writes no application properties: `docProps/app.xml` is an empty element.

**15. `numbers-300k.numbers`** (out of tree, sha256 in the table) — the Numbers columns of
the benchmark table. File → Open on `excel-win-300k.xlsx` (Numbers imports the formulas
with their values; the sheet keeps its name, `Sheet1`), then File → Save, named
`numbers-300k`, beside it. Numbers has no `NA()`, so the 3 000 `#N/A` cells of column H
come out empty: they fault under the plan's `i64` door as the errors do, and every harness
prints the same checksum over this file as over the other two. Numbers keeps fifteen
significant digits of a double, so column B's values are not Excel's bit for bit. The
package is 29.6 MB, its parts stored, not deflated; 8.8 MB of it is the calculation
engine's formulas and 2.1 MB the string list.

## Optional: Excel for the web

**16. `excel-web-data.xlsx`** — upload `excel-win-data.xlsx` to OneDrive, open in the
browser, change one cell and change it back (forces a rewrite), download. A different save
path from the desktop writer, cheap to collect.

## The files and their writers

| File | Writer and version | OS | sha256 (out-of-tree files only) |
| --- | --- | --- | --- |
| `excel-win-data.xlsx` | Microsoft Excel 16.0 (AppVersion 16.0300) | Windows | |
| `excel-win-strict.xlsx` | Microsoft Excel 16.0 (AppVersion 16.0300) | Windows | |
| `excel-win-1904.xlsx` | Microsoft Excel 16.0 (AppVersion 16.0300) | Windows | |
| `excel-win-sheets.xlsm` | Microsoft Excel 16.0 (AppVersion 16.0300) | Windows | |
| `excel-win-encrypted.xlsx` | Microsoft Excel 16.0 (AppVersion 16.0300) | Windows | |
| `excel-win-protected.xlsx` | Microsoft Excel 16.0 (AppVersion 16.0300) | Windows | |
| `excel-win-zip64.xlsx` | `excel-win-data.xlsx` re-wrapped by Python `zipfile` with its zip64 limits lowered; the parts are Excel's | — | |
| `excel-win-300k.xlsx` (out of tree) | Microsoft Excel 16.0 (AppVersion 16.0300) | Windows | `f11b3f99944f72c1251ea703358c33d601e8b0c3e34bb2be90cf2e077367053b` |
| `libreoffice-data.ods` | LibreOffice 26.2.6.3 | Linux x86-64 | |
| `libreoffice-data.xlsx` | LibreOffice 26.2.6.3 | Linux x86-64 | |
| `libreoffice-encrypted.ods` | LibreOffice 26.2.6.3 | Linux x86-64 | |
| `libreoffice-300k.ods` (out of tree) | LibreOffice 26.2.6.3 | Linux x86-64 | `74db9cf843bac6961c5962fe0a435313ea3c3c1c1bc1172013ec6929bd75df0c` |
| `google-data.xlsx` | Google Sheets, downloaded 2026-10-04 | web | |
| `google-data.ods` | Google Sheets, downloaded 2026-10-04 (generator: LibreOfficeDev 6.0.5.2) | web | |
| `numbers-data.numbers` | Numbers 26.3.1 | macOS | |
| `numbers-data.xlsx` | Numbers 26.3.1 (its export writes no application properties) | macOS | |
| `numbers-300k.numbers` (out of tree) | Numbers 26.3.1 | macOS | `857f29ca03521ee6772c102aa48490455824af31fe66d857d35ea087488e25bd` |
| `excel-web-data.xlsx` | Microsoft Excel Online (AppVersion 16.0300) | web | |
