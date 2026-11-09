// What the caller declares (columns, dialects, options, plans) and how a declaration the core
// cannot honour is refused: an outcome's error, never an exception.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { readFileSync }: typeof import('node:fs') = require('node:fs')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Dialect, Workbook } = api
const { DateOrder, ExcelEpoch, NumFormat, NumStyles, UnixPrecision } = hypercast

function refusal(outcome: {
  ok: boolean
  error?: { kind: string; message: string; column?: number }
}) {
  assert.ok(!outcome.ok, JSON.stringify(outcome))
  return outcome.error as { kind: string; message: string; column?: number }
}

function cells(
  text: string,
  plan: InstanceType<typeof Column>[],
  dialect?: import('../index.js').Dialect,
) {
  const opened = DelimitedReader.fromBytes(text, dialect, plan)
  assert.ok(opened.ok, JSON.stringify(opened))
  const read = opened.value.read()
  assert.ok(read.ok && read.value !== null)
  const batch = read.value
  return plan.map((_, column) => batch.get(column, 0))
}

test('a column reports its door and ordinal, and whether the core can honour it', () => {
  const column = Column.unix(3, UnixPrecision.Milliseconds)
  assert.equal(column.door, 'unix')
  assert.equal(column.ordinal, 3)
  assert.equal(column.valid, true)
  assert.equal(Column.excelSerial(0, ExcelEpoch.Y1904).door, 'excelSerial')
  assert.equal(Column.dateOrdered(0, DateOrder.DayMonthYear).door, 'dateOrdered')
  for (const bad of [
    Column.i32(-1),
    Column.i32(1.5),
    Column.unix(0, 9),
    Column.datetime(0, 0),
    Column.excelSerial(0, 3),
    Column.f64(0, { packed: new Uint8Array(92) }),
    Column.f64(0, { packed: new Uint8Array(4) }),
  ]) {
    assert.equal(bad.valid, false, `${bad.door} ${bad.ordinal}`)
  }
  assert.ok(Number.isNaN(Column.text('x' as unknown as number).ordinal))
})

test('a plan the core cannot honour is refused with the column it names', () => {
  const plan = [Column.text(0), Column.i32(0), Column.unix(0, 9)]
  for (const outcome of [
    DelimitedReader.fromBytes('a\n1\n', null, plan),
    DelimitedReader.fromBytes('a\n1\n', null, [Column.text(0), 'x', Column.i32(0)] as never),
  ]) {
    const error = refusal(outcome)
    assert.equal(error.kind, 'plan')
    assert.ok(error.column === 1 || error.column === 2)
  }
  // An ordinal the core cannot address is a valid column, refused by the core at bind.
  const far = refusal(
    DelimitedReader.fromBytes('a\n1\n', null, [Column.text(0), Column.i32(2 ** 20)]),
  )
  assert.deepEqual([far.kind, far.column], ['plan', 1])
  assert.equal(Column.i32(2 ** 20).valid, true)
  // Past the width a header gives every record: the Rust binding's own refusal.
  const error = refusal(DelimitedReader.fromBytes('a,b\n1,2\n', null, [Column.i32(2)]))
  assert.deepEqual([error.kind, error.column], ['plan', 0])
  assert.equal(refusal(DelimitedReader.fromBytes('a\n', null, 'plan' as never)).kind, 'argument')
})

test('a numeric column reads in the NumFormat it declares', () => {
  const euro = NumFormat.declare({
    decimalSep: ',',
    groupSep: '.',
    flags: NumStyles.All,
    currency: '€',
  })
  assert.ok(euro.ok)
  assert.deepEqual(
    cells(
      'a;b\n"1.234,5 €";1.234,5\n',
      [Column.decimal(0, euro.value), Column.f64(1, euro.value)],
      {
        separator: ';',
      },
    ),
    [
      { ok: true, value: '1234.5' },
      { ok: true, value: 1234.5 },
    ],
  )
  // Left out, the format is the invariant one; declared, it is the one declared.
  assert.deepEqual(cells('a\n"1,5"\n', [Column.f64(0)])[0], hypercast.castF64('1,5'))
  assert.deepEqual(
    cells('a\n"1,5"\n', [Column.f64(0, NumFormat.detect())])[0],
    hypercast.castF64('1,5', NumFormat.detect()),
  )
  assert.notDeepEqual(hypercast.castF64('1,5'), hypercast.castF64('1,5', NumFormat.detect()))
})

test('every door makes the value @skunkwerkx/hypercast makes of the same text', () => {
  const row = [
    'true',
    '-5',
    '18446744073709551615',
    '2.5',
    '1.10',
    '{01020304-0506-0708-090A-0B0C0D0E0F10}',
    '2026-01-02T15:04:05.5Z',
    '1700000000123',
    '45000',
    '2026-01-07',
    '7/1/2026',
    '1/7/2026 3:04 PM',
    '15:04:05',
    'PT1H30M',
    'plain',
  ]
  const plan = [
    Column.bool(0),
    Column.i8(1),
    Column.u64(2),
    Column.f32(3),
    Column.decimal(4),
    Column.uuid(5),
    Column.timestamp(6),
    Column.unix(7, UnixPrecision.Milliseconds),
    Column.excelSerial(8, ExcelEpoch.Y1900),
    Column.date(9),
    Column.dateOrdered(10, DateOrder.DayMonthYear),
    Column.datetime(11, DateOrder.MonthDayYear),
    Column.time(12),
    Column.duration(13),
    Column.text(14),
  ]
  const text = `${row.map((_, i) => `c${i}`).join('\t')}\n${row.join('\t')}\n`
  const got = cells(text, plan, Dialect.TSV)
  const want = [
    hypercast.castBool(row[0]),
    hypercast.castI8(row[1]),
    hypercast.castU64(row[2]),
    hypercast.castF32(row[3]),
    hypercast.castDecimal(row[4]),
    hypercast.castUuid(row[5]),
    hypercast.castTimestamp(row[6]),
    hypercast.castUnix(row[7], UnixPrecision.Milliseconds),
    hypercast.castExcelSerial(row[8], ExcelEpoch.Y1900),
    hypercast.castDate(row[9]),
    hypercast.castDate(row[10], DateOrder.DayMonthYear),
    hypercast.castDatetime(row[11], DateOrder.MonthDayYear),
    hypercast.castTime(row[12]),
    hypercast.castDuration(row[13]),
    { ok: true, value: 'plain' },
  ]
  assert.deepEqual(got, want)
  for (const verdict of got) assert.ok(verdict.ok, String(verdict.ok || verdict.fault.reason))
})

test('dialects and options are declared, and refused when they cannot be honoured', () => {
  assert.deepEqual(Dialect.CSV, {
    separator: ',',
    quoting: true,
    hasHeader: true,
    skipBlankLines: true,
  })
  assert.equal(Dialect.TSV.separator, '\t')
  assert.equal(Dialect.PSV.separator, '|')
  assert.ok(Object.isFrozen(Dialect) && Object.isFrozen(Dialect.CSV))
  assert.equal(refusal(DelimitedReader.fromBytes('a', { separator: '"' })).kind, 'separator')
  assert.equal(refusal(DelimitedReader.fromBytes('a', { separator: '\n' })).kind, 'separator')
  assert.equal(refusal(DelimitedReader.fromBytes('a', { separator: ',,' })).kind, 'separator')
  assert.equal(refusal(DelimitedReader.fromBytes('a', { separator: 1 } as never)).kind, 'argument')
  assert.equal(
    refusal(DelimitedReader.fromBytes('a', { separator: ',', quoting: 1 } as never)).kind,
    'argument',
  )
  assert.equal(
    refusal(DelimitedReader.fromBytes('a', null, null, { batchRows: 0 })).kind,
    'argument',
  )
  assert.equal(
    refusal(DelimitedReader.fromBytes('a', null, null, { bufferBytes: 1.5 })).kind,
    'argument',
  )
  // Without a header the reader knows no names, and every record is a row.
  const opened = DelimitedReader.fromBytes('1|x\n2|y\n', { ...Dialect.PSV, hasHeader: false }, [
    Column.i32(0),
  ])
  assert.ok(opened.ok)
  assert.equal(opened.value.header, null)
  const read = opened.value.read()
  assert.ok(read.ok && read.value !== null)
  assert.equal(read.value.rows, 2)
  assert.deepEqual([read.value.line(0), read.value.line(1)], [1, 2])
})

test('a header finds a column by name, or says which it lacks', () => {
  const opened = DelimitedReader.fromBytes('id,name,id\n1,a,2\n')
  assert.ok(opened.ok)
  const header = opened.value.header
  assert.ok(header !== null)
  assert.deepEqual(header.names, ['id', 'name', 'id'])
  assert.equal(header.length, 3)
  assert.equal(header.ordinal('id'), 0)
  assert.equal(header.ordinal(new TextEncoder().encode('name')), 1)
  assert.equal(header.ordinal('ID'), null)
  assert.deepEqual(header.require('name'), { ok: true, value: 1 })
  const missing = refusal(header.require('score'))
  assert.equal(missing.kind, 'noColumn')
  assert.match(missing.message, /"score"/)
  assert.equal(opened.value.columnCount, 3)
})

test('a workbook names its sheets, and an unknown one is refused', () => {
  const file = path.resolve(__dirname, '../../corpus/workbook/multisheet.xlsx')
  const opened = Workbook.fromBytes(new Uint8Array(readFileSync(file)))
  assert.ok(opened.ok)
  const book = opened.value
  assert.ok(book.sheets.length > 1)
  assert.equal(refusal(book.sheet('no such sheet')).kind, 'noSheet')
  assert.equal(refusal(book.sheet(book.sheets.length)).kind, 'noSheet')
  assert.equal(refusal(book.sheet(-1)).kind, 'argument')
  assert.equal(refusal(book.sheet(0, { batchRows: -1 })).kind, 'argument')
  assert.equal(refusal(Workbook.fromBytes(new TextEncoder().encode('not a zip'))).kind, 'structure')
  assert.equal(refusal(Workbook.open(path.join(__dirname, 'no such file.xlsx'))).kind, 'io')
  const sheet = book.sheet(book.sheets[1].name, { hasHeader: false }, [Column.text(0)])
  assert.ok(sheet.ok)
  assert.equal(sheet.value.header, null)
  assert.equal(sheet.value.isBound, true)
})

test('a batch outlives the next read', () => {
  const opened = DelimitedReader.fromBytes('n\n1\n2\n3\n', null, [Column.i32(0)], { batchRows: 1 })
  assert.ok(opened.ok)
  const batches = [...opened.value].map((read) => {
    assert.ok(read.ok)
    return read.value
  })
  assert.deepEqual(
    batches.map((batch) => batch.get(0, 0)),
    [1, 2, 3].map((value) => ({ ok: true, value })),
  )
  const rows = DelimitedReader.fromBytes('n\n4\n5\n', null, [Column.i32(0)], { batchRows: 1 })
  assert.ok(rows.ok)
  assert.deepEqual(
    [...rows.value.rows()].map((row) => (row.ok ? row.value.get(0) : row)),
    [4, 5].map((value) => ({ ok: true, value })),
  )
})
