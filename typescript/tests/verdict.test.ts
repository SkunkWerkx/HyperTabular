// A cell's verdict and a reader's outcome as a TypeScript caller holds them. `npm run
// typecheck` compiles this file, so the `@ts-expect-error` lines are assertions too: each must
// be an error, or tsc fails with "Unused '@ts-expect-error' directive". Narrowing on `ok` is
// what makes `value`, `fault` and `error` reachable; a cell's verdict is @skunkwerkx/hypercast's
// own type, both ways; and switches over a fault's reason and an error's kind are exhaustive.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader } = api
type Verdict<T> = import('../index.js').Verdict<T>
type Fault = import('../index.js').Fault
type CastFailure = import('../index.js').CastFailure
type TabularError = import('../index.js').TabularError
type Outcome<T> = import('../index.js').Outcome<T>
type Batch = import('../index.js').Batch
type Value = import('../index.js').Value

// Exhaustive by construction: a fourth reason would make `reason` not `never` here.
function describe(fault: Fault): string {
  switch (fault.reason) {
    case 'empty':
      return 'absent'
    case 'malformed':
      return `malformed at byte ${fault.offset}`
    case 'outOfRange':
      return `out of range at byte ${fault.offset}`
    default: {
      const unreachable: never = fault.reason
      return unreachable
    }
  }
}

// And over every kind of error a reader can stop with.
function explain(error: TabularError): string {
  switch (error.kind) {
    case 'structure':
      return `${error.failure?.kind} at line ${error.failure?.line}`
    case 'plan':
      return `plan column ${error.column}`
    case 'io':
    case 'separator':
    case 'noSheet':
    case 'noColumn':
    case 'unbound':
    case 'alreadyBound':
    case 'argument':
    case 'closed':
      return error.message
    default: {
      const unreachable: never = error.kind
      return unreachable
    }
  }
}

function first(text: string): Batch {
  const opened = DelimitedReader.fromBytes(text, null, [Column.i32(0), Column.i64(1)])
  assert.ok(opened.ok)
  const read: Outcome<Batch | null> = opened.value.read()
  assert.ok(read.ok && read.value !== null)
  return read.value
}

test('narrowing on ok reaches the value or the fault, and nothing else', () => {
  const batch = first('a,b\n"(1,234)",9007199254740993\n12x,\n')
  const verdict = batch.get<number>(0, 0)
  // @ts-expect-error: value exists only once ok is known to be true
  assert.equal(verdict.value, -1234)
  // @ts-expect-error: fault exists only once ok is known to be false
  assert.equal(verdict.fault, undefined)
  if (verdict.ok) {
    const value: number = verdict.value
    assert.equal(value, -1234)
    // @ts-expect-error: a success carries no fault
    assert.equal(verdict.fault, undefined)
  } else {
    assert.fail(describe(verdict.fault))
  }
  const failed = batch.get(0, 1)
  if (!failed.ok) {
    const reason: CastFailure = failed.fault.reason
    assert.equal(reason, 'malformed')
    assert.equal(describe(failed.fault), 'malformed at byte 2')
    // @ts-expect-error: a failure carries no value
    assert.equal(failed.value, undefined)
  } else {
    assert.fail('12x cast')
  }
  // Unparameterised, a value is the union of what the doors make.
  const any = batch.get(1, 0)
  if (any.ok) {
    const value: Value = any.value
    // @ts-expect-error: the union is not narrowed to one door's type
    const wide: bigint = any.value
    assert.equal(value, wide)
    assert.equal(value, 9007199254740993n)
  }
})

test("a cell's verdict is @skunkwerkx/hypercast's own Verdict, both ways", () => {
  const batch = first('a,b\n7,8\n')
  const ours: Verdict<number> = batch.get<number>(0, 0)
  const theirs: import('@skunkwerkx/hypercast').Verdict<number> = ours
  const back: Verdict<number> = hypercast.castI32('7')
  assert.deepEqual(theirs, back)
  const fromRow = batch.row(0)?.get<bigint>(1)
  const hc: import('@skunkwerkx/hypercast').Verdict<bigint> | undefined = fromRow
  assert.deepEqual(hc, { ok: true, value: 8n })
})

test('an outcome narrows to its value or its error', () => {
  const opened = DelimitedReader.fromBytes('a\n1\n2\n', { separator: '"' })
  // @ts-expect-error: value exists only once ok is known to be true
  assert.equal(opened.value, undefined)
  if (opened.ok) {
    assert.fail('a quote is no separator')
  } else {
    // @ts-expect-error: a refusal carries no value
    assert.equal(opened.value, undefined)
    assert.equal(opened.error.kind, 'separator')
    assert.match(explain(opened.error), /separator byte 0x22/)
  }
  const broken = DelimitedReader.fromBytes('a,b\n1,2\n3\n', null, [Column.i32(0)])
  assert.ok(broken.ok)
  const reasons: string[] = []
  for (const read of broken.value) {
    if (!read.ok) {
      reasons.push(explain(read.error))
      break
    }
    reasons.push(`${read.value.rows} rows`)
  }
  assert.deepEqual(reasons, ['1 rows', 'columnCount at line 3'])
})

test('the kinds and reasons are exactly the strings the types name', () => {
  // @ts-expect-error: 'out_of_range' is the corpus's spelling, not the binding's
  const wrong: CastFailure = 'out_of_range'
  assert.equal(wrong, 'out_of_range')
  // @ts-expect-error: 'column_count' is the corpus's spelling, not the binding's
  const kind: import('../index.js').FailureKind = 'column_count'
  assert.equal(kind, 'column_count')
})
