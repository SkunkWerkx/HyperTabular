const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const { Worker }: typeof import('node:worker_threads') = require('node:worker_threads')
const { Column, DelimitedReader } = require('../index.js') as typeof import('../index.js')

function firstRow(entry: string): unknown {
  const api = require(entry) as typeof import('../index.js')
  const opened = api.DelimitedReader.fromBytes('n,x\n"(1,234)",12x\n', null, [
    api.Column.i32(0),
    api.Column.i32(1),
  ])
  if (!opened.ok) return opened
  const read = opened.value.read()
  if (!read.ok || read.value === null) return read
  return [read.value.get(0, 0), read.value.get(1, 0)]
}

// Each worker loads the binding again (under WASI, its own module instance), and reads on its
// own while the main thread keeps reading too.
test('a worker thread reads with an addon of its own', async () => {
  const entry = path.resolve(__dirname, '../index.js')
  const script = `
    const { parentPort } = require('node:worker_threads')
    parentPort.postMessage((${firstRow.toString()})(${JSON.stringify(entry)}))
  `
  const fromWorker = await new Promise<unknown>((resolve, reject) => {
    const worker = new Worker(script, { eval: true })
    worker.once('message', resolve)
    worker.once('error', reject)
  })
  const expected = [
    { ok: true, value: -1234 },
    { ok: false, fault: { reason: 'malformed', offset: 2, length: 1 } },
  ]
  assert.deepEqual(fromWorker, expected)
  assert.deepEqual(firstRow(entry), expected)
  const opened = DelimitedReader.fromBytes('n\n7\n', null, [Column.i32(0)])
  assert.ok(opened.ok)
})
