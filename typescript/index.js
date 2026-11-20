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
  'checked',
])
const DelimitedReader = nativeClass('DelimitedReader', [
  'fromBytes',
  'open',
  'suggest',
  'suggestBytes',
])
const Workbook = nativeClass('Workbook', ['fromBytes', 'open'])
const Sheet = nativeClass('Sheet', [])
// A batch and a row are plain JavaScript: a batch holds its store (under Node, Bun and Deno an
// `ArrayBuffer` the engine owns) in a private field and reads it through the module, and a row
// is its batch and an index. So what a loop drops, batch or row, is reclaimed by the engine's
// ordinary collection, where a native object each would wait for a finalizer that runs only once
// the event loop turns. The store is private: nothing outside this file can reach it. The native
// `Batch` and `Row` stay for the WebAssembly package used on its own (the browser entry).
const made = Symbol('made')
class Batch {
  #store
  #rows
  #columns
  // The doors' names in plan order, a comma between them.
  #doors
  constructor(token, store, rows, columns, doors) {
    if (token !== made) throw new TypeError('a Batch is made by read(), never by new')
    this.#store = store
    this.#rows = rows
    this.#columns = columns
    this.#doors = doors
  }
  /** How many rows the batch holds. */
  get rows() {
    return this.#rows
  }
  /** How many columns: the plan's length. */
  get columnCount() {
    return this.#columns
  }
  /** The door each column was read through, in plan order: a new array each time. */
  get doors() {
    return this.#columns === 0 ? [] : this.#doors.split(',')
  }
  line(row) {
    return native.batchLine(this.#store, row)
  }
  get(column, row) {
    return native.batchGet(this.#store, column, row)
  }
  getChecked(column, row) {
    return native.batchGetChecked(this.#store, column, row)
  }
  detail(column, row) {
    return native.batchDetail(this.#store, column, row)
  }
  origin(column, row) {
    return native.batchOrigin(this.#store, column, row)
  }
  faults() {
    return native.batchFaults(this.#store)
  }
  raw(column, row) {
    return native.batchRaw(this.#store, column, row)
  }
  rawString(column, row) {
    return native.batchRawString(this.#store, column, row)
  }
  column(column) {
    return native.batchColumn(this.#store, column)
  }
  columnChecked(column) {
    return native.batchColumnChecked(this.#store, column)
  }
  faultCount(column) {
    return native.batchFaultCount(this.#store, column)
  }
  row(index) {
    return typeof index === 'number' && Number.isInteger(index) && index >= 0 && index < this.#rows
      ? new Row(made, this, index + 0)
      : null
  }
  *[Symbol.iterator]() {
    for (let index = 0; index < this.#rows; index++) yield new Row(made, this, index)
  }
}
class Row {
  #batch
  #index
  constructor(token, batch, index) {
    if (token !== made) throw new TypeError('a Row is made by Batch.row(), never by new')
    this.#batch = batch
    this.#index = index
  }
  /** The row's place in its batch. */
  get index() {
    return this.#index
  }
  /** The line or the sheet row it came from. */
  get line() {
    return this.#batch.line(this.#index)
  }
  get(column) {
    return this.#batch.get(column, this.#index)
  }
  getChecked(column) {
    return this.#batch.getChecked(column, this.#index)
  }
  detail(column) {
    return this.#batch.detail(column, this.#index)
  }
  origin(column) {
    return this.#batch.origin(column, this.#index)
  }
  raw(column) {
    return this.#batch.raw(column, this.#index)
  }
  rawString(column) {
    return this.#batch.rawString(column, this.#index)
  }
}
const Header = nativeClass('Header', [])
const Range = nativeClass('Range', ['parse', 'of'])

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
if (native) {
  for (const Reader of [DelimitedReader, Sheet]) {
    Reader.prototype.batches = batches
    Reader.prototype.rows = rows
    Reader.prototype[Symbol.iterator] = batches
  }
  // Every read in this module's environment now makes its batch here. `__makeBatchesWith` and
  // the `batch*` functions are private to this file, not API: the store they take is held in
  // `Batch`'s private field, out of reach of anything else.
  const make = (store, rows, columns, doors) => new Batch(made, store, rows, columns, doors)
  if (!native.__makeBatchesWith(make)) {
    // The WebAssembly fallback makes no call out of the module (its runtime assigns such a
    // call's arguments into an array of its own, under an index, where a setter someone put on
    // `Object.prototype` would run): a read answers the batch in parts, and it is made here.
    for (const Reader of [DelimitedReader, Sheet]) {
      const read = Reader.prototype.read
      Object.defineProperty(Reader.prototype, 'read', {
        ...Object.getOwnPropertyDescriptor(Reader.prototype, 'read'),
        value: {
          read() {
            const outcome = read.call(this)
            if (outcome.ok && outcome.value !== null) {
              const { store, rows, columns, doors } = outcome.value
              outcome.value = make(store, rows, columns, doors)
            }
            return outcome
          },
        }.read,
      })
    }
  }
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
// Asked inside a `catch` of whatever was caught, so it never throws: a Proxy whose traps throw,
// or a revoked one, is not this package's error.
const isHyperTabularError = (value) => {
  try {
    if (!(value instanceof Error)) return false
    const code = value.code
    return typeof code === 'string' && code.startsWith('ERR_HYPERTABULAR_')
  } catch {
    return false
  }
}
module.exports = {
  nativeVersion,
  isAvailable,
  isHyperTabularError,
  Dialect,
  Column,
  Header,
  Range,
  DelimitedReader,
  Workbook,
  Sheet,
  Batch,
  Row,
}
