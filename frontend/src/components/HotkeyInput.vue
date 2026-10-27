<script setup lang="ts">
// записываем клавишу напрямую, не принимая произвольный текст
import { nextTick, onBeforeUnmount, ref } from 'vue'
import Icon from './Icon.vue'
import { hotkeyFromEvent, hotkeyModifiersFromEvent } from '../services/overlayHelpers'

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
const recordingModifiers = ref<string[]>([])
const button = ref<HTMLElement | null>(null)

async function startRecording() {
  if (props.disabled || recording.value) return
  recording.value = true
  recordingModifiers.value = []
  await nextTick()
  button.value?.focus()
}

function stopRecording() {
  recording.value = false
  recordingModifiers.value = []
}

function onKeyDown(event: KeyboardEvent) {
  if (!recording.value) return
  event.preventDefault()
  event.stopPropagation()
  if (event.key === 'Escape') {
    stopRecording()
    return
  }
  if (event.key === 'Backspace') {
    stopRecording()
    emit('update:modelValue', '')
    return
  }
  recordingModifiers.value = hotkeyModifiersFromEvent(event)
  const combination = hotkeyFromEvent(event)
  if (combination) {
    stopRecording()
    emit('update:modelValue', combination)
  }
}

function onKeyUp(event: KeyboardEvent) {
  if (recording.value) recordingModifiers.value = hotkeyModifiersFromEvent(event)
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
    @keyup="onKeyUp"
    @click="startRecording"
    @blur="stopRecording"
  >
    <template v-if="recording">
      <span class="hotkey-input__recording"><Icon name="keyboard" :size="15" /> Удерживайте Ctrl/Alt/Shift/⌘, затем нажмите клавишу</span>
      <span v-if="recordingModifiers.length" class="hotkey-input__keys"><kbd v-for="modifier in recordingModifiers" :key="modifier">{{ modifier }}</kbd></span>
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
  max-width: 100%;
  font-size: 12px;
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
/* подсказка остаётся ниже клавиш, чтобы не перекрывать их в узких колонках */
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
