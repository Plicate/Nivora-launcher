import { invoke } from '@tauri-apps/api/core'

/// Загрузчик для API Plicate: сначала обычный fetch веб-окружения, при сетевом сбое
/// (CSP, CORS, особенности WebKitGTK) запрос повторяется через бэкенд лаунчера,
/// поэтому блокировка веб-окружения не может оставить страницу без данных.
export type PlicateFetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>

interface BackendHttpResponse {
  status: number
  contentType: string
  bodyBase64: string
}

function inTauriRuntime(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window
}

export const plicateFetch: PlicateFetcher = async (input, init = {}) => {
  try {
    return await fetch(input, init)
  } catch (error) {
    if (!inTauriRuntime()) throw error
    const url = typeof input === 'string' ? input : input instanceof URL ? input.toString() : input.url
    const headers: Record<string, string> = {}
    if (init.headers) new Headers(init.headers).forEach((value, key) => { headers[key] = value })
    let backend: BackendHttpResponse
    try {
      backend = await invoke<BackendHttpResponse>('plicate_http_fetch', {
        payload: {
          url,
          method: init.method || 'GET',
          headers,
          body: typeof init.body === 'string' ? init.body : undefined,
        },
      })
    } catch (backendError) {
      const reason = backendError instanceof Error ? backendError.message : String(backendError)
      throw new Error(`Сервер Plicate недоступен (${reason}). Проверьте подключение и повторите.`)
    }
    const bytes = Uint8Array.from(atob(backend.bodyBase64), (char) => char.charCodeAt(0))
    const responseHeaders = new Headers()
    if (backend.contentType) responseHeaders.set('content-type', backend.contentType)
    return new Response(bytes, { status: backend.status, headers: responseHeaders })
  }
}
