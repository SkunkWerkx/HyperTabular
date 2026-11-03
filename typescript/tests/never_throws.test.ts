// The binding's promise: nothing throws for data or for an argument of the wrong type. Every
// factory, every opener, every reader, batch, row and header method is called with every value
// below in each position, and each call must return what it promises: a column, a well-formed
// outcome (`ok` true with a value, or `ok` false with an error whose kind is one of nine and
// whose message is a string), or a well-formed verdict (HyperCast's: a reason of three and a
// span of two whole numbers inside the cell's raw text).
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { readFileSync }: typeof import('node:fs') = require('node:fs')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Workbook } = api

type Loose = (...args: unknown[]) => unknown

function detached(): Uint8Array {
  const view = new Uint8Array(8)
  structuredClone(view.buffer, { transfer: [view.buffer] })
  return view
}

const book = readFileSync(path.resolve(__dirname, '../../corpus/workbook/basic.xlsx'))

// Everything a JavaScript caller could pass, the types' contract broken every way it can be.
const hostile: [string, unknown][] = [
  ['undefined', undefined],
  ['null', null],
  ['true', true],
  ['0', 0],
  ['1', 1],
  ['-1', -1],
  ['1.5', 1.5],
  ['NaN', Number.NaN],
  ['Infinity', Number.POSITIVE_INFINITY],
  ['-0', -0],
  ['2^20', 2 ** 20],
  ['2^32', 2 ** 32],
  ['1n', 1n],
  ['symbol', Symbol('x')],
  ['empty string', ''],
  ['a separator', ';'],
  ['digits', '42'],
  ['lone surrogate', '\ud800x'],
  ['NUL', '\u0000'],
  ['csv', 'a,b\n1,2\n3\n'],
  ['Uint8Array', new Uint8Array([0x34, 0x32])],
  ['invalid UTF-8', new Uint8Array([0xff, 0xfe, 0x31])],
  ['Buffer', Buffer.from('a\n7\n')],
  ['a workbook', new Uint8Array(book)],
  ['empty Uint8Array', new Uint8Array(0)],
  ['detached Uint8Array', detached()],
  ['Uint16Array', new Uint16Array([0x31])],
  ['DataView', new DataView(new ArrayBuffer(2))],
  ['ArrayBuffer', new ArrayBuffer(2)],
  ['array', [0x31]],
  ['array of junk', [1, 'x', null]],
  ['empty plan', []],
  ['a plan', [Column.i32(0)]],
  ['a plan with a bad column', [Column.i32(-1)]],
  ['plain object', { separator: ',', batchRows: 1, packed: new Uint8Array(92) }],
  ['dialect with a bad flag', { separator: ',', quoting: 'yes' }],
  ['options with a bad size', { batchRows: 0, bufferBytes: -1 }],
  ['Date', new Date(0)],
  ['function', () => 1],
  ['String object', new String('1')],
  [
    'Proxy whose get trap throws',
    new Proxy(
      {},
      {
        get() {
          throw new Error('trap')
        },
      },
    ),
  ],
  [
    'getter that throws',
    Object.defineProperty({}, 'separator', {
      get: () => {
        throw new Error('getter')
      },
    }),
  ],
  ['NumFormat', hypercast.NumFormat.invariant()],
  ['a Column', Column.text(0)],
  ['Column.prototype', Column.prototype],
  ['Object.create(Column.prototype)', Object.create(Column.prototype)],
  ['ExcelEpoch member', hypercast.ExcelEpoch.Y1904],
]

// A Proxy whose [[GetPrototypeOf]] throws: natively nothing here reaches the prototype, so
// these join the hostile arguments; under the WASI build they throw, the recorded loss
// @skunkwerkx/hypercast documents (emnapi's handle lookup tests every value `instanceof
// WeakRef`, which runs the trap, before any Rust runs).
const revoked = Proxy.revocable({}, {})
revoked.revoke()
const prototypeThrowing: [string, unknown][] = [
  [
    'Proxy whose getPrototypeOf trap throws',
    new Proxy(
      {},
      {
        getPrototypeOf() {
          throw new Error('trap')
        },
      },
    ),
  ],
  ['revoked Proxy', revoked.proxy],
]
const wasi =
  (require('../binding.js') as { __napiBindingTarget: string }).__napiBindingTarget !== 'native'
if (!wasi) hostile.push(...prototypeThrowing)

const KINDS = [
  'io',
  'separator',
  'plan',
  'noSheet',
  'noColumn',
  'unbound',
  'alreadyBound',
  'structure',
  'argument',
  'closed',
]

function outcome(ctx: string, result: unknown): { ok: boolean; value?: unknown } {
  assert.equal(typeof result, 'object', ctx)
  const o = result as { ok: unknown; value?: unknown; error?: { kind: unknown; message: unknown } }
  assert.equal(typeof o.ok, 'boolean', ctx)
  if (o.ok) {
    assert.ok('value' in o && !('error' in o), ctx)
  } else {
    assert.ok(!('value' in o), ctx)
    assert.ok(KINDS.includes(o.error?.kind as string), `${ctx}: ${String(o.error?.kind)}`)
    assert.equal(typeof o.error?.message, 'string', ctx)
  }
  return o as { ok: boolean; value?: unknown }
}

function verdict(ctx: string, result: unknown, raw?: Uint8Array): void {
  assert.equal(typeof result, 'object', ctx)
  const v = result as { ok: unknown; value?: unknown; fault?: Record<string, unknown> }
  assert.equal(typeof v.ok, 'boolean', ctx)
  if (v.ok) {
    assert.ok('value' in v && !('fault' in v), ctx)
    return
  }
  const fault = v.fault as { reason: string; offset: number; length: number }
  assert.ok(['empty', 'malformed', 'outOfRange'].includes(fault.reason), ctx)
  assert.ok(Number.isInteger(fault.offset) && fault.offset >= 0, ctx)
  assert.ok(Number.isInteger(fault.length) && fault.length >= 0, ctx)
  if (raw !== undefined) assert.ok(fault.offset + fault.length <= raw.length, ctx)
}

// Calls `f` and hands back what it returned, failing the test if it threw.
function call(ctx: string, f: () => unknown): unknown {
  let result: unknown
  assert.doesNotThrow(() => {
    result = f()
  }, `${ctx} threw`)
  return result
}

const factories = [
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
] as const

test('no Column factory throws, and a column it cannot honour is refused at bind', () => {
  let calls = 0
  for (const name of factories) {
    const factory = Column[name] as unknown as Loose
    for (const [first, a] of hostile) {
      for (const [second, b] of hostile) {
        const ctx = `Column.${name}(${first}, ${second})`
        const column = call(ctx, () => factory(a, b)) as InstanceType<typeof Column>
        assert.ok(column instanceof Column, ctx)
        assert.equal(column.door, name, ctx)
        calls++
        if (!column.valid) {
          const bound = DelimitedReader.fromBytes('a\n1\n', null, [column])
          assert.ok(!bound.ok && bound.error.kind === 'plan' && bound.error.column === 0, ctx)
        }
      }
    }
  }
  assert.ok(calls >= factories.length * hostile.length * hostile.length)
})

test('no opener throws, for any argument in any position', () => {
  const fromBytes = DelimitedReader.fromBytes as unknown as Loose
  const open = DelimitedReader.open as unknown as Loose
  const valid: unknown[] = ['a,b\n1,2\n', null, [Column.i32(0)], null]
  for (const [name, value] of hostile) {
    for (let position = 0; position < 4; position++) {
      const args = [...valid]
      args[position] = value
      outcome(
        `fromBytes, ${name} at ${position}`,
        call(name, () => fromBytes(...args)),
      )
      args[0] = position === 0 ? value : path.join(__dirname, 'no such file')
      outcome(
        `open, ${name} at ${position}`,
        call(name, () => open(...args)),
      )
    }
    for (const [other, second] of hostile) {
      outcome(
        `fromBytes(${name}, ${other})`,
        call(name, () => fromBytes(value, second)),
      )
    }
    outcome(
      `Workbook.fromBytes(${name})`,
      call(name, () => (Workbook.fromBytes as Loose)(value)),
    )
    outcome(
      `Workbook.open(${name})`,
      call(name, () => (Workbook.open as Loose)(value)),
    )
  }
})

test('no reader, sheet, batch, row or header method throws', () => {
  const opened = Workbook.fromBytes(new Uint8Array(book))
  assert.ok(opened.ok)
  for (const [name, value] of hostile) {
    for (let position = 0; position < 3; position++) {
      const args: unknown[] = [0, null, null]
      args[position] = value
      const sheet = outcome(
        `sheet, ${name} at ${position}`,
        call(name, () => (opened.value.sheet as Loose).apply(opened.value, args)),
      )
      if (sheet.ok) {
        const s = sheet.value as InstanceType<typeof api.Sheet>
        outcome(
          `sheet.bind(${name})`,
          call(name, () => (s.bind as Loose).call(s, value)),
        )
        outcome(
          `sheet.read after bind(${name})`,
          call(name, () => s.read()),
        )
      }
    }
    const reader = DelimitedReader.fromBytes('id,name\n1,x\n2,y\n')
    assert.ok(reader.ok)
    const r = reader.value
    outcome(
      `bind(${name})`,
      call(name, () => (r.bind as Loose).call(r, value)),
    )
    const header = r.header as InstanceType<typeof api.Header>
    assert.ok(
      ['number', 'object'].includes(typeof call(name, () => (header.ordinal as Loose)(value))),
    )
    outcome(
      `require(${name})`,
      call(name, () => (header.require as Loose).call(header, value)),
    )
  }

  // Closed, every method still answers: reads and binds with `closed`, getters with nothing.
  const closedReader = DelimitedReader.fromBytes('id\n1\n')
  const closedSheet = opened.value.sheet(0)
  assert.ok(closedReader.ok && closedSheet.ok)
  for (const closed of [closedReader.value, closedSheet.value]) {
    closed.close()
    closed.close()
    for (const result of [closed.read(), closed.bind([Column.i32(0)])]) {
      assert.ok(!result.ok && result.error.kind === 'closed', JSON.stringify(result))
    }
    assert.equal(closed.header, null)
    assert.equal(closed.isBound, false)
  }
  assert.equal(closedReader.value.records, null)
  assert.equal(closedReader.value.columnCount, null)

  const read = DelimitedReader.fromBytes('id,n\n1,x\n2,\n', null, [Column.i32(0), Column.u8(1)])
  assert.ok(read.ok)
  const batch = read.value.read()
  assert.ok(batch.ok && batch.value !== null)
  const b = batch.value
  const row = b.row(0)
  assert.ok(row !== null)
  for (const [first, a] of hostile) {
    verdict(
      `row.get(${first})`,
      call(first, () => (row.get as Loose).call(row, a)),
    )
    assert.ok(call(first, () => (row.raw as Loose).call(row, a)) instanceof Uint8Array)
    assert.equal(typeof call(first, () => (row.rawString as Loose).call(row, a)), 'string')
    assert.equal(typeof call(first, () => (b.line as Loose).call(b, a)), 'number')
    assert.ok(Array.isArray(call(first, () => (b.column as Loose).call(b, a))))
    assert.equal(typeof call(first, () => (b.faultCount as Loose).call(b, a)), 'number')
    call(first, () => (b.row as Loose).call(b, a))
    for (const [second, c] of hostile) {
      const ctx = `batch.get(${first}, ${second})`
      const raw = call(ctx, () => (b.raw as Loose).call(b, a, c)) as Uint8Array
      verdict(
        ctx,
        call(ctx, () => (b.get as Loose).call(b, a, c)),
        raw,
      )
      assert.equal(typeof call(ctx, () => (b.rawString as Loose).call(b, a, c)), 'string')
    }
  }
})

test('a cell asked for where the batch has none is the contract violation', () => {
  const contract = { ok: false, fault: { reason: 'malformed', offset: 0, length: 0 } }
  const read = DelimitedReader.fromBytes('n\n1\n', null, [Column.i32(0)])
  assert.ok(read.ok)
  const batch = read.value.read()
  assert.ok(batch.ok && batch.value !== null)
  const b = batch.value
  assert.deepEqual(b.get(0, 0), { ok: true, value: 1 })
  assert.deepEqual(b.get(1, 0), contract)
  assert.deepEqual(b.get(0, 1), contract)
  assert.deepEqual(b.get(-1, 0), contract)
  assert.deepEqual(b.get(0.5, 0), contract)
  assert.deepEqual(b.raw(0, 1), new Uint8Array(0))
  assert.equal(b.line(9), 0)
  assert.deepEqual(b.column(3), [])
  assert.equal(b.row(1), null)
})

test('under WASI, a Proxy whose prototype lookup throws still throws (the recorded loss)', {
  skip: !wasi && 'native: these are among the hostile arguments above',
}, () => {
  for (const [name, proxy] of prototypeThrowing) {
    assert.throws(() => DelimitedReader.fromBytes(proxy as string), Error, name)
    // The module is still sound after the exception unwound through it.
    const read = DelimitedReader.fromBytes('n\n1\n', null, [Column.i32(0)])
    assert.ok(read.ok)
  }
})
