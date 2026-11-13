// Replays the shared conformance corpus (corpus/delimited.json and corpus/workbook.json at the
// repository root), the files the Rust binding and every other binding replay, through this
// one. Delimited text is read from memory, from a string and from a file, in batches of one
// row, two and many, through an input buffer of one byte, five and the default, header first
// (opened without a plan, the plan bound once the header is read) and with notes off. How the
// input is cut up is the binding's business and must not change the answer. Every workbook
// case is read from its bytes and its path, header first and with the plan up front, by index
// and (where the name finds it) by name, in batches of one row, two and many, and with notes
// off.
//
// Every cell is held to the corpus three ways: as `get` gives it, as the whole-column `column`
// gives it, and as its row gives it, with its detail and origin; every delimited cell is held
// to what `@skunkwerkx/hypercast`'s own door says of its raw text, HyperCast being the judge.
// Each batch's faults list is held to the case's `problems`, and a case's `suggest` to what a
// suggestion says. A key this replay doesn't know fails it, wherever in a case it is.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } =
  require('node:fs') as typeof import('node:fs')
const { tmpdir }: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { inspect }: typeof import('node:util') = require('node:util')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Range, Workbook } = api
// Under the WASI fallback on Windows no path reaches the module: napi-rs preopens the drive
// root under its Windows name (`D:\`), which wasi-libc matches only before a `/`. Opening by
// path is a recorded loss there (README), pinned below as the I/O error it is; every case is
// still read, from bytes.
const wasi =
  (require('../binding.js') as { __napiBindingTarget: string }).__napiBindingTarget !== 'native'
const pathsUnreachable = wasi && process.platform === 'win32'

type Batch = import('../index.js').Batch
type Outcome<T> = import('../index.js').Outcome<T>
type Value = import('../index.js').Value
type Verdict<T> = import('../index.js').Verdict<T>
type CheckedVerdict<T> = import('../index.js').CheckedVerdict<T>
type Fault = import('../index.js').Fault
type Constraints = InstanceType<typeof hypercast.Constraints>
type Problem = import('../index.js').Problem
type Suggestion = import('../index.js').Suggestion
type Dialect = import('../index.js').Dialect
type SheetOptions = import('../index.js').SheetOptions
type ColumnT = InstanceType<typeof Column>
type Reader = import('../index.js').DelimitedReader | import('../index.js').Sheet
type NumFormat = InstanceType<typeof hypercast.NumFormat>
// biome-ignore lint/suspicious/noExplicitAny: the corpus is JSON, read by key
type Json = any

function unreachable(label: string, outcome: Outcome<unknown>) {
  assert.ok(!outcome.ok && outcome.error.kind === 'io', `${label}: ${JSON.stringify(outcome)}`)
}

function corpusDirectory(): string {
  let dir = __dirname
  for (;;) {
    const candidate = path.join(dir, 'corpus')
    if (existsSync(path.join(candidate, 'delimited.json'))) return candidate
    const parent = path.dirname(dir)
    if (parent === dir) throw new Error(`corpus directory not found above ${__dirname}`)
    dir = parent
  }
}
// HYPERTABULAR_CORPUS points the replay at another copy (the negative control edits one).
const CORPUS = process.env.HYPERTABULAR_CORPUS ?? corpusDirectory()

// An integer literal past 2^53 keeps its exact digits (as a string, for the test to make a
// bigint of): JSON.parse alone would round 18446744073709551615 to a double.
function load(name: string): Json[] {
  return JSON.parse(
    readFileSync(path.join(CORPUS, name), 'utf8'),
    (_key, value, context?: { source?: string }) => {
      if (typeof value !== 'number' || Number.isSafeInteger(value)) return value
      if (context?.source === undefined) {
        throw new Error('this runtime gives JSON.parse revivers no source text')
      }
      return /^-?\d+$/.test(context.source) ? context.source : value
    },
  )
}

// Every key the replays read, by where it is: what isn't here fails `unknownKeys`, so a key
// added to the corpus can't be passed over. `note` is the one no replay reads: a sentence for
// whoever reads the file.
const CELL_KEYS = new Set([
  'expect',
  'value',
  'text',
  'seconds',
  'nanos',
  'year',
  'month',
  'day',
  'nanos_of_day',
  'magnitude',
  'scale',
  'negative',
  'fault',
  'raw',
  'raw_hex',
  'detail',
  'origin',
  'keyword',
  'adjusted',
  'enum_index',
  'bits',
])
const PROBLEM_KEYS = new Set([
  'row',
  'line',
  'plan_column',
  'source_column',
  'reason',
  'fault',
  'detail',
  'origin',
  'expected',
  'found',
  'keyword',
])
const FAILURE_KEYS = new Set(['kind', 'record', 'line', 'byte', 'expected', 'found'])
const PLAN_KEYS = new Set([
  'door',
  'ordinal',
  'precision',
  'order',
  'epoch',
  'format',
  'tokens',
  'options',
  'constraints',
])
// A plan entry's constraints, by the OpenAPI names HyperCast's corpus gives them, which are
// `Constraints.declare`'s own.
const CONSTRAINT_KEYS = new Set([
  'minimum',
  'exclusiveMinimum',
  'maximum',
  'exclusiveMaximum',
  'multipleOf',
  'rounding',
  'minLength',
  'maxLength',
  'trim',
  'caseSensitive',
  'enum',
])
const FORMAT_KEYS = new Set(['decimal_sep', 'group_sep', 'flags', 'currency'])
const DATE_OPTION_KEYS = new Set(['flags', 'pivot', 'months'])
const DELIMITED_KEYS = new Set([
  'name',
  'input',
  'input_hex',
  'dialect',
  'plan',
  'header',
  'rows',
  'problems',
  'failure',
  'suggest',
])
const DIALECT_KEYS = new Set([
  'separator',
  'quoting',
  'has_header',
  'skip_blank_lines',
  'skip',
  'header_row',
  'short_rows',
  'long_rows',
  'tokens',
  'encoding',
])
const SUGGEST_KEYS = new Set(['columns', 'truncated', 'sample_rows', 'format'])
const SUGGESTED_KEYS = new Set(['door', 'param', 'status', 'matched', 'sampled'])
const WORKBOOK_KEYS = new Set([
  'name',
  'file',
  'format',
  'epoch',
  'sheets',
  'sheet',
  'options',
  'plan',
  'header',
  'numbers',
  'rows',
  'problems',
  'failure',
  'note',
  'suggest',
])
const SHEET_OPTION_KEYS = new Set([
  'has_header',
  'skip_empty_rows',
  'range',
  'skip',
  'header_row',
  'tokens',
])
const SHEET_KEYS = new Set(['name', 'hidden'])

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

// Every key of a case no replay here reads, as a path.
function unknownKeys(c: Json, workbook: boolean): string[] {
  const name = c.name ?? 'a case with no name'
  const unknown: string[] = []
  const check = (value: unknown, at: string, known: Set<string>) => {
    if (!isObject(value)) return
    for (const key of Object.keys(value)) if (!known.has(key)) unknown.push(`${at}.${key}`)
  }
  check(c, name, workbook ? WORKBOOK_KEYS : DELIMITED_KEYS)
  if (workbook) {
    check(c.options, `${name}.options`, SHEET_OPTION_KEYS)
    for (const sheet of c.sheets ?? []) check(sheet, `${name}.sheets[]`, SHEET_KEYS)
  } else {
    check(c.dialect, `${name}.dialect`, DIALECT_KEYS)
  }
  for (const column of c.plan ?? []) {
    check(column, `${name}.plan[]`, PLAN_KEYS)
    check(column?.format, `${name}.plan[].format`, FORMAT_KEYS)
    check(column?.options, `${name}.plan[].options`, DATE_OPTION_KEYS)
    check(column?.constraints, `${name}.plan[].constraints`, CONSTRAINT_KEYS)
  }
  for (const row of c.rows ?? []) for (const cell of row) check(cell, `${name}.rows[][]`, CELL_KEYS)
  for (const problem of c.problems ?? []) check(problem, `${name}.problems[]`, PROBLEM_KEYS)
  check(c.failure, `${name}.failure`, FAILURE_KEYS)
  if (c.suggest !== undefined) {
    check(c.suggest, `${name}.suggest`, SUGGEST_KEYS)
    check(c.suggest.format, `${name}.suggest.format`, FORMAT_KEYS)
    for (const column of c.suggest.columns)
      check(column, `${name}.suggest.columns[]`, SUGGESTED_KEYS)
  }
  return unknown
}

// Every object in `value`, the first of each list only, as the path to it.
function keyPlaces(value: unknown, at: (string | number)[] = []): (string | number)[][] {
  if (isObject(value)) {
    const places = [at]
    for (const [key, inner] of Object.entries(value)) places.push(...keyPlaces(inner, [...at, key]))
    return places
  }
  if (Array.isArray(value) && value.length > 0) return keyPlaces(value[0], [...at, 0])
  return []
}

// Nothing in the corpus is passed over, and the check notices a key added anywhere.
function assertKeysAreKnown(cases: Json[], workbook: boolean): void {
  assert.deepEqual(
    cases.flatMap((c) => unknownKeys(c, workbook)),
    [],
  )
  for (const c of cases) {
    for (const place of keyPlaces(c)) {
      const edited = structuredClone(c)
      let at = edited
      for (const step of place) at = at[step]
      at.junk = true
      const unknown = unknownKeys(edited, workbook)
      assert.ok(
        unknown.length === 1 && unknown[0].endsWith('.junk'),
        `${JSON.stringify(place)}: ${unknown}`,
      )
    }
  }
}

const encoder = new TextEncoder()
// A byte-order mark in a cell is the cell's text: the decoder keeps it.
const decoder = new TextDecoder('utf-8', { ignoreBOM: true })
const camel = (name: string) => name.replace(/_([a-z0-9])/g, (_, letter) => letter.toUpperCase())
const snake = (name: string) => name.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`)
const hex = (text: string) => Uint8Array.from(Buffer.from(text, 'hex'))

const formats = new Map<string, NumFormat>()
// The format an entry names, its flags split as HyperCast's declaration takes them: the
// leniences, and the grouping policy above them.
function formatOf(spec: Json | undefined): NumFormat | undefined {
  if (spec === undefined) return undefined
  const key = JSON.stringify(spec)
  const cached = formats.get(key)
  if (cached) return cached
  const declared = hypercast.NumFormat.declare({
    decimalSep: spec.decimal_sep,
    groupSep: spec.group_sep,
    flags: spec.flags & 0xff,
    grouping: spec.flags & ~0xff,
    currency: spec.currency,
  })
  assert.ok(declared.ok, `${key}: ${JSON.stringify(declared)}`)
  formats.set(key, declared.value)
  return declared.value
}

const DATE_FLAGS: Record<string, number> = {
  compact_date: hypercast.DateStyles.CompactDate,
  space_separator: hypercast.DateStyles.SpaceSeparator,
  hour_offset: hypercast.DateStyles.HourOffset,
}

const NUMERIC = new Set([
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
])

const declarations = new Map<string, Constraints>()
// A plan entry's constraints as HyperCast's own declaration for the door it names, or
// `undefined`.
function constraintsOf(entry: Json): Constraints | undefined {
  if (entry.constraints === undefined) return undefined
  const key = JSON.stringify([entry.door, entry.constraints])
  const cached = declarations.get(key)
  if (cached) return cached
  const { rounding, ...rest } = entry.constraints
  const name = `${entry.door[0].toUpperCase()}${entry.door.slice(1)}` as 'I32'
  const declared = hypercast.Constraints.declare({
    door: hypercast.Door[name],
    ...rest,
    ...(rounding === undefined
      ? {}
      : {
          rounding:
            hypercast.Rounding[`${rounding[0].toUpperCase()}${rounding.slice(1)}` as 'Floor'],
        }),
  })
  assert.ok(declared.ok, `${key}: ${JSON.stringify(declared)}`)
  declarations.set(key, declared.value)
  return declared.value
}

// A plan entry names its door as HyperCast's corpus names its types; Column's factories carry
// the same names, in camel case, and an entry that declares constraints is `Column.checked`,
// whose door is the declaration's. Every door can carry a format (a suggested plan's text
// columns do), which only a number's reads: a factory takes one for a number door only, so an
// entry's format on another door is checked to be one HyperCast declares, and read no further.
function columnOf(entry: Json): ColumnT {
  const { door, ordinal } = entry
  let column: ColumnT
  const declared = constraintsOf(entry)
  if (declared !== undefined) {
    column = Column.checked(ordinal, declared, formatOf(entry.format))
    assert.ok(column.checked && column.door === camel(door), JSON.stringify(entry))
  } else
    switch (door) {
      case 'unix':
        column = Column.unix(ordinal, entry.precision)
        break
      case 'excel_serial':
        column = Column.excelSerial(ordinal, entry.epoch)
        break
      case 'date_ordered':
        column = Column.dateOrdered(ordinal, entry.order)
        break
      case 'datetime':
        column = Column.datetime(ordinal, entry.order)
        break
      case 'bool':
      case 'uuid':
      case 'timestamp':
      case 'date':
      case 'time':
      case 'duration':
      case 'text':
        formatOf(entry.format)
        column = Column[door as 'text'](ordinal)
        break
      default:
        assert.ok(NUMERIC.has(door), door)
        column = Column[door as 'i32'](ordinal, formatOf(entry.format))
        assert.ok(!column.checked, door)
    }
  if (entry.tokens !== undefined) column = column.withTokens(entry.tokens)
  if (entry.options !== undefined) {
    const options = entry.options
    let months: InstanceType<typeof hypercast.MonthNames> | undefined
    if (options.months !== undefined) {
      const declared = hypercast.MonthNames.declare(options.months)
      assert.ok(declared.ok)
      months = declared.value
    }
    const declared = hypercast.DateOptions.declare({
      flags: options.flags.reduce((all: number, flag: string) => all | DATE_FLAGS[flag], 0),
      pivot: options.pivot,
      months,
    })
    assert.ok(declared.ok, JSON.stringify(declared))
    column = column.withDateOptions(declared.value)
  }
  assert.ok(column.valid, JSON.stringify(entry))
  return column
}

// Whether HyperCast's plain door says what this column's says, with nothing declared beside it.
function plainVerdict(entry: Json): boolean {
  return entry.options === undefined && entry.tokens === undefined
}

function inputOf(c: Json): Uint8Array {
  return c.input_hex !== undefined ? hex(c.input_hex) : encoder.encode(c.input)
}

const ENCODINGS: Record<string, Dialect['encoding']> = {
  'windows-1252': 'windows1252',
  'latin-1': 'latin1',
  'utf-16le': 'utf16le',
}

function dialectOf(entry: Json): Dialect {
  const dialect: Dialect = {
    separator: entry.separator,
    quoting: entry.quoting,
    hasHeader: entry.has_header,
    skipBlankLines: entry.skip_blank_lines,
    skip: entry.skip,
    headerRow: entry.header_row,
    shortRows: entry.short_rows,
    longRows: entry.long_rows,
    tokens: entry.tokens,
  }
  if (entry.encoding !== undefined) {
    dialect.encoding = ENCODINGS[entry.encoding]
    assert.ok(dialect.encoding !== undefined, entry.encoding)
  }
  return dialect
}

function timeOf(nanosOfDay: number) {
  const seconds = Math.floor(nanosOfDay / 1e9)
  return {
    hour: Math.floor(seconds / 3600),
    minute: Math.floor(seconds / 60) % 60,
    second: seconds % 60,
    nanos: nanosOfDay % 1e9,
  }
}

// The value the corpus says a cell that cast must hold, in @skunkwerkx/hypercast's types.
function expectedValue(door: string, cell: Json): unknown {
  switch (door) {
    case 'i64':
    case 'u64':
      return BigInt(cell.value)
    case 'f32':
      return Math.fround(cell.value)
    case 'uuid':
      return cell.value.replace(/^(.{8})(.{4})(.{4})(.{4})(.{12})$/, '$1-$2-$3-$4-$5')
    case 'timestamp':
    case 'unix':
    case 'excel_serial':
    case 'duration':
      return { seconds: cell.seconds, nanos: cell.nanos }
    case 'date':
    case 'date_ordered':
      return { year: cell.year, month: cell.month, day: cell.day }
    case 'datetime':
      return { year: cell.year, month: cell.month, day: cell.day, ...timeOf(cell.nanos_of_day) }
    case 'time':
      return timeOf(cell.nanos)
    case 'text':
      return cell.text
    default:
      return cell.value
  }
}

// What HyperCast's own checked door says of a cell's text under the column's declaration.
function judgeChecked(entry: Json, raw: Uint8Array): CheckedVerdict<unknown> {
  const declared = constraintsOf(entry) as Constraints
  return entry.door === 'text'
    ? hypercast.castText(raw, declared)
    : hypercast.castChecked(raw, declared, formatOf(entry.format))
}

// What HyperCast's own package says of a cell's text through the column's door.
function judge(entry: Json, raw: Uint8Array): Verdict<unknown> {
  switch (entry.door) {
    case 'unix':
      return hypercast.castUnix(raw, entry.precision)
    case 'excel_serial':
      return hypercast.castExcelSerial(raw, entry.epoch)
    case 'date_ordered':
      return hypercast.castDate(raw, entry.order)
    case 'datetime':
      return hypercast.castDatetime(raw, entry.order)
    case 'bool':
      return hypercast.castBool(raw)
    case 'uuid':
      return hypercast.castUuid(raw)
    case 'timestamp':
      return hypercast.castTimestamp(raw)
    case 'date':
      return hypercast.castDate(raw)
    case 'time':
      return hypercast.castTime(raw)
    case 'duration':
      return hypercast.castDuration(raw)
    default: {
      const name = `cast${entry.door[0].toUpperCase()}${entry.door.slice(1)}` as 'castI32'
      return hypercast[name](raw, formatOf(entry.format))
    }
  }
}

const REASONS: Record<string, string> = {
  empty: 'empty',
  malformed: 'malformed',
  out_of_range: 'outOfRange',
}

function assertCell(
  label: string,
  entry: Json,
  batch: Batch,
  column: number,
  row: number,
  cell: Json,
  judged: boolean,
  notes: boolean,
): void {
  if (entry.constraints !== undefined) {
    assertCheckedCell(label, entry, batch, column, row, cell, judged, notes)
    return
  }
  const verdict = batch.get<Value>(column, row)
  const raw = batch.raw(column, row)
  assertNotes(label, batch, column, row, cell, notes)
  if (verdict.ok) {
    assert.equal(cell.expect, 'ok', `${label}: unexpectedly cast to ${String(verdict.value)}`)
    assert.deepEqual(verdict.value, expectedValue(entry.door, cell), label)
    if (entry.door === 'decimal') {
      // Equality of text pins the canonical scale: "1.10" never passes for "1.1".
      const text = verdict.value as string
      assert.equal(text.startsWith('-'), cell.negative, label)
      const digits = text.replace(/^-/, '')
      const point = digits.indexOf('.')
      assert.equal(point < 0 ? 0 : digits.length - point - 1, cell.scale, label)
      assert.equal(BigInt(digits.replace('.', '')), BigInt(cell.magnitude), label)
    }
    if (entry.door === 'text') assert.deepEqual(raw, encoder.encode(cell.text), label)
  } else {
    assertFault(label, verdict.fault, raw, cell)
  }
  // HyperCast is the judge: the cell is what its own package makes of the same text. A cell no
  // door was asked about is the read's word, not HyperCast's.
  if (judged && entry.door !== 'text' && cell.origin === undefined) {
    assert.deepEqual(verdict, judge(entry, raw), `${label}: disagrees with hypercast`)
  }
}

// A cell's fault, held to the corpus.
function assertFault(label: string, fault: Fault, raw: Uint8Array, cell: Json): void {
  const { reason, offset, length } = fault
  assert.notEqual(cell.expect, 'ok', `${label}: faulted ${reason}`)
  assert.notEqual(cell.expect, 'violation', `${label}: faulted ${reason}`)
  assert.equal(reason, REASONS[cell.expect], label)
  assert.ok(offset + length <= raw.length, `${label}: span ${offset}+${length} past ${raw.length}`)
  if (cell.fault !== undefined) {
    assert.deepEqual([offset, length], cell.fault, `${label}: span`)
    if (cell.raw_hex !== undefined) assert.deepEqual(raw, hex(cell.raw_hex), `${label}: raw`)
    else assert.equal(decoder.decode(raw), cell.raw, `${label}: raw`)
  }
  if (cell.expect === 'empty') {
    assert.deepEqual([offset, length], [0, 0], label)
    if (cell.origin === undefined) assert.equal(raw.length, 0, label)
  }
}

// What failed in a cell, and where its verdict came from.
function assertNotes(
  label: string,
  batch: Batch,
  column: number,
  row: number,
  cell: Json,
  notes: boolean,
): void {
  const detail = notes && cell.detail !== undefined ? camel(cell.detail) : null
  assert.equal(batch.detail(column, row), detail, `${label}: detail`)
  const origin = notes ? (cell.origin ?? 'cell') : 'cell'
  assert.equal(batch.origin(column, row), origin, `${label}: origin`)
}

// A float to its exact bits, as the corpus writes them: a float32 door's as a float32.
function bitsOf(door: string, value: number): string {
  const bytes = Buffer.alloc(door === 'f32' ? 4 : 8)
  if (door === 'f32') bytes.writeFloatBE(value)
  else bytes.writeDoubleBE(value)
  return `0x${bytes.toString('hex')}`
}

// A checked cell's value, passed or as cast: a float to its exact bits, anything else as a
// cast one is held.
function assertCheckedValue(label: string, door: string, value: unknown, cell: Json): void {
  if (door === 'f32' || door === 'f64') {
    assert.equal(typeof value, 'number', label)
    assert.equal(bitsOf(door, value as number), cell.bits, `${label}: ${String(value)}`)
    return
  }
  assert.deepEqual(value, expectedValue(door, cell), label)
}

// A cell of a column that declares constraints, through `getChecked`: what passed, a violation of
// a constraint the column declares, or the cell's fault. The plain getters answer HyperCast's
// contract violation for the column.
function assertCheckedCell(
  label: string,
  entry: Json,
  batch: Batch,
  column: number,
  row: number,
  cell: Json,
  judged: boolean,
  notes: boolean,
): void {
  const contract = { ok: false, fault: { reason: 'malformed', offset: 0, length: 0 } }
  assert.deepEqual(batch.get(column, row), contract, `${label}: get read a constrained column`)
  const verdict = batch.getChecked<Value>(column, row)
  const raw = batch.raw(column, row)
  if ('fault' in verdict) {
    assertNotes(label, batch, column, row, cell, notes)
    assertFault(label, verdict.fault, raw, cell)
  } else {
    // A cell that cast has no detail, and came from the cell itself.
    assert.equal(batch.detail(column, row), null, `${label}: detail`)
    assert.equal(batch.origin(column, row), 'cell', `${label}: origin`)
    if (verdict.ok) {
      assert.equal(cell.expect, 'ok', `${label}: passed with ${inspect(verdict)}`)
      assert.deepEqual(
        [verdict.adjusted, verdict.enumIndex],
        [cell.adjusted, cell.enum_index ?? null],
        label,
      )
      const index = verdict.enumIndex
      if (index !== null && entry.door === 'text') {
        // The entry as declared: the declaration's own string.
        const declared = constraintsOf(entry) as Constraints
        assert.equal(verdict.value, declared.enum?.[index], label)
      }
      assertCheckedValue(label, entry.door, verdict.value, cell)
    } else {
      const { keyword, value, offset, length } = verdict.violation
      assert.equal(cell.expect, 'violation', `${label}: broke ${keyword}`)
      assert.equal(keyword, cell.keyword, label)
      assert.equal(decoder.decode(raw), cell.raw, `${label}: raw text`)
      assert.deepEqual([offset, length], cell.fault, `${label}: span`)
      assertCheckedValue(label, entry.door, value, cell)
    }
  }
  if (judged && cell.origin === undefined) {
    assert.deepEqual(verdict, judgeChecked(entry, raw), `${label}: disagrees with hypercast`)
  }
}

// An entry of a batch's faults list in the corpus's shape, its row counted across the read.
function problemJson(problem: Problem, base: number): Json {
  const entry: Json = { row: base + problem.row, line: problem.line }
  if (problem.planColumn === null) {
    assert.ok(problem.sourceColumn === null && problem.reason === null)
    assert.equal(problem.keyword, null)
    assert.deepEqual([problem.offset, problem.length], [0, 0])
    entry.expected = problem.expected
    entry.found = problem.found
  } else if (problem.keyword !== null) {
    // A violation: the cell cast, so there's no reason, and the span is the judged text's.
    assert.ok(problem.sourceColumn !== null && problem.reason === null)
    assert.deepEqual([problem.expected, problem.found], [0, 0])
    entry.plan_column = problem.planColumn
    entry.source_column = problem.sourceColumn
    entry.reason = 'violation'
    entry.keyword = problem.keyword
    entry.fault = [problem.offset, problem.length]
  } else {
    assert.ok(problem.sourceColumn !== null && problem.reason !== null)
    assert.deepEqual([problem.expected, problem.found], [0, 0])
    entry.plan_column = problem.planColumn
    entry.source_column = problem.sourceColumn
    entry.reason = snake(problem.reason)
    if (problem.reason !== 'empty') entry.fault = [problem.offset, problem.length]
    else assert.deepEqual([problem.offset, problem.length], [0, 0])
  }
  if (problem.detail !== null) entry.detail = snake(problem.detail)
  if (problem.origin !== 'cell') entry.origin = problem.origin
  return entry
}

// What a read with notes off lists of `problems`: no formula with no cached value, and no
// detail or origin on the rest.
function problemsWithoutNotes(problems: Json[]): Json[] {
  return problems
    .filter((problem) => problem.origin !== 'uncached')
    .map(({ detail: _detail, origin: _origin, ...rest }) => rest)
}

// Holds one batch to the corpus rows from `seen` on, and adds its faults list to `problems`;
// returns the rows seen after it.
function assertBatch(
  label: string,
  plan: Json[],
  rows: Json[][],
  seen: number,
  batch: Batch,
  batchRows: number,
  problems: Json[],
  judged: boolean,
  notes: boolean,
  numbers?: number[],
): number {
  assert.ok(batch.rows >= 1 && batch.rows <= batchRows, `${label}: ${batch.rows} rows`)
  assert.ok(seen + batch.rows <= rows.length, `${label}: more rows than the corpus lists`)
  assert.equal(batch.columnCount, plan.length, label)
  assert.deepEqual(
    batch.doors,
    plan.map((entry) => camel(entry.door)),
    label,
  )
  problems.push(...batch.faults().map((problem) => problemJson(problem, seen)))
  for (let column = 0; column < plan.length; column++) {
    const whole = batch.column(column)
    assert.equal(whole.length, batch.rows, label)
    let faults = 0
    for (let row = 0; row < batch.rows; row++) {
      const at = `${label}, row ${seen + row}, column ${column}`
      assert.equal(rows[seen + row].length, plan.length, `${at}: a corpus row of another width`)
      const cell = rows[seen + row][column]
      assertCell(
        at,
        plan[column],
        batch,
        column,
        row,
        cell,
        judged && plainVerdict(plan[column]),
        notes,
      )
      const verdict = batch.get(column, row)
      assert.deepEqual(whole[row], verdict, at)
      assert.equal(batch.rawString(column, row), decoder.decode(batch.raw(column, row)), at)
      if ('fault' in batch.getChecked(column, row)) faults++
    }
    assert.equal(batch.faultCount(column), faults, label)
  }
  let index = 0
  for (const row of batch) {
    assert.equal(row.index, index, label)
    assert.equal(row.line, batch.line(index), label)
    if (numbers !== undefined) assert.equal(row.line, numbers[seen + index], `${label}: line`)
    for (let column = 0; column < plan.length; column++) {
      assert.deepEqual(row.get(column), batch.get(column, index), label)
      assert.deepEqual(row.getChecked(column), batch.getChecked(column, index), label)
      assert.deepEqual(row.raw(column), batch.raw(column, index), label)
      assert.equal(row.detail(column), batch.detail(column, index), label)
      assert.equal(row.origin(column), batch.origin(column, index), label)
    }
    index++
  }
  assert.equal(index, batch.rows, label)
  return seen + batch.rows
}

function assertFailure(label: string, error: Json, expected: Json): void {
  assert.equal(error.kind, 'structure', `${label}: ${JSON.stringify(error)}`)
  const failure = error.failure
  assert.ok(failure !== undefined, label)
  // The corpus names a kind as the core does, in snake case; the whole failure is held to
  // it, an absent `expected` and `found` being 0.
  assert.equal(failure.kind, camel(expected.kind), label)
  assert.deepEqual(
    [failure.record, failure.line, failure.byte, failure.expected, failure.found],
    [expected.record, expected.line, expected.byte, expected.expected ?? 0, expected.found ?? 0],
    label,
  )
}

// Reads a bound reader to its end (or its failure) against the corpus rows.
function replay(
  label: string,
  c: Json,
  reader: Reader,
  batchRows: number,
  judged: boolean,
  notes: boolean,
): void {
  assert.deepEqual(unknownKeys(c, c.file !== undefined), [], label)
  let seen = 0
  let stopped: Outcome<Batch | null> | undefined
  const problems: Json[] = []
  for (const read of reader) {
    if (!read.ok) {
      stopped = read
      break
    }
    seen = assertBatch(
      label,
      c.plan,
      c.rows,
      seen,
      read.value,
      batchRows,
      problems,
      judged,
      notes,
      c.numbers,
    )
  }
  assert.equal(seen, c.rows.length, `${label}: ${seen} rows, the corpus lists ${c.rows.length}`)
  const wanted = notes ? c.problems : problemsWithoutNotes(c.problems)
  assert.deepEqual(problems, wanted, `${label}: problems`)
  if (c.failure === undefined) {
    assert.equal(stopped, undefined, `${label}: ${JSON.stringify(stopped)}`)
    assert.deepEqual(reader.read(), { ok: true, value: null }, label)
  } else {
    assert.ok(stopped !== undefined && !stopped.ok, `${label}: no failure`)
    assertFailure(label, stopped.error, c.failure)
    // A structural failure is final: the same one, again.
    assert.deepEqual(reader.read(), stopped, label)
  }
  // Closing gives the buffers back now; a read after it is the caller's mistake, answered.
  assert.deepEqual(reader.close(), { ok: true, value: null }, label)
  const after = reader.read()
  assert.ok(!after.ok && after.error.kind === 'closed', label)
}

function opened<T>(label: string, outcome: Outcome<T>): T {
  assert.ok(outcome.ok, `${label}: ${outcome.ok ? '' : JSON.stringify(outcome.error)}`)
  return outcome.value
}

// A case whose header record itself fails comes from opening, with no rows.
function openedOrFailed<T>(label: string, c: Json, outcome: Outcome<T>): T | undefined {
  if (!outcome.ok && c.failure !== undefined && outcome.error.kind === 'structure') {
    assert.deepEqual([c.rows, c.problems], [[], []], label)
    assertFailure(label, outcome.error, c.failure)
    return undefined
  }
  return opened(label, outcome)
}

const delimited = load('delimited.json')
const workbooks = load('workbook.json')

test('the corpus is the whole contract, and every key in it is read', () => {
  assert.equal(delimited.length, 137)
  assert.equal(workbooks.length, 404)
  assert.equal(new Set(workbooks.map((c) => c.file)).size, 42)
  const doors = new Set(delimited.flatMap((c) => c.plan.map((entry: Json) => entry.door)))
  assert.equal(doors.size, 22, 'a door the corpus never opens')
  assertKeysAreKnown(delimited, false)
  assertKeysAreKnown(workbooks, true)
})

const ROWS = [1, 2, 4096]
const BUFFERS = [1, 5, undefined]

test('delimited.json, from memory, a string and a file, every batch size and buffer', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'hypertabular-corpus-'))
  try {
    for (const [at, c] of delimited.entries()) {
      const dialect = dialectOf(c.dialect)
      const bytes = inputOf(c)
      const file = path.join(dir, `${at}.txt`)
      writeFileSync(file, bytes)
      for (const batchRows of ROWS) {
        for (const bufferBytes of BUFFERS) {
          const options = { batchRows, bufferBytes }
          const sources: [
            string,
            (plan: ColumnT[]) => Outcome<InstanceType<typeof DelimitedReader>>,
          ][] = [
            ['memory', (plan) => DelimitedReader.fromBytes(bytes, dialect, plan, options)],
            ['a file', (plan) => DelimitedReader.open(file, dialect, plan, options)],
          ]
          if (c.input !== undefined) {
            sources.push([
              'a string',
              (plan) => DelimitedReader.fromBytes(c.input, dialect, plan, options),
            ])
          }
          for (const [where, open] of sources) {
            const label = `${c.name} (${where}, ${batchRows} rows, buffer ${bufferBytes})`
            const plan = c.plan.map(columnOf)
            if (pathsUnreachable && where === 'a file') {
              unreachable(label, open(plan))
              continue
            }
            const reader = openedOrFailed(label, c, open(plan))
            if (reader === undefined) continue
            assert.deepEqual(reader.header?.names ?? null, c.header, label)
            assert.equal(reader.isBound, true, label)
            replay(label, c, reader, batchRows, true, true)
          }
        }
      }
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('delimited.json, header first, and with notes off', () => {
  for (const c of delimited) {
    const dialect = dialectOf(c.dialect)
    for (const batchRows of ROWS) {
      const label = `${c.name} (header first, ${batchRows} rows)`
      const reader = openedOrFailed(
        label,
        c,
        DelimitedReader.fromBytes(inputOf(c), dialect, null, { batchRows }),
      )
      if (reader === undefined) continue
      assert.deepEqual(reader.header?.names ?? null, c.header, label)
      if (c.header !== null && c.header.length > 0) {
        assert.equal(reader.columnCount, c.header.length, label)
      }
      assert.equal(reader.isBound, false, label)
      // Reading before a plan is bound is the caller's mistake, and not a lasting one.
      const early = reader.read()
      assert.ok(!early.ok && early.error.kind === 'unbound', label)
      const plan = c.plan.map(columnOf)
      assert.deepEqual(reader.bind(plan), { ok: true, value: null }, label)
      const again = reader.bind(plan)
      assert.ok(!again.ok && again.error.kind === 'alreadyBound', label)
      replay(label, c, reader, batchRows, true, true)

      const quiet = `${c.name} (notes off, ${batchRows} rows)`
      const silent = openedOrFailed(
        quiet,
        c,
        DelimitedReader.fromBytes(inputOf(c), dialect, c.plan.map(columnOf), {
          batchRows,
          notes: false,
        }),
      )
      if (silent !== undefined) replay(quiet, c, silent, batchRows, true, false)
    }
  }
})

// Every batch of a read, its faults asked once at the read and again after every later read.
test('a batch held past the next read still answers for itself', () => {
  for (const c of delimited) {
    for (const batchRows of [1, 3]) {
      const open = () =>
        DelimitedReader.fromBytes(inputOf(c), dialectOf(c.dialect), c.plan.map(columnOf), {
          batchRows,
        })
      const first = open()
      if (!first.ok) continue
      const atOnce = [...first.value]
        .filter((read) => read.ok)
        .map((read) => (read.ok ? read.value.faults() : []))
      const second = open()
      assert.ok(second.ok)
      const held = [...second.value].flatMap((read) => (read.ok ? [read.value] : []))
      second.value.close()
      assert.deepEqual(
        held.map((batch) => batch.faults()),
        atOnce,
        c.name,
      )
      // A fresh array each call, the caller's own.
      if (held.length > 0) assert.notEqual(held[0].faults(), held[0].faults())
    }
  }
})

function sheetOptionsOf(
  settings: Json,
  batchRows: number,
  notes: boolean,
  range: 'text' | 'object',
): SheetOptions {
  const options: SheetOptions = {
    hasHeader: settings.has_header,
    skipEmptyRows: settings.skip_empty_rows,
    batchRows,
    skip: settings.skip,
    headerRow: settings.header_row,
    tokens: settings.tokens,
    notes,
  }
  if (settings.range !== undefined) {
    options.range =
      range === 'text' ? settings.range : opened(settings.range, Range.parse(settings.range))
  }
  return options
}

test('workbook.json, from bytes and a path, by index and by name, header first and bound up front', async () => {
  const books = new Map<string, Uint8Array>()
  for (const c of workbooks) {
    // Under the WASI fallback what a case dropped is finalized only when the event loop turns
    // (README): without a turn per case the whole corpus is held at once, past the default heap.
    await new Promise(setImmediate)
    const file = path.join(CORPUS, c.file)
    let bytes = books.get(file)
    if (bytes === undefined) {
      bytes = new Uint8Array(readFileSync(file))
      books.set(file, bytes)
    }
    if (c.sheet === undefined) {
      // A package the core refuses: every way of opening it gives the one failure.
      const refusals: Outcome<unknown>[] = [Workbook.fromBytes(bytes)]
      if (pathsUnreachable) unreachable(c.name, Workbook.open(file))
      else refusals.push(Workbook.open(file))
      for (const refused of refusals) {
        assert.ok(!refused.ok, c.name)
        assertFailure(c.name, refused.error, c.failure)
      }
      continue
    }
    const sources = [opened(c.name, Workbook.fromBytes(bytes))]
    if (!pathsUnreachable) sources.push(opened(c.name, Workbook.open(file)))
    for (const [source, book] of sources.entries()) {
      assert.equal(book.format, c.format, c.name)
      assert.equal(book.dateSystem, c.epoch, c.name)
      assert.deepEqual(book.sheets, c.sheets, c.name)
      const index = c.sheet as number
      const name = c.sheets[index].name
      const byName = book.sheets.findIndex((sheet) => sheet.name === name) === index
      for (const batchRows of ROWS) {
        const range = batchRows === 2 ? 'object' : 'text'
        const options = sheetOptionsOf(c.options, batchRows, true, range)
        const ways: (number | string)[] = byName ? [index, name] : [index]
        for (const which of ways) {
          for (const headerFirst of [false, true]) {
            const label = `${c.name} (${source === 0 ? 'bytes' : 'path'}, ${JSON.stringify(which)}, ${batchRows} rows${headerFirst ? ', header first' : ''})`
            const sheet = opened(
              label,
              book.sheet(which, options, headerFirst ? null : c.plan.map(columnOf)),
            )
            assert.deepEqual(sheet.header?.names ?? null, c.header, label)
            assert.equal(sheet.dateSystem, c.epoch, label)
            if (headerFirst) {
              assert.equal(sheet.isBound, false, label)
              const early = sheet.read()
              assert.ok(!early.ok && early.error.kind === 'unbound', label)
              assert.deepEqual(sheet.bind(c.plan.map(columnOf)), { ok: true, value: null }, label)
              const again = sheet.bind(c.plan.map(columnOf))
              assert.ok(!again.ok && again.error.kind === 'alreadyBound', label)
            }
            replay(label, c, sheet, batchRows, false, true)
          }
        }
        if (source === 0) {
          const quiet = `${c.name} (notes off, ${batchRows} rows)`
          const sheet = opened(
            quiet,
            book.sheet(
              index,
              sheetOptionsOf(c.options, batchRows, false, range),
              c.plan.map(columnOf),
            ),
          )
          replay(quiet, c, sheet, batchRows, false, false)
        }
      }
    }
  }
})

// A report in the corpus's shape. An ambiguous column's `param` is the set of its candidates,
// and a resolved number's is left at 0, since a report names the format the plan uses and
// can't say what `param` was.
const DOT = hypercast.NumFormat.invariant().packed
const COMMA = (() => {
  const declared = hypercast.NumFormat.declare({
    decimalSep: ',',
    groupSep: '.',
    flags: hypercast.NumStyles.All,
  })
  assert.ok(declared.ok)
  return declared.value
})()
const same = (a: Uint8Array, b: Uint8Array) => Buffer.from(a).equals(Buffer.from(b))

function suggestionJson(found: Suggestion): Json[] {
  return found.report.map((column) => {
    let param = 0
    if (column.candidates.length > 0) {
      for (const candidate of column.candidates) {
        if (candidate.kind === 'dateOrder') {
          assert.ok(candidate.order !== null && candidate.format === null)
          param |= 1 << candidate.order
        } else {
          assert.ok(candidate.format !== null && candidate.order === null)
          if (same(candidate.format.packed, DOT)) param |= 2
          else if (same(candidate.format.packed, COMMA.packed)) param |= 4
          else assert.fail('a candidate notation that is neither')
        }
      }
    } else if (column.door === 'dateOrdered' || column.door === 'datetime') {
      assert.ok(column.declared !== null)
      param = column.declared
    }
    return {
      door: snake(column.door),
      param,
      status: column.status,
      matched: column.matched,
      sampled: column.sampled,
    }
  })
}

function reported(columns: Json[]): Json[] {
  return columns.map((column) => {
    const dated = column.door === 'date_ordered' || column.door === 'datetime'
    return column.status === 'resolved' && !dated ? { ...column, param: 0 } : { ...column }
  })
}

// Each resolved number is planned and reported in the format the corpus's `param` says, or the
// one declared; anything else reports none.
function assertNotation(label: string, found: Suggestion, columns: Json[], declared?: NumFormat) {
  for (const [at, column] of found.report.entries()) {
    const numeric = NUMERIC.has(column.door)
    if (column.status === 'resolved' && numeric) {
      const wanted = declared ?? (columns[at].param === 4 ? COMMA : undefined)
      const packed = wanted?.packed ?? DOT
      assert.ok(column.format !== null, label)
      assert.ok(same(column.format.packed, packed), label)
      assert.ok(same(found.plan[at].format?.packed ?? new Uint8Array(), packed), label)
    } else {
      assert.equal(column.format, null, label)
    }
  }
}

test('every suggestion is what the corpus says, and its plan reads back', () => {
  const suggesting = delimited.filter((c) => c.suggest !== undefined)
  assert.equal(suggesting.length, 19)
  const dir = mkdtempSync(path.join(tmpdir(), 'hypertabular-suggest-'))
  try {
    for (const c of suggesting) {
      const label = c.name
      const expected = c.suggest
      const data = inputOf(c)
      const dialect = dialectOf(c.dialect)
      const declared = formatOf(expected.format)
      const asked = { sampleRows: expected.sample_rows, format: declared }
      const found = opened(label, DelimitedReader.suggestBytes(data, dialect, asked))
      assert.deepEqual(suggestionJson(found), reported(expected.columns), label)
      assert.equal(found.truncated, expected.truncated, label)
      assertNotation(label, found, expected.columns, declared)
      const file = path.join(dir, 'case.csv')
      writeFileSync(file, data)
      if (pathsUnreachable) unreachable(label, DelimitedReader.suggest(file, dialect, asked))
      else {
        const again = opened(label, DelimitedReader.suggest(file, dialect, asked))
        assert.deepEqual(suggestionJson(again), suggestionJson(found), label)
        assert.equal(again.truncated, found.truncated, label)
      }
      // The plan reads back refusing nothing it resolved in the sample.
      const sample = expected.sample_rows ?? c.rows.length
      const reader = opened(label, DelimitedReader.fromBytes(data, dialect, found.plan))
      let read = 0
      for (const outcome of reader) {
        if (!outcome.ok) {
          assert.ok(read >= sample, label)
          break
        }
        const refused = outcome.value.faults().filter((problem) => {
          if (problem.planColumn === null) return false
          const column = found.report[problem.planColumn]
          return (
            column.status === 'resolved' && column.door !== 'text' && problem.row + read < sample
          )
        })
        assert.deepEqual(refused, [], label)
        read += outcome.value.rows
      }
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
  // A suggestion fails only where a read of the case fails.
  let planned = 0
  let failed = 0
  for (const c of delimited) {
    const found = DelimitedReader.suggestBytes(inputOf(c), dialectOf(c.dialect))
    if (found.ok) planned++
    else {
      failed++
      assert.ok(c.failure !== undefined && found.error.kind === 'structure', c.name)
    }
  }
  assert.ok(planned > 80 && failed > 0, `${planned} ${failed}`)
})

test('every sheet suggestion is what the corpus says, and its plan reads back', () => {
  const suggesting = workbooks.filter((c) => c.suggest !== undefined)
  assert.equal(suggesting.length, 15)
  for (const c of suggesting) {
    const label = c.name
    const expected = c.suggest
    const declared = formatOf(expected.format)
    const book = opened(
      label,
      Workbook.fromBytes(new Uint8Array(readFileSync(path.join(CORPUS, c.file)))),
    )
    const options = sheetOptionsOf(c.options, 4096, true, 'text')
    const sheet = opened(label, book.sheet(c.sheet, options))
    const found = opened(label, sheet.suggest({ format: declared }))
    assert.deepEqual(suggestionJson(found), reported(expected.columns), label)
    assert.equal(found.truncated, expected.truncated, label)
    assertNotation(label, found, expected.columns, declared)
    for (const outcome of opened(label, book.sheet(c.sheet, options, found.plan))) {
      assert.ok(outcome.ok, label)
      const refused = outcome.value
        .faults()
        .filter((problem) => problem.reason === 'malformed' || problem.reason === 'outOfRange')
      assert.deepEqual(refused, [], label)
    }
  }
})
