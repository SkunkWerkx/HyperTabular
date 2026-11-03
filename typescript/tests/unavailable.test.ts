// What a consumer gets when neither the native addon nor the WebAssembly fallback loads: the
// package still requires, `isAvailable()` is false, `Dialect` is still there, and every opener
// throws ERR_HYPERTABULAR_UNAVAILABLE with the loader's error as its cause. That throw is the
// one this package makes: an install problem, not an input, and no reader could answer it with
// an outcome.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const { spawnSync }: typeof import('node:child_process') = require('node:child_process')
const {
  copyFileSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
}: typeof import('node:fs') = require('node:fs')
const { tmpdir }: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const api = require('../index.js') as typeof import('../index.js')

test('isHyperTabularError recognises the unavailable error and refuses everything else', () => {
  const error = Object.assign(new Error('x'), { code: 'ERR_HYPERTABULAR_UNAVAILABLE' })
  assert.equal(api.isHyperTabularError(error), true)
  assert.equal(api.isHyperTabularError(new Error('x')), false)
  assert.equal(api.isHyperTabularError(null), false)
  assert.equal(api.isHyperTabularError({ code: 'ERR_HYPERTABULAR_UNAVAILABLE' }), false)
  assert.equal(api.isHyperTabularError(Object.assign(new Error('x'), { code: 42 })), false)
})

// The loader is made to fail in a process of its own (the same runtime, Node or Bun): a copy of
// index.js beside a binding.js that throws, as an install with no addon and no fallback leaves
// it. A preload hook would do on Node, but Bun's require does not consult one.
const SCRIPT = `
const assert = require('node:assert/strict')
const api = require(process.env.HYPERTABULAR_INDEX)
assert.equal(api.isAvailable(), false)
assert.equal(api.Dialect.TSV.separator, '\\t')
for (const call of [
  () => api.DelimitedReader.fromBytes('a'),
  () => api.DelimitedReader.open('a.csv'),
  () => api.Workbook.open('a.xlsx'),
  () => api.Column.i32(0),
  () => api.nativeVersion(),
]) {
  let thrown
  try { call() } catch (error) { thrown = error }
  assert.ok(api.isHyperTabularError(thrown), String(thrown))
  assert.equal(thrown.code, 'ERR_HYPERTABULAR_UNAVAILABLE')
  assert.equal(thrown.cause.message, 'no addon here')
}
`

test('with no addon, the package loads and every opener throws ERR_HYPERTABULAR_UNAVAILABLE', () => {
  const dir = mkdtempSync(path.join(tmpdir(), 'hypertabular-unavailable-'))
  try {
    copyFileSync(path.resolve(__dirname, '..', 'index.js'), path.join(dir, 'index.js'))
    writeFileSync(path.join(dir, 'binding.js'), "throw new Error('no addon here')\n")
    const result = spawnSync(process.execPath, ['-e', SCRIPT], {
      env: { ...process.env, HYPERTABULAR_INDEX: path.join(dir, 'index.js') },
      encoding: 'utf8',
    })
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
