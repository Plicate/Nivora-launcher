<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { dialogQueue, resolveDialog } from '../services/dialogs'

const current = computed(() => dialogQueue[0])
const inputValue = ref('')
const inputRef = ref<HTMLInputElement | null>(null)
const choiceBoxRef = ref<HTMLElement | null>(null)
const dialogRef = ref<HTMLElement | null>(null)
let returnFocusElement: HTMLElement | null = null

watch(current, async (request, previousRequest) => {
  if (!previousRequest && request && document.activeElement instanceof HTMLElement) {
    returnFocusElement = document.activeElement
  }
  inputValue.value = request?.kind === 'choose'
    ? request.choices?.[0]?.value || ''
    : request?.initialValue || ''
  if (request) {
    await nextTick()
    const target = request.kind === 'prompt'
      ? inputRef.value
      : request.kind === 'choose'
        ? choiceBoxRef.value
        : dialogRef.value?.querySelector<HTMLElement>('.dialog-actions button:last-child')
    target?.focus()
    if (request.kind === 'prompt') inputRef.value?.select()
  } else if (previousRequest) {
    await nextTick()
    if (returnFocusElement?.isConnected) returnFocusElement.focus()
    returnFocusElement = null
  }
}, { immediate: true })

function cancel() {
  if (current.value?.kind === 'alert') resolveDialog(undefined)
  else resolveDialog(current.value?.kind === 'prompt' || current.value?.kind === 'choose' ? undefined : false)
}

function confirm() {
  if (current.value?.kind === 'prompt' || current.value?.kind === 'choose') resolveDialog(inputValue.value)
  else resolveDialog(true)
}

function handleDialogKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    cancel()
    return
  }
  if (event.key !== 'Tab') return

  const focusable = Array.from(dialogRef.value?.querySelectorAll<HTMLElement>(
    'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
  ) || []).filter((element) => element.getAttribute('aria-hidden') !== 'true')
  if (!focusable.length) {
    event.preventDefault()
    dialogRef.value?.focus()
    return
  }

  const first = focusable[0]
  const last = focusable[focusable.length - 1]
  if (event.shiftKey && (document.activeElement === first || !dialogRef.value?.contains(document.activeElement))) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && (document.activeElement === last || !dialogRef.value?.contains(document.activeElement))) {
    event.preventDefault()
    first.focus()
  }
}
</script>

<template>
  <Teleport to="body">
    <Transition name="dialog">
      <div v-if="current" class="dialog-scrim" role="presentation" @click.self="cancel">
        <section ref="dialogRef" class="dialog-card" role="dialog" tabindex="-1" aria-modal="true" :aria-labelledby="`dialog-title-${current.id}`" @keydown="handleDialogKeydown">
          <div class="dialog-icon" :class="{ 'dialog-icon--danger': current.danger }"><Icon :name="current.danger ? 'warning' : current.kind === 'prompt' ? 'edit' : 'info'" :size="18" /></div>
          <div class="dialog-copy">
            <div class="dialog-heading">
              <div><h2 :id="`dialog-title-${current.id}`">{{ current.title || (current.kind === 'confirm' ? 'Подтвердите действие' : current.kind === 'prompt' ? 'Введите значение' : 'Сообщение') }}</h2></div>
              <button class="icon-button" type="button" aria-label="Закрыть" @click="cancel"><Icon name="close" :size="16" /></button>
            </div>
            <p>{{ current.message }}</p>
            <small v-if="current.detail" class="dialog-detail">{{ current.detail }}</small>
            <input v-if="current.kind === 'prompt'" ref="inputRef" v-model="inputValue" class="dialog-input" name="dialog-value" :aria-label="current.title || current.placeholder || 'Значение'" :placeholder="current.placeholder" @keydown.enter.prevent="confirm" />
            <div v-if="current.kind === 'choose'" ref="choiceBoxRef" class="dialog-choices" role="listbox" aria-label="Выберите значение">
              <button
                v-for="choice in current.choices"
                :key="choice.value"
                class="dialog-choice"
                :class="{ 'dialog-choice--active': choice.value === inputValue }"
                type="button"
                role="option"
                :aria-selected="choice.value === inputValue"
                @click="inputValue = choice.value; confirm()"
                @keydown.enter.prevent="confirm()"
              >
                <span class="dialog-choice-mark"><Icon v-if="choice.value === inputValue" name="check" :size="13" /></span>
                <span class="dialog-choice-copy">
                  <strong>{{ choice.label }}</strong>
                  <small v-if="choice.description">{{ choice.description }}</small>
                </span>
              </button>
            </div>
            <div class="dialog-actions">
              <button v-if="current.kind !== 'alert'" class="button button--quiet" type="button" @click="cancel">{{ current.cancelLabel || 'Отмена' }}</button>
              <button class="button" :class="current.danger ? 'button--danger' : 'button--accent'" type="button" @click="confirm">{{ current.confirmLabel || (current.kind === 'alert' ? 'Понятно' : current.kind === 'prompt' ? 'Сохранить' : current.kind === 'choose' ? 'Выбрать' : 'Подтвердить') }}</button>
            </div>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>
