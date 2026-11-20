// JavaScript can run in the middle of a reader's call: a getter or a Proxy trap in an argument
// it reads (a plan's element, an option). A call back into the same reader or sheet from there
// is refused with the `busy` error: a method answers it, a getter, with no outcome to answer in,
// throws it. The outer call goes on unharmed. Nothing here reaches state the outer call is using.
// The answer a call builds is no such route: its properties are defined, never assigned.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { readFileSync }: typeof import('node:fs') = require('node:fs')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Workbook } = api

type Outcome<T> = import('../index.js').Outcome<T>

// Under the WASI fallback on Windows no path reaches the module (README): there a path's open
// is the I/O error it is, and the file's own bytes stand in for it. What is judged before the
// file is touched is still the path's answer.
const wasi =
  (require('../binding.js') as { __napiBindingTarget: string }).__napiBindingTarget !== 'native'
const pathsUnreachable = wasi && process.platform === 'win32'
function byPath<T>(
  opened: Outcome<T>,
  file: string,
  bytes: (data: Uint8Array) => Outcome<T>,
): Outcome<T> {
  if (!pathsUnreachable || opened.ok || opened.error.kind !== 'io') return opened
  return bytes(readFileSync(file))
}
const openBook = (file: string) =>
  byPath(Workbook.open(file), file, (data) => Workbook.fromBytes(data))
type ColumnT = InstanceType<typeof Column>

const WORKBOOKS = path.resolve(__dirname, '../../corpus/workbook')
const TEXT = 'n\n1\n3\n5\n'

function value<T>(outcome: Outcome<T>): T {
  assert.ok(outcome.ok, JSON.stringify(outcome))
  return outcome.value
}

function busy(outcome: Outcome<unknown>, what: 'reader' | 'sheet') {
  assert.ok(!outcome.ok, `taken mid-call: ${JSON.stringify(outcome)}`)
  assert.equal(outcome.error.kind, 'busy')
  assert.equal(
    outcome.error.message,
    `the ${what} is in the middle of a call; nothing that call runs can call back into it`,
  )
}

// What a getter throws mid-call: the `busy` error, as an `Error` with its code and kind.
function throwsBusy(get: () => unknown, what: 'reader' | 'sheet') {
  assert.throws(get, (error: unknown) => {
    assert.ok(api.isHyperTabularError(error))
    assert.equal(error.code, 'ERR_HYPERTABULAR_BUSY')
    assert.equal((error as Error & { kind: string }).kind, 'busy')
    assert.ok(Object.hasOwn(error, 'code') && Object.hasOwn(error, 'kind'))
    assert.equal(
      error.message,
      `the ${what} is in the middle of a call; nothing that call runs can call back into it`,
    )
    return true
  })
}

// A plan whose one element is read through a getter that first runs `inner`.
function planCalling(inner: () => void, column: ColumnT = Column.i32(0)): ColumnT[] {
  const plan: ColumnT[] = []
  Object.defineProperty(plan, 0, {
    enumerable: true,
    get: () => {
      inner()
      return column
    },
  })
  return plan
}

function values(reader: { read(): Outcome<import('../index.js').Batch | null> }) {
  const seen: unknown[] = []
  for (;;) {
    const read = value(reader.read())
    if (read === null) return seen
    seen.push(...read.column(0).map((cell) => (cell.ok ? cell.value : cell)))
  }
}

test('a call back into a reader from a plan it is binding is refused, and the bind goes on', () => {
  const calls: [string, (reader: InstanceType<typeof DelimitedReader>) => void][] = [
    ['read', (reader) => busy(reader.read(), 'reader')],
    ['close', (reader) => busy(reader.close(), 'reader')],
    ['bind', (reader) => busy(reader.bind([Column.i32(0)]), 'reader')],
    [
      'getters',
      (reader) => {
        throwsBusy(() => reader.isBound, 'reader')
        throwsBusy(() => reader.header, 'reader')
        throwsBusy(() => reader.columnCount, 'reader')
        throwsBusy(() => reader.records, 'reader')
      },
    ],
    [
      'iteration',
      (reader) => {
        const [first] = [...reader]
        busy(first, 'reader')
      },
    ],
    [
      'another reader',
      () =>
        assert.deepEqual(
          values(value(DelimitedReader.fromBytes('n\n7\n', null, [Column.i32(0)]))),
          [7],
        ),
    ],
  ]
  for (const [name, call] of calls) {
    const reader = value(DelimitedReader.fromBytes(TEXT))
    let ran = false
    const bound = reader.bind(
      planCalling(() => {
        call(reader)
        ran = true
      }),
    )
    assert.ok(ran, name)
    assert.deepEqual(bound, { ok: true, value: null }, name)
    assert.equal(reader.isBound, true, name)
    assert.deepEqual(values(reader), [1, 3, 5], name)
    assert.equal(reader.header?.names[0], 'n', name)
  }
})

test('a call back into a sheet from a plan it is binding is refused, and the bind goes on', () => {
  const book = value(openBook(path.join(WORKBOOKS, 'basic.xlsx')))
  for (const inner of [
    (sheet: InstanceType<typeof api.Sheet>) => busy(sheet.read(), 'sheet'),
    (sheet: InstanceType<typeof api.Sheet>) => busy(sheet.close(), 'sheet'),
    (sheet: InstanceType<typeof api.Sheet>) => busy(sheet.suggest(), 'sheet'),
    (sheet: InstanceType<typeof api.Sheet>) => throwsBusy(() => sheet.isBound, 'sheet'),
    (sheet: InstanceType<typeof api.Sheet>) => throwsBusy(() => sheet.header, 'sheet'),
  ]) {
    const sheet = value(book.sheet(0))
    const header = sheet.header?.names
    assert.deepEqual(sheet.bind(planCalling(() => inner(sheet), Column.text(0))), {
      ok: true,
      value: null,
    })
    assert.equal(sheet.isBound, true)
    assert.deepEqual(sheet.header?.names, header)
    assert.ok(value(sheet.read()) !== null)
  }
})

// An answer is built of properties and elements that are defined, never assigned: a setter a
// caller put on `Object.prototype`, under a field's name or an array's index, does not run, so it
// can neither call back in, nor throw out of a call, nor keep a field off the answer.
test('no answer is assigned through a setter on Object.prototype', () => {
  const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
  const most = hypercast.Constraints.declare({ door: hypercast.Door.I32, maximum: '5' })
  assert.ok(most.ok)
  const checked = Column.checked(0, most.value)
  const book = value(openBook(path.join(WORKBOOKS, 'basic.xlsx')))
  const absent = path.join(WORKBOOKS, 'no-such-file.csv')
  const batch = (text: string, plan: ColumnT[]) => {
    const read = value(value(DelimitedReader.fromBytes(text, null, plan)).read())
    assert.ok(read !== null)
    return read
  }
  const calls: Record<string, () => unknown> = {
    'an io error': () => DelimitedReader.open(absent),
    'a structural failure': () =>
      value(DelimitedReader.fromBytes('n\n"2\n', null, [Column.i32(0)])).read(),
    'a plan error': () => value(DelimitedReader.fromBytes(TEXT)).bind([Column.i32(-1)]),
    'an ambiguous column': () =>
      value(DelimitedReader.fromBytes('a,a\n1,2\n')).header?.require('a'),
    'a range error': () => api.Range.parse('nonsense'),
    'a header': () => value(DelimitedReader.fromBytes('a,b,c\n1,2,3\n')).header?.names,
    'a read, its doors and its rows': () => {
      const read = batch('a,b\n1,x\n2,y\n', [Column.i32(0), Column.text(1)])
      return [read.doors, read.rows, [...read].map((row) => row.get(1))]
    },
    'a fault': () => batch('n\nx\n', [Column.i32(0)]).get(0, 0),
    'a date': () => batch('d\n2026-01-02\n', [Column.date(0)]).get(0, 0),
    'a column': () => batch('n\n1\nx\n3\n', [Column.i32(0)]).column(0),
    'a checked column': () => batch('n\n1\n9\nx\n', [checked]).columnChecked(0),
    'a violation': () => batch('n\n9\n', [checked]).getChecked(0, 0),
    'the faults': () => batch('n\nx\n9\n', [checked]).faults(),
    'a suggestion': () => {
      const found = value(DelimitedReader.suggestBytes('a,b\n"1,234",01/02/2026\n"5,678",x\n'))
      return [found.report, found.truncated, found.plan.map((column) => column.door)]
    },
    'the sheets': () => book.sheets,
    'a sheet read': () => {
      const sheet = value(book.sheet(0, null, [Column.text(0), Column.text(1)]))
      const read = value(sheet.read())
      assert.ok(read !== null)
      return [read.doors, read.column(0), read.faults()]
    },
  }
  const keys = [
    ...['ok', 'value', 'error', 'kind', 'message', 'failure', 'record', 'line', 'byte'],
    ...['expected', 'found', 'column', 'first', 'second', 'fault', 'reason', 'offset', 'length'],
    ...['violation', 'keyword', 'adjusted', 'enumIndex', 'name', 'hidden', 'row', 'planColumn'],
    ...['sourceColumn', 'origin', 'detail', 'plan', 'report', 'truncated', 'ordinal', 'door'],
    ...['declared', 'format', 'candidates', 'status', 'matched', 'sampled', 'order', 'packed'],
    ...['year', 'month', 'day', '0', '1', '2'],
  ]
  const proto = Object.prototype as Record<string, unknown>
  for (const [name, call] of Object.entries(calls)) {
    const plain = call()
    for (const key of keys) {
      // Counted, not listed: `push` assigns, and would run the setter under an index itself.
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
      assert.equal(thrown, undefined, `${name}, a setter under ${key}: ${thrown}`)
      assert.equal(ran, 0, `${name}: the setter under ${key} ran`)
      assert.deepEqual(answer, plain, `${name}, a setter under ${key}`)
    }
  }
})

// An array is made by `Array.of`, which defines its elements, up to what one call takes, and an
// element at a time past that or where `Array.of` is no longer the engine's.
test('a whole column is whole past one call of Array.of, and without it', () => {
  const ROWS = 9000
  let text = 'n\n'
  for (let row = 0; row < ROWS; row++) text += `${row}\n`
  const expected = Array.from({ length: ROWS }, (_, row) => ({ ok: true, value: row }))
  const column = () => {
    const reader = value(
      DelimitedReader.fromBytes(text, null, [Column.i32(0)], { batchRows: ROWS }),
    )
    const read = value(reader.read())
    assert.ok(read !== null)
    return [read.column(0), read.columnChecked(0).length, read.doors]
  }
  assert.deepEqual(column(), [expected, ROWS, ['i32']])
  const of = Array.of
  for (const stand of [
    undefined,
    () => 'not an array',
    () => {
      throw new Error('of')
    },
  ]) {
    let made: unknown
    try {
      ;(Array as { of: unknown }).of = stand
      made = column()
    } finally {
      Array.of = of
    }
    assert.deepEqual(made, [expected, ROWS, ['i32']])
  }
})

// The WebAssembly fallback's runtime assigns the arguments of a call it makes out of the module
// into an array of its own, and a reference it is done with into a list: so a read hands its
// batch over without a call out, and a `Uint8Array` is read without a reference
// (`prototype_setters.test.ts` has every call, under 300 indices).
test('a setter under an index neither runs nor stops a read, the fallback included', () => {
  const reader = value(DelimitedReader.fromBytes(TEXT, null, [Column.i32(0)]))
  const proto = Object.prototype as Record<string, unknown>
  let ran = 0
  Object.defineProperty(proto, '0', {
    configurable: true,
    get: () => undefined,
    set() {
      ran += 1
      throw new Error('the setter under 0')
    },
  })
  let read: Outcome<import('../index.js').Batch | null>
  let column: unknown
  try {
    read = reader.read()
    column = read.ok ? read.value?.column(0) : undefined
  } finally {
    delete proto['0']
  }
  assert.equal(ran, 0)
  assert.ok(read.ok, JSON.stringify(read))
  assert.deepEqual(column, [
    { ok: true, value: 1 },
    { ok: true, value: 3 },
    { ok: true, value: 5 },
  ])
  assert.deepEqual(reader.close(), { ok: true, value: null })
})

test('a call back into a sheet from the options of its own suggestion is refused', () => {
  const book = value(openBook(path.join(WORKBOOKS, 'basic.xlsx')))
  const sheet = value(book.sheet(0))
  const answers: Outcome<unknown>[] = []
  const options = {
    get sampleRows() {
      answers.push(sheet.close(), sheet.read(), sheet.suggest(), sheet.bind([Column.text(0)]))
      return 10
    },
  }
  const found = sheet.suggest(options) as Outcome<unknown>
  assert.ok(found.ok)
  assert.equal(answers.length, 4)
  const [close, read, suggestion, bind] = answers
  for (const answer of [close, read, bind]) busy(answer, 'sheet')
  // Another suggestion only looks at the sheet, as this one does, and is taken.
  assert.ok(suggestion.ok)
  assert.deepEqual(
    (suggestion.value as import('../index.js').Suggestion).report,
    (found.value as import('../index.js').Suggestion).report,
  )
  // Still open, and still unbound.
  assert.equal(sheet.isBound, false)
  assert.deepEqual(sheet.bind([Column.text(0)]), { ok: true, value: null })
  assert.ok(value(sheet.read()) !== null)
})

test('a getter that throws mid-call is cleared, and the reader is free again', () => {
  const reader = value(DelimitedReader.fromBytes(TEXT))
  const plan: ColumnT[] = []
  Object.defineProperty(plan, 0, {
    get() {
      throw new Error('from the plan')
    },
  })
  const refused = reader.bind(plan)
  assert.ok(!refused.ok && refused.error.kind === 'plan' && refused.error.column === 0)
  assert.deepEqual(reader.bind([Column.i32(0)]), { ok: true, value: null })
  assert.deepEqual(values(reader), [1, 3, 5])
})

test("the busy error's code and kind are its own, defined without a setter", () => {
  const seen: string[] = []
  const proto = Object.prototype as Record<string, unknown>
  for (const name of ['code', 'kind']) {
    Object.defineProperty(proto, name, {
      configurable: true,
      get: () => undefined,
      set: () => {
        seen.push(name)
      },
    })
  }
  try {
    const reader = value(DelimitedReader.fromBytes(TEXT))
    reader.bind(planCalling(() => throwsBusy(() => reader.isBound, 'reader')))
  } finally {
    delete proto.code
    delete proto.kind
  }
  assert.deepEqual(seen, [])
})
