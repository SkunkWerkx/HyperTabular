// A batch size is the caller's, and the core judges it before anything is allocated: a zero, a
// size past the core's limit and a row past it are the `argument` error in the core's words.
// What this binding refuses itself is only what is no size at all (a negative, a fraction, a
// string). Omitted, each size is the core's default.
//
// Past the core's limit, every test that runs by default asks for 2^48 rows or more: were the
// core's check to go, the arrays asked for would be past any 64-bit address space, so the
// allocation fails rather than takes memory. The tests at sizes just over the limit (2^27 and
// 2^28 rows, whose refusals name the bytes asked for) and at it (2^29) would take gigabytes on
// a regression, so they run only with `HYPERTABULAR_BIG_SIZES=1`, in a container capped at 4 GiB.
// The read at 2^21 rows holds about 125 MiB untouched, however right the code is.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { mkdtempSync, rmSync, writeFileSync }: typeof import('node:fs') = require('node:fs')
const { tmpdir }: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Workbook } = api

type Outcome<T> = import('../index.js').Outcome<T>

const BOOK = path.resolve(__dirname, '../../corpus/workbook/basic-1904.xlsx')
const OVER_LIMIT = /^batch_rows asks for \d+ bytes, past the limit of 4294967296$/
const NO_ROWS = 'batch_rows must be at least 1'
const NO_BUFFER = 'buffer_bytes must be at least 1'
const TEXT = 'a\n1\n'
// Rows past any address space at every column's width: a regression can't allocate them.
const HUGE = 2 ** 48
const big =
  process.env.HYPERTABULAR_BIG_SIZES === '1'
    ? false
    : 'HYPERTABULAR_BIG_SIZES=1 runs it, in a container capped at 4 GiB'

function book() {
  const opened = Workbook.open(BOOK)
  assert.ok(opened.ok)
  return opened.value
}

// `outcome` is the argument error saying exactly `message`, or matching it.
function refuses(message: string | RegExp, outcome: Outcome<unknown>): void {
  assert.ok(!outcome.ok, `taken, where it should say: ${message}`)
  assert.equal(outcome.error.kind, 'argument', JSON.stringify(outcome.error))
  if (typeof message === 'string') assert.equal(outcome.error.message, message)
  else assert.match(outcome.error.message, message)
}

function firstRows(
  outcome: Outcome<import('../index.js').DelimitedReader | import('../index.js').Sheet>,
) {
  assert.ok(outcome.ok, JSON.stringify(outcome))
  const read = outcome.value.read()
  assert.ok(read.ok && read.value !== null, JSON.stringify(read))
  return read.value
}

test('every entry point opened with a zero is refused, and each omitted is its default', () => {
  const plan = [Column.i32(0)]
  const dir = mkdtempSync(path.join(tmpdir(), 'hypertabular-sizes-'))
  const file = path.join(dir, 'a.csv')
  const missing = path.join(dir, 'no such file.csv')
  writeFileSync(file, TEXT)
  try {
    for (const usedPlan of [plan, null]) {
      refuses(NO_ROWS, DelimitedReader.fromBytes(TEXT, null, usedPlan, { batchRows: 0 }))
      refuses(NO_BUFFER, DelimitedReader.fromBytes(TEXT, null, usedPlan, { bufferBytes: 0 }))
      refuses(
        NO_ROWS,
        DelimitedReader.fromBytes(new Uint8Array(0), null, usedPlan, { batchRows: 0 }),
      )
      refuses(NO_ROWS, DelimitedReader.open(file, null, usedPlan, { batchRows: 0 }))
      refuses(NO_BUFFER, DelimitedReader.open(file, null, usedPlan, { bufferBytes: 0 }))
      // A path's sizes are judged before the file is touched.
      refuses(NO_ROWS, DelimitedReader.open(missing, null, usedPlan, { batchRows: 0 }))
      refuses(NO_BUFFER, DelimitedReader.open(missing, null, usedPlan, { bufferBytes: 0 }))
      refuses(NO_ROWS, book().sheet(0, { batchRows: 0 }, usedPlan))
      refuses(NO_ROWS, book().sheet(99, { batchRows: 0 }, usedPlan))
    }
    const nowhere = DelimitedReader.open(missing)
    assert.ok(!nowhere.ok && nowhere.error.kind === 'io')

    // Omitted, or `undefined`, each is its default, and reads.
    for (const options of [undefined, null, {}, { batchRows: undefined, bufferBytes: undefined }]) {
      assert.deepEqual(firstRows(DelimitedReader.fromBytes(TEXT, null, plan, options)).get(0, 0), {
        ok: true,
        value: 1,
      })
      assert.equal(firstRows(DelimitedReader.open(file, null, plan, options)).rows, 1)
    }
    assert.ok(firstRows(book().sheet(0, {}, plan)).rows > 0)
    assert.ok(firstRows(book().sheet(0, { batchRows: undefined }, plan)).rows > 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('what is no size at all is refused here, before the core is asked', () => {
  for (const size of [-1, 1.5, Number.NaN, Number.POSITIVE_INFINITY, '1', 1n, true]) {
    for (const outcome of [
      DelimitedReader.fromBytes(TEXT, null, null, { batchRows: size as number }),
      DelimitedReader.fromBytes(TEXT, null, null, { bufferBytes: size as number }),
      book().sheet(0, { batchRows: size as number }),
      DelimitedReader.suggestBytes(TEXT, null, { sampleRows: size as number }),
    ]) {
      assert.ok(!outcome.ok && outcome.error.kind === 'argument', String(size))
      assert.match(outcome.error.message, /must be a whole number, 0 or more$/)
    }
  }
})

test('a sample of none is a sample of one; omitted, it is the thousand', () => {
  const three = 'a\n1\n2\n3\n'
  const sampled = (options?: import('../index.js').SuggestOptions) => {
    const found = DelimitedReader.suggestBytes(three, null, options)
    assert.ok(found.ok)
    return [found.value.report[0].sampled, found.value.truncated]
  }
  assert.deepEqual(sampled({ sampleRows: 0 }), [1, true])
  assert.deepEqual(sampled({ sampleRows: 1 }), [1, true])
  assert.deepEqual(sampled(), [3, false])
  assert.deepEqual(sampled({}), [3, false])
  assert.deepEqual(sampled({ sampleRows: undefined }), [3, false])
  const sheet = book().sheet(0)
  assert.ok(sheet.ok)
  const none = sheet.value.suggest({ sampleRows: 0 })
  const one = sheet.value.suggest({ sampleRows: 1 })
  assert.ok(none.ok && one.ok)
  assert.deepEqual(none.value.report, one.value.report)
})

test('a plan of no columns reads at any batch size', () => {
  for (const batchRows of [HUGE, Number.MAX_SAFE_INTEGER]) {
    const read = firstRows(DelimitedReader.fromBytes(TEXT, null, [], { batchRows }))
    assert.equal(read.rows, 1)
  }
})

test('a modest batch reads the same as a row at a time', () => {
  const plan = [Column.text(0)]
  const one = book().sheet(0, { batchRows: 1 }, plan)
  assert.ok(one.ok)
  const values = [...one.value].map((read) => (read.ok ? read.value.get(0, 0) : read))
  const whole = firstRows(book().sheet(0, { batchRows: 4096 }, plan))
  assert.deepEqual(values, whole.column(0))
})

test("a batch past the core's limit is refused before allocating", () => {
  const plan = [Column.i32(0)]
  refuses(OVER_LIMIT, DelimitedReader.fromBytes(TEXT, null, plan, { batchRows: HUGE }))
  refuses(OVER_LIMIT, DelimitedReader.open('/no/such/file.csv', null, plan, { batchRows: HUGE }))
  refuses(OVER_LIMIT, book().sheet(0, { batchRows: HUGE }, plan))
  // Past 2^53 a number is a whole number held only rounded, and still the core's to refuse.
  for (const batchRows of [Number.MAX_SAFE_INTEGER, 2 ** 53, 2 ** 64, Number.MAX_VALUE]) {
    refuses(OVER_LIMIT, DelimitedReader.fromBytes(TEXT, null, plan, { batchRows }))
  }
  for (const batchRows of [-1, 1.5, Number.POSITIVE_INFINITY, Number.NaN]) {
    const read = DelimitedReader.fromBytes(TEXT, null, plan, { batchRows })
    assert.ok(!read.ok)
    assert.match(read.error.message, /batchRows must be a whole number, 0 or more/)
  }
})

test("a batch just past the core's limit is refused, naming the bytes it asks for", {
  skip: big,
}, () => {
  const plan = [Column.i32(0)]
  const justOver = 'batch_rows asks for 5377130496 bytes, past the limit of 4294967296'
  refuses(justOver, DelimitedReader.fromBytes(TEXT, null, plan, { batchRows: 2 ** 28 }))
  refuses(justOver, DelimitedReader.open('/no/such/file.csv', null, plan, { batchRows: 2 ** 28 }))
  refuses(
    'batch_rows asks for 4831838224 bytes, past the limit of 4294967296',
    book().sheet(0, { batchRows: 2 ** 27 }, plan),
  )
})

function lateReader(batchRows: number) {
  const reader = DelimitedReader.fromBytes(TEXT, null, null, { batchRows })
  assert.ok(reader.ok)
  refuses(OVER_LIMIT, reader.value.bind([Column.i32(0)]))
  assert.equal(reader.value.isBound, false)
}

function noColumns(readerRows: number, sheetRows: number) {
  refuses(
    OVER_LIMIT,
    DelimitedReader.fromBytes(TEXT, { longRows: 'report' }, [], { batchRows: readerRows }),
  )
  refuses(OVER_LIMIT, book().sheet(0, { batchRows: sheetRows }, []))
}

// A sheet keeps its cells before any plan, so one at 2^48 rows is refused when it opens, and
// only a size just past the limit opens unbound and refuses the plan.
test('a late plan past the limit is refused, and leaves the reader unbound', () => {
  lateReader(HUGE)
  refuses(OVER_LIMIT, book().sheet(0, { batchRows: HUGE }))
})

test('a late plan just past the limit leaves the reader and the sheet unbound', {
  skip: big,
}, () => {
  lateReader(2 ** 28)
  const sheet = book().sheet(0, { batchRows: 2 ** 27 })
  assert.ok(sheet.ok)
  refuses(OVER_LIMIT, sheet.value.bind([Column.i32(0)]))
  assert.equal(sheet.value.isBound, false)
})

test("a plan of no columns is refused where a row's width or a sheet's cells are kept", () => {
  noColumns(HUGE, HUGE)
})

test('a plan of no columns just past the limit is refused the same', { skip: big }, () => {
  noColumns(2 ** 31, 2 ** 30)
})

test('a suggestion is never refused for the size of its own batch', () => {
  const width = 268_000
  const names = Array.from({ length: width }, (_, column) => `c${column}`).join(',')
  const row = Array(width).fill('1').join(',')
  const wide = DelimitedReader.suggestBytes(`${names}\n${row}\n${row}\n`)
  assert.ok(wide.ok)
  assert.equal(wide.value.report.length, width)
  assert.equal(wide.value.report[width - 1].sampled, 2)
  assert.equal(wide.value.truncated, false)
  // A sample past every batch.
  const sheet = book().sheet(0)
  assert.ok(sheet.ok)
  const huge = sheet.value.suggest({ sampleRows: HUGE })
  const small = sheet.value.suggest({ sampleRows: 4096 })
  assert.ok(huge.ok && small.ok)
  assert.deepEqual(huge.value.report, small.value.report)
  assert.equal(huge.value.truncated, small.value.truncated)
})

test("a suggestion at exactly the core's limit, with no plan, samples what a modest one does", {
  skip: big,
}, () => {
  const large = book().sheet(0, { batchRows: 2 ** 29 })
  const modest = book().sheet(0)
  assert.ok(large.ok && modest.ok)
  const huge = large.value.suggest({ sampleRows: HUGE })
  const small = modest.value.suggest({ sampleRows: 4096 })
  assert.ok(huge.ok && small.ok)
  assert.deepEqual(huge.value.report, small.value.report)
  assert.equal(huge.value.truncated, small.value.truncated)
})

test("a large batch's memory isn't written to; a read goes through it", () => {
  const rows = 2 ** 21
  const read = firstRows(
    DelimitedReader.fromBytes(TEXT, null, [Column.i32(0)], { batchRows: rows }),
  )
  assert.deepEqual(read.column(0), [{ ok: true, value: 1 }])
  assert.ok(firstRows(book().sheet(0, { batchRows: rows }, [Column.text(0)])).rows > 0)
})
