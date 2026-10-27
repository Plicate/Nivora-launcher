<script lang="ts">
import { defineComponent, h, reactive, watch, useId, type PropType, type VNodeChild } from 'vue'
import type { PluginJsonValue, PluginUiTone, PluginUiView } from '../../../plugins/api'

type ActionPayload = { action: string; input: PluginJsonValue }
type ConfigPayload = { key: string; value: PluginJsonValue }

const MAX_DEPTH = 10
const MAX_RENDERED_NODES = 512

const toneClasses: Readonly<Record<PluginUiTone, string>> = {
  default: 'plugin-tone--default',
  muted: 'plugin-tone--muted',
  success: 'plugin-tone--success',
  warning: 'plugin-tone--warning',
  danger: 'plugin-tone--danger',
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function requiredString(value: unknown, nonEmpty = true): string | null {
  return typeof value === 'string' && (!nonEmpty || value.trim().length > 0) ? value : null
}

function optionalString(value: unknown, nonEmpty = false): string | undefined | null {
  if (value === undefined) return undefined
  return requiredString(value, nonEmpty)
}

function optionalBoolean(value: unknown): boolean | undefined | null {
  if (value === undefined) return undefined
  return typeof value === 'boolean' ? value : null
}

function toneClass(value: unknown): string | null {
  if (value === undefined) return toneClasses.default
  if (typeof value !== 'string' || !Object.prototype.hasOwnProperty.call(toneClasses, value)) return null
  return toneClasses[value as PluginUiTone]
}

function safeHttpUrl(value: string): string | null {
  if (value !== value.trim() || /[\u0000-\u0020\u007f]/.test(value) || !/^https?:\/\//i.test(value)) return null
  const authority = /^https?:\/\/([^/?#]*)/i.exec(value)?.[1]
  if (!authority || authority.includes('@')) return null

  try {
    const url = new URL(value)
    if ((url.protocol !== 'http:' && url.protocol !== 'https:') || !url.hostname || url.username || url.password) return null
    return url.href
  } catch {
    return null
  }
}

export default defineComponent({
  name: 'PluginViewRenderer',
  props: {
    view: {
      type: Object as PropType<PluginUiView>,
      required: true,
    },
  },
  emits: {
    /** передаёт действие плагина и связанные с ним данные */
    action: (_payload: ActionPayload) => true,
    /** передаёт новое значение поля конфигурации */
    config: (_payload: ConfigPayload) => true,
    /** открывает абсолютную HTTP(S)-ссылку без учётных данных */
    openLink: (_url: string) => true,
  },
  setup(props, { emit }) {
    const values = reactive(new Map<string, string | boolean | number>())
    const previous = new Map<string, PluginJsonValue>()
    watch(() => props.view.root, (root) => {
      const valid = new Set<string>()
      let total = 0
      const visit = (node: unknown, depth: number) => {
        if (!isRecord(node) || depth > MAX_DEPTH || ++total > MAX_RENDERED_NODES) return
        if (typeof node.id === 'string') {
          const id = node.id
          valid.add(id); valid.add('tabs:' + id)
          if (['input', 'select', 'checkbox'].includes(String(node.type))) {
            const incoming: string | boolean = node.type === 'checkbox' ? node.checked === true : typeof node.value === 'string' ? node.value : ''
            if (!previous.has(id) || previous.get(id) !== incoming) values.set(id, incoming)
            previous.set(id, incoming)
          }
        }
        for (const key of ['children', 'fields']) if (Array.isArray(node[key])) for (const child of node[key]) visit(child, depth + 1)
        for (const key of ['tabs', 'sections']) if (Array.isArray(node[key])) for (const entry of node[key]) if (isRecord(entry)) visit(entry.content, depth + 1)
      }
      visit(root, 0)
      for (const id of values.keys()) if (!valid.has(id)) values.delete(id)
      for (const id of previous.keys()) if (!valid.has(id)) previous.delete(id)
    }, { immediate: true, deep: true })

    const titleId = `plugin-view-title-${useId()}`

    function emitControlValue(
      id: string,
      value: string | boolean | number,
      configKey: string | undefined,
      action: string | undefined,
    ) {
      values.set(id, value)
      if (configKey) emit('config', { key: configKey, value })
      if (action) emit('action', { action, input: value })
    }

    return () => {
      try {
        const view = props.view as unknown
        if (!isRecord(view)) return null

        const title = requiredString(view.title)
        const description = optionalString(view.description)
        if (title === null || description === null) return null

        let renderedNodes = 0
        const canAddNode = () => {
          if (renderedNodes >= MAX_RENDERED_NODES) return false
          renderedNodes += 1
          return true
        }

        function renderListItem(item: unknown, index: number): VNodeChild {
          if (typeof item === 'string') {
            return h('li', { class: 'plugin-list-item', key: index }, item)
          }
          if (!isRecord(item)) return null

          const id = requiredString(item.id)
          const label = requiredString(item.label)
          const detail = optionalString(item.detail)
          const badge = optionalString(item.badge)
          const action = optionalString(item.action, true)
          if (id === null || label === null || detail === null || badge === null || action === null) return null

          const copy = h('span', { class: 'plugin-list-item__copy' }, [
            h('span', { class: 'plugin-list-item__label' }, label),
            detail ? h('span', { class: 'plugin-list-item__detail' }, detail) : null,
          ])
          const badgeNode = badge ? h('span', { class: 'plugin-list-item__badge' }, badge) : null
          const contents = [copy, badgeNode]

          return h('li', { class: 'plugin-list-item', key: index }, action
            ? h('button', {
              class: 'plugin-list-action',
              type: 'button',
              onClick: () => emit('action', { action, input: id }),
            }, contents)
            : h('div', { class: 'plugin-list-item__row' }, contents))
        }

        function renderNode(value: unknown, depth: number): VNodeChild {
          if (depth > MAX_DEPTH || !canAddNode()) return null

          try {
            if (!isRecord(value)) return null
            const node = value
            switch (node.type) {
              case 'tabs':
              case 'accordion': {
                const id = requiredString(node.id)
                const entries = node.type === 'tabs' ? node.tabs : node.sections
                if (!id || !Array.isArray(entries) || entries.length > 64) return null
                const valid = entries.filter((entry) => isRecord(entry) && typeof entry.id === 'string' && typeof entry.label === 'string')
                if (node.type === 'accordion') return h('div', { class: 'plugin-accordion' }, valid.map((entry) => h('details', { key: String(entry.id) }, [h('summary', String(entry.label)), renderNode(entry.content, depth + 1)])))
                const selected = String(values.get('tabs:' + id) || valid[0]?.id || '')
                return h('div', { class: 'plugin-tabs' }, [
                  h('div', { role: 'tablist' }, valid.map((entry) => h('button', { type: 'button', role: 'tab', 'aria-selected': selected === entry.id, onClick: () => values.set('tabs:' + id, String(entry.id)) }, String(entry.label)))),
                  renderNode(valid.find((entry) => entry.id === selected)?.content, depth + 1),
                ])
              }
              case 'form': {
                if (!Array.isArray(node.fields) || typeof node.submitAction !== 'string') return null
                const ids = new Set<string>()
                const collect = (fields: unknown[], level: number) => {
                  if (level > MAX_DEPTH) return
                  for (const field of fields.slice(0, MAX_RENDERED_NODES)) if (isRecord(field)) {
                    if (typeof field.id === 'string' && ['input', 'textarea', 'select', 'checkbox', 'switch', 'number', 'slider', 'color', 'datetime'].includes(String(field.type))) ids.add(field.id)
                    if (Array.isArray(field.children)) collect(field.children, level + 1)
                  }
                }
                collect(node.fields, 0)
                return h('form', { class: 'plugin-group', onSubmit: (event: Event) => { event.preventDefault(); const input: Record<string, PluginJsonValue> = Object.create(null); for (const id of ids) input[id] = values.get(id) ?? null; emit('action', { action: String(node.submitAction), input }) } }, [
                  ...node.fields.map((field) => renderNode(field, depth + 1)), h('button', { type: 'submit', class: 'plugin-button' }, 'Отправить'),
                ])
              }
              case 'table': {
                if (!Array.isArray(node.columns) || !Array.isArray(node.rows)) return null
                const columns = node.columns.filter(isRecord).slice(0, 32)
                return h('div', { class: 'plugin-table-scroll' }, [h('table', {}, [h('thead', {}, [h('tr', {}, columns.map((column) => h('th', String(column.label || ''))))]), h('tbody', {}, node.rows.slice(0, 512).filter(isRecord).map((row) => h('tr', {}, columns.map((column) => h('td', String(row[String(column.key)] ?? ''))))))])])
              }
              case 'chart': {
                if (!Array.isArray(node.values) || node.values.length > 512 || node.values.some((item) => typeof item !== 'number' || !Number.isFinite(item))) return null
                const data = node.values as number[], maximum = Math.max(1, ...data), minimum = Math.min(0, ...data)
                const points = data.map((item, index) => ((index / Math.max(1, data.length - 1)) * 100) + ',' + (38 - (item - minimum) / (maximum - minimum) * 34)).join(' ')
                return h('figure', { class: 'plugin-chart' }, [h('figcaption', String(node.label || '')), h('svg', { viewBox: '0 0 100 42', role: 'img', 'aria-label': String(node.label || 'График') }, [h('polyline', { points, fill: 'none', stroke: 'currentColor', 'stroke-width': 1 })])])
              }
              case 'text': {
                const text = requiredString(node.text, false)
                const id = optionalString(node.id)
                const tone = toneClass(node.tone)
                if (text === null || id === null || tone === null) return null
                return h('p', { class: ['plugin-node-text', tone] }, text)
              }
              case 'badge': {
                const label = requiredString(node.label)
                const id = optionalString(node.id)
                const tone = toneClass(node.tone)
                if (label === null || id === null || tone === null) return null
                return h('span', { class: ['plugin-node-badge', tone] }, label)
              }
              case 'button': {
                const id = requiredString(node.id)
                const label = requiredString(node.label)
                const action = requiredString(node.action)
                const disabled = optionalBoolean(node.disabled)
                const tone = toneClass(node.tone)
                if (id === null || label === null || action === null || disabled === null || tone === null) return null
                return h('button', {
                  class: ['plugin-button', tone],
                  type: 'button',
                  disabled: disabled === true,
                  onClick: () => emit('action', { action, input: null }),
                }, label)
              }
              case 'textarea': {
                const id = requiredString(node.id)
                const label = requiredString(node.label)
                const initialValue = optionalString(node.value)
                const placeholder = optionalString(node.placeholder)
                const rows = typeof node.rows === 'number' && Number.isFinite(node.rows) ? Math.min(24, Math.max(1, Math.round(node.rows))) : 4
                const configKey = optionalString(node.config_key, true)
                const action = optionalString(node.onChange ?? node.action, true)
                if (id === null || label === null || initialValue === null || placeholder === null || configKey === null || action === null) return null
                const storedValue = values.get(id)
                const currentValue = typeof storedValue === 'string' ? storedValue : initialValue ?? ''
                return h('label', { class: 'plugin-field' }, [
                  h('span', { class: 'plugin-field__label' }, label),
                  h('textarea', {
                    class: 'plugin-control plugin-textarea',
                    rows,
                    value: currentValue,
                    placeholder,
                    onInput: (event: Event) => {
                      const nextValue = (event.currentTarget as HTMLTextAreaElement).value
                      emitControlValue(id, nextValue, configKey, action)
                    },
                  }, currentValue),
                ])
              }
              case 'number':
              case 'slider': {
                const isSlider = node.type === 'slider'
                const id = requiredString(node.id)
                const label = requiredString(node.label)
                const configKey = optionalString(node.config_key, true)
                const action = optionalString(node.onChange ?? node.action, true)
                const unit = optionalString(node.unit)
                const min = typeof node.min === 'number' && Number.isFinite(node.min) ? node.min : 0
                const max = typeof node.max === 'number' && Number.isFinite(node.max) ? Math.max(min + (isSlider ? 1 : 0), node.max) : isSlider ? 100 : undefined
                const step = typeof node.step === 'number' && Number.isFinite(node.step) && node.step > 0 ? node.step : 1
                const initialNumber = typeof node.value === 'number' && Number.isFinite(node.value) ? node.value : min
                if (id === null || label === null || configKey === null || action === null || unit === null) return null
                const storedValue = values.get(id)
                const currentNumber = typeof storedValue === 'number' && Number.isFinite(storedValue) ? storedValue : initialNumber
                const readout = `${currentNumber}${unit ? ` ${unit}` : ''}`
                const commit = (nextNumber: number) => {
                  const clamped = Math.min(max ?? nextNumber, Math.max(min, nextNumber))
                  emitControlValue(id, clamped, configKey, action)
                }
                const control = isSlider
                  ? h('div', { class: 'plugin-slider' }, [
                      h('input', {
                        type: 'range',
                        min,
                        max: max ?? 100,
                        step,
                        value: currentNumber,
                        onInput: (event: Event) => {
                          const nextNumber = Number((event.currentTarget as HTMLInputElement).value)
                          if (Number.isFinite(nextNumber)) commit(nextNumber)
                        },
                      }),
                      h('output', { class: 'plugin-slider__value' }, readout),
                    ])
                  : h('div', { class: 'plugin-number' }, [
                      h('input', {
                        class: 'plugin-control',
                        type: 'number',
                        min: max === undefined ? undefined : min,
                        max,
                        step,
                        value: currentNumber,
                        onInput: (event: Event) => {
                          const raw = (event.currentTarget as HTMLInputElement).value
                          const nextNumber = Number(raw)
                          if (raw !== '' && Number.isFinite(nextNumber)) commit(nextNumber)
                        },
                      }),
                      unit ? h('span', { class: 'plugin-number__unit' }, unit) : null,
                    ])
                return h('label', { class: 'plugin-field' }, [
                  h('span', { class: 'plugin-field__label' }, label),
                  control,
                ])
              }
              case 'color': {
                const id = requiredString(node.id)
                const label = requiredString(node.label)
                const configKey = optionalString(node.config_key, true)
                const action = optionalString(node.onChange, true)
                const initial = optionalString(node.value)
                if (id === null || label === null || configKey === null || action === null || initial === null) return null
                const storedValue = values.get(id)
                const currentColor = typeof storedValue === 'string' && /^#[0-9A-Fa-f]{6}$/.test(storedValue) ? storedValue : (initial && /^#[0-9A-Fa-f]{6}$/.test(initial) ? initial : '#8b653f')
                return h('label', { class: 'plugin-field plugin-field--row' }, [
                  h('span', { class: 'plugin-field__label' }, label),
                  h('span', { class: 'plugin-color' }, [
                    h('input', {
                      type: 'color',
                      value: currentColor,
                      onInput: (event: Event) => {
                        const nextValue = (event.currentTarget as HTMLInputElement).value
                        emitControlValue(id, nextValue, configKey, action)
                      },
                    }),
                    h('code', {}, currentColor),
                  ]),
                ])
              }
              case 'datetime': {
                const id = requiredString(node.id)
                const label = requiredString(node.label)
                const initialValue = optionalString(node.value)
                const configKey = optionalString(node.config_key, true)
                const action = optionalString(node.onChange ?? node.action, true)
                if (id === null || label === null || initialValue === null || configKey === null || action === null) return null
                const storedValue = values.get(id)
                const currentValue = typeof storedValue === 'string' ? storedValue : initialValue ?? ''
                return h('label', { class: 'plugin-field' }, [
                  h('span', { class: 'plugin-field__label' }, label),
                  // текстовое поле сохраняет единый формат ISO-минут без системного календаря
                  h('input', {
                    class: 'plugin-control',
                    type: 'text',
                    value: currentValue,
                    placeholder: '2026-10-11T19:30',
                    spellcheck: false,
                    autocomplete: 'off',
                    onChange: (event: Event) => {
                      const nextValue = (event.currentTarget as HTMLInputElement).value.trim()
                      emitControlValue(id, nextValue, configKey, action)
                    },
                  }),
                ])
              }
              case 'switch': {
                const id = requiredString(node.id)
                const label = requiredString(node.label)
                const hint = optionalString(node.hint)
                const initialValue = optionalBoolean(node.checked)
                const configKey = optionalString(node.config_key, true)
                const action = optionalString(node.onChange ?? node.action, true)
                if (id === null || label === null || hint === null || initialValue === null || configKey === null || action === null) return null
                const storedValue = values.get(id)
                const checked = typeof storedValue === 'boolean' ? storedValue : initialValue === true
                return h('label', { class: 'plugin-switch' }, [
                  h('input', {
                    class: 'plugin-switch__input',
                    type: 'checkbox',
                    role: 'switch',
                    checked,
                    onChange: (event: Event) => {
                      const nextValue = (event.currentTarget as HTMLInputElement).checked
                      emitControlValue(id, nextValue, configKey, action)
                    },
                  }),
                  h('span', { class: 'plugin-switch__track', 'aria-hidden': 'true' }, h('span', { class: 'plugin-switch__thumb' })),
                  h('span', { class: 'plugin-switch__copy' }, [
                    h('span', { class: 'plugin-switch__label' }, label),
                    hint ? h('small', { class: 'plugin-switch__hint' }, hint) : null,
                  ]),
                ])
              }
              case 'metric': {
                const label = requiredString(node.label)
                const value = requiredString(node.value)
                const unit = optionalString(node.unit)
                const tone = toneClass(node.tone)
                if (label === null || value === null || unit === null || tone === null) return null
                return h('div', { class: ['plugin-metric', tone] }, [
                  h('span', { class: 'plugin-metric__label' }, label),
                  h('strong', { class: 'plugin-metric__value' }, [value, unit ? h('small', {}, unit) : null]),
                ])
              }
              case 'alert': {
                const text = requiredString(node.text, false)
                const title = optionalString(node.title)
                const tone = toneClass(node.tone)
                if (text === null || title === null || tone === null) return null
                return h('div', { class: ['plugin-alert', tone], role: 'note' }, [
                  title ? h('strong', { class: 'plugin-alert__title' }, title) : null,
                  h('span', { class: 'plugin-alert__text' }, text),
                ])
              }
              case 'code': {
                const text = requiredString(node.text, false)
                if (text === null) return null
                return h('pre', { class: 'plugin-code' }, text)
              }
              case 'image': {
                const rawUrl = requiredString(node.url)
                const alt = optionalString(node.alt)
                const height = typeof node.height === 'number' && Number.isFinite(node.height) ? Math.min(720, Math.max(16, Math.round(node.height))) : undefined
                if (rawUrl === null || alt === null) return null
                const url = safeHttpUrl(rawUrl)
                if (!url) return null
                return h('img', { class: 'plugin-image', src: url, alt: alt || '', loading: 'lazy', height })
              }
              case 'select': {
                const id = requiredString(node.id)
                const label = requiredString(node.label)
                const initialValue = optionalString(node.value)
                const configKey = optionalString(node.config_key, true)
                const action = optionalString(node.onChange ?? node.action, true)
                if (id === null || label === null || initialValue === null || configKey === null || action === null || !Array.isArray(node.options)) return null

                const options: Array<{ value: string; label: string; description?: string; disabled: boolean }> = []
                for (const option of node.options) {
                  if (!canAddNode()) break
                  if (!isRecord(option)) return null
                  const optionValue = requiredString(option.value, false)
                  const optionLabel = requiredString(option.label)
                  const disabled = optionalBoolean(option.disabled)
                  const optionDescription = optionalString(option.description)
                  if (optionValue === null || optionLabel === null || disabled === null || optionDescription === null) return null
                  options.push({
                    value: optionValue,
                    label: optionLabel,
                    description: optionDescription || undefined,
                    disabled: disabled === true,
                  })
                }
                if (!options.length) return null

                const hasOption = (value: string) => options.some((option) => option.value === value)
                const storedValue = values.get(id)
                const currentValue = typeof storedValue === 'string' && hasOption(storedValue)
                  ? storedValue
                  : typeof initialValue === 'string' && hasOption(initialValue)
                    ? initialValue
                    : options[0].value

                return h('label', { class: 'plugin-field' }, [
                  h('span', { class: 'plugin-field__label' }, label),
                  h('select', {
                    class: 'plugin-control',
                    value: currentValue,
                    onChange: (event: Event) => {
                      const nextValue = (event.currentTarget as HTMLSelectElement).value
                      const option = options.find((candidate) => candidate.value === nextValue)
                      if (!option || option.disabled) return
                      emitControlValue(id, nextValue, configKey, action)
                    },
                  }, options.map((option, index) => h('option', {
                    key: index,
                    value: option.value,
                    disabled: option.disabled,
                  }, option.description ? `${option.label} - ${option.description}` : option.label))),
                ])
              }
              case 'checkbox': {
                const id = requiredString(node.id)
                const label = requiredString(node.label)
                const initialValue = optionalBoolean(node.checked)
                const configKey = optionalString(node.config_key, true)
                const action = optionalString(node.onChange ?? node.action, true)
                if (id === null || label === null || initialValue === null || configKey === null || action === null) return null
                const storedValue = values.get(id)
                const checked = typeof storedValue === 'boolean' ? storedValue : initialValue === true
                return h('label', { class: 'plugin-checkbox' }, [
                  h('input', {
                    class: 'plugin-checkbox__input',
                    type: 'checkbox',
                    checked,
                    onChange: (event: Event) => {
                      const nextValue = (event.currentTarget as HTMLInputElement).checked
                      emitControlValue(id, nextValue, configKey, action)
                    },
                  }),
                  h('span', { class: 'plugin-checkbox__label' }, label),
                ])
              }
              case 'progress': {
                const id = optionalString(node.id)
                const label = optionalString(node.label, true)
                const value = node.value
                const maximum = node.max === undefined ? 100 : node.max
                if (id === null || label === null || typeof value !== 'number' || !Number.isFinite(value)
                  || typeof maximum !== 'number' || !Number.isFinite(maximum) || maximum <= 0) return null
                return h('div', { class: 'plugin-progress' }, [
                  label ? h('span', { class: 'plugin-progress__label' }, label) : null,
                  h('progress', {
                    class: 'plugin-progress__bar',
                    value: Math.min(maximum, Math.max(0, value)),
                    max: maximum,
                    'aria-label': label ?? 'Progress',
                  }),
                ])
              }
              case 'list': {
                const id = optionalString(node.id)
                if (id === null || !Array.isArray(node.items)) return null
                const items: VNodeChild[] = []
                for (let index = 0; index < node.items.length; index += 1) {
                  if (!canAddNode()) break
                  const item = renderListItem(node.items[index], index)
                  if (item) items.push(item)
                }
                return items.length ? h('ul', { class: 'plugin-list' }, items) : null
              }
              case 'link': {
                const id = optionalString(node.id)
                const label = requiredString(node.label)
                const rawUrl = requiredString(node.url)
                const action = optionalString(node.onChange ?? node.action, true)
                if (id === null || label === null || rawUrl === null || action === null) return null
                const url = safeHttpUrl(rawUrl)
                if (!url) return null
                return h('a', {
                  class: 'plugin-link',
                  href: url,
                  target: '_blank',
                  rel: 'noopener noreferrer',
                  onClick: (event: MouseEvent) => {
                    event.preventDefault()
                    if (action) emit('action', { action, input: url })
                    emit('openLink', url)
                  },
                }, label)
              }
              case 'divider': {
                const id = optionalString(node.id)
                return id === null ? null : h('hr', { class: 'plugin-divider', role: 'separator' })
              }
              case 'group': {
                const id = optionalString(node.id)
                const label = optionalString(node.label, true)
                const direction = optionalString(node.direction)
                if (id === null || label === null || direction === null || !Array.isArray(node.children)) return null
                if (direction !== undefined && direction !== 'vertical' && direction !== 'horizontal') return null

                const children: VNodeChild[] = []
                for (const child of node.children) {
                  if (renderedNodes >= MAX_RENDERED_NODES) break
                  const rendered = renderNode(child, depth + 1)
                  if (rendered) children.push(rendered)
                }
                const classes = ['plugin-group', direction === 'horizontal' ? 'plugin-group--horizontal' : 'plugin-group--vertical']
                if (label) {
                  return h('fieldset', { class: classes }, [
                    h('legend', { class: 'plugin-group__label' }, label),
                    ...children,
                  ])
                }
                return h('div', { class: classes }, children)
              }
              case 'card': {
                const label = optionalString(node.label, true)
                const tone = toneClass(node.tone)
                if (label === null || tone === null || !Array.isArray(node.children)) return null
                const children: VNodeChild[] = []
                for (const child of node.children) {
                  if (renderedNodes >= MAX_RENDERED_NODES) break
                  const rendered = renderNode(child, depth + 1)
                  if (rendered) children.push(rendered)
                }
                return h('section', { class: ['plugin-card', tone] }, [
                  label ? h('header', { class: 'plugin-card__label' }, label) : null,
                  ...children,
                ])
              }
              case 'grid': {
                if (!Array.isArray(node.children)) return null
                const columns = typeof node.columns === 'number' && Number.isInteger(node.columns) ? Math.min(4, Math.max(1, node.columns)) : 2
                const children: VNodeChild[] = []
                for (const child of node.children) {
                  if (renderedNodes >= MAX_RENDERED_NODES) break
                  const rendered = renderNode(child, depth + 1)
                  if (rendered) children.push(rendered)
                }
                return h('div', { class: 'plugin-grid', style: { gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` } }, children)
              }
              default:
                return null
            }
          } catch {
            return null
          }
        }

        const content = renderNode(view.root, 0)
        return h('section', { class: 'plugin-view', 'aria-labelledby': titleId }, [
          h('header', { class: 'plugin-view__header' }, [
            h('h2', { id: titleId, class: 'plugin-view__title' }, title),
            description ? h('p', { class: 'plugin-view__description' }, description) : null,
          ]),
          content,
        ])
      } catch {
        return null
      }
    }
  },
})
</script>

<style scoped>
.plugin-table-scroll { overflow: auto; }
.plugin-table-scroll table { border-collapse: collapse; width: 100%; }
.plugin-table-scroll td, .plugin-table-scroll th { padding: 8px; border: 1px solid var(--line); }
.plugin-tabs [role="tablist"] { display: flex; gap: 8px; margin-bottom: 12px; }
.plugin-accordion details { padding: 12px; border: 1px solid var(--line); }
.plugin-chart svg { width: 100%; max-height: 160px; }

.plugin-view {
  display: grid;
  min-width: 0;
  gap: 14px;
  color: var(--text);
}

.plugin-view__header {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.plugin-view__title {
  margin: 0;
  color: var(--text);
  font-size: 18px;
  font-weight: 600;
  line-height: 1.25;
  overflow-wrap: anywhere;
}

.plugin-view__description {
  margin: 0;
  color: var(--muted);
  font-size: 13px;
  line-height: 1.55;
  overflow-wrap: anywhere;
}

.plugin-node-text {
  min-width: 0;
  margin: 0;
  color: var(--text);
  line-height: 1.55;
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}

.plugin-tone--default {
  color: var(--text);
}

.plugin-tone--muted {
  color: var(--muted);
}

.plugin-tone--success {
  color: var(--green);
}

.plugin-tone--warning {
  color: var(--warning, var(--purple));
}

.plugin-tone--danger {
  color: var(--danger);
}

.plugin-node-badge,
.plugin-list-item__badge {
  display: inline-flex;
  max-width: 100%;
  align-items: center;
  padding: 2px 8px;
  border: 1px solid var(--line);
  border-radius: 999px;
  background: var(--panel-soft);
  font-size: 11px;
  line-height: 1.4;
  overflow-wrap: anywhere;
}

.plugin-button {
  display: inline-flex;
  max-width: 100%;
  min-height: 38px;
  align-items: center;
  justify-content: center;
  padding: 7px 13px;
  border: 1px solid var(--line-strong);
  border-radius: var(--radius-button, 8px);
  color: var(--text);
  background: var(--panel-raised);
  cursor: pointer;
  font: inherit;
  text-align: center;
  overflow-wrap: anywhere;
  transition: border-color .15s ease, background-color .15s ease, color .15s ease;
}

.plugin-button:hover:not(:disabled),
.plugin-list-action:hover {
  border-color: var(--blue);
  background: var(--panel-soft);
}

.plugin-button:disabled {
  cursor: not-allowed;
  opacity: .6;
}

.plugin-field {
  display: grid;
  min-width: 0;
  gap: 6px;
  color: var(--quiet);
  font-size: 13px;
}

.plugin-field__label,
.plugin-checkbox__label {
  overflow-wrap: anywhere;
}

.plugin-control {
  width: 100%;
  min-width: 0;
  min-height: 40px;
  padding: 8px 11px;
  border: 1px solid var(--line);
  border-radius: var(--radius-button, 8px);
  color: var(--text);
  background: var(--panel-raised);
  font: inherit;
}

.plugin-control::placeholder {
  color: var(--muted);
}

.plugin-control:hover,
.plugin-control:focus {
  border-color: var(--blue);
  background: var(--panel-soft);
}

.plugin-checkbox {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 9px;
  color: var(--quiet);
  cursor: pointer;
}

.plugin-checkbox__input {
  flex: 0 0 auto;
  accent-color: var(--blue);
}

.plugin-list {
  display: grid;
  min-width: 0;
  gap: 7px;
  margin: 0;
  padding: 0;
  list-style: none;
}

.plugin-list-item {
  min-width: 0;
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: calc(var(--radius-button, 8px) * 1.125);
  color: var(--text);
  background: var(--panel-raised);
}

.plugin-list-item__row,
.plugin-list-action {
  display: flex;
  width: 100%;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
}

.plugin-list-action {
  border: 0;
  color: var(--text);
  background: var(--panel-raised);
  cursor: pointer;
  font: inherit;
  text-align: left;
}

.plugin-list-item__copy {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.plugin-list-item__label,
.plugin-list-item__detail {
  overflow-wrap: anywhere;
}

.plugin-list-item__detail {
  color: var(--muted);
  font-size: 12px;
}

.plugin-progress {
  display: grid;
  min-width: 0;
  gap: 6px;
}

.plugin-progress__label {
  color: var(--quiet);
  font-size: 12px;
  overflow-wrap: anywhere;
}

.plugin-progress__bar {
  width: 100%;
  height: 9px;
  accent-color: var(--blue);
}

.plugin-link {
  width: fit-content;
  max-width: 100%;
  color: var(--blue-bright);
  text-decoration: underline;
  text-underline-offset: 3px;
  overflow-wrap: anywhere;
}

.plugin-link:hover {
  color: var(--text);
}

.plugin-divider {
  width: 100%;
  margin: 2px 0;
  border: 0;
  border-top: 1px solid var(--line);
}

.plugin-group {
  display: flex;
  min-width: 0;
  flex-direction: column;
  gap: 12px;
  margin: 0;
  padding: 0;
  border: 0;
}

.plugin-group--horizontal {
  flex-direction: row;
  flex-wrap: wrap;
  align-items: flex-end;
}

.plugin-group--horizontal > .plugin-field {
  flex: 1 1 180px;
}

.plugin-group__label {
  padding: 0;
  color: var(--text);
  font-size: 14px;
  font-weight: 600;
  overflow-wrap: anywhere;
}

.plugin-view :focus-visible {
  outline: 2px solid var(--blue-bright);
  outline-offset: 2px;
}

@media (max-width: 540px) {
  .plugin-group--horizontal {
    align-items: stretch;
    flex-direction: column;
  }

  .plugin-group--horizontal > .plugin-field {
    flex-basis: auto;
  }

  .plugin-list-item__row,
  .plugin-list-action {
    align-items: flex-start;
  }
}

/* узлы интерфейса версии 2 */
.plugin-textarea { resize: vertical; min-height: 64px; font-family: inherit; }

.plugin-slider { display: flex; align-items: center; gap: 10px; min-width: 0; }
.plugin-slider input[type="range"] { flex: 1 1 auto; min-width: 0; accent-color: var(--accent); }
.plugin-slider__value { min-width: 64px; text-align: right; font-variant-numeric: tabular-nums; font-size: 12.5px; color: var(--text); }

.plugin-number { display: flex; align-items: center; gap: 8px; min-width: 0; }
.plugin-number input { flex: 1 1 auto; min-width: 0; }
.plugin-number__unit { color: var(--muted); font-size: 12px; white-space: nowrap; }

.plugin-field--row { align-items: center; }
.plugin-color { display: inline-flex; align-items: center; gap: 8px; }
.plugin-color input[type="color"] { width: 34px; height: 26px; padding: 0; border: 1px solid var(--line-strong); border-radius: 6px; background: none; cursor: pointer; }
.plugin-color code { font-family: var(--font-mono, monospace); font-size: 12px; color: var(--muted); }

.plugin-switch { display: flex; align-items: flex-start; gap: 10px; cursor: pointer; }
.plugin-switch__input { position: absolute; opacity: 0; width: 34px; height: 20px; margin: 0; pointer-events: none; }
.plugin-switch__track { position: relative; flex: 0 0 auto; width: 34px; height: 20px; margin-top: 1px; border-radius: 999px; background: var(--line-strong); transition: background .15s ease; }
.plugin-switch__thumb { position: absolute; top: 2px; left: 2px; width: 16px; height: 16px; border-radius: 999px; background: var(--panel-raised); transition: left .15s ease; }
.plugin-switch__input:checked + .plugin-switch__track { background: var(--accent); }
.plugin-switch__input:checked + .plugin-switch__track .plugin-switch__thumb { left: 16px; }
.plugin-switch__input:focus-visible + .plugin-switch__track { outline: 2px solid var(--accent); outline-offset: 2px; }
.plugin-switch__copy { display: grid; gap: 2px; min-width: 0; }
.plugin-switch__label { font-size: 13px; }
.plugin-switch__hint { color: var(--muted); font-size: 11.5px; line-height: 1.45; }

.plugin-metric { display: grid; gap: 4px; padding: 10px 12px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel); min-width: 0; }
.plugin-metric__label { color: var(--muted); font-size: 11px; letter-spacing: .05em; text-transform: uppercase; }
.plugin-metric__value { display: flex; align-items: baseline; gap: 4px; font-size: 19px; font-variant-numeric: tabular-nums; }
.plugin-metric__value small { font-size: 11.5px; color: var(--muted); }

.plugin-alert { display: grid; gap: 4px; padding: 10px 12px; border-left: 3px solid var(--line-strong); border-radius: 8px; background: var(--panel); }
.plugin-alert__title { font-size: 13px; }
.plugin-alert__text { color: var(--muted); font-size: 12.5px; line-height: 1.5; }
.plugin-alert.plugin-tone--success { border-color: var(--success); }
.plugin-alert.plugin-tone--warning { border-color: var(--warning); }
.plugin-alert.plugin-tone--danger { border-color: var(--danger); }

.plugin-code { margin: 0; padding: 10px 12px; border: 1px solid var(--line); border-radius: 8px; background: var(--panel); font-family: var(--font-mono, monospace); font-size: 12px; line-height: 1.55; overflow: auto; max-height: 300px; white-space: pre-wrap; overflow-wrap: anywhere; }

.plugin-image { display: block; max-width: 100%; border-radius: 10px; border: 1px solid var(--line); object-fit: cover; }

.plugin-card { display: grid; gap: 10px; padding: 12px; border: 1px solid var(--line); border-radius: 12px; background: var(--panel); min-width: 0; }
.plugin-card__label { font-size: 12px; letter-spacing: .05em; text-transform: uppercase; color: var(--muted); }

.plugin-grid { display: grid; gap: 10px; min-width: 0; }
@media (max-width: 640px) { .plugin-grid { grid-template-columns: 1fr !important; } }
</style>
