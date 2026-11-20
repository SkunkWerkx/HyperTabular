// A relative path means what it means to the process: the file under its working directory,
// wherever `process.chdir` has since moved it. The native addon hands the path to the operating
// system, which resolves it; the WebAssembly fallback's libc knows only the root it was preopened
// with, so there the binding puts the working directory in front before its libc sees the path. Every
// opener that takes a path is held to it, on both.
const assert: typeof import('node:assert/strict') = require('node:assert/strict')
const {
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  rmSync,
  writeFileSync,
}: typeof import('node:fs') = require('node:fs')
const { tmpdir }: typeof import('node:os') = require('node:os')
const path: typeof import('node:path') = require('node:path')
const { test }: typeof import('node:test') = require('node:test')
const { Column, DelimitedReader, Workbook } = require('../index.js') as typeof import('../index.js')

// Under the WASI fallback on Windows no path reaches the module (README).
const skip =
  (require('../binding.js') as { __napiBindingTarget: string }).__napiBindingTarget !== 'native' &&
  process.platform === 'win32' &&
  'the WebAssembly fallback opens no path on Windows'

const BOOK = path.resolve(__dirname, '../../corpus/workbook/basic.xlsx')

// Runs `inside` with the working directory at a new directory holding `people.csv`, `book.xlsx`
// and `nested/people.csv`, and puts the working directory back.
function inDirectory(inside: (dir: string) => void): void {
  const dir = realpathSync(mkdtempSync(path.join(tmpdir(), 'hypertabular-paths-')))
  const before = process.cwd()
  try {
    writeFileSync(path.join(dir, 'people.csv'), 'id,name\n1,alice\n2,bob\n')
    copyFileSync(BOOK, path.join(dir, 'book.xlsx'))
    mkdirSync(path.join(dir, 'nested'))
    writeFileSync(path.join(dir, 'nested', 'people.csv'), 'id,name\n3,carol\n')
    process.chdir(dir)
    inside(dir)
  } finally {
    process.chdir(before)
    rmSync(dir, { recursive: true, force: true })
  }
}

function names(file: string): unknown[] {
  const opened = DelimitedReader.open(file, null, [Column.i32(0), Column.text(1)])
  assert.ok(opened.ok, `${file}: ${JSON.stringify(opened)}`)
  const read = opened.value.read()
  assert.ok(read.ok && read.value !== null, file)
  const seen = read.value.column<string>(1).map((cell) => (cell.ok ? cell.value : cell))
  assert.deepEqual(opened.value.close(), { ok: true, value: null })
  return seen
}

test('a relative path is opened under the working directory', { skip }, () => {
  inDirectory((dir) => {
    for (const file of ['people.csv', './people.csv', 'nested/../people.csv']) {
      assert.deepEqual(names(file), ['alice', 'bob'], file)
    }
    assert.deepEqual(names('nested/people.csv'), ['carol'])
    assert.deepEqual(names(path.join(dir, 'people.csv')), ['alice', 'bob'])

    const unbound = DelimitedReader.open('people.csv')
    assert.ok(unbound.ok, JSON.stringify(unbound))
    assert.deepEqual(unbound.value.header?.names, ['id', 'name'])
    unbound.value.close()

    const found = DelimitedReader.suggest('./people.csv')
    assert.ok(found.ok, JSON.stringify(found))
    assert.deepEqual(
      found.value.report.map((column) => column.name),
      ['id', 'name'],
    )

    for (const file of ['book.xlsx', './book.xlsx']) {
      const book = Workbook.open(file)
      assert.ok(book.ok, `${file}: ${JSON.stringify(book)}`)
      assert.ok(book.value.sheets.length > 0)
    }
  })
})

test('a relative path follows the working directory as it moves', { skip }, () => {
  inDirectory(() => {
    assert.deepEqual(names('people.csv'), ['alice', 'bob'])
    process.chdir('nested')
    assert.deepEqual(names('people.csv'), ['carol'])
    assert.deepEqual(names('../people.csv'), ['alice', 'bob'])
  })
})

test('a relative path with no file behind it, or no path at all, is the error it was', {
  skip,
}, () => {
  inDirectory(() => {
    for (const open of [
      () => DelimitedReader.open('absent.csv'),
      () => DelimitedReader.open(''),
      () => DelimitedReader.suggest('absent.csv'),
      () => Workbook.open('absent.xlsx'),
      () => Workbook.open(''),
    ]) {
      const opened = open()
      assert.ok(!opened.ok && opened.error.kind === 'io', JSON.stringify(opened))
    }
    for (const open of [
      () => DelimitedReader.open(7 as unknown as string),
      () => DelimitedReader.open(new String('people.csv') as unknown as string),
      () => DelimitedReader.suggest(null as unknown as string),
      () => Workbook.open(undefined as unknown as string),
    ]) {
      const opened = open()
      assert.ok(!opened.ok && opened.error.kind === 'argument', JSON.stringify(opened))
      assert.equal(opened.error.message, 'the path must be a string')
    }
  })
})
