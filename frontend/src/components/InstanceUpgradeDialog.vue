<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { bridge } from '../services/bridge'
import Icon from './Icon.vue'
import CustomSelect from './CustomSelect.vue'
import type { InstanceInfo, InstanceUpgradeMod, InstanceUpgradePlan, InstanceUpgradeReport, LoaderId } from '../types'

const props = defineProps<{ instance: InstanceInfo }>()
const emit = defineEmits<{
  (event: 'close'): void
  (event: 'toast', message: string): void
  (event: 'changed'): void
}>()

type VersionOption = { value: string; label: string }
type LoaderChoice = 'keep' | 'vanilla' | 'fabric' | 'quilt' | 'forge' | 'neoforge'

const step = ref<'setup' | 'plan' | 'applying' | 'done'>('setup')
const error = ref('')
const versions = ref<VersionOption[]>([])
const versionsLoading = ref(true)
const targetVersion = ref('')
const loader = ref<LoaderChoice>('keep')
const loaderVersions = ref<VersionOption[]>([])
const loaderVersion = ref('')
const loaderVersionsLoading = ref(false)
const backupWorlds = ref(true)
const plan = ref<InstanceUpgradePlan | null>(null)
const planLoading = ref(false)
const report = ref<InstanceUpgradeReport | null>(null)

const currentLoader = computed(() => {
  const value = props.instance.loader?.trim().toLowerCase()
  return value && value !== '' ? value : 'vanilla'
})
const currentLoaderTitle = computed(() => {
  const entry = LOADER_TITLES[currentLoader.value]
  return entry ?? currentLoader.value
})

const LOADER_TITLES: Record<string, string> = {
  vanilla: 'Vanilla',
  fabric: 'Fabric Loader',
  quilt: 'Quilt Loader',
  forge: 'Forge',
  neoforge: 'NeoForge',
}

const loaderOptions = computed<VersionOption[]>(() => [
  { value: 'keep', label: `Текущий: ${currentLoaderTitle.value}` },
  { value: 'vanilla', label: 'Vanilla · без модлоадера' },
  { value: 'fabric', label: 'Fabric' },
  { value: 'quilt', label: 'Quilt' },
  { value: 'forge', label: 'Forge' },
  { value: 'neoforge', label: 'NeoForge' },
])

const effectiveLoader = computed<LoaderId>(() =>
  loader.value === 'keep' ? (currentLoader.value as LoaderId) : loader.value,
)

const directionTitle = computed(() => {
  switch (plan.value?.direction) {
    case 'upgrade':
      return 'Обновление'
    case 'downgrade':
      return 'Откат на старую версию'
    case 'same':
      return 'Версия не меняется'
    default:
      return 'Смена версии'
  }
})

const directionBadge = computed(() => {
  switch (plan.value?.direction) {
    case 'upgrade':
      return 'upg-direction--upgrade'
    case 'downgrade':
      return 'upg-direction--downgrade'
    default:
      return 'upg-direction--same'
  }
})

const incompatibleMods = computed(() => plan.value?.mods.incompatible ?? [])
const unknownMods = computed(() => plan.value?.mods.unknown ?? [])
const okCount = computed(() => plan.value?.mods.ok.length ?? 0)
const activeCount = computed(() => plan.value?.mods.active ?? 0)

function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 Б'
  const units = ['Б', 'КиБ', 'МиБ', 'ГиБ', 'ТиБ']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  const digits = unit === 0 ? 0 : value >= 100 ? 0 : 1
  return `${value.toFixed(digits)} ${units[unit]}`
}

function javaText(java: number | null): string {
  return java ? `Java ${java}` : 'не определена'
}

function loaderLine(planValue: InstanceUpgradePlan, key: 'current' | 'target'): string {
  const side = planValue[key]
  const loaderTitle = side.loader_title || LOADER_TITLES[side.loader] || side.loader
  const loaderVersion = side.loader_version ? ` ${side.loader_version}` : ''
  return `${side.version} · ${loaderTitle}${loaderVersion || (side.loader === 'vanilla' ? '' : '')}`
}

async function loadVersions() {
  versionsLoading.value = true
  try {
    const payload = await bridge.versions(true)
    versions.value = payload.versions
      .filter((value) => value.type === 'release')
      .slice(0, 60)
      .map((value) => ({ value: value.id, label: value.id }))
  } catch (loadError) {
    error.value = loadError instanceof Error ? loadError.message : String(loadError)
  } finally {
    versionsLoading.value = false
  }
}

async function refreshLoaderVersions() {
  loaderVersions.value = []
  loaderVersion.value = ''
  if (effectiveLoader.value === 'vanilla') return
  loaderVersionsLoading.value = true
  try {
    const result = await bridge.loaderChoices(effectiveLoader.value, targetVersion.value || props.instance.minecraft_version)
    loaderVersions.value = result.choices.slice(0, 30).map((value) => ({ value: value.version, label: value.version }))
    if (loaderVersions.value.length) loaderVersion.value = loaderVersions.value[0].value
  } catch {
    loaderVersions.value = []
  } finally {
    loaderVersionsLoading.value = false
  }
}

async function onLoaderChange() {
  if (step.value !== 'setup') return
  await refreshLoaderVersions()
}

function versionChanged() {
  if (effectiveLoader.value !== 'vanilla') void refreshLoaderVersions()
}

function canRequestPlan(): boolean {
  if (!targetVersion.value) return false
  if (plan.value) return true
  return targetVersion.value !== props.instance.minecraft_version || loader.value !== 'keep'
}

async function requestPlan() {
  if (!targetVersion.value) {
    emit('toast', 'Выберите целевую версию Minecraft')
    return
  }
  if (targetVersion.value === props.instance.minecraft_version && loader.value === 'keep') {
    emit('toast', 'Выберите другую версию или другой модлоадер')
    return
  }
  error.value = ''
  planLoading.value = true
  try {
    plan.value = await bridge.instanceUpgradePlan(props.instance.name, {
      version: targetVersion.value,
      loader: effectiveLoader.value,
      loader_version: loaderVersion.value || undefined,
    })
    step.value = 'plan'
  } catch (planError) {
    error.value = planError instanceof Error ? planError.message : String(planError)
    emit('toast', error.value)
  } finally {
    planLoading.value = false
  }
}

async function applyUpgrade() {
  if (!plan.value) return
  step.value = 'applying'
  error.value = ''
  try {
    report.value = await bridge.instanceUpgradeApply(props.instance.name, {
      version: targetVersion.value,
      loader: effectiveLoader.value,
      loader_version: loaderVersion.value || undefined,
      backup_worlds: backupWorlds.value,
    })
    step.value = 'done'
    emit('changed')
    emit('toast', `Экземпляр обновлён до ${report.value.minecraft_version}`)
  } catch (applyError) {
    error.value = applyError instanceof Error ? applyError.message : String(applyError)
    step.value = 'plan'
    emit('toast', error.value)
  }
}

function finish() {
  emit('close')
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close')
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown)
  void loadVersions()
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
})

defineExpose({})

function modTitle(entry: InstanceUpgradeMod): string {
  return entry.title || entry.mod_id || entry.file
}
</script>

<template>
  <div class="upg-scrim" role="presentation" @click.self="emit('close')">
    <section class="upg-card" role="dialog" aria-modal="true" aria-label="Обновление версии экземпляра">
      <header class="upg-head">
        <div>
          <h2>Обновление версии</h2>
          <small class="upg-hint">Смена версии Minecraft с сохранением миров, модов и конфигов · {{ instance.name }}</small>
        </div>
        <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')">
          <Icon name="close" :size="16" />
        </button>
      </header>

      <div v-if="step === 'setup'" class="upg-body">
        <div class="upg-field">
          <span class="upg-field-label">Целевая версия Minecraft</span>
          <CustomSelect
            v-model="targetVersion"
            :options="versions"
            :disabled="versionsLoading"
            aria-label="Целевая версия Minecraft"
            @update:model-value="versionChanged"
          />
          <small class="upg-field-help">{{ versionsLoading ? 'Загружаю список версий…' : `Сейчас: ${instance.minecraft_version}` }}</small>
        </div>
        <div class="upg-field">
          <span class="upg-field-label">Модлоадер</span>
          <CustomSelect v-model="loader" :options="loaderOptions" aria-label="Модлоадер" @update:model-value="onLoaderChange" />
        </div>
        <div v-if="effectiveLoader !== 'vanilla'" class="upg-field">
          <span class="upg-field-label">Версия модлоадера</span>
          <CustomSelect
            v-model="loaderVersion"
            :options="loaderVersions"
            :disabled="loaderVersionsLoading || !loaderVersions.length"
            aria-label="Версия модлоадера"
          />
          <small class="upg-field-help">{{ loaderVersionsLoading ? 'Загружаю сборки…' : loaderVersions.length ? 'Обычно подходит свежая версия.' : 'Сборки для этой версии не найдены.' }}</small>
        </div>
        <label class="upg-check">
          <input v-model="backupWorlds" type="checkbox" />
          <span>
            <strong>Резервные копии миров перед обновлением</strong>
            <small>Копия уйдёт во вкладку «Резервные копии миров».</small>
          </span>
        </label>
        <p class="upg-note">Сначала показываю план: что поменяется и какие моды не переживут смену версии. Моды, миры и конфиги не удаляются.</p>
      </div>

      <div v-else-if="step === 'plan' && plan" class="upg-body">
        <div class="upg-route">
          <span class="upg-route-side">
            <strong>{{ plan.current.version }}</strong>
            <small>{{ plan.current.loader_title }} {{ plan.current.loader_version }} · {{ javaText(plan.current.java) }}</small>
          </span>
          <Icon name="chevron" :size="16" />
          <span class="upg-route-side">
            <strong>{{ plan.target.version }}</strong>
            <small>{{ plan.target.loader_title }} {{ plan.target.loader_version || 'свежая' }} · {{ javaText(plan.target.java) }}</small>
          </span>
          <span class="upg-direction" :class="directionBadge">{{ directionTitle }}</span>
        </div>

        <div class="upg-grid">
          <div class="upg-tile">
            <span class="upg-tile-head"><Icon name="folder" :size="14" />Миры</span>
            <strong>{{ plan.worlds.count }}</strong>
            <small>{{ formatSize(plan.worlds.size_bytes) }} · переносятся целиком</small>
          </div>
          <div class="upg-tile">
            <span class="upg-tile-head"><Icon name="mods" :size="14" />Моды</span>
            <strong>{{ activeCount }}</strong>
            <small>{{ okCount }} совместимы · {{ incompatibleMods.length }} несовместимы · {{ unknownMods.length }} неизвестно{{ plan.mods.disabled ? ` · ${plan.mods.disabled} выключено` : '' }}</small>
          </div>
          <div class="upg-tile">
            <span class="upg-tile-head"><Icon name="archive" :size="14" />Ресурсы</span>
            <strong>{{ plan.resources.resourcepacks.length + plan.resources.shaders.length }}</strong>
            <small>ресурспаки {{ plan.resources.resourcepacks.length }} · шейдеры {{ plan.resources.shaders.length }} · датапаки {{ plan.resources.datapacks }}</small>
          </div>
        </div>

        <div v-if="plan.warnings.length" class="upg-warnings">
          <div v-for="warning in plan.warnings" :key="warning" class="upg-warning">
            <Icon name="warning" :size="14" />
            <span>{{ warning }}</span>
          </div>
        </div>

        <div v-if="incompatibleMods.length" class="upg-list-block upg-list-block--bad">
          <span class="upg-list-title">Сломаются и уйдут в карантин</span>
          <ul class="upg-list">
            <li v-for="entry in incompatibleMods" :key="entry.file">
              <strong>{{ modTitle(entry) }}</strong>
              <small>{{ entry.file }} · {{ entry.reason }}</small>
            </li>
          </ul>
        </div>
        <div v-if="unknownMods.length" class="upg-list-block upg-list-block--warn">
          <span class="upg-list-title">Проверить вручную</span>
          <ul class="upg-list">
            <li v-for="entry in unknownMods" :key="entry.file">
              <strong>{{ modTitle(entry) }}</strong>
              <small>{{ entry.file }} · {{ entry.reason }}</small>
            </li>
          </ul>
        </div>

        <div class="upg-steps">
          <span class="upg-list-title">Что произойдёт</span>
          <ol class="upg-steps-list">
            <li v-for="entry in plan.steps" :key="entry">{{ entry }}</li>
          </ol>
        </div>

        <div v-if="plan.no_changes" class="upg-warnings">
          <div class="upg-warning">
            <Icon name="info" :size="14" />
            <span>Эта версия с этим модлоадером уже установлена: обновлять нечего.</span>
          </div>
        </div>
      </div>

      <div v-else-if="step === 'applying'" class="upg-body upg-body--busy">
        <Icon name="refresh" :size="22" />
        <strong>Обновляю до {{ targetVersion }}</strong>
        <small>Скачиваю файлы версии и модлоадера, это может занять несколько минут.</small>
      </div>

      <div v-else-if="step === 'done' && report" class="upg-body">
        <div class="upg-done-head">
          <Icon name="check" :size="20" />
          <div>
            <strong>Готово: {{ report.minecraft_version }}</strong>
            <small>Профиль версии: {{ report.version_id }}</small>
          </div>
        </div>
        <div class="upg-done-grid">
          <div class="upg-tile">
            <span class="upg-tile-head"><Icon name="folder" :size="14" />Бэкап миров</span>
            <strong>{{ report.backup ? report.backup.backup_id : 'не создавался' }}</strong>
            <small v-if="report.backup">{{ report.backup.files }} файлов · {{ formatSize(report.backup.size_bytes) }}</small>
            <small v-else>миров в экземпляре не было</small>
          </div>
          <div class="upg-tile">
            <span class="upg-tile-head"><Icon name="mods" :size="14" />В карантин</span>
            <strong>{{ report.moved_mods.length }}</strong>
            <small v-if="report.moved_mods.length">{{ report.moved_mods.join(', ') }}</small>
            <small v-else>все моды совместимы</small>
          </div>
        </div>
        <p v-if="report.warnings.length" class="upg-note upg-note--warn">
          {{ report.warnings.join(' ') }}
        </p>
      </div>

      <footer class="upg-foot">
        <template v-if="step === 'setup'">
          <button class="button button--outline" type="button" :disabled="planLoading" @click="emit('close')">Отмена</button>
          <button class="button button--accent" type="button" :disabled="planLoading || !canRequestPlan()" @click="requestPlan">
            <Icon name="shield" :size="14" />{{ planLoading ? 'Считаю план…' : 'Проверить план' }}
          </button>
        </template>
        <template v-else-if="step === 'plan' && plan">
          <button class="button button--outline" type="button" :disabled="planLoading" @click="step = 'setup'">Назад</button>
          <button class="button button--accent" type="button" :disabled="plan.no_changes" @click="applyUpgrade">
            <Icon name="download" :size="14" />Обновить
          </button>
        </template>
        <template v-else-if="step === 'done'">
          <button class="button button--accent" type="button" @click="finish">Готово</button>
        </template>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.upg-scrim {
  position: fixed;
  inset: 0;
  z-index: 90;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(4, 6, 10, 0.55);
}
.upg-card {
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: min(920px, 96%);
  max-height: min(860px, 94vh);
  border-radius: var(--ds-radius-dialog, 16px);
  background: var(--ds-surface-1);
  border: 1px solid var(--ds-border);
  box-shadow: var(--ds-shadow-dialog, 0 24px 64px rgba(0, 0, 0, 0.35));
  padding: 14px 16px;
  animation: upg-card-in 0.34s cubic-bezier(0.22, 1, 0.36, 1) both;
}
.upg-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
  animation: upg-fade-up 0.4s cubic-bezier(0.22, 1, 0.36, 1) 0.06s both;
}
.upg-head h2 {
  margin: 0;
  font-size: 15.5px;
  font-weight: 650;
}
.upg-hint {
  color: var(--muted, inherit);
  font-size: 11.5px;
}
.upg-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
  overflow-y: auto;
  min-height: 0;
  padding-right: 4px;
  animation: upg-fade-up 0.4s cubic-bezier(0.22, 1, 0.36, 1) 0.1s both;
}
.upg-body--busy {
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 48px 0;
  text-align: center;
}
.upg-field {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.upg-field-label {
  font-size: 12.5px;
  font-weight: 600;
}
.upg-field-help {
  color: var(--muted, inherit);
  font-size: 11.5px;
}
.upg-check {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  cursor: pointer;
}
.upg-check input {
  appearance: none;
  width: 18px;
  height: 18px;
  margin: 2px 0 0;
  border-radius: 6px;
  border: 1px solid var(--ds-border);
  background: var(--ds-surface-2);
  cursor: pointer;
  flex-shrink: 0;
}
.upg-check input:checked {
  background: var(--accent, var(--ds-accent, #6b7a3f));
  border-color: var(--accent, var(--ds-accent, #6b7a3f));
}
.upg-check input:checked::after {
  content: '';
  display: block;
  width: 5px;
  height: 9px;
  margin: 2px auto 0;
  border: solid #fff;
  border-width: 0 2px 2px 0;
  transform: rotate(45deg);
}
.upg-check span {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.upg-check small {
  color: var(--muted, inherit);
}
.upg-note {
  margin: 0;
  color: var(--muted, inherit);
  font-size: 12px;
  line-height: 1.45;
}
.upg-note--warn {
  color: var(--warning, var(--accent, inherit));
}
.upg-route {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  padding: 10px 12px;
  border-radius: 12px;
  background: var(--ds-surface-2);
  border: 1px solid var(--ds-border);
}
.upg-route-side {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.upg-route-side strong {
  font-size: 14px;
}
.upg-route-side small {
  color: var(--muted, inherit);
  font-size: 11px;
}
.upg-direction {
  margin-left: auto;
  padding: 3px 10px;
  border-radius: 999px;
  font-size: 11.5px;
  font-weight: 600;
  border: 1px solid var(--ds-border);
  color: var(--muted, inherit);
}
.upg-direction--upgrade {
  color: var(--success, var(--accent, inherit));
  border-color: var(--success, var(--accent, var(--ds-border)));
}
.upg-direction--downgrade {
  color: var(--danger, #e5484d);
  border-color: var(--danger, #e5484d);
}
.upg-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 8px;
}
.upg-tile {
  display: flex;
  flex-direction: column;
  gap: 3px;
  padding: 10px 12px;
  border-radius: 12px;
  background: var(--ds-surface-2);
  border: 1px solid var(--ds-border);
}
.upg-tile-head {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 11.5px;
  color: var(--muted, inherit);
}
.upg-tile strong {
  font-size: 14px;
}
.upg-tile small {
  color: var(--muted, inherit);
  font-size: 11px;
  line-height: 1.35;
}
.upg-warnings {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.upg-warning {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 8px 10px;
  border-radius: 10px;
  background: var(--ds-surface-2);
  border: 1px solid var(--ds-border);
  font-size: 12.5px;
  line-height: 1.4;
}
.upg-warning svg {
  flex-shrink: 0;
  margin-top: 2px;
}
.upg-list-block {
  padding: 10px 12px;
  border-radius: 12px;
  border: 1px solid var(--ds-border);
}
.upg-list-block--bad {
  border-color: var(--danger, #e5484d);
}
.upg-list-block--warn {
  border-color: var(--warning, var(--ds-border));
}
.upg-list-title {
  font-size: 12.5px;
  font-weight: 650;
}
.upg-list {
  margin: 6px 0 0;
  padding: 0 0 0 18px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  max-height: 180px;
  overflow-y: auto;
}
.upg-list li {
  display: flex;
  flex-direction: column;
  font-size: 12.5px;
}
.upg-list small {
  color: var(--muted, inherit);
  font-size: 11px;
}
.upg-steps {
  padding: 10px 12px;
  border-radius: 12px;
  background: var(--ds-surface-2);
  border: 1px solid var(--ds-border);
}
.upg-steps-list {
  margin: 6px 0 0;
  padding: 0 0 0 18px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12.5px;
  line-height: 1.4;
}
.upg-done-head {
  display: flex;
  align-items: center;
  gap: 10px;
}
.upg-done-head div {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.upg-done-head small {
  color: var(--muted, inherit);
}
.upg-done-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}
.upg-foot {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  animation: upg-fade-up 0.4s cubic-bezier(0.22, 1, 0.36, 1) 0.14s both;
}
@keyframes upg-card-in {
  from {
    opacity: 0;
    transform: translateY(14px) scale(0.985);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
@keyframes upg-fade-up {
  from {
    opacity: 0;
    transform: translateY(8px);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
@media (prefers-reduced-motion: reduce) {
  .upg-scrim,
  .upg-card,
  .upg-head,
  .upg-body,
  .upg-foot {
    animation: none;
  }
}
</style>
