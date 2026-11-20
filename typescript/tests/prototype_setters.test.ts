// No call is reached by a setter a caller (or a library they loaded) put on `Object.prototype`
// under an array's index: an answer's elements are defined, never assigned, and nothing the
// binding does on the way in or out assigns into an array of the runtime's own, the native addon
// and the WebAssembly fallback alike. (`reentry.test.ts` holds the same for a field's name.)
//
// One route is left under the fallback, and it is the runtime's own (the last test): its tables
// of handles, references and scopes are arrays that grow by assignment, so the first call to
// need a slot none before it has used assigns under that slot's number.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const fs: typeof import('node:fs') = require('node:fs')
const os: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { after, test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { __napiBindingTarget } = require('../binding.js') as typeof import('../binding.js')
const { Column, DelimitedReader, Range, Workbook } = api

type Outcome<T> = import('../index.js').Outcome<T>
type ColumnT = InstanceType<typeof Column>

const WORKBOOKS = path.resolve(__dirname, '../../corpus/workbook')
const BOOK = fs.readFileSync(path.join(WORKBOOKS, 'basic.xlsx'))
const TEXT = 'n,name,when\n1,x,2026-01-02\n9,y,2026-02-03\nz,,nope\n'
const BYTES = new TextEncoder().encode(TEXT)
// Small, so that a probe of thousands of readers stays small under the WebAssembly fallback,
// whose memory never shrinks.
const SMALL = { batchRows: 8, bufferBytes: 4096 }
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'hypertabular-setters-'))
const FILE = path.join(dir, 'rows.csv')
fs.writeFileSync(FILE, TEXT)
// Made out here: `path.join` pushes, which assigns.
const ABSENT = path.join(dir, 'no-such-file.csv')
const BOOK_FILE = path.join(WORKBOOKS, 'basic.xlsx')
after(() => fs.rmSync(dir, { recursive: true, force: true }))

// HyperCast's own declarations are made once, out here: what is under test is this package.
const most = hypercast.Constraints.declare({ door: hypercast.Door.I32, maximum: '5' })
assert.ok(most.ok)
const format = hypercast.NumFormat.declare({
  decimalSep: ',',
  groupSep: '.',
  flags: hypercast.NumStyles.All,
})
assert.ok(format.ok)
const plan = (): ColumnT[] => [Column.checked(0, most.value), Column.text(1), Column.date(2)]

function value<T>(outcome: Outcome<T>): T {
  assert.ok(outcome.ok, JSON.stringify(outcome))
  return outcome.value
}

// Everything a batch answers, as plain data.
function told(batch: import('../index.js').Batch | null) {
  if (batch === null) return null
  return [
    [batch.rows, batch.columnCount, batch.doors, batch.line(0), batch.faultCount(0)],
    batch.column(1),
    batch.columnChecked(0),
    batch.faults(),
    [batch.get(2, 0), batch.getChecked(0, 1), batch.detail(0, 2), batch.origin(1, 2)],
    [Array.from(batch.raw(0, 2)), batch.rawString(1, 0)],
    [...batch].map((row) => [row.index, row.line, row.get(1), Array.from(row.raw(0))]),
  ]
}

// Every kind of call, each closing what it opened and answering plain data.
const calls: Record<string, () => unknown> = {
  'open from a string, read, close': () => {
    const reader = value(DelimitedReader.fromBytes(TEXT, null, plan(), SMALL))
    const answer = [told(value(reader.read())), value(reader.read()), reader.records]
    return [answer, reader.close(), reader.read()]
  },
  'open from bytes, read, close': () => {
    const reader = value(DelimitedReader.fromBytes(BYTES, null, plan(), SMALL))
    const second = value(DelimitedReader.fromBytes(Buffer.from(TEXT), null, plan(), SMALL))
    const answer = [told(value(reader.read())), told(value(second.read()))]
    return [answer, reader.close(), second.close()]
  },
  'open by path, read, close': () => {
    const opened = DelimitedReader.open(FILE, null, plan(), SMALL)
    // Under the WebAssembly fallback on Windows no path reaches the module: the same `io`
    // error with the setter and without.
    if (!opened.ok) return opened.error.kind
    return [told(value(opened.value.read())), opened.value.close()]
  },
  'bind, the header, iteration': () => {
    const reader = value(DelimitedReader.fromBytes(BYTES, null, null, SMALL))
    const header = reader.header
    const names = [
      header?.names,
      header?.length,
      header?.ordinal('name'),
      header?.ordinal(new TextEncoder().encode('when')),
      header?.require('n'),
      header?.require(new TextEncoder().encode('absent')),
    ]
    const bound = reader.bind(plan())
    const reads = [...reader].map((read) => (read.ok ? told(read.value) : read))
    const rows = [...value(DelimitedReader.fromBytes(TEXT, null, plan(), SMALL)).rows()].length
    return [names, bound, reader.isBound, reader.columnCount, reads, rows, reader.close()]
  },
  'a suggestion': () => {
    const text = 'a,b\n"1,234",01/02/2026\n"5,678",x\n'
    const options = { sampleRows: 4, columnDateOrders: [[1, 1]] as [number, number][] }
    const tell = (found: Outcome<import('../index.js').Suggestion>) =>
      found.ok
        ? [
            found.value.report.map((column) => ({
              ...column,
              format: column.format && Array.from(column.format.packed),
              candidates: column.candidates.map((candidate) => ({
                ...candidate,
                format: candidate.format && Array.from(candidate.format.packed),
              })),
            })),
            found.value.truncated,
            found.value.plan.map((column) => [column.door, column.ordinal, column.valid]),
          ]
        : found.error.kind
    return [
      tell(DelimitedReader.suggestBytes(text, null, options)),
      tell(DelimitedReader.suggestBytes(new TextEncoder().encode(text))),
      tell(DelimitedReader.suggest(FILE)),
    ]
  },
  'the columns a plan declares': () => {
    const columns = [
      Column.f64(0, format.value),
      Column.checked(0, most.value, format.value),
      Column.text(1).withTokens(['NA', 'n/a']),
      Column.dateOrdered(2, 1),
      Column.unix(0, 1),
      Column.i32(-1),
    ]
    const reader = value(
      DelimitedReader.fromBytes(
        'n;t\n"1.234,5";NA\n',
        { separator: ';', tokens: ['NA'] },
        [columns[0], columns[2]],
        SMALL,
      ),
    )
    const read = value(reader.read())
    return [
      columns.map((column) => [column.door, column.checked, column.ordinal, column.valid]),
      columns.map((column) => column.format && Array.from(column.format.packed)),
      read?.column(0),
      read?.column(1),
      reader.close(),
    ]
  },
  'a workbook from bytes, its sheets, a sheet read': () => {
    const book = value(Workbook.fromBytes(BOOK))
    const sheet = value(book.sheet(0, { batchRows: 8 }, [Column.text(0), Column.text(1)]))
    const read = value(sheet.read())
    const answer = [book.sheets, book.format, book.dateSystem, sheet.header?.names]
    return [answer, read?.doors, read?.column(0), read?.faults(), sheet.close(), sheet.read()]
  },
  'a workbook by path, a sheet by name, a range, a sheet suggestion': () => {
    const opened = Workbook.open(BOOK_FILE)
    const book = opened.ok ? opened.value : value(Workbook.fromBytes(BOOK))
    const name = book.sheets[0].name
    const range = value(Range.of(1, 1, 3, 2))
    const sheet = value(book.sheet(name, { batchRows: 8, range, tokens: ['NA'] }))
    const found = sheet.suggest({ sampleRows: 2 })
    const bound = sheet.bind([Column.text(0)])
    const read = value(sheet.read())
    const parsed = Range.parse('A1:B3')
    return [
      opened.ok,
      found.ok && found.value.report.map((column) => [column.name, column.door, column.status]),
      [range.firstRow, range.lastColumn, parsed.ok && parsed.value.lastRow],
      [bound, read?.rows, read?.column(0), sheet.close()],
    ]
  },
  'what is refused': () => [
    DelimitedReader.open(ABSENT),
    value(DelimitedReader.fromBytes('n\n"2\n', null, [Column.i32(0)], SMALL)).read(),
    DelimitedReader.fromBytes(TEXT, null, [Column.i32(-1)], SMALL),
    DelimitedReader.fromBytes(42 as never),
    DelimitedReader.fromBytes(TEXT, null, [{} as never]),
    value(DelimitedReader.fromBytes('a,a\n1,2\n', null, null, SMALL)).header?.require('a'),
    Workbook.fromBytes(new Uint8Array([1, 2, 3])),
    Workbook.fromBytes('text' as never),
    value(Workbook.fromBytes(BOOK)).sheet(99),
    Range.parse('nonsense'),
  ],
  'the version': () => api.nativeVersion(),
}

// Plants a setter that counts its runs and throws, under one key of `Object.prototype`, for the
// length of one call. The getter reads as an object with no such property does.
function underSetter(key: string, call: () => unknown) {
  const proto = Object.prototype as Record<string, unknown>
  // Counted, not listed: `push` assigns, and would run a setter under an index itself.
  let ran = 0
  let answer: unknown
  let thrown: unknown
  Object.defineProperty(proto, key, {
    configurable: true,
    get: () => undefined,
    set() {
      ran += 1
      throw new Error(`the setter under ${key}`)
    },
  })
  try {
    answer = call()
  } catch (error) {
    thrown = error
  } finally {
    delete proto[key]
  }
  return { ran, answer, thrown }
}

// The indices a call's own assignments could reach under the fallback: the arguments of a call
// out of the module, and the references a call lets go of. Each call has run once already, so
// every slot of the runtime's tables it needs has been used (the last test is the other case).
const INDICES = 300

test('no call assigns through a setter on Object.prototype under an index', async () => {
  const escapes = new Map<string, number>()
  for (const [name, call] of Object.entries(calls)) {
    const plain = call()
    for (let index = 0; index < INDICES; index++) {
      const { ran, answer, thrown } = underSetter(String(index), call)
      if (thrown !== undefined) {
        // Where it was assigned, the index aside: one line for each place, however many indices.
        const frames = String((thrown as Error).stack)
          .split('\n')
          .slice(2, 5)
          .map((frame) => frame.trim().replace(/^at /, ''))
        const route = `${name}: threw (${frames.join(' < ')})`
        escapes.set(route, (escapes.get(route) ?? 0) + 1)
      } else if (ran !== 0) {
        const route = `${name}: the setter ran`
        escapes.set(route, (escapes.get(route) ?? 0) + 1)
      } else {
        assert.deepEqual(answer, plain, `${name}, a setter under ${index}`)
      }
      // What the calls dropped is finalized as the event loop turns.
      if (index % 16 === 15) await new Promise(setImmediate)
    }
  }
  assert.deepEqual(
    [...escapes].map(([route, indices]) => `${route}, under ${indices} of ${INDICES} indices`),
    [],
  )
})

test('a read answers under a setter that throws under index 0, and the setter never runs', () => {
  const reader = value(DelimitedReader.fromBytes(TEXT, null, plan(), SMALL))
  const { ran, answer, thrown } = underSetter('0', () => reader.read())
  assert.equal(thrown, undefined, `${thrown}`)
  assert.equal(ran, 0)
  const read = value(answer as Outcome<import('../index.js').Batch | null>)
  assert.deepEqual(read?.column(1), [
    { ok: true, value: 'x' },
    { ok: true, value: 'y' },
    { ok: false, fault: { reason: 'empty', offset: 0, length: 0 } },
  ])
  assert.deepEqual(reader.close(), { ok: true, value: null })
})

// The recorded loss, which is the WebAssembly runtime's and no call's: a value the module makes
// is given a handle, the next slot of an array the runtime grows by assignment (as its tables of
// references and of scopes grow), so a setter under the number of a slot no call has filled yet
// runs, and one that throws stops the call. The module stays sound and the next call answers.
// The native addon has no such table.
test('a slot the fallback has never used is assigned under its number (the recorded loss)', () => {
  const ROWS = 70_000
  const SLOT = '60000'
  const reader = value(
    DelimitedReader.fromBytes(`n\n${'1\n'.repeat(ROWS)}`, null, [Column.i32(0)], {
      batchRows: ROWS,
    }),
  )
  const read = value(reader.read())
  assert.ok(read !== null)
  // The first call in this process to hold more than 60,000 values at once.
  const { ran, answer, thrown } = underSetter(SLOT, () => read.column(0).length)
  if (__napiBindingTarget === 'native') {
    assert.deepEqual([ran, answer, thrown], [0, ROWS, undefined])
  } else {
    assert.equal(ran, 1)
    assert.ok(thrown instanceof Error && thrown.message === `the setter under ${SLOT}`)
  }
  // Sound: the same call answers once the setter is gone. And with the slot filled once, it
  // is the array's own, which no setter is asked about again.
  assert.equal(read.column(0).length, ROWS)
  assert.deepEqual(
    underSetter(SLOT, () => read.column(0).length),
    { ran: 0, answer: ROWS, thrown: undefined },
  )
  assert.deepEqual(reader.close(), { ok: true, value: null })
})
