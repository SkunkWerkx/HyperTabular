// A suggestion: each column of a sample tried against the doors, a plan to inspect and change,
// and a report of how far the sample settles each column. Never applied. What a column is
// suggested as is the core's; these tests prove the options reach it, what it says comes
// back whole, and a suggested plan is a plan a read takes.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { mkdtempSync, rmSync, writeFileSync }: typeof import('node:fs') = require('node:fs')
const { tmpdir }: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { DelimitedReader, Workbook } = api

type Outcome<T> = import('../index.js').Outcome<T>
type SuggestOptions = import('../index.js').SuggestOptions

const WORKBOOKS = path.resolve(__dirname, '../../corpus/workbook')
const COMMA = (() => {
  const made = hypercast.NumFormat.declare({
    decimalSep: ',',
    groupSep: '.',
    flags: hypercast.NumStyles.All,
  })
  assert.ok(made.ok)
  return made.value
})()

function value<T>(outcome: Outcome<T>): T {
  assert.ok(outcome.ok, JSON.stringify(outcome))
  return outcome.value
}

const suggest = (data: string, options?: SuggestOptions | null) =>
  value(DelimitedReader.suggestBytes(data, null, options))

test('a report says what the plan does and why', () => {
  const found = suggest('id,when,price,note\n1,2026-01-02,1.234,x\n2,2026-01-03,2.345,\n')
  assert.deepEqual(
    found.report.map((c) => [c.ordinal, c.name, c.door, c.status, c.matched, c.sampled]),
    [
      [0, 'id', 'i32', 'resolved', 2, 2],
      [1, 'when', 'date', 'resolved', 2, 2],
      [2, 'price', 'f64', 'ambiguous', 2, 2],
      [3, 'note', 'text', 'resolved', 1, 1],
    ],
  )
  assert.equal(found.truncated, false)
  // An ambiguous column is planned as text, its door the first candidate's.
  assert.deepEqual(
    found.plan.map((c) => [c.door, c.ordinal]),
    [
      ['i32', 0],
      ['date', 1],
      ['text', 2],
      ['text', 3],
    ],
  )
  const price = found.report[2]
  assert.deepEqual(
    price.candidates.map((c) => [c.kind, c.order]),
    [
      ['numFormat', null],
      ['numFormat', null],
    ],
  )
  assert.deepEqual(price.candidates[0].format?.packed, hypercast.NumFormat.invariant().packed)
  assert.deepEqual(price.candidates[1].format?.packed, COMMA.packed)
  assert.equal(price.format, null)
  // Declaring the notation settles it, either way.
  const declared = suggest('price\n1.234\n2.345\n', { format: COMMA })
  assert.deepEqual([declared.report[0].door, declared.report[0].status], ['i32', 'resolved'])
  assert.deepEqual(declared.report[0].format?.packed, COMMA.packed)
  assert.deepEqual(declared.plan[0].format?.packed, COMMA.packed)
  const western = suggest('price\n1.234\n2.345\n', { format: hypercast.NumFormat.invariant() })
  assert.deepEqual([western.report[0].door, western.report[0].status], ['f64', 'resolved'])
  // The format handed back is a copy, the caller's own.
  const format = declared.report[0].format
  assert.ok(format !== null)
  format.packed[0] ^= 0xff
  assert.deepEqual(declared.plan[0].format?.packed, COMMA.packed)
})

test('a plan reads back what was sampled', () => {
  const data = 'n,t\n1,2026-01-02T03:04:05Z\n2,2026-01-03T03:04:05Z\n'
  const found = suggest(data)
  const batch = value(value(DelimitedReader.fromBytes(data, null, found.plan)).read())
  assert.ok(batch !== null)
  assert.deepEqual(
    batch.doors,
    found.plan.map((c) => c.door),
  )
  assert.deepEqual(batch.faults(), [])
})

test('a declared date order settles a date the sample left open, per column and for all', () => {
  const data = 'a,b\n01/02/2026,01/02/2026\n'
  const open = suggest(data)
  assert.equal(open.report[0].status, 'ambiguous')
  assert.deepEqual(
    open.report[0].candidates.map((c) => c.kind),
    ['dateOrder', 'dateOrder'],
  )
  const all = suggest(data, { dateOrder: hypercast.DateOrder.DayMonthYear })
  assert.deepEqual(
    all.report.map((c) => [c.door, c.declared, c.status]),
    [
      ['dateOrdered', hypercast.DateOrder.DayMonthYear, 'resolved'],
      ['dateOrdered', hypercast.DateOrder.DayMonthYear, 'resolved'],
    ],
  )
  const one = suggest(data, {
    dateOrder: hypercast.DateOrder.DayMonthYear,
    columnDateOrders: [[1, hypercast.DateOrder.MonthDayYear]],
  })
  assert.deepEqual(
    one.report.map((c) => c.declared),
    [hypercast.DateOrder.DayMonthYear, hypercast.DateOrder.MonthDayYear],
  )
  for (const bad of [-1, 99, 'dmy']) {
    const refused = DelimitedReader.suggestBytes(data, null, { dateOrder: bad as number })
    assert.ok(!refused.ok && refused.error.kind === 'argument')
  }
})

test('the last format declared for a column is its format', () => {
  const found = suggest('a\n"1,5"\n', {
    columnFormats: [
      [0, hypercast.NumFormat.invariant()],
      [0, COMMA],
    ],
  })
  assert.deepEqual(found.report[0].format?.packed, COMMA.packed)
  for (const bad of [[[0]], [[0, COMMA, 1]], [['x', COMMA]], 'x', [[0, 'x']]]) {
    const refused = DelimitedReader.suggestBytes('a\n1\n', null, { columnFormats: bad as never })
    assert.ok(!refused.ok, JSON.stringify(bad))
    assert.ok(['argument', 'plan'].includes(refused.error.kind))
  }
})

test('suggest applies nothing, and a path that is not there is an I/O error', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'hypertabular-suggest-'))
  const file = path.join(dir, 'a.csv')
  writeFileSync(file, 'a\n1\n')
  try {
    const found = value(DelimitedReader.suggest(file))
    assert.equal(found.report[0].door, 'bool')
    const reader = value(DelimitedReader.fromBytes('a\n1\n'))
    assert.equal(reader.isBound, false)
    const nowhere = DelimitedReader.suggest(path.join(dir, 'missing.csv'))
    assert.ok(!nowhere.ok && nowhere.error.kind === 'io')
    const wrong = DelimitedReader.suggest(1 as never)
    assert.ok(!wrong.ok && wrong.error.kind === 'argument')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('a sheet suggests from its first row under its own options, and its read stays where it is', () => {
  const book = value(Workbook.open(path.join(WORKBOOKS, 'basic.xlsx')))
  const sheet = value(book.sheet(0))
  const before = value(sheet.suggest())
  const plan = before.plan
  assert.deepEqual(value(sheet.bind(plan)), null)
  const first = value(sheet.read())
  assert.ok(first !== null)
  // Bound and part read, it answers the same.
  const after = value(sheet.suggest())
  assert.deepEqual(after.report, before.report)
  assert.equal(after.truncated, before.truncated)
  assert.deepEqual(
    after.plan.map((c) => [c.door, c.ordinal]),
    plan.map((c) => [c.door, c.ordinal]),
  )
  // Its sample is at most its batch.
  const small = value(value(book.sheet(0, { batchRows: 1 })).suggest({ sampleRows: 1000 }))
  assert.ok(small.report.every((c) => c.sampled <= 1))
})

test('a failure among the sampled rows is said, and one past them is not', () => {
  const failed = DelimitedReader.suggestBytes('a\n1\n"2\n')
  assert.ok(!failed.ok && failed.error.kind === 'structure')
  const past = suggest('a\n1\n"2\n', { sampleRows: 1 })
  assert.equal(past.truncated, true)
  assert.equal(past.report[0].sampled, 1)
})

test('nothing in the source is nothing to plan, and no header has no names', () => {
  assert.deepEqual(suggest('').plan, [])
  const headerless = value(DelimitedReader.suggestBytes('1,x\n2,y\n', { hasHeader: false }))
  assert.deepEqual(
    headerless.report.map((c) => c.name),
    [null, null],
  )
  assert.deepEqual(
    headerless.report.map((c) => c.sampled),
    [2, 2],
  )
})

test("a dialect's declarations are the sample's", () => {
  const found = value(
    DelimitedReader.suggestBytes('skip me\na;b\n1;NA\n2;3\n', {
      separator: ';',
      skip: 1,
      tokens: ['NA'],
    }),
  )
  assert.deepEqual(
    found.report.map((c) => [c.name, c.door, c.sampled]),
    [
      ['a', 'i32', 2],
      ['b', 'i32', 1],
    ],
  )
  const refused = DelimitedReader.suggestBytes('a', { separator: '"' })
  assert.ok(!refused.ok && refused.error.kind === 'separator')
})

test('options are checked where they are read, and a bad format is refused before the source', () => {
  for (const options of [1, 'x', true]) {
    const refused = DelimitedReader.suggestBytes('a\n1\n', null, options as never)
    assert.ok(!refused.ok && refused.error.kind === 'argument')
  }
  // The options come first: a stale format over bytes of the wrong type is still the format.
  const stale = { packed: new Uint8Array(92) }
  const first = DelimitedReader.suggestBytes(1 as never, null, { format: stale })
  assert.ok(!first.ok)
  assert.deepEqual([first.error.kind, first.error.column], ['plan', 0])
  const column = DelimitedReader.suggestBytes('a\n1\n', null, { columnFormats: [[3, stale]] })
  assert.ok(!column.ok)
  assert.deepEqual([column.error.kind, column.error.column], ['plan', 3])
  assert.equal(
    column.error.message,
    'plan column 3 declares a numeric format, date options or an ordinal that cannot be honoured',
  )
})
