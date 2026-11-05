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
interface TabularError { kind: ErrorKind; message: string; failure?: StructuralFailure; column?: number }
```

- **A bad value is a verdict.** A cell that does not cast is a `Failure` in its column, with
  HyperCast's reason (`'empty'`, `'malformed'`, `'outOfRange'`) and span, and the read goes
  on. `Verdict`, `Success`, `Failure`, `Fault` and `CastFailure` are re-exported from
  `@skunkwerkx/hypercast` unchanged, so a cell's verdict is assignable to HyperCast's
  `Verdict<T>` and back.
- **A broken file is an outcome's error.** A record of the wrong width, input that ends inside
  a quoted cell, a workbook whose container or parts cannot be read: `read()` answers
  `{ ok: false, error }` with `kind: 'structure'` and a `failure` saying what (`'columnCount'`,
  `'unclosedQuote'`, `'notAZip'`, `'encrypted'` and the rest of `FailureKind`) and where
  (`record`, `line`, `byte`, and `expected` and `found` for the kinds that say) — after every
  intact row before it has been delivered, and the same error again on every read after.
- **Every other error is an outcome's error too**, `kind` saying which: `'io'` (a file that
  cannot be opened or read), `'separator'` (a dialect the core cannot honour), `'plan'` (a
  column it cannot honour, `column` its place in the plan), `'noSheet'`, `'noColumn'`,
  `'unbound'` (a read before a plan), `'alreadyBound'`, `'closed'` (a read or bind after
  `close()`), and `'argument'`: a value of a type the declarations rule out — a number for
  the bytes, a plan that is not an array of `Column`s, an option that is not a whole number.
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

The one error the package throws is `ERR_HYPERTABULAR_UNAVAILABLE`, from any opener when no
addon and no fallback loaded: an installation problem, not an input
([below](#when-no-native-addon-fits)). `isAvailable()` says up front, and
`isHyperTabularError(error)` recognizes it. On the native addon every call also catches a Rust
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

The header is a `Header`: `names` (the strings, in order), `length`, `ordinal(name)` — the
first column with exactly that name, case and spaces included, or `null` — and
`require(name)`, the same as an outcome whose error is `'noColumn'`, naming it.

## Rows and batches

A batch is column-major, and owns what it shows: the reader's cells are copied out of it, so a
batch stays valid after the reader has moved on. It has `rows`, `columnCount`, `doors`,
`line(row)`, `get(column, row)`, `raw(column, row)`, `rawString(column, row)`,
`column(column)` (the whole column's verdicts), `faultCount(column)` and `row(row)`, and its
rows when iterated. A `Row` reads it across, for code that builds an object a row at a time,
with `index`, `line`, `get(column)`, `raw(column)` and `rawString(column)`.

A reader or a sheet is iterable: `for (const read of reader)` (or `reader.batches()`) yields
each read's outcome, the error that stops it (if one does) once, as the last, and
`reader.rows()` yields every row the same way, batch after batch. `read()` is the same one
step at a time: the batch, `null` past the end, or the error.

`close()` releases a reader's or a sheet's buffers (and a reader's file) now, rather than when
it is collected; a workbook's bytes go when the workbook and its sheets are collected. A sheet
read with a wide plan holds a cell table of several megabytes until then, so a loop opening
many is better off closing each.

## The shape

- **Nothing is sniffed.** The `Dialect` states the separator, the quoting, the header and
  whether blank lines are skipped (`Dialect.CSV`, `Dialect.TSV` and `Dialect.PSV` name the
  usual three); `SheetOptions` states a sheet's header, whether empty rows are skipped and the
  batch size; `DelimitedOptions` the batch size and the first input buffer; the plan states
  each column's door and, for numbers, its `NumFormat`.
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
- Memory is the module's own, up to 4 GiB, and V8 cannot see it to collect sooner, so
  `close()` matters more there: the suite reads every workbook case under the fallback only
  because it closes each sheet.
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
`read()`. `browser-smoke/` replays the whole corpus through it in headless Chrome, from a Vite
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
default, and header first; and `corpus/workbook.json` by index and by name, header first and
bound up front, in batches of one row and of many. Every cell is held to the corpus, and every
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
