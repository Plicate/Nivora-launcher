<script setup lang="ts">
import { ref } from 'vue'
import CustomSelect from './CustomSelect.vue'
import ColorPicker from './ColorPicker.vue'
import Icon from './Icon.vue'
import { safeDataUrl, type ButtonCustomization } from '../services/buttonCustomization'
import { pickFile, type PickedFile } from '../services/fileBrowser'
import type { SelectOption } from '../types'

const props = defineProps<{
  settings: ButtonCustomization
  resourcePrefix: string
  buttonLabel?: string
  labelPlaceholder?: string
}>()

const emit = defineEmits<{
  'update:settings': [patch: Partial<ButtonCustomization>]
  'update:buttonLabel': [value: string]
}>()

const error = ref('')

const busy = ref(false)
const MAX_ASSET_BYTES = 15 * 1024 * 1024
const MAX_IMAGE_PIXELS = 40_000_000
const AUDIO_DURATION_LIMIT_SECONDS = 5
const textureFitOptions: SelectOption[] = [
  { value: 'cover', label: 'Заполнить кнопку', meta: 'Увеличить маленькую, автоматически кадрировать большую' },
  { value: 'contain', label: 'Показать целиком', meta: 'Сохранить всю картинку внутри кнопки' },
]

type AssetKind = 'texture' | 'icon' | 'sound'
type AssetMime = 'image/png' | 'image/jpeg' | 'image/webp' | 'audio/mpeg' | 'audio/ogg' | 'audio/wav' | 'audio/webm'
type CropInteraction = { mode: 'move' | 'resize'; handle?: string; pointerX: number; pointerY: number; crop: ButtonCustomization['crop'] }

const IMAGE_EXTENSIONS: Record<string, AssetMime> = { '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.webp': 'image/webp' }
const AUDIO_EXTENSIONS: Record<string, AssetMime> = { '.mp3': 'audio/mpeg', '.mpeg': 'audio/mpeg', '.ogg': 'audio/ogg', '.oga': 'audio/ogg', '.wav': 'audio/wav', '.webm': 'audio/webm' }
const cropInteraction = ref<CropInteraction | null>(null)

function update<K extends keyof ButtonCustomization>(key: K, value: ButtonCustomization[K]) {
  emit('update:settings', { [key]: value } as Partial<ButtonCustomization>)
}

function updateOutline(key: 'enabled' | 'width' | 'color', value: boolean | number | string) {
  emit('update:settings', { outline: { ...props.settings.outline, [key]: value } })
}

function acceptFor(kind: AssetKind): string {
  return kind === 'sound' ? '.mp3,.ogg,.oga,.wav,.webm' : '.png,.jpg,.jpeg,.webp'
}

async function chooseAsset(kind: AssetKind) {
  if (busy.value) return
  error.value = ''
  const picked = await pickFile({
    accept: acceptFor(kind),
    title: kind === 'sound' ? 'Выберите звук кнопки' : kind === 'texture' ? 'Выберите текстуру кнопки' : 'Выберите иконку кнопки',
    maxSize: MAX_ASSET_BYTES,
  })
  if (picked) await importPickedAsset(picked, kind)
}

function extensionMime(fileName: string, kind: AssetKind): AssetMime | null {
  const extension = fileName.slice(fileName.lastIndexOf('.')).toLowerCase()
  return (kind === 'sound' ? AUDIO_EXTENSIONS : IMAGE_EXTENSIONS)[extension] || null
}

function normalizeDeclaredMime(value: string): string {
  const mime = value.trim().toLowerCase().split(';', 1)[0]
  if (mime === 'image/jpg') return 'image/jpeg'
  if (mime === 'audio/mp3' || mime === 'audio/x-mpeg') return 'audio/mpeg'
  if (mime === 'audio/x-wav' || mime === 'audio/wave') return 'audio/wav'
  return mime
}

function sniffMime(bytes: Uint8Array): AssetMime | null {
  if (bytes.length >= 8 && bytes[0] === 0x89 && bytes[1] === 0x50 && bytes[2] === 0x4e && bytes[3] === 0x47 && bytes[4] === 0x0d && bytes[5] === 0x0a && bytes[6] === 0x1a && bytes[7] === 0x0a) return 'image/png'
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) return 'image/jpeg'
  if (bytes.length >= 12 && ascii(bytes, 0, 4) === 'RIFF' && ascii(bytes, 8, 4) === 'WEBP') return 'image/webp'
  if (bytes.length >= 4 && ascii(bytes, 0, 4) === 'OggS') return 'audio/ogg'
  if (bytes.length >= 12 && ascii(bytes, 0, 4) === 'RIFF' && ascii(bytes, 8, 4) === 'WAVE') return 'audio/wav'
  if (bytes.length >= 4 && bytes[0] === 0x1a && bytes[1] === 0x45 && bytes[2] === 0xdf && bytes[3] === 0xa3) return 'audio/webm'
  if (bytes.length >= 3 && ascii(bytes, 0, 3) === 'ID3') return 'audio/mpeg'
  if (bytes.length >= 2 && bytes[0] === 0xff && (bytes[1] & 0xe0) === 0xe0) return 'audio/mpeg'
  return null
}

function ascii(bytes: Uint8Array, offset: number, length: number): string {
  return String.fromCharCode(...bytes.subarray(offset, offset + length))
}

function matchesAssetFormat(fileName: string, declaredType: string, bytes: Uint8Array, kind: AssetKind): AssetMime | null {
  const detected = sniffMime(bytes)
  if (!detected || (kind === 'sound') !== detected.startsWith('audio/')) return null
  const extension = extensionMime(fileName, kind)
  const declared = normalizeDeclaredMime(declaredType)
  const typeIsGeneric = !declared || declared === 'application/octet-stream'
  if (extension && extension !== detected) return null
  if (typeIsGeneric ? extension !== detected : declared !== detected) return null
  return detected
}

function decodeBase64(value: string, size: number): Uint8Array | null {
  if (!Number.isSafeInteger(size) || size <= 0 || size > MAX_ASSET_BYTES || value.length > Math.ceil(MAX_ASSET_BYTES / 3) * 4) return null
  try {
    const binary = atob(value)
    if (binary.length !== size || binary.length > MAX_ASSET_BYTES) return null
    return Uint8Array.from(binary, (char) => char.charCodeAt(0))
  } catch {
    return null
  }
}

function encodeBase64(bytes: Uint8Array): string {
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000))
  }
  return btoa(binary)
}

function assetBlob(bytes: Uint8Array, mime: AssetMime): Blob {
  const buffer = new ArrayBuffer(bytes.byteLength)
  new Uint8Array(buffer).set(bytes)
  return new Blob([buffer], { type: mime })
}

function validateImage(bytes: Uint8Array, mime: AssetMime): Promise<boolean> {
  return new Promise((resolve) => {
    const url = URL.createObjectURL(assetBlob(bytes, mime))
    const image = new Image()
    let settled = false
    const timeout = window.setTimeout(() => finish(false), 10_000)
    const finish = (valid: boolean) => {
      if (settled) return
      settled = true
      const width = image.naturalWidth
      const height = image.naturalHeight
      window.clearTimeout(timeout)
      image.onload = null
      image.onerror = null
      image.src = ''
      URL.revokeObjectURL(url)
      resolve(valid && width > 0 && height > 0 && width * height <= MAX_IMAGE_PIXELS)
    }
    image.onload = () => finish(true)
    image.onerror = () => finish(false)
    image.src = url
  })
}

function validateAudioDuration(bytes: Uint8Array, mime: AssetMime): Promise<number | null> {
  return new Promise((resolve) => {
    const url = URL.createObjectURL(assetBlob(bytes, mime))
    const audio = new Audio()
    let settled = false
    const finish = (duration: number | null) => {
      if (settled) return
      settled = true
      window.clearTimeout(timeout)
      audio.removeEventListener('loadedmetadata', onMetadata)
      audio.removeEventListener('error', onError)
      audio.removeAttribute('src')
      audio.load()
      URL.revokeObjectURL(url)
      resolve(duration)
    }
    const onMetadata = () => finish(Number.isFinite(audio.duration) && audio.duration > 0 ? audio.duration : null)
    const onError = () => finish(null)
    const timeout = window.setTimeout(() => finish(null), 15_000)
    audio.preload = 'metadata'
    audio.addEventListener('loadedmetadata', onMetadata, { once: true })
    audio.addEventListener('error', onError, { once: true })
    audio.src = url
    audio.load()
  })
}

async function importPickedAsset(picked: PickedFile, kind: AssetKind) {
  if (busy.value) return
  busy.value = true
  error.value = ''
  try {
    const bytes = decodeBase64(picked.data, picked.size)
    if (!bytes) {
      error.value = 'Файл пустой или превышает лимит 15 MiB.'
      return
    }
    const mime = matchesAssetFormat(picked.fileName, '', bytes, kind)
    if (!mime) {
      error.value = kind === 'sound' ? 'Выберите корректный WAV, OGG, MP3 или WebM.' : 'Выберите корректный PNG, JPEG или WebP.'
      return
    }
    await saveAsset(bytes, mime, kind)
  } catch {
    error.value = 'Не удалось проверить или прочитать файл.'
  } finally {
    busy.value = false
  }
}

async function saveAsset(bytes: Uint8Array, mime: AssetMime, kind: AssetKind) {
  if (kind === 'sound') {
    const duration = await validateAudioDuration(bytes, mime)
    if (duration === null) {
      error.value = 'Не удалось прочитать длительность звука.'
      return
    }
    if (duration > AUDIO_DURATION_LIMIT_SECONDS) {
      error.value = 'Длительность звука не должна превышать 5 секунд.'
      return
    }
  } else if (!(await validateImage(bytes, mime))) {
    error.value = 'Не удалось прочитать изображение или оно слишком большое по разрешению.'
    return
  }
  const safe = safeDataUrl(`data:${mime};base64,${encodeBase64(bytes)}`, kind === 'sound' ? 'audio' : 'image')
  if (!safe) {
    error.value = 'Файл не прошёл проверку формата или превышает лимит 15 MiB.'
    return
  }
  const extension = mime === 'image/jpeg' ? 'jpg' : mime === 'audio/mpeg' ? 'mp3' : mime.split('/')[1]
  const prefix = props.resourcePrefix === 'global' ? 'button' : `button-${props.resourcePrefix}`
  emit('update:settings', {
    [kind]: `resources/${prefix}-${kind}.${extension}`,
    [`${kind}_data`]: safe,
  } as Partial<ButtonCustomization>)
  error.value = ''
}

function clearAsset(kind: AssetKind) {
  emit('update:settings', { [kind]: '', [`${kind}_data`]: '' } as Partial<ButtonCustomization>)
  error.value = ''
}

function startCropMove(event: PointerEvent) {
  startCropInteraction(event, 'move')
}

function startCropResize(event: PointerEvent, handle: string) {
  startCropInteraction(event, 'resize', handle)
}

function startCropInteraction(event: PointerEvent, mode: CropInteraction['mode'], handle?: string) {
  if (event.button !== 0) return
  event.preventDefault()
  const target = event.currentTarget as HTMLElement
  target.setPointerCapture(event.pointerId)
  cropInteraction.value = {
    mode,
    handle,
    pointerX: event.clientX,
    pointerY: event.clientY,
    crop: { ...props.settings.crop },
  }
}

function moveCrop(event: PointerEvent) {
  const interaction = cropInteraction.value
  const stage = event.currentTarget as HTMLElement
  if (!interaction || !stage.clientWidth || !stage.clientHeight) return
  const dx = (event.clientX - interaction.pointerX) / stage.clientWidth * 100
  const dy = (event.clientY - interaction.pointerY) / stage.clientHeight * 100
  const crop = { ...interaction.crop }
  if (interaction.mode === 'move') {
    crop.x = clamp(interaction.crop.x + dx, 0, 100 - crop.width)
    crop.y = clamp(interaction.crop.y + dy, 0, 100 - crop.height)
  } else {
    const handle = interaction.handle || ''
    const right = interaction.crop.x + interaction.crop.width
    const bottom = interaction.crop.y + interaction.crop.height
    if (handle.includes('e')) crop.width = clamp(interaction.crop.width + dx, 5, 100 - interaction.crop.x)
    if (handle.includes('s')) crop.height = clamp(interaction.crop.height + dy, 5, 100 - interaction.crop.y)
    if (handle.includes('w')) {
      crop.x = clamp(interaction.crop.x + dx, 0, right - 5)
      crop.width = right - crop.x
    }
    if (handle.includes('n')) {
      crop.y = clamp(interaction.crop.y + dy, 0, bottom - 5)
      crop.height = bottom - crop.y
    }
  }
  emit('update:settings', { crop })
}

function endCropInteraction(event: PointerEvent) {
  const target = event.currentTarget as HTMLElement
  if (target.hasPointerCapture(event.pointerId)) target.releasePointerCapture(event.pointerId)
  cropInteraction.value = null
}

function clamp(value: number, min: number, max: number): number {
  return Math.round(Math.max(min, Math.min(max, value)))
}

</script>

<template>
  <div class="theme-button-settings-editor">
    <section class="studio-inspector-group">
      <h4>Размер и оформление</h4>
      <label class="field"><span>Ширина <strong>{{ settings.width }} px</strong></span><input type="range" min="120" max="280" step="4" :value="settings.width" @input="update('width', Number(($event.target as HTMLInputElement).value))" /></label>
      <label class="field"><span>Высота <strong>{{ settings.height }} px</strong></span><input type="range" min="28" max="64" step="2" :value="settings.height" @input="update('height', Number(($event.target as HTMLInputElement).value))" /></label>
      <label class="workspace-check"><input type="checkbox" :checked="settings.fill" @change="update('fill', ($event.target as HTMLInputElement).checked)" /><span><strong>Заливка</strong><small>Отключите, чтобы оставить прозрачную кнопку</small></span></label>
      <label class="workspace-check"><input type="checkbox" :checked="settings.border" @change="update('border', ($event.target as HTMLInputElement).checked)" /><span><strong>Контур кнопки</strong><small>Показывать границу кнопки</small></span></label>
    </section>

    <section class="studio-inspector-group">
      <h4>Текст кнопки</h4>
      <label v-if="buttonLabel !== undefined" class="field"><span>Содержание</span><input :value="buttonLabel" type="text" maxlength="16" :placeholder="labelPlaceholder" aria-label="Текст выбранной кнопки" @input="emit('update:buttonLabel', ($event.target as HTMLInputElement).value)" /></label>
      <div class="field"><span>Шрифт</span><CustomSelect :model-value="settings.font_family" :options="[{ value: 'system', label: 'Системный' }, { value: 'inter', label: 'Inter' }, { value: 'manrope', label: 'Manrope' }, { value: 'mono', label: 'DM Mono' }]" @update:model-value="update('font_family', $event as ButtonCustomization['font_family'])" /></div>
      <label class="field"><span>Размер текста <strong>{{ settings.font_size }} px</strong></span><input type="range" min="9" max="20" step="1" :value="settings.font_size" @input="update('font_size', Number(($event.target as HTMLInputElement).value))" /></label>
      <label class="workspace-check"><input type="checkbox" :checked="settings.outline.enabled" @change="updateOutline('enabled', ($event.target as HTMLInputElement).checked)" /><span><strong>Контур текста</strong><small>Улучшает читаемость на текстуре</small></span></label>
      <template v-if="settings.outline.enabled">
        <label class="field"><span>Толщина контура <strong>{{ settings.outline.width }} px</strong></span><input type="range" min="0" max="3" step="0.5" :value="settings.outline.width" @input="updateOutline('width', Number(($event.target as HTMLInputElement).value))" /></label>
        <div class="studio-token-row"><div class="studio-token-meta"><span class="studio-token-swatch" :style="{ background: settings.outline.color }" /><div><strong>Цвет контура</strong><small>{{ settings.outline.color }}</small></div></div><ColorPicker :model-value="settings.outline.color" @update:model-value="updateOutline('color', $event)" /></div>
      </template>
    </section>

    <section class="studio-inspector-group">
      <h4>Текстура, иконка и звук</h4>
      <div class="button-asset-row"><span><strong>Текстура</strong><small>{{ settings.texture ? 'Подогнана под размер кнопки' : 'Не выбрана' }}</small></span><button class="button button--small button--outline" type="button" :disabled="busy" @click="chooseAsset('texture')">{{ settings.texture ? 'Заменить' : 'Загрузить' }}</button><button v-if="settings.texture" class="icon-button" type="button" aria-label="Убрать текстуру" @click="clearAsset('texture')"><Icon name="close" :size="14" /></button></div>
      <div class="field"><span>Масштаб</span><CustomSelect :model-value="settings.texture_fit" :options="textureFitOptions" @update:model-value="update('texture_fit', $event as ButtonCustomization['texture_fit'])" /></div>
      <label class="workspace-check"><input type="checkbox" :checked="settings.crop_enabled" @change="update('crop_enabled', ($event.target as HTMLInputElement).checked)" /><span><strong>Кадрировать вручную</strong><small>Необязательно: выберите область текстуры</small></span></label>
      <div v-if="settings.crop_enabled && settings.texture_data" class="button-crop-editor">
        <div class="button-crop-stage" @pointermove="moveCrop" @pointerup="endCropInteraction" @pointercancel="endCropInteraction">
          <img :src="settings.texture_data" alt="Текстура кнопки" draggable="false" />
          <div class="button-crop-region" :style="{ left: `${settings.crop.x}%`, top: `${settings.crop.y}%`, width: `${settings.crop.width}%`, height: `${settings.crop.height}%` }" role="group" aria-label="Область кадрирования" @pointerdown="startCropMove">
            <span class="button-crop-region-hint">Перетащите область</span>
            <span class="button-crop-handle button-crop-handle--nw" aria-hidden="true" @pointerdown.stop="startCropResize($event, 'nw')" />
            <span class="button-crop-handle button-crop-handle--ne" aria-hidden="true" @pointerdown.stop="startCropResize($event, 'ne')" />
            <span class="button-crop-handle button-crop-handle--sw" aria-hidden="true" @pointerdown.stop="startCropResize($event, 'sw')" />
            <span class="button-crop-handle button-crop-handle--se" aria-hidden="true" @pointerdown.stop="startCropResize($event, 'se')" />
          </div>
        </div>
      </div>
      <div class="button-asset-row"><span><strong>Иконка</strong><small>{{ settings.icon ? 'Заменяет или добавляет иконку' : 'Не выбрана' }}</small></span><button class="button button--small button--outline" type="button" :disabled="busy" @click="chooseAsset('icon')">{{ settings.icon ? 'Заменить' : 'Добавить' }}</button><button v-if="settings.icon" class="icon-button" type="button" aria-label="Убрать иконку" @click="clearAsset('icon')"><Icon name="close" :size="14" /></button></div>
      <div class="button-asset-row"><span><strong>Звук нажатия</strong><small>{{ settings.sound ? 'Будет звучать при нажатии' : 'Не выбран' }}</small></span><button class="button button--small button--outline" type="button" :disabled="busy" @click="chooseAsset('sound')">{{ settings.sound ? 'Заменить' : 'Добавить' }}</button><button v-if="settings.sound" class="icon-button" type="button" aria-label="Убрать звук" @click="clearAsset('sound')"><Icon name="close" :size="14" /></button></div>
      <p class="studio-inspector-note">Изображения и звук до 15 MiB; длительность звука — не более 5 секунд.</p>
      <p v-if="error" class="studio-inspector-note studio-inspector-note--error" role="alert">{{ error }}</p>
    </section>
  </div>
</template>

<style scoped>
.button-crop-editor { display: grid; gap: 7px; }
.button-crop-stage { position: relative; justify-self: start; width: fit-content; max-width: 100%; max-height: 220px; overflow: hidden; border: 1px solid var(--line); border-radius: 6px; background: repeating-conic-gradient(var(--panel-soft) 0% 25%, var(--panel-raised) 0% 50%) 50% / 14px 14px; touch-action: none; user-select: none; }
.button-crop-stage > img { display: block; width: auto; height: auto; max-width: 100%; max-height: 220px; pointer-events: none; }
.button-crop-region { position: absolute; box-sizing: border-box; min-width: 5%; min-height: 5%; border: 2px solid var(--accent, #4a8dff); box-shadow: 0 0 0 1px var(--panel), 0 0 0 999px rgb(0 0 0 / 48%); cursor: move; touch-action: none; }
.button-crop-region-hint { position: absolute; bottom: 4px; left: 50%; padding: 2px 5px; border-radius: 3px; color: var(--text); background: var(--panel-raised); font-size: 8px; line-height: 1.2; white-space: nowrap; transform: translateX(-50%); pointer-events: none; }
.button-crop-handle { position: absolute; width: 10px; height: 10px; border: 1px solid var(--panel); border-radius: 2px; background: var(--accent, #4a8dff); }
.button-crop-handle--nw { top: 0; left: 0; transform: translate(-50%, -50%); cursor: nwse-resize; }
.button-crop-handle--ne { top: 0; right: 0; transform: translate(50%, -50%); cursor: nesw-resize; }
.button-crop-handle--sw { bottom: 0; left: 0; transform: translate(-50%, 50%); cursor: nesw-resize; }
.button-crop-handle--se { right: 0; bottom: 0; transform: translate(50%, 50%); cursor: nwse-resize; }
</style>
