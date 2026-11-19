import { ref } from 'vue'
import { bridge } from './bridge'

/** пользовательский шрифт: файл хранится как data URL, имя показывает список */
export interface FontAsset {
  id: string
  name: string
  data: string
}

const FONT_ID_PATTERN = /^[a-z\d][a-z\d._-]{0,47}$/
const MAX_FONT_BYTES = 20 * 1024 * 1024
const MAX_FONTS = 8

const fontFamilies = new Set(['system', 'inter', 'manrope', 'mono'])

export function isFontAsset(value: unknown): value is FontAsset {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false
  const asset = value as Record<string, unknown>
  if (typeof asset.id !== 'string' || !FONT_ID_PATTERN.test(asset.id)) return false
  if (typeof asset.name !== 'string' || !asset.name.trim() || asset.name.length > 64) return false
  return isValidFontDataUrl(asset.data)
}

/** нормализация списка шрифтов из настроек: пропускаем битые записи, режем лимит */
export function normalizeFontAssets(value: unknown): FontAsset[] {
  if (!Array.isArray(value)) return []
  const seen = new Set<string>()
  const result: FontAsset[] = []
  for (const entry of value) {
    if (!isFontAsset(entry) || seen.has(entry.id)) continue
    seen.add(entry.id)
    result.push({ id: entry.id, name: entry.name, data: entry.data })
    if (result.length >= MAX_FONTS) break
  }
  return result
}

export function isValidFontDataUrl(value: unknown): boolean {
  if (typeof value !== 'string') return false
  if (!value.startsWith('data:font/') || !value.includes(';base64,')) return false
  const payload = value.slice(value.indexOf(',') + 1)
  const padding = payload.endsWith('==') ? 2 : payload.endsWith('=') ? 1 : 0
  const base64 = padding ? payload.slice(0, -padding) : payload
  if (!base64 || !/^[a-z\d+/]+$/i.test(base64)) return false
  if (padding ? base64.length % 4 !== 4 - padding : payload.length % 4 !== 0) return false
  const decodedBytes = Math.floor(payload.length * 3 / 4) - padding
  return decodedBytes > 0 && decodedBytes <= MAX_FONT_BYTES
}

function fontMimeFromExtension(fileName: string): 'font/ttf' | 'font/otf' | 'font/woff' | 'font/woff2' | null {
  const extension = fileName.slice(fileName.lastIndexOf('.')).toLowerCase()
  if (extension === '.ttf') return 'font/ttf'
  if (extension === '.otf') return 'font/otf'
  if (extension === '.woff') return 'font/woff'
  if (extension === '.woff2') return 'font/woff2'
  return null
}

function fontMimeFromBytes(bytes: Uint8Array): 'font/ttf' | 'font/otf' | 'font/woff' | 'font/woff2' | null {
  const ascii = (offset: number, length: number) => String.fromCharCode(...bytes.subarray(offset, offset + length))
  if (bytes.length >= 4 && bytes[0] === 0 && bytes[1] === 1 && bytes[2] === 0 && bytes[3] === 0) return 'font/ttf'
  if (bytes.length >= 4 && ascii(0, 4) === 'OTTO') return 'font/otf'
  if (bytes.length >= 4 && ascii(0, 4) === 'wOFF') return 'font/woff'
  if (bytes.length >= 4 && ascii(0, 4) === 'wOF2') return 'font/woff2'
  return null
}

export interface LoadedFont {
  id: string
  name: string
  family: string
  data: string
}

/** проверки расширенные, приёмка файла: расширение, содержимое и размер */
export function validateFontFile(fileName: string, declaredMime: string, bytes: Uint8Array): { mime: string; base64: string } | null {
  if (bytes.length === 0 || bytes.length > MAX_FONT_BYTES) return null
  const detected = fontMimeFromBytes(bytes)
  if (!detected) return null
  const extensionMime = fontMimeFromExtension(fileName)
  if (extensionMime && extensionMime !== detected) return null
  const declared = declaredMime.trim().toLowerCase().split(';', 1)[0]
  const typeIsGeneric = !declared || declared === 'application/octet-stream'
  if (extensionMime ? extensionMime !== detected : !typeIsGeneric && declared !== detected) return null
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000))
  }
  return { mime: detected, base64: btoa(binary) }
}

export function fontIdFromName(fileName: string): string {
  const base = fileName.replace(/\.[^.]+$/, '').toLowerCase().normalize('NFKD').replace(/[^a-z\d]+/g, '-').replace(/^-+|-+$/g, '')
  return (base || 'font').slice(0, 40)
}

export function uniqueFontId(base: string, existing: FontAsset[]): string {
  const taken = new Set(existing.map((font) => font.id))
  if (!taken.has(base)) return base
  for (let suffix = 2; ; suffix += 1) {
    const candidate = `${base.slice(0, 46 - String(suffix).length)}-${suffix}`
    if (!taken.has(candidate)) return candidate
  }
}

const FONT_FAMILY_PREFIX = 'custom:'
const SYSTEM_FONT_PREFIX = 'font:'

/** CSS-стек для выбранного гарнитуры, включая пользовательские и системные шрифты */
export function fontFamilyStack(value: string, fonts: FontAsset[]): string {
  const themeStack = themeFontFamilyStack(value)
  if (themeStack) return themeStack
  if (value.startsWith(FONT_FAMILY_PREFIX)) {
    const id = value.slice(FONT_FAMILY_PREFIX.length)
    const font = fonts.find((entry) => entry.id === id)
    if (!font) return 'system-ui, sans-serif'
    return `"Nivora Font ${id}", system-ui, sans-serif`
  }
  if (value.startsWith(SYSTEM_FONT_PREFIX)) {
    const family = value.slice(SYSTEM_FONT_PREFIX.length).trim().replace(/["\\]/g, '')
    if (!family) return 'system-ui, sans-serif'
    return `"${family}", system-ui, sans-serif`
  }
  if (fontFamilies.has(value)) {
    if (value === 'inter') return 'Inter, system-ui, sans-serif'
    if (value === 'manrope') return 'Manrope, system-ui, sans-serif'
    if (value === 'mono') return 'DM Mono, ui-monospace, monospace'
  }
  return 'system-ui, sans-serif'
}

/** CSS-семейство пользовательского шрифта из пресета темы */
export function themeFontFamilyStack(value: string): string {
  if (!value.startsWith('theme-font:')) return ''
  const family = value.slice('theme-font:'.length)
  if (!/^[a-z\d][a-z\d._-]{0,63}$/i.test(family)) return ''
  return `"${family}", system-ui, sans-serif`
}

const registeredFontFamilies = ref(new Set<string>())

/** регистрация FontFace в документе; повторные вызовы не дублируют шрифт */
export function registerFontFaces(family: string, data: string): void {
  if (typeof document === 'undefined' || typeof FontFace === 'undefined') return
  if (registeredFontFamilies.value.has(family)) return
  try {
    const face = new FontFace(family, `url(${data})`)
    const loaded = face.load()
    if (loaded instanceof Promise) loaded.then(() => document.fonts.add(face)).catch(() => {})
    else document.fonts.add(face)
    registeredFontFamilies.value.add(family)
  } catch {
    registeredFontFamilies.value.delete(family)
  }
}

/** регистрирует все пользовательские шрифты из настроек */
export function registerFontAssets(fonts: FontAsset[]): void {
  for (const font of fonts) registerFontFaces(`Nivora Font ${font.id}`, font.data)
}

/** ID пользовательского шрифта из CSS-семейства темы */
export function themeFontFamilyId(family: string): string {
  return family.replace(/^theme-font:/, '')
}

export function isFontFamilyValue(value: string): boolean {
  return fontFamilies.has(value) || value.startsWith(FONT_FAMILY_PREFIX) || value.startsWith(SYSTEM_FONT_PREFIX)
}

export function unregisterFontFamilies(): void {
  if (typeof document === 'undefined' || typeof document.fonts?.delete !== 'function') return
  for (const family of [...registeredFontFamilies.value]) {
    const faces = [...(document.fonts as unknown as Set<FontFace>)].filter((face) => face.family === family)
    for (const face of faces) document.fonts.delete(face)
    registeredFontFamilies.value.delete(family)
  }
}

/** гарнитуры, установленные в системе: список собирает rust-движок из каталогов шрифтов */
export async function getInstalledFontFamilies(): Promise<string[]> {
  try {
    const result = await bridge.fsFonts()
    const fonts = Array.isArray(result?.fonts) ? result.fonts : []
    return fonts
      .filter((name): name is string => typeof name === 'string' && name.trim().length > 0 && name.length <= 64)
      .slice(0, 500)
  } catch {
    return []
  }
}
