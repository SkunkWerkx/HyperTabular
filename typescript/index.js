let native = null
let loadError = null
try {
  native = require('./binding.js')
} catch (error) {
  loadError = error
}
const unavailable = (name) => () => {
  const error = new Error(
    `hypertabular: the native addon did not load, so ${name}() is unavailable`,
    { cause: loadError },
  )
  error.code = 'ERR_HYPERTABULAR_UNAVAILABLE'
  throw error
}
const nativeVersion = native ? native.nativeVersion : unavailable('nativeVersion')
// The classes, or stand-ins whose static methods throw the same unavailable error.
const nativeClass = (name, statics) => {
  if (native) return native[name]
  const stand = { [name]: class {} }[name]
  for (const method of statics) stand[method] = unavailable(`${name}.${method}`)
  return stand
}
const Column = nativeClass('Column', [
  'bool',
  'i8',
  'i16',
  'i32',
  'i64',
  'u8',
  'u16',
  'u32',
  'u64',
  'f32',
  'f64',
  'decimal',
  'uuid',
  'timestamp',
  'unix',
  'excelSerial',
  'date',
  'dateOrdered',
  'datetime',
  'time',
  'duration',
  'text',
])
const DelimitedReader = nativeClass('DelimitedReader', ['fromBytes', 'open'])
const Workbook = nativeClass('Workbook', ['fromBytes', 'open'])
const Sheet = nativeClass('Sheet', [])
const Batch = nativeClass('Batch', [])
const Row = nativeClass('Row', [])
const Header = nativeClass('Header', [])

// Iteration, which never throws either: a reader yields each read's outcome, the error (if
// one stops it) once, as the last; a batch yields its rows.
function* batches() {
  for (;;) {
    const read = this.read()
    if (!read.ok) {
      yield read
      return
    }
    if (read.value === null) return
    yield read
  }
}
function* rows() {
  for (const read of this.batches()) {
    if (!read.ok) {
      yield read
      return
    }
    for (const row of read.value) yield { ok: true, value: row }
  }
}
function* batchRows() {
  for (let index = 0; index < this.rows; index++) yield this.row(index)
}
if (native) {
  for (const Reader of [DelimitedReader, Sheet]) {
    Reader.prototype.batches = batches
    Reader.prototype.rows = rows
    Reader.prototype[Symbol.iterator] = batches
  }
  Batch.prototype[Symbol.iterator] = batchRows
}

const dialect = (separator) =>
  Object.freeze({ separator, quoting: true, hasHeader: true, skipBlankLines: true })
// The three dialects by name: comma, tab and pipe, each quoted, with a header, blank lines
// skipped.
const Dialect = Object.freeze({
  CSV: dialect(','),
  TSV: dialect('\t'),
  PSV: dialect('|'),
})
const isAvailable = () => native !== null
const isHyperTabularError = (value) =>
  value instanceof Error &&
  typeof value.code === 'string' &&
  value.code.startsWith('ERR_HYPERTABULAR_')
module.exports = {
  nativeVersion,
  isAvailable,
  isHyperTabularError,
  Dialect,
  Column,
  Header,
  DelimitedReader,
  Workbook,
  Sheet,
  Batch,
  Row,
}
