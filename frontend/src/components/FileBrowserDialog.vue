<script setup lang="ts">
// встроенный проводник лаунчера. Заменяет и окно ОС «Открыть папку», и
// системный выбор файлов: история назад/вперёд (включая кнопки мыши),
// адресная строка с вводом пути, сортировка, боковая панель корней.
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
import { closeContextMenu, openContextMenu } from '../services/contextMenu'

const props = defineProps<{ request: FileBrowserRequest }>()
const emit = defineEmits<{
  (event: 'close'): void
  (event: 'chosen', file: PickedFile): void
  (event: 'chosen-many', files: PickedFile[]): void
  (event: 'chosen-directory', directory: { name: string; path: string }): void
}>()

const listing = ref<FileBrowserListing | null>(null)
const currentPath = ref('')
const loading = ref(false)
const error = ref('')
const filter = ref('')
const selected = ref<FileBrowserEntry | null>(null)
// мультивыбор: имена выбранных файлов (папки в выбор не попадают).
const selectedMany = ref<Set<string>>(new Set())
const reading = ref(0)
const readingTotal = ref(0)
const dialogElement = ref<HTMLElement | null>(null)
const listElement = ref<HTMLElement | null>(null)
const addressInput = ref<HTMLInputElement | null>(null)

// история навигации: реактивный счётчик отмечает изменения PathHistory
// (класс не реактивен), кнопки пересчитываются по нему.
const history = new PathHistory()
const historyTick = ref(0)
const canBack = computed(() => (historyTick.value, history.canBack))
const canForward = computed(() => (historyTick.value, history.canForward))

type SortKey = 'name' | 'size' | 'modified'
const sortKey = ref<SortKey>('name')
const sortAsc = ref(true)

const addressEditing = ref(false)
const addressDraft = ref('')

// «Показать скрытые» переживает переоткрытия окна (сессия).
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
  // папки всегда выше файлов — как в обычных проводниках.
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
  } catch (loadError) {
    error.value = loadError instanceof Error ? loadError.message : String(loadError)
  } finally {
    loading.value = false
  }
}

/** переход по клику/вводу — с записью в историю. */
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

// ── адресная строка ──────────────────────────────────────────────────

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

/** «~» — домашний каталог, как в обычных оболочках. */
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

// ── крошки ────────────────────────────────────────────────────────────

function crumbs(): Array<{ label: string; path: string }> {
  const raw = currentPath.value
  if (!raw) return []
  const separator = raw.includes('\\') ? '\\' : '/'
  const drive = /^[A-Za-z]:/.exec(raw)?.[0] || ''
  const result: Array<{ label: string; path: string }> = []
  let acc = raw.startsWith('\\\\') ? '\\\\' : ''
  for (const part of raw.split(/[\\/]+/).filter(Boolean)) {
    // Windows: «C:» — это диск, а не папка; путь к нему — «C:\».
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

// ── навигация по списку ──────────────────────────────────────────────

function entryPath(entry: FileBrowserEntry): string {
  const base = currentPath.value.replace(/[\\/]+$/, '')
  const separator = currentPath.value.includes('\\') ? '\\' : '/'
  return `${base}${separator}${entry.name}`
}

async function openEntry(entry: FileBrowserEntry) {
  if (entry.kind === 'dir') {
    await navigateTo(entryPath(entry))
    return
  }
  if (props.request.mode !== 'open') {
    entryMenu(entry, new MouseEvent('contextmenu'))
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

function entryMenu(entry: FileBrowserEntry, event: MouseEvent) {
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
  if (props.request.mode === 'open' && entry.kind === 'file') {
    items.push({ id: 'choose', label: 'Выбрать файл', icon: 'check', action: () => void chooseEntry(entry) })
  }
  if (props.request.mode === 'directory' && entry.kind === 'dir') {
    items.push({ id: 'choose-dir', label: 'Выбрать папку', icon: 'check', action: chooseDirectory })
  }
  openContextMenu(event, items, entry.name)
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

// ── клавиатура и мышь ─────────────────────────────────────────────────

function isFormField(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)
}

function onKeydown(event: KeyboardEvent) {
  if (addressEditing.value) return // ввод пути сам обработает свои клавиши
  if (event.key === 'Escape') {
    emit('close')
    return
  }
  if (isFormField(event.target)) {
    // фильтр не должен перехватывать навигацию, но буквы печатаются в нём.
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

// боковые кнопки мыши (XButton1/XButton2 = «назад»/«вперёд»): гасим
// навигацию webview и ведём свою историю.
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
  closeContextMenu()
})

watch(() => props.request.startPath, () => {
  if (props.request.startPath) void load(props.request.startPath)
})
</script>

<template>
  <Teleport to="body">
    <div class="modal-scrim" role="presentation" @click.self="emit('close')">
      <section ref="dialogElement" class="modal-card file-browser-card" role="dialog" tabindex="-1" aria-modal="true" :aria-label="title">
        <div class="panel-heading">
          <div>
            <span class="eyebrow">Файлы</span>
            <h2>{{ title }}</h2>
          </div>
          <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')"><Icon name="close" :size="17" /></button>
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
                :class="{ 'file-browser-row--selected': selected?.name === entry.name, 'file-browser-row--disabled': request.mode === 'open' && entry.kind === 'dir' }"
                role="option"
                :aria-selected="selected?.name === entry.name"
                @click="request.multiple && entry.kind === 'file' ? toggleMany(entry) : (selected = entry)"
                @dblclick="openEntry(entry)"
                @contextmenu.prevent="entryMenu(entry, $event)"
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
          <button class="button button--quiet" type="button" @click="emit('close')">Закрыть</button>
          <button v-if="request.mode === 'directory'" class="button button--accent" type="button" :disabled="!listing" @click="chooseDirectory"><Icon name="check" :size="15" />Выбрать «{{ selected?.kind === 'dir' ? selected.name : 'эту папку' }}»</button>
          <button v-if="request.mode === 'open' && !request.multiple" class="button button--accent" type="button" :disabled="!selected || selected.kind !== 'file' || Boolean(reading)" @click="selected && chooseEntry(selected)"><Icon name="check" :size="15" />Открыть</button>
          <button v-if="request.mode === 'open' && request.multiple" class="button button--accent" type="button" :disabled="!selectedMany.size || Boolean(reading)" @click="chooseSelection"><Icon name="check" :size="15" />{{ reading ? 'Читаю…' : `Открыть (${selectedMany.size})` }}</button>
        </div>
      </section>
    </div>
  </Teleport>
</template>
