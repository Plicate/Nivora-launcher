// встроенный проводник и выбор файлов — вместо системных диалогов.
import { ref } from 'vue'
import { bridge } from './bridge'

export interface FileBrowserEntry {
  name: string
  kind: 'dir' | 'file'
  size: number | null
  modified_at: number | null
}

export interface FileBrowserListing {
  path: string
  parent: string | null
  root: { label: string; path: string } | null
  roots: Array<{ label: string; path: string }>
  entries: FileBrowserEntry[]
}

/**
 * История «назад/вперёд» как в обычных проводниках: navigate() усекает
 * forward-хвост, back/forward только передвигают курсор. Чистый класс,
 * чтобы его покрыть node-тестом вне Vue.
 */
export class PathHistory {
  private paths: string[] = []
  private index = -1

  get current(): string | null {
    return this.index >= 0 ? this.paths[this.index] : null
  }
  get canBack(): boolean {
    return this.index > 0
  }
  get canForward(): boolean {
    return this.index >= 0 && this.index < this.paths.length - 1
  }

  navigate(path: string): void {
    if (path === this.current) return
    this.paths.length = this.index + 1
    this.paths.push(path)
    this.index = this.paths.length - 1
  }

  back(): string | null {
    if (!this.canBack) return null
    this.index -= 1
    return this.paths[this.index]
  }

  forward(): string | null {
    if (!this.canForward) return null
    this.index += 1
    return this.paths[this.index]
  }

  /** повторный заход в ту же папку не раздувает историю. */
  reset(path: string): void {
    this.paths = [path]
    this.index = 0
  }
}

export type FileBrowserMode = 'browse' | 'open' | 'directory'

export interface FileBrowserRequest {
  mode: FileBrowserMode
  /** Расширения для режима open, например '.zip,.mrpack'. */
  accept?: string
  title?: string
  startPath?: string
  /** Разрешить выбрать несколько файлов сразу. */
  multiple?: boolean
}

export interface PickedFile {
  fileName: string
  /** base64 содержимого без префикса data-URL. */
  data: string
  size: number
}

export interface PickedDirectory {
  name: string
  path: string
}

/** Состояние открытого окна проводника; рендерит его App.vue. */
export const fileBrowserRequest = ref<FileBrowserRequest | null>(null)
let fileBrowserResolve: ((value: PickedFile | null) => void) | null = null
let fileBrowserResolveMany: ((value: PickedFile[] | null) => void) | null = null
let directoryResolve: ((value: PickedDirectory | null) => void) | null = null

export function isDesktopShell(): boolean {
  return typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

export async function browsePath(path?: string, showHidden = false): Promise<FileBrowserListing> {
  return bridge.fsBrowse(path, { showHidden })
}

/** Читает файл кусками и собирает в одну base64-строку. */
export async function readFileBase64(path: string, onProgress?: (loaded: number, total: number) => void): Promise<{ data: string; size: number }> {
  let offset = 0
  let total = 0
  let data = ''
  for (;;) {
    const chunk = await bridge.fsReadFile(path, offset)
    total = chunk.size
    data += chunk.data
    offset += chunk.length
    onProgress?.(offset, total)
    if (chunk.eof) break
  }
  return { data, size: total }
}

function browserFallbackPick(accept: string): Promise<PickedFile | null> {
  return new Promise((resolve) => {
    const input = document.createElement('input')
    input.type = 'file'
    input.style.display = 'none'
    input.accept = accept
    input.addEventListener('change', () => {
      const file = input.files?.[0] || null
      input.remove()
      if (!file) {
        resolve(null)
        return
      }
      const reader = new FileReader()
      reader.onload = () => {
        const raw = String(reader.result || '')
        resolve({ fileName: file.name, data: raw.replace(/^data:[^;]*;base64,/, ''), size: file.size })
      }
      reader.onerror = () => resolve(null)
      reader.readAsDataURL(file)
    })
    document.body.appendChild(input)
    input.click()
  })
}

/**
 * Выбор одного файла встроенным окном. В desktop-сборке открывает
 * FileBrowserDialog (там же живёт прогресс чтения больших файлов),
 * в браузерном предпросмотре — скрытый input.
 */
export function pickFile(options: { accept?: string; title?: string; startPath?: string } = {}): Promise<PickedFile | null> {
  if (!isDesktopShell()) return browserFallbackPick(options.accept || '')
  if (fileBrowserRequest.value) return Promise.resolve(null)
  return new Promise((resolve) => {
    fileBrowserResolve = resolve
    fileBrowserRequest.value = { mode: 'open', ...options }
  })
}

/** Открыть каталог во встроенном проводнике (вместо окна ОС). */
export function openInLauncherExplorer(path?: string): void {
  if (!isDesktopShell()) return
  fileBrowserRequest.value = { mode: 'browse', startPath: path }
}

/** Выбор нескольких файлов сразу (загрузка файлов в экземпляр). */
export function pickFiles(options: { accept?: string; title?: string; startPath?: string } = {}): Promise<PickedFile[] | null> {
  if (!isDesktopShell()) {
    return browserFallbackPickMany(options.accept || '')
  }
  if (fileBrowserRequest.value) return Promise.resolve(null)
  return new Promise((resolve) => {
    fileBrowserResolveMany = resolve
    fileBrowserRequest.value = { mode: 'open', multiple: true, ...options }
  })
}

/** Выбор папки: путь отдаёт бэкенд, содержимое читается на месте — без выгрузки в интерфейс. */
export function pickDirectory(options: { title?: string; startPath?: string } = {}): Promise<PickedDirectory | null> {
  if (!isDesktopShell()) return Promise.resolve(null)
  if (fileBrowserRequest.value) return Promise.resolve(null)
  return new Promise((resolve) => {
    directoryResolve = resolve
    fileBrowserRequest.value = { mode: 'directory', ...options }
  })
}

/** Готовый ответ для режима выбора папки. */
export function resolveFileBrowserDirectory(value: PickedDirectory): void {
  const resolve = directoryResolve
  directoryResolve = null
  resolve?.(value)
}

function browserFallbackPickMany(accept: string): Promise<PickedFile[] | null> {
  return new Promise((resolve) => {
    const input = document.createElement('input')
    input.type = 'file'
    input.style.display = 'none'
    input.accept = accept
    input.multiple = true
    input.addEventListener('change', () => {
      const files = [...(input.files || [])]
      input.remove()
      if (!files.length) {
        resolve(null)
        return
      }
      Promise.all(files.map((file) => new Promise<PickedFile>((resolveFile) => {
        const reader = new FileReader()
        reader.onload = () => resolveFile({ fileName: file.name, data: String(reader.result || '').replace(/^data:[^;]*;base64,/, ''), size: file.size })
        reader.onerror = () => resolveFile({ fileName: file.name, data: '', size: 0 })
        reader.readAsDataURL(file)
      }))).then(resolve)
    })
    document.body.appendChild(input)
    input.click()
  })
}

/** Вызывается диалогом из App.vue при выборе файла(ов) или закрытии. */
export function resolveFileBrowser(file: PickedFile | null): void {
  fileBrowserRequest.value = null
  fileBrowserResolve?.(file)
  fileBrowserResolve = null
}

export function resolveFileBrowserMany(files: PickedFile[] | null): void {
  fileBrowserRequest.value = null
  fileBrowserResolveMany?.(files)
  fileBrowserResolveMany = null
}
