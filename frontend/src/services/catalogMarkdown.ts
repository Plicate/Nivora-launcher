// Разбор описаний Modrinth (body) без внешних зависимостей.
// Поддерживаем подмножество Markdown, которым реально пишут карточки:
// заголовки, абзацы, бейджи-картинки со ссылками, блок-цитаты, списки,
// таблицы, код, горизонтальные линии и YouTube-превью.
export interface CatalogInline {
  kind: 'text' | 'strong' | 'em' | 'code' | 'link'
  text: string
  url?: string
}

export interface CatalogBadge {
  alt: string
  url: string
  href: string
}

export type CatalogBlock =
  | { type: 'heading'; level: 2 | 3 | 4; inline: CatalogInline[] }
  | { type: 'paragraph'; inline: CatalogInline[] }
  | { type: 'badges'; items: CatalogBadge[] }
  | { type: 'media'; url: string; href: string; alt: string; video: boolean }
  | { type: 'quote'; paragraphs: CatalogInline[][] }
  | { type: 'list'; ordered: boolean; items: CatalogInline[][] }
  | { type: 'code'; text: string }
  | { type: 'table'; head: CatalogInline[][]; rows: CatalogInline[][][] }
  | { type: 'hr' }

const IMAGE_LINK = /^\s*\[!\[([^\]]*)\]\(([^)\s]+)\)\]\(([^)\s]+)\)\s*$/
const REF_DEF = /^\s*\[([^\]\s]+)\]:\s*(\S+)\s*$/
const IMAGE_ONLY = /^\s*!\[([^\]]*)\]\(([^)\s]+)\)\s*$/
const YOUTUBE_THUMB = /img\.youtube\.com\/vi(?:deos)?\/([\w-]+)/
const HEADING = /^(#{1,4})\s+(.+)$/
const RULE = /^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/
const LIST_ITEM = /^\s*(?:[-*+]\s+|(\d+)[.)]\s+)(.*)$/
const TABLE_ROW = /^\s*\|(.+)\|\s*$/
const TAGS = /<[^>]+>/g

function isHttpUrl(value: string): boolean {
  return /^https?:\/\//i.test(value)
}

function youtubeHref(url: string): string | null {
  const match = url.match(YOUTUBE_THUMB)
  return match ? `https://www.youtube.com/watch?v=${match[1]}` : null
}

// Инлайновый разбор: жирный, курсив, код, ссылки и картинки-ссылки.
function parseInline(raw: string, refs: Record<string, string> = {}): CatalogInline[] {
  const out: CatalogInline[] = []
  const pattern = /(!?)\[([^\]]*)\]\(([^)\s]+)\)|\[([^\]]+)\]\[([^\]\s]+)\]|\*\*([^*]+)\*\*|__([^_]+)__|\*([^*\s][^*]*)\*|`([^`]+)`/g
  let cursor = 0
  let match: RegExpExecArray | null
  while ((match = pattern.exec(raw))) {
    if (match.index > cursor) out.push({ kind: 'text', text: raw.slice(cursor, match.index) })
    if (match[1] === '!') {
      // одиночная картинка внутри текста — показываем как ссылку с подписью
      if (match[2]) out.push({ kind: 'link', text: match[2], url: match[3] })
    } else if (match[2] !== undefined) {
      if (match[2]) out.push({ kind: 'link', text: match[2], url: match[3] })
    } else if (match[4] !== undefined) {
      // ссылка-сноска [текст][ref]: адрес берём из определений внизу документа
      const resolved = refs[match[5]]
      if (resolved && isHttpUrl(resolved)) out.push({ kind: 'link', text: match[4], url: resolved })
      else if (match[4]) out.push({ kind: 'text', text: match[4] })
    } else if (match[6] !== undefined) out.push({ kind: 'strong', text: match[6] })
    else if (match[7] !== undefined) out.push({ kind: 'strong', text: match[7] })
    else if (match[8] !== undefined) out.push(...emphasized(match[8], refs))
    else if (match[9] !== undefined) out.push({ kind: 'code', text: match[9] })
    cursor = pattern.lastIndex
  }
  if (cursor < raw.length) out.push({ kind: 'text', text: raw.slice(cursor) })
  return out.filter((part) => part.text !== '')
}

// em-содержимое может тянуть в себе ссылку-сноску: выкусываем ссылки, остальное — курсив
function emphasized(inner: string, refs: Record<string, string>): CatalogInline[] {
  if (!/\[/.test(inner)) return [{ kind: 'em', text: inner }]
  const parts: CatalogInline[] = []
  const splitter = /\[([^\]]+)\]\[([^\]\s]+)\]|\[([^\]]+)\]\(([^)\s]+)\)/g
  let cursor = 0
  let found: RegExpExecArray | null
  while ((found = splitter.exec(inner))) {
    if (found.index > cursor) parts.push({ kind: 'em', text: inner.slice(cursor, found.index) })
    const text = found[1] ?? found[3]
    const url = found[2] !== undefined ? refs[found[2]] : found[4]
    if (url && isHttpUrl(url)) parts.push({ kind: 'link', text, url })
    else parts.push({ kind: 'em', text: found[0] })
    cursor = splitter.lastIndex
  }
  if (cursor < inner.length) parts.push({ kind: 'em', text: inner.slice(cursor) })
  return parts.filter((part) => part.text !== '')
}

function stripTags(line: string): string {
  return line.replace(TAGS, '').trim()
}

export function parseCatalogBody(body: string): CatalogBlock[] {
  const blocks: CatalogBlock[] = []
  const lines = String(body || '').split('\n')
  // определения ссылок-сносок ([1]: https://…) собираем заранее: в тексте они могут стоять ниже
  const refs: Record<string, string> = {}
  for (const line of lines) {
    const def = line.match(REF_DEF)
    if (def && isHttpUrl(def[2])) refs[def[1]] = def[2]
  }
  let index = 0
  while (index < lines.length) {
    const line = lines[index]

    if (REF_DEF.test(line)) {
      index += 1
      continue
    }

    if (/^\s*```/.test(line)) {
      const code: string[] = []
      index += 1
      while (index < lines.length && !/^\s*```/.test(lines[index])) {
        code.push(lines[index])
        index += 1
      }
      index += 1
      if (code.length) blocks.push({ type: 'code', text: code.join('\n') })
      continue
    }

    const heading = line.match(HEADING)
    if (heading) {
      const level = Math.min(Math.max(heading[1].length, 2), 4) as 2 | 3 | 4
      blocks.push({ type: 'heading', level, inline: parseInline(stripTags(heading[2]), refs) })
      index += 1
      continue
    }

    if (RULE.test(line)) {
      blocks.push({ type: 'hr' })
      index += 1
      continue
    }

    const badge = line.match(IMAGE_LINK)
    if (badge && youtubeHref(badge[2])) {
      blocks.push({ type: 'media', url: badge[2], href: youtubeHref(badge[2]) as string, alt: badge[1], video: true })
      index += 1
      continue
    }
    if (badge && isHttpUrl(badge[2]) && isHttpUrl(badge[3])) {
      const items: CatalogBadge[] = [{ alt: badge[1], url: badge[2], href: badge[3] }]
      index += 1
      // подряд идущие бейджи собираем в одну строку
      while (index < lines.length) {
        const next = lines[index].match(IMAGE_LINK)
        if (!next || !isHttpUrl(next[2]) || !isHttpUrl(next[3])) break
        items.push({ alt: next[1], url: next[2], href: next[3] })
        index += 1
      }
      blocks.push({ type: 'badges', items })
      continue
    }

    const image = line.match(IMAGE_ONLY)
    if (image && isHttpUrl(image[2])) {
      const url = image[2]
      blocks.push({ type: 'media', url, href: youtubeHref(url) || url, alt: image[1], video: Boolean(youtubeHref(url)) })
      index += 1
      continue
    }

    if (line.trimStart().startsWith('>')) {
      const quote: string[] = []
      while (index < lines.length && lines[index].trimStart().startsWith('>')) {
        quote.push(lines[index].replace(/^\s*>\s?/, ''))
        index += 1
      }
      const paragraphs = quote
        .map((entry) => stripTags(entry))
        .join(' ')
        .split(/\s{2,}|\n/) // сдвоенный пробел в Markdown = перенос строки внутри цитаты
        .map((entry) => parseInline(entry.trim(), refs))
        .filter((entry) => entry.length)
      if (paragraphs.length) blocks.push({ type: 'quote', paragraphs })
      continue
    }

    const listItem = line.match(LIST_ITEM)
    if (listItem) {
      const ordered = Boolean(listItem[1])
      const items: CatalogInline[][] = [parseInline(stripTags(listItem[2]), refs)]
      index += 1
      while (index < lines.length) {
        const next = lines[index].match(LIST_ITEM)
        if (!next || Boolean(next[1]) !== ordered) break
        items.push(parseInline(stripTags(next[2]), refs))
        index += 1
      }
      blocks.push({ type: 'list', ordered, items })
      continue
    }

    const tableRow = line.match(TABLE_ROW)
    if (tableRow && index + 1 < lines.length && /^\s*\|[\s:|-]+\|\s*$/.test(lines[index + 1])) {
      const cells = (row: string) => row.split('|').map((cell) => parseInline(stripTags(cell.trim()), refs))
      const head = cells(tableRow[1])
      index += 2
      const rows: CatalogInline[][][] = []
      while (index < lines.length) {
        const next = lines[index].match(TABLE_ROW)
        if (!next) break
        rows.push(cells(next[1]))
        index += 1
      }
      blocks.push({ type: 'table', head, rows })
      continue
    }

    if (!line.trim()) {
      index += 1
      continue
    }

    // абзац: собираем подряд идущие строки, сырой HTML вычищаем
    const chunk: string[] = []
    while (index < lines.length && lines[index].trim() && !/^\s*(?:```|#{1,4}\s|>|- |\* |\d+[.)]\s|\|)/.test(lines[index])) {
      chunk.push(lines[index])
      index += 1
    }
    const text = stripTags(chunk.join(' '))
    if (text) blocks.push({ type: 'paragraph', inline: parseInline(text, refs) })
  }
  return blocks
}

// Точка, на которой длинное описание можно свернуть.
export function collapsePoint(blocks: CatalogBlock[], budget = 1800): number {
  let used = 0
  for (let i = 0; i < blocks.length; i += 1) {
    const block = blocks[i]
    used += block.type === 'code' ? block.text.length : JSON.stringify(block).length * 2
    if (used >= budget && i > 2) return i
  }
  return blocks.length
}
