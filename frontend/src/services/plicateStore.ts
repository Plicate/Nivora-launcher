import type { LauncherThemePreset } from '../types'
import type { PlicateFetcher } from './tauriFetch'

export type PlicateStoreKind = 'theme' | 'plugin'

export interface PlicateStoreItem {
  id: string
  name: string
  author: string
  description: string
  version: string
  kind: PlicateStoreKind
  download_url: string
  screenshots?: Array<{ url: string; mime?: string }>
  can_update?: boolean
  /** Палитра темы (приходит из details публикации или из встроенной темы). */
  background?: string
  panel?: string
  panel_soft?: string
  accent?: string
  accent_strong?: string
  /** Встроенная тема лаунчера: применяется локально, без скачивания архива. */
  preset?: boolean
}

export interface PlicateStoreApi {
  listCatalog(): Promise<PlicateStoreItem[]>
  download(item: PlicateStoreItem): Promise<{ name: string; data: string }>
  assetUrl(path: string): string
}

function storeApiBase(value: string): { origin: URL; apiBase: URL } {
  const raw = value.trim().replace(/\/+$/, '')
  if (!raw) throw new Error('Не задан адрес API Plicate Store и Rates.')
  let url: URL
  try { url = new URL(raw) } catch { throw new Error('Адрес Plicate API должен быть URL с HTTPS.') }
  const localHttp = url.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname)
  if (url.protocol !== 'https:' && !localHttp) throw new Error('Plicate API должен использовать HTTPS (кроме локальной разработки).')
  const basePath = url.pathname.replace(/\/+$/, '')
  if (url.username || url.password || !['', '/api'].includes(basePath) || url.search || url.hash) throw new Error('Укажите origin Plicate API или адрес сайта с путём /api.')
  const origin = new URL(url.origin)
  return { origin, apiBase: new URL('/api/', origin) }
}

function safeItem(value: unknown): value is PlicateStoreItem {
  if (!value || typeof value !== 'object') return false
  const item = value as Partial<PlicateStoreItem>
  return typeof item.id === 'string' && /^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/.test(item.id)
    && typeof item.name === 'string' && typeof item.author === 'string'
    && typeof item.description === 'string' && typeof item.version === 'string'
    && (item.kind === 'theme' || item.kind === 'plugin') && typeof item.download_url === 'string'
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  const chunkSize = 0x8000
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, Math.min(offset + chunkSize, bytes.length)))
  }
  return btoa(binary)
}

export function createPlicateStoreApi(baseUrl: string, fetcher: PlicateFetcher = fetch): PlicateStoreApi {
  const { origin, apiBase } = storeApiBase(baseUrl)
  async function get(url: URL): Promise<Response> {
    const response = await fetcher(url.toString(), { credentials: 'omit', headers: { Accept: 'application/json, application/zip' } })
    if (!response.ok) {
      let message = 'Не удалось связаться с Plicate Store.'
      try {
        const body = await response.json() as { error?: unknown }
        if (typeof body?.error === 'string') message = body.error
      } catch { /* binary or HTML response: use the generic error */ }
      throw new Error(message)
    }
    return response
  }

  return {
    assetUrl(path) {
      const url = new URL(path, origin)
      if (url.origin !== origin.origin || !/^\/api\/store\/media\/[a-f0-9]{32}\.(?:jpg|png|webp)$/.test(url.pathname)) throw new Error('Store вернул небезопасную ссылку на изображение.')
      return url.toString()
    },
    async listCatalog() {
      const response = await get(new URL('store/catalog', apiBase))
      if (!(response.headers.get('content-type') || '').includes('application/json')) throw new Error('Сервер вместо каталога вернул HTML или другой формат. Проверьте адрес Plicate API.')
      let data: { items?: unknown }
      try { data = await response.json() as { items?: unknown } } catch { throw new Error('Ответ каталога Plicate повреждён или не является JSON.') }
      return Array.isArray(data.items) ? data.items.filter(safeItem) : []
    },
    async download(item) {
      const url = new URL(item.download_url, origin)
      if (url.origin !== origin.origin || !/^\/api\/store\/items\/[a-z][a-z0-9]*(?:-[a-z0-9]+)*(?:\/v[a-f0-9]{24})?\.zip$/.test(url.pathname)) {
        throw new Error('Store вернул небезопасную ссылку на архив.')
      }
      const response = await get(url)
      const bytes = new Uint8Array(await response.arrayBuffer())
      if (!bytes.length || bytes.length > 64 * 1024 * 1024) throw new Error('Размер ZIP-архива должен быть от 1 байта до 64 MiB.')
      if (bytes[0] !== 0x50 || bytes[1] !== 0x4b) throw new Error('Вместо ZIP-архива сервер вернул другой файл. Проверьте адрес Plicate API и публикацию.')
      return { name: `${item.id}.zip`, data: bytesToBase64(bytes) }
    },
  }
}

export function compareStoreVersions(left: string, right: string): number {
  const tokenize = (value: string) => value.trim().replace(/^v/i, '').split(/[.+-]/).map((part) => /^\d+$/.test(part) ? Number(part) : part.toLowerCase())
  const a = tokenize(left)
  const b = tokenize(right)
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const x = a[index] ?? 0
    const y = b[index] ?? 0
    if (x === y) continue
    if (typeof x === 'number' && typeof y === 'number') return x < y ? -1 : 1
    if (typeof x === 'number') return 1
    if (typeof y === 'number') return -1
    return String(x).localeCompare(String(y))
  }
  return 0
}

export function presetStoreItems(themes: LauncherThemePreset[]): PlicateStoreItem[] {
  return themes
    .filter((theme) => theme.source === 'builtin' && theme.id && theme.name)
    .map((theme) => {
      const item: PlicateStoreItem = {
        id: theme.id,
        name: theme.name,
        author: theme.author || 'Nivora Launcher',
        description: theme.description || '',
        version: theme.version || '1.0.0',
        kind: 'theme',
        download_url: '',
        preset: true,
      }
      // Палитру добавляем только когда она задана: тесты и потребители каталога
      // не должны видеть ключи с пустыми значениями.
      for (const [key, value] of [['background', theme.background], ['panel', theme.panel], ['panel_soft', theme.panel_soft], ['accent', theme.accent], ['accent_strong', theme.accent_strong]] as const) {
        if (typeof value === 'string' && value) item[key] = value
      }
      return item
    })
}

export function isPresetStoreItem(item: PlicateStoreItem): boolean {
  return item.preset === true && item.kind === 'theme' && !item.download_url
}
