import test from 'node:test'
import assert from 'node:assert/strict'
import { nicknameError } from '../src/services/nickname.ts'

test('допустимые ники проходят', () => {
  for (const name of ['Steve', 'Player_1', 'abc', 'A'.repeat(16), '___']) assert.equal(nicknameError(name), '', name)
})

test('кириллица, пробелы и спецсимволы дают ошибку о недопустимых символах', () => {
  assert.equal(nicknameError('Игрок'), 'Ник содержит недопустимые символы: «И», «г», «р», «о», «к». Разрешены только латинские буквы, цифры и _')
  assert.match(nicknameError('Steve Jobs'), /^Ник содержит недопустимые символы: «пробел»/)
  assert.match(nicknameError('Steve-1!'), /«-», «!»/)
  assert.match(nicknameError('Stéve'), /«é»/)
  assert.match(nicknameError('Steve😀'), /«😀»/)
  assert.match(nicknameError('a\tb'), /«пробельный символ»/)
})

test('длина: 3–16 символов', () => {
  assert.equal(nicknameError('ab'), 'Ник должен быть не короче 3 символов')
  assert.equal(nicknameError('a'.repeat(17)), 'Ник должен быть не длиннее 16 символов')
  assert.equal(nicknameError(''), 'Введите ник')
})
