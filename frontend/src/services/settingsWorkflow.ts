import { computed, nextTick, ref, watch, type Ref } from 'vue'
import { bridge } from './bridge'
import { confirmDialog } from './dialogs'
import { launcherThemes } from './launcherExtensions'
import { setExplorerMode } from './fileBrowser'
import { normalizeHotkey, normalizeOverlayHotkey } from './overlay'
import { setLocale, type Locale } from '../i18n'
import type {
  AppState,
  AppearanceSettings,
  AnimationStyle,
  LauncherProfile,
  LauncherThemePreset,
  MemoryUnit,
  PluginCatalog,
  ProjectType,
  ResourceCatalog,
  SelectOption,
  SettingsInfo,
  SettingsProfile,
  ViewName,
} from '../types'

export type SettingsDraftState = Omit<SettingsInfo,
  keyof AppearanceSettings
  | 'security_profile'
  | 'security_permissions'
  | 'memory_kib'
  | 'memory_total_kib'
  | 'memory_recommended_kib'
  | 'has_client_id'
  | 'instance_groups'
  | 'pinned_instance_groups'
  | 'download_source'
> & {
  download_source: string
}

type SettingsDraftSnapshot = {
  settings: Record<string, unknown>
  appearance: Record<string, unknown>
}

type SettingsHydration = {
  previousSavedSnapshot?: SettingsDraftSnapshot
  preservedFontFamily: string
}

type SettingsWorkflowDependencies = {
  settingsDraft: SettingsDraftState
  appearanceDraft: AppearanceSettings
  state: AppState
  currentView: Ref<ViewName>
  systemDark: Ref<boolean>
  localThemes: Ref<LauncherThemePreset[]>
  wallpaperPreviewUrl: Ref<string>
  wallpaperProcessedUrl: Ref<string>
  memoryValue: Ref<number>
  memoryUnit: Ref<MemoryUnit>
  javaSelection: Ref<string>
  activeCatalogId: Ref<string>
  modProjectType: Ref<ProjectType>
  busy: Ref<boolean>
  persistentViewOptions: SelectOption[]
  shortcutRows: Array<{ key: string; label: string; default: string }>
  defaultColorTokens: Record<string, string>
  lightColorTokens: Record<string, string>
  defaultLauncherProfile: LauncherProfile
  showToast: (message: string) => void
  errorMessage: (error: unknown) => string
  memorySetting: () => string
  effectiveJavaPath: () => string
  commitWallpaperBlur: () => void
  applyState: (next: AppState, preserveAppearance?: boolean) => void
  ensureCatalogForView: (view: ViewName) => void
  broadcastSettingsChanged: (keys: string[]) => void
  syncActiveTheme: () => void
}

export function createSettingsWorkflow(deps: SettingsWorkflowDependencies) {
  const {
    settingsDraft,
    appearanceDraft,
    state,
    currentView,
    systemDark,
    localThemes,
    wallpaperPreviewUrl,
    wallpaperProcessedUrl,
    memoryValue,
    memoryUnit,
    javaSelection,
    activeCatalogId,
    modProjectType,
    busy,
    persistentViewOptions,
    shortcutRows,
    defaultColorTokens,
    lightColorTokens,
    defaultLauncherProfile,
    showToast,
    errorMessage,
    memorySetting,
    effectiveJavaPath,
    commitWallpaperBlur,
    applyState,
    ensureCatalogForView,
    broadcastSettingsChanged,
    syncActiveTheme,
  } = deps

  const settingsSaveState = ref<'saved' | 'dirty' | 'saving' | 'error'>('saved')
  const settingsSaveMessage = ref('Изменения сохранены')
  const hydratingSettings = ref(false)
  const sidebarResizeActive = ref(false)
  let savedSettingsSnapshot: SettingsDraftSnapshot | undefined
  const availableThemes = computed(() => [...launcherThemes, ...localThemes.value])

  const languageOptions: SelectOption[] = [
    { value: 'ru', label: 'Русский' },
    { value: 'en', label: 'English' },
  ]

  function setLanguageFromSettings(value: string): void {
    const next: Locale = value === 'en' ? 'en' : 'ru'
    settingsDraft.language = next
    setLocale(next)
  }

  const fontFamilyCss = computed(() => {
    const families: Record<string, string> = {
      system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI Variable Text', 'Segoe UI', Roboto, sans-serif",
      inter: "Inter, 'Segoe UI Variable Text', 'Segoe UI', system-ui, sans-serif",
      manrope: "Manrope, Inter, 'Segoe UI Variable Text', 'Segoe UI', system-ui, sans-serif",
      mono: "'DM Mono', 'Cascadia Mono', 'Cascadia Code', Consolas, ui-monospace, monospace",
    }
    return families[settingsDraft.font_family] || families.system
  })

  function cloneSettingsValue(value: unknown): unknown {
    if (Array.isArray(value)) return value.map(cloneSettingsValue)
    if (value && typeof value === 'object') {
      return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, cloneSettingsValue(item)]))
    }
    return value
  }

  function currentSettingsDraftValues(): SettingsDraftSnapshot {
    return {
      settings: { ...settingsDraft, memory: memorySetting(), java_path: effectiveJavaPath() },
      appearance: appearanceDraft as unknown as Record<string, unknown>,
    }
  }

  function takeSettingsDraftSnapshot(): SettingsDraftSnapshot {
    return cloneSettingsValue(currentSettingsDraftValues()) as SettingsDraftSnapshot
  }

  function settingsValuesMatch(left: unknown, right: unknown): boolean {
    if (Object.is(left, right)) return true
    if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false
    if (Array.isArray(left) || Array.isArray(right)) {
      return Array.isArray(left) && Array.isArray(right) && left.length === right.length && left.every((value, index) => settingsValuesMatch(value, right[index]))
    }
    const leftRecord = left as Record<string, unknown>
    const rightRecord = right as Record<string, unknown>
    const leftKeys = Object.keys(leftRecord)
    const rightKeys = Object.keys(rightRecord)
    return leftKeys.length === rightKeys.length && leftKeys.every((key) => Object.hasOwn(rightRecord, key) && settingsValuesMatch(leftRecord[key], rightRecord[key]))
  }

  function refreshSettingsSaveState() {
    if (!savedSettingsSnapshot || hydratingSettings.value || settingsSaveState.value === 'saving') return
    const saved = settingsValuesMatch(currentSettingsDraftValues(), savedSettingsSnapshot)
    settingsSaveState.value = saved ? 'saved' : 'dirty'
    settingsSaveMessage.value = saved ? 'Изменения сохранены' : 'Есть несохранённые изменения'
  }

  let settingsSaveStateTimer: number | undefined
  function scheduleSettingsSaveStateRefresh() {
    if (settingsSaveStateTimer !== undefined) window.clearTimeout(settingsSaveStateTimer)
    settingsSaveStateTimer = window.setTimeout(() => {
      settingsSaveStateTimer = undefined
      refreshSettingsSaveState()
    }, 250)
  }

  function mergeSavedSettingsBaseline(patch: Record<string, unknown>) {
    if (!savedSettingsSnapshot) {
      savedSettingsSnapshot = takeSettingsDraftSnapshot()
    } else {
      savedSettingsSnapshot = {
        ...savedSettingsSnapshot,
        settings: { ...savedSettingsSnapshot.settings, ...(cloneSettingsValue(patch) as Record<string, unknown>) },
      }
    }
    refreshSettingsSaveState()
  }

  function updateSavedSettingsBaseline(patch: Record<string, unknown>) {
    if (savedSettingsSnapshot) {
      savedSettingsSnapshot.settings = {
        ...savedSettingsSnapshot.settings,
        ...(cloneSettingsValue(patch) as Record<string, unknown>),
      }
    }
    refreshSettingsSaveState()
  }

  function setInstanceLayout(value: string) {
    const next = value === 'compact' ? 'compact' : 'default'
    appearanceDraft.instance_layout = next
    void bridge.saveSettings({ instance_layout: next }).then((result) => {
      state.settings.instance_layout = result.settings.instance_layout
      mergeSavedSettingsBaseline({ instance_layout: result.settings.instance_layout })
    }).catch((error) => showToast(errorMessage(error)))
  }

  watch([settingsDraft, appearanceDraft, memoryValue, memoryUnit, javaSelection], scheduleSettingsSaveStateRefresh, { deep: true })

  watch(() => settingsDraft.file_explorer, (mode) => setExplorerMode(mode === 'system' ? 'system' : 'builtin'))

  function beginSettingsHydration(preserveAppearance: boolean): SettingsHydration {
    const previousSavedSnapshot = preserveAppearance ? savedSettingsSnapshot : undefined
    hydratingSettings.value = true
    return {
      previousSavedSnapshot,
      preservedFontFamily: preserveAppearance ? settingsDraft.font_family : '',
    }
  }

  function finishSettingsHydration(hydration: SettingsHydration) {
    const nextSavedSnapshot = takeSettingsDraftSnapshot()
    if (hydration.previousSavedSnapshot) {
      nextSavedSnapshot.appearance = hydration.previousSavedSnapshot.appearance
      nextSavedSnapshot.settings.font_family = hydration.previousSavedSnapshot.settings.font_family
    }
    savedSettingsSnapshot = nextSavedSnapshot
    const hasUnsavedSettings = !settingsValuesMatch(currentSettingsDraftValues(), savedSettingsSnapshot)
    settingsSaveState.value = hasUnsavedSettings ? 'dirty' : 'saved'
    settingsSaveMessage.value = hasUnsavedSettings ? 'Есть несохранённые изменения' : 'Изменения сохранены'
    setLocale(settingsDraft.language === 'en' ? 'en' : 'ru')
    void nextTick(() => {
      hydratingSettings.value = false
      refreshSettingsSaveState()
    })
  }

  const resolvedGlobalTheme = computed(() => {
    if (appearanceDraft.theme === 'system') return systemDark.value ? 'dark' : 'light'
    return appearanceDraft.theme
  })
  const currentTheme = computed(() => {
    const override = appearanceDraft.page_themes[currentView.value]
    if (!override || override === 'inherit') return resolvedGlobalTheme.value
    return override
  })
  const currentLayout = computed(() => appearanceDraft.page_layouts[currentView.value] || 'balanced')
  const effectiveColorTokens = computed<Record<string, string>>(() => {
    const base = currentTheme.value === 'light' ? lightColorTokens : defaultColorTokens
    return Object.fromEntries(
      Object.entries(defaultColorTokens).map(([key, fallback]) => [
        key,
        appearanceDraft.color_tokens[key] === fallback ? base[key] : appearanceDraft.color_tokens[key] || base[key],
      ]),
    )
  })
  const effectiveAccent = computed(() => effectiveColorTokens.value.accent)
  const wallpaperUrl = computed(() => wallpaperPreviewUrl.value || appearanceDraft.wallpaper_data || (appearanceDraft.wallpaper_path ? bridge.fileUrl(appearanceDraft.wallpaper_path) : ''))
  const wallpaperDisplayUrl = computed(() => wallpaperProcessedUrl.value || wallpaperUrl.value)
  const accentForeground = computed(() => {
    const hex = effectiveAccent.value.replace('#', '')
    const red = Number.parseInt(hex.slice(0, 2), 16) || 0
    const green = Number.parseInt(hex.slice(2, 4), 16) || 0
    const blue = Number.parseInt(hex.slice(4, 6), 16) || 0
    return (red * 299 + green * 587 + blue * 114) / 1000 > 154 ? '#080808' : '#ffffff'
  })
  const sidebarWidth = computed(() => {
    if (appearanceDraft.sidebar_position === 'top' || appearanceDraft.sidebar_position === 'bottom') return 252
    if (appearanceDraft.sidebar_mode === 'compact') return 72
    if (appearanceDraft.sidebar_width_mode === 'full') return 300
    if (appearanceDraft.sidebar_width_mode === 'manual') return Math.min(360, Math.max(168, appearanceDraft.sidebar_width))
    return 252
  })
  const sidebarHeight = computed(() => {
    if (appearanceDraft.sidebar_position !== 'top' && appearanceDraft.sidebar_position !== 'bottom') return 0
    return appearanceDraft.sidebar_mode === 'compact' ? 128 : 184
  })
  const activeLauncherProfile = computed(() => settingsDraft.launcher_profiles.find((profile) => profile.id === settingsDraft.active_launcher_profile_id) || settingsDraft.launcher_profiles[0])
  const effectiveAnimationStyle = computed<AnimationStyle>(() => {
    if (appearanceDraft.animations === 'off') return 'none'
    return appearanceDraft.animation_settings.page_styles[currentView.value] || appearanceDraft.animation_settings.page_style
  })
  const effectiveAnimationDuration = computed(() => appearanceDraft.animation_settings.page_durations_ms[currentView.value] || appearanceDraft.animation_settings.page_duration_ms)
  const appStyle = computed<Record<string, string>>(() => ({
    '--bg': effectiveColorTokens.value.background,
    '--panel': effectiveColorTokens.value.panel,
    '--panel-raised': effectiveColorTokens.value.panel_raised,
    '--panel-soft': effectiveColorTokens.value.panel_soft,
    '--text': effectiveColorTokens.value.text,
    '--muted': effectiveColorTokens.value.muted,
    '--quiet': effectiveColorTokens.value.quiet,
    '--blue-bright': effectiveColorTokens.value.accent_strong,
    '--green': effectiveColorTokens.value.success,
    '--danger': effectiveColorTokens.value.danger,
    '--warning': effectiveColorTokens.value.warning,
    '--line': `color-mix(in srgb, ${effectiveColorTokens.value.border} 16%, transparent)`,
    '--line-strong': `color-mix(in srgb, ${effectiveColorTokens.value.border} 30%, transparent)`,
    '--accent': effectiveAccent.value,
    '--accent-foreground': accentForeground.value,
    '--blue': effectiveAccent.value,
    '--brand-color': currentTheme.value === 'light' ? '#111214' : '#f4f7ff',
    '--sidebar-bg': `color-mix(in srgb, ${effectiveColorTokens.value.panel} 92%, ${effectiveColorTokens.value.background})`,
    '--ui-scale': `${appearanceDraft.ui_scale / 100}`,
    '--radius': `${appearanceDraft.radius_tokens.card}px`,
    '--radius-card': `${appearanceDraft.radius_tokens.card}px`,
    '--radius-button': `${appearanceDraft.radius_tokens.button}px`,
    '--radius-control': `${appearanceDraft.radius_tokens.control}px`,
    '--banner-shade': effectiveColorTokens.value.banner_shade,
    '--wallpaper-opacity': `${appearanceDraft.wallpaper_opacity / 100}`,
    '--sidebar-width': `${sidebarWidth.value}px`,
    '--sidebar-height': `${sidebarHeight.value}px`,
    '--font-family': fontFamilyCss.value,
    '--page-duration': `${effectiveAnimationDuration.value}ms`,
    '--surface-duration': `${appearanceDraft.animation_settings.surface_duration_ms}ms`,
    '--surface-stagger': `${appearanceDraft.animation_settings.surface_stagger_ms}ms`,
  }))
  const toastThemeStyle = computed<Record<string, string>>(() => ({
    '--text': effectiveColorTokens.value.text,
    '--muted': effectiveColorTokens.value.muted,
    '--panel-raised': effectiveColorTokens.value.panel_raised,
    '--panel-soft': effectiveColorTokens.value.panel_soft,
    '--line-strong': `color-mix(in srgb, ${effectiveColorTokens.value.border} 30%, transparent)`,
    '--accent': effectiveAccent.value,
    '--green': effectiveColorTokens.value.success,
    '--danger': effectiveColorTokens.value.danger,
    '--warning': effectiveColorTokens.value.warning,
  }))

  const settingsProfiles = ref<SettingsProfile[]>([])
  const settingsProfilesLoading = ref(false)
  const settingsProfileBusy = ref(false)
  const settingsProfileName = ref('')
  const backupDestinationOptions: SelectOption[] = [
    { value: 'local', label: 'Локально', meta: 'game_dir/backups/<экземпляр>' },
    { value: 'cloud', label: 'WebDAV', meta: 'Тот же endpoint, что у облака' },
  ]

  async function loadSettingsProfiles() {
    settingsProfilesLoading.value = true
    try {
      settingsProfiles.value = (await bridge.settingsProfilesList()).profiles
    } catch {
      settingsProfiles.value = []
    } finally {
      settingsProfilesLoading.value = false
    }
  }

  function formatDateShort(value: number): string {
    if (!value) return ''
    return new Date(value > 10_000_000_000 ? value : value * 1000).toLocaleString()
  }

  async function saveSettingsProfileAs() {
    const name = settingsProfileName.value.trim()
    if (!name) return
    settingsProfileBusy.value = true
    try {
      settingsProfiles.value = (await bridge.settingsProfileSave(name)).profiles
      settingsProfileName.value = ''
      showToast(`Профиль «${name}» сохранён`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      settingsProfileBusy.value = false
    }
  }

  async function overwriteSettingsProfile(name: string) {
    if (!(await confirmDialog(`Перезаписать профиль «${name}» текущими настройками?`, { confirmLabel: 'Перезаписать' }))) return
    settingsProfileBusy.value = true
    try {
      settingsProfiles.value = (await bridge.settingsProfileSave(name)).profiles
      showToast(`Профиль «${name}» обновлён`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      settingsProfileBusy.value = false
    }
  }

  async function applySettingsProfile(name: string) {
    if (!(await confirmDialog(`Применить профиль «${name}»? Текущие настройки будут заменены.`, { confirmLabel: 'Применить' }))) return
    settingsProfileBusy.value = true
    try {
      const result = await bridge.settingsProfileApply(name)
      applyState({ instances: state.instances, accounts: state.accounts, settings: result.settings, system: state.system })
      showToast(`Профиль «${name}» применён (${result.applied.length} параметров)`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      settingsProfileBusy.value = false
    }
  }

  async function deleteSettingsProfile(name: string) {
    if (!(await confirmDialog(`Удалить профиль «${name}»?`, { danger: true, confirmLabel: 'Удалить' }))) return
    settingsProfileBusy.value = true
    try {
      settingsProfiles.value = (await bridge.settingsProfileDelete(name)).profiles
      showToast('Профиль удалён')
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      settingsProfileBusy.value = false
    }
  }

  async function exportSettingsProfile(name: string) {
    settingsProfileBusy.value = true
    try {
      const result = await bridge.settingsProfileExport(name)
      const url = URL.createObjectURL(new Blob([result.json], { type: 'application/json' }))
      const link = document.createElement('a')
      link.href = url
      link.download = result.file_name
      link.click()
      window.setTimeout(() => URL.revokeObjectURL(url), 1000)
      showToast(`Профиль «${result.name}» экспортирован`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      settingsProfileBusy.value = false
    }
  }

  async function importSettingsProfile() {
    const input = document.createElement('input')
    input.type = 'file'
    input.accept = '.json,application/json'
    input.onchange = async () => {
      const file = input.files?.[0]
      if (!file) return
      settingsProfileBusy.value = true
      try {
        const json = await file.text()
        settingsProfiles.value = (await bridge.settingsProfileImport(json, file.name.replace(/\.json$/i, ''))).profiles
        showToast(`Профиль из «${file.name}» импортирован`)
      } catch (error) {
        showToast(errorMessage(error))
      } finally {
        settingsProfileBusy.value = false
      }
    }
    input.click()
  }

  function updateSidebarWidth(event: PointerEvent) {
    if (!sidebarResizeActive.value) return
    const position = appearanceDraft.sidebar_position
    const next = position === 'right' ? window.innerWidth - event.clientX : event.clientX
    appearanceDraft.sidebar_width = Math.min(360, Math.max(168, Math.round(next)))
  }

  function stopSidebarResize() {
    if (!sidebarResizeActive.value) return
    sidebarResizeActive.value = false
    document.body.style.cursor = ''
    window.removeEventListener('pointermove', updateSidebarWidth)
    window.removeEventListener('pointerup', stopSidebarResize)
  }

  function startSidebarResize(event: PointerEvent) {
    if (appearanceDraft.sidebar_width_mode !== 'manual') return
    event.preventDefault()
    sidebarResizeActive.value = true
    document.body.style.cursor = 'col-resize'
    window.addEventListener('pointermove', updateSidebarWidth)
    window.addEventListener('pointerup', stopSidebarResize)
  }

  function setProfileArguments(profile: LauncherProfile, key: 'jvm_arguments' | 'game_arguments', event: Event) {
    const value = (event.target as HTMLTextAreaElement).value
    profile[key] = value.split('\n').map((item) => item.trim()).filter(Boolean)
  }

  function addLauncherProfile() {
    let index = settingsDraft.launcher_profiles.length + 1
    const profile: LauncherProfile = {
      ...defaultLauncherProfile,
      id: `profile-${index}`,
      name: `Профиль ${index}`,
      jvm_arguments: [],
      game_arguments: [],
    }
    while (settingsDraft.launcher_profiles.some((item) => item.id === profile.id)) {
      index += 1
      profile.id = `profile-${index}`
      profile.name = `Профиль ${index}`
    }
    settingsDraft.launcher_profiles.push(profile)
    settingsDraft.active_launcher_profile_id = profile.id
  }

  function removeLauncherProfile(profileId: string) {
    if (settingsDraft.launcher_profiles.length <= 1) {
      showToast('Нельзя удалить последний профиль запуска')
      return
    }
    settingsDraft.launcher_profiles = settingsDraft.launcher_profiles.filter((profile) => profile.id !== profileId)
    if (settingsDraft.active_launcher_profile_id === profileId) settingsDraft.active_launcher_profile_id = settingsDraft.launcher_profiles[0].id
  }

  function settingsValidationError(): string {
    const integerRules: Array<[string, number, number, number]> = [
      ['Параллельные загрузки', settingsDraft.max_downloads, 1, 32],
      ['Обновление metadata', settingsDraft.metadata_refresh_hours, 1, 168],
      ['Timeout загрузки', settingsDraft.download_timeout_seconds, 5, 120],
      ['Повторные попытки', settingsDraft.download_retries, 1, 6],
      ['Хранение логов', settingsDraft.keep_logs_days, 0, 365],
    ]
    const invalid = integerRules.find(([, value, min, max]) => !Number.isInteger(value) || value < min || value > max)
    if (invalid) return `${invalid[0]}: укажите целое число от ${invalid[2]} до ${invalid[3]}.`
    for (const profile of settingsDraft.launcher_profiles) {
      if (!/^auto$|^\d+(?:\.\d+)?(?:KiB|MiB|GiB)$/i.test(profile.memory.trim())) return `Профиль «${profile.name}»: память должна быть auto, KiB, MiB или GiB.`
      if (!Number.isInteger(profile.window_width) || profile.window_width < 320 || profile.window_width > 8192) return `Профиль «${profile.name}»: ширина окна должна быть от 320 до 8192.`
      if (!Number.isInteger(profile.window_height) || profile.window_height < 240 || profile.window_height > 8192) return `Профиль «${profile.name}»: высота окна должна быть от 240 до 8192.`
    }
    return ''
  }

  function scrollSettingsSection(id: string) {
    document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  async function saveSettings(): Promise<boolean> {
    const validationError = settingsValidationError()
    if (validationError) {
      settingsSaveState.value = 'error'
      settingsSaveMessage.value = validationError
      showToast(validationError)
      return false
    }
    commitWallpaperBlur()
    settingsSaveState.value = 'saving'
    settingsSaveMessage.value = 'Сохраняю изменения…'
    busy.value = true
    try {
      const values: Record<string, unknown> = {
        game_dir: settingsDraft.game_dir,
        java_path: effectiveJavaPath(),
        memory: memorySetting(),
        max_downloads: settingsDraft.max_downloads,
        startup_view: settingsDraft.startup_view,
        last_view: settingsDraft.last_view,
        remember_last_view: settingsDraft.remember_last_view,
        default_version_filter: settingsDraft.default_version_filter,
        default_loader: settingsDraft.default_loader,
        default_account_id: settingsDraft.default_account_id,
        open_last_instance: settingsDraft.open_last_instance,
        launch_after_install: settingsDraft.launch_after_install,
        auto_refresh_metadata: settingsDraft.auto_refresh_metadata,
        metadata_refresh_hours: settingsDraft.metadata_refresh_hours,
        download_timeout_seconds: settingsDraft.download_timeout_seconds,
        download_retries: settingsDraft.download_retries,
        download_source: settingsDraft.download_source,
        confirm_destructive_actions: settingsDraft.confirm_destructive_actions,
        auto_repair_enabled: settingsDraft.auto_repair_enabled,
        auto_repair_muted: [...settingsDraft.auto_repair_muted],
        show_activity_panel: settingsDraft.show_activity_panel,
        show_toasts: settingsDraft.show_toasts,
         minimize_on_launch: settingsDraft.minimize_on_launch,
          language: settingsDraft.language ?? null,
          file_explorer: settingsDraft.file_explorer,
          file_editor: settingsDraft.file_editor,
          font_family: settingsDraft.font_family,
        keep_logs_days: settingsDraft.keep_logs_days,
        telemetry: settingsDraft.telemetry,
        telemetry_endpoint: settingsDraft.telemetry_endpoint.trim(),
         graphics_mode: settingsDraft.graphics_mode,
         keyboard_shortcuts: {
          ...settingsDraft.keyboard_shortcuts,
          open_settings: normalizeHotkey(settingsDraft.keyboard_shortcuts.open_settings, 'Ctrl+,'),
          refresh: normalizeHotkey(settingsDraft.keyboard_shortcuts.refresh, 'Ctrl+R'),
          new_instance: normalizeHotkey(settingsDraft.keyboard_shortcuts.new_instance, 'Ctrl+N'),
          focus_search: normalizeHotkey(settingsDraft.keyboard_shortcuts.focus_search, 'Ctrl+K'),
          toggle_activity: normalizeHotkey(settingsDraft.keyboard_shortcuts.toggle_activity, 'Ctrl+J'),
        },
         overlay_settings: {
          ...settingsDraft.overlay_settings,
          hotkey_overlay: normalizeOverlayHotkey(settingsDraft.overlay_settings.hotkey_overlay),
          hotkey_recording: normalizeHotkey(settingsDraft.overlay_settings.hotkey_recording, 'F7'),
        },
         backup_schedule: { ...settingsDraft.backup_schedule },
         launcher_profiles: settingsDraft.launcher_profiles.map((profile) => ({ ...profile, jvm_arguments: [...profile.jvm_arguments], game_arguments: [...profile.game_arguments] })),
         active_launcher_profile_id: settingsDraft.active_launcher_profile_id,
         http_proxy: settingsDraft.http_proxy.trim(),
        developer_mode: settingsDraft.developer_mode,
        ...appearanceDraft,
        page_themes: { ...appearanceDraft.page_themes },
        page_layouts: { ...appearanceDraft.page_layouts },
        widget_visibility: { ...appearanceDraft.widget_visibility },
        color_tokens: { ...appearanceDraft.color_tokens },
        radius_tokens: { ...appearanceDraft.radius_tokens },
        login_source: settingsDraft.login_source,
        login_api_url: settingsDraft.login_api_url.trim(),
        catalog_source: settingsDraft.catalog_source,
        catalog_api_url: settingsDraft.catalog_api_url.trim(),
        data_source: settingsDraft.data_source,
        data_api_url: settingsDraft.data_api_url.trim(),
        resource_catalogs: settingsDraft.resource_catalogs.map((catalog) => ({ ...catalog })),
        active_catalog_id: settingsDraft.active_catalog_id,
        plugin_catalogs: settingsDraft.plugin_catalogs.map((catalog) => ({ ...catalog })),
        active_plugin_catalog_id: settingsDraft.active_plugin_catalog_id,
      }
      const currentSettings = state.settings as unknown as Record<string, unknown>
      const changedKeys = Object.keys(values).filter((key) => JSON.stringify(values[key]) !== JSON.stringify(currentSettings[key]))
      const result = await bridge.saveSettings(values)
      applyState({ instances: state.instances, accounts: state.accounts, settings: result.settings, system: state.system })
      if (changedKeys.length) broadcastSettingsChanged(changedKeys)
      settingsSaveState.value = 'saved'
      settingsSaveMessage.value = 'Изменения сохранены'
      showToast('Настройки сохранены')
      return true
    } catch (error) {
      settingsSaveState.value = 'error'
      settingsSaveMessage.value = errorMessage(error)
      showToast(errorMessage(error))
      return false
    } finally {
      busy.value = false
    }
  }

  function addResourceCatalog() {
    let index = settingsDraft.resource_catalogs.length + 1
    const catalog: ResourceCatalog = {
      id: `catalog-${index}`,
      name: `Каталог ${index}`,
      kind: 'custom',
      api_url: 'https://example.invalid/modrinth/v2',
    }
    while (settingsDraft.resource_catalogs.some((item) => item.id === catalog.id)) catalog.id = `catalog-${++index}`
    settingsDraft.resource_catalogs.push(catalog)
    settingsDraft.active_catalog_id = catalog.id
    activeCatalogId.value = catalog.id
  }

  function removeResourceCatalog(catalogId: string) {
    if (settingsDraft.resource_catalogs.length <= 1) {
      showToast('Нельзя удалить последний каталог')
      return
    }
    settingsDraft.resource_catalogs = settingsDraft.resource_catalogs.filter((catalog) => catalog.id !== catalogId)
    if (settingsDraft.active_catalog_id === catalogId) {
      settingsDraft.active_catalog_id = settingsDraft.resource_catalogs[0].id
      activeCatalogId.value = settingsDraft.active_catalog_id
    }
  }

  function addPluginCatalog() {
    let index = settingsDraft.plugin_catalogs.length + 1
    const catalog = { id: `plugin-catalog-${index}`, name: `Plugin catalog ${index}`, kind: 'custom', api_url: 'https://example.invalid/plugins', enabled: true }
    while (settingsDraft.plugin_catalogs.some((item) => item.id === catalog.id)) catalog.id = `plugin-catalog-${++index}`
    settingsDraft.plugin_catalogs.push(catalog)
    settingsDraft.active_plugin_catalog_id = catalog.id
  }

  function removePluginCatalog(catalogId: string) {
    if (settingsDraft.plugin_catalogs.length <= 1) {
      showToast('Нельзя удалить последний каталог плагинов')
      return
    }
    settingsDraft.plugin_catalogs = settingsDraft.plugin_catalogs.filter((catalog) => catalog.id !== catalogId)
    if (settingsDraft.active_plugin_catalog_id === catalogId) settingsDraft.active_plugin_catalog_id = settingsDraft.plugin_catalogs[0].id
  }

  async function selectActiveCatalog(catalogId: string) {
    const pluginCatalogView = currentView.value === 'mods' && modProjectType.value === 'plugin'
    const allowed = pluginCatalogView
      ? settingsDraft.plugin_catalogs.filter((catalog) => catalog.enabled !== false)
      : currentView.value === 'modpacks'
        ? settingsDraft.resource_catalogs.filter((catalog) => ['modrinth', 'ftb', 'technic'].includes(catalog.kind))
        : currentView.value === 'mods'
          ? settingsDraft.resource_catalogs.filter((catalog) => !['ftb', 'technic'].includes(catalog.kind))
          : settingsDraft.resource_catalogs
    if (!allowed.some((catalog) => catalog.id === catalogId)) {
      ensureCatalogForView(currentView.value)
      showToast('Этот каталог не поддерживает текущий раздел')
      return
    }
    const previous = activeCatalogId.value
    activeCatalogId.value = catalogId
    if (pluginCatalogView) settingsDraft.active_plugin_catalog_id = catalogId
    else settingsDraft.active_catalog_id = catalogId
    try {
      const result = await bridge.saveSettings(pluginCatalogView ? { active_plugin_catalog_id: catalogId } : { active_catalog_id: catalogId })
      state.settings = result.settings
      const savedCatalogId = pluginCatalogView ? result.settings.active_plugin_catalog_id : result.settings.active_catalog_id
      if (pluginCatalogView) settingsDraft.active_plugin_catalog_id = savedCatalogId
      else settingsDraft.active_catalog_id = savedCatalogId
      activeCatalogId.value = savedCatalogId
      mergeSavedSettingsBaseline(pluginCatalogView ? { active_plugin_catalog_id: savedCatalogId } : { active_catalog_id: savedCatalogId })
      broadcastSettingsChanged([pluginCatalogView ? 'active_plugin_catalog_id' : 'active_catalog_id'])
      showToast('Каталог выбран')
    } catch (error) {
      activeCatalogId.value = previous
      if (pluginCatalogView) settingsDraft.active_plugin_catalog_id = previous
      else settingsDraft.active_catalog_id = previous
      refreshSettingsSaveState()
      showToast(errorMessage(error))
    }
  }

  function setSimpleAppearance(values: Partial<Pick<AppearanceSettings, 'theme' | 'ui_scale' | 'animations'>>) {
    Object.assign(appearanceDraft, values)
    if (!busy.value) void saveSettings()
  }

  function setSimpleAccent(color: string) {
    appearanceDraft.accent_color = color
    appearanceDraft.color_tokens = { ...appearanceDraft.color_tokens, accent: color, accent_strong: color }
    if (!busy.value) void saveSettings()
  }

  return {
    availableThemes,
    languageOptions,
    setLanguageFromSettings,
    fontFamilyCss,
    settingsSaveState,
    settingsSaveMessage,
    beginSettingsHydration,
    finishSettingsHydration,
    resolvedGlobalTheme,
    currentTheme,
    currentLayout,
    effectiveColorTokens,
    effectiveAccent,
    wallpaperUrl,
    wallpaperDisplayUrl,
    accentForeground,
    sidebarWidth,
    sidebarHeight,
    activeLauncherProfile,
    effectiveAnimationStyle,
    effectiveAnimationDuration,
    appStyle,
    toastThemeStyle,
    setInstanceLayout,
    mergeSavedSettingsBaseline,
    updateSavedSettingsBaseline,
    settingsProfiles,
    settingsProfilesLoading,
    settingsProfileBusy,
    settingsProfileName,
    backupDestinationOptions,
    loadSettingsProfiles,
    formatDateShort,
    saveSettingsProfileAs,
    overwriteSettingsProfile,
    applySettingsProfile,
    deleteSettingsProfile,
    exportSettingsProfile,
    importSettingsProfile,
    updateSidebarWidth,
    stopSidebarResize,
    startSidebarResize,
    setProfileArguments,
    addLauncherProfile,
    removeLauncherProfile,
    scrollSettingsSection,
    saveSettings,
    addResourceCatalog,
    removeResourceCatalog,
    addPluginCatalog,
    removePluginCatalog,
    selectActiveCatalog,
    setSimpleAppearance,
    setSimpleAccent,
  }
}
