// The corpus, run against the WASI package in a browser. Each check appends a line to
// `#out`; the last line is `PASS`, or the first failure is `FAIL: <reason>` and nothing
// follows it. `check-dom.mjs` reads that verdict from the DOM Chrome dumps.
//
// `tests/corpus.test.ts` is the source of truth for the corpus replay; it is ported here for
// the browser, which has neither `node:test` nor `node:assert`: every delimited case from
// memory in batches of one row and of many, through a one-byte buffer and the default, with
// the plan up front and header first; every workbook case by index, in batches of one row and
// of many, its bytes fetched as the bundle's own assets. The two numeric formats the corpus
// declares are made by HyperCast's own WASI package, whose `NumFormat.packed` a column takes.
import delimitedText from '../../corpus/delimited.json?raw'
import workbookText from '../../corpus/workbook.json?raw'

const assets = import.meta.glob('../../corpus/workbook/*', {
  query: '?url',
  import: 'default',
  eager: true,
})

const out = document.getElementById('out')
const log = (line) => {
  out.textContent += `${line}\n`
}

const parse = (text) =>
  JSON.parse(text, (_key, value, context) => {
    if (typeof value !== 'number' || Number.isSafeInteger(value)) return value
    if (context?.source === undefined) throw new Error('JSON.parse revivers get no source text')
    return /^-?\d+$/.test(context.source) ? context.source : value
  })

// Structural equality over the plain values a cell holds.
function same(a, b) {
  if (Object.is(a, b) || a === b) return true
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false
  const keys = Object.keys(a)
  if (keys.length !== Object.keys(b).length) return false
  return keys.every((key) => same(a[key], b[key]))
}

function check(ok, context) {
  if (!ok) throw new Error(context)
}

const decoder = new TextDecoder()
const encoder = new TextEncoder()
const reasons = { empty: 'empty', malformed: 'malformed', out_of_range: 'outOfRange' }
const camel = (name) => name.replace(/_(.)/g, (_whole, letter) => letter.toUpperCase())

function timeOf(nanosOfDay) {
  const seconds = Math.floor(nanosOfDay / 1e9)
  return {
    hour: Math.floor(seconds / 3600),
    minute: Math.floor(seconds / 60) % 60,
    second: seconds % 60,
    nanos: nanosOfDay % 1e9,
  }
}

function expectedValue(door, cell) {
  switch (door) {
    case 'i64':
    case 'u64':
      return BigInt(cell.value)
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

function checkCell(ctx, entry, verdict, raw, cell) {
  if (verdict.ok) {
    check(cell.expect === 'ok', `${ctx}: unexpectedly cast to ${String(verdict.value)}`)
    const expected = expectedValue(entry.door, cell)
    check(same(verdict.value, expected), `${ctx}: ${String(verdict.value)} != ${expected}`)
    return
  }
  const { reason, offset, length } = verdict.fault
  check(cell.expect !== 'ok', `${ctx}: faulted ${reason}`)
  check(reason === reasons[cell.expect], `${ctx}: reason ${reason}`)
  check(offset + length <= raw.length, `${ctx}: span ${offset}+${length} past ${raw.length}`)
  if (cell.fault !== undefined) {
    check(offset === cell.fault[0] && length === cell.fault[1], `${ctx}: span`)
    check(decoder.decode(raw) === cell.raw, `${ctx}: raw`)
  }
  if (cell.expect === 'empty') check(offset === 0 && length === 0 && raw.length === 0, ctx)
}

function checkFailure(ctx, error, expected) {
  check(error.kind === 'structure' && error.failure !== undefined, `${ctx}: ${error.kind}`)
  const { failure } = error
  check(failure.kind === camel(expected.kind), `${ctx}: ${failure.kind}`)
  check(
    failure.record === expected.record &&
      failure.line === expected.line &&
      failure.byte === expected.byte,
    `${ctx}: at ${failure.record}/${failure.line}/${failure.byte}`,
  )
}

// Reads a bound reader or sheet to its end (or its failure), then closes it.
function replay(ctx, reader, plan, rows, failure, batchRows, numbers) {
  let seen = 0
  for (;;) {
    const read = reader.read()
    if (!read.ok) {
      check(failure !== undefined, `${ctx}: ${read.error.kind}: ${read.error.message}`)
      checkFailure(ctx, read.error, failure)
      break
    }
    const batch = read.value
    if (batch === null) {
      check(failure === undefined, `${ctx}: no failure`)
      break
    }
    check(batch.rows >= 1 && batch.rows <= batchRows, `${ctx}: ${batch.rows} rows`)
    check(seen + batch.rows <= rows.length, `${ctx}: more rows than the corpus lists`)
    for (let row = 0; row < batch.rows; row++) {
      if (numbers !== undefined) check(batch.line(row) === numbers[seen + row], `${ctx}: line`)
      for (let column = 0; column < plan.length; column++) {
        const at = `${ctx}, row ${seen + row}, column ${column}`
        const raw = batch.raw(column, row)
        checkCell(at, plan[column], batch.get(column, row), raw, rows[seen + row][column])
      }
    }
    seen += batch.rows
  }
  check(seen === rows.length, `${ctx}: ${seen} rows, the corpus lists ${rows.length}`)
  reader.close()
  const after = reader.read()
  check(!after.ok && after.error.kind === 'closed', `${ctx}: read after close`)
}

try {
  // Each package's `browser` condition resolves to napi's `*.wasip1-browser.js`, which fetches
  // and instantiates the module under top-level await and then exports the bindings as plain
  // named exports. The imports are dynamic so that a failure to load lands in `#out` as a
  // FAIL line instead of leaving the page blank.
  const api = await import('@skunkwerkx/hypertabular-wasm32-wasip1')
  const { NumFormat } = await import('@skunkwerkx/hypercast-wasm32-wasip1')
  const { Column, DelimitedReader, Workbook } = api
  log(`loaded: target=${api.__napiBindingTarget}`)

  const formats = new Map()
  const formatOf = (entry) => {
    if (entry.format === undefined) return undefined
    const key = JSON.stringify(entry.format)
    if (!formats.has(key)) {
      const f = entry.format
      const declared = NumFormat.declare({
        decimalSep: f.decimal_sep,
        groupSep: f.group_sep,
        flags: f.flags,
        currency: f.currency,
      })
      check(declared.ok, `${key}: ${declared.message}`)
      formats.set(key, declared.value)
    }
    return formats.get(key)
  }
  const columnOf = (entry) => {
    const { door, ordinal } = entry
    switch (door) {
      case 'unix':
        return Column.unix(ordinal, entry.precision)
      case 'excel_serial':
        return Column.excelSerial(ordinal, entry.epoch)
      case 'date_ordered':
        return Column.dateOrdered(ordinal, entry.order)
      case 'datetime':
        return Column.datetime(ordinal, entry.order)
      default:
        return Column[door](ordinal, formatOf(entry))
    }
  }
  const opened = (ctx, outcome) => {
    check(
      outcome.ok,
      `${ctx}: ${outcome.ok ? '' : `${outcome.error.kind}: ${outcome.error.message}`}`,
    )
    return outcome.value
  }

  const delimited = parse(delimitedText)
  for (const c of delimited) {
    const dialect = {
      separator: c.dialect.separator,
      quoting: c.dialect.quoting,
      hasHeader: c.dialect.has_header,
      skipBlankLines: c.dialect.skip_blank_lines,
    }
    const bytes = encoder.encode(c.input)
    for (const batchRows of [1, 4096]) {
      for (const bufferBytes of [1, undefined]) {
        for (const headerFirst of [false, true]) {
          const ctx = `delimited.json ${c.name} (${batchRows} rows, buffer ${bufferBytes}${headerFirst ? ', header first' : ''})`
          const plan = c.plan.map(columnOf)
          const options = { batchRows, bufferBytes }
          const reader = opened(
            ctx,
            DelimitedReader.fromBytes(bytes, dialect, headerFirst ? null : plan, options),
          )
          check(same(reader.header?.names ?? null, c.header), `${ctx}: header`)
          if (headerFirst) check(opened(ctx, reader.bind(plan)) === null, `${ctx}: bind`)
          replay(ctx, reader, c.plan, c.rows, c.failure, batchRows)
        }
      }
    }
  }
  log(`delimited.json: ${delimited.length} cases`)

  const workbooks = parse(workbookText)
  const books = new Map()
  for (const c of workbooks) {
    const url = assets[`../../corpus/${c.file}`]
    check(url !== undefined, `workbook.json ${c.name}: ${c.file} is not bundled`)
    if (!books.has(url)) books.set(url, new Uint8Array(await (await fetch(url)).arrayBuffer()))
    const opening = Workbook.fromBytes(books.get(url))
    if (c.format === undefined) {
      check(!opening.ok, `workbook.json ${c.name}: opened`)
      checkFailure(`workbook.json ${c.name}`, opening.error, c.failure)
      continue
    }
    const book = opened(c.name, opening)
    check(book.format === c.format && book.dateSystem === c.epoch, `${c.name}: format`)
    check(same(book.sheets, c.sheets), `${c.name}: sheets`)
    for (const batchRows of [1, 4096]) {
      const ctx = `workbook.json ${c.name} (${batchRows} rows)`
      const options = {
        hasHeader: c.options.has_header,
        skipEmptyRows: c.options.skip_empty_rows,
        batchRows,
      }
      const sheet = opened(ctx, book.sheet(c.sheet, options, c.plan.map(columnOf)))
      check(same(sheet.header?.names ?? null, c.header ?? null), `${ctx}: header`)
      replay(ctx, sheet, c.plan, c.rows, c.failure, batchRows, c.numbers)
    }
  }
  log(`workbook.json: ${workbooks.length} cases`)

  // No reader throws for an argument of the wrong type: it is an outcome's error.
  const refused = DelimitedReader.fromBytes(42)
  check(
    !refused.ok && refused.error.kind === 'argument',
    `fromBytes(42): ${JSON.stringify(refused)}`,
  )
  const contract = Column.i32(-1)
  check(contract.valid === false, 'Column.i32(-1) is valid')
  log('contract: a wrong-typed argument and an impossible column, neither thrown')

  log('PASS')
} catch (error) {
  log(`FAIL: ${error?.stack ?? error}`)
}
