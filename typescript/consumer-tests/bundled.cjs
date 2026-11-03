// The public-API half of smoke.cjs, for the bundler scenario: a self-contained bundle is
// judged on what it does, not on which file it loaded, so binding.js is never inspected. The
// scenario runs it under NAPI_RS_FORCE_WASI=error, under which the loader never consults the
// native addon, so a pass here is a pass on the WASI module.
'use strict'
const fs = require('node:fs')
const path = require('node:path')
const api = require('@skunkwerkx/hypertabular')

function fail(message) {
  process.stdout.write(`FAIL: ${message}\n`)
  process.exit(1)
}

if (!api.isAvailable()) fail('isAvailable() is false')
process.stdout.write(`available=true version=${api.nativeVersion()}\n`)
const { Column, DelimitedReader, Workbook } = api
const opened = (what, outcome) => {
  if (!outcome.ok) fail(`${what}: ${outcome.error.kind}: ${outcome.error.message}`)
  return outcome.value
}
const reader = opened(
  'fromBytes',
  DelimitedReader.fromBytes('n,m\n(1234),12x4\n', null, [Column.i32(0), Column.i32(1)]),
)
const batch = opened('read', reader.read())
const accounting = batch.get(0, 0)
if (!(accounting.ok && accounting.value === -1234)) fail(`(1234): ${JSON.stringify(accounting)}`)
const bad = batch.get(1, 0)
if (bad.ok || bad.fault.offset !== 2 || bad.fault.length !== 1) fail(`12x4: ${JSON.stringify(bad)}`)
const corpus = process.env.HYPERTABULAR_CORPUS
if (!corpus) fail('HYPERTABULAR_CORPUS is not set')
const load = (name) => JSON.parse(fs.readFileSync(path.join(corpus, name), 'utf8'))
const sheetCase = load('workbook.json').find((c) => c.format !== undefined && !c.failure)
const sheetFile = path.join(corpus, sheetCase.file)
// On Windows no path reaches the WASI module (the README's recorded loss): the path must be
// refused as an I/O error, and the same file is read from its bytes.
const unreachable = process.platform === 'win32'
if (unreachable) {
  const refused = Workbook.open(sheetFile)
  if (refused.ok || refused.error.kind !== 'io')
    fail(`${sheetCase.file} by path under WASI on Windows: ${JSON.stringify(refused)}`)
}
const book = opened(
  sheetCase.file,
  unreachable
    ? Workbook.fromBytes(new Uint8Array(fs.readFileSync(sheetFile)))
    : Workbook.open(sheetFile),
)
if (book.format !== sheetCase.format) fail(`${sheetCase.file}: ${book.format}`)
process.stdout.write('checks: i32 fault workbook OK\n')
