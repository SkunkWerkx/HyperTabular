// A column that declares HyperCast's constraints: what the corpus does not reach of the
// package's surface. `Column.checked` takes the declaration's door as its own, so a column can't
// pair one door with another's declaration; what it can be handed is something that is no
// declaration, which makes a column that is not `valid`, refused when its plan is bound. `bun run
// typecheck` compiles this file, so the narrowing below is checked too.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Workbook } = api
type Batch = import('../index.js').Batch
type CheckedVerdict<T> = import('../index.js').CheckedVerdict<T>
type ColumnT = InstanceType<typeof Column>
type Constraints = InstanceType<typeof hypercast.Constraints>
type ConstraintsSpec = import('@skunkwerkx/hypercast').ConstraintsSpec

const headerless = { hasHeader: false }
const contract = { ok: false, fault: { reason: 'malformed', offset: 0, length: 0 } }

function declare(spec: ConstraintsSpec): Constraints {
  const declared = hypercast.Constraints.declare(spec)
  assert.ok(declared.ok, JSON.stringify(declared))
  return declared.value
}

const atMost = (maximum: string) => declare({ door: hypercast.Door.I32, maximum })

function read(text: string, plan: ColumnT[], options?: { batchRows: number }): Batch {
  const opened = DelimitedReader.fromBytes(text, headerless, plan, options)
  assert.ok(opened.ok, JSON.stringify(opened))
  const batch = opened.value.read()
  assert.ok(batch.ok && batch.value !== null, JSON.stringify(batch))
  return batch.value
}

// A violation's keyword, or why the cell didn't pass otherwise: the narrowing a caller writes.
function said(verdict: CheckedVerdict<unknown>): string {
  if (verdict.ok) return `ok ${String(verdict.value)}`
  if ('fault' in verdict) return `fault ${verdict.fault.reason}`
  return `broke ${verdict.violation.keyword}`
}

test('a checked column reads a violation, keeping the value as cast', () => {
  const column = Column.checked(0, atMost('10'))
  assert.deepEqual([column.door, column.checked, column.valid], ['i32', true, true])
  const batch = read('11\n7\nx\n', [column])
  assert.deepEqual(batch.getChecked(0, 0), {
    ok: false,
    violation: { keyword: 'maximum', value: 11, offset: 0, length: 2 },
  })
  assert.deepEqual(batch.getChecked(0, 1), { ok: true, value: 7, adjusted: false, enumIndex: null })
  assert.deepEqual(batch.getChecked(0, 2), {
    ok: false,
    fault: { reason: 'malformed', offset: 0, length: 1 },
  })
  assert.deepEqual(
    [0, 1, 2].map((row) => said(batch.getChecked(0, row))),
    ['broke maximum', 'ok 7', 'fault malformed'],
  )
  // A violation cast, so it isn't counted among the column's faults.
  assert.equal(batch.faultCount(0), 1)
  assert.equal(batch.detail(0, 0), null)
  assert.equal(batch.origin(0, 0), 'cell')
  for (const row of batch) assert.deepEqual(row.getChecked(0), batch.getChecked(0, row.index))
})

test("it is HyperCast's own checked door, cell for cell", () => {
  const cash = declare({ door: hypercast.Door.Decimal, multipleOf: '0.05', maximum: '100' })
  const cells = ['12.37', '12.35', '-0', 'x', '', '1e400', '100.01']
  // Each cell quoted, so the empty one is a cell rather than a blank line the read skips.
  const batch = read(cells.map((cell) => `"${cell}"\n`).join(''), [Column.checked(0, cash)])
  cells.forEach((cell, row) => {
    assert.deepEqual(batch.getChecked(0, row), hypercast.castChecked(cell, cash), cell)
  })
})

test('a rounded value is flagged adjusted', () => {
  const floor = declare({
    door: hypercast.Door.Decimal,
    multipleOf: '0.5',
    rounding: hypercast.Rounding.Floor,
  })
  const batch = read('1.3\n1.5\n', [Column.checked(0, floor)])
  assert.deepEqual(batch.getChecked(0, 0), {
    ok: true,
    value: '1',
    adjusted: true,
    enumIndex: null,
  })
  assert.deepEqual(batch.getChecked(0, 1), {
    ok: true,
    value: '1.5',
    adjusted: false,
    enumIndex: null,
  })
})

test('a text enum match delivers the declared entry', () => {
  const sex = declare({ door: hypercast.Door.Text, enum: ['Female', 'Male'] })
  const batch = read('MALE\nother\n', [Column.checked(0, sex)])
  assert.deepEqual(batch.getChecked(0, 0), {
    ok: true,
    value: 'Male',
    adjusted: false,
    enumIndex: 1,
  })
  assert.equal(batch.rawString(0, 0), 'MALE', 'the text as read')
  assert.deepEqual(batch.getChecked(0, 1), {
    ok: false,
    violation: { keyword: 'enum', value: 'other', offset: 0, length: 5 },
  })
})

test('an unconstrained column keeps its faults under the checked getter', () => {
  const column = Column.i32(0)
  assert.equal(column.checked, false)
  const batch = read('5\n""\nx\n', [column])
  assert.deepEqual(batch.getChecked(0, 0), { ok: true, value: 5, adjusted: false, enumIndex: null })
  for (const row of [1, 2]) assert.deepEqual(batch.getChecked(0, row), batch.get(0, row))
  assert.deepEqual(
    [1, 2].map((row) => said(batch.getChecked(0, row))),
    ['fault empty', 'fault malformed'],
  )
  // A cell the batch does not have is the contract violation, through either getter.
  assert.deepEqual(batch.getChecked(0, 9), contract)
  assert.deepEqual(batch.getChecked(9, 0), contract)
})

test('a whole column reads as checked verdicts, a constrained one or not', () => {
  const batch = read('11\n7\nx\n', [Column.checked(0, atMost('10')), Column.i32(0)])
  for (const column of [0, 1]) {
    const verdicts: CheckedVerdict<number>[] = batch.columnChecked<number>(column)
    assert.deepEqual(
      verdicts,
      [0, 1, 2].map((row) => batch.getChecked(column, row)),
      `column ${column}`,
    )
  }
  assert.deepEqual(batch.columnChecked(0).map(said), ['broke maximum', 'ok 7', 'fault malformed'])
  assert.deepEqual(batch.columnChecked(1)[0], {
    ok: true,
    value: 11,
    adjusted: false,
    enumIndex: null,
  })
  // A column the batch does not have reads as empty, as `column` does.
  assert.deepEqual(batch.columnChecked(2), [])
  const row = batch.row(0)
  assert.ok(row !== null)
  assert.deepEqual(row.getChecked(0), batch.columnChecked(0)[0])
})

test('the plain getters refuse a constrained column', () => {
  const short = declare({ door: hypercast.Door.Text, maxLength: 3 })
  const batch = read('7,a\n', [
    Column.checked(0, atMost('10')),
    Column.checked(1, short),
    Column.i32(0),
  ])
  const row = batch.row(0)
  assert.ok(row !== null)
  for (const column of [0, 1]) {
    assert.deepEqual(batch.get(column, 0), contract, `get ${column}`)
    assert.deepEqual(batch.column(column), [contract], `column ${column}`)
    assert.deepEqual(row.get(column), contract, `row get ${column}`)
  }
  // The control: the checked getter reads them, and the unconstrained column reads as ever.
  assert.deepEqual(batch.getChecked(0, 0), { ok: true, value: 7, adjusted: false, enumIndex: null })
  assert.deepEqual(batch.getChecked(1, 0), {
    ok: true,
    value: 'a',
    adjusted: false,
    enumIndex: null,
  })
  assert.deepEqual(batch.get(2, 0), { ok: true, value: 7 })
  assert.deepEqual(batch.column(2), [{ ok: true, value: 7 }])
})

test('what is no declaration makes a column that is not valid, never one with no rules', () => {
  const declared = atMost('10')
  const refused = hypercast.Constraints.declare({ door: hypercast.Door.I32, multipleOf: '0' })
  assert.ok(!refused.ok)
  const notDeclarations: [string, unknown][] = [
    ["a failed declaration's missing value", (refused as { value?: Constraints }).value],
    ['null', null],
    ['a number', 7],
    // A door a declaration can't be bound to.
    [
      'the uuid door',
      { door: hypercast.Door.Uuid, packed: declared.packed, packedEnum: declared.packedEnum },
    ],
    // Bytes that aren't the layout, and bytes HyperCast refuses.
    [
      'short bytes',
      {
        door: hypercast.Door.I32,
        packed: declared.packed.slice(1),
        packedEnum: declared.packedEnum,
      },
    ],
    [
      'a refused declaration',
      { door: hypercast.Door.I32, packed: new Uint8Array(104), packedEnum: new Uint8Array() },
    ],
    ['no enum bytes', { door: hypercast.Door.I32, packed: declared.packed }],
  ]
  for (const [name, given] of notDeclarations) {
    const column = Column.checked(0, given as Constraints)
    assert.deepEqual([column.valid, column.checked], [false, true], name)
    const bound = DelimitedReader.fromBytes('1\n', headerless, [Column.i32(0), column])
    assert.ok(!bound.ok && bound.error.kind === 'plan' && bound.error.column === 1, name)
  }
  assert.equal(Column.checked(0, undefined as unknown as Constraints).door, null)
  // The control: the declaration itself, and the same bytes as a plain object, are bound.
  const copied = { door: declared.door, packed: declared.packed, packedEnum: declared.packedEnum }
  for (const given of [declared, copied]) {
    const batch = read('11\n', [Column.checked(0, given as Constraints)])
    assert.equal(said(batch.getChecked(0, 0)), 'broke maximum')
  }
})

test('faults list a violation with its keyword', () => {
  const listed = read('11\nx\n', [Column.checked(0, atMost('10'))]).faults()
  assert.equal(listed.length, 2)
  assert.deepEqual(
    [listed[0].keyword, listed[0].reason, listed[0].offset, listed[0].length],
    ['maximum', null, 0, 2],
  )
  assert.deepEqual([listed[1].keyword, listed[1].reason], [null, 'malformed'])
})

test('checks are sized like verdicts, across every batch', () => {
  const opened = DelimitedReader.fromBytes(
    '11\n'.repeat(37),
    headerless,
    [Column.checked(0, atMost('10'))],
    { batchRows: 5 },
  )
  assert.ok(opened.ok)
  const keywords: (string | null)[] = []
  for (const read of opened.value) {
    assert.ok(read.ok)
    assert.ok(read.value.rows <= 5)
    keywords.push(...read.value.faults().map((problem) => problem.keyword))
  }
  assert.deepEqual(keywords, Array(37).fill('maximum'))
})

test('a constrained sheet column is judged', () => {
  const file = path.join(__dirname, '..', '..', 'corpus', 'workbook', 'constraints.xlsx')
  const book = Workbook.open(file)
  assert.ok(book.ok, JSON.stringify(book))
  const sheet = book.value.sheet(0, { hasHeader: true }, [Column.checked(0, atMost('10'))])
  assert.ok(sheet.ok, JSON.stringify(sheet))
  const verdicts: CheckedVerdict<unknown>[] = []
  for (const read of sheet.value) {
    assert.ok(read.ok)
    for (const row of read.value) verdicts.push(row.getChecked(0))
  }
  assert.deepEqual(verdicts[0], { ok: true, value: 0, adjusted: false, enumIndex: null })
  assert.ok(
    verdicts.some(
      (verdict) => !verdict.ok && 'violation' in verdict && verdict.violation.value === 42,
    ),
  )
})

test('a column keeps its declaration through withTokens, and reads its numbers in its format', () => {
  const column = Column.checked(0, atMost('10')).withTokens(['NA'])
  assert.deepEqual([column.door, column.checked, column.valid], ['i32', true, true])
  const batch = read('NA\n11\n', [column])
  assert.equal(batch.origin(0, 0), 'token')
  assert.equal(said(batch.getChecked(0, 0)), 'fault empty')
  assert.equal(said(batch.getChecked(0, 1)), 'broke maximum')
  const comma = hypercast.NumFormat.declare({ decimalSep: ',', groupSep: '.', flags: 0 })
  assert.ok(comma.ok)
  const cash = declare({ door: hypercast.Door.Decimal, maximum: '1000' })
  const european = read('"1234,5"\n', [Column.checked(0, cash, comma.value)])
  const broken = {
    ok: false,
    violation: { keyword: 'maximum', value: '1234.5', offset: 0, length: 6 },
  }
  assert.deepEqual(european.getChecked(0, 0), broken)
  assert.deepEqual(hypercast.castChecked('1234,5', cash, comma.value), broken)
})
