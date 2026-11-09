import assert from 'node:assert/strict'
import { test } from 'node:test'
import * as all from '../index.js'
import { Column, DelimitedReader, Dialect, isAvailable } from '../index.js'

test('named imports work from ESM', () => {
  assert.equal(isAvailable(), true)
  const opened = DelimitedReader.fromBytes('n\n42\n', Dialect.CSV, [Column.i32(0)])
  assert.ok(opened.ok)
  const read = opened.value.read()
  assert.ok(read.ok && read.value !== null)
  assert.deepEqual(read.value.get(0, 0), { ok: true, value: 42 })
})

test('the namespace exposes exactly the public names', () => {
  const keys = Object.keys(all)
    .filter((k) => k !== 'default' && k !== 'module.exports')
    .sort()
  const expected = [
    'nativeVersion',
    'isAvailable',
    'isHyperTabularError',
    'Dialect',
    'Column',
    'Header',
    'Range',
    'DelimitedReader',
    'Workbook',
    'Sheet',
    'Batch',
    'Row',
  ].sort()
  assert.deepEqual(keys, expected)
})
