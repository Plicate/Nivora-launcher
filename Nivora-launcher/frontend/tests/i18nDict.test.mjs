// Словарь локализации: источник ключей — русские строки разметки. Тест
// держит словарь целым: дубли ключей, пустые переводы, кириллица в
// английских значениях и битые шаблоны паттернов ломали бы тихо.
import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { translate } from '../src/i18n.ts'

const here = dirname(fileURLToPath(import.meta.url))
const source = readFileSync(join(here, '..', 'src', 'i18n.ts'), 'utf8')

function extractBlock(name) {
  const start = source.indexOf(`const ${name}`)
  assert.ok(start >= 0, `блок ${name} найден`)
  const open = source.indexOf('{', start)
  let depth = 0
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === '{') depth += 1
    if (source[i] === '}') {
      depth -= 1
      if (depth === 0) return source.slice(open, i + 1)
    }
  }
  assert.fail(`блок ${name} не закрыт`)
}

test('словарь EN без дублей, пустот и кириллицы в значениях', () => {
  const block = extractBlock('EN')
  const pairs = [...block.matchAll(/'((?:[^'\\]|\\.)+)':\s*'((?:[^'\\]|\\.)*)'/g)]
  assert.ok(pairs.length > 300, `словарь объёмный: ${pairs.length} записей`)
  const seen = new Set()
  for (const [, key, value] of pairs) {
    assert.ok(!seen.has(key), `дубль ключа: ${key}`)
    seen.add(key)
    assert.ok(value.trim().length > 0, `пустой перевод: ${key}`)
    assert.ok(!/[А-Яа-яЁё]/.test(value), `кириллица в EN-значении: ${key} → ${value}`)
    assert.ok(/[А-Яа-яЁё]/.test(key), `ключ без русского (словарь keyed русскими строками): ${key}`)
  }
})

test('translate: точные ключи, паттерны с группами, passthrough', () => {
  assert.equal(translate('Сохранить'), 'Save')
  assert.equal(translate('  Сохранить  '), '  Save  ')
  assert.equal(translate('Доступна версия 1.2.3.'), 'Version 1.2.3 is available.')
  assert.equal(translate('Скачано 10 из 20 байт.'), 'Downloaded 10 of 20 bytes.')
  // Неизвестная строка возвращается как есть — graceful fallback.
  assert.equal(translate('Совершенно новая строка'), 'Совершенно новая строка')
  assert.equal(translate(''), '')
})
