import { ref } from 'vue'
import { bridge, mockFileSystemAvailable } from './bridge'

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

/** новая навигация отбрасывает путь вперёд, а назад и вперёд двигают курсор */
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

  /** повторный заход в ту же папку не раздувает историю */
  reset(path: string): void {
    this.paths = [path]
    this.index = 0
  }
}

type FileBrowserMode = 'browse' | 'open' | 'directory'

export interface FileBrowserRequest {
  mode: FileBrowserMode
  /** расширения для режима open, например '.zip,.mrpack' */
  accept?: string
  title?: string
  startPath?: string
  /** файл, который нужно подсветить в списке (полный путь) */
  reveal?: string
  /** разрешить выбрать несколько файлов сразу */
  multiple?: boolean
  /** maximum bytes read when opening a file */
  maxSize?: number
}

export interface PickedFile {
  fileName: string
  /** base64 содержимого без префикса data-URL */
  data: string
  size: number
}

interface PickedDirectory {
  name: string
  path: string
}

export const fileBrowserRequest = ref<FileBrowserRequest | null>(null)
let fileBrowserResolve: ((value: PickedFile | null) => void) | null = null
let fileBrowserResolveMany: ((value: PickedFile[] | null) => void) | null = null
let directoryResolve: ((value: PickedDirectory | null) => void) | null = null

function isDesktopShell(): boolean {
  return typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

// выбор встроенного или системного проводника
export type FileExplorerMode = 'builtin' | 'system'
const FILE_EXPLORER_MODE_KEY = 'nivora-file-explorer-mode'
const explorerMode = ref<FileExplorerMode>(readStoredExplorerMode())

function readStoredExplorerMode(): FileExplorerMode {
  const stored = localStorage.getItem(FILE_EXPLORER_MODE_KEY)
  return stored === 'system' ? 'system' : 'builtin'
}

export function setExplorerMode(mode: FileExplorerMode): void {
  explorerMode.value = mode === 'system' ? 'system' : 'builtin'
  localStorage.setItem(FILE_EXPLORER_MODE_KEY, explorerMode.value)
}

export async function browsePath(path?: string, showHidden = false): Promise<FileBrowserListing> {
  return bridge.fsBrowse(path, { showHidden })
}

export async function readFileBase64(path: string, onProgress?: (loaded: number, total: number) => void): Promise<{ data: string; size: number }> {
  let offset = 0
  let total = 0
  let data = ''
  const maxSize = fileBrowserRequest.value?.mode === 'open' ? fileBrowserRequest.value.maxSize : undefined
  for (;;) {
    const chunk = await bridge.fsReadFile(path, offset)
    total = chunk.size
    if (maxSize !== undefined && total > maxSize) throw new Error(`Файл превышает лимит ${Math.floor(maxSize / (1024 * 1024))} MiB.`)
    if (maxSize !== undefined && offset + chunk.length > maxSize) throw new Error(`Файл превышает лимит ${Math.floor(maxSize / (1024 * 1024))} MiB.`)
    data += chunk.data
    offset += chunk.length
    onProgress?.(offset, total)
    if (chunk.eof) break
  }
  return { data, size: total }
}

function browserFallbackPick(accept: string, maxSize?: number): Promise<PickedFile | null> {
  return new Promise((resolve) => {
    const input = document.createElement('input')
    input.type = 'file'
    input.style.display = 'none'
    input.accept = accept
    let settled = false
    const finish = (value: PickedFile | null) => {
      if (settled) return
      settled = true
      input.remove()
      resolve(value)
    }
    input.addEventListener('change', () => {
      const file = input.files?.[0] || null
      if (!file) {
        finish(null)
        return
      }
      if (maxSize !== undefined && file.size > maxSize) {
        finish({ fileName: file.name, data: '', size: file.size })
        return
      }
      const reader = new FileReader()
      reader.onload = () => {
        const raw = String(reader.result || '')
        finish({ fileName: file.name, data: raw.replace(/^data:[^;]*;base64,/, ''), size: file.size })
      }
      reader.onerror = () => finish(null)
      reader.readAsDataURL(file)
    })
    // событие cancel завершает запрос и убирает скрытый input
    input.addEventListener('cancel', () => finish(null))
    document.body.appendChild(input)
    input.click()
  })
}

/** открывает picker по сохранённому режиму проводника, в браузере использует input */
export function pickFile(options: { accept?: string; title?: string; startPath?: string; maxSize?: number } = {}): Promise<PickedFile | null> {
  if (!isDesktopShell() || explorerMode.value === 'system') return browserFallbackPick(options.accept || '', options.maxSize)
  if (fileBrowserRequest.value) return Promise.resolve(null)
  return new Promise((resolve) => {
    fileBrowserResolve = resolve
    fileBrowserRequest.value = { mode: 'open', ...options }
  })
}

/** открывает каталог во встроенном или системном проводнике */
export function openInLauncherExplorer(path?: string, reveal?: string): void {
  // в браузере доступна только виртуальная файловая система
  if (!isDesktopShell() && !mockFileSystemAvailable) return
  if (explorerMode.value === 'system' && isDesktopShell()) {
    void bridge.fsReveal(reveal || path)
    return
  }
  fileBrowserRequest.value = { mode: 'browse', startPath: path, reveal }
}

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

/** выбирает папку без передачи её содержимого в интерфейс */
export function pickDirectory(options: { title?: string; startPath?: string } = {}): Promise<PickedDirectory | null> {
  if (!isDesktopShell()) return Promise.resolve(null)
  if (fileBrowserRequest.value) return Promise.resolve(null)
  return new Promise((resolve) => {
    directoryResolve = resolve
    fileBrowserRequest.value = { mode: 'directory', ...options }
  })
}

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
    let settled = false
    const finish = (value: PickedFile[] | null) => {
      if (settled) return
      settled = true
      input.remove()
      resolve(value)
    }
    input.addEventListener('change', () => {
      const files = [...(input.files || [])]
      if (!files.length) {
        finish(null)
        return
      }
      Promise.all(files.map((file) => new Promise<PickedFile>((resolveFile) => {
        const reader = new FileReader()
        reader.onload = () => resolveFile({ fileName: file.name, data: String(reader.result || '').replace(/^data:[^;]*;base64,/, ''), size: file.size })
        reader.onerror = () => resolveFile({ fileName: file.name, data: '', size: 0 })
        reader.readAsDataURL(file)
      }))).then(finish)
    })
    input.addEventListener('cancel', () => finish(null))
    document.body.appendChild(input)
    input.click()
  })
}

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
