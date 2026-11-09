// JavaScript can run in the middle of a reader's call: a getter or a Proxy trap in an argument
// it reads (a plan's element, an option), or a setter a caller put on `Object.prototype` that
// the answer it builds assigns through. A call back into the same reader or sheet from there is
// refused with the `busy` error: a method answers it, a getter, with no outcome to answer in,
// throws it. The outer call goes on unharmed. Nothing here reaches state the outer call is using.
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
const pathsUnreachable =
  (require('../binding.js') as { __napiBindingTarget: string }).__napiBindingTarget !== 'native' &&
  process.platform === 'win32'
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

test('a call back into a reader from what its read assigns through is refused, and the read goes on', () => {
  // A structural failure's error carries optional fields, which JavaScript assigns: a setter on
  // Object.prototype runs mid-read, inside the reader's call.
  const reader = value(DelimitedReader.fromBytes('n\n1\n"2\n', null, [Column.i32(0)]))
  const answers: Outcome<unknown>[] = []
  Object.defineProperty(Object.prototype, 'failure', {
    configurable: true,
    set() {
      answers.push(reader.read(), reader.close(), reader.bind([Column.i32(0)]))
    },
  })
  let first: Outcome<unknown>
  let failed: Outcome<unknown>
  try {
    first = reader.read()
    failed = reader.read()
  } finally {
    delete (Object.prototype as { failure?: unknown }).failure
  }
  assert.ok(first.ok)
  assert.ok(!failed.ok && failed.error.kind === 'structure')
  assert.ok(answers.length >= 3)
  for (const answer of answers) busy(answer, 'reader')
  // The reader is where the read left it: failed, and open.
  const again = reader.read()
  assert.ok(!again.ok && again.error.kind === 'structure')
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
