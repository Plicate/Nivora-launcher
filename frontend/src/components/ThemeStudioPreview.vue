<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import Icon from './Icon.vue'
import type { AppearanceSettings } from '../types'
import { LAUNCHER_VERSION_LABEL } from '../version'

const props = withDefaults(defineProps<{
  appearance: AppearanceSettings
  tokens: Record<string, string>
  defaultTokens?: Record<string, string>
  lightTokens?: Record<string, string>
  theme: string
  fontFamily: string
  wallpaperUrl: string
  interactive?: boolean
  selected?: string
  hovered?: string
}>(), { interactive: false, selected: '', hovered: '', defaultTokens: undefined, lightTokens: undefined })

const emit = defineEmits<{
  select: [target: string]
  hover: [target: string | null]
  replay: []
}>()

const navItems = [
  { id: 'instances', label: 'Экземпляры', icon: 'instances' },
  { id: 'accounts', label: 'Аккаунты', icon: 'accounts' },
  { id: 'mods', label: 'Каталог', icon: 'mods' },
  { id: 'modpacks', label: 'Модпаки', icon: 'archive' },
  { id: 'themes', label: 'Студия тем', icon: 'palette' },
] as const

const pageHeaders: Record<string, { eyebrow: string; title: string; action: string }> = {
  instances: { eyebrow: 'Библиотека', title: 'Экземпляры', action: 'Новый экземпляр' },
  accounts: { eyebrow: 'Аккаунты', title: 'Аккаунты Minecraft', action: 'Добавить' },
  mods: { eyebrow: 'Каталог ресурсов', title: 'Моды и ресурсы', action: 'Обновить' },
  modpacks: { eyebrow: 'Модпаки', title: 'Готовые сборки', action: 'Импорт' },
  themes: { eyebrow: 'Студия оформления', title: 'Центр настройки тем', action: 'Сохранить' },
}

const tabs = [
  { id: 'all', label: 'Все' },
  { id: 'survival', label: 'Survival' },
  { id: 'servers', label: 'Серверы' },
] as const

const clientRows = [
  { title: 'Survival', meta: 'Fabric · 1.20.1 · 12 ч 42 мин' },
  { title: 'Creative+', meta: 'Quilt · 1.20.4 · 3 ч 05 мин' },
]
const serverRow = { title: 'SMP Server', meta: 'Paper · 1.21 · онлайн' }

const modRows = [
  { title: 'Sodium', meta: 'Мод · 1.20.1 · Fabric · 4.6M' },
  { title: 'Complementary Shaders', meta: 'Шейдер · 1.20+ · 1.1M' },
]

const modpackRows = [
  { title: 'Fabulously Optimized', meta: 'Modrinth · 1.20.4 · Fabric' },
  { title: 'Better MC', meta: 'CurseForge · 1.19.2 · Forge' },
]

const accountRows = [
  { name: 'Steve', meta: 'Microsoft · по умолчанию' },
  { name: 'Alex', meta: 'Offline' },
]

const activeNav = ref('instances')
const activeTab = ref<'all' | 'survival' | 'servers'>('all')
const replayKey = ref(0)

const header = computed(() => pageHeaders[activeNav.value] || pageHeaders.instances)

const visibleRows = computed(() => {
  if (activeTab.value === 'servers') return [serverRow]
  if (activeTab.value === 'survival') return clientRows
  return [...clientRows, serverRow]
})

const pageTheme = computed(() => {
  const override = props.appearance.page_themes[activeNav.value as keyof typeof props.appearance.page_themes]
  if (!override || override === 'inherit') return props.theme === 'light' ? 'light' : props.theme
  return override
})

const activeTokens = computed<Record<string, string>>(() => {
  const dark = props.defaultTokens
  const base = props.lightTokens
  if (!dark || !base) return props.tokens
  const user = props.appearance.color_tokens
  const resolved: Record<string, string> = {}
  for (const key of Object.keys(dark)) {
    resolved[key] = user[key] === dark[key] || !user[key] ? (pageTheme.value === 'light' ? base[key] : dark[key]) : user[key]
  }
  return resolved
})

const token = (key: string) => activeTokens.value[key] || props.tokens[key] || '#ffffff'

const pageLayout = computed(() => props.appearance.page_layouts[activeNav.value as keyof typeof props.appearance.page_layouts] || 'balanced')

const pageStyleOverrides = computed(() => props.appearance.animation_settings.page_styles)
const animationStyleId = computed(() => pageStyleOverrides.value[activeNav.value as keyof typeof pageStyleOverrides.value] || props.appearance.animation_settings.page_style)
const animationDurationMs = computed(() => pageStyleOverrides.value && pageStyleOverrides.value[activeNav.value as keyof typeof pageStyleOverrides.value]
  ? props.appearance.animation_settings.page_durations_ms[activeNav.value as keyof typeof props.appearance.animation_settings.page_durations_ms] || props.appearance.animation_settings.page_duration_ms
  : props.appearance.animation_settings.page_duration_ms)

const animationClass = computed(() => {
  if (props.appearance.animations === 'off') return 'preview-page--none'
  return `preview-page--${animationStyleId.value}`
})

const animationStyle = computed(() => ({ animationDuration: `${Math.max(0, animationDurationMs.value)}ms` }))

watch([activeNav, animationStyleId, () => props.appearance.animations], () => {
  replayKey.value += 1
})

const compactSidebar = computed(() => props.appearance.sidebar_mode === 'compact')
const horizontalSidebar = computed(() => props.appearance.sidebar_position === 'top' || props.appearance.sidebar_position === 'bottom')
const hoverSidebar = computed(() => props.appearance.sidebar_visibility === 'hover')

const fontStack = computed(() => {
  if (props.fontFamily === 'inter') return 'Inter, system-ui, sans-serif'
  if (props.fontFamily === 'manrope') return 'Manrope, system-ui, sans-serif'
  if (props.fontFamily === 'mono') return '\'DM Mono\', ui-monospace, monospace'
  return 'system-ui, -apple-system, BlinkMacSystemFont, \'Segoe UI\', sans-serif'
})

const density = computed(() => {
  if (props.appearance.density === 'compact') return 0.8
  if (props.appearance.density === 'spacious') return 1.25
  return 1
})

const sidebarWidthCss = computed(() => {
  if (horizontalSidebar.value || compactSidebar.value) return ''
  const mode = props.appearance.sidebar_width_mode
  const width = Math.max(168, Math.min(360, Number(props.appearance.sidebar_width) || 184))
  const factor = mode === 'manual' ? width / 184 : mode === 'content' ? 0.92 : 1.08
  return `${Math.round(27 * factor)}%`
})

function accentForeground(color: string): string {
  const clean = color.trim().replace('#', '')
  if (clean.length !== 6) return '#ffffff'
  const red = Number.parseInt(clean.slice(0, 2), 16) || 0
  const green = Number.parseInt(clean.slice(2, 4), 16) || 0
  const blue = Number.parseInt(clean.slice(4, 6), 16) || 0
  return (red * 299 + green * 587 + blue * 114) / 1000 > 154 ? '#080808' : '#ffffff'
}

const radius = computed(() => ({
  card: `${props.appearance.radius_tokens.card}px`,
  button: `${props.appearance.radius_tokens.button}px`,
  control: `${props.appearance.radius_tokens.control}px`,
}))

const borderColor = (strength = 24) => `color-mix(in srgb, ${token('border')} ${strength}%, transparent)`

// glass-тема: панели полупрозрачны, обои под ними видно насквозь
const panelColor = (key: 'panel' | 'panel_raised' | 'panel_soft', alpha: number) => pageTheme.value === 'glass' && props.wallpaperUrl
  ? `color-mix(in srgb, ${token(key)} ${alpha}%, transparent)`
  : token(key)

const rootStyle = computed(() => ({
  background: token('background'),
  color: token('text'),
  fontFamily: fontStack.value,
  '--preview-accent': token('accent'),
  '--preview-accent-strong': token('accent_strong'),
  '--preview-accent-foreground': accentForeground(token('accent')),
  '--preview-panel': panelColor('panel', 62),
  '--preview-panel-raised': panelColor('panel_raised', 74),
  '--preview-panel-soft': panelColor('panel_soft', 86),
  '--preview-text': token('text'),
  '--preview-muted': token('muted'),
  '--preview-quiet': token('quiet'),
  '--preview-border': borderColor(32),
  '--preview-success': token('success'),
  '--preview-radius-card': radius.value.card,
  '--preview-radius-button': radius.value.button,
  '--preview-radius-control': radius.value.control,
  '--preview-gap': `${Math.round(11 * density.value)}px`,
  '--preview-row-pad': `${Math.round(9 * density.value)}px ${Math.round(11 * density.value)}px`,
  '--preview-page-pad': pageLayout.value === 'compact' ? '8px 10px' : pageLayout.value === 'wide' ? '18px 22px' : '12px 14px',
  '--preview-scale': String(Math.max(0.8, Math.min(1.25, props.appearance.ui_scale / 100))),
}))

const sidebarStyle = computed(() => {
  const base: Record<string, string> = {
    background: `color-mix(in srgb, ${panelColor('panel', 72)} 92%, ${token('background')})`,
    border: `1px solid ${borderColor(24)}`,
  }
  if (sidebarWidthCss.value) base.width = sidebarWidthCss.value
  if (props.appearance.sidebar_floating) {
    base.borderRadius = radius.value.card
    if (props.appearance.sidebar_position === 'bottom') base.margin = '0 10px 10px'
    else if (horizontalSidebar.value) base.margin = '10px 10px 0'
    else base.margin = '10px'
  }
  return base
})

const wallpaperStyle = computed(() => ({
  backgroundImage: props.wallpaperUrl ? `url(${props.wallpaperUrl})` : 'none',
  opacity: `${Math.min(0.9, props.appearance.wallpaper_opacity / 100)}`,
}))

const bannerStyle = computed(() => ({
  background: token('accent'),
}))

function targetClass(id: string) {
  if (!props.interactive) return undefined
  return {
    'preview-target': true,
    'preview-target--selected': props.selected === id,
    'preview-target--hovered': props.hovered === id && props.selected !== id,
  }
}

function pick(id: string) {
  if (!props.interactive) return
  emit('select', props.selected === id ? '' : id)
}

function pickNav(id: string) {
  if (!props.interactive) return
  activeNav.value = id
  emit('select', props.selected === 'nav-item' ? '' : 'nav-item')
}

function pickBackground() {
  if (!props.interactive) return
  emit('select', props.selected === 'background' ? '' : 'background')
}

function hover(id: string | null) {
  if (!props.interactive) return
  emit('hover', id)
}

function replay() {
  replayKey.value += 1
  emit('replay')
}
</script>

<template>
  <div class="studio-preview" :style="rootStyle">
    <div class="studio-preview-caption">
      <Icon name="eye" :size="13" :stroke-width="1.8" />
      <strong>Предпросмотр оформления</strong>
      <small>кликните элемент, чтобы настроить его</small>
    </div>
    <div
      class="studio-preview-frame"
      :class="{ 'studio-preview-frame--interactive': interactive }"
      @click="pickBackground"
    >
      <div class="studio-preview-chrome" aria-hidden="true">
        <i /><i /><i />
        <span>Nivora Launcher — предпросмотр</span>
      </div>
      <div v-if="wallpaperUrl" class="studio-preview-wallpaper" :style="wallpaperStyle" />
      <div class="studio-preview-scale">
      <div class="studio-preview-shell" :class="{ 'studio-preview-shell--horizontal': horizontalSidebar }">
        <aside
          class="studio-preview-sidebar"
          :class="[
            targetClass('sidebar'),
            {
              'studio-preview-sidebar--compact': compactSidebar,
              'studio-preview-sidebar--right': appearance.sidebar_position === 'right',
              'studio-preview-sidebar--bar': horizontalSidebar,
              'studio-preview-sidebar--bar-bottom': appearance.sidebar_position === 'bottom',
              'studio-preview-sidebar--hover': hoverSidebar,
            },
          ]"
          :style="sidebarStyle"
          @click.stop="pick('sidebar')"
          @pointerenter="hover('sidebar')"
          @pointerleave="hover(null)"
        >
          <div class="studio-preview-brand">
            <span class="studio-preview-logo" :style="{ background: pageTheme === 'light' ? '#111214' : '#f4f7ff' }" />
            <span v-if="!compactSidebar" class="studio-preview-brand-copy"><strong>Nivora</strong><small>Launcher</small></span>
          </div>
          <button
            v-for="item in navItems"
            :key="item.id"
            type="button"
            class="studio-preview-nav"
            :class="[targetClass('nav-item'), { 'studio-preview-nav--active': activeNav === item.id }]"
            v-tooltip="item.label"
            :style="{ borderRadius: radius.button }"
            @click.stop="pickNav(item.id)"
            @pointerenter="hover('nav-item')"
            @pointerleave="hover(null)"
          >
            <Icon :name="item.icon" :size="13" :stroke-width="1.7" />
            <span v-if="!compactSidebar">{{ item.label }}</span>
          </button>
          <div class="studio-preview-spacer" />
          <div
            class="studio-preview-account"
            :class="targetClass('account-chip')"
            :style="{ borderRadius: radius.control }"
            @click.stop="pick('account-chip')"
            @pointerenter="hover('account-chip')"
            @pointerleave="hover(null)"
          >
            <span class="studio-preview-avatar" :style="{ background: token('accent'), color: accentForeground(token('accent')) }">S</span>
            <span v-if="!compactSidebar" class="studio-preview-account-copy"><strong>Steve</strong><small>Microsoft</small></span>
          </div>
        </aside>

        <div class="studio-preview-main">
          <div :key="replayKey" class="studio-preview-page" :class="animationClass" :style="animationStyle">
            <header class="studio-preview-header">
              <div class="studio-preview-header-copy">
                <span class="studio-preview-eyebrow">{{ header.eyebrow }}</span>
                <strong
                  :class="targetClass('text')"
                  @click.stop="pick('text')"
                  @pointerenter="hover('text')"
                  @pointerleave="hover(null)"
                >{{ header.title }}</strong>
              </div>
              <div class="studio-preview-header-actions">
                <span
                  class="studio-preview-button studio-preview-button--quiet"
                  :class="targetClass('card-button')"
                  :style="{ borderRadius: radius.button }"
                  @click.stop="pick('card-button')"
                  @pointerenter="hover('card-button')"
                  @pointerleave="hover(null)"
                ><Icon name="upload" :size="11" />Импорт</span>
                <span
                  class="studio-preview-button studio-preview-button--accent"
                  :class="targetClass('topbar-button')"
                  :style="{ borderRadius: radius.button, background: token('accent'), color: accentForeground(token('accent')) }"
                  @click.stop="pick('topbar-button')"
                  @pointerenter="hover('topbar-button')"
                  @pointerleave="hover(null)"
                ><Icon name="plus" :size="11" />{{ header.action }}</span>
              </div>
            </header>

            <template v-if="activeNav === 'instances'">
              <div class="studio-preview-tabs">
                <button
                  v-for="tab in tabs"
                  :key="tab.id"
                  type="button"
                  class="studio-preview-tab"
                  :class="{ 'studio-preview-tab--active': activeTab === tab.id }"
                  :style="{ borderRadius: radius.button }"
                  @click.stop="activeTab = tab.id"
                >{{ tab.label }}</button>
              </div>
              <div class="studio-preview-rows">
                <article
                  v-for="(row, index) in visibleRows"
                  :key="row.title"
                  class="studio-preview-row"
                  :class="targetClass(index === 0 ? 'hero-card' : 'grid-card')"
                  :style="{ borderRadius: radius.card, padding: 'var(--preview-row-pad)', background: index === 0 ? panelColor('panel', 68) : panelColor('panel_raised', 80), border: `1px solid ${borderColor(index === 0 ? 24 : 18)}` }"
                  @click.stop="pick(index === 0 ? 'hero-card' : 'grid-card')"
                  @pointerenter="hover(index === 0 ? 'hero-card' : 'grid-card')"
                  @pointerleave="hover(null)"
                >
                  <span
                    class="studio-preview-mark"
                    :class="targetClass('hero-banner')"
                    :style="[bannerStyle, { borderRadius: radius.control }]"
                    @click.stop="pick('hero-banner')"
                    @pointerenter="hover('hero-banner')"
                    @pointerleave="hover(null)"
                  ><Icon name="instances" :size="13" /></span>
                  <span class="studio-preview-row-copy">
                    <strong>{{ row.title }}</strong>
                    <small>{{ row.meta }}</small>
                  </span>
                  <span class="studio-preview-row-actions">
                    <span
                      class="studio-preview-button studio-preview-button--quiet"
                      :class="targetClass('card-button')"
                      :style="{ borderRadius: radius.button }"
                      @click.stop="pick('card-button')"
                      @pointerenter="hover('card-button')"
                      @pointerleave="hover(null)"
                    ><Icon name="settings" :size="11" /></span>
                    <span
                      class="studio-preview-button studio-preview-button--accent"
                      :class="targetClass('hero-play')"
                      :style="{ borderRadius: radius.button, background: token('accent'), color: accentForeground(token('accent')) }"
                      @click.stop="pick('hero-play')"
                      @pointerenter="hover('hero-play')"
                      @pointerleave="hover(null)"
                    ><Icon name="play" :size="11" />Играть</span>
                  </span>
                </article>
              </div>
            </template>

            <template v-else-if="activeNav === 'accounts'">
              <div class="studio-preview-rows">
                <article
                  v-for="account in accountRows"
                  :key="account.name"
                  class="studio-preview-row"
                  :class="targetClass('grid-card')"
                  :style="{ borderRadius: radius.card, padding: 'var(--preview-row-pad)', background: panelColor('panel_raised', 80), border: `1px solid ${borderColor(18)}` }"
                  @click.stop="pick('grid-card')"
                  @pointerenter="hover('grid-card')"
                  @pointerleave="hover(null)"
                >
                  <span class="studio-preview-avatar studio-preview-avatar--lg" :style="{ background: token('accent'), color: accentForeground(token('accent')) }">{{ account.name[0] }}</span>
                  <span class="studio-preview-row-copy"><strong>{{ account.name }}</strong><small>{{ account.meta }}</small></span>
                  <span class="studio-preview-badge" :style="{ borderRadius: radius.button }">{{ account.name === 'Steve' ? 'По умолчанию' : 'Офлайн' }}</span>
                </article>
              </div>
            </template>

            <template v-else-if="activeNav === 'mods'">
              <div
                class="studio-preview-search"
                :class="targetClass('search-field')"
                :style="{ borderRadius: radius.control }"
                @click.stop="pick('search-field')"
                @pointerenter="hover('search-field')"
                @pointerleave="hover(null)"
              ><Icon name="search" :size="12" /><small>Поиск по Modrinth…</small></div>
              <div class="studio-preview-rows">
                <article
                  v-for="entry in modRows"
                  :key="entry.title"
                  class="studio-preview-row"
                  :class="targetClass('grid-card')"
                  :style="{ borderRadius: radius.card, padding: 'var(--preview-row-pad)', background: panelColor('panel_raised', 80), border: `1px solid ${borderColor(18)}` }"
                  @click.stop="pick('grid-card')"
                  @pointerenter="hover('grid-card')"
                  @pointerleave="hover(null)"
                >
                  <span class="studio-preview-mark studio-preview-mark--soft" :style="{ borderRadius: radius.control, background: `color-mix(in srgb, ${token('accent')} 26%, ${token('panel_soft')})` }"><Icon name="mods" :size="13" /></span>
                  <span class="studio-preview-row-copy"><strong>{{ entry.title }}</strong><small>{{ entry.meta }}</small></span>
                  <span
                    class="studio-preview-button studio-preview-button--accent"
                    :class="targetClass('card-button')"
                    :style="{ borderRadius: radius.button }"
                    @click.stop="pick('card-button')"
                    @pointerenter="hover('card-button')"
                    @pointerleave="hover(null)"
                  ><Icon name="download" :size="11" />Установить</span>
                </article>
              </div>
            </template>

            <template v-else-if="activeNav === 'modpacks'">
              <div class="studio-preview-cards">
                <article
                  v-for="pack in modpackRows"
                  :key="pack.title"
                  class="studio-preview-pack"
                  :class="targetClass('grid-card')"
                  :style="{ borderRadius: radius.card, background: panelColor('panel_raised', 80), border: `1px solid ${borderColor(18)}` }"
                  @click.stop="pick('grid-card')"
                  @pointerenter="hover('grid-card')"
                  @pointerleave="hover(null)"
                >
                  <span class="studio-preview-pack-banner" :style="bannerStyle" />
                  <span class="studio-preview-row-copy"><strong>{{ pack.title }}</strong><small>{{ pack.meta }}</small></span>
                  <span
                    class="studio-preview-button studio-preview-button--accent"
                    :class="targetClass('card-button')"
                    :style="{ borderRadius: radius.button, background: token('accent'), color: accentForeground(token('accent')) }"
                    @click.stop="pick('card-button')"
                    @pointerenter="hover('card-button')"
                    @pointerleave="hover(null)"
                  ><Icon name="download" :size="11" />Установить</span>
                </article>
              </div>
            </template>

            <template v-else>
              <div
                class="studio-preview-studio"
                :class="targetClass('grid-card')"
                :style="{ borderRadius: radius.card, background: panelColor('panel_raised', 80), border: `1px solid ${borderColor(18)}` }"
                @click.stop="pick('grid-card')"
                @pointerenter="hover('grid-card')"
                @pointerleave="hover(null)"
              >
                <div class="studio-preview-swatches">
                  <span v-for="color in [token('accent'), token('panel'), token('panel_soft'), token('text'), token('success')]" :key="color" :style="{ background: color, borderRadius: radius.control }" />
                </div>
                <div class="studio-preview-sliders">
                  <span v-for="bar in 2" :key="bar" :style="{ borderRadius: radius.control }"><i :style="{ width: bar === 1 ? '70%' : '40%', background: token('accent') }" /></span>
                </div>
              </div>
            </template>

            <footer
              class="studio-preview-status"
              :class="targetClass('status-chip')"
              :style="{ borderRadius: radius.control }"
              @click.stop="pick('status-chip')"
              @pointerenter="hover('status-chip')"
              @pointerleave="hover(null)"
            >
              <span class="studio-preview-dot" :style="{ background: token('success') }" />
              <small>Синхронизировано · Nivora {{ LAUNCHER_VERSION_LABEL }}</small>
            </footer>
          </div>
        </div>
      </div>
      </div>
    </div>
    <div class="studio-preview-bar">
      <button class="studio-preview-replay" type="button" @click="replay"><Icon name="refresh" :size="12" />Проиграть анимацию</button>
      <small>Страница «{{ navItems.find((item) => item.id === activeNav)?.label || activeNav }}» · {{ animationStyleId }} · {{ animationDurationMs }} ms</small>
    </div>
  </div>
</template>

<style scoped>
.studio-preview { display: grid; gap: 9px; }
.studio-preview-caption { display: flex; align-items: center; gap: 8px; padding: 0 2px 8px; color: var(--muted); font-size: 11px; }
.studio-preview-caption strong { color: var(--text); font-size: 11.5px; letter-spacing: .02em; }
.studio-preview-caption small { margin-left: auto; font-size: 10px; opacity: .85; }
.studio-preview-frame { position: relative; overflow: hidden; border: 1px solid var(--line-strong); border-radius: max(calc(var(--radius-card, 7px) * 2), 12px); aspect-ratio: 16 / 11; background: var(--panel-soft); }
.studio-preview-chrome { position: absolute; inset: 0 0 auto 0; z-index: 5; display: flex; align-items: center; gap: 6px; height: 22px; padding: 0 10px; border-bottom: 1px solid color-mix(in srgb, var(--line) 80%, transparent); background: color-mix(in srgb, var(--panel) 92%, transparent); font-size: 9px; font-weight: 600; letter-spacing: .06em; color: var(--muted); text-transform: uppercase; pointer-events: none; }
.studio-preview-chrome i { width: 7px; height: 7px; border-radius: 50%; background: color-mix(in srgb, var(--muted) 45%, transparent); }
.studio-preview-chrome span { margin-left: 4px; }
.studio-preview-frame--interactive { cursor: default; }
.studio-preview-wallpaper { position: absolute; inset: -16px; background-size: cover; background-position: center; pointer-events: none; }
.studio-preview-scale { position: absolute; inset: 22px 0 0 0; zoom: var(--preview-scale, 1); }
.studio-preview-shell { position: relative; z-index: 1; display: flex; width: 100%; height: 100%; }
.studio-preview-shell--horizontal { flex-direction: column; }
.studio-preview-sidebar { display: flex; z-index: 2; width: 27%; min-width: 0; flex-direction: column; gap: 3px; padding: 12px 9px; overflow: hidden; }
.studio-preview-sidebar--compact { width: 50px; flex: 0 0 50px; align-items: center; }
.studio-preview-sidebar--right { order: 2; }
.studio-preview-sidebar--bar { width: 100%; flex-direction: row; align-items: center; }
.studio-preview-sidebar--bar-bottom { order: 2; }
.studio-preview-sidebar--bar.studio-preview-sidebar--compact { width: auto; flex: 0 0 auto; }
.studio-preview-sidebar--hover { opacity: .45; transition: opacity .15s ease; }
.studio-preview-sidebar--hover:hover { opacity: 1; }
.studio-preview-brand { display: flex; align-items: center; gap: 7px; padding: 2px 6px 9px; }
.studio-preview-logo { width: 18px; height: 18px; flex: 0 0 18px; border-radius: calc(var(--radius-card, 7px) * .857); }
.studio-preview-brand-copy { display: grid; line-height: 1.15; }
.studio-preview-brand-copy strong { font-size: 12px; letter-spacing: -.02em; color: var(--preview-text); }
.studio-preview-brand-copy small { font-size: 9px; color: var(--preview-muted); }
.studio-preview-nav { display: flex; align-items: center; gap: 8px; width: 100%; padding: 6px 8px; border: 0; color: var(--preview-quiet); background: transparent; font: inherit; font-size: 10.5px; text-align: left; cursor: pointer; }
.studio-preview-nav:hover { color: var(--preview-text); background: color-mix(in srgb, var(--preview-text) 7%, transparent); }
.studio-preview-nav--active { color: var(--preview-text); background: color-mix(in srgb, var(--preview-text) 9%, transparent); }
.studio-preview-sidebar--compact .studio-preview-nav { justify-content: center; padding: 7px 0; }
.studio-preview-sidebar--bar .studio-preview-brand { padding: 0 10px 0 4px; }
.studio-preview-sidebar--bar .studio-preview-nav { width: auto; padding: 6px 9px; }
.studio-preview-spacer { flex: 1; }
.studio-preview-account { display: flex; align-items: center; gap: 7px; padding: 6px; background: color-mix(in srgb, var(--preview-text) 6%, transparent); }
.studio-preview-avatar { display: grid; width: 20px; height: 20px; flex: 0 0 20px; place-items: center; border-radius: 50%; font-size: 10px; font-weight: 700; }
.studio-preview-avatar--lg { width: 26px; height: 26px; flex: 0 0 26px; font-size: 12px; }
.studio-preview-account-copy { display: grid; line-height: 1.15; }
.studio-preview-account-copy strong { font-size: 10px; color: var(--preview-text); }
.studio-preview-account-copy small { font-size: 8.5px; color: var(--preview-muted); }
.studio-preview-main { display: flex; z-index: 1; min-width: 0; flex: 1; flex-direction: column; padding: var(--preview-page-pad, 12px 14px); overflow: hidden; }
.studio-preview-page { display: flex; min-height: 0; flex: 1; flex-direction: column; gap: var(--preview-gap); }
.studio-preview-header { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.studio-preview-header-copy { display: grid; min-width: 0; gap: 2px; }
.studio-preview-eyebrow { color: var(--preview-muted); font-size: 9px; font-weight: 600; }
.studio-preview-header-copy strong { font-size: 13px; letter-spacing: -.02em; color: var(--preview-text); }
.studio-preview-header-actions { display: flex; flex: 0 0 auto; align-items: center; gap: 6px; }
.studio-preview-tabs { display: flex; gap: 5px; }
.studio-preview-tab { padding: 4px 11px; border: 1px solid var(--preview-border); color: var(--preview-muted); background: transparent; font: inherit; font-size: 9.5px; font-weight: 600; cursor: pointer; }
.studio-preview-tab:hover { color: var(--preview-text); background: color-mix(in srgb, var(--preview-text) 6%, transparent); }
.studio-preview-tab--active { color: var(--preview-text); border-color: transparent; background: color-mix(in srgb, var(--preview-text) 10%, transparent); }
.studio-preview-rows { display: grid; min-height: 0; flex: 1; align-content: start; gap: calc(var(--preview-gap, 8px) * .8); overflow: hidden; }
.studio-preview-row { display: flex; align-items: center; gap: 10px; min-width: 0; }
.studio-preview-mark { display: grid; width: 30px; height: 30px; flex: 0 0 30px; place-items: center; color: #fff; }
.studio-preview-mark--soft { color: var(--preview-text); }
.studio-preview-row-copy { display: grid; min-width: 0; flex: 1; gap: 2px; }
.studio-preview-row-copy strong { overflow: hidden; font-size: 11px; letter-spacing: -.01em; color: var(--preview-text); text-overflow: ellipsis; white-space: nowrap; }
.studio-preview-row-copy small { overflow: hidden; color: var(--preview-muted); font-size: 9px; text-overflow: ellipsis; white-space: nowrap; }
.studio-preview-row-actions { display: flex; flex: 0 0 auto; align-items: center; gap: 5px; }
.studio-preview-button { display: inline-flex; align-items: center; gap: 4px; padding: 5px 9px; font-size: 9.5px; font-weight: 600; }
.studio-preview-button--accent { color: var(--preview-accent-foreground, #fff); }
.studio-preview-button--quiet { border: 1px solid var(--preview-border); color: var(--preview-quiet); background: color-mix(in srgb, var(--preview-text) 4%, transparent); }
.studio-preview-badge { padding: 3px 8px; border: 1px solid var(--preview-border); color: var(--preview-muted); font-size: 8.5px; }
.studio-preview-search { display: flex; align-items: center; gap: 7px; padding: 6px 10px; border: 1px solid var(--preview-border); color: var(--preview-muted); font-size: 9.5px; background: color-mix(in srgb, var(--preview-panel-raised) 70%, transparent); }
.studio-preview-cards { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); min-height: 0; flex: 1; align-content: start; gap: 8px; overflow: hidden; }
.studio-preview-pack { display: grid; gap: 7px; padding: 10px; min-width: 0; }
.studio-preview-pack-banner { height: 34px; }
.studio-preview-studio { display: grid; gap: 9px; padding: 11px; }
.studio-preview-swatches { display: flex; gap: 6px; }
.studio-preview-swatches span { width: 22px; height: 22px; flex: 0 0 22px; border: 1px solid var(--preview-border); }
.studio-preview-sliders { display: grid; gap: 6px; }
.studio-preview-sliders span { display: block; height: 5px; background: color-mix(in srgb, var(--preview-text) 10%, transparent); }
.studio-preview-sliders i { display: block; height: 100%; }
.studio-preview-status { display: flex; align-items: center; gap: 7px; align-self: flex-start; padding: 5px 10px; background: color-mix(in srgb, var(--preview-text) 6%, transparent); color: var(--preview-muted); font-size: 9px; }
.studio-preview-dot { width: 6px; height: 6px; border-radius: 50%; }
.studio-preview-bar { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.studio-preview-replay { display: inline-flex; align-items: center; gap: 5px; padding: 4px 10px; border: 1px solid var(--line); border-radius: var(--radius-button, 8px); color: var(--muted); background: transparent; font: inherit; font-size: 10px; cursor: pointer; }
.studio-preview-replay:hover { color: var(--text); background: var(--panel-soft); }
.studio-preview-bar small { overflow: hidden; color: var(--muted); font-size: 9.5px; text-overflow: ellipsis; white-space: nowrap; }
.preview-page--fade { animation: preview-fade ease both; }
.preview-page--slide { animation: preview-slide ease both; }
.preview-page--scale { animation: preview-scale ease both; }
.preview-page--none { animation: none; }
@keyframes preview-fade { from { opacity: 0; } to { opacity: 1; } }
@keyframes preview-slide { from { opacity: 0; transform: translateY(10px); } to { opacity: 1; transform: none; } }
@keyframes preview-scale { from { opacity: 0; transform: scale(.97); } to { opacity: 1; transform: none; } }

.preview-target { cursor: pointer; }
.preview-target--hovered { outline: 1.5px dashed color-mix(in srgb, var(--accent, var(--blue)) 72%, transparent); outline-offset: 2px; }
.preview-target--selected { outline: 2px solid var(--accent, var(--blue)); outline-offset: 2px; }
.preview-target--selected.preview-target--hovered { outline: 2px solid var(--accent-strong, var(--blue-bright, var(--accent, var(--blue)))); }
</style>
