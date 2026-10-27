<script setup lang="ts">
// встроенный проводник лаунчера; заменяет и окно ОС "Открыть папку", и
// системный выбор файлов: история назад/вперёд (включая кнопки мыши),
// адресная строка с вводом пути, сортировка, боковая панель корней
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import {
  PathHistory,
  browsePath,
  readFileBase64,
  type FileBrowserEntry,
  type FileBrowserListing,
  type FileBrowserRequest,
  type PickedFile,
} from '../services/fileBrowser'
import { bridge } from '../services/bridge'
import { closeContextMenu, openContextMenu, openContextMenuAt } from '../services/contextMenu'
import { confirmDialog, promptDialog } from '../services/dialogs'

const props = withDefaults(defineProps<{ request: FileBrowserRequest; editorMode?: 'builtin' | 'system' }>(), { editorMode: 'builtin' })
const emit = defineEmits<{
  (event: 'close'): void
  (event: 'chosen', file: PickedFile): void
  (event: 'chosen-many', files: PickedFile[]): void
  (event: 'chosen-directory', directory: { name: string; path: string }): void
  (event: 'toast', message: string, tone?: 'default' | 'muted' | 'success' | 'warning' | 'danger'): void
}>()

const listing = ref<FileBrowserListing | null>(null)
const currentPath = ref('')
const loading = ref(false)
const error = ref('')
const filter = ref('')
const selected = ref<FileBrowserEntry | null>(null)
// мультивыбор: имена выбранных файлов (папки в выбор не попадают)
const selectedMany = ref<Set<string>>(new Set())
const reading = ref(0)
const readingTotal = ref(0)
const dialogElement = ref<HTMLElement | null>(null)
const listElement = ref<HTMLElement | null>(null)
const addressInput = ref<HTMLInputElement | null>(null)

// история навигации: реактивный счётчик отмечает изменения PathHistory
// (класс не реактивен), кнопки пересчитываются по нему
const history = new PathHistory()
const historyTick = ref(0)
const canBack = computed(() => (historyTick.value, history.canBack))
const canForward = computed(() => (historyTick.value, history.canForward))

type SortKey = 'name' | 'size' | 'modified'
const sortKey = ref<SortKey>('name')
const sortAsc = ref(true)

const addressEditing = ref(false)
const addressDraft = ref('')
const textEditorPath = ref('')
const textEditorName = ref('')
const textEditorContent = ref('')
const textEditorOriginal = ref('')
const textEditorOriginalData = ref('')
const textEditorNewline = ref('\n')
const textEditorBusy = ref(false)
const textEditorError = ref('')
const MAX_TEXT_EDITOR_BYTES = 2 * 1024 * 1024

// "Показать скрытые" переживает переоткрытия окна (сессия)
const showHidden = ref(localStorage.getItem('nivora-filebrowser-show-hidden') === '1')

async function toggleShowHidden() {
  showHidden.value = !showHidden.value
  localStorage.setItem('nivora-filebrowser-show-hidden', showHidden.value ? '1' : '0')
  await load(currentPath.value || undefined, { record: false, keepFilter: true })
}

const MIME_EXTENSIONS: Record<string, string[]> = {
  'image/png': ['.png'],
  'image/jpeg': ['.jpg', '.jpeg'],
  'image/webp': ['.webp'],
  'image/gif': ['.gif'],
  'application/zip': ['.zip'],
  'application/octet-stream': ['.bin'],
}

const acceptExtensions = computed(() => {
  const result: string[] = []
  for (const value of (props.request.accept || '').split(',')) {
    const raw = value.trim().toLowerCase()
    if (!raw) continue
    if (raw.startsWith('.')) {
      result.push(raw)
    } else if (MIME_EXTENSIONS[raw]) {
      result.push(...MIME_EXTENSIONS[raw])
    }
  }
  return result
})

const entries = computed(() => {
  const query = filter.value.trim().toLowerCase()
  let list = listing.value?.entries || []
  if (query) list = list.filter((entry) => entry.name.toLowerCase().includes(query))
  const direction = sortAsc.value ? 1 : -1
  const by: Record<SortKey, (a: FileBrowserEntry, b: FileBrowserEntry) => number> = {
    name: (a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }),
    size: (a, b) => (a.size ?? -1) - (b.size ?? -1),
    modified: (a, b) => (a.modified_at ?? 0) - (b.modified_at ?? 0),
  }
  const cmp = (a: FileBrowserEntry, b: FileBrowserEntry) => direction * by[sortKey.value](a, b)
  // группируем папки выше файлов
  const dirs = list.filter((entry) => entry.kind === 'dir').sort(cmp)
  const files = list.filter((entry) => entry.kind === 'file').sort(cmp)
  return [...dirs, ...files]
})

const entryCount = computed(() => (listing.value?.entries || []).length)

const title = computed(() => props.request.title || (props.request.mode === 'open' ? 'Выбор файла' : props.request.mode === 'directory' ? 'Выбор папки' : 'Проводник'))

const homeRoot = computed(() => listing.value?.roots.find((root) => root.label === 'Дом')?.path || listing.value?.roots[0]?.path || '')

async function load(path?: string, options: { record?: boolean; keepFilter?: boolean } = {}) {
  if (loading.value) return
  loading.value = true
  error.value = ''
  selected.value = null
  selectedMany.value = new Set()
  try {
    const result = await browsePath(path, showHidden.value)
    listing.value = result
    currentPath.value = result.path
    if (!options.keepFilter) filter.value = ''
    if (options.record !== false) history.navigate(result.path)
    historyTick.value += 1
    revealRequestedEntry()
  } catch (loadError) {
    error.value = loadError instanceof Error ? loadError.message : String(loadError)
  } finally {
    loading.value = false
  }
}

// подсветка нового файла
const revealedName = ref('')

function revealRequestedEntry() {
  const reveal = props.request.reveal
  if (!reveal || !listing.value) return
  const normalized = reveal.replace(/[\\/]+$/, '')
  const dir = normalized.replace(/[\\/][^\\/]*$/, '')
  const name = normalized.split(/[\\/]/).pop() || ''
  const current = currentPath.value.replace(/[\\/]+$/, '')
  if (!name || dir !== current) return
  const entry = listing.value.entries.find((item) => item.name === name)
  if (!entry) return
  selected.value = entry
  revealedName.value = name
  void nextTick(() => {
    listElement.value?.querySelector(`[data-entry-name="${CSS.escape(name)}"]`)?.scrollIntoView({ block: 'center' })
  })
}

async function navigateTo(path: string) {
  await load(path)
}

async function goBack() {
  const target = history.back()
  historyTick.value += 1
  if (target) await load(target, { record: false })
}

async function goForward() {
  const target = history.forward()
  historyTick.value += 1
  if (target) await load(target, { record: false })
}

async function goUp() {
  if (listing.value?.parent) await navigateTo(listing.value.parent)
}

async function refresh() {
  if (!currentPath.value) return
  await load(currentPath.value, { record: false, keepFilter: true })
}

const mutating = ref(false)

function pathContains(root: string, path: string): boolean {
  const normalize = (value: string) => value.replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase()
  const base = normalize(root)
  const candidate = normalize(path)
  return candidate === base || candidate.startsWith(`${base}/`)
}

function clearTextEditor() {
  textEditorPath.value = ''
  textEditorName.value = ''
  textEditorContent.value = ''
  textEditorOriginal.value = ''
  textEditorOriginalData.value = ''
  textEditorError.value = ''
}

async function closeEditorBeforeDelete(path: string, directory: boolean): Promise<boolean> {
  if (!textEditorPath.value || !(directory ? pathContains(path, textEditorPath.value) : pathContains(path, textEditorPath.value) && pathContains(textEditorPath.value, path))) return true
  if (textEditorContent.value !== textEditorOriginal.value && !(await confirmDialog('Закрыть файл без сохранения и продолжить?', { danger: true, confirmLabel: 'Закрыть и продолжить' }))) return false
  clearTextEditor()
  return true
}

function selectNamedEntry(name: string) {
  const entry = listing.value?.entries.find((candidate) => candidate.name === name)
  if (!entry) return
  selected.value = entry
  revealedName.value = name
  void nextTick(() => listElement.value?.querySelector(`[data-entry-name="${CSS.escape(name)}"]`)?.scrollIntoView({ block: 'center' }))
}

async function createDirectory() {
  if (!currentPath.value || mutating.value) return
  const name = (await promptDialog('Название новой папки', { placeholder: 'Например, config', confirmLabel: 'Создать' }))?.trim()
  if (!name) return
  mutating.value = true
  error.value = ''
  try {
    const result = await bridge.fsCreateDirectory(currentPath.value, name)
    await refresh()
    selectNamedEntry(result.path.split(/[\\/]/).pop() || name)
    emit('toast', 'Папка создана', 'success')
  } catch (createError) {
    error.value = createError instanceof Error ? createError.message : String(createError)
  } finally {
    mutating.value = false
  }
}

async function createFile() {
  if (!currentPath.value || mutating.value) return
  const name = (await promptDialog('Имя нового файла', { placeholder: 'Например, notes.txt', confirmLabel: 'Создать' }))?.trim()
  if (!name) return
  mutating.value = true
  error.value = ''
  try {
    const result = await bridge.fsCreateFile(currentPath.value, name)
    await refresh()
    const createdName = result.path.split(/[\\/]/).pop() || name
    selectNamedEntry(createdName)
    const entry = listing.value?.entries.find((candidate) => candidate.name === createdName)
    if (entry) await openTextFile(entry)
    emit('toast', 'Файл создан', 'success')
  } catch (createError) {
    error.value = createError instanceof Error ? createError.message : String(createError)
  } finally {
    mutating.value = false
  }
}

async function renameEntry(entry: FileBrowserEntry) {
  if (mutating.value) return
  const oldPath = entryPath(entry)
  const newName = (await promptDialog('Новое имя', { initialValue: entry.name, confirmLabel: 'Переименовать' }))?.trim()
  if (!newName || newName === entry.name) return
  mutating.value = true
  error.value = ''
  try {
    const result = await bridge.fsRename(oldPath, newName)
    if (textEditorPath.value && (entry.kind === 'dir' ? pathContains(oldPath, textEditorPath.value) : textEditorPath.value === oldPath)) {
      textEditorPath.value = `${result.path}${textEditorPath.value.slice(oldPath.length)}`
      if (entry.kind === 'file') textEditorName.value = newName
    }
    await refresh()
    selectNamedEntry(result.path.split(/[\\/]/).pop() || newName)
    emit('toast', 'Переименовано', 'success')
  } catch (renameError) {
    error.value = renameError instanceof Error ? renameError.message : String(renameError)
  } finally {
    mutating.value = false
  }
}

async function deleteEntry(entry: FileBrowserEntry) {
  if (mutating.value) return
  const path = entryPath(entry)
  const isDirectory = entry.kind === 'dir'
  const description = isDirectory ? `Удалить папку «${entry.name}» и всё её содержимое?` : `Удалить файл «${entry.name}»?`
  if (!(await confirmDialog(description, { danger: true, confirmLabel: 'Удалить' }))) return
  if (!(await closeEditorBeforeDelete(path, isDirectory))) return
  mutating.value = true
  error.value = ''
  try {
    await bridge.fsDelete(path)
    await refresh()
    emit('toast', isDirectory ? 'Папка удалена' : 'Файл удалён', 'success')
  } catch (deleteError) {
    error.value = deleteError instanceof Error ? deleteError.message : String(deleteError)
  } finally {
    mutating.value = false
  }
}

// адресная строка

async function startAddressEdit() {
  addressDraft.value = currentPath.value
  addressEditing.value = true
  await nextTick()
  addressInput.value?.focus()
  addressInput.value?.select()
}

function cancelAddressEdit() {
  addressEditing.value = false
  addressDraft.value = ''
}

/** «~» обозначает домашний каталог */
function expandTilde(value: string): string {
  if (value === '~' || value.startsWith('~/') || value.startsWith('~\\')) {
    if (homeRoot.value) return homeRoot.value.replace(/[\\/]+$/, '') + value.slice(1)
  }
  return value
}

async function applyAddress() {
  const raw = addressDraft.value.trim()
  addressEditing.value = false
  if (!raw || expandTilde(raw) === currentPath.value) return
  await navigateTo(expandTilde(raw))
}

// крошки

function crumbs(): Array<{ label: string; path: string }> {
  const raw = currentPath.value
  if (!raw) return []
  const separator = raw.includes('\\') ? '\\' : '/'
  const drive = /^[A-Za-z]:/.exec(raw)?.[0] || ''
  const result: Array<{ label: string; path: string }> = []
  let acc = raw.startsWith('\\\\') ? '\\\\' : ''
  for (const part of raw.split(/[\\/]+/).filter(Boolean)) {
    // в Windows путь к корню диска включает обратную косую черту
    if (drive && part === drive) {
      acc = drive
      result.push({ label: `${drive}${separator}`, path: `${drive}${separator}` })
      continue
    }
    acc = acc ? (acc.endsWith(separator) ? acc + part : acc + separator + part) : `/${part}`
    result.push({ label: part, path: acc })
  }
  return result
}

// навигация по списку

function entryPath(entry: FileBrowserEntry): string {
  const base = currentPath.value.replace(/[\\/]+$/, '')
  const separator = currentPath.value.includes('\\') ? '\\' : '/'
  return `${base}${separator}${entry.name}`
}

async function openEntry(entry: FileBrowserEntry, event?: MouseEvent) {
  if (entry.kind === 'dir') {
    await navigateTo(entryPath(entry))
    return
  }
  if (props.request.mode === 'browse') {
    await openTextFile(entry)
    return
  }
  if (props.request.mode !== 'open') {
    entryMenu(entry, event || null)
    return
  }
  if (acceptExtensions.value.length && !acceptExtensions.value.some((extension) => entry.name.toLowerCase().endsWith(extension))) {
    error.value = `Подходят файлы: ${acceptExtensions.value.join(', ')}`
    return
  }
  await chooseEntry(entry)
}

function toggleSort(key: SortKey) {
  if (sortKey.value === key) {
    sortAsc.value = !sortAsc.value
  } else {
    sortKey.value = key
    sortAsc.value = true
  }
}

function sortIndicator(key: SortKey): string {
  return sortKey.value === key ? (sortAsc.value ? '↑' : '↓') : ''
}

function moveSelection(step: number) {
  const list = entries.value
  if (!list.length) return
  const index = selected.value ? list.findIndex((entry) => entry.name === selected.value?.name) : -1
  const next = index < 0 ? (step > 0 ? 0 : list.length - 1) : Math.min(list.length - 1, Math.max(0, index + step))
  selected.value = list[next]
  listElement.value
    ?.querySelector('.file-browser-row--selected')
    ?.scrollIntoView({ block: 'nearest' })
}

async function readEntry(entry: FileBrowserEntry): Promise<PickedFile> {
  reading.value = 1
  readingTotal.value = entry.size || 1
  error.value = ''
  try {
    const file = await readFileBase64(entryPath(entry), (loaded, total) => {
      reading.value = loaded
      readingTotal.value = total
    })
    return { fileName: entry.name, data: file.data, size: file.size }
  } finally {
    reading.value = 0
  }
}

async function readUtf8Text(path: string): Promise<{ content: string; newline: string; rawBase64: string }> {
  const chunks: Uint8Array[] = []
  let offset = 0
  let total = 0
  let eof = false
  while (!eof) {
    const chunk = await bridge.fsReadFile(path, offset, 64 * 1024)
    if (chunk.size > MAX_TEXT_EDITOR_BYTES) throw new Error('Редактор поддерживает файлы до 2 MiB')
    const binary = atob(chunk.data)
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
    total += bytes.length
    if (total > MAX_TEXT_EDITOR_BYTES) throw new Error('Редактор поддерживает файлы до 2 MiB')
    if (bytes.includes(0)) throw new Error('Похоже, это бинарный файл — встроенный редактор его не открывает')
    chunks.push(bytes)
    offset += chunk.length
    eof = chunk.eof
    if (!chunk.length && !eof) throw new Error('Чтение файла остановилось раньше конца')
  }
  const all = new Uint8Array(total)
  let cursor = 0
  for (const chunk of chunks) {
    all.set(chunk, cursor)
    cursor += chunk.length
  }
  let decoded: string
  try {
    decoded = new TextDecoder('utf-8', { fatal: true }).decode(all)
  } catch {
    throw new Error('Файл не является текстом UTF-8')
  }
  const newline = decoded.includes('\r\n') ? '\r\n' : '\n'
  return { content: decoded.replace(/\r\n/g, '\n'), newline, rawBase64: bytesToBase64(all) }
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000))
  }
  return btoa(binary)
}

function textToBase64(content: string): string {
  return bytesToBase64(new TextEncoder().encode(content))
}

async function openTextFile(entry: FileBrowserEntry) {
  const path = entryPath(entry)
  if (textEditorPath.value && textEditorContent.value !== textEditorOriginal.value && !(await confirmDialog('Закрыть файл и потерять несохранённые изменения?', { danger: true, confirmLabel: 'Закрыть без сохранения' }))) return
  if (props.editorMode === 'system') {
    try {
      await bridge.fsOpenFile(path)
      textEditorPath.value = ''
      textEditorName.value = ''
      textEditorContent.value = ''
      textEditorOriginal.value = ''
      textEditorOriginalData.value = ''
    } catch (openError) {
      error.value = openError instanceof Error ? openError.message : String(openError)
    }
    return
  }
  textEditorBusy.value = true
  textEditorError.value = ''
  try {
    const file = await readUtf8Text(path)
    textEditorPath.value = path
    textEditorName.value = entry.name
    textEditorContent.value = file.content
    textEditorOriginal.value = file.content
    textEditorOriginalData.value = file.rawBase64
    textEditorNewline.value = file.newline
  } catch (openError) {
    error.value = openError instanceof Error ? openError.message : String(openError)
  } finally {
    textEditorBusy.value = false
  }
}

async function openSelectedFile() {
  if (selected.value?.kind === 'file') await openTextFile(selected.value)
}

async function closeTextEditor() {
  if (textEditorBusy.value) return
  if (textEditorContent.value !== textEditorOriginal.value && !(await confirmDialog('Закрыть файл и потерять несохранённые изменения?', { danger: true, confirmLabel: 'Закрыть без сохранения' }))) return
  textEditorPath.value = ''
  textEditorName.value = ''
  textEditorContent.value = ''
  textEditorOriginal.value = ''
  textEditorOriginalData.value = ''
  textEditorError.value = ''
}

async function saveTextEditor() {
  if (!textEditorPath.value || textEditorBusy.value || textEditorContent.value === textEditorOriginal.value) return
  textEditorBusy.value = true
  textEditorError.value = ''
  try {
    const diskContent = textEditorContent.value.replace(/\n/g, textEditorNewline.value)
    const encoded = textToBase64(diskContent)
    await bridge.fsSaveTextFile(textEditorPath.value, textEditorOriginalData.value, encoded)
    textEditorOriginalData.value = encoded
    textEditorOriginal.value = textEditorContent.value
    emit('toast', 'Файл сохранён')
    await load(currentPath.value, { record: false, keepFilter: true })
  } catch (saveError) {
    textEditorError.value = saveError instanceof Error ? saveError.message : String(saveError)
  } finally {
    textEditorBusy.value = false
  }
}

async function chooseEntry(entry: FileBrowserEntry) {
  if (entry.kind !== 'file' || reading.value) return
  try {
    if (props.request.multiple) {
      emit('chosen-many', [await readEntry(entry)])
      emit('close')
      return
    }
    emit('chosen', await readEntry(entry))
    emit('close')
  } catch (readError) {
    error.value = readError instanceof Error ? readError.message : String(readError)
  }
}

async function chooseSelection() {
  if (reading.value) return
  if (!props.request.multiple) {
    if (selected.value) await chooseEntry(selected.value)
    return
  }
  const picked = (listing.value?.entries || []).filter(
    (entry) => entry.kind === 'file' && selectedMany.value.has(entry.name),
  )
  if (!picked.length) return
  try {
    const files: PickedFile[] = []
    for (const entry of picked) files.push(await readEntry(entry))
    emit('chosen-many', files)
    emit('close')
  } catch (readError) {
    error.value = readError instanceof Error ? readError.message : String(readError)
  }
}

function toggleMany(entry: FileBrowserEntry) {
  if (entry.kind !== 'file') return
  const next = new Set(selectedMany.value)
  if (next.has(entry.name)) next.delete(entry.name)
  else next.add(entry.name)
  selectedMany.value = next
}

async function saveEntryCopy(entry: FileBrowserEntry) {
  try {
    const file = await readFileBase64(entryPath(entry))
    const binary = atob(file.data)
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
    const url = URL.createObjectURL(new Blob([bytes], { type: guessMime(entry.name) }))
    const link = document.createElement('a')
    link.href = url
    link.download = entry.name
    link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 1000)
  } catch (readError) {
    error.value = readError instanceof Error ? readError.message : String(readError)
  }
}

function entryMenu(entry: FileBrowserEntry, event: MouseEvent | null) {
  // правый клик и двойной клик выделяют файл: без этого меню открывалось
  // у неотмеченной строки, а статусбар молчал о выбранном элементе
  selected.value = entry
  const items = [
    {
      id: 'copy-path',
      label: 'Копировать путь',
      icon: 'copy',
      action: () => void navigator.clipboard?.writeText(entryPath(entry)),
    },
    {
      id: 'info',
      label: 'Свойства',
      icon: 'info',
      action: () => {
        selected.value = entry
      },
    },
  ]
  if (entry.kind === 'file') {
    items.push({ id: 'save-copy', label: 'Сохранить копию…', icon: 'download', action: () => void saveEntryCopy(entry) })
    if (props.request.mode === 'browse') items.push({ id: 'edit-file', label: props.editorMode === 'system' ? 'Открыть в приложении ОС' : 'Открыть в редакторе', icon: 'edit', action: () => void openTextFile(entry) })
  }
  if (props.request.mode === 'browse') {
    items.push({ id: 'rename', label: 'Переименовать', icon: 'edit', action: () => void renameEntry(entry) })
    items.push({ id: 'delete', label: 'Удалить', icon: 'trash', action: () => void deleteEntry(entry) })
  }
  if (props.request.mode === 'open' && entry.kind === 'file') {
    items.push({ id: 'choose', label: 'Выбрать файл', icon: 'check', action: () => void chooseEntry(entry) })
  }
  if (props.request.mode === 'directory' && entry.kind === 'dir') {
    items.push({ id: 'choose-dir', label: 'Выбрать папку', icon: 'check', action: chooseDirectory })
  }
  if (event) {
    openContextMenu(event, items, entry.name)
    return
  }
  // вызов без события (Enter с клавиатуры): anchoring по строке файла
  const row = listElement.value?.querySelector(`[data-entry-name="${CSS.escape(entry.name)}"]`) as HTMLElement | null
  const rect = row?.getBoundingClientRect()
  openContextMenuAt(rect ? Math.min(rect.left + 28, window.innerWidth - 16) : window.innerWidth / 2, rect ? Math.min(rect.bottom + 6, window.innerHeight - 16) : window.innerHeight / 2, items, entry.name)
}

// режим выбора папки: текущий каталог или выделенная папка
function chooseDirectory() {
  const target = selected.value?.kind === 'dir' ? selected.value : null
  const path = target ? entryPath(target) : listing.value?.path
  if (!path) return
  const name = target?.name || path.split(/[\\/]/).filter(Boolean).pop() || 'Папка'
  emit('chosen-directory', { name, path })
  emit('close')
}

function isActiveRoot(root: { label: string; path: string }): boolean {
  const normalize = (value: string) => value.replace(/[\\/]+$/, '')
  return Boolean(currentPath.value) && normalize(currentPath.value).startsWith(normalize(root.path))
}

// приём файлов из системы
const dropActive = ref(false)
const dropBusy = ref(false)
let dropDepth = 0

function hasDraggedFiles(event: DragEvent): boolean {
  return Boolean(event.dataTransfer?.types.includes('Files'))
}

function onDragEnter(event: DragEvent) {
  if (!hasDraggedFiles(event)) return
  dropDepth += 1
  dropActive.value = true
}

function onDragOver(event: DragEvent) {
  if (!hasDraggedFiles(event)) return
  event.preventDefault()
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy'
  dropActive.value = true
}

function onDragLeave(event: DragEvent) {
  if (!hasDraggedFiles(event)) return
  dropDepth = Math.max(0, dropDepth - 1)
  if (!dropDepth) dropActive.value = false
}

function readDroppedFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result || '').replace(/^data:[^;]*;base64,/, ''))
    reader.onerror = () => reject(new Error(`Не удалось прочитать ${file.name}`))
    reader.readAsDataURL(file)
  })
}

async function onDrop(event: DragEvent) {
  dropDepth = 0
  dropActive.value = false
  if (!hasDraggedFiles(event)) return
  event.preventDefault()
  const files = [...(event.dataTransfer?.files || [])]
  if (!files.length || dropBusy.value || !currentPath.value) return
  dropBusy.value = true
  error.value = ''
  let written = 0
  let firstPath = ''
  try {
    for (const file of files.slice(0, 32)) {
      try {
        const data = await readDroppedFile(file)
        const base = currentPath.value.replace(/[\\/]+$/, '')
        const separator = currentPath.value.includes('\\') ? '\\' : '/'
        const target = `${base}${separator}${file.name}`
        const result = await bridge.fsWriteFile(target, data)
        written += 1
        if (!firstPath) firstPath = result.path || target
      } catch (writeError) {
        error.value = writeError instanceof Error ? writeError.message : String(writeError)
      }
    }
    if (written) {
      await load(currentPath.value, { record: false, keepFilter: true })
      if (firstPath) {
        const name = firstPath.split(/[\\/]/).pop() || ''
        const entry = listing.value?.entries.find((item) => item.name === name)
        if (entry) {
          selected.value = entry
          revealedName.value = name
          void nextTick(() => listElement.value?.querySelector(`[data-entry-name="${CSS.escape(name)}"]`)?.scrollIntoView({ block: 'center' }))
        }
      }
      emit('toast', written === 1 ? `Файл скопирован в текущую папку` : `Скопировано файлов: ${written}`, 'success')
    }
  } finally {
    dropBusy.value = false
  }
}

// для перетаскивания из Chromium заранее готовим содержимое файла
const dragBlobs = new Map<string, string>()

function guessMime(name: string): string {
  const lower = name.toLowerCase()
  for (const [mime, extensions] of Object.entries(MIME_EXTENSIONS)) {
    if (extensions.some((extension) => lower.endsWith(extension))) return mime
  }
  if (lower.endsWith('.txt') || lower.endsWith('.md') || lower.endsWith('.log')) return 'text/plain'
  return 'application/octet-stream'
}

async function primeDragBlob(entry: FileBrowserEntry) {
  if (entry.kind !== 'file') return
  const path = entryPath(entry)
  if (dragBlobs.has(path) || (entry.size ?? 0) > 64 * 1024 * 1024) return
  try {
    const file = await readFileBase64(path)
    const binary = atob(file.data)
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
    dragBlobs.set(path, URL.createObjectURL(new Blob([bytes], { type: guessMime(entry.name) })))
  } catch {
    // перетаскивание просто отдаст текстовый путь
  }
}

function onEntryDragStart(entry: FileBrowserEntry, event: DragEvent) {
  if (entry.kind !== 'file' || !event.dataTransfer) return
  const path = entryPath(entry)
  const blobUrl = dragBlobs.get(path)
  if (blobUrl) {
    event.dataTransfer.setData('DownloadURL', `${guessMime(entry.name)}:${entry.name}:${blobUrl}`)
    event.dataTransfer.effectAllowed = 'copy'
  } else {
    event.dataTransfer.setData('text/plain', path)
    event.dataTransfer.effectAllowed = 'copy'
    void primeDragBlob(entry)
  }
}

function formatSize(size: number | null): string {
  if (size === null || size === undefined) return ''
  if (size < 1024) return `${size} Б`
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} КиБ`
  if (size < 1024 * 1024 * 1024) return `${(size / 1024 / 1024).toFixed(1)} МиБ`
  return `${(size / 1024 / 1024 / 1024).toFixed(2)} ГиБ`
}

function formatDate(entry: FileBrowserEntry): string {
  if (!entry.modified_at) return '—'
  return new Date(entry.modified_at).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

// клавиатура и мышь

function isFormField(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)
}

function onKeydown(event: KeyboardEvent) {
  if (addressEditing.value || textEditorPath.value) return
  if (event.key === 'Escape') {
    void requestClose()
    return
  }
  if (isFormField(event.target)) {
    // фильтр не должен перехватывать навигацию, но буквы печатаются в нём
    if (event.key === 'Backspace' && (event.target as HTMLInputElement).value === '') void goUp()
    return
  }
  if (event.altKey && event.key === 'ArrowLeft') {
    event.preventDefault()
    void goBack()
    return
  }
  if (event.altKey && event.key === 'ArrowRight') {
    event.preventDefault()
    void goForward()
    return
  }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'l') {
    event.preventDefault()
    void startAddressEdit()
    return
  }
  if (event.key === 'F5') {
    event.preventDefault()
    void refresh()
    return
  }
  if (props.request.mode === 'browse' && event.key === 'F2' && selected.value) {
    event.preventDefault()
    void renameEntry(selected.value)
    return
  }
  if (props.request.mode === 'browse' && event.key === 'Delete' && selected.value) {
    event.preventDefault()
    void deleteEntry(selected.value)
    return
  }
  if (props.request.mode === 'browse' && event.ctrlKey && event.shiftKey && event.key.toLowerCase() === 'n') {
    event.preventDefault()
    void createDirectory()
    return
  }
  switch (event.key) {
    case 'Backspace':
      event.preventDefault()
      void goUp()
      break
    case 'ArrowDown':
      event.preventDefault()
      moveSelection(1)
      break
    case 'ArrowUp':
      event.preventDefault()
      moveSelection(-1)
      break
    case 'Enter':
      if (selected.value) {
        event.preventDefault()
        void openEntry(selected.value)
      }
      break
  }
}

// боковые кнопки переключают историю проводника, а не встроенного окна
function onMouseNavDown(event: MouseEvent) {
  if (event.button === 3 || event.button === 4) {
    event.preventDefault()
    event.stopPropagation()
  }
}

function onMouseNavUp(event: MouseEvent) {
  if (event.button === 3 || event.button === 4) {
    event.preventDefault()
    event.stopPropagation()
    if (event.button === 3) void goBack()
    else void goForward()
  }
}

// изменение и сохранение размера окна
const FILE_BROWSER_SIZE_KEY = 'nivora-filebrowser-size'
const FILE_BROWSER_MIN_WIDTH = 640
const FILE_BROWSER_MIN_HEIGHT = 380

function readStoredSize(): { width: number; height: number } | null {
  try {
    const raw = localStorage.getItem(FILE_BROWSER_SIZE_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw) as { width?: unknown; height?: unknown }
    if (typeof parsed.width !== 'number' || typeof parsed.height !== 'number') return null
    return {
      width: Math.round(Math.max(FILE_BROWSER_MIN_WIDTH, parsed.width)),
      height: Math.round(Math.max(FILE_BROWSER_MIN_HEIGHT, parsed.height)),
    }
  } catch {
    return null // битая запись не мешает использовать стандартный размер
  }
}

const dialogSize = ref<{ width: number; height: number } | null>(readStoredSize())
const resizing = ref(false)
let resizeStart = { pointerX: 0, pointerY: 0, width: 0, height: 0 }

const dialogStyle = computed(() => (dialogSize.value ? { width: `${dialogSize.value.width}px`, height: `${dialogSize.value.height}px` } : undefined))

function onResizeStart(event: PointerEvent) {
  if (event.button !== 0) return
  const rect = dialogElement.value?.getBoundingClientRect()
  if (!rect) return
  resizeStart = { pointerX: event.clientX, pointerY: event.clientY, width: rect.width, height: rect.height }
  resizing.value = true
  document.body.style.userSelect = 'none'
  window.addEventListener('pointermove', onResizeMove)
  window.addEventListener('pointerup', onResizeEnd)
  event.preventDefault()
}

function onResizeMove(event: PointerEvent) {
  if (!resizing.value) return
  const width = Math.max(FILE_BROWSER_MIN_WIDTH, Math.min(window.innerWidth - 32, resizeStart.width + event.clientX - resizeStart.pointerX))
  const height = Math.max(FILE_BROWSER_MIN_HEIGHT, Math.min(window.innerHeight - 32, resizeStart.height + event.clientY - resizeStart.pointerY))
  dialogSize.value = { width: Math.round(width), height: Math.round(height) }
}

function onResizeEnd() {
  resizing.value = false
  document.body.style.userSelect = ''
  window.removeEventListener('pointermove', onResizeMove)
  window.removeEventListener('pointerup', onResizeEnd)
  if (dialogSize.value) localStorage.setItem(FILE_BROWSER_SIZE_KEY, JSON.stringify(dialogSize.value))
}

function resetDialogSize() {
  dialogSize.value = null
  localStorage.removeItem(FILE_BROWSER_SIZE_KEY)
}

async function requestClose() {
  if (textEditorPath.value && textEditorContent.value !== textEditorOriginal.value && !(await confirmDialog('Закрыть файл и потерять несохранённые изменения?', { danger: true, confirmLabel: 'Закрыть без сохранения' }))) return
  emit('close')
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown)
  window.addEventListener('mousedown', onMouseNavDown, true)
  window.addEventListener('mouseup', onMouseNavUp, true)
  void load(props.request.startPath)
  void nextTick(() => dialogElement.value?.focus())
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  window.removeEventListener('mousedown', onMouseNavDown, true)
  window.removeEventListener('mouseup', onMouseNavUp, true)
  onResizeEnd()
  closeContextMenu()
  for (const url of dragBlobs.values()) URL.revokeObjectURL(url)
  dragBlobs.clear()
})

watch(() => props.request.startPath, () => {
  if (props.request.startPath) void load(props.request.startPath)
})
</script>

<template>
  <Teleport to="body">
    <div class="modal-scrim" role="presentation" @click.self="requestClose">
      <section
        ref="dialogElement"
        class="modal-card file-browser-card"
        :class="{ 'file-browser-card--drop': dropActive, 'file-browser-card--resizing': resizing }"
        role="dialog"
        tabindex="-1"
        aria-modal="true"
        :aria-label="title"
        :style="dialogStyle"
        @dragenter="onDragEnter"
        @dragover="onDragOver"
        @dragleave="onDragLeave"
        @drop="onDrop"
      >
        <div v-if="dropActive" class="file-browser-dropzone" aria-hidden="true"><Icon name="upload" :size="22" /><span>Отпустите — файлы попадут в текущую папку</span></div>
        <div class="panel-heading">
          <div>
            
            <h2>{{ title }}</h2>
          </div>
          <button class="icon-button" type="button" aria-label="Закрыть" @click="requestClose"><Icon name="close" :size="17" /></button>
        </div>

        <div class="file-browser-toolbar">
          <div class="file-browser-nav" role="group" aria-label="Навигация">
            <button class="icon-button" type="button" aria-label="Назад" title="Назад (Alt+←, кнопка мыши «назад»)" :disabled="!canBack || loading" @click="goBack"><Icon name="arrow-left" :size="16" /></button>
            <button class="icon-button" type="button" aria-label="Вперёд" title="Вперёд (Alt+→, кнопка мыши «вперёд»)" :disabled="!canForward || loading" @click="goForward"><Icon name="arrow-right" :size="16" /></button>
            <button class="icon-button" type="button" aria-label="Уровень выше" title="Уровень выше (Backspace)" :disabled="!listing?.parent || loading" @click="goUp"><Icon name="arrow-up" :size="16" /></button>
            <button class="icon-button" type="button" aria-label="Обновить" title="Обновить (F5)" :disabled="!listing || loading" @click="refresh"><Icon name="refresh" :size="16" /></button>
            <button class="icon-button" type="button" :class="{ 'icon-button--active': showHidden }" aria-label="Показать скрытые файлы" :title="showHidden ? 'Скрыть скрытые файлы' : 'Показать скрытые файлы'" :disabled="loading" @click="toggleShowHidden"><Icon name="eye" :size="16" /></button>
          </div>
          <div v-if="!addressEditing" class="file-browser-address" title="Клик — ввести путь вручную (Ctrl+L)" @click="startAddressEdit">
            <nav class="file-browser-crumbs" aria-label="Путь">
              <button v-for="crumb in crumbs()" :key="crumb.path" type="button" @click.stop="navigateTo(crumb.path)">{{ crumb.label }}</button>
            </nav>
            <Icon name="edit" :size="13" class="file-browser-address-edit" />
          </div>
          <input
            v-else
            ref="addressInput"
            v-model="addressDraft"
            class="file-browser-address-input"
            type="text"
            spellcheck="false"
            autocomplete="off"
            placeholder="Путь к папке, например ~/Documents"
            aria-label="Путь"
            @keydown.enter.prevent="applyAddress"
            @keydown.escape.prevent="cancelAddressEdit"
            @blur="cancelAddressEdit"
          />
          <label class="file-browser-filter" title="Фильтр по имени в текущей папке"><Icon name="search" :size="14" /><input v-model="filter" placeholder="Фильтр" spellcheck="false" /></label>
        </div>

        <div class="file-browser-body">
          <aside v-if="listing?.roots.length" class="file-browser-sidebar" aria-label="Места">
            <button
              v-for="root in listing.roots"
              :key="root.path"
              type="button"
              class="file-browser-side-item"
              :class="{ 'file-browser-side-item--active': isActiveRoot(root) }"
              @click="navigateTo(root.path)"
            >
              <Icon :name="root.label === 'Дом' ? 'home' : 'folder'" :size="15" />
              <span>{{ root.label }}</span>
            </button>
          </aside>

          <div class="file-browser-main">
            <div class="file-browser-headrow" aria-hidden="true">
              <button type="button" class="file-browser-headcell file-browser-headcell--name" @click="toggleSort('name')">Имя {{ sortIndicator('name') }}</button>
              <button type="button" class="file-browser-headcell file-browser-headcell--size" @click="toggleSort('size')">Размер {{ sortIndicator('size') }}</button>
              <button type="button" class="file-browser-headcell file-browser-headcell--date" @click="toggleSort('modified')">Изменён {{ sortIndicator('modified') }}</button>
            </div>
            <div ref="listElement" class="file-browser-list" role="listbox" aria-label="Содержимое каталога">
              <div v-if="loading" class="file-browser-status"><span class="file-browser-spinner" />Открываю каталог…</div>
              <div v-else-if="error" class="file-browser-status file-browser-status--error"><Icon name="warning" :size="16" />{{ error }}</div>
              <div v-else-if="!entries.length" class="file-browser-status">{{ filter ? 'Ничего не подходит под фильтр' : 'Пусто' }}</div>
              <button
                v-for="entry in entries"
                :key="entry.name"
                type="button"
                class="file-browser-row"
                :class="{ 'file-browser-row--selected': selected?.name === entry.name, 'file-browser-row--disabled': request.mode === 'open' && entry.kind === 'dir', 'file-browser-row--revealed': revealedName === entry.name }"
                role="option"
                :aria-selected="selected?.name === entry.name"
                :data-entry-name="entry.name"
                :draggable="entry.kind === 'file'"
                @click="request.multiple && entry.kind === 'file' ? toggleMany(entry) : (selected = entry)"
                @dblclick="openEntry(entry, $event)"
                @contextmenu.prevent="entryMenu(entry, $event)"
                @pointerdown="primeDragBlob(entry)"
                @dragstart="onEntryDragStart(entry, $event)"
              >
                <span v-if="request.multiple && entry.kind === 'file'" class="file-browser-check" :class="{ 'file-browser-check--on': selectedMany.has(entry.name) }" aria-hidden="true" />
                <span class="file-browser-cell file-browser-cell--name"><Icon :name="entry.kind === 'dir' ? 'folder' : 'file'" :size="15" /><span class="file-browser-name">{{ entry.name }}</span></span>
                <span class="file-browser-cell file-browser-cell--size">{{ entry.kind === 'dir' ? '—' : formatSize(entry.size) }}</span>
                <span class="file-browser-cell file-browser-cell--date">{{ formatDate(entry) }}</span>
              </button>
            </div>
          </div>
        </div>

        <div class="file-browser-statusbar" aria-live="polite">
          <span>{{ entryCount }} {{ entryCount === 1 ? 'элемент' : entryCount >= 2 && entryCount <= 4 ? 'элемента' : 'элементов' }}</span>
          <template v-if="selected">
            <span class="file-browser-statusbar-sep" aria-hidden="true">·</span>
            <strong>{{ selected.name }}</strong>
            <span v-if="selected.kind === 'file'" class="muted">{{ formatSize(selected.size) }}<template v-if="formatDate(selected) !== '—'"> · {{ formatDate(selected) }}</template></span>
          </template>
          <span v-if="reading" class="file-browser-progress" role="status"><span class="file-browser-spinner" />Читаю: {{ formatSize(reading) }} из {{ formatSize(readingTotal) }}</span>
        </div>

        <div class="modal-actions">
          <div v-if="request.mode === 'browse'" class="file-browser-manage-actions">
            <button class="button button--small button--outline" type="button" :disabled="!listing || loading || mutating" @click="createDirectory"><Icon name="folder" :size="14" />Новая папка</button>
            <button class="button button--small button--outline" type="button" :disabled="!listing || loading || mutating" @click="createFile"><Icon name="plus" :size="14" />Новый файл</button>
            <button class="button button--small button--outline" type="button" :disabled="!selected || loading || mutating" @click="selected && renameEntry(selected)"><Icon name="edit" :size="14" />Переименовать</button>
            <button class="button button--small button--danger" type="button" :disabled="!selected || loading || mutating" @click="selected && deleteEntry(selected)"><Icon name="trash" :size="14" />Удалить</button>
          </div>
          <button class="button button--quiet" type="button" @click="requestClose">Закрыть</button>
          <button v-if="request.mode === 'browse' && selected?.kind === 'file'" class="button button--accent" type="button" :disabled="textEditorBusy" @click="openSelectedFile"><Icon name="edit" :size="15" />{{ editorMode === 'system' ? 'Открыть в ОС' : 'Открыть файл' }}</button>
          <button v-if="request.mode === 'directory'" class="button button--accent" type="button" :disabled="!listing" @click="chooseDirectory"><Icon name="check" :size="15" />Выбрать «{{ selected?.kind === 'dir' ? selected.name : 'эту папку' }}»</button>
          <button v-if="request.mode === 'open' && !request.multiple" class="button button--accent" type="button" :disabled="!selected || selected.kind !== 'file' || Boolean(reading)" @click="selected && chooseEntry(selected)"><Icon name="check" :size="15" />Открыть</button>
          <button v-if="request.mode === 'open' && request.multiple" class="button button--accent" type="button" :disabled="!selectedMany.size || Boolean(reading)" @click="chooseSelection"><Icon name="check" :size="15" />{{ reading ? 'Читаю…' : `Открыть (${selectedMany.size})` }}</button>
        </div>
        <div
          class="file-browser-resize-handle"
          aria-label="Изменить размер окна"
          title="Потяните, чтобы изменить размер; двойной клик — стандартный размер"
          @pointerdown="onResizeStart"
          @dblclick.stop="resetDialogSize"
        />
      </section>
      <div v-if="textEditorPath" class="modal-scrim file-text-editor-scrim" role="presentation" @click.self="closeTextEditor">
        <section class="modal-card file-text-editor" role="dialog" tabindex="-1" aria-modal="true" :aria-label="`Редактор: ${textEditorName}`" @keydown.esc.stop.prevent="closeTextEditor">
          <header class="file-text-editor-heading"><div><strong>{{ textEditorName }}</strong><small :title="textEditorPath">{{ textEditorPath }}</small></div><button class="icon-button" type="button" aria-label="Закрыть редактор" :disabled="textEditorBusy" @click="closeTextEditor"><Icon name="close" :size="16" /></button></header>
          <textarea v-model="textEditorContent" class="file-text-editor-content" spellcheck="false" aria-label="Содержимое файла" :disabled="textEditorBusy" />
          <p v-if="textEditorError" class="file-text-editor-error" role="alert">{{ textEditorError }}</p>
          <footer class="file-text-editor-footer"><span>{{ textEditorContent.length.toLocaleString() }} символов<template v-if="textEditorContent !== textEditorOriginal"> · есть несохранённые изменения</template></span><div><button class="button button--quiet" type="button" :disabled="textEditorBusy" @click="closeTextEditor">Закрыть</button><button class="button button--accent" type="button" :disabled="textEditorBusy || textEditorContent === textEditorOriginal" @click="saveTextEditor"><Icon name="save" :size="14" />{{ textEditorBusy ? 'Сохраняю…' : 'Сохранить' }}</button></div></footer>
        </section>
      </div>
    </div>
  </Teleport>
</template>
