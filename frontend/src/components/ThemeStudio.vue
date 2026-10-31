<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, type WritableComputedRef } from 'vue'
import ColorPicker from './ColorPicker.vue'
import CustomSelect from './CustomSelect.vue'
import Icon from './Icon.vue'
import NumericStepper from './NumericStepper.vue'
import ThemeStudioPreview from './ThemeStudioPreview.vue'
import { normalizeButtonCustomization, safeDataUrl, type ButtonLabelKey } from '../services/buttonCustomization'
import type { AppearanceSettings, SelectOption, ViewName } from '../types'

const props = defineProps<{
  appearance: AppearanceSettings
  tokens: Record<string, string>
  theme: string
  baseTheme?: string
  fontFamily: string
  language: string
  languageOptions: SelectOption[]
  pages: Array<{ id: ViewName; title: string; kicker: string }>
  wallpaperUrl: string
  defaultColorTokens: Record<string, string>
  defaultRadiusTokens: Record<string, number>
  lightTokens?: Record<string, string>
  saveState: string
  saveMessage: string
  busy: boolean
}>()

const emit = defineEmits<{
  'update:fontFamily': [value: string]
  'update:language': [value: string]
  save: []
  resetAppearance: []
  importWallpaperFile: [] | [void]
  clearWallpaper: []
  commitWallpaperBlur: []
  openPage: [view: ViewName]
}>()

const buttonTextureInput = ref<HTMLInputElement>()
const buttonIconInput = ref<HTMLInputElement>()
const buttonSoundInput = ref<HTMLInputElement>()
const buttonAssetError = ref('')
const labelOptions: Array<{ key: ButtonLabelKey; label: string }> = [
  { key: 'instances', label: 'Экземпляры' },
  { key: 'accounts', label: 'Аккаунты' },
  { key: 'mods', label: 'Каталог' },
  { key: 'modpacks', label: 'Модпаки' },
  { key: 'themes', label: 'Темы' },
  { key: 'settings', label: 'Настройки' },
]
const textureFitOptions: SelectOption[] = [
  { value: 'cover', label: 'Заполнить кнопку', meta: 'Увеличить маленькую, автоматически кадрировать большую' },
  { value: 'contain', label: 'Показать целиком', meta: 'Сохранить всю картинку внутри кнопки' },
]

function chooseButtonAsset(kind: 'texture' | 'icon' | 'sound') {
  const input = kind === 'texture' ? buttonTextureInput.value : kind === 'icon' ? buttonIconInput.value : buttonSoundInput.value
  input?.click()
}

async function loadButtonAsset(event: Event, kind: 'texture' | 'icon' | 'sound') {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  const imageTypes = new Set(['image/png', 'image/jpeg', 'image/webp'])
  const audioTypes = new Set(['audio/mpeg', 'audio/ogg', 'audio/wav', 'audio/webm'])
  const accepted = kind === 'sound' ? audioTypes : imageTypes
  const limit = kind === 'sound' ? 1_048_576 : 524_288
  if (!accepted.has(file.type) || file.size > limit) {
    buttonAssetError.value = kind === 'sound' ? 'Выберите WAV, OGG, MP3 или WebM до 1 MiB.' : 'Выберите PNG, JPEG или WebP до 512 KiB.'
    return
  }
  const data = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => typeof reader.result === 'string' ? resolve(reader.result) : reject(new Error('Не удалось прочитать файл'))
    reader.onerror = () => reject(new Error('Не удалось прочитать файл'))
    reader.readAsDataURL(file)
  }).catch(() => '')
  const safe = safeDataUrl(data, kind === 'sound' ? 'audio' : 'image')
  if (!safe) {
    buttonAssetError.value = 'Файл не прошёл проверку формата.'
    return
  }
  const extension = file.type === 'image/jpeg' ? 'jpg' : file.type.split('/')[1].replace('mpeg', 'mp3')
  const settings = normalizeButtonCustomization(props.appearance.button_customization)
  if (kind === 'texture') {
    settings.texture = `resources/button-texture.${extension}`
    settings.texture_data = safe
  } else if (kind === 'icon') {
    settings.icon = `resources/button-icon.${extension}`
    settings.icon_data = safe
  } else {
    settings.sound = `resources/button-click.${extension}`
    settings.sound_data = safe
  }
  props.appearance.button_customization = settings
  buttonAssetError.value = ''
}

function clearButtonAsset(kind: 'texture' | 'icon' | 'sound') {
  const settings = normalizeButtonCustomization(props.appearance.button_customization)
  if (kind === 'texture') { settings.texture = ''; settings.texture_data = '' }
  else if (kind === 'icon') { settings.icon = ''; settings.icon_data = '' }
  else { settings.sound = ''; settings.sound_data = '' }
  props.appearance.button_customization = settings
  buttonAssetError.value = ''
}

const themeOptions: SelectOption[] = [
  { value: 'system', label: 'Как в системе', meta: 'Автоматически' },
  { value: 'dark', label: 'Тёмная', meta: 'Насыщенный интерфейс' },
  { value: 'light', label: 'Светлая', meta: 'Чистый интерфейс' },
  { value: 'glass', label: 'Glass', meta: 'Полупрозрачные панели' },
]
const fontFamilyOptions: SelectOption[] = [
  { value: 'system', label: 'Системный', meta: 'Шрифт текущей ОС' },
  { value: 'inter', label: 'Inter', meta: 'Нейтральная UI-гарнитура' },
  { value: 'manrope', label: 'Manrope', meta: 'Более мягкие формы' },
  { value: 'mono', label: 'DM Mono', meta: 'Технический моноширинный стиль' },
]
const accentPresets = ['#5b8dd9', '#3d6ba8', '#8b653f', '#65764b', '#65747a', '#8c5145', '#756157', '#596f5b']
const colorTokenOptions: SelectOption[] = [
  { value: 'accent', label: 'Основной акцент', meta: 'Кнопки, selection, focus' },
  { value: 'accent_strong', label: 'Яркий акцент', meta: 'Hover и подсветка' },
  { value: 'background', label: 'Фон приложения', meta: 'Основной фон' },
  { value: 'panel', label: 'Панели', meta: 'Карточки и секции' },
  { value: 'panel_raised', label: 'Поднятые панели', meta: 'Меню и dropdown' },
  { value: 'panel_soft', label: 'Мягкие поверхности', meta: 'Hover и secondary' },
  { value: 'text', label: 'Основной текст', meta: 'Заголовки и значения' },
  { value: 'muted', label: 'Приглушённый текст', meta: 'Подписи и описания' },
  { value: 'quiet', label: 'Вторичный текст', meta: 'Кнопки и метаданные' },
  { value: 'border', label: 'Границы', meta: 'Контуры элементов' },
  { value: 'danger', label: 'Ошибка / удаление', meta: 'Опасные действия' },
  { value: 'success', label: 'Успех', meta: 'Статусы и подтверждения' },
  { value: 'warning', label: 'Предупреждение', meta: 'Активность и внимание' },
  { value: 'banner_shade', label: 'Затемнение баннера', meta: 'Нижний градиент карточки' },
]
const tokenLabels: Record<string, string> = Object.fromEntries(colorTokenOptions.map((option) => [option.value, option.label]))
const pageThemeOptions: SelectOption[] = [
  { value: 'inherit', label: 'Как глобальная', meta: 'Без переопределения' },
  { value: 'dark', label: 'Тёмная' },
  { value: 'light', label: 'Светлая' },
  { value: 'glass', label: 'Glass' },
]
const pageLayoutOptions: SelectOption[] = [
  { value: 'balanced', label: 'Сбалансированная', meta: 'Рекомендуется' },
  { value: 'compact', label: 'Компактная', meta: 'Больше элементов' },
  { value: 'wide', label: 'Свободная', meta: 'Больше воздуха' },
]
const densityOptions: SelectOption[] = [
  { value: 'compact', label: 'Компактная', meta: 'Плотная сетка' },
  { value: 'comfortable', label: 'Комфортная', meta: 'Стандарт' },
  { value: 'spacious', label: 'Просторная', meta: 'Больше отступов' },
]
const instanceLayoutOptions: SelectOption[] = [
  { value: 'default', label: 'Обычный список', meta: 'Строки с подборкой и действиями' },
  { value: 'compact', label: 'Компактный режим', meta: 'Плотные плитки экземпляров' },
]
const sidebarPositionOptions: SelectOption[] = [
  { value: 'left', label: 'Слева' },
  { value: 'right', label: 'Справа' },
  { value: 'top', label: 'Сверху' },
  { value: 'bottom', label: 'Снизу' },
]
const sidebarModeOptions: SelectOption[] = [
  { value: 'expanded', label: 'Развёрнутый', meta: 'Иконки и подписи' },
  { value: 'adaptive', label: 'Адаптивный', meta: 'Текст только если есть место' },
  { value: 'compact', label: 'Компактный', meta: 'Только иконки' },
]
const sidebarWidthModeOptions: SelectOption[] = [
  { value: 'full', label: 'Во всю ширину', meta: 'Максимальный rail' },
  { value: 'content', label: 'По содержимому', meta: 'Подстраивается под навигацию' },
  { value: 'manual', label: 'Вручную', meta: 'Ползунок или растягивание' },
]
const sidebarVisibilityOptions: SelectOption[] = [
  { value: 'always', label: 'Показывать всегда', meta: 'Закреплён на экране' },
  { value: 'hover', label: 'Только при наведении', meta: 'Плавно скрывается к краю' },
]
const animationOptions: SelectOption[] = [
  { value: 'full', label: 'Полные', meta: 'Все переходы и эффекты' },
  { value: 'reduced', label: 'Умеренные', meta: 'Без тяжёлых эффектов' },
  { value: 'off', label: 'Выключены', meta: 'Максимальная стабильность' },
]
const animationStyleOptions: SelectOption[] = [
  { value: 'fade', label: 'Fade', meta: 'Мягкое появление' },
  { value: 'slide', label: 'Slide', meta: 'Смещение по вертикали' },
  { value: 'scale', label: 'Scale', meta: 'Лёгкое масштабирование' },
  { value: 'none', label: 'Без эффекта', meta: 'Только мгновенная смена' },
]
const perPageAnimationStyleOptions: SelectOption[] = [
  { value: '', label: 'Как глобальная', meta: 'Наследует общий стиль' },
  ...animationStyleOptions,
]
const radiusPresets: Array<{ label: string; hint: string; value: Record<string, number> }> = [
  { label: 'Острые', hint: 'Строгие прямые углы', value: { card: 0, button: 0, control: 0 } },
  { label: 'Стандарт', hint: 'Значения Nivora по умолчанию', value: { card: 7, button: 8, control: 10 } },
  { label: 'Скруглённые', hint: 'Мягкий современный вид', value: { card: 12, button: 12, control: 12 } },
  { label: 'Мягкие', hint: 'Максимально плавные формы', value: { card: 18, button: 20, control: 22 } },
]

type RailSection = 'general' | 'buttons' | 'colors' | 'shape' | 'wallpaper' | 'navigation' | 'motion' | 'pages'
const railSections: Array<{ id: RailSection; label: string; icon: string; hint: string }> = [
  { id: 'general', label: 'Тема', icon: 'palette', hint: 'Режим, акцент, шрифт, масштаб' },
  { id: 'buttons', label: 'Кнопки', icon: 'settings', hint: 'Размер, текст, иконка, текстура и звук' },
  { id: 'colors', label: 'Цвета', icon: 'mods', hint: 'Вся палитра лаунчера' },
  { id: 'shape', label: 'Формы', icon: 'instances', hint: 'Закруглённости и плотность' },
  { id: 'wallpaper', label: 'Обои', icon: 'image', hint: 'Картинка, размытие, видимость' },
  { id: 'navigation', label: 'Навигация', icon: 'monitor', hint: 'Сайдбар: положение и режим' },
  { id: 'motion', label: 'Движение', icon: 'activity', hint: 'Анимации страниц и поверхностей' },
  { id: 'pages', label: 'Страницы', icon: 'file', hint: 'Темы и раскладки страниц' },
]
const activeSection = ref<RailSection>('general')
const selectedTarget = ref('')
const hoveredTarget = ref('')

function selectRailSection(id: RailSection) {
  activeSection.value = id
  selectedTarget.value = ''
}

function selectTarget(target: string) {
  selectedTarget.value = target
}

function clearSelection() {
  selectedTarget.value = ''
}

function handleEscape() {
  if (selectedTarget.value) selectedTarget.value = ''
}

onMounted(() => { window.addEventListener('keydown', handleStudioKeydown) })
onBeforeUnmount(() => { window.removeEventListener('keydown', handleStudioKeydown) })

type TargetEditor = {
  label: string
  description: string
  tokens: string[]
  radius?: { key: 'card' | 'button' | 'control'; label: string }
  extras?: 'wallpaper' | 'sidebar' | 'text'
  note?: string
}

const targetRegistry: Record<string, TargetEditor> = {
  background: {
    label: 'Фон лаунчера',
    description: 'Подложка всего окна. Обои рисуются поверх этого цвета.',
    tokens: ['background'],
    extras: 'wallpaper',
  },
  sidebar: {
    label: 'Сайдбар',
    description: 'Навигационная панель с брендом и меню.',
    tokens: ['panel'],
    radius: { key: 'card', label: 'Закруглённость панели' },
    extras: 'sidebar',
  },
  'nav-item': {
    label: 'Пункт меню',
    description: 'Кнопки навигации в сайдбаре.',
    tokens: ['quiet', 'muted', 'accent'],
    radius: { key: 'button', label: 'Закруглённость пункта' },
    note: 'Активный пункт подсвечивается акцентом, соседние — вторичным текстом.',
  },
  'account-chip': {
    label: 'Чип аккаунта',
    description: 'Карточка текущего пользователя внизу сайдбара.',
    tokens: ['panel_soft', 'text', 'muted'],
    radius: { key: 'control', label: 'Закруглённость чипа' },
  },
  'search-field': {
    label: 'Поле поиска',
    description: 'Строка поиска в верхней панели страницы.',
    tokens: ['panel_raised', 'border', 'muted'],
    radius: { key: 'control', label: 'Закруглённость поля' },
  },
  'topbar-button': {
    label: 'Кнопка действия',
    description: 'Акцентная кнопка в верхней панели («Импортировать»).',
    tokens: ['accent', 'accent_strong'],
    radius: { key: 'button', label: 'Закруглённость кнопки' },
    note: 'Цвет текста на кнопке подбирается автоматически для контраста.',
  },
  'hero-card': {
    label: 'Карточка экземпляра',
    description: 'Главная строка выбранного экземпляра в библиотеке.',
    tokens: ['panel', 'border', 'text'],
    radius: { key: 'card', label: 'Закруглённость карточки' },
  },
  'hero-banner': {
    label: 'Миниатюра строки',
    description: 'Квадратная иконка-миниатюра слева в строке экземпляра.',
    tokens: ['accent', 'accent_strong', 'banner_shade'],
    note: 'Градиент строится от акцента к яркому акценту с затемнением.',
  },
  'hero-play': {
    label: 'Кнопка «Играть»',
    description: 'Главная кнопка запуска игры в строке экземпляра.',
    tokens: ['accent'],
    radius: { key: 'button', label: 'Закруглённость кнопки' },
  },
  'grid-card': {
    label: 'Карточка списка',
    description: 'Карточки остальных экземпляров и списков.',
    tokens: ['panel_raised', 'border', 'text', 'muted'],
    radius: { key: 'card', label: 'Закруглённость карточки' },
  },
  'card-button': {
    label: 'Вторичная кнопка',
    description: 'Тихие кнопки на карточках («Открыть», «Запустить»).',
    tokens: ['quiet', 'border'],
    radius: { key: 'button', label: 'Закруглённость кнопки' },
  },
  'status-chip': {
    label: 'Статус-чип',
    description: 'Индикатор состояния внизу страницы.',
    tokens: ['success', 'muted'],
    radius: { key: 'control', label: 'Закруглённость чипа' },
  },
  text: {
    label: 'Заголовок страницы',
    description: 'Заголовки и подписи страниц лаунчера.',
    tokens: ['text', 'muted'],
    extras: 'text',
  },
}

const currentTarget = computed(() => targetRegistry[selectedTarget.value])
const currentRailSection = computed(() => railSections.find((section) => section.id === activeSection.value)!)

const inspectorTitle = computed(() => currentTarget.value?.label || currentRailSection.value.label)
const inspectorSubtitle = computed(() => currentTarget.value?.description || currentRailSection.value.hint)
const contextLabel = computed(() => selectedTarget.value
  ? `Редактируется: ${currentTarget.value?.label}`
  : `Общие настройки · ${currentRailSection.value.label}`)
const statusHint = computed(() => {
  if (hoveredTarget.value && targetRegistry[hoveredTarget.value]) return `${targetRegistry[hoveredTarget.value].label} — клик, чтобы настроить`
  if (selectedTarget.value) return 'Esc или клик по фону — вернуться к общим настройкам'
  return 'Кликните элемент в предпросмотре — откроется его редактор'
})

const themeLabel = computed(() => themeOptions.find((option) => option.value === props.appearance.theme)?.label || props.appearance.theme)
const fontLabel = computed(() => fontFamilyOptions.find((option) => option.value === props.fontFamily)?.label || props.fontFamily)
const radiusSummary = computed(() => `${props.appearance.radius_tokens.card}/${props.appearance.radius_tokens.button}/${props.appearance.radius_tokens.control}`)

function setAccentColor(value: string) {
  const normalized = value.trim().toLowerCase()
  if (!/^#[0-9a-f]{6}$/.test(normalized)) return
  pushHistory()
  props.appearance.accent_color = normalized
  props.appearance.color_tokens.accent = normalized
}

const accentPickerColor = computed({
  get: () => props.tokens.accent,
  set: (value: string) => setAccentColor(value),
})

const tokenModels = Object.fromEntries(colorTokenOptions.map((option) => {
  const token = option.value
  return [token, computed<string>({
    get: () => props.tokens[token] || '#ffffff',
    set: (value: string) => {
      const normalized = value.trim().toLowerCase()
      if (!/^#[0-9a-f]{6}$/.test(normalized)) return
      if (token === 'accent') {
        setAccentColor(normalized)
        return
      }
      pushHistory()
      props.appearance.color_tokens[token] = normalized
    },
  })]
})) as Record<string, WritableComputedRef<string>>

const selectedColorToken = ref('accent')
const selectedTokenPickerColor = computed({
  get: () => props.tokens[selectedColorToken.value] || '#ffffff',
  set: (value: string) => {
    const model = tokenModels[selectedColorToken.value]
    if (model) model.value = value
  },
})

function resetToken(token: string) {
  pushHistory()
  if (token === 'accent') {
    setAccentColor(props.defaultColorTokens.accent)
    return
  }
  props.appearance.color_tokens[token] = props.defaultColorTokens[token] || props.appearance.color_tokens[token]
}

function resetColorTokens() {
  pushHistory()
  props.appearance.color_tokens = { ...props.defaultColorTokens }
  props.appearance.accent_color = props.defaultColorTokens.accent
}

const activeRadiusPreset = computed(() => {
  const tokens = props.appearance.radius_tokens
  return radiusPresets.findIndex((preset) =>
    Object.entries(preset.value).every(([key, value]) => tokens[key] === value),
  )
})

function applyRadiusPreset(preset: Record<string, number>) {
  pushHistory()
  props.appearance.radius_tokens = { ...preset }
}

function resetRadiusTokens() {
  pushHistory()
  props.appearance.radius_tokens = { ...props.defaultRadiusTokens }
}

type StudioSnapshot = { accent: string; tokens: Record<string, string>; radius: Record<string, number> }
const undoStack = ref<StudioSnapshot[]>([])
const redoStack = ref<StudioSnapshot[]>([])

function snapshotAppearance(): StudioSnapshot {
  return {
    accent: props.appearance.accent_color,
    tokens: { ...props.appearance.color_tokens },
    radius: { ...props.appearance.radius_tokens },
  }
}

function pushHistory() {
  const current = JSON.stringify(snapshotAppearance())
  if (undoStack.value.length && JSON.stringify(undoStack.value[undoStack.value.length - 1]) === current) return
  undoStack.value = [...undoStack.value.slice(-49), snapshotAppearance()]
  redoStack.value = []
}

function restoreSnapshot(snapshot: StudioSnapshot) {
  props.appearance.accent_color = snapshot.accent
  props.appearance.color_tokens = { ...snapshot.tokens }
  props.appearance.radius_tokens = { ...snapshot.radius }
}

function undoAppearance() {
  if (!undoStack.value.length) return
  const previous = undoStack.value[undoStack.value.length - 1]
  undoStack.value = undoStack.value.slice(0, -1)
  redoStack.value = [...redoStack.value, snapshotAppearance()]
  restoreSnapshot(previous)
}

function redoAppearance() {
  if (!redoStack.value.length) return
  const next = redoStack.value[redoStack.value.length - 1]
  redoStack.value = redoStack.value.slice(0, -1)
  undoStack.value = [...undoStack.value, snapshotAppearance()]
  restoreSnapshot(next)
}

function handleStudioKeydown(event: KeyboardEvent) {
  if (!(event.ctrlKey || event.metaKey)) return
  const key = event.key.toLowerCase()
  if (key === 'z' && !event.shiftKey) { event.preventDefault(); undoAppearance() }
  else if (key === 'y' || (key === 'z' && event.shiftKey)) { event.preventDefault(); redoAppearance() }
}

function relativeLuminance(hex: string): number {
  const match = /^#?([0-9a-f]{6})$/i.exec(hex.trim())
  if (!match) return 0
  const raw = match[1]
  const channel = (index: number) => {
    const value = parseInt(raw.slice(index * 2, index * 2 + 2), 16) / 255
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2)
}

function contrastRatio(first: string, second: string): number {
  const a = relativeLuminance(first)
  const b = relativeLuminance(second)
  const [lighter, darker] = a >= b ? [a, b] : [b, a]
  return (lighter + 0.05) / (darker + 0.05)
}

const contrastPairs: Array<{ id: string; label: string; foreground: string; background: string; minimum: number }> = [
  { id: 'text', label: 'Текст на фоне', foreground: 'text', background: 'background', minimum: 4.5 },
  { id: 'text-panel', label: 'Текст на панели', foreground: 'text', background: 'panel', minimum: 4.5 },
  { id: 'muted-panel', label: 'Второстепенный текст на панели', foreground: 'muted', background: 'panel', minimum: 3 },
  { id: 'accent-text', label: 'Акцентный текст на фоне', foreground: 'accent', background: 'background', minimum: 3 },
  { id: 'button', label: 'Текст кнопки на акценте', foreground: 'background', background: 'accent', minimum: 3 },
  { id: 'danger', label: 'Опасное действие на фоне', foreground: 'danger', background: 'background', minimum: 3 },
  { id: 'success', label: 'Успех на фоне', foreground: 'success', background: 'background', minimum: 3 },
]

const contrastAudit = computed(() => contrastPairs.map((pair) => {
  const foreground = String(props.tokens[pair.foreground] || '#000000')
  const background = String(props.tokens[pair.background] || '#ffffff')
  const ratio = contrastRatio(foreground, background)
  return {
    ...pair,
    ratio,
    passes: ratio >= pair.minimum,
    grade: ratio >= 7 ? 'AAA' : ratio >= 4.5 ? 'AA' : ratio >= pair.minimum ? `AA-крупный (${pair.minimum}:1)` : 'провал',
  }
}))
const contrastFailures = computed(() => contrastAudit.value.filter((entry) => !entry.passes))

function fixContrast(entry: { foreground: string; background: string }) {
  // сдвигаем яркость текста от фона, пока контраст не станет комфортным
  const foregroundToken = entry.foreground
  const background = String(props.tokens[entry.background] || '#000000')
  const backgroundIsLight = relativeLuminance(background) > 0.5
  const adjust = (hex: string, lighten: boolean): string => {
    const raw = /^#?([0-9a-f]{6})$/i.exec(hex.trim())?.[1]
    if (!raw) return hex
    const channel = (index: number) => {
      const value = parseInt(raw.slice(index * 2, index * 2 + 2), 16)
      const step = 26
      return Math.max(0, Math.min(255, lighten ? value + step : value - step))
    }
    return `#${[0, 1, 2].map((index) => channel(index).toString(16).padStart(2, '0')).join('')}`
  }
  pushHistory()
  let current = String(props.tokens[foregroundToken] || (backgroundIsLight ? '#000000' : '#ffffff'))
  for (let attempt = 0; attempt < 10; attempt += 1) {
    if (contrastRatio(current, background) >= 4.5) break
    current = adjust(current, backgroundIsLight)
  }
  props.appearance.color_tokens[foregroundToken] = current
}

async function copyTokensJson() {
  const payload = JSON.stringify({
    accent: props.tokens.accent,
    tokens: { ...props.tokens },
    radius: { ...props.appearance.radius_tokens },
  }, null, 2)
  try {
    await navigator.clipboard.writeText(payload)
    copiedTokens.value = true
    window.setTimeout(() => { copiedTokens.value = false }, 1600)
  } catch {
    copiedTokens.value = false
  }
}
const copiedTokens = ref(false)

const accentSuggestions = computed(() => {
  // гармоничные оттенки от тона фона: приглушённые и достаточно контрастные
  const background = String(props.tokens.background || '#0b1220')
  const match = /^#?([0-9a-f]{6})$/i.exec(background.trim())
  if (!match) return []
  const raw = match[1]
  const [r, g, b] = [0, 1, 2].map((index) => parseInt(raw.slice(index * 2, index * 2 + 2), 16) / 255)
  const max = Math.max(r, g, b), min = Math.min(r, g, b)
  let hue = 0
  if (max !== min) {
    const delta = max - min
    if (max === r) hue = ((g - b) / delta + (g < b ? 6 : 0)) / 6
    else if (max === g) hue = ((b - r) / delta + 3) / 6
    else hue = ((r - g) / delta + 6) / 6
    hue = (hue * 360 + 360) % 360
  }
  return [-30, 0, 30, 150, 210].map((offset) => {
    const nextHue = (hue + offset + 360) % 360
    const h = nextHue / 360
    const l = 0.42
    const s = 0.3
    const q = l < 0.5 ? l * (1 + s) : l + s - l * s
    const p = 2 * l - q
    const channel = (t: number) => {
      let tt = t
      if (tt < 0) tt += 1
      if (tt > 1) tt -= 1
      if (tt < 1 / 6) return p + (q - p) * 6 * tt
      if (tt < 1 / 2) return q
      if (tt < 2 / 3) return p + (q - p) * (2 / 3 - tt) * 6
      return p
    }
    return `#${[channel(h + 1 / 3), channel(h), channel(h - 1 / 3)]
      .map((value) => Math.round(value * 255).toString(16).padStart(2, '0'))
      .join('')}`
  })
})

type PreviewWidth = 'compact' | 'standard' | 'wide'
const previewWidth = ref<PreviewWidth>('standard')
const previewWidthOptions: Array<{ id: PreviewWidth; label: string; hint: string }> = [
  { id: 'compact', label: 'Компакт', hint: 'Узкое окно' },
  { id: 'standard', label: 'Стандарт', hint: 'Обычное окно' },
  { id: 'wide', label: 'Широкое', hint: 'Максимум места' },
]
const previewDesignWidth = computed(() => ({ compact: 560, standard: 860, wide: 1180 }[previewWidth.value]))

const radiusKeys: Array<{ key: 'card' | 'button' | 'control'; label: string; hint: string }> = [
  { key: 'card', label: 'Панели и карточки', hint: 'Карточки, модальные окна, секции' },
  { key: 'button', label: 'Кнопки и навигация', hint: 'Кнопки, пункты меню, чипы действий' },
  { key: 'control', label: 'Поля ввода', hint: 'Поиск, поля, мелкие элементы' },
]

const sidebarWidthDisabled = computed(() =>
  props.appearance.sidebar_position === 'top' ||
  props.appearance.sidebar_position === 'bottom' ||
  props.appearance.sidebar_width_mode !== 'manual',
)

function chooseWallpaper() {
  emit('importWallpaperFile')
}

const fontFamilyValue = computed({
  get: () => props.fontFamily,
  set: (value: string) => emit('update:fontFamily', value),
})

const effectiveAnimationStyle = computed(() => props.appearance.animation_settings.page_style)
const effectiveAnimationDuration = computed(() => props.appearance.animation_settings.page_duration_ms)

const saveLabel = computed(() => {
  if (props.saveState === 'saving') return 'Сохраняю…'
  if (props.saveState === 'error') return 'Повторить сохранение'
  if (props.saveState === 'dirty') return 'Сохранить изменения'
  return 'Всё сохранено'
})
</script>

<template>
  <section class="studio-app" @keydown.esc="handleEscape">
    <header class="studio-toolbar">
      <div class="studio-toolbar-copy">
        
        <strong>Центр настройки тем</strong>
        <span class="studio-toolbar-context">{{ contextLabel }}</span>
      </div>
      <div class="studio-toolbar-actions">
        <span class="tag" :class="{ 'tag--danger': saveState === 'error' }">{{ saveMessage || 'Изменения применяются после сохранения' }}</span>
        <div class="studio-history-controls">
          <button class="icon-button" type="button" aria-label="Отменить" v-tooltip="'Отменить (Ctrl+Z)'" :disabled="!undoStack.length" @click="undoAppearance"><Icon name="undo" :size="16" /></button>
          <button class="icon-button" type="button" aria-label="Повторить" v-tooltip="'Повторить (Ctrl+Shift+Z)'" :disabled="!redoStack.length" @click="redoAppearance"><Icon name="redo" :size="16" /></button>
        </div>
        <button class="button button--quiet" type="button" :class="{ 'button--outline': copiedTokens }" @click="copyTokensJson"><Icon name="copy" :size="15" />{{ copiedTokens ? 'Скопировано' : 'JSON палитры' }}</button>
        <button class="button button--outline" type="button" :disabled="busy" @click="emit('resetAppearance')"><Icon name="refresh" :size="15" />Сбросить оформление</button>
        <button class="button button--accent" type="button" :disabled="busy || saveState === 'saving' || saveState === 'saved'" @click="emit('save')"><Icon name="check" :size="15" />{{ saveLabel }}</button>
      </div>
    </header>

    <div class="studio-workbench">
      <nav class="studio-rail" aria-label="Разделы студии">
        <button
          v-for="section in railSections"
          :key="section.id"
          type="button"
          :class="{ 'studio-rail-item--active': !selectedTarget && activeSection === section.id }"
          class="studio-rail-item"
          v-tooltip="section.hint"
          @click="selectRailSection(section.id)"
        >
          <Icon :name="section.icon" :size="16" />
          <span>{{ section.label }}</span>
        </button>
      </nav>

      <div class="studio-viewport">
        <div class="studio-viewport-bar">
          <span><Icon name="search" :size="13" />{{ statusHint }}</span>
          <div class="studio-preview-widths" role="group" aria-label="Ширина превью">
            <button v-for="option in previewWidthOptions" :key="option.id" type="button" :class="{ active: previewWidth === option.id }" v-tooltip="option.hint" @click="previewWidth = option.id">{{ option.label }}</button>
          </div>
          <button v-if="selectedTarget" class="text-button" type="button" @click="clearSelection"><Icon name="close" :size="13" />Общие настройки</button>
        </div>
        <div class="studio-preview-host" :style="{ maxWidth: `${previewDesignWidth}px` }">
          <ThemeStudioPreview
            interactive
            :appearance="appearance"
            :tokens="tokens"
            :theme="baseTheme || theme"
            :default-tokens="defaultColorTokens"
            :light-tokens="lightTokens"
            :font-family="fontFamily"
            :language="language"
            :wallpaper-url="wallpaperUrl"
            :selected="selectedTarget"
            :hovered="hoveredTarget"
            @select="selectTarget"
            @hover="hoveredTarget = $event || ''"
          />
        </div>
      </div>

      <aside class="studio-inspector">
        <header class="studio-inspector-head">
          <div>
            <small>{{ selectedTarget ? 'Элемент интерфейса' : 'Общие настройки' }}</small>
            <strong>{{ inspectorTitle }}</strong>
            <span>{{ inspectorSubtitle }}</span>
          </div>
          <button v-if="selectedTarget" class="icon-button" type="button" aria-label="Закрыть редактор элемента" @click="clearSelection"><Icon name="close" :size="15" /></button>
        </header>

        <div class="studio-inspector-body">
          <template v-if="currentTarget">
            <div v-for="token in currentTarget.tokens" :key="token" class="studio-token-row">
              <div class="studio-token-meta">
                <span class="studio-token-swatch" :style="{ background: tokens[token] }" />
                <div>
                  <strong>{{ tokenLabels[token] || token }}</strong>
                  <small>{{ tokens[token] }}</small>
                </div>
                <button class="icon-button" type="button" :aria-label="`Сбросить ${tokenLabels[token] || token}`" v-tooltip="'Вернуть стандартный цвет'" @click="resetToken(token)"><Icon name="refresh" :size="13" /></button>
              </div>
              <ColorPicker :model-value="tokenModels[token]?.value ?? '#ffffff'" @update:model-value="tokenModels[token] && (tokenModels[token].value = $event)" />
            </div>

            <div v-if="currentTarget.radius" class="studio-inspector-field">
              <label class="field"><span>{{ currentTarget.radius.label }} <strong>{{ appearance.radius_tokens[currentTarget.radius.key] }} px</strong></span><input v-model.number="appearance.radius_tokens[currentTarget.radius.key]" type="range" min="0" max="24" /></label>
            </div>

            <div v-if="currentTarget.extras === 'wallpaper'" class="studio-inspector-group">
              <h4>Обои</h4>
              <div class="wallpaper-controls">
                
                <button class="button button--small button--outline" type="button" @click="chooseWallpaper">Загрузить</button>
                <button v-if="appearance.wallpaper_path" class="button button--small button--quiet" type="button" @click="emit('clearWallpaper')">Убрать</button>
              </div>
              <label class="field"><span>Размытие <strong>{{ appearance.wallpaper_blur }} px</strong></span><input v-model.number="appearance.wallpaper_blur" type="range" min="0" max="32" @change="emit('commitWallpaperBlur')" /></label>
              <label class="field"><span>Видимость <strong>{{ appearance.wallpaper_opacity }}%</strong></span><input v-model.number="appearance.wallpaper_opacity" type="range" min="0" max="80" /></label>
            </div>

            <div v-else-if="currentTarget.extras === 'sidebar'" class="studio-inspector-group">
              <h4>Поведение сайдбара</h4>
              <div class="field"><span>Положение</span><CustomSelect v-model="appearance.sidebar_position" :options="sidebarPositionOptions" /></div>
              <div class="field"><span>Режим содержимого</span><CustomSelect v-model="appearance.sidebar_mode" :options="sidebarModeOptions" /></div>
              <label class="workspace-check"><input v-model="appearance.sidebar_floating" type="checkbox" /><span><strong>Плавающий режим</strong><small>Сайдбар не приклеен к краю окна</small></span></label>
              <button class="text-button" type="button" @click="selectRailSection('navigation')"><Icon name="monitor" :size="13" />Все настройки навигации</button>
            </div>

            <div v-else-if="currentTarget.extras === 'text'" class="studio-inspector-group">
              <h4>Шрифт интерфейса</h4>
              <div class="field"><span>Гарнитура</span><CustomSelect v-model="fontFamilyValue" :options="fontFamilyOptions" /></div>
              <p class="studio-inspector-note">Шрифт применяется ко всему лаунчеру, не только к заголовкам.</p>
            </div>

            <p v-if="currentTarget.note" class="studio-inspector-note">{{ currentTarget.note }}</p>
          </template>

          <template v-else-if="activeSection === 'general'">
            <div class="studio-inspector-field"><div class="field"><span>Основная тема</span><CustomSelect v-model="appearance.theme" :options="themeOptions" /></div></div>
            <div class="studio-inspector-field"><div class="field"><span>Шрифт интерфейса</span><CustomSelect v-model="fontFamilyValue" :options="fontFamilyOptions" /></div></div>
            <div class="studio-inspector-field"><div class="field"><span>Язык интерфейса</span><CustomSelect :model-value="language" :options="languageOptions" @update:model-value="emit('update:language', $event)" /></div></div>
            <p class="studio-inspector-note">При применении темы её язык выбирается автоматически. Здесь можно переключить язык отдельно от темы.</p>
            <div class="studio-inspector-group">
              <h4>Акцент</h4>
              <div class="studio-token-row">
                <div class="studio-token-meta">
                  <span class="studio-token-swatch" :style="{ background: tokens.accent }" />
                  <div><strong>Основной акцент</strong><small>{{ tokens.accent }}</small></div>
                  <button class="icon-button" type="button" aria-label="Сбросить акцент" v-tooltip="'Вернуть стандартный цвет'" @click="resetToken('accent')"><Icon name="refresh" :size="13" /></button>
                </div>
                <ColorPicker :model-value="accentPickerColor" @update:model-value="accentPickerColor = $event" />
              </div>
              <div class="studio-swatches"><button v-for="color in accentPresets" :key="color" type="button" v-tooltip="color" :aria-label="`Акцент ${color}`" :class="{ active: tokens.accent.toLowerCase() === color }" :style="{ background: color }" @click="setAccentColor(color)" /></div>
              <div v-if="accentSuggestions.length" class="studio-accent-suggest">
                <small>Подобрано под фон интерфейса:</small>
                <div class="studio-swatches"><button v-for="color in accentSuggestions" :key="color" type="button" v-tooltip="color" :aria-label="`Акцент ${color}`" :class="{ active: tokens.accent.toLowerCase() === color }" :style="{ background: color }" @click="setAccentColor(color)" /></div>
              </div>
            </div>
            <div class="studio-inspector-field"><div class="field"><span>Плотность</span><CustomSelect v-model="appearance.density" :options="densityOptions" /></div></div>
            <div class="studio-inspector-field"><div class="field"><span>Экземпляры</span><CustomSelect v-model="appearance.instance_layout" :options="instanceLayoutOptions" /></div></div>
            <label class="field"><span>Масштаб интерфейса <strong>{{ appearance.ui_scale }}%</strong></span><input v-model.number="appearance.ui_scale" type="range" min="80" max="125" step="5" /></label>
          </template>

          <template v-else-if="activeSection === 'buttons'">
            <div class="studio-inspector-group">
              <h4>Размер и оформление</h4>
              <label class="field"><span>Ширина <strong>{{ appearance.button_customization.width }} px</strong></span><input v-model.number="appearance.button_customization.width" type="range" min="120" max="280" step="4" /></label>
              <label class="field"><span>Высота <strong>{{ appearance.button_customization.height }} px</strong></span><input v-model.number="appearance.button_customization.height" type="range" min="28" max="64" step="2" /></label>
              <label class="workspace-check"><input v-model="appearance.button_customization.fill" type="checkbox" /><span><strong>Заливка</strong><small>Отключите, чтобы оставить прозрачную кнопку</small></span></label>
              <label class="workspace-check"><input v-model="appearance.button_customization.border" type="checkbox" /><span><strong>Контур кнопки</strong><small>Показывать границу кнопки</small></span></label>
            </div>
            <div class="studio-inspector-group">
              <h4>Текст кнопок</h4>
              <div class="field"><span>Шрифт</span><CustomSelect v-model="appearance.button_customization.font_family" :options="fontFamilyOptions" /></div>
              <label class="field"><span>Размер текста <strong>{{ appearance.button_customization.font_size }} px</strong></span><input v-model.number="appearance.button_customization.font_size" type="range" min="9" max="20" step="1" /></label>
              <label class="workspace-check"><input v-model="appearance.button_customization.outline.enabled" type="checkbox" /><span><strong>Контур текста</strong><small>Улучшает читаемость на текстуре</small></span></label>
              <template v-if="appearance.button_customization.outline.enabled">
                <label class="field"><span>Толщина контура <strong>{{ appearance.button_customization.outline.width }} px</strong></span><input v-model.number="appearance.button_customization.outline.width" type="range" min="0" max="3" step="0.5" /></label>
                <div class="studio-token-row"><div class="studio-token-meta"><span class="studio-token-swatch" :style="{ background: appearance.button_customization.outline.color }" /><div><strong>Цвет контура</strong><small>{{ appearance.button_customization.outline.color }}</small></div></div><ColorPicker v-model="appearance.button_customization.outline.color" /></div>
              </template>
            </div>
            <div class="studio-inspector-group">
              <h4>Текстура, иконка и звук</h4>
              <input ref="buttonTextureInput" class="visually-hidden" type="file" accept="image/png,image/jpeg,image/webp" @change="loadButtonAsset($event, 'texture')" />
              <input ref="buttonIconInput" class="visually-hidden" type="file" accept="image/png,image/jpeg,image/webp" @change="loadButtonAsset($event, 'icon')" />
              <input ref="buttonSoundInput" class="visually-hidden" type="file" accept="audio/mpeg,audio/ogg,audio/wav,audio/webm" @change="loadButtonAsset($event, 'sound')" />
              <div class="button-asset-row"><span><strong>Текстура</strong><small>{{ appearance.button_customization.texture ? 'Подогнана под размер кнопки' : 'Не выбрана' }}</small></span><button class="button button--small button--outline" type="button" @click="chooseButtonAsset('texture')">{{ appearance.button_customization.texture ? 'Заменить' : 'Загрузить' }}</button><button v-if="appearance.button_customization.texture" class="icon-button" type="button" aria-label="Убрать текстуру" @click="clearButtonAsset('texture')"><Icon name="close" :size="14" /></button></div>
              <div class="field"><span>Масштаб</span><CustomSelect v-model="appearance.button_customization.texture_fit" :options="textureFitOptions" /></div>
              <label class="workspace-check"><input v-model="appearance.button_customization.crop_enabled" type="checkbox" /><span><strong>Кадрировать вручную</strong><small>Необязательно: выберите область текстуры</small></span></label>
              <div v-if="appearance.button_customization.crop_enabled" class="button-crop-grid">
                <label class="field"><span>Смещение X <strong>{{ appearance.button_customization.crop.x }}%</strong></span><input v-model.number="appearance.button_customization.crop.x" type="range" min="0" max="100" /></label>
                <label class="field"><span>Смещение Y <strong>{{ appearance.button_customization.crop.y }}%</strong></span><input v-model.number="appearance.button_customization.crop.y" type="range" min="0" max="100" /></label>
                <label class="field"><span>Область X <strong>{{ appearance.button_customization.crop.width }}%</strong></span><input v-model.number="appearance.button_customization.crop.width" type="range" min="5" max="100" /></label>
                <label class="field"><span>Область Y <strong>{{ appearance.button_customization.crop.height }}%</strong></span><input v-model.number="appearance.button_customization.crop.height" type="range" min="5" max="100" /></label>
              </div>
              <div class="button-asset-row"><span><strong>Иконка</strong><small>{{ appearance.button_customization.icon ? 'Заменяет или добавляет иконку' : 'Не выбрана' }}</small></span><button class="button button--small button--outline" type="button" @click="chooseButtonAsset('icon')">{{ appearance.button_customization.icon ? 'Заменить' : 'Добавить' }}</button><button v-if="appearance.button_customization.icon" class="icon-button" type="button" aria-label="Убрать иконку" @click="clearButtonAsset('icon')"><Icon name="close" :size="14" /></button></div>
              <div class="button-asset-row"><span><strong>Звук нажатия</strong><small>{{ appearance.button_customization.sound ? 'Будет звучать при нажатии' : 'Не выбран' }}</small></span><button class="button button--small button--outline" type="button" @click="chooseButtonAsset('sound')">{{ appearance.button_customization.sound ? 'Заменить' : 'Добавить' }}</button><button v-if="appearance.button_customization.sound" class="icon-button" type="button" aria-label="Убрать звук" @click="clearButtonAsset('sound')"><Icon name="close" :size="14" /></button></div>
              <p class="studio-inspector-note">PNG, JPEG или WebP до 512 KiB; звук WAV, OGG, MP3 или WebM до 1 MiB. Кнопка заполняет область текстурой автоматически; большие изображения кадрируются по центру.</p>
              <p v-if="buttonAssetError" class="studio-inspector-note studio-inspector-note--error" role="alert">{{ buttonAssetError }}</p>
            </div>
            <div class="studio-inspector-group">
              <h4>Подписи навигации</h4>
              <label v-for="item in labelOptions" :key="item.key" class="field"><span>{{ item.label }} <strong>{{ Array.from(appearance.button_customization.labels[item.key]).length }}/16</strong></span><input v-model="appearance.button_customization.labels[item.key]" type="text" maxlength="16" :placeholder="item.label" /></label>
              <p class="studio-inspector-note">Если подпись не помещается, она сокращается с многоточием. Наведение показывает полный текст в подсказке лаунчера.</p>
            </div>
          </template>

          <template v-else-if="activeSection === 'colors'">
            <div class="studio-inspector-toolbar">
              <div class="field"><span>Токен цвета</span><CustomSelect v-model="selectedColorToken" :options="colorTokenOptions" /></div>
              <button class="button button--small button--quiet" type="button" @click="resetColorTokens"><Icon name="refresh" :size="13" />Сбросить палитру</button>
            </div>
            <div class="studio-token-row">
              <div class="studio-token-meta">
                <span class="studio-token-swatch" :style="{ background: tokens[selectedColorToken] }" />
                <div><strong>{{ tokenLabels[selectedColorToken] }}</strong><small>{{ tokens[selectedColorToken] }}</small></div>
                <button class="icon-button" type="button" aria-label="Сбросить цвет" v-tooltip="'Вернуть стандартный цвет'" @click="resetToken(selectedColorToken)"><Icon name="refresh" :size="13" /></button>
              </div>
              <ColorPicker :model-value="selectedTokenPickerColor" @update:model-value="selectedTokenPickerColor = $event" />
            </div>
            <div class="studio-contrast-audit">
              <div class="studio-contrast-head">
                <h4><Icon name="eye" :size="13" />Аудит контраста WCAG</h4>
                <span class="tag" :class="contrastFailures.length ? 'tag--danger' : 'tag--success'">{{ contrastFailures.length ? `${contrastFailures.length} проблем` : 'всё читаемо' }}</span>
              </div>
              <ul>
                <li v-for="entry in contrastAudit" :key="entry.id" :class="{ 'studio-contrast-row--fail': !entry.passes }">
                  <span class="studio-contrast-pair"><i :style="{ background: tokens[entry.background] }" /><i :style="{ background: tokens[entry.foreground] }" />{{ entry.label }}</span>
                  <span class="studio-contrast-grade">{{ entry.ratio.toFixed(2) }}:1 · {{ entry.grade }}</span>
                  <button v-if="!entry.passes" class="text-button" type="button" @click="fixContrast(entry)">Исправить</button>
                </li>
              </ul>
              <p class="studio-inspector-note">Пороги: 4.5:1 для обычного текста, 3:1 для крупного и второстепенного. «Исправить» двигает яркость текста, пока контраст не станет достаточным.</p>
            </div>
            <div class="studio-token-list">
              <button
                v-for="option in colorTokenOptions"
                :key="option.value"
                type="button"
                :class="{ active: selectedColorToken === option.value }"
                v-tooltip="option.meta"
                @click="selectedColorToken = option.value"
              ><span :style="{ background: tokens[option.value] }" />{{ option.label }}</button>
            </div>
          </template>

          <template v-else-if="activeSection === 'shape'">
            <div class="studio-radius-presets">
              <button
                v-for="(preset, index) in radiusPresets"
                :key="preset.label"
                type="button"
                :class="{ active: activeRadiusPreset === index }"
                @click="applyRadiusPreset(preset.value)"
              ><strong>{{ preset.label }}</strong><small>{{ preset.hint }}</small></button>
            </div>
            <label v-for="item in radiusKeys" :key="item.key" class="field">
              <span>{{ item.label }} <strong>{{ appearance.radius_tokens[item.key] }} px</strong></span>
              <input v-model.number="appearance.radius_tokens[item.key]" type="range" min="0" max="24" />
              <small>{{ item.hint }}</small>
            </label>
            <div class="studio-inspector-field"><div class="field"><span>Плотность</span><CustomSelect v-model="appearance.density" :options="densityOptions" /></div></div>
            <button class="button button--small button--quiet" type="button" @click="resetRadiusTokens"><Icon name="refresh" :size="13" />Сбросить формы</button>
          </template>

          <template v-else-if="activeSection === 'wallpaper'">
            <div class="wallpaper-controls">
              
              <button class="button button--small button--outline" type="button" @click="chooseWallpaper">Загрузить изображение</button>
              <button v-if="appearance.wallpaper_path" class="button button--small button--quiet" type="button" @click="emit('clearWallpaper')">Убрать обои</button>
            </div>
            <label class="field"><span>Размытие <strong>{{ appearance.wallpaper_blur }} px</strong></span><input v-model.number="appearance.wallpaper_blur" type="range" min="0" max="32" @change="emit('commitWallpaperBlur')" /></label>
            <label class="field"><span>Видимость <strong>{{ appearance.wallpaper_opacity }}%</strong></span><input v-model.number="appearance.wallpaper_opacity" type="range" min="0" max="80" /></label>
            <p class="studio-inspector-note">Изображение уменьшается и размывается заранее, поэтому обои не нагружают интерфейс. Максимальный размер файла — 32 MiB.</p>
          </template>

          <template v-else-if="activeSection === 'navigation'">
            <div class="studio-inspector-field"><div class="field"><span>Положение</span><CustomSelect v-model="appearance.sidebar_position" :options="sidebarPositionOptions" /></div></div>
            <div class="studio-inspector-field"><div class="field"><span>Режим содержимого</span><CustomSelect v-model="appearance.sidebar_mode" :options="sidebarModeOptions" /></div></div>
            <div class="studio-inspector-field"><div class="field"><span>Ширина</span><CustomSelect v-model="appearance.sidebar_width_mode" :options="sidebarWidthModeOptions" /></div></div>
            <div class="studio-inspector-field"><div class="field"><span>Видимость</span><CustomSelect v-model="appearance.sidebar_visibility" :options="sidebarVisibilityOptions" /></div></div>
            <label class="field" :class="{ 'field--disabled': sidebarWidthDisabled }"><span>Размер rail <strong>{{ sidebarWidthDisabled ? 'фиксированный' : `${appearance.sidebar_width} px` }}</strong></span><input v-model.number="appearance.sidebar_width" type="range" min="168" max="360" :disabled="sidebarWidthDisabled" /><small>{{ appearance.sidebar_position === 'top' || appearance.sidebar_position === 'bottom' ? 'Для верхней и нижней панели ширина задаётся окном.' : appearance.sidebar_width_mode === 'manual' ? 'Перетащите ползунок или растяните rail за край.' : 'Ползунок недоступен: выберите режим «Вручную», чтобы менять ширину.' }}</small></label>
            <label class="workspace-check"><input v-model="appearance.sidebar_floating" type="checkbox" /><span><strong>Плавающий режим</strong><small>Сайдбар не приклеен к краю окна</small></span></label>
          </template>

          <template v-else-if="activeSection === 'motion'">
            <div class="studio-inspector-field"><div class="field"><span>Анимации</span><CustomSelect v-model="appearance.animations" :options="animationOptions" /></div></div>
            <div class="studio-inspector-group">
              <h4>Тонкая настройка <span class="tag">{{ effectiveAnimationStyle }} · {{ effectiveAnimationDuration }} ms</span></h4>
              <div class="studio-inspector-field"><div class="field"><span>Стиль страницы</span><CustomSelect v-model="appearance.animation_settings.page_style" :options="animationStyleOptions" /></div></div>
              <label class="field"><span>Длительность страницы <strong>{{ appearance.animation_settings.page_duration_ms }} ms</strong></span><input v-model.number="appearance.animation_settings.page_duration_ms" type="range" min="0" max="1000" step="10" /></label>
              <label class="field"><span>Длительность поверхностей <strong>{{ appearance.animation_settings.surface_duration_ms }} ms</strong></span><input v-model.number="appearance.animation_settings.surface_duration_ms" type="range" min="0" max="1200" step="10" /></label>
              <label class="field"><span>Шаг каскада <strong>{{ appearance.animation_settings.surface_stagger_ms }} ms</strong></span><input v-model.number="appearance.animation_settings.surface_stagger_ms" type="range" min="0" max="200" step="5" /></label>
            </div>
            <div class="studio-inspector-group">
              <h4>Отдельные страницы</h4>
              <p class="studio-inspector-note">«Как глобальная» наследует общий стиль. Переключайте страницы в предпросмотре — смена проигрывает анимацию именно этой страницы.</p>
              <div v-for="page in pages" :key="`animation-${page.id}`" class="page-animation-row"><span><strong>{{ page.title }}</strong></span><CustomSelect v-model="appearance.animation_settings.page_styles[page.id]" :options="perPageAnimationStyleOptions" /><NumericStepper v-model="appearance.animation_settings.page_durations_ms[page.id]" :min="0" :max="1000" :step="10" :aria-label="`Длительность страницы ${page.title}`" /></div>
            </div>
          </template>

          <template v-else-if="activeSection === 'pages'">
            <p class="studio-inspector-note">Тема и раскладка применяются к конкретной странице. Название страницы открывает её в лаунчере.</p>
            <div v-for="page in pages" :key="page.id" class="page-customization-row">
              <span class="page-customization-target" role="button" tabindex="0" v-tooltip="`Открыть страницу «${page.title}»`" @click="emit('openPage', page.id)" @keydown.enter="emit('openPage', page.id)"><strong>{{ page.title }}</strong><small>{{ page.kicker }}</small></span>
              <CustomSelect v-model="appearance.page_themes[page.id]" :options="pageThemeOptions" />
              <CustomSelect v-model="appearance.page_layouts[page.id]" :options="pageLayoutOptions" />
            </div>
            <div class="studio-inspector-group">
              <h4>Виджеты</h4>
              <label class="workspace-check"><input v-model="appearance.widget_visibility['instances.workspace']" type="checkbox" /><span>Рабочая область экземпляра</span></label>
              <label class="workspace-check"><input v-model="appearance.widget_visibility['accounts.status']" type="checkbox" /><span>Статус аккаунтов</span></label>
              <label class="workspace-check"><input v-model="appearance.widget_visibility['mods.context']" type="checkbox" /><span>Контекст каталога ресурсов</span></label>
            </div>
          </template>
        </div>
      </aside>
    </div>

    <footer class="studio-statusbar">
      <span>Тема: {{ themeLabel }}</span>
      <span>Шрифт: {{ fontLabel }}</span>
      <span>Масштаб: {{ appearance.ui_scale }}%</span>
      <span>Радиусы: {{ radiusSummary }}</span>
      <span class="studio-statusbar-hint">{{ statusHint }}</span>
    </footer>
  </section>
</template>
