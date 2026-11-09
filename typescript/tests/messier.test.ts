// What messier files need: a dialect's skip, header row, short and long rows, encoding and
// tokens; a sheet's skip, header row, range and tokens; a column's own tokens and date
// options; each cell's detail and origin; a batch's faults list; a header's names. Each option
// is the core's, read by the core: these tests prove each reaches it, and what it answers comes
// back.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { readFileSync }: typeof import('node:fs') = require('node:fs')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Range, Workbook } = api

type Batch = import('../index.js').Batch
type Dialect = import('../index.js').Dialect
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

function value<T>(outcome: Outcome<T> | { ok: true; value: T } | { ok: false }): T {
  assert.ok(outcome.ok, JSON.stringify(outcome))
  return outcome.value
}

function batches(
  text: string | Uint8Array,
  dialect: Dialect | null,
  plan: ColumnT[],
  options?: import('../index.js').DelimitedOptions,
): Batch[] {
  const reader = value(DelimitedReader.fromBytes(text, dialect, plan, options))
  return [...reader].map((read) => value(read))
}

function only(text: string | Uint8Array, dialect: Dialect | null, plan: ColumnT[]): Batch {
  const read = batches(text, dialect, plan)
  assert.equal(read.length, 1)
  return read[0]
}

test('each dialect option does its own work', () => {
  const text = 'a title line\n"a quote, never closed\nid,n\n1,x\n2\n3,y,z\n'
  const dialect: Dialect = { skip: 2, shortRows: 'pad', longRows: 'report' }
  const batch = only(text, dialect, [Column.i32(0), Column.text(1)])
  const reader = value(DelimitedReader.fromBytes(text, dialect))
  assert.deepEqual(reader.header?.names, ['id', 'n'])
  assert.deepEqual(
    batch.column(0),
    [1, 2, 3].map((v) => ({ ok: true, value: v })),
  )
  assert.equal(batch.origin(1, 1), 'padded')
  assert.deepEqual(batch.get(1, 1), { ok: false, fault: { reason: 'empty', offset: 0, length: 0 } })
  // The lines a batch says count what was passed over.
  assert.deepEqual([batch.line(0), batch.line(1), batch.line(2)], [4, 5, 6])
  assert.deepEqual(
    batch.faults().map((p) => [p.row, p.planColumn, p.expected, p.found]),
    [[2, null, 2, 3]],
  )
  // Refused by default, the same rows are failures.
  const strict = [...value(DelimitedReader.fromBytes(text, { skip: 2 }, [Column.i32(0)]))]
  assert.equal(strict.length, 2)
  assert.deepEqual(value(strict[0]).column(0), [{ ok: true, value: 1 }])
  const stopped = strict[1]
  assert.ok(!stopped.ok && stopped.error.kind === 'structure')
  assert.equal(stopped.error.failure?.kind, 'columnCount')
  // Truncated, the long row reads and is not listed.
  const truncated = only(text, { ...dialect, longRows: 'truncate' }, [Column.i32(0)])
  assert.deepEqual(truncated.faults(), [])
  // A header row discards records ahead of the header.
  const late = value(DelimitedReader.fromBytes('junk,1\nid,n\n7,x\n', { headerRow: 1 }))
  assert.deepEqual(late.header?.names, ['id', 'n'])
  // A header row past the input has no header to name.
  assert.equal(value(DelimitedReader.fromBytes('a\n', { headerRow: 5 })).header?.length ?? 0, 0)
})

test('an encoding is declared, never sniffed, and its failure says the file’s byte', () => {
  const bytes = Uint8Array.from([0x61, 0x0a, 0x63, 0x61, 0x66, 0xe9, 0x0a])
  const latin = only(bytes, { encoding: 'latin1' }, [Column.text(0)])
  assert.deepEqual(latin.get(0, 0), { ok: true, value: 'café' })
  const windows = only(Uint8Array.from([0x61, 0x0a, 0x80, 0x0a]), { encoding: 'windows1252' }, [
    Column.text(0),
  ])
  assert.deepEqual(windows.get(0, 0), { ok: true, value: '€' })
  const utf16 = only(Buffer.from('a\nñ\n', 'utf16le'), { encoding: 'utf16le' }, [Column.text(0)])
  assert.deepEqual(utf16.get(0, 0), { ok: true, value: 'ñ' })
  // Undeclared, the same bytes are UTF-8: the cell's text is invalid, and says so.
  const undeclared = only(bytes, null, [Column.text(0)])
  assert.equal(undeclared.get(0, 0).ok, false)
  assert.equal(undeclared.detail(0, 0), 'invalidUtf8')
  // A lone surrogate under UTF-16 comes after every intact row.
  const broken = Buffer.concat([
    Buffer.from('a\n1\n', 'utf16le'),
    Buffer.from([0x00, 0xd8, 0x0a, 0x00]),
  ])
  const reader = value(DelimitedReader.fromBytes(broken, { encoding: 'utf16le' }, [Column.i32(0)]))
  const first = value(reader.read())
  assert.ok(first !== null)
  assert.deepEqual(first.column(0), [{ ok: true, value: 1 }])
  const failed = reader.read()
  assert.ok(!failed.ok && failed.error.failure?.kind === 'encoding')
  assert.equal(failed.error.failure?.byte, 8)
  for (const bad of ['utf-8', 'UTF8', 1, true]) {
    const refused = DelimitedReader.fromBytes('a', { encoding: bad as never })
    assert.ok(!refused.ok && refused.error.kind === 'argument')
    assert.match(refused.error.message, /^dialect\.encoding must be one of 'utf8', /)
  }
})

test("a column's own tokens replace the dialect's, and a quoted cell is never a token", () => {
  const text = 'a,b\nNA,NA\n-,-\n"NA",3\n'
  const dialect: Dialect = { tokens: ['NA'] }
  const batch = only(text, dialect, [Column.i32(0), Column.i32(1).withTokens(['-'])])
  assert.deepEqual(
    [batch.origin(0, 0), batch.origin(1, 0), batch.origin(0, 1), batch.origin(1, 1)],
    ['token', 'cell', 'cell', 'token'],
  )
  assert.equal(batch.get(0, 0).ok, false)
  assert.equal(batch.detail(0, 0), null)
  assert.equal(batch.detail(1, 0), 'unexpectedCharacter')
  // Quoted, NA is the cell's text, judged by its door.
  assert.equal(batch.origin(0, 2), 'cell')
  assert.equal(batch.detail(0, 2), 'unexpectedCharacter')
  // An empty list opts the column out of its dialect's.
  const out = only(text, dialect, [Column.i32(0).withTokens([])])
  assert.equal(out.origin(0, 0), 'cell')
  // A token is not a problem; the cells that did not cast are.
  assert.deepEqual(
    batch.faults().map((p) => [p.row, p.planColumn, p.detail]),
    [
      [0, 1, 'unexpectedCharacter'],
      [1, 0, 'noDigits'],
      [2, 0, 'unexpectedCharacter'],
    ],
  )
  for (const bad of ['NA', [1], [null]]) {
    const refused = DelimitedReader.fromBytes(text, { tokens: bad as never })
    assert.ok(!refused.ok && refused.error.kind === 'argument', JSON.stringify(bad))
  }
  // A column's tokens of the wrong type make a column the plan refuses, at its place.
  const refused = DelimitedReader.fromBytes(text, null, [
    Column.i32(0),
    Column.i32(1).withTokens('x' as never),
  ])
  assert.ok(!refused.ok && refused.error.kind === 'plan' && refused.error.column === 1)
})

test("a column's date options and month names are read", () => {
  const names = [
    'janv',
    'févr',
    'mars',
    'avr',
    'mai',
    'juin',
    'juil',
    'août',
    'sept',
    'oct',
    'nov',
    'déc',
  ]
  const months = value(
    hypercast.MonthNames.declare(names.map((name, at) => [at + 1, name] as const)),
  )
  const options = value(hypercast.DateOptions.declare({ flags: 0, pivot: 2029, months }))
  const plan = [
    Column.dateOrdered(0, hypercast.DateOrder.DayMonthYear).withDateOptions(options),
    Column.date(1),
    Column.dateOrdered(0, hypercast.DateOrder.DayMonthYear),
  ]
  const batch = only('a,b\n3-févr-26,20260203\n', null, plan)
  assert.equal(batch.get(2, 0).ok, false)
  assert.deepEqual(batch.get(0, 0), { ok: true, value: { year: 2026, month: 2, day: 3 } })
  assert.equal(batch.get(1, 0).ok, false)
  const compact = value(hypercast.DateOptions.declare({ flags: hypercast.DateStyles.CompactDate }))
  const read = only('b\n20260203\n', null, [Column.date(0).withDateOptions(compact)])
  assert.deepEqual(read.get(0, 0), { ok: true, value: { year: 2026, month: 2, day: 3 } })
  // Options that are not HyperCast's make a column the plan refuses.
  for (const bad of [
    null,
    'x',
    { flags: -1 },
    { pivot: 2 ** 16 },
    { months: { packed: new Uint8Array([1]) } },
  ]) {
    const column = Column.date(0).withDateOptions(bad as never)
    assert.equal(column.valid, false, JSON.stringify(bad))
    const refused = DelimitedReader.fromBytes('a\n1\n', null, [column])
    assert.ok(!refused.ok && refused.error.kind === 'plan' && refused.error.column === 0)
  }
})

test('a detail says what failed, and notes off says nothing but the reason', () => {
  const text = 'a,b\n12x,ok\n,1\n'
  const plan = [Column.i32(0), Column.text(1)]
  const noted = only(text, null, plan)
  assert.equal(noted.detail(0, 0), 'unexpectedCharacter')
  assert.equal(noted.detail(0, 1), 'blank')
  assert.equal(noted.detail(1, 0), null)
  assert.equal(noted.row(0)?.detail(0), 'unexpectedCharacter')
  const [problem] = noted.faults()
  assert.deepEqual(problem, {
    row: 0,
    line: 2,
    planColumn: 0,
    sourceColumn: 0,
    reason: 'malformed',
    detail: 'unexpectedCharacter',
    origin: 'cell',
    offset: 2,
    length: 1,
    expected: 0,
    found: 0,
  })
  // Notes off, the verdicts are the same, and nothing more is said of them.
  const quiet = batches(text, null, plan, { notes: false })[0]
  assert.deepEqual(quiet.column(0), noted.column(0))
  assert.equal(quiet.detail(0, 0), null)
  assert.equal(quiet.origin(0, 0), 'cell')
  assert.deepEqual(
    quiet.faults().map((p) => [p.row, p.detail, p.origin]),
    [[0, null, 'cell']],
  )
  // A token with notes off is an empty cell and says no more.
  const token = batches('a\nNA\n', { tokens: ['NA'] }, [Column.i32(0)], { notes: false })[0]
  assert.equal(token.origin(0, 0), 'cell')
  assert.deepEqual(token.get(0, 0), { ok: false, fault: { reason: 'empty', offset: 0, length: 0 } })
})

test('a problem names its source column as well as its plan’s', () => {
  const batch = only('a,b,c\n1,2,x\n', null, [Column.i32(2), Column.i32(0)])
  assert.deepEqual(
    batch.faults().map((p) => [p.planColumn, p.sourceColumn]),
    [[0, 2]],
  )
})

test('a faults list is the caller’s own, and a batch outlives its reader and the next read', () => {
  const reader = value(
    DelimitedReader.fromBytes('a\nx\ny\n', null, [Column.i32(0)], { batchRows: 1 }),
  )
  const first = value(reader.read())
  assert.ok(first !== null)
  const listed = first.faults()
  listed[0].row = 99
  listed.pop()
  assert.equal(first.faults()[0].row, 0)
  const second = value(reader.read())
  assert.ok(second !== null)
  assert.deepEqual(reader.close(), { ok: true, value: null })
  assert.equal(first.faults()[0].line, 2)
  assert.equal(second.faults()[0].line, 3)
  assert.equal(first.rawString(0, 0), 'x')
  assert.equal(first.detail(0, 0), 'unexpectedCharacter')
})

test('a header names every column; require refuses two of a name, and ordinal takes the first', () => {
  const reader = value(DelimitedReader.fromBytes('id,name,id\n1,a,2\n'))
  const header = reader.header
  assert.ok(header !== null)
  assert.equal(header.ordinal('id'), 0)
  const twice = header.require('id')
  assert.ok(!twice.ok)
  assert.deepEqual(
    [twice.error.kind, twice.error.first, twice.error.second],
    ['ambiguousColumn', 0, 2],
  )
  assert.equal(
    twice.error.message,
    'the header names two columns "id", at positions 0 and 2: say which by position',
  )
  assert.deepEqual(header.require('name'), { ok: true, value: 1 })
  const none = header.require('nope')
  assert.ok(!none.ok && none.error.kind === 'noColumn')
  assert.equal(none.error.message, 'the header has no column named "nope"')
  const wrong = header.require(1 as never)
  assert.ok(!wrong.ok && wrong.error.kind === 'argument')
})

test('a range is a value, from its text or its four numbers', () => {
  const range = value(Range.parse('A5:F200'))
  assert.deepEqual(
    [range.firstRow, range.firstColumn, range.lastRow, range.lastColumn],
    [5, 1, 200, 6],
  )
  const open = value(Range.of(5, 0, 0, 0))
  assert.deepEqual([open.firstRow, open.lastRow], [5, 0])
  for (const bad of ['B2:A1', 'a1:b2', 'A1', '']) {
    const refused = Range.parse(bad)
    assert.ok(!refused.ok && refused.error.kind === 'range', bad)
    assert.equal(refused.error.message, `not a range: ${JSON.stringify(bad)}`)
  }
  const backwards = Range.of(9, 1, 2, 1)
  assert.ok(!backwards.ok && backwards.error.kind === 'range')
  const wrong = Range.of(-1, 1, 2, 1)
  assert.ok(!wrong.ok && wrong.error.kind === 'argument')
  // A sheet takes either, and refuses text that is not one as the core does.
  const book = value(openBook(path.join(WORKBOOKS, 'basic.xlsx')))
  const sheet = book.sheet(0, { range: 'nope' })
  assert.ok(!sheet.ok && sheet.error.kind === 'range')
  const object = book.sheet(0, { range: {} as never })
  assert.ok(!object.ok && object.error.kind === 'argument')
  const byText = value(
    book.sheet(0, { range: 'A1:B3', hasHeader: false }, [Column.text(0), Column.text(1)]),
  )
  const byValue = value(
    book.sheet(0, { range: value(Range.parse('A1:B3')), hasHeader: false }, [
      Column.text(0),
      Column.text(1),
    ]),
  )
  const a = value(byText.read())
  const b = value(byValue.read())
  assert.ok(a !== null && b !== null)
  assert.equal(a.rows, 3)
  assert.deepEqual(a.column(0), b.column(0))
})

test("a sheet's skip, header row and tokens change what it reads", () => {
  const book = value(openBook(path.join(WORKBOOKS, 'basic.xlsx')))
  const plain = value(book.sheet(0, { hasHeader: false }, [Column.text(0)]))
  const all = value(plain.read())
  assert.ok(all !== null && all.rows >= 3)
  const first = all.get(0, 0)
  const second = all.get(0, 1)
  const skipped = value(
    value(book.sheet(0, { hasHeader: false, skip: 1 }, [Column.text(0)])).read(),
  )
  assert.ok(skipped !== null)
  assert.deepEqual(skipped.get(0, 0), second)
  const headed = value(book.sheet(0, { headerRow: 1 }))
  assert.equal(headed.header?.names[0], second.ok ? second.value : undefined)
  assert.ok(first.ok)
  const tokened = value(
    value(
      book.sheet(0, { hasHeader: false, tokens: [first.value as string] }, [Column.text(0)]),
    ).read(),
  )
  assert.ok(tokened !== null)
  assert.equal(tokened.origin(0, 0), 'token')
  assert.equal(tokened.get(0, 0).ok, false)
})
