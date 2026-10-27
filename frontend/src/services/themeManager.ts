import type { ComputedRef, Ref } from 'vue'
import type { PluginUiTone } from '../../../plugins/api'
import { bridge } from './bridge'
import type { AnimationSettings, AppState, AppearanceSettings, LauncherThemePreset, ThemeSaveRequest } from '../types'

type ThemeManagerDependencies = {
  state: AppState
  appearanceDraft: AppearanceSettings
  settingsDraft: { font_family: string }
  localThemes: Ref<LauncherThemePreset[]>
  themeOperationBusy: Ref<boolean>
  githubThemeInstallBusy: Ref<boolean>
  activeThemeId: Ref<string>
  availableThemes: ComputedRef<LauncherThemePreset[]>
  defaultColorTokens: Record<string, string>
  lightColorTokens: Record<string, string>
  defaultRadiusTokens: Record<string, number>
  defaultAnimationSettings: AnimationSettings
  busy: Ref<boolean>
  wallpaperPreviewUrl: Ref<string>
  wallpaperRenderBlur: Ref<number>
  showToast: (message: string, tone?: PluginUiTone) => void
  errorMessage: (error: unknown) => string
  saveSettings: () => Promise<boolean>
  releaseWallpaperPreview: () => void
}

export function createThemeManager(deps: ThemeManagerDependencies) {
  const {
    state,
    appearanceDraft,
    settingsDraft,
    localThemes,
    themeOperationBusy,
    githubThemeInstallBusy,
    activeThemeId,
    availableThemes,
    defaultColorTokens,
    lightColorTokens,
    defaultRadiusTokens,
    defaultAnimationSettings,
    busy,
    wallpaperPreviewUrl,
    wallpaperRenderBlur,
    showToast,
    errorMessage,
    saveSettings,
    releaseWallpaperPreview,
  } = deps

  function applyTheme(theme: LauncherThemePreset) {
    activeThemeId.value = theme.id
    appearanceDraft.active_theme_id = theme.id
    appearanceDraft.theme = theme.mode
    appearanceDraft.accent_color = theme.accent
    appearanceDraft.sidebar_mode = theme.sidebar_mode
    appearanceDraft.density = theme.density
    // дополняем цвета пресета базовой палитрой, чтобы не сохранять цвета старой темы
    const base = theme.mode === 'light' ? lightColorTokens : defaultColorTokens
    appearanceDraft.color_tokens = {
      ...base,
      accent: theme.accent,
      accent_strong: theme.accent_strong,
      background: theme.background,
      panel: theme.panel,
      panel_soft: theme.panel_soft,
      banner_shade: theme.background,
    }
    showToast(`Тема «${theme.name}» применена`)
    if (!busy.value) void saveSettings()
  }

  async function resetAppearance() {
    if (appearanceDraft.wallpaper_path || appearanceDraft.wallpaper_data || wallpaperPreviewUrl.value) {
      try {
        const result = await bridge.clearWallpaper()
        releaseWallpaperPreview()
        appearanceDraft.wallpaper_path = result.settings.wallpaper_path
        appearanceDraft.wallpaper_data = result.settings.wallpaper_data || ''
        state.settings.wallpaper_path = result.settings.wallpaper_path
        state.settings.wallpaper_data = result.settings.wallpaper_data || ''
      } catch (error) {
        showToast(errorMessage(error))
      }
    }
    activeThemeId.value = 'nivora-dark'
    appearanceDraft.active_theme_id = 'nivora-dark'
    appearanceDraft.theme = 'dark'
    appearanceDraft.accent_color = defaultColorTokens.accent
    appearanceDraft.color_tokens = { ...defaultColorTokens }
    appearanceDraft.radius_tokens = { ...defaultRadiusTokens }
    appearanceDraft.sidebar_position = 'left'
    appearanceDraft.sidebar_mode = 'adaptive'
    appearanceDraft.sidebar_width_mode = 'content'
    appearanceDraft.sidebar_width = 228
    appearanceDraft.sidebar_floating = false
    appearanceDraft.sidebar_visibility = 'always'
    appearanceDraft.animations = 'full'
    appearanceDraft.animation_settings = {
      ...defaultAnimationSettings,
      page_styles: { ...defaultAnimationSettings.page_styles },
      page_durations_ms: { ...defaultAnimationSettings.page_durations_ms },
    } as AnimationSettings
    appearanceDraft.density = 'comfortable'
    appearanceDraft.ui_scale = 100
    appearanceDraft.wallpaper_blur = 12
    appearanceDraft.wallpaper_opacity = 34
    wallpaperRenderBlur.value = 12
    appearanceDraft.page_themes = { instances: 'inherit', 'instance-settings': 'inherit', accounts: 'inherit', mods: 'inherit', modpacks: 'inherit', plugins: 'inherit', themes: 'inherit', settings: 'inherit' }
    appearanceDraft.page_layouts = { instances: 'balanced', 'instance-settings': 'balanced', accounts: 'balanced', mods: 'wide', modpacks: 'wide', plugins: 'wide', themes: 'wide', settings: 'balanced' }
    appearanceDraft.widget_visibility = { 'instances.workspace': true, 'accounts.status': true, 'mods.context': true }
    settingsDraft.font_family = 'system'
    syncActiveTheme()
    showToast('Оформление сброшено к значениям по умолчанию')
    if (!busy.value) void saveSettings()
  }

  function themeSlug(value: string): string {
    const transliteration: Record<string, string> = {
      а: 'a', б: 'b', в: 'v', г: 'g', д: 'd', е: 'e', ё: 'yo', ж: 'zh', з: 'z', и: 'i', й: 'y', к: 'k', л: 'l', м: 'm', н: 'n', о: 'o', п: 'p', р: 'r', с: 's', т: 't', у: 'u', ф: 'f', х: 'kh', ц: 'ts', ч: 'ch', ш: 'sh', щ: 'shch', ъ: '', ы: 'y', ь: '', э: 'e', ю: 'yu', я: 'ya',
    }
    const slug = [...value.toLowerCase()].map((char) => transliteration[char] ?? char).join('').normalize('NFKD').replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)
    return slug || 'local-theme'
  }

  function uniqueThemeId(name: string): string {
    const existing = new Set(availableThemes.value.map((theme) => theme.id))
    const base = themeSlug(name)
    let id = base
    let suffix = 2
    while (existing.has(id)) {
      id = `${base.slice(0, 40 - String(suffix).length - 1)}-${suffix}`
      suffix += 1
    }
    return id
  }

  async function refreshLocalThemes() {
    try {
      localThemes.value = (await bridge.themeList()).themes
    } catch (error) {
      showToast(errorMessage(error))
    }
  }

  async function saveCurrentTheme(metadata: ThemeSaveRequest) {
    themeOperationBusy.value = true
    const tokens = appearanceDraft.color_tokens
    const theme = {
      schema_version: 1 as const,
      id: uniqueThemeId(metadata.name),
      name: metadata.name,
      author: metadata.author,
      description: metadata.description,
      mode: appearanceDraft.theme,
      accent: String(tokens.accent || appearanceDraft.accent_color),
      accent_strong: String(tokens.accent_strong || defaultColorTokens.accent_strong),
      background: String(tokens.background || defaultColorTokens.background),
      panel: String(tokens.panel || defaultColorTokens.panel),
      panel_soft: String(tokens.panel_soft || defaultColorTokens.panel_soft),
      density: appearanceDraft.density,
      sidebar_mode: appearanceDraft.sidebar_mode,
    }
    try {
      const result = await bridge.themeSave(theme, metadata)
      localThemes.value = result.themes
      syncActiveTheme()
      showToast(`Тема «${result.theme.name}» сохранена локально`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      themeOperationBusy.value = false
    }
  }

  async function importThemeArchive(payload: { name: string; data: string; replace: boolean }) {
    themeOperationBusy.value = true
    try {
      const result = await bridge.themeImport(payload.data, payload.replace)
      localThemes.value = result.themes
      showToast(`Тема «${result.theme.name}» импортирована`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      themeOperationBusy.value = false
    }
  }

  async function deleteLocalTheme(themeId: string) {
    themeOperationBusy.value = true
    try {
      const result = await bridge.themeDelete(themeId)
      localThemes.value = result.themes
      if (activeThemeId.value === themeId) {
        activeThemeId.value = ''
        appearanceDraft.active_theme_id = ''
        syncActiveTheme()
        if (!busy.value) void saveSettings()
      }
      showToast('Локальная тема удалена')
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      themeOperationBusy.value = false
    }
  }

  async function exportLocalTheme(themeId: string) {
    themeOperationBusy.value = true
    try {
      const result = await bridge.themeExport(themeId)
      const binary = atob(result.data)
      const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
      const url = URL.createObjectURL(new Blob([bytes], { type: 'application/zip' }))
      const link = document.createElement('a')
      link.href = url
      link.download = result.file_name
      link.click()
      window.setTimeout(() => URL.revokeObjectURL(url), 1000)
      showToast(`Архив темы «${themeId}» сохранён`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      themeOperationBusy.value = false
    }
  }

  async function installGithubTheme(payload: { owner: string; repo: string; replace: boolean }) {
    if (githubThemeInstallBusy.value) return
    githubThemeInstallBusy.value = true
    try {
      const result = await bridge.githubThemeInstall(payload.owner, payload.repo, payload.replace)
      localThemes.value = result.themes
      showToast(`Тема «${result.theme.name}» добавлена из GitHub · SHA-256 ${result.metadata.sha256}`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      githubThemeInstallBusy.value = false
    }
  }

  function themeMatchesSettings(theme: LauncherThemePreset): boolean {
    const tokens = appearanceDraft.color_tokens
    return theme.mode === appearanceDraft.theme
      && theme.accent.toLowerCase() === appearanceDraft.accent_color.toLowerCase()
      && theme.accent.toLowerCase() === String(tokens.accent || '').toLowerCase()
      && theme.accent_strong.toLowerCase() === String(tokens.accent_strong || '').toLowerCase()
      && theme.background.toLowerCase() === String(tokens.background || '').toLowerCase()
      && theme.panel.toLowerCase() === String(tokens.panel || '').toLowerCase()
      && theme.panel_soft.toLowerCase() === String(tokens.panel_soft || '').toLowerCase()
      && theme.density === appearanceDraft.density
      && theme.sidebar_mode === appearanceDraft.sidebar_mode
  }

  function syncActiveTheme() {
    const persisted = availableThemes.value.find((theme) => theme.id === appearanceDraft.active_theme_id && themeMatchesSettings(theme))
    const matching = persisted || availableThemes.value.find((theme) => themeMatchesSettings(theme))
    activeThemeId.value = matching?.id || ''
    appearanceDraft.active_theme_id = activeThemeId.value
  }


  return {
    applyTheme,
    resetAppearance,
    refreshLocalThemes,
    saveCurrentTheme,
    importThemeArchive,
    deleteLocalTheme,
    exportLocalTheme,
    installGithubTheme,
    themeMatchesSettings,
    syncActiveTheme,
  }
}
