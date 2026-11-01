// What a consumer sees: require('@skunkwerkx/hypertabular'), ask whether anything loaded, and if
// so use it. Prints one line, `available=<bool> target=<native|wasm32-wasip1|none>
// [version=<v>]`, which run.sh and container/consumer.sh assert on. When nothing loaded, the
// first opener must throw ERR_HYPERTABULAR_UNAVAILABLE and its cause chain is printed one per
// line, so the negative controls can see that both the native attempts and the WASI one were
// made. binding.js is consulted for the target only after isAvailable() says true: index.js is
// what tolerates a failing binding.js, and this script never requires it when nothing loaded.
'use strict'
const fs = require('node:fs')
const path = require('node:path')
const api = require('@skunkwerkx/hypertabular')

function fail(message) {
  process.stdout.write(`FAIL: ${message}\n`)
  process.exit(1)
}

if (!api.isAvailable()) {
  process.stdout.write('available=false target=none\n')
  let thrown
  try {
    api.DelimitedReader.fromBytes('a\n1\n')
  } catch (error) {
    thrown = error
  }
  if (!thrown) fail('DelimitedReader.fromBytes() did not throw with nothing loaded')
  if (thrown.code !== 'ERR_HYPERTABULAR_UNAVAILABLE')
    fail(`DelimitedReader.fromBytes() threw ${thrown.code}, not ERR_HYPERTABULAR_UNAVAILABLE`)
  for (let cause = thrown.cause; cause; cause = cause.cause) {
    process.stdout.write(`cause: ${cause.constructor.name}: ${cause.message.split('\n')[0]}\n`)
  }
  process.exit(0)
}

const binding = require(
  path.join(path.dirname(require.resolve('@skunkwerkx/hypertabular')), 'binding.js'),
)
process.stdout.write(
  `available=true target=${binding.__napiBindingTarget} version=${api.nativeVersion()}\n`,
)

const { Column, DelimitedReader, Workbook } = api
const same = (a, b) =>
  JSON.stringify(a, (_k, v) => (typeof v === 'bigint' ? `${v}n` : v)) ===
  JSON.stringify(b, (_k, v) => (typeof v === 'bigint' ? `${v}n` : v))
const opened = (what, outcome) => {
  if (!outcome.ok) fail(`${what}: ${outcome.error.kind}: ${outcome.error.message}`)
  return outcome.value
}

// The u64 maximum, and a cell that fails its door, as a verdict with its span in the raw bytes.
const wide = opened(
  'fromBytes',
  DelimitedReader.fromBytes('n,m\n18446744073709551615,12x4\n', null, [
    Column.u64(0),
    Column.i32(1),
  ]),
)
const batch = opened('read', wide.read())
if (!same(batch.get(0, 0), { ok: true, value: 2n ** 64n - 1n })) fail('u64 of the u64 maximum')
const bad = batch.get(1, 0)
if (!same(bad, { ok: false, fault: { reason: 'malformed', offset: 2, length: 1 } }))
  fail(`12x4 as i32: ${JSON.stringify(bad)}`)
// The peer: the cell's verdict is what @skunkwerkx/hypercast's own door says of its bytes.
const hypercast = require('@skunkwerkx/hypercast')
if (!same(hypercast.castI32(batch.raw(1, 0)), bad)) fail('the peer disagrees on 12x4')
wide.close()
// A contract violation is an outcome's error, not a throw.
const contract = DelimitedReader.fromBytes(42)
if (contract.ok || contract.error.kind !== 'argument')
  fail(`fromBytes(42): ${JSON.stringify(contract)}`)

const corpus = process.env.HYPERTABULAR_CORPUS
if (!corpus) fail('HYPERTABULAR_CORPUS is not set')
const load = (name) => JSON.parse(fs.readFileSync(path.join(corpus, name), 'utf8'))
const columnOf = (entry) => {
  switch (entry.door) {
    case 'unix':
      return Column.unix(entry.ordinal, entry.precision)
    case 'excel_serial':
      return Column.excelSerial(entry.ordinal, entry.epoch)
    case 'date_ordered':
      return Column.dateOrdered(entry.ordinal, entry.order)
    case 'datetime':
      return Column.datetime(entry.ordinal, entry.order)
    default:
      return Column[entry.door](entry.ordinal)
  }
}
const readAll = (reader) => {
  let rows = 0
  for (const read of reader.batches()) {
    if (!read.ok) return { rows, error: read.error }
    rows += read.value.rows
  }
  return { rows }
}

// A structural failure from the delimited corpus: every row before it, then the failure.
const broken = load('delimited.json').find((c) => c.failure && !c.plan.some((p) => p.format))
const reader = opened(
  broken.name,
  DelimitedReader.fromBytes(
    broken.input,
    {
      separator: broken.dialect.separator,
      quoting: broken.dialect.quoting,
      hasHeader: broken.dialect.has_header,
      skipBlankLines: broken.dialect.skip_blank_lines,
    },
    broken.plan.map(columnOf),
  ),
)
const delimited = readAll(reader)
if (delimited.rows !== broken.rows.length || delimited.error?.failure?.byte !== broken.failure.byte)
  fail(`delimited case ${broken.name}: ${JSON.stringify(delimited)}`)

// A workbook from its file: its rows, as many as the corpus lists.
const sheetCase = load('workbook.json').find((c) => c.format !== undefined && !c.failure)
const book = opened(sheetCase.file, Workbook.open(path.join(corpus, sheetCase.file)))
const sheet = opened(
  sheetCase.name,
  book.sheet(
    sheetCase.sheet,
    { hasHeader: sheetCase.options.has_header, skipEmptyRows: sheetCase.options.skip_empty_rows },
    sheetCase.plan.map(columnOf),
  ),
)
const sheetRows = readAll(sheet)
if (sheetRows.error || sheetRows.rows !== sheetCase.rows.length)
  fail(`workbook case ${sheetCase.name}: ${JSON.stringify(sheetRows)}`)
sheet.close()
process.stdout.write('checks: u64 fault peer contract delimited-case workbook-case OK\n')
