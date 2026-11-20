// A reader, a sheet and a workbook tell the engine's collector what they hold outside its heap
// (the input buffer, the plan's arrays, the cell table, a workbook's bytes and tables), settle
// the difference when a call grows it, and take it all back at `close()`.
//
// What the engine has been told is read from the engine itself, through the module
// (`__externalMemory`, its answer to an adjustment of nothing): Node 26's
// `process.memoryUsage().external` does not include it. JavaScriptCore (Bun) keeps a count of
// what it is told and never takes any back, so there only the telling is checked. Under the
// WebAssembly fallback every buffer is in the module's own memory, which the engine already
// counts, so nothing is told.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const fs: typeof import('node:fs') = require('node:fs')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const v8: typeof import('node:v8') = require('node:v8')
const vm: typeof import('node:vm') = require('node:vm')
const { Column, DelimitedReader, Workbook } = require('../index.js') as typeof import('../index.js')
const { __externalMemory, __napiBindingTarget } =
  require('../binding.js') as typeof import('../binding.js')

type Outcome<T> = import('../index.js').Outcome<T>

const skip =
  __napiBindingTarget !== 'native' &&
  "the WebAssembly fallback's buffers are in the module's memory, which the engine counts"
// Whether the engine takes back what it is told is no longer held.
const takesBack = !('Bun' in globalThis)

const MIB = 1 << 20
// What the engine has been told is held outside its heap. Nothing between two readings here
// makes an `ArrayBuffer` of any size, which the engine counts in the same total.
const external = () => __externalMemory()
// A collection before the first reading, so that none between two readings takes back what
// the test runner dropped.
function collect(): void {
  if ('Bun' in globalThis) {
    ;(globalThis as unknown as { Bun: { gc(full: boolean): void } }).Bun.gc(true)
  } else if (!skip) {
    v8.setFlagsFromString('--expose-gc')
    ;(vm.runInNewContext('gc') as () => void)()
  }
}
function settled(): number {
  collect()
  return external()
}

function value<T>(outcome: Outcome<T>): T {
  assert.ok(outcome.ok, JSON.stringify(outcome))
  return outcome.value
}

test('a reader tells the collector what it holds and takes it back at close', { skip }, () => {
  // One record of 4 MiB in a column the plan does not read, so the batch's own copy is small.
  const text = `a,b\n${'y'.repeat(4 * MIB)},1\nz,2\n`
  const plan = [Column.i32(1)]
  const before = settled()
  const reader = value(DelimitedReader.fromBytes(text, null, plan, { bufferBytes: MIB }))
  // The input (a copy the reader reads from) and the buffer it is read through.
  const opened = external() - before
  assert.ok(opened >= 4 * MIB + MIB, `open told ${opened} bytes`)
  assert.ok(opened < 4 * MIB + 3 * MIB, `open told ${opened} bytes`)
  // The record is longer than the buffer, which grows to hold it.
  assert.ok(value(reader.read()) !== null)
  const grown = external() - before
  assert.ok(grown >= opened + 3 * MIB, `the read told ${grown} bytes, the open ${opened}`)
  // A read that grows nothing tells nothing.
  const steady = external()
  assert.equal(value(reader.read()), null)
  assert.ok(Math.abs(external() - steady) < MIB / 4)
  assert.deepEqual(reader.close(), { ok: true, value: null })
  const closed = external() - before
  if (takesBack) assert.ok(Math.abs(closed) < MIB / 2, `after close ${closed} bytes are still told`)
  // Closing again gives nothing back twice.
  assert.deepEqual(reader.close(), { ok: true, value: null })
  assert.ok(Math.abs(external() - before - closed) < MIB / 4)
})

test('a reader dropped open gives it back when it is finalized, and a closed one nothing more', {
  skip: skip || (!takesBack && 'JavaScriptCore takes nothing back'),
}, async () => {
  const before = settled()
  // In a function of their own, so that nothing here still holds them.
  ;(() => {
    const options = { bufferBytes: 4 * MIB }
    const dropped = value(DelimitedReader.fromBytes('a\n1\n', null, [Column.i32(0)], options))
    const closed = value(DelimitedReader.fromBytes('a\n1\n', null, [Column.i32(0)], options))
    assert.ok(value(dropped.read()) !== null)
    assert.ok(external() - before >= 8 * MIB)
    closed.close()
    assert.ok(external() - before >= 4 * MIB)
    assert.ok(external() - before < 6 * MIB)
  })()
  // A finalizer runs once the event loop turns after the collection that found its object.
  for (let turn = 0; turn < 20 && external() - before > MIB; turn++) {
    collect()
    await new Promise(setImmediate)
  }
  // Both are finalized by now or neither: a few more turns for the closed one's.
  for (let turn = 0; turn < 5; turn++) {
    collect()
    await new Promise(setImmediate)
  }
  const left = external() - before
  assert.ok(Math.abs(left) < MIB / 2, `${left} bytes are told after both were finalized`)
})

test('binding a plan tells the collector its arrays', { skip }, () => {
  const before = settled()
  const reader = value(DelimitedReader.fromBytes('a,b\n1,2\n', null, null, { batchRows: 100_000 }))
  const unbound = external() - before
  assert.deepEqual(reader.bind([Column.i64(0), Column.i64(1)]), { ok: true, value: null })
  const bound = external() - before
  // 100,000 rows of two columns' values (8 bytes) and verdicts (12), at the least.
  assert.ok(bound >= unbound + 2 * 100_000 * 20, `bound ${bound}, unbound ${unbound}`)
  reader.close()
  if (takesBack) assert.ok(Math.abs(external() - before) < MIB / 2)
})

test('a sheet tells the collector what it holds and takes it back at close', { skip }, () => {
  const bytes = fs.readFileSync(path.resolve(__dirname, '../../corpus/workbook/basic.xlsx'))
  const book = value(Workbook.fromBytes(bytes))
  const before = settled()
  const sheet = value(book.sheet(0, { batchRows: 100_000 }, [Column.text(0), Column.text(1)]))
  const opened = external() - before
  assert.ok(opened >= 2 * 100_000 * 20, `the sheet told ${opened} bytes`)
  assert.ok(value(sheet.read()) !== null)
  assert.deepEqual(sheet.close(), { ok: true, value: null })
  const closed = external() - before
  if (takesBack) assert.ok(Math.abs(closed) < MIB / 2, `after close ${closed} bytes are still told`)
})

test('a workbook tells the collector its bytes', { skip }, () => {
  // A file that is no workbook is refused and tells nothing; one that is tells its bytes, held
  // until the workbook and its sheets are collected.
  const bytes = fs.readFileSync(path.resolve(__dirname, '../../corpus/workbook/basic.xlsx'))
  const before = settled()
  const refused = Workbook.fromBytes(new Uint8Array(8))
  assert.ok(!refused.ok)
  assert.ok(Math.abs(external() - before) < 1024)
  const book = value(Workbook.fromBytes(bytes))
  const told = external() - before
  assert.ok(told >= bytes.length, `the workbook told ${told} bytes of ${bytes.length}`)
  assert.equal(book.sheets.length > 0, true)
})
