const NICKNAME_MIN_LENGTH = 3
const NICKNAME_MAX_LENGTH = 16
const ALLOWED_CHARACTER = /^[A-Za-z0-9_]$/

function describeCharacter(character: string): string {
  if (character === ' ') return '«пробел»'
  if (/\s/.test(character)) return '«пробельный символ»'
  return `«${character}»`
}

/** текст ошибки или пустая строка, если ник допустим */
export function nicknameError(value: string): string {
  if (!value) return 'Введите ник'
  const characters = Array.from(value)
  const invalid = [...new Set(characters.filter((character) => !ALLOWED_CHARACTER.test(character)))]
  if (invalid.length) {
    const shown = invalid.slice(0, 6).map(describeCharacter).join(', ') + (invalid.length > 6 ? ' …' : '')
    return `Ник содержит недопустимые символы: ${shown}. Разрешены только латинские буквы, цифры и _`
  }
  if (characters.length < NICKNAME_MIN_LENGTH) return `Ник должен быть не короче ${NICKNAME_MIN_LENGTH} символов`
  if (characters.length > NICKNAME_MAX_LENGTH) return `Ник должен быть не длиннее ${NICKNAME_MAX_LENGTH} символов`
  return ''
}
