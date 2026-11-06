import type { InstanceInfo } from '../types'

export const DEFAULT_PLICATE_API_BASE_URL = 'https://plicate.art/api'

/// цель подключения из ссылки nivora://join (кнопка «Быстрый вход» на сайте Plicate Rates)
export interface PlicateJoinTarget {
  host: string
  port: number
  edition: string
  name: string
  slug: string
}

export interface PlicateRatesServer {
  id?: number
  slug: string
  name: string
  host: string
  port: number
  edition: 'Java' | 'Bedrock' | string
  online: number | null
  maximum: number | null
  version?: string | null
  motd?: string
  icon?: string | null
  category?: string
  description?: string
  rating?: number | null
  reviews?: number
  tags?: string[]
  rank?: number
}

export interface PlicateRatesCatalog {
  servers: PlicateRatesServer[]
  categories: string[]
}

/// собирает сервер для диалога выбора экземпляра из цели nivora://join
export function externalJoinServer(target: { host: string; port?: number | string; edition?: string; name?: string }): PlicateRatesServer | null {
  const host = String(target.host || '').trim()
  if (!host) return null
  const edition = target.edition === 'Bedrock' ? 'Bedrock' : 'Java'
  return {
    slug: 'quick-join',
    name: String(target.name || '').trim() || host,
    host,
    port: Number(target.port) || (edition === 'Bedrock' ? 19132 : 25565),
    edition,
    online: null,
    maximum: null,
  }
}

export interface PlicateRatesApi {
  listServers(query?: string): Promise<PlicateRatesCatalog>
  recordCopy(slug: string): Promise<void>
}

export function isRatesJoinableInstance(instance: InstanceInfo): boolean {
  return instance.kind !== 'server' && instance.game_edition !== 'bedrock'
}

export function serverAddress(server: Pick<PlicateRatesServer, 'host' | 'port' | 'edition'>): string {
  const defaultPort = server.edition === 'Bedrock' ? 19132 : 25565
  return server.port === defaultPort ? server.host : `${server.host}:${server.port}`
}

export function createPlicateRatesApi(baseUrl: string, fetcher: typeof fetch = fetch): PlicateRatesApi {
  const raw = baseUrl.trim().replace(/\/+$/, '')
  if (!raw) throw new Error('Не задан адрес API Plicate Rates.')
  let parsedBase: URL
  try { parsedBase = new URL(raw) } catch { throw new Error('Адрес Plicate API должен быть URL с HTTPS.') }
  const localHttp = parsedBase.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(parsedBase.hostname)
  if (parsedBase.protocol !== 'https:' && !localHttp) throw new Error('Plicate API должен использовать HTTPS (кроме локальной разработки).')
  const basePath = parsedBase.pathname.replace(/\/+$/, '')
  if (parsedBase.username || parsedBase.password || !['', '/api'].includes(basePath) || parsedBase.search || parsedBase.hash) throw new Error('Укажите origin Plicate API или адрес сайта с путём /api.')
  const apiBase = new URL('/api/', parsedBase.origin)

  async function request<T>(path: string, init?: RequestInit): Promise<T> {
    const response = await fetcher(new URL(path, apiBase).toString(), {
      credentials: 'omit',
      ...init,
      headers: { Accept: 'application/json', ...(init?.headers || {}) },
    })
    const contentType = response.headers.get('content-type') || ''
    const data: unknown = contentType.includes('application/json') ? await response.json() : await response.text()
    if (!response.ok) {
      const message = typeof data === 'object' && data !== null && 'error' in data
        ? String((data as { error: unknown }).error)
        : `Plicate Rates вернул HTTP ${response.status} вместо JSON API.`
      throw new Error(message)
    }
    if (!contentType.includes('application/json')) throw new Error('Сервер вместо каталога вернул HTML или другой формат. Проверьте адрес Plicate API.')
    return data as T
  }

  return {
    async listServers(query = '') {
      const params = new URLSearchParams()
      if (query.trim()) params.set('q', query.trim())
      const suffix = params.size ? `?${params.toString()}` : ''
      const result = await request<Partial<PlicateRatesCatalog>>(`servers${suffix}`)
      return {
        servers: Array.isArray(result.servers) ? result.servers : [],
        categories: Array.isArray(result.categories) ? result.categories : [],
      }
    },
    async recordCopy(slug) {
      await request(`servers/${encodeURIComponent(slug)}/copy`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      })
    },
  }
}
