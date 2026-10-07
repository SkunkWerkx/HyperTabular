# Corpus

Three things live here: the two conformance contracts every binding replays — one for
delimited text, one for workbooks — and the files real applications wrote.

## `delimited.json` — the contract for delimited text

Every binding reads each case and must produce exactly what it says: the header, every
row's cells through the case's plan, and the structural failure if there is one. The Rust
binding replays it in `rust/tests/corpus_delimited.rs`.

It is written by `cargo run --example corpus` (in `rust/`), from cases that state two
things by hand: the input, and the text that must reach each cell. What each plan column
makes of that text is not this layer's to decide — HyperCast is the judge — so a cell's
verdict is whatever HyperCast's own door says of the hand-written text, asked directly.

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

- `input` is UTF-8 text; a byte-order mark is written `\uFEFF`.
- A plan entry names its door as HyperCast's corpus names its types, with what the door
  declares beside it (`precision` for `unix`, `order` for `date_ordered` and `datetime`,
  `epoch` for `excel_serial`, numbered as HyperCast numbers them) and a `format` in
  HyperCast's shape where the numeric notation is not the invariant one.
- `header` is `null` when the dialect declares none, and `[]` for an input with no record.
- A cell is a verdict in HyperCast's corpus shape — `{"expect": "ok", …the value…}`, with
  `text` for the text door — or `{"expect": "empty"}`, or a fault with its span and `raw`,
  the cell's own text, which a binding has to be able to give back.
- `failure`, when present, ends the input after the rows listed; its positions count the
  header, skipped blank lines and a byte-order mark.

How the input is cut into chunks is the binding's business and must not change the answer;
a binding should replay each case more than one way.

## `workbook.json` and `workbook/` — the contract for workbooks

`workbook/` holds ten small packages, and beside them the thirteen files real applications
wrote (the table at the bottom lists them). The ten: five fixtures (`basic.xlsx`,
`basic-1904.xlsx`, `multisheet.xlsx`, `rich.xlsx` from openpyxl and `basic.ods` from the ODF schema by hand —
`make_fixtures.py` writes them) and five generated ones — two XLSX and two ODS, one of
each deflated and one stored, the XLSX pair one in each date system, and `broken.xlsx`,
whose sheet names a shared string that is not there. Between them the generated packages
hold every kind of cell either format has: shared, inline and rich strings, numbers under
date, time and elapsed formats, booleans, errors, ISO dates, CDATA, prefixed elements,
sparse rows, empty rows in both spellings, gaps in the row numbers, and ODS's repeated
rows and columns, nested tables, annotations and paragraphs.

`workbook.json` is 198 cases over those 23 packages — every sheet of every package, read
several ways, and the two encrypted files refused — 765 rows and 39,548 cells. A case:

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

- `file` is relative to this directory. `format` is `xlsx` or `ods`; `epoch` is the
  workbook's date system as HyperCast numbers `ExcelEpoch` (`1` 1900, `2` 1904).
- `sheets` is the workbook's whole listing; `sheet` is the index of the one this case
  reads. A binding should open it by index and, where the name is unique, by name.
- `plan`, `header`, `rows` and a cell are as in `delimited.json`. With the default
  options the plan is every door over every column (ten columns for the fixtures, four
  for the generated packages, with what a door declares varied by column); with the other
  three the options are the point, and the plan is the text door over every column.
- `numbers` is each row's number in the sheet, parallel to `rows`.
- A typed cell that fails its door has the cell said as text for its `raw` and a fault
  that spans all of it; a typed cell through the text door is that same text.
- `failure`, when present (the one shown is `broken.xlsx`'s; no other case has one),
  ends the sheet after the rows listed: `kind` is one of
  `not_a_zip`, `container`, `encrypted`, `method`, `missing_part`, `xml`, `deflate`,
  `not_a_workbook`, `shared_string`, `too_large`; `record` is the part of the package
  being read (`6` a worksheet), `line` the sheet row, `byte` the offset in the part's
  inflated bytes.
- `basic.ods` is not read with empty rows delivered: a real ODS sheet ends in a million
  empty rows, and they would be most of the file.

**Where the file's authority comes from.** It was written by the std workbook reader this
crate had before its core could read a workbook — a second implementation, which owed the
core nothing — and the core was required to agree with it, cell for cell, before the file
was written. That reader has been deleted. Nothing independent of the core reads a
workbook in this repository any more, so the file is that reader's frozen word:
`cargo run --example corpus` now writes it from the core, through the Rust binding, and
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
- Anything over ~50 MB stays out of the tree (GitHub's limit is 100 MB): keep it under
  `corpus/generate/out/`, which is ignored, and record its sha256 in the table instead.

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
`purl.oclc.org` namespaces. Proves the `t="d"` path, and will tell whether the relationship
matching survives strict URIs — nothing checks that today.

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
minute; the file is around 30–40 MB.

**Not on your list: zip64.** Excel only writes zip64 structures past 4 GiB or 65,535
entries. I will produce that fixture by re-wrapping `excel-win-data.xlsx` with Python's
`zipfile` forcing zip64 — it is a container test, and the parts inside can be Excel's own.

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
Every ODS code path today runs on hand-written XML; this is the first file LibreOffice
wrote. Its million-row padding rows come free.

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
relationship targets its own way; the design's "third-party writers root parts
inconsistently" line has never been checked against it.

## Apple Numbers, without a Mac

**13. `numbers-data.xlsx`** — if you have an Apple ID: iCloud.com → Numbers → upload
`excel-win-data.xlsx`, open it, then the *…* / tools menu → **Download a Copy** →
*Excel*. Numbers is the writer most often cited for oddly rooted parts. Skip if no Apple
ID; nothing else depends on it.

## Optional: Excel for the web

**14. `excel-web-data.xlsx`** — upload `excel-win-data.xlsx` to OneDrive, open in the
browser, change one cell and change it back (forces a rewrite), download. A different save
path from the desktop writer, cheap to collect.

## The table to fill in

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
| `numbers-data.xlsx` | | | |
| `excel-web-data.xlsx` | Microsoft Excel Online (AppVersion 16.0300) | web | |
