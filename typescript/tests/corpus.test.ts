// Replays the shared conformance corpus (corpus/delimited.json and corpus/workbook.json at the
// repository root), the files the Rust binding and every other binding replay, through this
// one. Delimited text is read from memory and from a file, in batches of one row, two and
// many, through an input buffer of one byte, five and the default, and header first: opened
// without a plan, the plan bound once the header is read. How the input is cut up is the
// binding's business and must not change the answer. Every workbook case is read header
// first and with the plan up front, by index and (where it is unique) by name, in batches of
// one row and of many. Every cell is held to the corpus three ways: as `get` gives it, as the
// whole-column `column` gives it, and as its row gives it; and every delimited cell is held to
// what `@skunkwerkx/hypercast`'s own door says of its raw text, HyperCast being the judge.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } =
  require('node:fs') as typeof import('node:fs')
const { tmpdir }: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const hypercast = require('@skunkwerkx/hypercast') as typeof import('@skunkwerkx/hypercast')
const api = require('../index.js') as typeof import('../index.js')
const { Column, DelimitedReader, Workbook } = api
type Batch = import('../index.js').Batch
type Outcome<T> = import('../index.js').Outcome<T>
type Value = import('../index.js').Value
type Verdict<T> = import('../index.js').Verdict<T>
type Reader = import('../index.js').DelimitedReader | import('../index.js').Sheet

interface Cell {
  expect: 'ok' | 'empty' | 'malformed' | 'out_of_range'
  fault?: [number, number]
  raw?: string
  value?: unknown
  text?: string
  magnitude?: string
  negative?: boolean
  scale?: number
  seconds?: number
  nanos?: number
  year?: number
  month?: number
  day?: number
  nanos_of_day?: number
}
interface PlanEntry {
  door: string
  ordinal: number
  precision?: number
  order?: number
  epoch?: number
  format?: Record<string, string | number>
}
interface Failure {
  kind: string
  record: number
  line: number
  byte: number
  expected?: number
  found?: number
}
interface DelimitedCase {
  name: string
  input: string
  dialect: { separator: string; quoting: boolean; has_header: boolean; skip_blank_lines: boolean }
  plan: PlanEntry[]
  header: string[] | null
  rows: Cell[][]
  failure?: Failure
}
interface WorkbookCase {
  name: string
  file: string
  format?: string
  epoch?: number
  sheets?: { name: string; hidden: boolean }[]
  sheet?: number
  options?: { has_header: boolean; skip_empty_rows: boolean }
  plan?: PlanEntry[]
  header?: string[] | null
  numbers?: number[]
  rows?: Cell[][]
  failure?: Failure
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
// HYPERTABULAR_CORPUS points the replay at another copy (the negative control corrupts one).
const CORPUS = process.env.HYPERTABULAR_CORPUS ?? corpusDirectory()

// An integer literal past 2^53 keeps its exact digits (as a string, for the test to make a
// bigint of): JSON.parse alone would round 18446744073709551615 to a double.
function load<T>(name: string): T[] {
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

const encoder = new TextEncoder()
const decoder = new TextDecoder()
const reasons = { empty: 'empty', malformed: 'malformed', out_of_range: 'outOfRange' } as const
const failureKinds: Record<string, string> = {
  unclosed_quote: 'unclosedQuote',
  column_count: 'columnCount',
  row_too_long: 'rowTooLong',
  not_a_zip: 'notAZip',
  container: 'container',
  encrypted: 'encrypted',
  method: 'method',
  missing_part: 'missingPart',
  xml: 'xml',
  deflate: 'deflate',
  not_a_workbook: 'notAWorkbook',
  shared_string: 'sharedString',
  too_large: 'tooLarge',
  iwa: 'iwa',
}

const formats = new Map<string, InstanceType<typeof hypercast.NumFormat>>()
function formatOf(entry: PlanEntry): InstanceType<typeof hypercast.NumFormat> | undefined {
  if (entry.format === undefined) return undefined
  const key = JSON.stringify(entry.format)
  const cached = formats.get(key)
  if (cached) return cached
  const f = entry.format
  const declared = hypercast.NumFormat.declare({
    decimalSep: f.decimal_sep as string,
    groupSep: f.group_sep as string,
    flags: f.flags as number,
    currency: f.currency as string | undefined,
  })
  assert.ok(declared.ok, `${key}: ${declared.ok ? '' : declared.message}`)
  formats.set(key, declared.value)
  return declared.value
}

// A plan entry names its door as HyperCast's corpus names its types; Column's factories carry
// the same names, in camel case.
function columnOf(entry: PlanEntry): InstanceType<typeof Column> {
  const { door, ordinal } = entry
  switch (door) {
    case 'unix':
      return Column.unix(ordinal, entry.precision as number)
    case 'excel_serial':
      return Column.excelSerial(ordinal, entry.epoch as number)
    case 'date_ordered':
      return Column.dateOrdered(ordinal, entry.order as number)
    case 'datetime':
      return Column.datetime(ordinal, entry.order as number)
    case 'bool':
    case 'uuid':
    case 'timestamp':
    case 'date':
    case 'time':
    case 'duration':
    case 'text':
      return Column[door](ordinal)
    default: {
      const numeric = door as 'i8' | 'i16' | 'i32' | 'i64' | 'u8' | 'u16' | 'u32' | 'u64'
      return Column[numeric](ordinal, formatOf(entry))
    }
  }
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
function expectedValue(door: string, cell: Cell): unknown {
  switch (door) {
    case 'i64':
    case 'u64':
      return BigInt(cell.value as string | number)
    case 'uuid':
      return (cell.value as string).replace(/^(.{8})(.{4})(.{4})(.{4})(.{12})$/, '$1-$2-$3-$4-$5')
    case 'timestamp':
    case 'unix':
    case 'excel_serial':
    case 'duration':
      return { seconds: cell.seconds, nanos: cell.nanos }
    case 'date':
    case 'date_ordered':
      return { year: cell.year, month: cell.month, day: cell.day }
    case 'datetime':
      return {
        year: cell.year,
        month: cell.month,
        day: cell.day,
        ...timeOf(cell.nanos_of_day as number),
      }
    case 'time':
      return timeOf(cell.nanos as number)
    case 'text':
      return cell.text
    default:
      return cell.value
  }
}

// What HyperCast's own package says of a cell's text through the column's door.
function judge(entry: PlanEntry, raw: Uint8Array): Verdict<unknown> {
  switch (entry.door) {
    case 'unix':
      return hypercast.castUnix(raw, entry.precision as number)
    case 'excel_serial':
      return hypercast.castExcelSerial(raw, entry.epoch as number)
    case 'date_ordered':
      return hypercast.castDate(raw, entry.order as number)
    case 'datetime':
      return hypercast.castDatetime(raw, entry.order as number)
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
      return hypercast[name](raw, formatOf(entry))
    }
  }
}

function assertCell(
  label: string,
  entry: PlanEntry,
  verdict: Verdict<Value>,
  raw: Uint8Array,
  cell: Cell,
  judged: boolean,
): void {
  if (verdict.ok) {
    assert.equal(cell.expect, 'ok', `${label}: unexpectedly cast to ${String(verdict.value)}`)
    assert.deepEqual(verdict.value, expectedValue(entry.door, cell), label)
    if (entry.door === 'text') assert.deepEqual(raw, encoder.encode(cell.text as string), label)
  } else {
    const { reason, offset, length } = verdict.fault
    assert.notEqual(cell.expect, 'ok', `${label}: faulted ${reason}`)
    assert.equal(reason, reasons[cell.expect as keyof typeof reasons], label)
    // The span indexes the raw text the batch hands back.
    assert.ok(
      offset + length <= raw.length,
      `${label}: span ${offset}+${length} past ${raw.length}`,
    )
    if (cell.fault !== undefined) {
      assert.deepEqual([offset, length], cell.fault, `${label}: span`)
      assert.equal(decoder.decode(raw), cell.raw, `${label}: raw`)
    }
    if (cell.expect === 'empty') {
      assert.deepEqual([offset, length], [0, 0], label)
      assert.equal(raw.length, 0, label)
    }
  }
  if (judged && entry.door !== 'text') {
    assert.deepEqual(verdict, judge(entry, raw), `${label}: disagrees with hypercast`)
  }
}

// Holds one batch to the corpus rows from `seen` on; returns the rows seen after it.
function assertBatch(
  label: string,
  plan: PlanEntry[],
  rows: Cell[][],
  seen: number,
  batch: Batch,
  batchRows: number,
  judged: boolean,
  numbers?: number[],
): number {
  assert.ok(batch.rows >= 1 && batch.rows <= batchRows, `${label}: ${batch.rows} rows`)
  assert.ok(seen + batch.rows <= rows.length, `${label}: more rows than the corpus lists`)
  assert.equal(batch.columnCount, plan.length, label)
  for (let column = 0; column < plan.length; column++) {
    const whole = batch.column(column)
    assert.equal(whole.length, batch.rows, label)
    let faults = 0
    for (let row = 0; row < batch.rows; row++) {
      const at = `${label}, row ${seen + row}, column ${column}`
      const verdict = batch.get(column, row)
      assertCell(
        at,
        plan[column],
        verdict,
        batch.raw(column, row),
        rows[seen + row][column],
        judged,
      )
      assert.deepEqual(whole[row], verdict, at)
      assert.equal(batch.rawString(column, row), decoder.decode(batch.raw(column, row)), at)
      if (!verdict.ok) faults++
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
      assert.deepEqual(row.raw(column), batch.raw(column, index), label)
    }
    index++
  }
  assert.equal(index, batch.rows, label)
  return seen + batch.rows
}

function assertFailure(label: string, error: unknown, expected: Failure): void {
  const failure = (error as { kind: string; failure?: Record<string, unknown> }).failure
  assert.equal((error as { kind: string }).kind, 'structure', label)
  assert.ok(failure !== undefined, label)
  assert.equal(failure.kind, failureKinds[expected.kind], label)
  assert.deepEqual(
    [failure.record, failure.line, failure.byte],
    [expected.record, expected.line, expected.byte],
    label,
  )
  if (expected.expected !== undefined) {
    assert.deepEqual([failure.expected, failure.found], [expected.expected, expected.found], label)
  }
}

// Reads a bound reader to its end (or its failure) against the corpus rows.
function replay(
  label: string,
  reader: Reader,
  plan: PlanEntry[],
  rows: Cell[][],
  failure: Failure | undefined,
  batchRows: number,
  judged: boolean,
  numbers?: number[],
): void {
  let seen = 0
  let stopped: Outcome<Batch | null> | undefined
  for (const read of reader) {
    if (!read.ok) {
      stopped = read
      break
    }
    seen = assertBatch(label, plan, rows, seen, read.value, batchRows, judged, numbers)
  }
  assert.equal(seen, rows.length, `${label}: ${seen} rows, the corpus lists ${rows.length}`)
  if (failure === undefined) {
    assert.equal(stopped, undefined, `${label}: ${JSON.stringify(stopped)}`)
    assert.deepEqual(reader.read(), { ok: true, value: null }, label)
  } else {
    assert.ok(stopped !== undefined && !stopped.ok, `${label}: no failure`)
    assertFailure(label, stopped.error, failure)
    // A structural failure is final: the same one, again.
    assert.deepEqual(reader.read(), stopped, label)
  }
  // Closing gives the buffers back now; a read after it is the caller's mistake, answered.
  reader.close()
  const after = reader.read()
  assert.ok(!after.ok && after.error.kind === 'closed', label)
}

function opened<T>(label: string, outcome: Outcome<T>): T {
  assert.ok(outcome.ok, `${label}: ${outcome.ok ? '' : JSON.stringify(outcome.error)}`)
  return outcome.value
}

const delimited = load<DelimitedCase>('delimited.json')
const workbooks = load<WorkbookCase>('workbook.json')

test('the corpus is the whole contract', () => {
  assert.ok(delimited.length > 0 && workbooks.length > 0)
})

const ROWS = [1, 2, 4096]
const BUFFERS = [1, 5, undefined]

test('delimited.json, from memory and from a file, every batch size and buffer', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'hypertabular-corpus-'))
  try {
    for (const [at, c] of delimited.entries()) {
      const dialect = {
        separator: c.dialect.separator,
        quoting: c.dialect.quoting,
        hasHeader: c.dialect.has_header,
        skipBlankLines: c.dialect.skip_blank_lines,
      }
      const bytes = encoder.encode(c.input)
      const file = path.join(dir, `${at}.txt`)
      writeFileSync(file, bytes)
      for (const batchRows of ROWS) {
        for (const bufferBytes of BUFFERS) {
          const options = { batchRows, bufferBytes }
          const sources: [string, (plan: InstanceType<typeof Column>[] | null) => unknown][] = [
            ['memory', (plan) => DelimitedReader.fromBytes(bytes, dialect, plan, options)],
            ['a string', (plan) => DelimitedReader.fromBytes(c.input, dialect, plan, options)],
            ['a file', (plan) => DelimitedReader.open(file, dialect, plan, options)],
          ]
          for (const [where, open] of sources) {
            const label = `${c.name} (${where}, ${batchRows} rows, buffer ${bufferBytes})`
            const plan = c.plan.map(columnOf)
            const reader = opened(
              label,
              open(plan) as Outcome<InstanceType<typeof DelimitedReader>>,
            )
            assert.deepEqual(reader.header?.names ?? null, c.header, label)
            assert.equal(reader.isBound, true, label)
            replay(label, reader, c.plan, c.rows, c.failure, batchRows, true)
          }
        }
      }
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('delimited.json, header first', () => {
  for (const c of delimited) {
    const dialect = {
      separator: c.dialect.separator,
      quoting: c.dialect.quoting,
      hasHeader: c.dialect.has_header,
      skipBlankLines: c.dialect.skip_blank_lines,
    }
    for (const batchRows of [1, 4096]) {
      const label = `${c.name} (header first, ${batchRows} rows)`
      const reader = opened(
        label,
        DelimitedReader.fromBytes(encoder.encode(c.input), dialect, null, { batchRows }),
      )
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
      replay(label, reader, c.plan, c.rows, c.failure, batchRows, true)
    }
  }
})

test('workbook.json, by index and by name, header first and bound up front', () => {
  const books = new Map<string, Uint8Array>()
  for (const c of workbooks) {
    const file = path.join(CORPUS, c.file)
    if (c.format === undefined) {
      // A file the core refuses: the refusal, never a throw.
      const refused = Workbook.open(file)
      assert.ok(!refused.ok, c.name)
      assertFailure(c.name, refused.error, c.failure as Failure)
      continue
    }
    let bytes = books.get(file)
    if (bytes === undefined) {
      bytes = new Uint8Array(readFileSync(file))
      books.set(file, bytes)
    }
    const book = opened(c.name, Workbook.fromBytes(bytes))
    assert.equal(book.format, c.format, c.name)
    assert.equal(book.dateSystem, c.epoch, c.name)
    assert.deepEqual(book.sheets, c.sheets, c.name)
    const sheets = c.sheets as { name: string }[]
    const index = c.sheet as number
    const name = sheets[index].name
    const unique = sheets.filter((sheet) => sheet.name === name).length === 1
    const options = c.options as { has_header: boolean; skip_empty_rows: boolean }
    const plan = c.plan as PlanEntry[]
    for (const batchRows of [1, 4096]) {
      const sheetOptions = {
        hasHeader: options.has_header,
        skipEmptyRows: options.skip_empty_rows,
        batchRows,
      }
      const ways: [string, number | string][] = [['index', index]]
      if (unique) ways.push(['name', name])
      for (const [how, which] of ways) {
        for (const headerFirst of [false, true]) {
          const label = `${c.name} (by ${how}, ${batchRows} rows${headerFirst ? ', header first' : ''})`
          const sheet = opened(
            label,
            book.sheet(which, sheetOptions, headerFirst ? null : plan.map(columnOf)),
          )
          assert.deepEqual(sheet.header?.names ?? null, c.header ?? null, label)
          assert.equal(sheet.dateSystem, c.epoch, label)
          if (headerFirst) {
            assert.equal(sheet.isBound, false, label)
            const early = sheet.read()
            assert.ok(!early.ok && early.error.kind === 'unbound', label)
            assert.deepEqual(sheet.bind(plan.map(columnOf)), { ok: true, value: null }, label)
          }
          replay(label, sheet, plan, c.rows as Cell[][], c.failure, batchRows, false, c.numbers)
        }
      }
    }
  }
})

test('the same workbook opened from its path', () => {
  const c = workbooks.find((each) => each.format !== undefined) as WorkbookCase
  const book = opened(c.name, Workbook.open(path.join(CORPUS, c.file)))
  assert.equal(book.format, c.format)
  assert.deepEqual(book.sheets, c.sheets)
})
