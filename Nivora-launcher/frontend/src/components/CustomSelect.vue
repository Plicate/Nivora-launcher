<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import type { SelectOption } from '../types'

const props = withDefaults(
  defineProps<{
    modelValue: string
    options: SelectOption[]
    placeholder?: string
    disabled?: boolean
    ariaLabel?: string
  }>(),
  { placeholder: 'Выберите значение', disabled: false, ariaLabel: '' },
)

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()
const root = ref<HTMLElement | null>(null)
const menu = ref<HTMLElement | null>(null)
const open = ref(false)
const highlightedIndex = ref(-1)
const menuStyle = ref<Record<string, string>>({})
const menuId = `select-menu-${Math.random().toString(36).slice(2, 10)}`
const selected = computed(() => props.options.find((option) => option.value === props.modelValue))

function choose(option: SelectOption) {
  if (props.disabled) return
  emit('update:modelValue', option.value)
  open.value = false
}

function resetHighlight() {
  highlightedIndex.value = Math.max(0, props.options.findIndex((option) => option.value === props.modelValue))
}

function closeOnOutside(event: MouseEvent) {
  const path = event.composedPath()
  if (path.includes(root.value as EventTarget) || path.includes(menu.value as EventTarget)) return
  open.value = false
}

// Масштаб интерфейса (--ui-scale) вешается на body как CSS `zoom`, а меню
// телепортировано в body и позиционируется `fixed`. Формула «делим на масштаб»
// расходилась с реальным движком (Chromium и WebKitGTK трактуют zoom по-разному),
// поэтому меню калибруется по факту: ставим его в видимых пикселях, измеряем,
// какой коэффициент реально применил движок, и пересчитываем координаты один раз.
function uiScaleGuess(): number {
  const value = Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--ui-scale'))
  return Number.isFinite(value) && value > 0 ? value : 1
}

function visualBox(guess: number) {
  const trigger = root.value?.querySelector('.custom-select-trigger')
  if (!trigger) return null
  const rect = trigger.getBoundingClientRect()
  const viewportPadding = 8
  const estimatedHeight = Math.max(80, props.options.length * 46 + 12)
  const measuredHeight = (menu.value?.scrollHeight ?? 0) * guess
  const naturalHeight = Math.min(Math.max(measuredHeight || estimatedHeight, 80), 260)
  const bottom = rect.bottom
  const spaceBelow = Math.max(0, window.innerHeight - bottom - viewportPadding - 6)
  const spaceAbove = Math.max(0, rect.top - viewportPadding - 6)
  const openAbove = spaceBelow < naturalHeight && spaceAbove > spaceBelow
  const availableHeight = Math.max(80, Math.min(naturalHeight, openAbove ? spaceAbove : spaceBelow))
  const top = openAbove
    ? Math.max(viewportPadding, rect.top - availableHeight - 6)
    : Math.min(window.innerHeight - availableHeight - viewportPadding, bottom + 6)
  const width = Math.min(rect.width, window.innerWidth - viewportPadding * 2)
  const left = Math.min(Math.max(viewportPadding, rect.left), Math.max(viewportPadding, window.innerWidth - width - viewportPadding))
  return { top, left, width, maxHeight: availableHeight, openAbove }
}

function applyBox(box: NonNullable<ReturnType<typeof visualBox>>, zoom: number) {
  const element = root.value
  const theme = element ? getComputedStyle(element) : null
  const css = (name: string) => theme?.getPropertyValue(name).trim() ?? ''
  menuStyle.value = {
    top: `${box.top / zoom}px`,
    left: `${box.left / zoom}px`,
    width: `${box.width / zoom}px`,
    maxHeight: `${box.maxHeight / zoom}px`,
    transformOrigin: box.openAbove ? 'bottom center' : 'top center',
    '--text': css('--text'),
    '--muted': css('--muted'),
    '--panel-raised': css('--panel-raised'),
    '--panel-soft': css('--panel-soft'),
    '--line-strong': css('--line-strong'),
    '--accent': css('--accent'),
    '--blue-bright': css('--blue-bright'),
  }
}

async function updatePosition() {
  if (!open.value || !root.value) return
  await nextTick()
  const guess = uiScaleGuess()
  const box = visualBox(guess)
  if (!box) return
  applyBox(box, guess)
  await nextTick()
  const element = menu.value
  if (!element || box.width <= 0) return
  // Фактический коэффициент: видимая ширина меню делить на заданную в CSS ширину
  const visibleWidth = element.getBoundingClientRect().width
  const actual = guess * visibleWidth / box.width
  if (Number.isFinite(actual) && actual > 0.2 && actual < 5 && Math.abs(actual - guess) > 0.005) applyBox(box, actual)
}

function reposition() {
  if (!open.value) return
  window.requestAnimationFrame(updatePosition)
}

function onKeydown(event: KeyboardEvent) {
  if (props.disabled) return
  if (event.key === 'Escape') {
    open.value = false
    return
  }
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    if (!open.value) {
      open.value = true
      resetHighlight()
      return
    }
    const direction = event.key === 'ArrowDown' ? 1 : -1
    const next = highlightedIndex.value + direction
    highlightedIndex.value = Math.min(Math.max(next, 0), props.options.length - 1)
    return
  }
  if (event.key === 'Home' || event.key === 'End') {
    if (!open.value) return
    event.preventDefault()
    highlightedIndex.value = event.key === 'Home' ? 0 : props.options.length - 1
    return
  }
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    if (open.value && props.options[highlightedIndex.value]) choose(props.options[highlightedIndex.value])
    else {
      open.value = true
      resetHighlight()
    }
  }
}

watch(open, async (value) => {
  if (!value) return
  resetHighlight()
  await nextTick()
  updatePosition()
})

watch(() => props.options.length, () => {
  if (open.value) updatePosition()
})

onMounted(() => {
  document.addEventListener('click', closeOnOutside)
  window.addEventListener('resize', reposition)
  window.addEventListener('scroll', reposition, true)
})

onBeforeUnmount(() => {
  document.removeEventListener('click', closeOnOutside)
  window.removeEventListener('resize', reposition)
  window.removeEventListener('scroll', reposition, true)
})
</script>

<template>
  <div ref="root" class="custom-select" :class="{ 'custom-select--open': open, 'custom-select--disabled': disabled }">
    <button
      class="custom-select-trigger"
      type="button"
      :disabled="disabled"
      :aria-label="ariaLabel || undefined"
      aria-haspopup="listbox"
      :aria-expanded="open"
      :aria-controls="open ? menuId : undefined"
      :aria-activedescendant="open && highlightedIndex >= 0 ? `${menuId}-option-${highlightedIndex}` : undefined"
      @click="open = !open"
      @keydown="onKeydown"
    >
      <span class="custom-select-value">
        <strong>{{ selected?.label || placeholder }}</strong>
        <small v-if="selected?.meta">{{ selected.meta }}</small>
      </span>
      <Icon name="chevron" :size="17" />
    </button>
    <Teleport to="body">
      <Transition name="dropdown">
        <div v-if="open" :id="menuId" ref="menu" class="custom-select-menu" :style="menuStyle" role="listbox">
          <button
            v-for="(option, index) in options"
            :id="`${menuId}-option-${index}`"
            :key="option.value"
            class="custom-select-option"
            :class="{ 'custom-select-option--active': option.value === modelValue, 'custom-select-option--highlighted': index === highlightedIndex }"
            type="button"
            role="option"
            :aria-selected="option.value === modelValue"
            @mouseenter="highlightedIndex = index"
            @click="choose(option)"
          >
            <span class="custom-select-option-copy">
              <strong>{{ option.label }}</strong>
              <small v-if="option.meta">{{ option.meta }}</small>
            </span>
            <span v-if="option.value === modelValue" class="custom-select-check" />
          </button>
          <span v-if="!options.length" class="custom-select-empty">Нет доступных вариантов</span>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>
