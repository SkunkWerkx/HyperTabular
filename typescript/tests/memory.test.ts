// A synchronous read loop's memory stays bounded: what it drops (batches, rows) is reclaimed by
// the engine's ordinary collection while the loop runs, with no turn of the event loop. Each of
// the README's idioms reads a generated 45 MB file three times, one of them a batch per row; the
// process may not grow by more than 64 MB from the end of the first pass to the end of the third,
// where a batch or a row held until the loop yields grew it by hundreds of MB a pass.
//
// It measures the whole process, so it runs only when `HYPERTABULAR_MEMORY_CHECK=1` says so, in
// a container capped in memory, never on a workstation. It checks the native addon: the
// WebAssembly fallback's memory is the module's own and never shrinks, a recorded loss.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const fs: typeof import('node:fs') = require('node:fs')
const os: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const { Column, DelimitedReader } = require('../index.js') as typeof import('../index.js')
const { __napiBindingTarget } = require('../binding.js') as typeof import('../binding.js')

const ROWS = 1_000_000
const PASSES = 3
const BOUND_MB = 64

const skip =
  process.env.HYPERTABULAR_MEMORY_CHECK !== '1'
    ? 'HYPERTABULAR_MEMORY_CHECK=1 runs it, in a container capped in memory'
    : __napiBindingTarget !== 'native'
      ? "the WebAssembly fallback's memory never shrinks"
      : false

function generate(file: string): void {
  const out = fs.openSync(file, 'w')
  fs.writeSync(out, 'id,qty,price,name,when,flag\n')
  const pad = (n: number) => String(n).padStart(2, '0')
  for (let from = 0; from < ROWS; from += 10_000) {
    let chunk = ''
    for (let i = from; i < from + 10_000; i++) {
      chunk += `${i},${(i * 7919) % 100_000},${i % 10_000}.${pad(i % 100)},name${i % 1000},2026-${pad((i % 12) + 1)}-${pad((i % 28) + 1)},${i % 2 ? 'true' : 'false'}\n`
    }
    fs.writeSync(out, chunk)
  }
  fs.closeSync(out)
}

const plan = () => [
  Column.i64(0),
  Column.i32(1),
  Column.f64(2),
  Column.text(3),
  Column.date(4),
  Column.bool(5),
]

function open(file: string, batchRows?: number) {
  const opened = DelimitedReader.open(file, null, plan(), batchRows ? { batchRows } : null)
  assert.ok(opened.ok)
  return opened.value
}

const idioms: Record<string, (file: string) => number> = {
  'for (const read of reader)': (file) => {
    let rows = 0
    for (const read of open(file)) {
      assert.ok(read.ok)
      rows += read.value.rows
    }
    return rows
  },
  'while over read()': (file) => {
    const reader = open(file)
    let rows = 0
    for (let read = reader.read(); read.ok && read.value !== null; read = reader.read()) {
      rows += read.value.rows
    }
    return rows
  },
  'for (const read of reader), a row a batch': (file) => {
    let rows = 0
    for (const read of open(file, 1)) {
      assert.ok(read.ok)
      rows += read.value.rows
    }
    return rows
  },
  'rows()': (file) => {
    let rows = 0
    for (const row of open(file).rows()) {
      assert.ok(row.ok)
      if (row.value.get(0).ok) rows++
    }
    return rows
  },
  'rows(), holding the last Row': (file) => {
    let held = null
    let rows = 0
    for (const row of open(file).rows()) {
      assert.ok(row.ok)
      held = row.value
      rows++
    }
    assert.ok(held?.get(0).ok)
    return rows
  },
}

test('a synchronous read loop reclaims what it drops as it goes', { skip }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'hypertabular-memory-'))
  const file = path.join(dir, 'rows.csv')
  try {
    generate(file)
    const mb = () => process.memoryUsage().rss / 1e6
    for (const [idiom, read] of Object.entries(idioms)) {
      let first = 0
      for (let pass = 0; pass < PASSES; pass++) {
        assert.equal(read(file), ROWS, idiom)
        if (pass === 0) first = mb()
      }
      const grown = mb() - first
      assert.ok(grown < BOUND_MB, `${idiom}: grew ${grown.toFixed(0)} MB over ${PASSES - 1} passes`)
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
