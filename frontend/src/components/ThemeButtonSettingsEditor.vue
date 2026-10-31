<script setup lang="ts">
import { ref } from 'vue'
import CustomSelect from './CustomSelect.vue'
import ColorPicker from './ColorPicker.vue'
import Icon from './Icon.vue'
import { safeDataUrl } from '../services/buttonCustomization'
import type { ButtonCustomization } from '../services/buttonCustomization'
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

const textureInput = ref<HTMLInputElement>()
const iconInput = ref<HTMLInputElement>()
const soundInput = ref<HTMLInputElement>()
const error = ref('')
const textureFitOptions: SelectOption[] = [
  { value: 'cover', label: 'Заполнить кнопку', meta: 'Увеличить маленькую, автоматически кадрировать большую' },
  { value: 'contain', label: 'Показать целиком', meta: 'Сохранить всю картинку внутри кнопки' },
]

function update<K extends keyof ButtonCustomization>(key: K, value: ButtonCustomization[K]) {
  emit('update:settings', { [key]: value } as Partial<ButtonCustomization>)
}

function updateOutline(key: 'enabled' | 'width' | 'color', value: boolean | number | string) {
  emit('update:settings', { outline: { ...props.settings.outline, [key]: value } })
}

function updateCrop(key: 'x' | 'y' | 'width' | 'height', value: number) {
  emit('update:settings', { crop: { ...props.settings.crop, [key]: value } })
}

function chooseAsset(kind: 'texture' | 'icon' | 'sound') {
  const input = kind === 'texture' ? textureInput.value : kind === 'icon' ? iconInput.value : soundInput.value
  input?.click()
}

async function loadAsset(event: Event, kind: 'texture' | 'icon' | 'sound') {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  const imageTypes = new Set(['image/png', 'image/jpeg', 'image/webp'])
  const audioTypes = new Set(['audio/mpeg', 'audio/ogg', 'audio/wav', 'audio/webm'])
  const accepted = kind === 'sound' ? audioTypes : imageTypes
  const limit = kind === 'sound' ? 1_048_576 : 524_288
  if (!accepted.has(file.type) || file.size > limit) {
    error.value = kind === 'sound' ? 'Выберите WAV, OGG, MP3 или WebM до 1 MiB.' : 'Выберите PNG, JPEG или WebP до 512 KiB.'
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
    error.value = 'Файл не прошёл проверку формата.'
    return
  }
  const extension = file.type === 'image/jpeg' ? 'jpg' : file.type.split('/')[1].replace('mpeg', 'mp3')
  const prefix = props.resourcePrefix === 'global' ? 'button' : `button-${props.resourcePrefix}`
  emit('update:settings', {
    [kind]: `resources/${prefix}-${kind}.${extension}`,
    [`${kind}_data`]: safe,
  } as Partial<ButtonCustomization>)
  error.value = ''
}

function clearAsset(kind: 'texture' | 'icon' | 'sound') {
  emit('update:settings', { [kind]: '', [`${kind}_data`]: '' } as Partial<ButtonCustomization>)
  error.value = ''
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
      <input ref="textureInput" class="visually-hidden" type="file" accept="image/png,image/jpeg,image/webp" @change="loadAsset($event, 'texture')" />
      <input ref="iconInput" class="visually-hidden" type="file" accept="image/png,image/jpeg,image/webp" @change="loadAsset($event, 'icon')" />
      <input ref="soundInput" class="visually-hidden" type="file" accept="audio/mpeg,audio/ogg,audio/wav,audio/webm" @change="loadAsset($event, 'sound')" />
      <div class="button-asset-row"><span><strong>Текстура</strong><small>{{ settings.texture ? 'Подогнана под размер кнопки' : 'Не выбрана' }}</small></span><button class="button button--small button--outline" type="button" @click="chooseAsset('texture')">{{ settings.texture ? 'Заменить' : 'Загрузить' }}</button><button v-if="settings.texture" class="icon-button" type="button" aria-label="Убрать текстуру" @click="clearAsset('texture')"><Icon name="close" :size="14" /></button></div>
      <div class="field"><span>Масштаб</span><CustomSelect :model-value="settings.texture_fit" :options="textureFitOptions" @update:model-value="update('texture_fit', $event as ButtonCustomization['texture_fit'])" /></div>
      <label class="workspace-check"><input type="checkbox" :checked="settings.crop_enabled" @change="update('crop_enabled', ($event.target as HTMLInputElement).checked)" /><span><strong>Кадрировать вручную</strong><small>Необязательно: выберите область текстуры</small></span></label>
      <div v-if="settings.crop_enabled" class="button-crop-grid">
        <label class="field"><span>Смещение X <strong>{{ settings.crop.x }}%</strong></span><input type="range" min="0" max="100" :value="settings.crop.x" @input="updateCrop('x', Number(($event.target as HTMLInputElement).value))" /></label>
        <label class="field"><span>Смещение Y <strong>{{ settings.crop.y }}%</strong></span><input type="range" min="0" max="100" :value="settings.crop.y" @input="updateCrop('y', Number(($event.target as HTMLInputElement).value))" /></label>
        <label class="field"><span>Область X <strong>{{ settings.crop.width }}%</strong></span><input type="range" min="5" max="100" :value="settings.crop.width" @input="updateCrop('width', Number(($event.target as HTMLInputElement).value))" /></label>
        <label class="field"><span>Область Y <strong>{{ settings.crop.height }}%</strong></span><input type="range" min="5" max="100" :value="settings.crop.height" @input="updateCrop('height', Number(($event.target as HTMLInputElement).value))" /></label>
      </div>
      <div class="button-asset-row"><span><strong>Иконка</strong><small>{{ settings.icon ? 'Заменяет или добавляет иконку' : 'Не выбрана' }}</small></span><button class="button button--small button--outline" type="button" @click="chooseAsset('icon')">{{ settings.icon ? 'Заменить' : 'Добавить' }}</button><button v-if="settings.icon" class="icon-button" type="button" aria-label="Убрать иконку" @click="clearAsset('icon')"><Icon name="close" :size="14" /></button></div>
      <div class="button-asset-row"><span><strong>Звук нажатия</strong><small>{{ settings.sound ? 'Будет звучать при нажатии' : 'Не выбран' }}</small></span><button class="button button--small button--outline" type="button" @click="chooseAsset('sound')">{{ settings.sound ? 'Заменить' : 'Добавить' }}</button><button v-if="settings.sound" class="icon-button" type="button" aria-label="Убрать звук" @click="clearAsset('sound')"><Icon name="close" :size="14" /></button></div>
      <p class="studio-inspector-note">PNG, JPEG или WebP до 512 KiB; звук WAV, OGG, MP3 или WebM до 1 MiB.</p>
      <p v-if="error" class="studio-inspector-note studio-inspector-note--error" role="alert">{{ error }}</p>
    </section>
  </div>
</template>
