<script setup lang="ts">
// поле записи горячей клавиши: вместо свободного текста ловим живое
// нажатие и собираем комбинацию сами — «ввести что угодно» больше нельзя.
import { nextTick, onBeforeUnmount, ref } from 'vue'
import Icon from './Icon.vue'
import { hotkeyFromEvent } from '../services/overlayHelpers'

const props = withDefaults(
  defineProps<{
    modelValue: string
    disabled?: boolean
    defaultHint?: string
  }>(),
  { disabled: false, defaultHint: '' },
)
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

const recording = ref(false)
const button = ref<HTMLElement | null>(null)

async function startRecording() {
  if (props.disabled || recording.value) return
  recording.value = true
  await nextTick()
  button.value?.focus()
}

function stopRecording() {
  recording.value = false
}

function onKeyDown(event: KeyboardEvent) {
  if (!recording.value) return
  event.preventDefault()
  event.stopPropagation()
  if (event.key === 'Escape') {
    recording.value = false
    return
  }
  if (event.key === 'Backspace') {
    recording.value = false
    emit('update:modelValue', '')
    return
  }
  const combination = hotkeyFromEvent(event)
  if (combination) {
    recording.value = false
    emit('update:modelValue', combination)
  }
}

onBeforeUnmount(stopRecording)
</script>

<template>
  <button
    ref="button"
    class="hotkey-input"
    :class="{ 'hotkey-input--recording': recording, 'hotkey-input--empty': !modelValue && !recording }"
    type="button"
    :disabled="disabled"
    :aria-label="recording ? 'Идёт запись комбинации, Esc — отмена' : 'Нажмите, чтобы записать комбинацию клавиш'"
    @keydown="onKeyDown"
    @click="startRecording"
    @blur="stopRecording"
  >
    <template v-if="recording">
      <span class="hotkey-input__recording"><Icon name="keyboard" :size="15" /> Нажмите комбинацию…</span>
      <small class="hotkey-input__hint">Esc — отмена · Backspace — сброс</small>
    </template>
    <template v-else-if="modelValue">
      <span class="hotkey-input__keys">
        <kbd v-for="part in modelValue.split('+')" :key="part">{{ part }}</kbd>
      </span>
      <small class="hotkey-input__hint">нажмите, чтобы изменить</small>
    </template>
    <template v-else>
      <span class="hotkey-input__placeholder">Не назначено</span>
      <small v-if="defaultHint" class="hotkey-input__hint">{{ defaultHint }}</small>
    </template>
  </button>
</template>

<style scoped>
.hotkey-input {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  justify-content: center;
  gap: 4px 10px;
  width: 100%;
  min-height: 42px;
  padding: 7px 11px;
  border: 1px solid var(--line);
  border-radius: var(--radius-button, 8px);
  background: var(--panel-raised);
  color: var(--text);
  cursor: pointer;
  text-align: left;
  transition: border-color .15s ease, background-color .15s ease, box-shadow .18s ease, transform .12s ease;
}
.hotkey-input:hover:not(:disabled) { border-color: var(--blue); background: var(--panel-soft); }
.hotkey-input:active:not(:disabled) { transform: translateY(1px); }
.hotkey-input:focus-visible { outline: none; border-color: var(--blue); box-shadow: 0 0 0 3px color-mix(in srgb, var(--blue) 25%, transparent); }
.hotkey-input:disabled { opacity: .55; cursor: not-allowed; }
.hotkey-input--recording {
  border-color: var(--blue);
  animation: hotkey-input-pulse 1.2s ease-in-out infinite;
}
.hotkey-input--empty .hotkey-input__placeholder { color: var(--muted); }
.hotkey-input__recording {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  font-size: 13px;
  color: var(--blue-bright);
  font-weight: 600;
}
.hotkey-input__recording svg { color: var(--blue-bright); }
.hotkey-input__keys { display: inline-flex; align-items: center; gap: 5px; flex-wrap: wrap; min-width: 0; }
.hotkey-input__keys kbd {
  padding: 3px 8px;
  border: 1px solid var(--line-strong);
  border-bottom-width: 2px;
  border-radius: 6px;
  background: var(--panel);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  font-weight: 600;
  color: var(--text);
  white-space: nowrap;
}
/* Подсказка — отдельной строкой под клавишами: раньше стояла рядом в одной
   строке без переноса и в узких колонках наезжала на сами клавиши. */
.hotkey-input__hint {
  max-width: 100%;
  color: var(--muted);
  font-size: 10.5px;
  line-height: 1.35;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.hotkey-input__placeholder { font-size: 13px; color: var(--muted); }
@keyframes hotkey-input-pulse {
  0%, 100% { box-shadow: 0 0 0 2px color-mix(in srgb, var(--blue) 18%, transparent); }
  50% { box-shadow: 0 0 0 5px color-mix(in srgb, var(--blue) 18%, transparent); }
}
</style>
