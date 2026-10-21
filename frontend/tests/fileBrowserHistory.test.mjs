import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

// История навигации проводника (PathHistory в services/fileBrowser.ts)
// проверяется транспиляцией исходника: класс чистый, DOM не нужен.
// Vue-зависимости файла не тянем: извлекаем только тело класса.

function loadPathHistory() {
  const source = readFileSync(new URL('../src/services/fileBrowser.ts', import.meta.url), 'utf8')
  const start = source.indexOf('export class PathHistory')
  assert.ok(start >= 0, 'PathHistory must exist in fileBrowser.ts')
  const end = source.indexOf('\n}\n', start)
  assert.ok(end > start, 'PathHistory class body must be delimited')
  const body = source.slice(start, end + 3)
  const js = body
    .replace('export class PathHistory', 'class PathHistory')
    .replace(/private /g, '')
    .replace(/(\w+): string \| null/g, '$1')
    .replace(/(\w+): string\[\]/g, '$1')
    .replace(/(\w+): string\b/g, '$1')
    .replace(/(\w+): number/g, '$1')
    .replace(/(\w+): boolean/g, '$1')
    .replace(/\): [^{]+\{/g, ') {')
  const factory = new Function(`${js}; return PathHistory`)
  return factory()
}

const PathHistory = loadPathHistory()

test('navigate/back/forward behave like a normal file manager', () => {
  const history = new PathHistory()
  assert.equal(history.current, null)
  assert.equal(history.canBack, false)
  assert.equal(history.canForward, false)

  history.navigate('/home')
  history.navigate('/home/games')
  history.navigate('/home/games/mods')
  assert.equal(history.current, '/home/games/mods')
  assert.equal(history.canBack, true)

  assert.equal(history.back(), '/home/games')
  assert.equal(history.back(), '/home')
  assert.equal(history.canBack, false)
  assert.equal(history.back(), null)

  assert.equal(history.forward(), '/home/games')
  assert.equal(history.forward(), '/home/games/mods')
  assert.equal(history.canForward, false)
  assert.equal(history.forward(), null)
})

test('navigate after back drops the forward tail', () => {
  const history = new PathHistory()
  history.navigate('/a')
  history.navigate('/b')
  history.navigate('/c')
  history.back()
  history.back()
  assert.equal(history.current, '/a')
  history.navigate('/x')
  assert.equal(history.canForward, false)
  assert.equal(history.forward(), null)
  assert.equal(history.back(), '/a')
})

test('re-entering the same path does not duplicate history', () => {
  const history = new PathHistory()
  history.navigate('/a')
  history.navigate('/a')
  history.navigate('/b')
  history.navigate('/b')
  history.back()
  assert.equal(history.current, '/a')
  assert.equal(history.canBack, false)
})

test('reset starts a clean history', () => {
  const history = new PathHistory()
  history.navigate('/a')
  history.navigate('/b')
  history.reset('/fresh')
  assert.equal(history.current, '/fresh')
  assert.equal(history.canBack, false)
  assert.equal(history.canForward, false)
})
