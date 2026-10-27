<script setup lang="ts">
import { proxyRefs, ref } from 'vue'
import CatalogBadge from './CatalogBadge.vue'
import CloudStoragePanel from './CloudStoragePanel.vue'
import CustomSelect from './CustomSelect.vue'
import HotkeyInput from './HotkeyInput.vue'
import Icon from './Icon.vue'
import MemoryField from './MemoryField.vue'
import NumericStepper from './NumericStepper.vue'
import PluginViewRenderer from './PluginViewRenderer.vue'
import { bridge } from '../services/bridge'
import { overlaySelfTest, type OverlaySelfTestStep } from '../services/overlay'
import { createLauncherUpdates } from '../services/launcherUpdates'
import type { AppState, LauncherProfile, PluginCatalog, ResourceCatalog, SelectOption } from '../types'

type SettingsPageDraft = Record<string, any> & {
  download_source: string
  launcher_profiles: LauncherProfile[]
  plugin_catalogs: PluginCatalog[]
  resource_catalogs: ResourceCatalog[]
}

const props = defineProps<{ model: Record<string, any> & { settingsDraft: SettingsPageDraft; state: AppState } }>()
const model = proxyRefs(props.model)
const javaValidation = ref('')
const overlaySelfTestReport = ref<OverlaySelfTestStep[]>([])
const overlaySelfTestBusy = ref(false)
const updaterBusy = ref(false)
const updaterStatus = ref('Самообновление не проверялось в этой сессии.')
const updaterVersion = ref('')
const updaterNotes = ref('')
const updaterProgressDownloaded = ref(0)
const updaterProgressTotal = ref(0)
// без latest.json обновление открывается по ручной ссылке
const updaterManualUrl = ref('')
const { checkLauncherUpdates, installLauncherUpdate } = createLauncherUpdates({
  updaterBusy,
  updaterStatus,
  updaterVersion,
  updaterNotes,
  updaterProgressDownloaded,
  updaterProgressTotal,
  updaterManualUrl,
  showToast: (message, tone) => model.showToast(message, tone),
  errorMessage: (error) => model.errorMessage(error),
})

const fileExplorerOptions: SelectOption[] = [
  { value: 'builtin', label: 'Встроенный' },
  { value: 'system', label: 'Системный' },
]
const fileEditorOptions: SelectOption[] = [...fileExplorerOptions]
const jvmPresetOptions: SelectOption[] = [
  { value: 'none', label: 'Без пресета', meta: 'Аргументы только из профиля' },
  { value: 'fps', label: 'Высокий FPS', meta: 'Меньше пауз сборки мусора' },
  { value: 'balanced', label: 'Сбалансированный', meta: 'Универсальный вариант' },
  { value: 'heavy_modpack', label: 'Тяжёлый модпак', meta: 'Флаги Aikar для больших сборок' },
  { value: 'compatibility', label: 'Совместимость', meta: 'Старые сборки и моды' },
  { value: 'streaming', label: 'Запись и трансляция', meta: 'Стабильные кадры записи' },
]
const recordingEncoderOptions: SelectOption[] = [
  { value: 'auto', label: 'Автоматически', meta: 'NVENC → VAAPI → x264' },
  { value: 'nvenc', label: 'NVENC', meta: 'Аппаратно, NVIDIA' },
  { value: 'vaapi', label: 'VAAPI', meta: 'Аппаратно, Intel/AMD' },
  { value: 'x264', label: 'libx264', meta: 'Процессор, очень быстрый пресет' },
]
const recordingContainerOptions: SelectOption[] = [
  { value: 'mp4', label: 'MP4', meta: 'Универсальный' },
  { value: 'mkv', label: 'MKV', meta: 'Устойчив к прерываниям' },
]
const overlayPositionOptions: SelectOption[] = [
  { value: 'top-left', label: 'Слева сверху', meta: 'Как в Afterburner' },
  { value: 'top-right', label: 'Справа сверху', meta: '' },
  { value: 'bottom-left', label: 'Слева снизу', meta: '' },
  { value: 'bottom-right', label: 'Справа снизу', meta: '' },
]
const systemMetricsDisplayOptions: SelectOption[] = [
  { value: 'overlay', label: 'Только оверлей', meta: 'По горячей клавише в игре' },
  { value: 'always', label: 'Постоянно', meta: 'Полоса метрик в верхней панели лаунчера' },
]
const versionFilterOptions: SelectOption[] = [
  { value: 'release', label: 'Релизы', meta: 'Стабильные версии' },
  { value: 'snapshot', label: 'Снапшоты', meta: 'Недельные сборки и pre/rc' },
  { value: 'old_beta', label: 'Бета', meta: 'Исторические beta-версии' },
  { value: 'old_alpha', label: 'Альфа', meta: 'Исторические alpha-версии' },
  { value: 'all', label: 'Все версии', meta: 'Релизы, снапшоты и alpha/beta' },
]
const loginSourceOptions: SelectOption[] = [
  { value: 'microsoft', label: 'Microsoft OAuth', meta: 'Автоматический device-code вход' },
  { value: 'disabled', label: 'Отключён', meta: 'Скрыть внешний вход' },
]
const dataSourceOptions: SelectOption[] = [
  { value: 'mojang', label: 'Mojang metadata', meta: 'Официальный manifest по умолчанию' },
  { value: 'custom', label: 'Свой источник', meta: 'Совместимый version manifest' },
]

function autoRepairKindMuted(kind: string): boolean {
  return model.settingsDraft.auto_repair_muted.includes(kind)
}

function toggleAutoRepairKind(kind: string, enabled: boolean) {
  const rest = model.settingsDraft.auto_repair_muted.filter((item: string) => item !== kind)
  model.settingsDraft.auto_repair_muted = enabled ? rest : [...rest, kind]
}

function setProfileArguments(profile: LauncherProfile, key: 'jvm_arguments' | 'game_arguments', event: Event) {
  const value = (event.target as HTMLTextAreaElement).value
  profile[key] = value.split('\n').map((item) => item.trim()).filter(Boolean)
}

function scrollSettingsSection(id: string) {
  document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

async function validateJavaPath() {
  const path = model.settingsDraft.java_path.trim()
  if (!path) {
    javaValidation.value = 'Сначала укажите путь к Java executable.'
    return
  }
  try {
    const result = await bridge.javaValidate(path)
    model.settingsDraft.java_path = result.path
    model.javaSelection = 'manual'
    javaValidation.value = `Java ${result.version} найдена и готова к запуску.`
  } catch (error) {
    javaValidation.value = model.errorMessage(error)
  }
}

async function runOverlaySelfTest() {
  if (overlaySelfTestBusy.value) return
  overlaySelfTestBusy.value = true
  try {
    overlaySelfTestReport.value = await overlaySelfTest(model.settingsDraft.overlay_settings)
    const failed = overlaySelfTestReport.value.filter((step) => !step.ok).length
    model.showToast(failed === 0
      ? 'Диагностика оверлея: все шаги в порядке. Если окно не видно поверх игры — игре нужен оконный режим (F11).'
      : `Диагностика оверлея: проблемных шагов — ${failed}. Отчёт ниже в настройках.`, failed === 0 ? 'success' : 'danger')
  } catch (error) {
    model.showToast(`Диагностика оверлея прервалась: ${model.errorMessage(error)}`, 'danger')
  } finally {
    overlaySelfTestBusy.value = false
  }
}
</script>

<template>
             <section data-page-view="settings" class="page-content settings-page">
               <div class="section-intro">
                 <div>

                   <h2>Настройки</h2>
                   <p>Пути, память, источники и резервная копия файлов настраиваются здесь.</p>
                 </div>
                 <div class="section-intro-actions">
                   <button class="button button--outline" type="button" @click="model.replayLauncherTour"><Icon name="refresh" :size="15" />Повторить экскурсию</button>
                   <button class="button button--outline" type="button" @click="model.setView('themes')"><Icon name="palette" :size="15" />Центр настройки тем</button>
                 </div>
               </div>
               <div class="settings-save-bar" :class="`settings-save-bar--${model.settingsSaveState}`">
                 <div class="settings-save-copy"><span class="status-dot" :class="{ 'status-dot--green': model.settingsSaveState === 'saved', 'status-dot--busy': model.settingsSaveState === 'saving' }" /><div><strong>{{ model.settingsSaveState === 'saving' ? 'Сохраняю настройки' : model.settingsSaveState === 'error' ? 'Нужно исправление' : model.settingsSaveState === 'dirty' ? 'Есть несохранённые изменения' : 'Настройки сохранены' }}</strong><small>{{ model.settingsSaveMessage }}</small></div></div>
                 <button class="button" :class="model.settingsSaveState === 'dirty' ? 'button--accent' : 'button--quiet'" type="button" :disabled="model.busy || model.settingsSaveState === 'saved' || model.settingsSaveState === 'saving'" @click="void model.saveSettings()"><Icon name="check" :size="15" />{{ model.settingsSaveState === 'saving' ? 'Сохраняю…' : model.settingsSaveState === 'saved' ? 'Сохранено' : 'Сохранить изменения' }}</button>
               </div>
               <nav class="settings-section-nav" aria-label="Разделы настроек">
                  <button type="button" @click="scrollSettingsSection('settings-system')">Среда запуска</button>
                 <button type="button" @click="scrollSettingsSection('settings-behavior')">Поведение</button>
                 <button type="button" @click="scrollSettingsSection('settings-auto-repair')">Авто-починка</button>
                 <button type="button" @click="scrollSettingsSection('settings-hotkeys')">Горячие клавиши</button>
                 <button type="button" @click="scrollSettingsSection('settings-overlay')">Оверлей</button>
                 <button type="button" @click="scrollSettingsSection('settings-metrics')">Метрики</button>
                 <button type="button" @click="scrollSettingsSection('settings-sources')">Источники</button>
                 <button type="button" @click="scrollSettingsSection('settings-profiles')">Профили запуска</button>
                 <button type="button" @click="scrollSettingsSection('settings-catalogs')">Каталоги</button>
                 <button type="button" @click="scrollSettingsSection('settings-cloud')">Облако</button>
               </nav>
               <div class="settings-masonry">
                 <section id="settings-system" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Среда запуска</h2></div></div>
                   <div class="settings-form">
                     <div class="field"><span>Язык интерфейса</span><CustomSelect :model-value="model.settingsDraft.language ?? 'ru'" :options="model.languageOptions" @update:model-value="model.setLanguageFromSettings" /></div>
                     <label class="field"><span>Папка игры</span><input v-model="model.settingsDraft.game_dir" :placeholder="model.defaultGameDirectory" /></label>
                     <div class="field">
                       <span>Java runtime <button class="text-button" type="button" @click="model.refreshJavaOptions">Обновить список</button></span>
                       <CustomSelect v-model="model.javaSelection" :options="model.javaChoiceOptions" />
                     </div>
                     <div class="field"><span>Параллельные загрузки</span><NumericStepper v-model="model.settingsDraft.max_downloads" :min="1" :max="32" aria-label="Параллельные загрузки" /></div>
                     <label v-if="model.javaSelection === 'manual'" class="field field--wide">
                       <span>Путь к Java executable <small>можно указать JDK/JRE или бинарник</small></span>
                       <input v-model="model.settingsDraft.java_path" placeholder="/usr/lib/jvm/.../bin/java" />
                       <div class="java-path-actions"><button class="button button--small button--quiet" type="button" @click="validateJavaPath">Проверить Java</button><span v-if="javaValidation" class="field-help">{{ javaValidation }}</span></div>
                     </label>
                     <div class="field field--wide memory-control">
                       <span>Память JVM <small>максимум {{ model.memoryTotalLabel }}</small></span>
                       <div class="memory-input-row">
                         <NumericStepper :model-value="model.memoryValue" :min="1" :max="model.memoryMaxValue || 8192" :step="model.memoryUnit === 'GiB' ? 1 : 128" :decimals="model.memoryUnit === 'GiB' ? 2 : 0" aria-label="Память JVM" @update:model-value="model.memoryValue = $event; model.clampMemoryDraft()" />
                         <CustomSelect v-model="model.memoryUnit" :options="model.memoryUnitOptions" />
                       </div>
                       <small class="field-help">До {{ model.memoryMaxValue || '—' }} {{ model.memoryUnit }} · рекомендуем {{ model.formatMemoryLabel(model.state.settings.memory_recommended_kib) }}</small>
                     </div>
                     <label class="field field--wide"><span>Прокси <small>HTTP/HTTPS или SOCKS5, если сеть блокирует каталоги</small></span><input v-model="model.settingsDraft.http_proxy" placeholder="http://127.0.0.1:порт или socks5://127.0.0.1:1080" /><small class="field-help">SOCKS5 резолвит имена на стороне прокси; авторизация socks5://user:pass@host:port поддерживается.</small></label>
                     <div class="field field--wide">
                       <span>Graphics / GLX</span>
                       <CustomSelect v-model="model.settingsDraft.graphics_mode" :options="model.graphicsOptions" />
                       <small class="field-help">Auto включает NVIDIA PRIME, если nvidia-smi видит видеокарту; иначе используется системный GLX. Экземпляры с Auto наследуют эту настройку.</small>
                     </div>
                     <div class="diagnostic-note">
                       <span class="status-dot" :class="{ 'status-dot--green': model.state.system.nvidia_smi }" />
                       <span>{{ model.state.system.session }} · {{ model.state.system.display }} · {{ model.state.system.nvidia_smi ? 'nvidia-smi найден' : 'nvidia-smi не найден' }}</span>
                       <button class="text-button" type="button" @click="model.loadGraphicsDiagnostics">Проверить GLX</button>
                     </div>
                     <pre v-if="model.graphicsReport" class="diagnostic-output">{{ model.graphicsReport }}</pre>
                   </div>
                 </section>

                 <section id="settings-behavior" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Запуск и интерфейс</h2></div></div>
                   <div class="settings-form">
                     <div class="field"><span>Экран при запуске</span><CustomSelect v-model="model.settingsDraft.startup_view" :options="model.persistentViewOptions" /></div>
                     <div class="field"><span>Последний экран</span><CustomSelect v-model="model.settingsDraft.last_view" :options="model.persistentViewOptions" /></div>
                     <div class="field"><span>Версии по умолчанию</span><CustomSelect v-model="model.settingsDraft.default_version_filter" :options="versionFilterOptions" /></div>
                     <div class="field"><span>Loader по умолчанию</span><CustomSelect v-model="model.settingsDraft.default_loader" :options="model.loaderOptions" /></div>
                     <div class="field"><span>Проводник файлов</span><CustomSelect v-model="model.settingsDraft.file_explorer" :options="fileExplorerOptions" /></div>
                     <div class="field"><span>Редактор текстовых файлов</span><CustomSelect v-model="model.settingsDraft.file_editor" :options="fileEditorOptions" /></div>
                     <div class="field field--wide"><span>Аккаунт по умолчанию</span><CustomSelect v-model="model.settingsDraft.default_account_id" :options="model.accountOptions" /></div>
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="model.settingsDraft.remember_last_view" type="checkbox" /><span><strong>Запоминать экран</strong><small>Возвращаться на последний открытый раздел</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.open_last_instance" type="checkbox" /><span><strong>Открывать последний экземпляр</strong><small>Сохранять рабочий контекст выбранного профиля</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.launch_after_install" type="checkbox" /><span><strong>Запускать после установки</strong><small>Автоматически открыть готовый экземпляр</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.minimize_on_launch" type="checkbox" /><span><strong>Сворачивать при запуске</strong><small>Скрыть окно лаунчера после старта игры</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.confirm_destructive_actions" type="checkbox" /><span><strong>Подтверждать опасные действия</strong><small>Удаление экземпляров, ресурсов, плагинов и аккаунтов</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.show_activity_panel" type="checkbox" /><span><strong>Показывать панель активности</strong><small>Логи, прогресс и отмена операций</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.show_toasts" type="checkbox" /><span><strong>Показывать уведомления</strong><small>Короткие сообщения о результате действий</small></span></label>
                     </div>
                   </div>
                 </section>

                 <section id="settings-auto-repair" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Авто-починка</h2></div></div>
                   <div class="settings-form">
                     <p class="field-help field--wide">Если игра упала или не запустилась, лаунчер разберёт лог, отчёт о краше и папку mods и предложит способ починки. Ничего не меняется без вашего согласия.</p>
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="model.settingsDraft.auto_repair_enabled" type="checkbox" /><span><strong>Предлагать починку после краша</strong><small>Показывать окно с проблемой и вариантами исправления</small></span></label>
                     </div>
                     <div class="field field--wide auto-repair-kinds">
                       <span>Какие проблемы предлагать чинить <small>Снятые галочки — то, для чего нажали «Больше не показывать»</small></span>
                       <div class="settings-toggle-grid">
                         <label v-for="kind in model.state.settings.auto_repair_kinds || []" :key="kind.id" class="workspace-check"><input type="checkbox" :checked="!autoRepairKindMuted(kind.id)" :disabled="!model.settingsDraft.auto_repair_enabled" @change="toggleAutoRepairKind(kind.id, ($event.target as HTMLInputElement).checked)" /><span><strong>{{ kind.title }}</strong></span></label>
                       </div>
                       <button v-if="model.settingsDraft.auto_repair_muted.length" class="button button--small button--outline" type="button" @click="model.settingsDraft.auto_repair_muted = []"><Icon name="undo" :size="13" />Снова показывать всё</button>
                     </div>
                   </div>
                 </section>

                 <section id="settings-downloads" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Загрузки и metadata</h2></div></div>
                   <div class="settings-form">
                     <div class="field"><span>Обновление metadata, часы</span><NumericStepper v-model="model.settingsDraft.metadata_refresh_hours" :min="1" :max="168" aria-label="Обновление metadata" /></div>
                     <div class="field"><span>Timeout загрузки, секунд</span><NumericStepper v-model="model.settingsDraft.download_timeout_seconds" :min="5" :max="120" aria-label="Timeout загрузки" /></div>
                     <div class="field field--wide"><span>Источник файлов Minecraft <small>Mojang, Forge, NeoForge, Fabric, Quilt</small></span><CustomSelect v-model="model.settingsDraft.download_source" :options="model.downloadSourceOptions" /><small class="field-help">Если официальный сервер не отвечает или обрывает загрузку, файл берётся с зеркала. Подлинность всех файлов проверяется по хешам.</small></div>
                     <div class="field"><span>Повторные попытки загрузки</span><NumericStepper v-model="model.settingsDraft.download_retries" :min="1" :max="6" aria-label="Повторные попытки загрузки" /></div>
                     <div class="field"><span>Хранить логи, дней <small>0 = без лимита</small></span><NumericStepper v-model="model.settingsDraft.keep_logs_days" :min="0" :max="365" aria-label="Хранение логов" /></div>
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="model.settingsDraft.auto_refresh_metadata" type="checkbox" /><span><strong>Обновлять metadata автоматически</strong><small>Проверять версии при открытии лаунчера</small></span></label>
                     </div>
                   </div>
                 </section>

                 <section id="settings-updates" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Самообновление</h2></div></div>
                   <div class="settings-form">
                     <div class="diagnostic-note diagnostic-note--stacked">
                       <span class="status-dot" :class="{ 'status-dot--busy': updaterBusy, 'status-dot--green': !updaterBusy && updaterVersion }" />
                       <div>
                         <div>{{ updaterStatus }}</div>
                         <small v-if="updaterVersion">К установке: {{ updaterVersion }}</small>
                         <small v-if="updaterProgressTotal">{{ updaterProgressDownloaded }} / {{ updaterProgressTotal }} байт</small>
                       </div>
                     </div>
                     <div class="settings-inline-actions">
                       <button class="button button--small button--quiet" type="button" :disabled="updaterBusy" @click="void checkLauncherUpdates()"><Icon name="refresh" :size="14" />{{ updaterBusy ? 'Проверяю…' : 'Проверить обновления' }}</button>
                       <button class="button button--small button--accent" type="button" :disabled="updaterBusy || !updaterVersion" @click="void installLauncherUpdate()"><Icon name="download" :size="14" />{{ updaterManualUrl ? 'Открыть страницу' : 'Установить' }}</button>
                     </div>
                     <small v-if="updaterNotes" class="field-help">{{ updaterNotes }}</small>
                   </div>
                 </section>

                 <section id="settings-privacy" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Телеметрия</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="model.settingsDraft.telemetry" type="checkbox" /><span><strong>Телеметрия</strong><small>Выключено по умолчанию</small></span></label>
                     </div>
                     <label class="field field--wide"><span>Crash-report endpoint <small>Sentry-compatible webhook или self-hosted collector</small></span><input v-model="model.settingsDraft.telemetry_endpoint" :disabled="!model.settingsDraft.telemetry" placeholder="https://errors.example.com/api/nivora-launcher/crash" /><small class="field-help">Отправляется только при включённой телеметрии и только для падений/неуспешных завершений Minecraft.</small></label>
                   </div>
                 </section>

                 <section id="settings-hotkeys" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Горячие клавиши</h2></div></div>
                   <div class="settings-form">
                     <!-- запись клавиш общая для настроек и оверлея -->
                     <div class="hotkey-grid">
                       <label v-for="shortcut in model.shortcutRows" :key="shortcut.key" class="field"><span>{{ shortcut.label }}</span><HotkeyInput v-model="model.settingsDraft.keyboard_shortcuts[shortcut.key]" :default-hint="`по умолчанию ${shortcut.default}`" /></label>
                     </div>
                     <small class="field-help">Кликните по полю и нажмите нужную комбинацию — ввести текст руками нельзя.</small>
                   </div>
                 </section>

                 <section id="settings-backups" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Резервные копии по умолчанию</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="model.settingsDraft.backup_schedule.enabled" type="checkbox" /><span><strong>Копировать миры по расписанию</strong><small>Расписание для всех экземпляров; в экземпляре можно переопределить</small></span></label>
                     </div>
                     <div class="field"><span>Интервал, часов</span><NumericStepper v-model="model.settingsDraft.backup_schedule.interval_hours" :min="1" :max="720" :disabled="!model.settingsDraft.backup_schedule.enabled" aria-label="Интервал копий" /></div>
                     <div class="field"><span>Хранить копий</span><NumericStepper v-model="model.settingsDraft.backup_schedule.keep" :min="1" :max="50" :disabled="!model.settingsDraft.backup_schedule.enabled" aria-label="Количество копий" /></div>
                     <div class="field field--wide"><span>Куда</span><CustomSelect v-model="model.settingsDraft.backup_schedule.destination" :options="model.backupDestinationOptions" :disabled="!model.settingsDraft.backup_schedule.enabled" /></div>
                     <small class="field-help">Копируется только игровое содержимое (saves/ или world*), запущенные экземпляры пропускаются и копируются на следующем цикле. WebDAV использует облачный endpoint из раздела облака.</small>
                   </div>
                 </section>

                 <section id="settings-presets" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Профили настроек</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-profiles">
                       <div v-for="profile in model.settingsProfiles" :key="profile.name" class="settings-profile-row">
                         <span><strong>{{ profile.name }}</strong><small>{{ profile.key_count }} параметров · {{ profile.updated_at ? model.formatDateShort(profile.updated_at) : '—' }}</small></span>
                         <span>
                           <button class="button button--small button--quiet" type="button" :disabled="model.settingsProfileBusy" @click="model.applySettingsProfile(profile.name)">Применить</button>
                           <button class="button button--small button--quiet" type="button" :disabled="model.settingsProfileBusy" @click="model.overwriteSettingsProfile(profile.name)">Обновить</button>
                           <button class="button button--small button--quiet" type="button" :disabled="model.settingsProfileBusy" @click="model.exportSettingsProfile(profile.name)">Экспорт</button>
                           <button class="icon-button" type="button" aria-label="Удалить профиль" :disabled="model.settingsProfileBusy" @click="model.deleteSettingsProfile(profile.name)"><Icon name="trash" :size="14" /></button>
                         </span>
                       </div>
                       <span v-if="!model.settingsProfiles.length && !model.settingsProfilesLoading" class="muted">Профилей пока нет — сохраните текущие настройки как пресет.</span>
                       <span v-if="model.settingsProfilesLoading" class="muted">Загружаю…</span>
                       <div class="settings-profile-actions">
                         <input v-model="model.settingsProfileName" class="settings-profile-name" placeholder="Имя профиля" maxlength="48" />
                         <button class="button button--small button--accent" type="button" :disabled="model.settingsProfileBusy || !model.settingsProfileName.trim()" @click="model.saveSettingsProfileAs">{{ model.settingsProfileBusy ? 'Работаю…' : 'Сохранить пресет' }}</button>
                         <button class="button button--small button--outline" type="button" :disabled="model.settingsProfileBusy" @click="model.importSettingsProfile">Импорт YML</button>
                       </div>
                       <small class="field-help">Пресет — снимок всех настроек лаунчера, кроме папки игры и client_id: внешность, сеть, профили запуска. Экспорт отдаёт JSON-файл, импорт принимает его же.</small>
                     </div>
                   </div>
                 </section>

                 <section id="settings-overlay" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Игровой оверлей и запись</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.enabled" type="checkbox" /><span><strong>Оверлей в игре</strong><small>{{ model.settingsDraft.overlay_settings.hotkey_overlay }} открывает оверлей поверх Minecraft</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.show_mods" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled" /><span><strong>Виджет модов</strong><small>Список модов экземпляра в оверлее</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.show_metrics" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled" /><span><strong>Виджет метрик</strong><small>CPU, RAM и диск игры</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.recording_audio" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled" /><span><strong>Записывать звук</strong><small>Системный звук через PulseAudio</small></span></label>
                     </div>
                     <div class="hotkey-grid">
                       <label class="field"><span>Клавиша оверлея</span><HotkeyInput v-model="model.settingsDraft.overlay_settings.hotkey_overlay" :disabled="!model.settingsDraft.overlay_settings.enabled" default-hint="по умолчанию Shift+Tab" /></label>
                       <label class="field"><span>Клавиша записи</span><HotkeyInput v-model="model.settingsDraft.overlay_settings.hotkey_recording" :disabled="!model.settingsDraft.overlay_settings.enabled" default-hint="по умолчанию F7" /></label>
                     </div>
                     <div class="field"><span>Кодировщик</span><CustomSelect v-model="model.settingsDraft.overlay_settings.recording_encoder" :options="recordingEncoderOptions" :disabled="!model.settingsDraft.overlay_settings.enabled" /></div>
                     <div class="field"><span>Контейнер</span><CustomSelect v-model="model.settingsDraft.overlay_settings.recording_container" :options="recordingContainerOptions" :disabled="!model.settingsDraft.overlay_settings.enabled" /></div>
                     <div class="field"><span>FPS записи</span><NumericStepper v-model="model.settingsDraft.overlay_settings.recording_fps" :min="15" :max="120" :disabled="!model.settingsDraft.overlay_settings.enabled" aria-label="FPS записи" /></div>
                     <div class="field"><span>Качество <small>CRF/CQ: меньше — лучше</small></span><NumericStepper v-model="model.settingsDraft.overlay_settings.recording_quality" :min="15" :max="35" :disabled="!model.settingsDraft.overlay_settings.enabled" aria-label="Качество записи" /></div>
                     <div class="overlay-selftest">
                       <button class="button button--small button--outline" type="button" :disabled="overlaySelfTestBusy || !model.settingsDraft.overlay_settings.enabled" @click="() => { void runOverlaySelfTest() }"><Icon name="info" :size="14" />{{ overlaySelfTestBusy ? 'Проверяю…' : 'Диагностика оверлея' }}</button>
                       <pre v-if="overlaySelfTestReport.length" class="selftest-report">{{ overlaySelfTestReport.map((step) => `${step.ok ? '✓' : '✗'} ${step.name} — ${step.detail}`).join('\n') }}</pre>
                     </div>
                     <small class="field-help">Глобальные клавиши не работают в нативных Wayland-сессиях (в пакетной сборке лаунчер работает через XWayland — там клавиши работают) и в эксклюзивном полноэкранном режиме игры: там оверлей открывается кнопкой «Оверлей» в шапке и пунктом «Игровой оверлей» в меню значка Nivora в трее.</small>
                   </div>
                 </section>

                 <section id="settings-metrics" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Системные метрики и виджеты</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.show_system_metrics" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled" /><span><strong>Системные метрики</strong><small>CPU, GPU, RAM, диски и температуры</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.metrics_cpu" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled || !model.settingsDraft.overlay_settings.show_system_metrics" /><span><strong>CPU</strong><small>Загрузка процессора</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.metrics_ram" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled || !model.settingsDraft.overlay_settings.show_system_metrics" /><span><strong>RAM</strong><small>Занятая память</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.metrics_gpu" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled || !model.settingsDraft.overlay_settings.show_system_metrics" /><span><strong>GPU</strong><small>NVIDIA через nvidia-smi</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.metrics_disk" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled || !model.settingsDraft.overlay_settings.show_system_metrics" /><span><strong>Диски</strong><small>Заполнение разделов</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.metrics_temperatures" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled || !model.settingsDraft.overlay_settings.show_system_metrics" /><span><strong>Температуры</strong><small>Linux hwmon, NVIDIA</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.show_clock" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled" /><span><strong>Часы</strong><small>Текущее время в оверлее</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.show_chat" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled" /><span><strong>Быстрый чат сервера</strong><small>Команды и хвост консоли</small></span></label>
                       <label class="workspace-check"><input v-model="model.settingsDraft.overlay_settings.compact" type="checkbox" :disabled="!model.settingsDraft.overlay_settings.enabled" /><span><strong>Компактный режим</strong><small>Меньше отступов</small></span></label>
                     </div>
                     <div class="field"><span>Позиция оверлея</span><CustomSelect v-model="model.settingsDraft.overlay_settings.position" :options="overlayPositionOptions" :disabled="!model.settingsDraft.overlay_settings.enabled" /></div>
                     <div class="field"><span>Метрики вне игры</span><CustomSelect v-model="model.settingsDraft.overlay_settings.system_metrics_display" :options="systemMetricsDisplayOptions" /></div>
                     <div class="field"><span>Масштаб, %</span><NumericStepper v-model="model.settingsDraft.overlay_settings.scale" :min="80" :max="130" :disabled="!model.settingsDraft.overlay_settings.enabled" aria-label="Масштаб оверлея" /></div>
                     <div class="field"><span>Непрозрачность, %</span><NumericStepper v-model="model.settingsDraft.overlay_settings.opacity" :min="60" :max="100" :disabled="!model.settingsDraft.overlay_settings.enabled" aria-label="Непрозрачность оверлея" /></div>
                     <small class="field-help">Метрики вне игры: «Постоянно» показывает CPU, RAM, GPU и температуры в верхней панели лаунчера, «Только оверлей» — лишь по горячей клавише. Датчики и GPU зависят от системы: чего нет — не показываем.</small>
                   </div>
                 </section>

                 <section id="settings-sources" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Вход и данные версий</h2></div></div>
                   <div class="settings-form">
                     <div class="automatic-integration"><span class="status-dot" :class="{ 'status-dot--green': model.settingsDraft.login_source === 'microsoft' && model.state.settings.has_client_id }" /><div><strong>Microsoft OAuth</strong><span>{{ model.settingsDraft.login_source !== 'microsoft' ? 'Внешний вход отключён.' : model.state.settings.has_client_id ? 'Public client ID настроен в сборке; вручную вводить его не нужно.' : 'В этой сборке нет настроенного client ID; он должен поставляться вместе с лаунчером.' }}</span></div></div>
                     <div class="field"><span>Источник входа</span><CustomSelect v-model="model.settingsDraft.login_source" :options="loginSourceOptions" /></div>
                     <div class="field"><span>Источник данных</span><CustomSelect v-model="model.settingsDraft.data_source" :options="dataSourceOptions" /></div>
                     <label class="field field--wide"><span>Login API URL <small>база device-code и token endpoint</small></span><input v-model="model.settingsDraft.login_api_url" placeholder="https://login.microsoftonline.com/consumers/oauth2/v2.0" /></label>
                     <label class="field field--wide"><span>Data API URL <small>{{ model.settingsDraft.data_source === 'custom' ? 'совместимый version manifest' : 'официальный Mojang manifest по умолчанию' }}</small></span><input v-model="model.settingsDraft.data_api_url" placeholder="https://piston-meta.mojang.com/mc/game/version_manifest_v2.json" /></label>
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="model.settingsDraft.developer_mode" type="checkbox" /><span><strong>Режим разработчика</strong><small>Разрешает нестандартные HTTPS endpoint. Не включайте на рабочем аккаунте: токены OAuth могут уйти на указанный адрес.</small></span></label>
                     </div>
                     <div class="security-note"><Icon name="info" :size="18" /><span>Для распространения лаунчер должен содержать собственный Azure public-client ID. Это публичная настройка сборки, а не секрет и не поле для пользователя.</span></div>
                   </div>
                 </section>

                 <section v-if="model.settingsSectionViews.length" id="settings-plugins" class="panel settings-card">
                   <div class="panel-heading"><div><h2>Настройки плагинов</h2></div></div>
                   <div class="plugin-settings-sections">
                     <article v-for="entry in model.settingsSectionViews" :key="`${entry.pluginId}:${entry.view.id}`" class="plugin-settings-section">
                       <div class="workspace-card-heading"><div><h3>{{ entry.view.title }}</h3><span class="muted">{{ entry.pluginName }} · UI v2</span></div></div>
                       <PluginViewRenderer
                         :view="entry.view"
                         @action="(payload) => model.runPluginAction({ pluginId: entry.pluginId, ...payload })"
                         @config="(payload) => void model.savePluginConfig({ pluginId: entry.pluginId, key: payload.key, value: payload.value })"
                         @open-link="(url) => void model.openPluginLink({ pluginId: entry.pluginId, url })"
                       />
                     </article>
                   </div>
                 </section>
               </div>

               <section id="settings-profiles" class="panel settings-card settings-card--wide">
                 <div class="panel-heading">
                   <div><h2>Параметры игры</h2><p>Сохраните отдельные наборы памяти, Java, графики, окна и аргументов. Активный профиль применяется к кнопке «Играть».</p></div>
                   <button class="button button--small button--outline" type="button" @click="model.addLauncherProfile"><Icon name="plus" :size="14" />Новый профиль</button>
                 </div>
                 <div class="launcher-profile-toolbar">
                   <div class="field"><span>Активный профиль <small>{{ model.activeLauncherProfile?.name || 'не выбран' }}</small></span><CustomSelect v-model="model.settingsDraft.active_launcher_profile_id" :options="model.settingsDraft.launcher_profiles.map((profile) => ({ value: profile.id, label: profile.name, meta: `${profile.memory} · ${profile.graphics_mode}` }))" /></div>
                 </div>
                 <div class="launcher-profile-list">
                   <article v-for="profile in model.settingsDraft.launcher_profiles" :key="profile.id" class="launcher-profile-card" :class="{ 'launcher-profile-card--active': profile.id === model.settingsDraft.active_launcher_profile_id }">
                     <div class="launcher-profile-heading"><div><span class="profile-active-dot" :class="{ 'profile-active-dot--on': profile.id === model.settingsDraft.active_launcher_profile_id }" /><strong>{{ profile.name }}</strong><small>{{ profile.id }}</small></div><div class="launcher-profile-actions"><button class="button button--small button--quiet" type="button" @click="model.settingsDraft.active_launcher_profile_id = profile.id">Использовать</button><button v-if="model.settingsDraft.launcher_profiles.length > 1" class="icon-button" type="button" aria-label="Удалить профиль запуска" @click="model.removeLauncherProfile(profile.id)"><Icon name="trash" :size="15" /></button></div></div>
                     <div class="field-grid">
                       <label class="field"><span>Название</span><input v-model="profile.name" maxlength="80" /></label>
                       <div class="field"><span>Память JVM</span><MemoryField v-model="profile.memory" :max-kib="model.state.settings.memory_total_kib" /></div>
                       <label class="field"><span>Java executable</span><input v-model="profile.java_path" placeholder="Пусто = системная Java" /></label>
                       <div class="field"><span>Графика</span><CustomSelect v-model="profile.graphics_mode" :options="model.graphicsOptions" /></div>
                       <div class="field"><span>Аккаунт</span><CustomSelect v-model="profile.account_id" :options="model.accountOptions" /></div>
                       <div class="field"><span>Ширина окна</span><NumericStepper v-model="profile.window_width" :min="320" :max="8192" aria-label="Ширина окна" /></div>
                       <div class="field"><span>Высота окна</span><NumericStepper v-model="profile.window_height" :min="240" :max="8192" aria-label="Высота окна" /></div>
                       <label class="workspace-check"><input v-model="profile.fullscreen" type="checkbox" /><span><strong>Полноэкранный режим</strong><small>Добавляет --fullscreen к запуску.</small></span></label>
                       <label class="field"><span>Quick Play: мир</span><input v-model="profile.quick_play_world" placeholder="Имя сохранения" /></label>
                       <label class="field"><span>Quick Play: сервер</span><input v-model="profile.quick_play_server" placeholder="host:port" /></label>
                       <div class="field field--wide"><span>JVM-пресет оптимизации</span>
                         <CustomSelect :model-value="profile.jvm_preset ?? 'none'" :options="jvmPresetOptions" @update:model-value="profile.jvm_preset = $event === 'none' ? undefined : $event" />
                       </div>
                       <label class="field field--wide"><span>Дополнительные JVM-аргументы <small>по одному на строку</small></span><textarea :value="profile.jvm_arguments.join('\n')" rows="2" @input="setProfileArguments(profile, 'jvm_arguments', $event)" /></label>
                       <label class="field field--wide"><span>Дополнительные game-аргументы <small>по одному на строку</small></span><textarea :value="profile.game_arguments.join('\n')" rows="2" @input="setProfileArguments(profile, 'game_arguments', $event)" /></label>
                     </div>
                   </article>
                 </div>
               </section>

               <section id="settings-catalogs" class="panel settings-card settings-card--wide">
                 <div class="panel-heading"><div><h2>Каталоги</h2></div></div>
                 <div class="settings-catalog-columns">
                   <div class="settings-catalog-block">
                     <div class="source-divider"><span>Каталоги ресурсов</span><i /></div>
                     <div class="resource-catalog-toolbar">
                       <div class="field"><span>Активный каталог <small>используется в поиске и установке</small></span><CustomSelect v-model="model.settingsDraft.active_catalog_id" :options="model.resourceCatalogOptions" @update:model-value="model.selectActiveCatalog" /></div>
                       <button class="button button--small button--outline" type="button" @click="model.addResourceCatalog"><Icon name="plus" :size="14" />Добавить каталог</button>
                     </div>
                     <div class="resource-catalog-list">
                       <article v-for="catalog in model.settingsDraft.resource_catalogs" :key="catalog.id" class="resource-catalog-row">
                         <div class="field"><span class="field-heading-with-icon"><span>Название</span><CatalogBadge :kind="catalog.kind" compact /></span><input v-model="catalog.name" maxlength="60" /></div>
                         <div class="field"><span>Тип</span><CustomSelect v-model="catalog.kind" :options="model.catalogKindOptions" /></div>
                         <div class="field resource-catalog-url"><span>API URL <small>только HTTPS</small></span><input v-model="catalog.api_url" placeholder="https://api.modrinth.com/v2" /></div>
                         <button class="icon-button" type="button" aria-label="Удалить каталог" @click="model.removeResourceCatalog(catalog.id)"><Icon name="trash" :size="15" /></button>
                       </article>
                     </div>
                   </div>
                   <div class="settings-catalog-block">
                     <div class="source-divider"><span>Каталоги плагинов серверов</span><i /></div>
                     <div class="resource-catalog-toolbar"><div class="field"><span>Активный каталог <small>используется в server workspace</small></span><CustomSelect v-model="model.settingsDraft.active_plugin_catalog_id" :options="model.settingsDraft.plugin_catalogs.map((catalog) => ({ value: catalog.id, label: catalog.name, meta: catalog.kind }))" /></div><button class="button button--small button--outline" type="button" @click="model.addPluginCatalog"><Icon name="plus" :size="14" />Добавить каталог</button></div>
                     <div class="resource-catalog-list">
                       <article v-for="catalog in model.settingsDraft.plugin_catalogs" :key="catalog.id" class="resource-catalog-row">
                         <div class="field"><span>Название</span><input v-model="catalog.name" maxlength="80" /></div>
                         <div class="field"><span>Тип</span><CustomSelect v-model="catalog.kind" :options="model.pluginCatalogKindOptions" /></div>
                         <div class="field resource-catalog-url"><span>API URL <small>только HTTPS</small></span><input v-model="catalog.api_url" placeholder="https://api.modrinth.com/v2" /></div>
                         <label class="workspace-check"><input v-model="catalog.enabled" type="checkbox" /><span>Вкл.</span></label>
                         <button class="icon-button" type="button" aria-label="Удалить каталог плагинов" @click="model.removePluginCatalog(catalog.id)"><Icon name="trash" :size="15" /></button>
                       </article>
                     </div>
                   </div>
                 </div>
               </section>

               <div id="settings-cloud" class="settings-cloud-anchor">
                 <CloudStoragePanel
                   :url="model.settingsDraft.cloud_url"
                   :username="model.settingsDraft.cloud_username"
                   :remote-path="model.settingsDraft.cloud_remote_path"
                   :include-accounts="model.settingsDraft.cloud_include_accounts"
                   @saved="Object.assign(model.settingsDraft, $event)"
                   @toast="model.showToast"
                   @changed="model.refreshBootstrap"
                 />
               </div>
               <section class="panel paths-panel">
                 <div class="path-line"><span>Платформа</span><strong>{{ model.state.system.platform }} · {{ model.state.system.machine }}</strong></div>
                 <div class="path-line"><span>Движок</span><strong>{{ model.state.system.python || 'подключение…' }}</strong></div>
                 <button class="button button--quiet" type="button" @click="model.openGameFolder"><Icon name="folder" :size="16" />Открыть папку игры</button>
               </section>
             </section>
</template>
