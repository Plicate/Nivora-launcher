<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { pickFile } from '../services/fileBrowser'
import ColorPicker from './ColorPicker.vue'
import CustomSelect from './CustomSelect.vue'
import Icon from './Icon.vue'
import { SkinRenderer } from '../services/skinRenderer'
import { regionAt, skinRegions, type SkinRegion } from '../services/skinRegions'
import { bridge } from '../services/bridge'
import type { SelectOption, SkinInfo } from '../types'

const props = defineProps<{
  accountId: string
  accountName: string
  skins: SkinInfo[]
  currentImage: string
  currentModel: 'classic' | 'slim'
}>()

const emit = defineEmits<{
  close: []
  toast: [message: string, kind?: 'success' | 'danger' | 'default']
  saved: [skin: SkinInfo]
  applied: [skin: SkinInfo]
}>()

type ToolId = 'brush' | 'eraser' | 'fill' | 'picker' | 'line' | 'rect' | 'lighten' | 'darken' | 'noise'

const SIZE = 64
// ключ текстуры предпросмотра: постоянный, меняется только содержимое.
const EDITOR_TEXTURE_KEY = 'skin-editor:current'
const OVERLAY_SCALE = 12
const HISTORY_LIMIT = 60

const tools: Array<{ id: ToolId; label: string; icon: string; hint: string }> = [
  { id: 'brush', label: 'Кисть', icon: 'edit', hint: 'рисует квадратом выбранного размера с заданной непрозрачностью' },
  { id: 'eraser', label: 'Ластик', icon: 'close', hint: 'стирает пиксели в прозрачность' },
  { id: 'fill', label: 'Заливка стороны', icon: 'image', hint: 'заливает всю сторону части тела, например перед головы' },
  { id: 'picker', label: 'Пипетка', icon: 'copy', hint: 'берёт цвет прямо с текстуры' },
  { id: 'line', label: 'Линия', icon: 'arrow-right', hint: 'прямая от начальной точки до курсора' },
  { id: 'rect', label: 'Прямоугольник', icon: 'stop', hint: 'заполненный прямоугольник' },
  { id: 'lighten', label: 'Осветлить', icon: 'sun', hint: 'осветляет пиксели под кистью' },
  { id: 'darken', label: 'Затемнить', icon: 'moon', hint: 'затемняет пиксели под кистью' },
  { id: 'noise', label: 'Шум', icon: 'sparkle', hint: 'кисть с лёгкой пестротой — для ткани и волос' },
]

const tool = ref<ToolId>('brush')
const brushSize = ref(1)
const brushOpacity = ref(100)
const color = ref('#8b653f')
const model = ref<'classic' | 'slim'>(props.currentModel === 'slim' ? 'slim' : 'classic')
const recentColors = ref<string[]>(['#e9e8de', '#8b653f', '#3d3f38', '#1d1e1a', '#9bac78', '#d58a7d'])
const palettePreset = [
  '#f2e3c8', '#e8c39e', '#d69a6e', '#b5714a', '#8a4b32', '#5d3320',
  '#f5f5f0', '#c9c9bb', '#9a9a8c', '#6b6b5f', '#474740', '#23241f',
  '#e3b7a8', '#c58c7d', '#a35c4e', '#7c3f37', '#542a26', '#2f1a18',
  '#9bac78', '#7b8f5c', '#5c7044', '#3f5230', '#2a3a20', '#1a2415',
  '#7fa6c8', '#5c82a6', '#3d5c7d', '#2a405c', '#1b2b40', '#101b2b',
  '#c5a274', '#a5824f', '#7c6238', '#57431f', '#3a2d12', '#241c08',
]

const textureCanvas = ref<HTMLCanvasElement | null>(null)
const gridCanvas = ref<HTMLCanvasElement | null>(null)
const previewCanvas = ref<HTMLCanvasElement | null>(null)
const librarySelect = ref('')
const statusMessage = ref('')
const busy = ref(false)
const hoverRegion = ref<SkinRegion | null>(null)
const canUndo = ref(false)
const canRedo = ref(false)
const hideOverlay = ref(false)
const previewSupported = ref(true)
const hiddenParts = reactive<Record<number, boolean>>({ 0: false, 1: false, 2: false, 3: false, 4: false, 5: false })
const partLabels = ['Голова', 'Торс', 'Рука R', 'Рука L', 'Нога R', 'Нога L']

let context: CanvasRenderingContext2D | null = null
let gridContext: CanvasRenderingContext2D | null = null
let undoStack: ImageData[] = []
let redoStack: ImageData[] = []
let strokeTouched = new Set<number>()
let strokeAnchor: { x: number; y: number } | null = null
let strokeSnapshot: ImageData | null = null
let strokeActive = false
let renderer: SkinRenderer | undefined
let rendererDispose: (() => void) | undefined
let previewFrame = 0
let previewYaw = -0.55
let previewPitch = 0.1
let previewZoom = 1
let previewDragging = false
let previewLastX = 0
let previewLastY = 0
let previewDirty = true

const libraryOptions = computed<SelectOption[]>(() => [
  { value: '', label: 'Скин из библиотеки…', meta: props.skins.length ? `${props.skins.length} шт.` : 'библиотека пуста' },
  ...props.skins.map((skin) => ({ value: skin.skin_id, label: skin.name, meta: skin.model === 'slim' ? 'стройные руки' : 'широкие руки' })),
])

const regions = computed(() => skinRegions(model.value))

/* ── цвет ─────────────────────────────────────────────────────────────── */

function hexToRgb(hex: string): [number, number, number] {
  const normalized = hex.trim().replace(/^#/, '')
  if (!/^[0-9a-f]{6}$/i.test(normalized)) return [139, 101, 63]
  return [
    Number.parseInt(normalized.slice(0, 2), 16),
    Number.parseInt(normalized.slice(2, 4), 16),
    Number.parseInt(normalized.slice(4, 6), 16),
  ]
}

function rgbToHex(red: number, green: number, blue: number): string {
  return `#${[red, green, blue].map((channel) => Math.round(channel).toString(16).padStart(2, '0')).join('')}`
}

function rememberColor(value: string) {
  recentColors.value = [value, ...recentColors.value.filter((item) => item.toLowerCase() !== value.toLowerCase())].slice(0, 12)
}

/* ── история ──────────────────────────────────────────────────────────── */

function pushHistory() {
  if (!context) return
  undoStack.push(context.getImageData(0, 0, SIZE, SIZE))
  if (undoStack.length > HISTORY_LIMIT) undoStack.shift()
  redoStack = []
  canUndo.value = true
  canRedo.value = false
}

function undo() {
  if (!context || !undoStack.length) return
  redoStack.push(context.getImageData(0, 0, SIZE, SIZE))
  context.putImageData(undoStack.pop()!, 0, 0)
  canUndo.value = undoStack.length > 0
  canRedo.value = true
  schedulePreview()
}

function redo() {
  if (!context || !redoStack.length) return
  undoStack.push(context.getImageData(0, 0, SIZE, SIZE))
  context.putImageData(redoStack.pop()!, 0, 0)
  canUndo.value = true
  canRedo.value = redoStack.length > 0
  schedulePreview()
}

/* ── рисование ────────────────────────────────────────────────────────── */

function clampByte(value: number): number {
  return Math.min(255, Math.max(0, Math.round(value)))
}

function blendPixel(x: number, y: number, red: number, green: number, blue: number, alpha: number) {
  if (!context) return
  const [r, g, b, a] = context.getImageData(x, y, 1, 1).data
  if (a === 0) {
    context.clearRect(x, y, 1, 1)
    context.fillStyle = `rgba(${red},${green},${blue},${alpha})`
    context.fillRect(x, y, 1, 1)
    return
  }
  const mergedAlpha = alpha + (a / 255) * (1 - alpha)
  if (mergedAlpha <= 0) return
  const outRed = (red * alpha + r * (a / 255) * (1 - alpha)) / mergedAlpha
  const outGreen = (green * alpha + g * (a / 255) * (1 - alpha)) / mergedAlpha
  const outBlue = (blue * alpha + b * (a / 255) * (1 - alpha)) / mergedAlpha
  context.clearRect(x, y, 1, 1)
  context.fillStyle = `rgba(${outRed},${outGreen},${outBlue},${mergedAlpha})`
  context.fillRect(x, y, 1, 1)
}

function shiftBrightness(x: number, y: number, factor: number) {
  if (!context) return
  const data = context.getImageData(x, y, 1, 1)
  const [r, g, b, a] = data.data
  if (!a) return
  data.data[0] = clampByte(r * factor)
  data.data[1] = clampByte(g * factor)
  data.data[2] = clampByte(b * factor)
  context.putImageData(data, x, y)
}

function applyBrushStamp(cx: number, cy: number) {
  if (!context) return
  const size = Math.max(1, Math.min(8, brushSize.value))
  const offset = Math.floor((size - 1) / 2)
  const [red, green, blue] = hexToRgb(color.value)
  const alpha = brushOpacity.value / 100
  for (let dy = 0; dy < size; dy++) {
    for (let dx = 0; dx < size; dx++) {
      const x = cx - offset + dx
      const y = cy - offset + dy
      if (x < 0 || y < 0 || x >= SIZE || y >= SIZE) continue
      const key = y * SIZE + x
      if (strokeTouched.has(key)) continue
      strokeTouched.add(key)
      if (tool.value === 'eraser') context.clearRect(x, y, 1, 1)
      else if (tool.value === 'lighten') shiftBrightness(x, y, 1 + 0.18 * alpha)
      else if (tool.value === 'darken') shiftBrightness(x, y, 1 - 0.18 * alpha)
      else if (tool.value === 'noise') {
        const shift = Math.round((Math.random() * 2 - 1) * 22)
        blendPixel(x, y, clampByte(red + shift), clampByte(green + shift), clampByte(blue + shift), alpha)
      } else blendPixel(x, y, red, green, blue, alpha)
    }
  }
}

function drawLine(x0: number, y0: number, x1: number, y1: number, stamp: (x: number, y: number) => void) {
  let x = x0
  let y = y0
  const dx = Math.abs(x1 - x0)
  const dy = Math.abs(y1 - y0)
  const sx = x0 < x1 ? 1 : -1
  const sy = y0 < y1 ? 1 : -1
  let error = dx - dy
  for (let guard = 0; guard < SIZE * 4; guard++) {
    stamp(x, y)
    if (x === x1 && y === y1) break
    const doubled = 2 * error
    if (doubled > -dy) { error -= dy; x += sx }
    if (doubled < dx) { error += dx; y += sy }
  }
}

function fillRegion(region: SkinRegion) {
  if (!context) return
  pushHistory()
  const [red, green, blue] = hexToRgb(color.value)
  context.clearRect(region.x, region.y, region.width, region.height)
  context.fillStyle = `rgba(${red},${green},${blue},${brushOpacity.value / 100})`
  context.fillRect(region.x, region.y, region.width, region.height)
  schedulePreview()
  statusMessage.value = `Залито: ${region.partLabel}, ${region.faceLabel}${region.layer === 'overlay' ? ' (второй слой)' : ''}`
}

function strokeFrom(x0: number, y0: number, x1: number, y1: number) {
  if (!context || !strokeSnapshot) return
  context.putImageData(strokeSnapshot, 0, 0)
  strokeTouched.clear()
  if (tool.value === 'rect') {
    const [red, green, blue] = hexToRgb(color.value)
    context.fillStyle = `rgba(${red},${green},${blue},${brushOpacity.value / 100})`
    context.fillRect(Math.min(x0, x1), Math.min(y0, y1), Math.abs(x1 - x0) + 1, Math.abs(y1 - y0) + 1)
  } else {
    drawLine(x0, y0, x1, y1, (x, y) => applyBrushStamp(x, y))
  }
  schedulePreview()
}

function texelFromEvent(event: PointerEvent): { x: number; y: number } | null {
  const canvas = textureCanvas.value
  if (!canvas) return null
  const bounds = canvas.getBoundingClientRect()
  const x = Math.floor(((event.clientX - bounds.left) / bounds.width) * SIZE)
  const y = Math.floor(((event.clientY - bounds.top) / bounds.height) * SIZE)
  if (x < 0 || y < 0 || x >= SIZE || y >= SIZE) return null
  return { x, y }
}

function onTexturePointerDown(event: PointerEvent) {
  if (!context) return
  const point = texelFromEvent(event)
  if (!point) return
  ;(event.target as HTMLElement).setPointerCapture?.(event.pointerId)
  if (tool.value === 'picker') {
    const [r, g, b, a] = context.getImageData(point.x, point.y, 1, 1).data
    if (a) {
      color.value = rgbToHex(r, g, b)
      statusMessage.value = `Цвет взят: ${color.value}`
    } else statusMessage.value = 'Пиксель прозрачный — цвет не взят'
    tool.value = 'brush'
    return
  }
  if (tool.value === 'fill') {
    const region = regionAt(regions.value, point.x, point.y)
    if (region) fillRegion(region)
    else statusMessage.value = 'Под курсором нет стороны части тела'
    return
  }
  strokeActive = true
  strokeTouched.clear()
  strokeAnchor = point
  strokeSnapshot = context.getImageData(0, 0, SIZE, SIZE)
  pushHistory()
  if (tool.value === 'line' || tool.value === 'rect') return
  applyBrushStamp(point.x, point.y)
  schedulePreview()
}

function onTexturePointerMove(event: PointerEvent) {
  const point = texelFromEvent(event)
  hoverRegion.value = point ? regionAt(regions.value, point.x, point.y) : null
  if (!strokeActive || !point) return
  if (tool.value === 'line' || tool.value === 'rect') {
    if (strokeAnchor) strokeFrom(strokeAnchor.x, strokeAnchor.y, point.x, point.y)
    return
  }
  const previous = strokeAnchor ?? point
  drawLine(previous.x, previous.y, point.x, point.y, (x, y) => applyBrushStamp(x, y))
  strokeAnchor = point
  schedulePreview()
}

function onTexturePointerUp() {
  if (!strokeActive) return
  strokeActive = false
  strokeAnchor = null
  strokeSnapshot = null
  strokeTouched.clear()
  schedulePreview()
}

function clearTexture() {
  if (!context) return
  pushHistory()
  context.clearRect(0, 0, SIZE, SIZE)
  schedulePreview()
  statusMessage.value = 'Текстура очищена'
}

/* ── загрузка скинов ──────────────────────────────────────────────────── */

function mirrorRegion(source: SkinRegion, target: SkinRegion) {
  if (!context) return
  const data = context.getImageData(source.x, source.y, source.width, source.height)
  const flipped = context.createImageData(source.width, source.height)
  for (let y = 0; y < source.height; y++) {
    for (let x = 0; x < source.width; x++) {
      const srcIndex = (y * source.width + x) * 4
      const dstIndex = (y * source.width + (source.width - 1 - x)) * 4
      flipped.data[dstIndex] = data.data[srcIndex]
      flipped.data[dstIndex + 1] = data.data[srcIndex + 1]
      flipped.data[dstIndex + 2] = data.data[srcIndex + 2]
      flipped.data[dstIndex + 3] = data.data[srcIndex + 3]
    }
  }
  context.putImageData(flipped, target.x, target.y)
}

/** 64×32 → 64×64: копируем правые конечности в левые слоты (зеркально),
 * как это делает сама Minecraft при загрузке старых скинов. */
function upgradeLegacyTexture() {
  if (!context) return
  const source = skinRegions('classic')
  const find = (id: string) => source.find((region) => region.id === id)
  for (const [fromPart, toPart] of [['rightArm', 'leftArm'], ['rightLeg', 'leftLeg']] as const) {
    for (const face of ['top', 'bottom', 'right', 'front', 'left', 'back'] as const) {
      const from = find(`${fromPart}.base.${face}`)
      const to = find(`${toPart}.base.${face}`)
      if (from && to) mirrorRegion(from, to)
    }
  }
}

function loadSkinFromImage(source: string, label: string) {
  const image = new Image()
  image.onload = () => {
    if (!context) return
    const width = image.naturalWidth || SIZE
    const height = image.naturalHeight || SIZE
    if (![64, 128].includes(width) || ![32, 64, 128].includes(height) || (height > width)) {
      statusMessage.value = `Неожиданный размер скина: ${width}×${height} — нужен 64×64, 64×32 или 128×128`
      return
    }
    pushHistory()
    context.clearRect(0, 0, SIZE, SIZE)
    context.imageSmoothingEnabled = false
    const legacy = height * 2 <= width
    if (legacy) {
      context.drawImage(image, 0, 0, width, height, 0, 0, width, height)
      upgradeLegacyTexture()
    } else {
      context.drawImage(image, 0, 0, width, height, 0, 0, SIZE, SIZE)
    }
    schedulePreview(true)
    statusMessage.value = `Загружено для правки: ${label}${legacy ? ' (старый формат 64×32 преобразован в 64×64)' : ''}`
  }
  image.onerror = () => {
    statusMessage.value = `Не удалось прочитать скин: ${label}`
  }
  image.src = source
}

function onLibrarySelect(skinId: string) {
  const skin = props.skins.find((item) => item.skin_id === skinId)
  if (skin?.image) {
    loadSkinFromImage(skin.image, skin.name)
    if (skin.model === 'slim' || skin.model === 'classic') model.value = skin.model
  }
  librarySelect.value = ''
}

async function onFileChosen() {
  const file = await pickFile({ accept: 'image/png', title: 'PNG-файл скина' })
  if (!file) return
  if (!/\.png$/i.test(file.fileName)) {
    statusMessage.value = 'Скин должен быть PNG-файлом'
    return
  }
  loadSkinFromImage(`data:image/png;base64,${file.data}`, file.fileName)
}

/* ── сохранение ───────────────────────────────────────────────────────── */

function textureDataUrl(): string {
  return textureCanvas.value?.toDataURL('image/png') || ''
}

function base64Png(): string {
  return textureDataUrl().replace(/^data:image\/png;base64,/, '')
}

async function saveToLibrary(apply: boolean) {
  if (busy.value) return
  const data = base64Png()
  if (!data || data === 'data:') {
    statusMessage.value = 'Текстура пуста — нечего сохранять'
    return
  }
  busy.value = true
  const stamp = new Date().toISOString().slice(0, 10)
  try {
    const { skin } = await bridge.skinsImport(`skin-${stamp}.png`, data, model.value)
    emit('saved', skin)
    if (apply) {
      await bridge.applySkin(props.accountId, skin.skin_id, model.value)
      emit('applied', skin)
      emit('toast', `Скин «${skin.name}» установлен на аккаунт ${props.accountName}`)
      statusMessage.value = 'Скин сохранён и установлен на аккаунт'
    } else {
      emit('toast', `Скин «${skin.name}» сохранён в библиотеку`)
      statusMessage.value = 'Скин сохранён в библиотеку'
    }
  } catch (error) {
    statusMessage.value = error instanceof Error ? error.message : String(error)
    emit('toast', statusMessage.value, 'danger')
  } finally {
    busy.value = false
  }
}

async function exportFile() {
  if (busy.value) return
  const data = base64Png()
  if (!data || data === 'data:') {
    statusMessage.value = 'Текстура пуста — нечего сохранять'
    return
  }
  busy.value = true
  try {
    const result = await bridge.skinsExport(`nivora-skin-${new Date().toISOString().slice(0, 10)}.png`, data)
    statusMessage.value = `Файл сохранён: ${result.path}`
    emit('toast', `Скин сохранён как файл: ${result.path}`)
  } catch (error) {
    statusMessage.value = error instanceof Error ? error.message : String(error)
    emit('toast', statusMessage.value, 'danger')
  } finally {
    busy.value = false
  }
}

/* ── 3D-предпросмотр ──────────────────────────────────────────────────── */

function schedulePreview(immediate = false) {
  previewDirty = true
  if (immediate) renderPreview()
}

function renderPreview() {
  if (!renderer) return
  const canvas = textureCanvas.value
  if (!canvas) return
  // текстура редактора живёт под постоянным ключом и обновляется в GPU
  renderer.updateCanvasTexture(EDITOR_TEXTURE_KEY, canvas)
  const hidden = Object.entries(hiddenParts).filter(([, value]) => value).map(([index]) => Number(index))
  renderer.render(
    { skin: EDITOR_TEXTURE_KEY, model: model.value, yaw: previewYaw, pitch: previewPitch, zoom: previewZoom, hidden, hideOverlay: hideOverlay.value },
    renderer.canvas.width,
    renderer.canvas.height,
  )
}

function previewTick() {
  if (previewDirty && !previewDragging) {
    previewDirty = false
    renderPreview()
  }
  previewFrame = window.requestAnimationFrame(previewTick)
}

function onPreviewPointerDown(event: PointerEvent) {
  previewDragging = true
  previewLastX = event.clientX
  previewLastY = event.clientY
  ;(event.target as HTMLElement).setPointerCapture?.(event.pointerId)
}

function onPreviewPointerMove(event: PointerEvent) {
  if (!previewDragging) return
  previewYaw += (event.clientX - previewLastX) * 0.012
  previewPitch = Math.min(1.2, Math.max(-1.2, previewPitch + (event.clientY - previewLastY) * 0.01))
  previewLastX = event.clientX
  previewLastY = event.clientY
  renderPreview()
}

function onPreviewPointerUp() {
  previewDragging = false
}

function onPreviewWheel(event: WheelEvent) {
  event.preventDefault()
  previewZoom = Math.min(4, Math.max(0.6, previewZoom * (event.deltaY > 0 ? 0.92 : 1.08)))
  renderPreview()
}

function togglePart(index: number) {
  hiddenParts[index] = !hiddenParts[index]
  renderPreview()
}

/* ── сетка-оверлей ────────────────────────────────────────────────────── */

function drawGridOverlay() {
  if (!gridContext) return
  const total = SIZE * OVERLAY_SCALE
  gridContext.clearRect(0, 0, total, total)
  gridContext.strokeStyle = 'rgba(255,255,255,0.055)'
  gridContext.lineWidth = 1
  for (let i = 0; i <= SIZE; i += 4) {
    const position = i * OVERLAY_SCALE + 0.5
    gridContext.beginPath()
    gridContext.moveTo(position, 0)
    gridContext.lineTo(position, total)
    gridContext.stroke()
    gridContext.beginPath()
    gridContext.moveTo(0, position)
    gridContext.lineTo(total, position)
    gridContext.stroke()
  }
  if (hoverRegion.value) {
    gridContext.strokeStyle = 'rgba(197,162,116,0.9)'
    gridContext.lineWidth = 2
    gridContext.strokeRect(
      hoverRegion.value.x * OVERLAY_SCALE + 1,
      hoverRegion.value.y * OVERLAY_SCALE + 1,
      hoverRegion.value.width * OVERLAY_SCALE - 2,
      hoverRegion.value.height * OVERLAY_SCALE - 2,
    )
  }
}

watch(hoverRegion, () => drawGridOverlay())
watch(color, (value) => rememberColor(value))
watch(hideOverlay, () => renderPreview())
watch(model, () => schedulePreview(true))

function onKeydown(event: KeyboardEvent) {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z') {
    event.preventDefault()
    if (event.shiftKey) redo()
    else undo()
  }
}

onMounted(() => {
  const canvas = textureCanvas.value
  const grid = gridCanvas.value
  if (!canvas) return
  canvas.width = SIZE
  canvas.height = SIZE
  context = canvas.getContext('2d', { willReadFrequently: true })
  if (context) {
    context.imageSmoothingEnabled = false
    context.clearRect(0, 0, SIZE, SIZE)
  }
  if (grid) {
    grid.width = SIZE * OVERLAY_SCALE
    grid.height = SIZE * OVERLAY_SCALE
    gridContext = grid.getContext('2d')
    drawGridOverlay()
  }
  const preview = previewCanvas.value
  if (preview) {
    preview.width = 440
    preview.height = 560
    try {
      renderer = new SkinRenderer(preview)
      rendererDispose = renderer.onChange(() => renderPreview())
    } catch {
      renderer = undefined
      previewSupported.value = false
    }
  }
  window.addEventListener('keydown', onKeydown)
  previewFrame = window.requestAnimationFrame(previewTick)
  const starter = props.currentImage || props.skins[0]?.image || ''
  if (starter) loadSkinFromImage(starter, props.currentImage ? 'текущий скин аккаунта' : props.skins[0]?.name || 'скин')
  else statusMessage.value = 'Чистая текстура 64×64 — нарисуйте скин с нуля или загрузите готовый'
})

onBeforeUnmount(() => {
  window.cancelAnimationFrame(previewFrame)
  window.removeEventListener('keydown', onKeydown)
  rendererDispose?.()
  renderer?.dispose()
  renderer = undefined
})
</script>

<template>
  <Teleport to="body">
    <div class="dialog-scrim skin-editor-scrim" role="presentation" @click.self="emit('close')">
      <section class="dialog-card skin-editor-card" role="dialog" tabindex="-1" aria-modal="true" aria-label="Редактор скинов">
        <div class="panel-heading">
          <div>
            <span class="eyebrow">Skin studio</span>
            <h2>Сделать скин</h2>
            <p>{{ accountName }} · {{ model === 'slim' ? 'стройные руки' : 'широкие руки' }}</p>
          </div>
          <div class="skin-editor-head-actions">
            <button class="icon-button" type="button" aria-label="Отменить" :disabled="!canUndo" v-tooltip="'Отменить (Ctrl+Z)'" @click="undo"><Icon name="undo" :size="16" /></button>
            <button class="icon-button" type="button" aria-label="Вернуть" :disabled="!canRedo" v-tooltip="'Вернуть (Ctrl+Shift+Z)'" @click="redo"><Icon name="redo" :size="16" /></button>
            <button class="icon-button" type="button" aria-label="Закрыть редактор" @click="emit('close')"><Icon name="close" :size="17" /></button>
          </div>
        </div>

        <div class="skin-editor-grid">
          <div class="skin-editor-left">
            <div class="skin-editor-tools" role="toolbar" aria-label="Инструменты рисования">
              <button
                v-for="item in tools"
                :key="item.id"
                class="skin-tool"
                :class="{ 'skin-tool--active': tool === item.id }"
                type="button"
                :aria-pressed="tool === item.id"
                v-tooltip="`${item.label} — ${item.hint}`"
                @click="tool = item.id"
              >
                <Icon :name="item.icon" :size="15" />
                <span>{{ item.label }}</span>
              </button>
            </div>

            <div class="skin-editor-options">
              <div class="field"><span>Размер кисти</span>
                <div class="segmented-control" role="group" aria-label="Размер кисти">
                  <button v-for="size in [1, 2, 3, 4, 6, 8]" :key="size" type="button" :class="{ active: brushSize === size }" @click="brushSize = size">{{ size }}<small>px</small></button>
                </div>
              </div>
              <div class="field"><span>Непрозрачность</span>
                <div class="skin-editor-opacity">
                  <input v-model.number="brushOpacity" type="range" min="5" max="100" step="5" aria-label="Непрозрачность кисти" />
                  <output>{{ brushOpacity }}%</output>
                </div>
              </div>
            </div>

            <div class="skin-canvas-wrap">
              <canvas
                ref="textureCanvas"
                class="skin-texture-canvas"
                @pointerdown.prevent="onTexturePointerDown"
                @pointermove.prevent="onTexturePointerMove"
                @pointerup="onTexturePointerUp"
                @pointercancel="onTexturePointerUp"
              />
              <canvas ref="gridCanvas" class="skin-grid-canvas" aria-hidden="true" />
            </div>

            <div class="skin-editor-hint-row">
              <span v-if="hoverRegion" class="skin-region-hint"><Icon name="info" :size="13" />{{ hoverRegion.partLabel }} · {{ hoverRegion.faceLabel }}{{ hoverRegion.layer === 'overlay' ? ' · второй слой' : '' }}</span>
              <span v-else class="muted">Наведите курсор на текстуру — увидите часть тела</span>
              <span class="muted">64×64</span>
            </div>

            <div class="skin-color-row">
              <div class="skin-color-current"><span :style="{ background: color }" /><code>{{ color }}</code></div>
              <ColorPicker v-model="color" />
              <div class="skin-palette">
                <button v-for="swatch in recentColors" :key="`recent-${swatch}`" class="skin-swatch" :style="{ background: swatch }" type="button" :aria-label="`Недавний цвет ${swatch}`" @click="color = swatch" />
                <span class="skin-palette-divider" aria-hidden="true" />
                <button v-for="swatch in palettePreset" :key="swatch" class="skin-swatch" :style="{ background: swatch }" type="button" :aria-label="`Цвет ${swatch}`" @click="color = swatch" />
              </div>
            </div>
          </div>

          <div class="skin-editor-right">
            <div class="skin-preview-wrap">
              <canvas
                v-if="previewSupported"
                ref="previewCanvas"
                class="skin-preview-canvas"
                @pointerdown.prevent="onPreviewPointerDown"
                @pointermove.prevent="onPreviewPointerMove"
                @pointerup="onPreviewPointerUp"
                @pointercancel="onPreviewPointerUp"
                @wheel.prevent="onPreviewWheel"
              />
              <p v-else class="inline-error">Предпросмотр недоступен (нет WebGL), но редактировать текстуру можно.</p>
              <span v-if="previewSupported" class="muted skin-preview-note">Вращайте мышью · колесо — масштаб</span>
            </div>

            <div class="skin-editor-part-toggles">
              <button
                v-for="(label, index) in partLabels"
                :key="label"
                class="skin-part-chip"
                :class="{ 'skin-part-chip--hidden': hiddenParts[index] }"
                type="button"
                :aria-pressed="hiddenParts[index]"
                v-tooltip="hiddenParts[index] ? 'Показать часть тела' : 'Скрыть часть тела — чтобы красить закрытые места'"
                @click="togglePart(index)"
              >{{ label }}</button>
            </div>
            <label class="workspace-check skin-overlay-toggle">
              <input v-model="hideOverlay" type="checkbox" />
              <span><strong>Скрыть второй слой</strong><small>Чтобы красить базовый слой под ним</small></span>
            </label>

            <div class="field"><span>Модель рук</span>
              <div class="segmented-control">
                <button type="button" :class="{ active: model === 'classic' }" @click="model = 'classic'">Широкие</button>
                <button type="button" :class="{ active: model === 'slim' }" @click="model = 'slim'">Стройные</button>
              </div>
            </div>

            <div class="field"><span>Загрузить для правки</span>
              <div class="skin-editor-load-row">
                <CustomSelect :model-value="librarySelect" :options="libraryOptions" aria-label="Скин из библиотеки" @update:model-value="onLibrarySelect" />
                <button class="button button--small button--outline" type="button" @click="onFileChosen"><Icon name="upload" :size="14" />Из файла</button>
                <button class="button button--small button--quiet" type="button" v-tooltip="'Очистить всю текстуру'" @click="clearTexture"><Icon name="trash" :size="14" /></button>
              </div>
            </div>

            <div class="skin-editor-actions">
              <button class="button button--quiet" type="button" :disabled="busy" @click="exportFile"><Icon name="download" :size="15" />Как файл</button>
              <button class="button button--outline" type="button" :disabled="busy" @click="saveToLibrary(false)"><Icon name="save" :size="15" />В библиотеку</button>
              <button class="button button--accent" type="button" :disabled="busy" @click="saveToLibrary(true)"><Icon name="check" :size="15" />Установить на Minecraft</button>
            </div>

            <p v-if="statusMessage" class="skin-editor-status" role="status">{{ statusMessage }}</p>
          </div>
        </div>

      </section>
    </div>
  </Teleport>
</template>
