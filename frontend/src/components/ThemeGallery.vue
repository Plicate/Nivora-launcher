<script setup lang="ts">
import { computed, ref } from 'vue'
import { pickFile } from '../services/fileBrowser'
import Icon from './Icon.vue'
import BrandLogo from './BrandLogo.vue'
import { bridge } from '../services/bridge'
import { confirmDialog } from '../services/dialogs'
import { useModalFocus } from '../services/modalFocus'
import type { GitHubThemeRepository, LauncherThemePreset, ThemeSaveRequest } from '../types'

const props = defineProps<{
  themes: LauncherThemePreset[]
  activeThemeId: string
  busy: boolean
  githubInstallBusy: boolean
}>()

const emit = defineEmits<{
  apply: [theme: LauncherThemePreset]
  save: [metadata: ThemeSaveRequest]
  import: [payload: { name: string; data: string; replace: boolean }]
  export: [themeId: string]
  remove: [themeId: string]
  installGithub: [payload: { owner: string; repo: string; replace: boolean }]
  toast: [message: string]
}>()

const saveDialogOpen = ref(false)
const saveDialogElement = ref<HTMLElement | null>(null)
const handleSaveDialogKeydown = useModalFocus(saveDialogOpen, saveDialogElement, () => { saveDialogOpen.value = false })
const saveName = ref('Моя тема')
const saveAuthor = ref('')
const saveDescription = ref('Сохранённый набор настроек Nivora Launcher.')
const saveVersion = ref('1.0.0')
const saveCreators = ref('')
const githubQuery = ref('')
const githubResults = ref<GitHubThemeRepository[]>([])
const githubBusy = ref(false)
const githubError = ref('')
const githubPage = ref(1)
const canLoadMore = computed(() => githubResults.value.length >= githubPage.value * 30 && githubPage.value < 10)

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return typeof error === 'string' ? error : 'Неизвестная ошибка'
}

function themeSourceLabel(theme: LauncherThemePreset): string {
  if (theme.source === 'builtin') return 'Встроенная'
  return theme.source === 'github' ? 'GitHub · локальная копия' : 'Сохранена локально'
}

function chooseThemeFile() {
  void pickFile({ accept: '.zip,application/zip', title: 'ZIP-пакет темы' }).then((file) => {
    if (file) void importThemeData(file)
  })
}

function encodeBytes(bytes: Uint8Array): string {
  let binary = ''
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000))
  }
  return btoa(binary)
}

async function importThemeData(file: { fileName: string; data: string; size: number }) {
  if (file.size > 8 * 1024 * 1024) {
    emit('toast', 'ZIP темы больше 8 MiB')
    return
  }
  try {
    const data = file.data
    const preview = await bridge.themePreview(data)
    const theme = preview.theme
    const builtIn = props.themes.find((item) => item.source === 'builtin' && item.id === theme.id)
    if (builtIn) {
      emit('toast', `id «${theme.id}» зарезервирован встроенной темой`)
      return
    }
    const existing = props.themes.find((item) => item.id === theme.id && item.source !== 'builtin')
    const replace = existing
      ? await confirmDialog(`Тема «${existing.name}» уже есть. Заменить её импортированным пакетом?`, { confirmLabel: 'Заменить тему' })
      : false
    if (existing && !replace) return
    emit('import', { name: file.fileName, data, replace })
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

function openSaveDialog() {
  saveDialogOpen.value = true
}

function saveCurrentTheme() {
  const metadata: ThemeSaveRequest = {
    name: saveName.value.trim(),
    author: saveAuthor.value.trim(),
    description: saveDescription.value.trim(),
    version: saveVersion.value.trim(),
    creators: [...new Set(saveCreators.value.split(',').map((item) => item.trim()).filter(Boolean))],
  }
  if (!metadata.name || !metadata.author || !metadata.description || !metadata.version) {
    emit('toast', 'Заполните название, автора, описание и версию темы')
    return
  }
  if (!metadata.creators.length) metadata.creators = [metadata.author]
  emit('save', metadata)
  saveDialogOpen.value = false
}

async function searchGithub(reset = true) {
  if (githubBusy.value) return
  githubBusy.value = true
  githubError.value = ''
  if (reset) {
    githubResults.value = []
    githubPage.value = 1
  }
  try {
    const page = reset ? 1 : githubPage.value + 1
    const response = await bridge.githubThemeSearch(githubQuery.value.trim(), page)
    githubPage.value = response.page
    githubResults.value = reset ? response.results : [...githubResults.value, ...response.results]
  } catch (error) {
    githubError.value = errorMessage(error)
  } finally {
    githubBusy.value = false
  }
}

async function addGithubTheme(repository: GitHubThemeRepository) {
  const builtIn = props.themes.find((theme) => theme.source === 'builtin' && theme.id === repository.repo.toLowerCase())
  if (builtIn) {
    emit('toast', `id «${repository.repo}» занят встроенной темой`)
    return
  }
  const existing = props.themes.find((theme) => theme.source !== 'builtin' && theme.id === repository.repo.toLowerCase())
  const replace = existing
    ? await confirmDialog(`Обновить локальную тему «${existing.name}» из ${repository.full_name}?`, { confirmLabel: 'Обновить тему' })
    : false
  if (existing && !replace) return
  emit('installGithub', { owner: repository.owner, repo: repository.repo, replace })
}

async function removeTheme(theme: LauncherThemePreset) {
  if (await confirmDialog(`Удалить локальную тему «${theme.name}»?` , { danger: true, confirmLabel: 'Удалить тему' })) {
    emit('remove', theme.id)
  }
}

function formatDate(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleDateString()
}
</script>

<template>
  <section class="theme-page">
    <div class="section-intro theme-page-intro">
      <div>
        
        <h2>Готовые темы</h2>
        <p>Начните с готового набора настроек: примените тему из списка, сохраните свой вариант, добавьте тему из файла или установите опубликованную тему с GitHub.</p>
      </div>
      <div class="theme-page-actions">
        
        <button class="button button--outline" type="button" :disabled="busy" @click="chooseThemeFile"><Icon name="upload" :size="15" />Добавить из файла</button>
        <button class="button button--accent" type="button" :disabled="busy" @click="openSaveDialog"><Icon name="save" :size="15" />Сохранить текущую</button>
      </div>
    </div>

    <section class="theme-hero panel">
      <div class="theme-hero-copy"><h3>Что меняют темы</h3><p>Темы меняют палитру, плотность и параметры сайдбара. Игровые файлы, настройки профилей и учётные записи остаются без изменений; архивы тем проверяются перед импортом.</p></div>
    </section>

    <section class="theme-library-section">
      <div class="theme-section-heading"><div><h3>Доступные темы</h3></div><span class="theme-count">{{ themes.length }}</span></div>
      <div class="theme-grid">
        <article v-for="theme in themes" :key="`${theme.source}-${theme.id}`" class="theme-card panel" :class="{ 'theme-card--active': activeThemeId === theme.id }">
          <div class="theme-preview" :style="{ '--theme-bg': theme.background, '--theme-panel': theme.panel, '--theme-soft': theme.panel_soft, '--theme-accent': theme.accent, '--theme-strong': theme.accent_strong }"><div class="theme-preview-sidebar"><BrandLogo class="theme-preview-mark" :color="theme.accent" label="Nivora Launcher" /><i /><i /><i /></div><div class="theme-preview-main"><span class="theme-preview-top" /><div class="theme-preview-card"><span /><strong /><small /></div><div class="theme-preview-button" /></div></div>
          <div class="theme-card-copy">
          <div class="theme-card-title"><div><h3>{{ theme.name }}</h3><span>{{ themeSourceLabel(theme) }} · {{ theme.author }}</span></div><span v-if="theme.featured" class="theme-featured">По умолчанию</span></div>
            <p>{{ theme.description }}</p>
            <div class="theme-swatches"><span :style="{ background: theme.accent }" /><span :style="{ background: theme.accent_strong }" /><span :style="{ background: theme.panel_soft }" /><small>{{ theme.mode }} · {{ theme.density }}<template v-if="theme.version"> · v{{ theme.version }}</template></small></div>
            <small v-if="theme.creators?.length" class="theme-creators">Создатели: {{ theme.creators.join(', ') }}</small>
          </div>
          <div class="theme-card-footer">
            <span v-if="activeThemeId === theme.id" class="theme-applied"><Icon name="check" :size="13" />Применена</span>
            <span v-else class="muted">{{ themeSourceLabel(theme) }}</span>
            <div class="theme-card-actions">
              <button v-if="theme.source !== 'builtin'" class="button button--small button--quiet" type="button" :disabled="busy" @click="emit('export', theme.id)"><Icon name="download" :size="13" />ZIP</button>
              <button v-if="theme.source !== 'builtin'" class="icon-button theme-delete-button" type="button" :disabled="busy" :aria-label="`Удалить тему ${theme.name}`" @click="removeTheme(theme)"><Icon name="trash" :size="14" /></button>
              <button class="button button--small" :class="activeThemeId === theme.id ? 'button--quiet' : 'button--accent'" type="button" :disabled="busy" @click="emit('apply', theme)">{{ activeThemeId === theme.id ? 'Используется' : 'Применить' }}</button>
            </div>
          </div>
        </article>
      </div>
    </section>

    <section class="github-theme-panel panel">
      <div class="theme-section-heading"><div><h3>Каталог GitHub</h3><p>Ищите темы в публичных репозиториях с GitHub topic <code>trial-launcher-theme</code>.</p></div><span class="theme-count"><Icon name="globe" :size="15" /></span></div>
      <div class="theme-github-search"><Icon name="search" :size="17" /><input v-model="githubQuery" maxlength="120" placeholder="Название или описание темы" @keyup.enter="searchGithub()" /><button class="button button--small button--accent" type="button" :disabled="githubBusy" @click="searchGithub()">{{ githubBusy ? 'Ищу…' : 'Найти' }}</button></div>
      <p v-if="githubError" class="inline-error">{{ githubError }}</p>
      <div v-if="githubResults.length" class="github-theme-grid">
        <article v-for="repository in githubResults" :key="repository.full_name" class="github-theme-card">
          <div class="github-theme-card-top"><span class="github-theme-icon"><Icon name="palette" :size="18" /></span><span class="tag"><Icon name="star" :size="12" />{{ repository.stars.toLocaleString() }}</span></div>
          <h4>{{ repository.full_name }}</h4>
          <p>{{ repository.description || 'Описание темы не указано.' }}</p>
          <small v-if="formatDate(repository.updated_at)">Обновлено {{ formatDate(repository.updated_at) }}</small>
          <button class="button button--small button--outline" type="button" :disabled="busy || githubInstallBusy" @click="addGithubTheme(repository)"><Icon name="download" :size="13" />{{ themes.some((theme) => theme.source !== 'builtin' && theme.id === repository.repo.toLowerCase()) ? 'Добавить обновление' : 'Добавить тему' }}</button>
        </article>
      </div>
      <div v-else-if="githubBusy" class="github-theme-grid" aria-label="Загрузка результатов">
        <article v-for="item in 3" :key="item" class="github-theme-skeleton" role="status" aria-label="Загружаются темы">
          <span /><span /><span /><span />
        </article>
      </div>
      <div v-else-if="!githubBusy && !githubError" class="github-theme-empty"><Icon name="globe" :size="20" /><span>Введите запрос и найдите темы из публичных GitHub Releases.</span></div>
      <button v-if="canLoadMore" class="button button--quiet theme-load-more" type="button" :disabled="githubBusy" @click="searchGithub(false)">{{ githubBusy ? 'Загружаю…' : 'Показать ещё' }}</button>
    </section>

    <section class="panel theme-notes"><div class="theme-note"><span class="extension-icon"><Icon name="palette" :size="18" /></span><div><strong>Формат темы</strong><p>ZIP содержит theme.json с настройками и theme.yml с автором, описанием, версией и списком создателей.</p></div></div><div class="theme-note"><span class="extension-icon"><Icon name="check" :size="18" /></span><div><strong>Применение без перезапуска</strong><p>Акцент, цветовые токены, плотность и боковая панель применяются сразу.</p></div></div></section>

    <Teleport to="body">
      <Transition name="modal">
      <div v-if="saveDialogOpen" class="modal-scrim" role="presentation" @click.self="saveDialogOpen = false">
      <section ref="saveDialogElement" class="modal-card theme-save-dialog" role="dialog" tabindex="-1" aria-modal="true" aria-labelledby="theme-save-title" @keydown="handleSaveDialogKeydown">
        <div class="panel-heading"><div><h2 id="theme-save-title">Сохранить текущую тему</h2><p>В архив попадут текущие цвета, плотность и режим боковой панели.</p></div><button class="icon-button" type="button" aria-label="Закрыть" @click="saveDialogOpen = false"><Icon name="close" :size="17" /></button></div>
        <div class="theme-save-fields">
          <label class="field"><span>Название</span><input v-model="saveName" maxlength="80" placeholder="Например, Моя тема" /></label>
          <label class="field"><span>Автор</span><input v-model="saveAuthor" maxlength="120" placeholder="Имя автора" /></label>
          <label class="field"><span>Описание темы</span><textarea v-model="saveDescription" maxlength="500" rows="3" placeholder="Что меняет эта тема" /></label>
          <div class="theme-save-row"><label class="field"><span>Версия</span><input v-model="saveVersion" maxlength="64" placeholder="1.0.0" /></label><label class="field"><span>Создатели</span><input v-model="saveCreators" placeholder="Имена через запятую" /></label></div>
        </div>
        <div class="modal-actions"><button class="button button--quiet" type="button" @click="saveDialogOpen = false">Отмена</button><button class="button button--accent" type="button" :disabled="busy" @click="saveCurrentTheme"><Icon name="save" :size="14" />Сохранить локально</button></div>
      </section>
      </div>
      </Transition>
    </Teleport>
  </section>
</template>

<style scoped>
.theme-page-intro { align-items: flex-end; }
.theme-page-actions, .theme-card-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 7px; }
.theme-section-heading { display: flex; align-items: center; justify-content: space-between; gap: 12px; margin-bottom: 11px; }
.theme-section-heading > div { display: grid; gap: 4px; }
.theme-section-heading h3 { margin: 0; font-size: 16px; }
.theme-section-heading p { margin: 0; color: var(--muted); font-size: 10px; }
.theme-count { display: grid; min-width: 28px; height: 28px; place-items: center; border: 1px solid var(--line); border-radius: 999px; color: var(--muted); background: var(--panel-soft); font-size: 10px; }
.theme-library-section { min-width: 0; }
.theme-card { grid-template-rows: auto 1fr auto; }
.theme-card-copy { min-width: 0; }
.theme-card-copy > p { min-height: 2.8em; }
.theme-card-footer { flex-wrap: wrap; padding: 0 17px 17px; }
.theme-card-actions { justify-content: flex-end; }
.theme-delete-button { width: 30px; height: 30px; }
.theme-creators { color: var(--muted); font-size: 9px; line-height: 1.45; }
.theme-hero { min-height: 0; padding: 20px; border-color: var(--line); background: var(--panel); }
.theme-hero-copy { max-width: 720px; gap: 6px; }
.theme-hero h3 { margin: 0; font-size: 22px; font-weight: 600; letter-spacing: -.03em; }
.theme-hero p { font-size: 12px; }
.theme-card { gap: 11px; border-radius: var(--radius-card, 7px); }
.theme-card:hover, .theme-card--active { border-color: var(--line-strong); box-shadow: none; transform: none; }
.theme-preview { height: 128px; border-bottom: 1px solid var(--line); background: var(--theme-bg); }
.theme-preview-sidebar { background: var(--theme-panel); }
.theme-preview-card { border-radius: calc(var(--radius-button, 8px) * .625); background: var(--theme-panel); }
.theme-preview-card > span { background: var(--theme-accent); }
.theme-card-title span:not(.theme-featured), .theme-swatches small { font-family: var(--font-family); font-size: 10px; }
.theme-featured { color: var(--accent-strong, var(--accent)); background: var(--panel-soft); }
.github-theme-card { border-radius: calc(var(--radius-card, 7px) * .857); }
.github-theme-skeleton { display: grid; align-content: start; gap: 11px; min-height: 190px; padding: 14px; border: 1px solid var(--line); border-radius: calc(var(--radius-card, 7px) * .857); background: var(--panel-raised); }
.github-theme-skeleton span { display: block; height: 10px; border-radius: 2px; background: var(--panel-soft); }
.github-theme-skeleton span:first-child { width: 34px; height: 34px; margin-bottom: 2px; }
.github-theme-skeleton span:nth-child(2) { width: 62%; }
.github-theme-skeleton span:nth-child(3) { width: 92%; }
.github-theme-skeleton span:nth-child(4) { width: 78%; margin-top: 8px; }
.github-theme-panel { display: grid; gap: 12px; padding: 20px; }
.theme-github-search { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; align-items: center; gap: 9px; padding: 8px 10px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.1); color: var(--muted); background: var(--panel-soft); }
.theme-github-search input { min-width: 0; min-height: 34px; border: 0; color: var(--text); background: transparent; outline: none; font: inherit; }
.github-theme-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 10px; }
.github-theme-card { display: grid; min-width: 0; align-content: start; gap: 9px; padding: 14px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.2); background: var(--panel-raised); }
.github-theme-card-top { display: flex; align-items: center; justify-content: space-between; gap: 9px; }
.github-theme-icon { display: grid; width: 36px; height: 36px; place-items: center; border: 1px solid color-mix(in srgb, var(--accent) 35%, var(--line)); border-radius: var(--radius-control, 10px); color: var(--accent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
.github-theme-card h4 { overflow: hidden; margin: 0; text-overflow: ellipsis; white-space: nowrap; font-size: 12px; }
.github-theme-card p { min-height: 3.2em; margin: 0; color: var(--muted); font-size: 10px; line-height: 1.55; }
.github-theme-card small { color: var(--muted); font-size: 9px; }
.github-theme-card .button { justify-self: start; }
.github-theme-empty { display: flex; min-height: 82px; align-items: center; justify-content: center; gap: 9px; border: 1px dashed var(--line-strong); border-radius: var(--radius-control, 10px); color: var(--muted); font-size: 11px; text-align: center; }
.theme-load-more { justify-self: center; }
.theme-save-dialog { display: grid; gap: 16px; }
.theme-save-dialog .panel-heading h2 { margin: 4px 0; }
.theme-save-dialog .panel-heading p { margin: 0; color: var(--muted); font-size: 10px; line-height: 1.5; }
.theme-save-fields { display: grid; gap: 11px; }
.theme-save-fields .field { display: grid; gap: 6px; }
.theme-save-fields .field > span { color: var(--muted); font-size: 10px; }
.theme-save-fields input, .theme-save-fields textarea { width: 100%; min-width: 0; padding: 9px 10px; border: 1px solid var(--line); border-radius: var(--radius-button, 8px); color: var(--text); background: var(--panel-soft); font: inherit; font-size: 11px; }
.theme-save-fields textarea { resize: vertical; }
.theme-save-row { display: grid; grid-template-columns: minmax(90px, .6fr) minmax(0, 1.4fr); gap: 10px; }
@media (max-width: 760px) {
  .theme-page-intro { align-items: stretch; flex-direction: column; }
  .theme-page-actions .button { flex: 1 1 100%; }
  .github-theme-grid { grid-template-columns: repeat(2, minmax(0, 1fr)); }
}
@media (max-width: 520px) {
  .github-theme-panel { padding: 14px; }
  .github-theme-grid, .theme-save-row { grid-template-columns: 1fr; }
  .theme-github-search { grid-template-columns: auto minmax(0, 1fr); }
  .theme-github-search .button { grid-column: 1 / -1; }
}
</style>
