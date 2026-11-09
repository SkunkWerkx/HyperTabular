# hypertabular

[![CI](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml/badge.svg)](https://github.com/SkunkWerkx/HyperTabular/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@skunkwerkx/hypertabular.svg)](https://www.npmjs.com/package/@skunkwerkx/hypertabular)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://github.com/SkunkWerkx/HyperTabular/blob/master/LICENSE)

Delimited text — CSV, TSV, any single-byte ASCII separator — and workbooks — XLSX, ODS and
Numbers — read a batch at a time into typed columns, with a
[HyperCast](https://github.com/SkunkWerkx/HyperCast) verdict for every cell: the
`{ ok: true, value } | { ok: false, fault }` union `@skunkwerkx/hypercast` types, narrowed on
`ok`. A Node-API addon built with [napi-rs](https://napi.rs): the Rust core every other
HyperTabular binding calls, linked straight into the `.node` file, with types for TypeScript
and the same API under Node and Bun, and a WebAssembly fallback where no addon fits.

## Install

```sh
npm install @skunkwerkx/hypertabular@<version>
```

`bun add @skunkwerkx/hypertabular@<version>` installs the same packages, and Node runs the result.
`@skunkwerkx/hypercast` is a peer dependency (`^1.1.0`), which npm and Bun install beside it: a cell's
verdict and its values are HyperCast's own types, and a numeric column's `NumFormat` is
HyperCast's own class.

The package itself is JavaScript and types; the core comes from one of nine companion
packages. The native addon is a platform package that npm picks for you through
`optionalDependencies`, so nothing compiles at install, one for each of eight platforms, as
`@skunkwerkx/hypertabular-<platform>`. The ninth, `@skunkwerkx/hypertabular-wasm32-wasip1`, is
the WebAssembly fallback: a plain dependency of the root, so it is always installed, and loaded
only when no native addon fits.

| | x64 | arm64 |
| --- | :---: | :---: |
| Linux, glibc (`linux-*-gnu`) | ✓ | ✓ |
| Linux, musl (`linux-*-musl`, Alpine) | ✓ | ✓ |
| macOS (`darwin-*`) | ✓ | ✓ |
| Windows (`win32-*-msvc`) | ✓ | ✓ |
| Everything else: the WebAssembly fallback (`wasm32-wasip1`) | ✓ | ✓ |

It runs on Node 22.14 or newer (`^22.14.0 || >=23.6.0`, the first releases with Node-API
10), or Bun. An install run with `--omit=optional`, or on a platform outside the eight,
gets the fallback instead of the addon ([below](#when-no-native-addon-fits)).

## Usage

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

```js
const { Column, DelimitedReader, Dialect, Workbook } = require('@skunkwerkx/hypertabular')

// Opening, binding and reading answer an outcome; this script stops on the first error.
function must(outcome) {
  if (!outcome.ok) throw new Error(`${outcome.error.kind}: ${outcome.error.message}`)
  return outcome.value
}

// The header first; then a plan that reads the columns by name, wherever the file put them.
const reader = must(DelimitedReader.open('people.csv', Dialect.CSV))
const header = reader.header
console.log('header:', header.names) // header: [ 'id', 'name', 'score' ]
must(
  reader.bind([
    Column.i32(must(header.require('id'))), // a 'noColumn' error naming it if it is missing
    Column.text(must(header.require('name'))),
    Column.f64(must(header.require('score'))),
  ]),
)
for (const read of reader) {
  const batch = must(read) // a broken file stops here, after every intact row before it
  console.log('faults:', batch.faultCount(2)) // faults: 1
  for (let row = 0; row < batch.rows; row++) {
    const name = batch.get(1, row)
    const score = batch.get(2, row)
    if (score.ok) {
      console.log(`${name.value}: ${score.value}`) // alice: 2.5
    } else {
      const { reason, offset, length } = score.fault
      const raw = batch.raw(2, row) // the cell's bytes; the span indexes them
      const text = new TextDecoder().decode(raw.subarray(offset, offset + length))
      console.log(`line ${batch.line(row)}: ${reason} at "${text}"`) // line 3: malformed at "o"
    }
  }
}
reader.close()

// A workbook reads into the same batch, its sheet header first or with the plan up front.
const book = must(Workbook.open('people.xlsx'))
console.log('sheets:', book.sheets) // sheets: [ { name: 'People', hidden: false } ]
const sheet = must(book.sheet('People', null, [Column.text(1), Column.decimal(2)]))
for (const read of sheet.rows()) {
  const row = must(read)
  const score = row.get(1)
  console.log(`line ${row.line}: ${row.get(0).value} ${score.ok ? score.value : score.fault.reason}`)
}
sheet.close()
```

ESM works the same way, named imports included (`import { DelimitedReader } from
'@skunkwerkx/hypertabular'`). A plan known up front goes straight to the opener —
`DelimitedReader.open(path, dialect, plan)`, `DelimitedReader.fromBytes(bytes, dialect, plan)`,
`book.sheet(which, options, plan)` — which binds it before the header is read.

## Outcomes and verdicts

No call throws for data, nor for an argument of the wrong type. Two unions carry every answer:

```ts
type Verdict<T> = Success<T> | Failure // a cell: HyperCast's own, from @skunkwerkx/hypercast
type Outcome<T> = Success<T> | Broken  // opening, binding, reading
interface Success<T> { ok: true; value: T }
interface Failure { ok: false; fault: Fault }
interface Broken { ok: false; error: TabularError }
interface TabularError {
  kind: ErrorKind
  message: string
  failure?: StructuralFailure // for 'structure'
  column?: number // for 'plan'
  first?: number // for 'ambiguousColumn'
  second?: number
}
```

- **A bad value is a verdict.** A cell that does not cast is a `Failure` in its column, with
  HyperCast's reason (`'empty'`, `'malformed'`, `'outOfRange'`) and span, and the read goes
  on. `Verdict`, `Success`, `Failure`, `Fault` and `CastFailure` are re-exported from
  `@skunkwerkx/hypercast` unchanged, so a cell's verdict is assignable to HyperCast's
  `Verdict<T>` and back.
- **A broken file is an outcome's error.** A record of the wrong width, input that ends inside
  a quoted cell, a workbook whose container or parts cannot be read: `read()` answers
  `{ ok: false, error }` with `kind: 'structure'` and a `failure` saying what (`'columnCount'`,
  `'unclosedQuote'`, `'encoding'`, `'notAZip'`, `'encrypted'` and the rest of `FailureKind`) and where
  (`record`, `line`, `byte`, and `expected` and `found` for the kinds that say) — after every
  intact row before it has been delivered, and the same error again on every read after.
- **Every other error is an outcome's error too**, `kind` saying which: `'io'` (a file that
  cannot be opened or read), `'separator'` (a dialect the core cannot honour), `'plan'` (a
  column it cannot honour, `column` its place in the plan), `'noSheet'`, `'noColumn'`,
  `'ambiguousColumn'` (a name two header columns have, `first` and `second` their places),
  `'range'` (a sheet range that is not one), `'unbound'` (a read before a plan),
  `'alreadyBound'`, `'closed'` (a read or bind after `close()`), `'busy'` (a call back into a
  reader or sheet from inside one of its own calls, [below](#calling-back-in)), and
  `'argument'`: a size the core refuses, in the core's words (`batch_rows must be at least 1`,
  `buffer_bytes must be at least 1`, `batch_rows asks for … bytes, past the limit of …`), or a
  value of a type the declarations rule out — a number for the bytes, a plan that is not an
  array of `Column`s, a size that is not a whole number.
  The kinds are a closed string union, so a `switch` over `error.kind` is exhaustive.

```ts
import { NumFormat, NumStyles } from '@skunkwerkx/hypercast'
import {
  Column,
  DelimitedReader,
  Dialect,
  type Outcome,
  type TabularError,
  type Verdict,
} from '@skunkwerkx/hypertabular'

function explain(error: TabularError): string {
  switch (error.kind) {
    case 'structure':
      return `${error.failure?.kind} at line ${error.failure?.line}, byte ${error.failure?.byte}`
    case 'plan':
      return `plan column ${error.column}: ${error.message}`
    default:
      return error.message
  }
}

const euro = NumFormat.declare({ decimalSep: ',', groupSep: '.', flags: NumStyles.All })
if (!euro.ok) throw new Error(euro.message)
const semicolons = { ...Dialect.CSV, separator: ';' }
const text = 'item;price\npen;1.234,50\nink;n/a\nnib\n'
const opened = DelimitedReader.fromBytes(text, semicolons, [
  Column.text(0),
  Column.decimal(1, euro.value),
])
if (opened.ok) {
  for (const read of opened.value) {
    if (!read.ok) {
      console.log(explain(read.error)) // columnCount at line 4, byte 32
      break
    }
    const prices: Verdict<string>[] = read.value.column<string>(1)
    console.log(prices) // [ { ok: true, value: '1234.5' }, { ok: false, fault: { ... } } ]
  }
}
const refused: Outcome<DelimitedReader> = DelimitedReader.fromBytes(text, { separator: '"' })
if (!refused.ok) console.log(refused.error.kind) // separator
```

A column is never refused when it is made: `Column.i32(-1)` is a `Column` whose `valid` is
`false`, and the plan holding it is refused, with its place, when it is bound. A cell asked for
where the batch has none (`batch.get(9, 0)`) is HyperCast's contract violation, `malformed` at
offset 0, length 0.

The package throws two errors of its own. `ERR_HYPERTABULAR_UNAVAILABLE` comes from any opener
when no addon and no fallback loaded: an installation problem, not an input
([below](#when-no-native-addon-fits)), which `isAvailable()` says up front.
`ERR_HYPERTABULAR_BUSY` (`kind` `'busy'`) comes from a reader's or a sheet's getter read from
inside one of its own calls, where it has no outcome to answer in
([below](#calling-back-in)). `isHyperTabularError(error)` recognizes both. On the native addon every call also catches a Rust
panic and throws it as napi's `GenericFailure` rather than taking the process down; that is a
backstop, and on the fallback a panic aborts instead. An engine out of memory throws as it would
anywhere. `tests/never_throws.test.ts` calls every opener, method and factory with some forty
hostile values (symbols, `bigint`s, detached buffers, Proxies whose traps throw, the classes'
own prototypes) in every position.

## Spans

A fault's span is in bytes of the cell's raw text, which `raw(column, row)` hands back as a
`Uint8Array`, so `raw.subarray(offset, offset + length)` is the offending text, and every span
satisfies `offset + length <= raw.length`. `rawString(column, row)` is the same bytes decoded
for display; a span does not index it once the text leaves ASCII. A typed workbook cell that
cast has no text, so its `raw` is empty.

## Header first

A reader made without a plan — `DelimitedReader.open(path, dialect)`,
`DelimitedReader.fromBytes(bytes, dialect)`, `book.sheet(which, options)` — reads the header
and waits: `bind(plan)` declares the plan once, before the first read. A read before it is an
`'unbound'` error (and a later `bind` puts that right); a second `bind` is `'alreadyBound'`.
`isBound` says which it is, and a delimited reader's `columnCount` is the header's width, or
the first record's once it has been read. Once a delimited header has fixed that width, a plan
column at or past it could only ever be empty, so it is a `'plan'` error naming the column.

The header is a `Header`: `names` (the strings, in order; a header that is not valid in the
declared encoding is a `'structure'` error of kind `'encoding'`), `length`, `ordinal(name)` — the first column with exactly
that name, case and spaces included, or `null` — and `require(name)`, the one column with that
name as an outcome: `'noColumn'` naming it when there is none, `'ambiguousColumn'` with both
places when there are two, since a plan should then say which by position.

## Rows and batches

A batch is column-major, and owns what it shows: the reader's cells are copied out of it, so a
batch stays valid after the reader has moved on. It has `rows`, `columnCount`, `doors`,
`line(row)`, `get(column, row)`, `detail(column, row)`, `origin(column, row)`,
`raw(column, row)`, `rawString(column, row)`, `column(column)` (the whole column's verdicts),
`faultCount(column)`, `faults()` (worked out when it is asked for, so a read costs nothing for
it) and `row(row)`, and its rows when iterated. A `Row` reads it across, for code that builds an
object a row at a time, with `index`, `line`, `get(column)`, `detail(column)`, `origin(column)`,
`raw(column)` and `rawString(column)`; it is the batch and an index, nothing copied.

A reader or a sheet is iterable: `for (const read of reader)` (or `reader.batches()`) yields
each read's outcome, the error that stops it (if one does) once, as the last, and
`reader.rows()` yields every row the same way, batch after batch. `read()` is the same one
step at a time: the batch, `null` past the end, or the error.

`close()` releases a reader's or a sheet's buffers (and a reader's file) now, rather than when
it is collected, and answers `{ ok: true, value: null }` (closing twice is no error); a workbook's bytes go when the workbook and its sheets are collected. A sheet
read with a wide plan holds a cell table of several megabytes until then, so a loop opening
many is better off closing each. A `Batch` and a `Row` are plain JavaScript, and a batch's copy
an `ArrayBuffer` the engine owns, so what a loop drops is reclaimed by the engine's ordinary
collection, a synchronous loop's included. Three passes over a 91 MB file of 2M rows in one
synchronous loop, five runs each in Docker on an Apple M6, peaked at 125 to 126 MB on Node 26.13
read with `for (const read of reader)` or `while` over `read()`, 71 to 74 MB with `rows()` and 69
to 73 MB holding a `Row` (84 to 86, 66 to 69 and 66 to 71 MB on Node 22.14), and at 69 MB with
`batchRows: 1`, six million batches (62 MB on 22.14). The WebAssembly fallback differs
([below](#when-no-native-addon-fits)).

## A messier file

Everything about a file is still declared, on the `Dialect` and on the `SheetOptions`, whose
defaults read as before: lines to pass over, records to discard, what a record of the wrong
width is, the encoding, and what stands for no value.

```js
const { NumFormat, Grouping } = require('@skunkwerkx/hypercast')
const { Column, DelimitedReader, Range } = require('@skunkwerkx/hypertabular')

function must(outcome) {
  if (!outcome.ok) throw new Error(`${outcome.error.kind}: ${outcome.error.message}`)
  return outcome.value
}

// A title line, a short row, a long row, and "n/a" for nothing.
const text = 'Exported 2026-11-07\nid,score,note\n1,2.5,ok\n2,n/a\n3,x,ok,extra\n'
const dialect = { skip: 1, shortRows: 'pad', longRows: 'report', tokens: ['n/a'] }
const reader = must(DelimitedReader.fromBytes(text, dialect, [Column.i32(0), Column.f64(1)]))
for (const read of reader) {
  const batch = must(read)
  for (const row of batch) {
    const score = row.get(1)
    console.log(row.line, score.ok ? score.value : score.fault.reason, row.origin(1), row.detail(1))
  }
  // 3 2.5 cell null
  // 4 empty token null
  // 5 malformed cell unexpectedCharacter
  for (const problem of batch.faults()) {
    console.log(problem.line, problem.planColumn, problem.reason, problem.detail, problem.found)
  }
  // 5 null null null 4
  // 5 1 malformed unexpectedCharacter 0
}

// A plan to look at, from a sample. It reads nothing until it is handed to a read.
const found = must(DelimitedReader.suggestBytes('id,total\n1,"1,234.50"\n2,1.234\n'))
for (const column of found.report) {
  console.log(column.name, column.status, column.door, `${column.matched} of ${column.sampled}`)
}
// id resolved i32 2 of 2
// total resolved f64 2 of 2

// An Indian column beside a Western one.
const indian = must(NumFormat.invariant().withGrouping(Grouping.Indian))
const both = must(
  DelimitedReader.fromBytes('a,b\n"1,23,456","1,234,567"\n', null, [
    Column.i32(0, indian), // 1,23,456 is 123456, and 1,234,567 is malformed
    Column.i32(1), // 1,234,567 is 1234567, and 1,23,456 is malformed
  ]),
)
console.log(must(both.read()).row(0).get(0), must(both.read()))
// { ok: true, value: 123456 } null

// A sheet's range, by its text or its numbers.
const range = must(Range.parse('B2:D40'))
console.log(range.firstColumn, range.lastRow) // 2 40
```

A dialect's `skip` passes over lines before anything is read, and `headerRow` discards that
many records after them, ahead of the header. `shortRows: 'pad'` reads a short record's missing
cells as empty (origin `'padded'`), and `longRows` is `'reject'` (a structural `columnCount`),
`'truncate'`, or `'report'`, which keeps the row and lists it among the batch's faults with its
`expected` and `found` widths. `encoding` is `'utf8'`, `'windows1252'`, `'latin1'` or
`'utf16le'`, declared and never sniffed: text that is not valid in it is a structural failure of
kind `'encoding'` after every intact row before it, and its `byte` alone counts the file's own
bytes, a read's spans being the decoded text's. `tokens` are cells that mean no value, empty
with origin `'token'`; a quoted cell is never one. `Column.withTokens(tokens)` gives one column
its own list in place of the dialect's (`withTokens([])` says it has none), and
`Column.withDateOptions({ flags, pivot, months })` hands a date door HyperCast's
`DateOptions` and `MonthNames`; on a sheet both reach only a text cell. Every distinct list of
tokens, and every distinct list of month names, is kept for the life of the process, as the
Rust API keeps them: declare them, don't build a new list per file from what the files say.

A sheet's `SheetOptions` take `skip`, `headerRow` and `tokens` the same way, and `range`, a
`Range` or its text. A range is parsed by the core, so `Range.parse('A5:F200')` means what it
means in every binding, and `Range.of(firstRow, firstColumn, lastRow, lastColumn)`, 1-based
and `0` for no limit, says what text cannot, such as every row from the fifth; both answer an
outcome, the `'range'` error for one that is not a range. A plan's ordinals count from the
range's first column, while `line()` and a problem's `sourceColumn` stay the sheet's own.

A `Fault` is HyperCast's three fields, and what failed in the cell is beside it:
`batch.detail(column, row)` and `row.detail(column)` are the `Detail` (`'unexpectedCharacter'`,
`'groupSize'`, `'notWhole'` and the rest), or `null` where the cell cast or nothing in it
failed (a blank, a token, padding). `origin` says where a cell's verdict came from: `'cell'`,
`'token'`, `'padded'` or `'uncached'` (a workbook formula with no cached value).
`batch.faults()` is an array of `Problem`s, the row, the line, the plan and source columns, the
reason, detail, origin and span, a long row reported with `null` for what a cell would say. Each
call hands back a new array, the caller's own, worked out when the batch is read; a batch is a
copy, so it stays right after the next read and after the reader has gone. `notes: false`, in
`DelimitedOptions` or `SheetOptions`, drops the detail and the origin, which then say `null`
and `'cell'`: the read costs a little less, and `faults()` can't list a formula with no cached
value.

**To keep every row of a sheet, set `skipEmptyRows: false`.** By default a row whose only cells
are formulas with no cached value is filtered out as empty, with nothing to say so; off, those
rows are delivered, their cells at origin `'uncached'`.

## Suggesting a plan

Declaring the locale, a number format and a date order, is the recommended route, and then
nothing needs inferring. A suggestion is a starting point for a plan to inspect, and it is
never applied. Read the report first: only a resolved column reads through its door in the
plan, the rest as text. With no format declared, each of two notations has to read the whole
column: `1,234,567` and `1,234.5` read only with `,` grouping, `1.234.567`, `1.234,5`, `2,5` and
`0,125` only with `,` as the decimal, and the plan is in whichever is left. `1,234` and `1.234`
read both ways, so they come back `'ambiguous'`, with both formats among the `candidates`, and
read as text until a format is declared or another cell settles them, which takes in a column
of prices at three decimals; `dateOrder` does the same for `01/02/2026`. `2,5` beside `2.5` is
`'unresolved'`, and `1,23,456`, Indian grouping, is text until the column declares Indian
grouping, because nothing says its culture.

`DelimitedReader.suggest(path, dialect?, options?)` and `suggestBytes(bytes, dialect?, options?)`
sample a file or bytes, and `sheet.suggest(options?)` a sheet, whatever it has been up to,
leaving it where it was. Each answers an outcome of a `Suggestion`: `plan`, the `Column`s to
bind, `report`, a `SuggestedColumn` per source column (`ordinal`, `name`, `door`, `declared`,
`format`, `candidates`, `status`, `matched` of `sampled`), and `truncated`. `sampleRows: 0`
means 1; left out it is a thousand rows, at most a batch, and what is not a whole number is
refused. A failure among the sampled rows is the outcome's error, and one past them only says
`truncated`. `columnFormats` and `columnDateOrders`, arrays of `[ordinal, declaration]` pairs,
give a source column its own, ahead of `format` and `dateOrder`, and a format anywhere in them
that the core refuses is a `'plan'` error before anything is opened, whether or not the sample
would reach its column. A report's `format` is `{ packed }`, HyperCast's 92 bytes, so two are
compared by those.

A sheet's number cell has no notation, so a column of them is a number under any format, where
text holding the same digits asks. A column of `0` and `1` is bool, `007` and an integer past
int64 are text, and a typed date in a sheet is text, for a plan to change by hand. With nothing
to choose between, a resolved number is planned in `,` groups of three, and its `format` names
what the plan uses: a later `1,23,456` is refused.

## Grouping

A column given no format is Western, the same as `NumFormat.invariant()`: `1,234,567` is 1234567
and `1,23,456` is `malformed`. If a column of Indian numbers comes back refused, declare it, one
column at a time, so an Indian column reads beside a Western one in the same read, as the
example above does with `NumFormat.invariant().withGrouping(Grouping.Indian)`. Neither policy
reads `0,125`, `12,34`, `1,2,345` or `1234,567` as a number. `NumFormat.detect()` isn't a
policy: a column that declares it infers `,` and `.` from each cell, so `Column.f64` reads
`1,2345` as 1.2345, `12,34` as 12.34 and `0,125` as 0.125. A format holding the retired grouping
bit `256` (the words `351` and `863`) can't be made by HyperCast; one forged past it is refused,
a `'plan'` error naming its column, when the plan is bound, before any cell, and in a suggest's
options before it opens anything. HyperCast's
[grouping notes](https://github.com/SkunkWerkx/HyperCast/blob/master/docs/grouping.md) have
every shape and the locales neither policy matches.

## Calling back in

JavaScript can run in the middle of a reader's call: a getter or a Proxy in a plan or an option
it reads, or a setter someone put on `Object.prototype` that the answer it builds assigns
through. A call from there back into the same reader or sheet answers the `'busy'` error
(`the reader is in the middle of a call; nothing that call runs can call back into it`), and
its getters (`isBound`, `header`, `columnCount`, `records`), with no outcome to answer in,
throw it as an `Error` whose `code` is `ERR_HYPERTABULAR_BUSY` and whose `kind` is `'busy'`.
The outer call goes on unharmed. One exception: a sheet's
`suggest` only looks at the sheet, so another `suggest` from inside it is taken. Other readers
and sheets are there to use. A source is bytes or a path, read synchronously, so no stream of
the caller's runs in the middle of a read.

## The shape

- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header,
  whether blank lines are skipped (`Dialect.CSV`, `Dialect.TSV` and `Dialect.PSV` name the
  usual three) and, [above](#a-messier-file), `skip`, `headerRow`, `shortRows`, `longRows`,
  `encoding` and `tokens`; `SheetOptions` states a sheet's header, whether empty rows are
  skipped, the batch size, `skip`, `headerRow`, `range`, `tokens` and `notes`;
  `DelimitedOptions` the batch size, the first input buffer and `notes`; the plan states each
  column's door and, for numbers, its `NumFormat`. A field left out is the core's default; a
  size of `0` is the core's to refuse, and does.
- **HyperCast is the judge.** A text cell means exactly what `@skunkwerkx/hypercast`'s door
  says of the same bytes, a typed workbook cell is converted by the door directly, and its
  value is the type that door gives: `boolean`; `number` for `i8` to `u32`, `f32` and `f64`;
  `bigint` for `i64` and `u64`; `string` for `decimal` (canonical), `uuid` (lowercase
  hyphenated) and `text`; and HyperCast's `Timestamp`, `CalendarDate`, `TimeOfDay`,
  `CivilDateTime` and `Duration` objects at nanosecond resolution. The column factories carry
  the doors' names: `Column.i32(ordinal, format?)`, `Column.unix(ordinal, precision)`,
  `Column.excelSerial(ordinal, epoch)`, `Column.dateOrdered(ordinal, order)`,
  `Column.datetime(ordinal, order)`, with HyperCast's `UnixPrecision`, `ExcelEpoch` and
  `DateOrder` numbers, and `Workbook.dateSystem` in `ExcelEpoch`'s.
- **Bytes or a path.** `fromBytes` takes a `Uint8Array` (a `Buffer` is one) or a string, read
  as its UTF-8; `open` reads a file, a delimited one a buffer at a time and a workbook whole,
  since a zip is read from its directory at the end back. Every call is synchronous.

`nativeVersion()` reports the core the addon was built from.

## When no native addon fits

`require('@skunkwerkx/hypertabular')` tries the platform package for the host first. When none
fits (the install ran with `--omit=optional`, or the platform is not one of the eight) it loads
the WebAssembly fallback instead: the same core compiled to `wasm32-wasip1`, run in-process
through Node's `node:wasi` by
[`@napi-rs/wasm-runtime`](https://www.npmjs.com/package/@napi-rs/wasm-runtime), from
`@skunkwerkx/hypertabular-wasm32-wasip1`, which the root depends on outright. The API is the
same, the suite is the same, and `isAvailable()` is `true`. What differs:

- Node prints, once, at `require`:

  ```text
  (node:NNN) ExperimentalWarning: WASI is an experimental feature and might change at any time
  ```

  That is Node's own warning about `node:wasi`, not this package's;
  `node --disable-warning=ExperimentalWarning` silences it.
- The install carries the runtime's three dependencies (`@napi-rs/wasm-runtime`,
  `@emnapi/core`, `@emnapi/runtime`) and what they pull in, beside the module.
- Memory is the module's own: it never shrinks, and it can't pass 4 GiB. A batch's copy lives
  there and goes back only when the engine finalizes the batch, once the event loop turns, so a
  long synchronous loop holds every batch it drops until it yields, and one large file is
  enough to run out: three passes over a 91 MB file in one loop reach 1 GB. Yield now and then
  in a long loop (`await new Promise(setImmediate)` after every few dozen reads: the same three
  passes peak at 281 MB yielding after every 64, 215 MB after every read), and `close()` each
  reader and sheet; the suite reads every workbook case under the fallback only because it
  closes each sheet and yields after each case.
- A Proxy whose prototype lookup throws (a `getPrototypeOf` trap that throws, or a revoked
  Proxy) makes a call throw under the fallback, a recorded loss: the runtime's handle lookup
  tests every argument `instanceof WeakRef`, which runs the trap, and its exception unwinds
  through the module before any Rust runs. The module stays sound and the next call answers.
  The native addon never looks at an argument's prototype, so there these are answered like
  any other value.
- On Windows, opening by path is a recorded loss: `DelimitedReader.open` and `Workbook.open`
  answer an `io` error for every path, since the runtime preopens the drive root under its
  Windows name (`D:\`), which the module's libc matches only before a `/`. Read the file
  yourself and pass its bytes to `fromBytes`; the suite reads every case that way there.
- Panics abort. `wasm32-wasip1` has no unwinder, so the fallback is built with
  `panic = "abort"`: a panic surfaces as `RuntimeError: unreachable` thrown from the call.
- Bun 1.4.4 cannot run it: its `node:wasi` has no reactor `initialize`, so Bun with no native
  addon has `isAvailable()` `false`. Bun with the addon passes the whole suite.

`NAPI_RS_FORCE_WASI=error` in the environment loads the fallback even where an addon is
installed and fails rather than falling back further; that is how CI runs the whole suite
against it, and how to try it locally. The consumer smoke runs from the packed tarballs on
Alpine, at the exact floor, Node 22.14.0, and on the floating Node 22 and 24 images; the Node
floor is enforced (`engines`), not advisory.

**Big-endian hosts** (s390x, tested in `node:24-alpine` for `linux/s390x` under QEMU) are a
loss, as for `@skunkwerkx/hypercast`: the fallback loads and every check of the consumer smoke
passes, but the process never exits on its own, since the WebAssembly runtime writes the
module's threadsafe-function reference count little-endian and reads it back host-endian.
Nothing more is claimed for big-endian hosts than the smoke's checks.

**Bundlers.** esbuild and webpack cannot fold the loader's platform `require`s into one file:
webpack fails the build on them, and an esbuild bundle cannot load the module from a directory
with no `node_modules`. The form that works, for both, is the package external (esbuild
`--external:@skunkwerkx/hypertabular`, webpack
`externals: { '@skunkwerkx/hypertabular': 'commonjs @skunkwerkx/hypertabular' }`) with it
installed beside the bundle.

**Browsers.** The fallback package's `browser` entry exports the same classes over a fetched
module (`import('@skunkwerkx/hypertabular-wasm32-wasip1')` under a bundler that honours the
`browser` condition), without this package's iteration helpers: a reader is read with
`read()`, and a batch and `batch.row(index)` are the module's own `Batch` and `Row`, a native
object each, so a loop reading row by row there is better off with `batch.get(column, row)`. Its memory is the
fallback's (above). `browser-smoke/` replays the whole corpus through it in headless Chrome, from a Vite
build served without cross-origin isolation, a numeric column's `NumFormat` coming from
`@skunkwerkx/hypercast-wasm32-wasip1`. Firefox and Safari are not tested.

When the fallback is missing too (a lockfile that pinned neither package, say, or a
`node_modules` pruned by hand), `require` still succeeds, `isAvailable()` is `false`,
`Dialect` is still there, and every opener and factory throws `ERR_HYPERTABULAR_UNAVAILABLE`
with napi's loader error as its `cause`, whose own `cause` chain names every attempt, native
and WebAssembly.

## Bun

The same package runs under Bun on the native addon: `bun test tests/` runs this package's
whole suite. The WebAssembly fallback does not load on Bun 1.4.4
([above](#when-no-native-addon-fits)).

## Local loop

From `typescript/`, with a Rust toolchain installed:

```sh
bun install --frozen-lockfile
bun run build       # the release addon for this machine, plus binding.js and binding.d.ts
bun run test        # node --test: the corpus, never-throws, the types, the declarations
bun run test:bun    # bun test
bun run typecheck
bun run lint
cargo fmt --check && cargo clippy --release -- -D warnings
```

The WebAssembly fallback needs the `wasm32-wasip1` target (`rustup target add wasm32-wasip1`):

```sh
bun run build:wasi                  # hypertabular.wasm32-wasip1.wasm and the hypertabular.wasip1* loaders
NAPI_RS_FORCE_WASI=error bun run test   # the same suite on the fallback, failing rather than falling back
```

`tests/corpus.test.ts` replays `corpus/delimited.json` from memory, from a string and from a
file, in batches of one row, two and many, through an input buffer of one byte, five and the
default, header first and with notes off; `corpus/workbook.json` from bytes and from a path,
by index and by name, header first and bound up front, in batches of one row and of many; and
every suggestion the two corpora hold, reading each plan back. Beside it,
`batch_sizes.test.ts` holds the zero, omitted and over-the-limit sizes (past the limit it asks
for 2^48 rows or more, which no regression could allocate; the sizes just over and at the limit
run only with `HYPERTABULAR_BIG_SIZES=1`, in a container capped at 4 GiB), `messier.test.ts` each dialect and sheet
option, `grouping.test.ts` the two policies and the retired words, `suggest.test.ts` the
suggestions, `reentry.test.ts` the calls back in, and `memory.test.ts`, run only with
`HYPERTABULAR_MEMORY_CHECK=1` and in a container capped in memory, holds a synchronous loop's
growth under 64 MB over three passes of each reading idiom, a batch per row among them. Every cell is held to the corpus, and every
delimited cell to what `@skunkwerkx/hypercast` says of its raw bytes. `HYPERTABULAR_CORPUS`
points it at another directory. Pass `--js binding.js --dts binding.d.ts` and `--platform` to
any other `napi build`, as the build scripts do: without the first two napi writes its
generated loader to `index.js` and `index.d.ts`, over this package's own, and without
`--platform` the WASI build deletes `binding.js` as a stale managed file.

What a consumer gets from the published tarballs is proven by installing them:
[`consumer-tests/`](consumer-tests/README.md) packs the packages, serves them from a local
registry and runs the install scenarios against them.

## License

[MIT](https://github.com/SkunkWerkx/HyperTabular/blob/master/LICENSE)
