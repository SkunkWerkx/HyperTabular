// A column's grouping: Western unless the caller declares Indian on that column, in the same
// read as a column that doesn't, and a stale policy word refused where it is handed over,
// before any cell. What the shapes read is HyperCast's and the corpus's.
//
// HyperCast's own constructor refuses a stale word, so what an old program's stored format
// would hand over is stood in for here by an object carrying the 92 packed bytes with that word
// in them. The door these tests prove is this binding's `format_of`, which every factory and
// every suggest takes, and which resolves a format through HyperCast.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { mkdtempSync, rmSync, writeFileSync }: typeof import('node:fs') = require('node:fs')
const { tmpdir }: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Workbook } = api

type Outcome<T> = import('../index.js').Outcome<T>
type Packed = { packed: Uint8Array }

const WESTERN = hypercast.NumFormat.invariant()
const INDIAN = (() => {
  const made = WESTERN.withGrouping(hypercast.Grouping.Indian)
  assert.ok(made.ok)
  return made.value
})()
const BOTH = 'indian,western\n"1,23,456","1,23,456"\n"1,234,567","1,234,567"\n'
const WORKBOOKS = path.resolve(__dirname, '../../corpus/workbook')
const GROUPED = ['xlsx', 'ods', 'numbers'].map((kind) => path.join(WORKBOOKS, `grouping.${kind}`))

// Where a format's packed bytes hold its flags and policy word: where the two formats differ.
const WORD = (() => {
  let at = 0
  while (WESTERN.packed[at] === INDIAN.packed[at]) at++
  return at & ~3
})()
const policy = (format: Packed) =>
  new DataView(format.packed.buffer, format.packed.byteOffset).getUint32(WORD, true)

// A format stored by an older HyperCast, or an old program's literal: the word as it arrives.
function stale(word: number): Packed {
  const packed = Uint8Array.from(WESTERN.packed)
  new DataView(packed.buffer).setUint32(WORD, word, true)
  return { packed }
}

const WORDS: [number, boolean][] = [
  [351, true],
  [863, true],
  [95, false],
  [607, false],
]

function value<T>(outcome: Outcome<T>): T {
  assert.ok(outcome.ok, JSON.stringify(outcome))
  return outcome.value
}

// `outcome` is the core's `plan` error at `column`, or it is taken.
function rejects(refused: boolean, outcome: Outcome<unknown>, column: number) {
  if (!refused) {
    assert.ok(outcome.ok, JSON.stringify(outcome))
    return
  }
  assert.ok(!outcome.ok, 'taken')
  assert.deepEqual([outcome.error.kind, outcome.error.column], ['plan', column])
  assert.match(outcome.error.message, /cannot be honoured$/)
}

test('the stand-in packs what the word says', () => {
  assert.equal(policy(WESTERN), 95)
  assert.equal(policy(INDIAN), 607)
  assert.deepEqual(stale(95).packed, WESTERN.packed)
  assert.deepEqual(stale(607).packed, INDIAN.packed)
  const batch = value(
    value(DelimitedReader.fromBytes(BOTH, null, [Column.i32(0, stale(607))])).read(),
  )
  assert.ok(batch !== null)
  assert.deepEqual(batch.get(0, 0), { ok: true, value: 123456 })
  // Why it takes a stand-in: HyperCast itself won't make one.
  for (const word of [351, 863]) {
    const made = hypercast.NumFormat.declare({
      decimalSep: '.',
      groupSep: ',',
      flags: word & 0xff,
      grouping: (word & ~0xff) as never,
    })
    assert.equal(made.ok, false)
  }
})

function assertBoth(batch: import('../index.js').Batch, indian: number, western: number) {
  assert.deepEqual(batch.get(indian, 0), { ok: true, value: 123456 })
  assert.equal(batch.get(western, 0).ok, false)
  assert.equal(batch.detail(western, 0), 'groupSize')
  assert.equal(batch.get(indian, 1).ok, false)
  assert.deepEqual(batch.get(western, 1), { ok: true, value: 1234567 })
}

test('an Indian column reads beside a Western one', () => {
  const batch = value(
    value(DelimitedReader.fromBytes(BOTH, null, [Column.i32(0, INDIAN), Column.i32(1)])).read(),
  )
  assert.ok(batch !== null)
  assertBoth(batch, 0, 1)
  // Left out, a column's format is the core's default, Western.
  assert.equal(policy(Column.i32(1).format as Packed), 95)
  assert.equal(policy(Column.i32(1, INDIAN).format as Packed), 607)
})

test('an Indian sheet column reads beside a Western one', () => {
  for (const file of GROUPED) {
    const sheet = value(
      value(Workbook.open(file)).sheet(0, null, [Column.i32(2, INDIAN), Column.i32(2)]),
    )
    const batch = value(sheet.read())
    assert.ok(batch !== null)
    assertBoth(batch, 0, 1)
    // A typed number has no notation: it is the same under either.
    assert.deepEqual(batch.get(0, 2), { ok: true, value: 1234 })
    assert.deepEqual(batch.get(1, 2), { ok: true, value: 1234 })
  }
})

test('a stale policy is refused at every delimited entry point, before any cell', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'hypertabular-grouping-'))
  const file = path.join(dir, 'grouping.csv')
  writeFileSync(file, BOTH)
  try {
    for (const [word, refused] of WORDS) {
      // A format is taken by a number door's factory, which is where it is handed over.
      const numeric = [Column.i32(0), Column.i32(1, stale(word))]
      for (const each of [numeric, [Column.text(0), Column.decimal(1, stale(word))]]) {
        rejects(refused, DelimitedReader.fromBytes(BOTH, null, each), 1)
        rejects(refused, DelimitedReader.open(file, null, each), 1)
        const unbound = value(DelimitedReader.fromBytes(BOTH))
        const records = unbound.records
        rejects(refused, unbound.bind(each), 1)
        assert.equal(unbound.isBound, !refused)
        if (refused) {
          // Nothing bound, and no cell read.
          assert.equal(unbound.records, records)
          const early = unbound.read()
          assert.ok(!early.ok && early.error.kind === 'unbound')
        }
      }
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('a stale policy is refused at every sheet entry point', () => {
  const book = value(Workbook.open(GROUPED[0]))
  for (const [word, refused] of WORDS) {
    const plan = [Column.i32(0), Column.i32(1, stale(word))]
    rejects(refused, book.sheet(0, null, plan), 1)
    const unbound = value(book.sheet(0))
    rejects(refused, unbound.bind(plan), 1)
    assert.equal(unbound.isBound, !refused)
    if (refused) {
      const early = unbound.read()
      assert.ok(!early.ok && early.error.kind === 'unbound')
    }
  }
})

test('a stale format is refused before a suggest opens anything', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'hypertabular-grouping-'))
  const missing = path.join(dir, 'no such file.csv')
  try {
    for (const [word, refused] of WORDS) {
      const format = stale(word)
      const every = { format }
      const past = { columnFormats: [[99, format]] as [number, Packed][] }
      const replaced = { format, columnFormats: [[0, INDIAN]] as [number, Packed][] }
      const twice = {
        columnFormats: [
          [0, format],
          [0, INDIAN],
        ] as [number, Packed][],
      }
      const at = (options: object) =>
        'format' in options ? 0 : (options as typeof past).columnFormats[0][0]
      for (const [data, dialect, options] of [
        ['a\n1\n', null, every],
        ['', null, every],
        ['', { hasHeader: false }, every],
        ['a\n1\n', null, past],
        ['a\n1\n', null, replaced],
        ['a\n1\n', null, twice],
      ] as const) {
        rejects(refused, DelimitedReader.suggestBytes(data, dialect, options), at(options))
      }
      // A broken source, or one not there, says so only of a format that is sound.
      for (const [data, dialect] of [
        ['a\n"1\n', null],
        ['"1\n', { hasHeader: false }],
      ] as const) {
        const found = DelimitedReader.suggestBytes(data, dialect, every)
        assert.ok(!found.ok)
        assert.equal(found.error.kind, refused ? 'plan' : 'structure')
      }
      const nowhere = DelimitedReader.suggest(missing, null, every)
      assert.ok(!nowhere.ok)
      assert.equal(nowhere.error.kind, refused ? 'plan' : 'io')
      // A sheet: the same, and one whose sample would meet its broken part.
      const sheet = value(value(Workbook.open(GROUPED[0])).sheet(0))
      for (const options of [every, past, replaced, twice])
        rejects(refused, sheet.suggest(options), at(options))
      const broken = value(value(Workbook.open(path.join(WORKBOOKS, 'broken.xlsx'))).sheet(0))
      const failed = broken.suggest(every)
      assert.ok(!failed.ok)
      assert.equal(failed.error.kind, refused ? 'plan' : 'structure')
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('an Indian shape has no culture until one is declared', () => {
  const ask = (data: string, options?: import('../index.js').SuggestOptions) =>
    value(DelimitedReader.suggestBytes(data, null, options))
  let found = ask('a\n"1,23,456"\n')
  assert.deepEqual([found.report[0].door, found.report[0].status], ['text', 'resolved'])
  found = ask('a\n"1,23,456"\n5\n')
  assert.deepEqual([found.report[0].door, found.report[0].status], ['text', 'unresolved'])
  // A suggested plan is Western: flags 95, and a later Indian shape is refused.
  found = ask('a\n"1,234,567"\n')
  assert.deepEqual(found.report[0].format?.packed, WESTERN.packed)
  assert.equal(policy(found.plan[0].format as Packed), 95)
  let batch = value(value(DelimitedReader.fromBytes('a\n"1,23,456"\n', null, found.plan)).read())
  assert.ok(batch !== null)
  assert.equal(batch.get(0, 0).ok, false)
  // Declared, the column is a number, its plan reads it, and a Western shape is what's refused.
  const declared = { columnFormats: [[0, INDIAN]] as [number, Packed][] }
  const both = 'a,b\n"1,23,456","1,23,456"\n'
  found = ask(both, declared)
  assert.deepEqual([found.report[0].door, found.report[0].status], ['i32', 'resolved'])
  assert.deepEqual(found.report[0].format?.packed, INDIAN.packed)
  assert.equal(policy(found.plan[0].format as Packed), 607)
  batch = value(value(DelimitedReader.fromBytes(both, null, found.plan)).read())
  assert.ok(batch !== null)
  assert.deepEqual(batch.get(0, 0), { ok: true, value: 123456 })
  assert.equal(found.report[1].door, 'text')
  found = ask('a\n"1,234,567"\n', declared)
  assert.deepEqual([found.report[0].door, found.report[0].status], ['text', 'resolved'])
  assert.equal(found.report[0].format, null)
})

test('a default is Western however it is reached, and neither policy reads what neither writes', () => {
  const data = 'a\n"1,234,567"\n"1,23,456"\n'
  for (const column of [
    Column.i32(0),
    Column.i32(0, WESTERN),
    Column.i32(0, stale(95)),
    Column.i32(0, null),
  ]) {
    const batch = value(value(DelimitedReader.fromBytes(data, null, [column])).read())
    assert.ok(batch !== null)
    assert.deepEqual(batch.get(0, 0), { ok: true, value: 1234567 })
    assert.equal(batch.get(0, 1).ok, false)
  }
  for (const format of [WESTERN, INDIAN]) {
    const batch = value(
      value(
        DelimitedReader.fromBytes('a\n"12,34"\n"1,2,345"\n', null, [Column.i32(0, format)]),
      ).read(),
    )
    assert.ok(batch !== null)
    assert.equal(batch.get(0, 0).ok, false)
    assert.equal(batch.get(0, 1).ok, false)
  }
})
