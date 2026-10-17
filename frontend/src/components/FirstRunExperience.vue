<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { nicknameError } from '../services/nickname'
import launcherLogo from '../assets/uni.png'
import type { AccountInfo, ThemeMode } from '../types'
import { FIRST_RUN_SETUP_STAGES, type FirstRunPhase } from '../services/firstRunExperience'
import { useModalFocus } from '../services/modalFocus'

const props = defineProps<{
  phase: FirstRunPhase | null
  accounts: readonly AccountInfo[]
  selectedAccountId: string
  busy: boolean
  microsoftLoginAvailable: boolean
  error: string
  microsoftCode: string
  microsoftVerificationUri: string
  gameDirectory: string
  memoryGib: number
  recommendedMemoryGib: number
  maximumMemoryGib: number
  theme: ThemeMode
  graphicsMode: string
}>()

const emit = defineEmits<{
  (event: 'phase-change', phase: FirstRunPhase): void
  (event: 'pick-language', locale: 'ru' | 'en'): void
  (event: 'select-account', accountId: string): void
  (event: 'add-microsoft'): void
  (event: 'add-offline', name: string): void
  (event: 'save-basics', values: { gameDirectory: string; memory: string; theme: ThemeMode; graphicsMode: string }): void
  (event: 'finish-setup'): void
  (event: 'skip-setup'): void
  (event: 'finish-tour'): void
  (event: 'skip-tour'): void
}>()

const setupSteps = FIRST_RUN_SETUP_STAGES
const themeOptions: Array<{ value: ThemeMode; label: string; icon: string }> = [
  { value: 'system', label: 'Как в системе', icon: 'monitor' },
  { value: 'dark', label: 'Тёмная', icon: 'moon' },
  { value: 'light', label: 'Светлая', icon: 'sun' },
  { value: 'glass', label: 'Glass', icon: 'palette' },
]
const graphicsChoices = [
  { value: 'auto', label: 'Автоматически', detail: 'Лаунчер сам выберет подходящий режим.' },
  { value: 'nvidia', label: 'NVIDIA', detail: 'Для систем с проприетарным драйвером NVIDIA.' },
  { value: 'mesa', label: 'Mesa', detail: 'Открытые драйверы AMD и Intel.' },
  { value: 'software', label: 'Программный', detail: 'Запасной вариант, если GLX не работает.' },
] as const

const tourSteps = [
  {
    target: 'instances', icon: 'instances', title: 'Экземпляры игры',
    body: 'Здесь живут отдельные игровые профили и серверы. У каждого — своя версия Minecraft, загрузчик, моды и миры.',
    hint: 'Создайте профиль один раз: настройки и файлы не смешаются с остальными.',
  },
  {
    target: 'accounts', icon: 'accounts', title: 'Аккаунты',
    body: 'Подключите аккаунт Microsoft или создайте локальный профиль. Перед запуском игры аккаунт можно сменить.',
    hint: 'Локальный профиль годится для одиночной игры; для официальных серверов нужен Microsoft.',
  },
  {
    target: 'mods', icon: 'mods', title: 'Каталог',
    body: 'Поиск модов, ресурспаков, шейдеров и датапаков с Modrinth. Перед установкой выбирается совместимый экземпляр.',
    hint: 'Поддерживаемые версии и подробности проекта видны до установки.',
  },
  {
    target: 'modpacks', icon: 'archive', title: 'Модпаки',
    body: 'Готовая сборка превращается в отдельный экземпляр — существующие профили не трогаются.',
    hint: 'Версия Minecraft и загрузчик подбираются под требования сборки автоматически.',
  },
  {
    target: 'plugins', icon: 'plug', title: 'Инструменты',
    body: 'Расширения лаунчера и сторонние источники данных. Код расширений не запускается без вашего разрешения.',
    hint: 'Каждому расширению вы сами выдаёте список разрешений.',
  },
  {
    target: 'themes', icon: 'palette', title: 'Студия тем',
    body: 'Палитра, закруглённости, шрифты, обои и анимации. Слева — разделы, в центре — живой предпросмотр, справа — инспектор.',
    hint: 'Клик по элементу предпросмотра открывает редактор именно этого элемента.',
  },
  {
    target: 'settings', icon: 'settings', title: 'Настройки',
    body: 'Java, память, графика, папка игры и источники данных — когда понадобится, всё меняется здесь.',
    hint: 'Экскурсию можно пройти заново в любой момент из этого раздела.',
  },
] as const

const phaseIndex = computed(() => setupSteps.findIndex((step) => step.phase === props.phase))
const activeTourIndex = ref(0)
const offlineName = ref('Player')
const offlineNameError = computed(() => (offlineName.value ? nicknameError(offlineName.value) : ''))

function submitOffline() {
  if (props.busy || nicknameError(offlineName.value)) return
  emit('add-offline', offlineName.value)
}
const gameDirectoryDraft = ref(props.gameDirectory)
const memoryDraft = ref(Math.max(1, Math.floor(props.memoryGib || props.recommendedMemoryGib || 4)))
const themeDraft = ref<ThemeMode>(props.theme || 'system')
const graphicsDraft = ref(props.graphicsMode || 'auto')
const highlight = ref<{ top: number; left: number; width: number; height: number } | null>(null)
const dialogElement = ref<HTMLElement | null>(null)
const dialogOpen = computed(() => Boolean(props.phase))
const handleDialogKeydown = useModalFocus(dialogOpen, dialogElement, () => {
  if (props.phase === 'tour') emit('skip-tour')
})
const activeTourStep = computed(() => tourSteps[activeTourIndex.value])
const maxMemory = computed(() => Math.max(1, Math.floor(props.maximumMemoryGib || 8)))
const recommendedMemory = computed(() => Math.min(maxMemory.value, Math.max(1, Math.floor(props.recommendedMemoryGib || 4))))
const progressPercent = computed(() => ((phaseIndex.value + 1) / setupSteps.length) * 100)
const savedAccountId = computed(() => props.accounts.some((account) => account.account_id === props.selectedAccountId)
  ? props.selectedAccountId
  : (props.accounts.find((account) => account.is_default)?.account_id || props.accounts[0]?.account_id || ''))
const selectedAccount = computed(() => props.accounts.find((account) => account.account_id === savedAccountId.value))
const canSaveBasics = computed(() => Boolean(gameDirectoryDraft.value.trim()) && !props.busy)
const isLastTourStep = computed(() => activeTourIndex.value >= tourSteps.length - 1)

// Куда поставить карточку экскурсии: рядом с подсвеченным элементом, но
// не поверх него. Если места сбоку нет — поднимаем карточку вверх/вниз.
const tourCardStyle = computed(() => {
  const cardWidth = 400
  const margin = 20
  if (!highlight.value) {
    return { right: `${margin}px`, bottom: `${margin}px` }
  }
  const rect = highlight.value
  const spaceRight = window.innerWidth - (rect.left + rect.width)
  const spaceLeft = rect.left
  if (spaceRight >= cardWidth + margin + 8) {
    const top = Math.min(Math.max(margin, rect.top), Math.max(margin, window.innerHeight - 340))
    return { left: `${Math.round(rect.left + rect.width + 14)}px`, top: `${Math.round(top)}px`, bottom: 'auto', right: 'auto' }
  }
  if (spaceLeft >= cardWidth + margin + 8) {
    const top = Math.min(Math.max(margin, rect.top), Math.max(margin, window.innerHeight - 340))
    return { left: `${Math.round(Math.max(margin, rect.left - cardWidth - 14))}px`, top: `${Math.round(top)}px`, bottom: 'auto', right: 'auto' }
  }
  const below = rect.top + rect.height
  if (window.innerHeight - below >= 300) {
    return { left: `${Math.round(Math.max(margin, Math.min(rect.left, window.innerWidth - cardWidth - margin)))}px`, top: `${Math.round(below + 14)}px`, bottom: 'auto', right: 'auto' }
  }
  return { left: `${Math.round(Math.max(margin, Math.min(rect.left, window.innerWidth - cardWidth - margin)))}px`, bottom: `${Math.round(Math.max(margin, window.innerHeight - rect.top + 14))}px`, top: 'auto', right: 'auto' }
})

function goTo(phase: FirstRunPhase) {
  emit('phase-change', phase)
}

function setMemoryToRecommended() {
  memoryDraft.value = recommendedMemory.value
}

function saveBasics() {
  if (!canSaveBasics.value) return
  emit('save-basics', {
    gameDirectory: gameDirectoryDraft.value.trim(),
    memory: `${Math.min(maxMemory.value, Math.max(1, Math.floor(memoryDraft.value)))}GiB`,
    theme: themeDraft.value,
    graphicsMode: graphicsDraft.value,
  })
}

function updateHighlight() {
  if (props.phase !== 'tour' || typeof document === 'undefined') {
    highlight.value = null
    return
  }
  const target = document.querySelector<HTMLElement>(`[data-tour="${activeTourStep.value.target}"]`)
  if (!target) {
    highlight.value = null
    return
  }
  const rect = target.getBoundingClientRect()
  if (rect.width <= 0 || rect.height <= 0 || rect.right <= 0 || rect.bottom <= 0 || rect.left >= window.innerWidth || rect.top >= window.innerHeight) {
    highlight.value = null
    return
  }
  const padding = 5
  highlight.value = {
    top: Math.max(4, rect.top - padding),
    left: Math.max(4, rect.left - padding),
    width: Math.min(window.innerWidth - Math.max(4, rect.left - padding) - 4, rect.width + padding * 2),
    height: Math.min(window.innerHeight - Math.max(4, rect.top - padding) - 4, rect.height + padding * 2),
  }
}

async function measureTourStep() {
  if (props.phase !== 'tour') return
  const target = document.querySelector<HTMLElement>(`[data-tour="${activeTourStep.value.target}"]`)
  // Цель может быть за пределами вьюпорта — сначала подкатываем её в кадр.
  target?.scrollIntoView({ block: 'nearest', inline: 'nearest', behavior: 'instant' as ScrollBehavior })
  await nextTick()
  updateHighlight()
}

function nextTourStep() {
  if (isLastTourStep.value) {
    emit('finish-tour')
    return
  }
  activeTourIndex.value += 1
}

function previousTourStep() {
  activeTourIndex.value = Math.max(0, activeTourIndex.value - 1)
}

function handleTourKeydown(event: KeyboardEvent) {
  handleDialogKeydown(event)
  if (event.key === 'ArrowRight') {
    event.preventDefault()
    nextTourStep()
  } else if (event.key === 'ArrowLeft') {
    event.preventDefault()
    previousTourStep()
  }
}

watch(() => props.phase, async (phase) => {
  if (phase === 'basics') {
    gameDirectoryDraft.value = props.gameDirectory
    memoryDraft.value = Math.min(maxMemory.value, Math.max(1, Math.floor(props.memoryGib || recommendedMemory.value)))
    themeDraft.value = props.theme || 'system'
    graphicsDraft.value = props.graphicsMode || 'auto'
  }
  if (phase === 'tour') activeTourIndex.value = 0
  await nextTick()
  if (phase === 'tour') await measureTourStep()
  else updateHighlight()
})
watch(activeTourIndex, () => {
  void measureTourStep()
})
watch(() => props.phase, async (phase) => {
  await nextTick()
  if (phase) dialogElement.value?.querySelector<HTMLElement>('button:not(:disabled), input:not(:disabled)')?.focus()
})
onMounted(() => {
  window.addEventListener('resize', updateHighlight)
  window.addEventListener('scroll', updateHighlight, true)
  updateHighlight()
})
onBeforeUnmount(() => {
  window.removeEventListener('resize', updateHighlight)
  window.removeEventListener('scroll', updateHighlight, true)
})
</script>

<template>
  <Teleport to="body">
    <Transition name="first-run-fade">
      <div v-if="phase && phase !== 'tour'" class="first-run-scrim">
        <section ref="dialogElement" class="first-run-card" role="dialog" tabindex="-1" aria-modal="true" aria-labelledby="first-run-title" @keydown="handleDialogKeydown">
          <header class="first-run-header">
            <span class="first-run-brand"><img class="first-run-brand-icon" :src="launcherLogo" alt="" />Nivora Launcher</span>
            <span class="first-run-step">{{ phase === 'welcome' || phase === 'language' ? 'Первая настройка' : `Шаг ${phaseIndex + 1} из ${setupSteps.length}` }}</span>
          </header>
          <div v-if="phase !== 'welcome' && phase !== 'language'" class="first-run-progress" aria-hidden="true"><span :style="{ width: `${progressPercent}%` }" /></div>

          <Transition name="first-run-step" mode="out-in">
          <div v-if="phase === 'language'" key="language" class="first-run-content first-run-welcome">
            <img class="first-run-hero-icon" :src="launcherLogo" alt="" />
            <h1 id="first-run-title">Language · Язык</h1>
            <p>Выберите язык интерфейса — его можно поменять позже в настройках.<br />Choose your interface language — you can change it later in Settings.</p>
            <div class="first-run-roadmap first-run-language-options">
              <button class="button button--outline first-run-language-option" type="button" @click="emit('pick-language', 'ru')"><strong>Русский</strong><small>Тема, настройки и подсказки на русском</small></button>
              <button class="button button--outline first-run-language-option" type="button" @click="emit('pick-language', 'en')"><strong>English</strong><small>Theme, settings and hints in English</small></button>
            </div>
          </div>
          <div v-else-if="phase === 'welcome'" key="welcome" class="first-run-content first-run-welcome">
            <img class="first-run-hero-icon" :src="launcherLogo" alt="" />
            <h1 id="first-run-title">Добро пожаловать</h1>
            <p>Настроим главное: аккаунт, папку игры и базовые параметры. Это займёт пару минут — остальное можно поменять позже.</p>
            <div class="first-run-roadmap">
              <span><i>1</i>Добавим аккаунт</span><span><i>2</i>Выберем основные настройки</span><span><i>3</i>Короткая экскурсия по разделам</span>
            </div>
            <div class="first-run-footer first-run-footer--center">
              <button class="button button--accent button--large first-run-primary" type="button" @click="goTo('account')">Начать настройку<Icon name="arrow-right" :size="16" /></button>
              <button class="button button--quiet" type="button" @click="emit('skip-setup')">Начать без настройки</button>
            </div>
          </div>

          <div v-else-if="phase === 'account'" key="account" class="first-run-content">
            <span class="eyebrow">Шаг 2 · Аккаунт</span>
            <h1 id="first-run-title">Игровой профиль</h1>
            <p class="first-run-lead">Выберите, от чьего имени запускать игру. Аккаунт можно добавить или сменить позже.</p>

            <div v-if="accounts.length" class="first-run-account-list">
              <button v-for="account in accounts" :key="account.account_id" class="first-run-account-option" :class="{ 'first-run-account-option--active': account.account_id === savedAccountId }" type="button" :aria-pressed="account.account_id === savedAccountId" @click="emit('select-account', account.account_id)">
                <span class="first-run-option-icon"><Icon name="accounts" :size="17" /></span>
                <span class="first-run-option-copy"><strong>{{ account.display_name }}</strong><small>{{ account.kind === 'microsoft' ? 'Microsoft · онлайн-возможности' : 'Локальный профиль · одиночная игра' }}</small></span>
                <Icon v-if="account.account_id === savedAccountId" class="first-run-selected-mark" name="check" :size="17" />
              </button>
            </div>

            <div v-else class="first-run-account-choice">
              <button class="first-run-choice-card first-run-choice-card--recommended" type="button" :disabled="busy || !microsoftLoginAvailable" @click="emit('add-microsoft')">
                <span class="first-run-choice-icon"><Icon name="accounts" :size="20" /></span>
                <span><strong>Войти через Microsoft</strong><small>{{ microsoftLoginAvailable ? 'Для официальных серверов и профиля Minecraft.' : 'Появится в сборке с настроенным входом; вводить ID вручную не нужно.' }}</small></span>
                <Icon name="arrow-right" :size="16" />
              </button>
              <div class="first-run-offline-box">
                <div class="first-run-offline-heading"><span class="first-run-option-icon"><Icon name="user" :size="17" /></span><span><strong>Локальный профиль</strong><small>Для одиночной игры; сервисы Microsoft не подключаются.</small></span></div>
                <div class="first-run-offline-actions"><input v-model="offlineName" type="text" maxlength="16" autocomplete="nickname" aria-label="Имя локального профиля" placeholder="Например, Player" spellcheck="false" :aria-invalid="Boolean(offlineNameError)" :class="{ 'input--invalid': offlineNameError }" @keydown.enter.prevent="submitOffline" /><button class="button button--outline" type="button" :disabled="busy || Boolean(nicknameError(offlineName))" @click="submitOffline">Создать</button></div>
                <p v-if="offlineNameError" class="inline-error first-run-nickname-error" role="alert">{{ offlineNameError }}</p>
              </div>
            </div>

            <div v-if="accounts.length" class="first-run-account-add">
              <button class="button button--quiet" type="button" :disabled="busy || !microsoftLoginAvailable" @click="emit('add-microsoft')"><Icon name="plus" :size="14" />Добавить Microsoft-аккаунт</button>
              <span class="first-run-or">или</span>
              <div class="first-run-offline-actions"><input v-model="offlineName" type="text" maxlength="16" autocomplete="nickname" aria-label="Имя нового локального профиля" placeholder="Имя локального профиля" spellcheck="false" :aria-invalid="Boolean(offlineNameError)" :class="{ 'input--invalid': offlineNameError }" @keydown.enter.prevent="submitOffline" /><button class="button button--small button--outline" type="button" :disabled="busy || Boolean(nicknameError(offlineName))" @click="submitOffline">Создать</button></div>
              <p v-if="offlineNameError" class="inline-error first-run-nickname-error" role="alert">{{ offlineNameError }}</p>
            </div>

            <div v-if="error" class="first-run-error" role="alert"><Icon name="warning" :size="15" />{{ error }}</div>
            <div v-if="busy" class="first-run-busy" role="status"><span class="first-run-spinner" />{{ accounts.length ? 'Подключаю аккаунт…' : 'Ждём подтверждения входа в браузере…' }}</div>
            <div v-if="busy && microsoftCode" class="first-run-auth-code" role="status"><span>Откройте страницу входа Microsoft и введите код:</span><strong>{{ microsoftCode }}</strong><small>{{ microsoftVerificationUri || 'https://microsoft.com/devicelogin' }}</small></div>
            <div class="first-run-footer" :class="{ 'first-run-footer--back': !accounts.length }">
              <button class="button button--quiet" type="button" @click="goTo('welcome')"><Icon name="arrow-left" :size="14" />Назад</button>
              <span v-if="!accounts.length" class="first-run-footnote">После входа продолжим автоматически</span>
              <button v-if="accounts.length" class="button button--accent" type="button" :disabled="busy" @click="goTo('basics')">Продолжить<Icon name="arrow-right" :size="15" /></button>
            </div>
          </div>

          <div v-else-if="phase === 'basics'" key="basics" class="first-run-content">
            <span class="eyebrow">Шаг 3 · Базовая настройка</span>
            <h1 id="first-run-title">Основные параметры</h1>
            <p class="first-run-lead">Рекомендованные значения уже выбраны — можно просто сохранить.</p>
            <label class="first-run-field"><span><strong>Папка игры</strong><small>Здесь будут храниться версии, моды и сохранения.</small></span><input v-model="gameDirectoryDraft" type="text" autocomplete="off" spellcheck="false" placeholder="Папка игры" /></label>
            <div class="first-run-memory">
              <div class="first-run-memory-heading"><span><strong>Память для Minecraft</strong><small>Если не уверены — оставьте рекомендованное значение.</small></span><strong>{{ memoryDraft }} GiB</strong></div>
              <input v-model.number="memoryDraft" type="range" min="1" :max="maxMemory" step="1" aria-label="Память для Minecraft" />
              <div class="first-run-memory-meta"><span>1 GiB</span><button class="text-button" type="button" @click="setMemoryToRecommended">Рекомендация — {{ recommendedMemory }} GiB</button><span>{{ maxMemory }} GiB</span></div>
            </div>
            <fieldset class="first-run-graphics"><legend>Графический драйвер</legend>
              <button v-for="option in graphicsChoices" :key="option.value" type="button" :class="{ 'first-run-theme-option--active': graphicsDraft === option.value }" :aria-pressed="graphicsDraft === option.value" @click="graphicsDraft = option.value"><strong>{{ option.label }}</strong><small>{{ option.detail }}</small></button>
            </fieldset>
            <fieldset class="first-run-theme"><legend>Внешний вид</legend>
              <button v-for="option in themeOptions" :key="option.value" type="button" class="first-run-theme-option" :class="{ 'first-run-theme-option--active': themeDraft === option.value }" :aria-pressed="themeDraft === option.value" @click="themeDraft = option.value"><Icon :name="option.icon" :size="16" />{{ option.label }}</button>
            </fieldset>
            <div v-if="error" class="first-run-error" role="alert"><Icon name="warning" :size="15" />{{ error }}</div>
            <div class="first-run-footer"><button class="button button--quiet" type="button" :disabled="busy" @click="goTo('account')"><Icon name="arrow-left" :size="14" />Назад</button><button class="button button--accent" type="button" :disabled="!canSaveBasics" @click="saveBasics">{{ busy ? 'Сохраняю…' : 'Сохранить и продолжить' }}<Icon name="arrow-right" :size="15" /></button></div>
          </div>

          <div v-else-if="phase === 'finish'" key="finish" class="first-run-content first-run-finish">
            <span class="first-run-finish-icon"><Icon name="check" :size="25" /></span>
            <h1 id="first-run-title">Всё готово к запуску</h1>
            <p>Аккаунт добавлен, параметры сохранены. Дальше — короткая экскурсия по разделам; она займёт меньше минуты.</p>
            <div class="first-run-summary">
              <span><Icon name="accounts" :size="14" />Аккаунт: {{ selectedAccount?.display_name || 'не выбран' }}</span>
              <span><Icon name="folder" :size="14" />Папка: {{ gameDirectory || gameDirectoryDraft }}</span>
              <span><Icon name="memory" :size="14" />Память: {{ memoryGib }} GiB</span>
            </div>
            <div class="first-run-footer first-run-footer--center">
              <button class="button button--accent button--large first-run-primary" type="button" @click="emit('finish-setup')">Показать экскурсию<Icon name="arrow-right" :size="16" /></button>
              <button class="button button--quiet" type="button" @click="emit('skip-setup')">Готово, без экскурсии</button>
            </div>
          </div>

          </Transition>
          <footer v-if="phase !== 'welcome' && phase !== 'language'" class="first-run-card-footer"><span>Всё это доступно в настройках позже</span><span>{{ phaseIndex + 1 }} / {{ setupSteps.length }}</span></footer>
        </section>
      </div>
    </Transition>

    <Transition name="first-run-fade">
      <div v-if="phase === 'tour'" class="first-run-tour-shell" role="presentation">
        <div v-if="highlight" class="first-run-tour-highlight" :style="{ top: `${highlight.top}px`, left: `${highlight.left}px`, width: `${highlight.width}px`, height: `${highlight.height}px` }" aria-hidden="true" />
        <div v-else class="first-run-tour-scrim" aria-hidden="true" />
        <section ref="dialogElement" class="first-run-tour-card" :style="tourCardStyle" role="dialog" tabindex="-1" aria-modal="true" aria-labelledby="first-run-tour-title" @keydown="handleTourKeydown">
          <header class="first-run-tour-header">
            <span class="first-run-tour-badge"><Icon :name="activeTourStep.icon" :size="15" />{{ activeTourIndex + 1 }} / {{ tourSteps.length }}</span>
            <button class="icon-button" type="button" aria-label="Закрыть экскурсию" v-tooltip="'Закрыть экскурсию'" @click="emit('skip-tour')"><Icon name="close" :size="16" /></button>
          </header>
          <div class="first-run-tour-progress" aria-label="Прогресс экскурсии"><span v-for="(step, index) in tourSteps" :key="step.target" :class="{ 'first-run-tour-progress--active': index === activeTourIndex, 'first-run-tour-progress--done': index < activeTourIndex }" /></div>
          <Transition name="first-run-step" mode="out-in">
            <div :key="activeTourIndex" class="first-run-tour-content">
              <h2 id="first-run-tour-title">{{ activeTourStep.title }}</h2>
              <p>{{ activeTourStep.body }}</p>
              <div class="first-run-tour-hint"><Icon name="info" :size="15" /><span>{{ activeTourStep.hint }}</span></div>
            </div>
          </Transition>
          <footer class="first-run-tour-actions">
            <button class="button button--quiet" type="button" :disabled="activeTourIndex === 0" @click="previousTourStep"><Icon name="arrow-left" :size="14" />Назад</button>
            <button class="button button--quiet first-run-tour-skip" type="button" @click="emit('skip-tour')">Пропустить</button>
            <button class="button button--accent" type="button" @click="nextTourStep">{{ isLastTourStep ? 'Завершить' : 'Далее' }}<Icon v-if="!isLastTourStep" name="arrow-right" :size="14" /></button>
          </footer>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.first-run-scrim, .first-run-tour-scrim { position: fixed; z-index: 2500; inset: 0; display: grid; place-items: center; padding: 20px; background: rgba(5, 9, 16, .72); }
.first-run-card { display: grid; width: min(640px, 100%); max-height: min(820px, calc(100dvh - 40px)); grid-template-rows: auto auto minmax(0, 1fr) auto; overflow: hidden; border: 1px solid var(--line-strong, #24334c); border-radius: var(--radius-card, 8px); color: var(--text, #e8edf7); background: var(--panel-raised, #121b2a); box-shadow: 0 24px 64px rgba(0, 0, 0, .45); }
.first-run-header { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 16px 22px; border-bottom: 1px solid var(--line, rgba(255,255,255,.1)); }
.first-run-brand { display: inline-flex; align-items: center; gap: 9px; color: var(--text, #e8edf7); font-size: 13px; font-weight: 600; }
.first-run-brand-icon { width: 22px; height: 22px; object-fit: contain; }
.first-run-step, .first-run-card-footer { color: var(--muted, #8d9bb3); font-size: 12px; font-weight: 500; }
.first-run-progress { height: 2px; background: var(--line, #24334c); }
.first-run-progress > span { display: block; height: 100%; border-radius: 0 2px 2px 0; background: var(--accent, #7aa2ff); transition: width .2s ease; }
.first-run-content { display: grid; min-height: 0; align-content: start; gap: 16px; padding: 26px clamp(20px, 5vw, 40px) 28px; overflow: auto; }
.first-run-welcome, .first-run-finish { align-content: center; min-height: 400px; }
.first-run-welcome { justify-items: center; text-align: center; }
.first-run-welcome > p { margin-inline: auto; }
.first-run-hero-icon { width: 52px; height: 52px; object-fit: contain; }
.first-run-finish-icon { display: grid; width: 52px; height: 52px; place-items: center; border: 1px solid color-mix(in srgb, var(--success, #4fae7d) 45%, transparent); border-radius: var(--radius-card, 8px); color: var(--success, #6bd4a5); background: color-mix(in srgb, var(--success, #37bd85) 12%, transparent); }
.first-run-content h1, .first-run-tour-card h2 { margin: 0; color: var(--text, #e8edf7); font-size: clamp(21px, 3.4vw, 26px); font-weight: 700; letter-spacing: -.01em; line-height: 1.2; }
.first-run-content > p { max-width: 500px; margin: 0; color: var(--muted, #9aa9c0); font-size: 13px; line-height: 1.6; }
.first-run-content .eyebrow { color: var(--accent, #9bb6ff); font-size: 12px; font-weight: 600; }
.first-run-roadmap { display: grid; width: min(100%, 440px); gap: 8px; margin: 4px 0; text-align: left; }
/* Шаг выбора языка: две крупные кнопки-плитки на всю ширину карточки. */
.first-run-language-options { gap: 10px; }
.first-run-language-option { display: grid; justify-items: start; gap: 4px; padding: 14px 18px; }
.first-run-language-option strong { font-size: 15px; }
.first-run-language-option small { color: var(--muted, #8b95a8); font-size: 11.5px; }
.first-run-roadmap > span { display: flex; align-items: center; gap: 10px; color: var(--text, #e8edf7); font-size: 13px; }
.first-run-roadmap i { display: grid; width: 24px; height: 24px; flex: 0 0 24px; place-items: center; border: 1px solid var(--line, #293952); border-radius: var(--radius-button, 8px); color: var(--accent, #9bb6ff); background: var(--panel, #101827); font-size: 11px; font-style: normal; font-weight: 600; }
.first-run-primary { min-width: 210px; justify-content: center; gap: 8px; }
.first-run-footer { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.first-run-footer--back { justify-content: flex-start; }
.first-run-footer--center { justify-content: center; flex-wrap: wrap; }
.first-run-lead { margin-bottom: 2px !important; }
.first-run-account-list, .first-run-account-choice { display: grid; gap: 10px; }
.first-run-account-option, .first-run-choice-card { display: flex; width: 100%; align-items: center; gap: 12px; padding: 12px; border: 1px solid var(--line, #293952); border-radius: var(--radius-card, 8px); color: var(--text, #e8edf7); background: color-mix(in srgb, var(--panel, #101827) 80%, transparent); text-align: left; cursor: pointer; transition: border-color .14s ease, background-color .14s ease; }
.first-run-account-option:hover, .first-run-choice-card:hover { border-color: var(--line-strong, #3a4d6d); }
.first-run-account-option--active, .first-run-choice-card--recommended { border-color: color-mix(in srgb, var(--accent, #7aa2ff) 55%, var(--line, #293952)); background: color-mix(in srgb, var(--accent, #7aa2ff) 8%, var(--panel, #101827)); }
.first-run-option-icon, .first-run-choice-icon { display: grid; width: 36px; height: 36px; flex: 0 0 36px; place-items: center; border: 1px solid var(--line, #293952); border-radius: var(--radius-control, 8px); color: var(--accent, #9bb6ff); background: color-mix(in srgb, var(--accent, #7aa2ff) 9%, transparent); }
.first-run-option-copy, .first-run-choice-card > span:nth-child(2), .first-run-offline-heading > span:last-child { display: grid; flex: 1; gap: 3px; }
.first-run-option-copy strong, .first-run-choice-card strong, .first-run-offline-heading strong { font-size: 13px; font-weight: 600; }
.first-run-option-copy small, .first-run-choice-card small, .first-run-offline-heading small { color: var(--muted, #96a5bb); font-size: 11.5px; line-height: 1.45; }
.first-run-selected-mark { color: var(--success, #74e0b2); }
.first-run-offline-box { display: grid; gap: 12px; padding: 12px; border: 1px solid var(--line, #293952); border-radius: var(--radius-card, 8px); background: color-mix(in srgb, var(--panel, #101827) 80%, transparent); }
.first-run-offline-heading { display: flex; align-items: center; gap: 10px; }
.first-run-offline-actions { display: flex; gap: 8px; }
.first-run-offline-actions input, .first-run-field > input { width: 100%; min-width: 0; min-height: 38px; padding: 8px 11px; border: 1px solid var(--line-strong, #293952); border-radius: var(--radius-button, 8px); outline: none; color: var(--text, #e8edf7); background: var(--panel, #101827); font: inherit; font-size: 13px; }
.first-run-offline-actions input:focus, .first-run-field > input:focus { border-color: var(--accent, #7aa2ff); }
.first-run-account-add { display: grid; gap: 8px; justify-items: start; padding-top: 2px; }
.first-run-or { color: var(--muted, #8291a7); font-size: 11.5px; }
.first-run-error { display: flex; align-items: flex-start; gap: 8px; padding: 10px 12px; border: 1px solid color-mix(in srgb, var(--danger, #f27d82) 35%, transparent); border-radius: var(--radius-control, 8px); color: var(--danger, #ffb4b7); background: rgba(180, 52, 62, .1); font-size: 12.5px; line-height: 1.5; }
.first-run-busy { display: flex; align-items: center; gap: 9px; color: var(--muted, #96a5bb); font-size: 12.5px; }
.first-run-auth-code { display: grid; gap: 6px; padding: 12px; border: 1px solid var(--line-strong, #293952); border-radius: var(--radius-control, 8px); background: color-mix(in srgb, var(--accent, #7aa2ff) 7%, var(--panel, #101827)); }
.first-run-auth-code > span { color: var(--muted, #a0adc1); font-size: 11.5px; }
.first-run-auth-code > strong { width: fit-content; padding: 4px 9px; border-radius: var(--radius-button, 8px); color: var(--text, #e8edf7); background: color-mix(in srgb, var(--accent, #7aa2ff) 15%, var(--panel, #101827)); font-family: ui-monospace, monospace; font-size: 19px; letter-spacing: .1em; user-select: text; }
.first-run-auth-code > small { color: var(--accent, #a9c1ff); font-size: 11.5px; user-select: text; }
.first-run-spinner { width: 14px; height: 14px; border: 2px solid color-mix(in srgb, var(--accent, #7aa2ff) 26%, transparent); border-top-color: var(--accent, #7aa2ff); border-radius: 50%; animation: first-run-spin .8s linear infinite; }
.first-run-footnote { color: var(--muted, #8796ac); font-size: 11.5px; }
.first-run-field { display: grid; gap: 8px; }
.first-run-field > span, .first-run-memory-heading > span { display: grid; gap: 3px; }
.first-run-field strong, .first-run-memory-heading > span > strong { color: var(--text, #e8edf7); font-size: 13px; }
.first-run-field small, .first-run-memory-heading small { color: var(--muted, #96a5bb); font-size: 11.5px; line-height: 1.45; }
.first-run-memory { display: grid; gap: 10px; padding: 13px; border: 1px solid var(--line, #293952); border-radius: var(--radius-card, 8px); background: color-mix(in srgb, var(--panel, #101827) 80%, transparent); }
.first-run-memory-heading { display: flex; align-items: center; justify-content: space-between; gap: 14px; }
.first-run-memory-heading > strong { color: var(--accent, #9bb6ff); font-size: 15px; white-space: nowrap; }
.first-run-memory input[type='range'] { width: 100%; accent-color: var(--accent, #7aa2ff); }
.first-run-memory-meta { display: flex; align-items: center; justify-content: space-between; gap: 8px; color: var(--muted, #8291a7); font-size: 11px; }
.first-run-memory-meta .text-button { color: var(--accent, #9bb6ff); font-size: 11.5px; }
.first-run-theme { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 8px; min-width: 0; margin: 0; padding: 12px; border: 1px solid var(--line, #293952); border-radius: var(--radius-card, 8px); }
.first-run-theme legend, .first-run-graphics legend { padding: 0 6px; color: var(--muted, #96a5bb); font-size: 11.5px; }
.first-run-theme-option { display: flex; min-height: 38px; align-items: center; justify-content: center; gap: 7px; padding: 8px; border: 1px solid var(--line, #293952); border-radius: var(--radius-button, 8px); color: var(--muted, #96a5bb); background: var(--panel, #101827); font: inherit; font-size: 12px; cursor: pointer; }
.first-run-theme-option--active { border-color: var(--accent, #7aa2ff); color: var(--text, #e8edf7); background: color-mix(in srgb, var(--accent, #7aa2ff) 12%, var(--panel, #101827)); }
.first-run-graphics { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; min-width: 0; margin: 0; padding: 12px; border: 1px solid var(--line, #293952); border-radius: var(--radius-card, 8px); }
.first-run-graphics button { display: grid; min-height: 52px; align-content: center; gap: 3px; padding: 8px 10px; border: 1px solid var(--line, #293952); border-radius: var(--radius-button, 8px); color: var(--muted, #96a5bb); background: var(--panel, #101827); text-align: left; cursor: pointer; }
.first-run-graphics button strong { font-size: 12px; }
.first-run-graphics button small { font-size: 10.5px; line-height: 1.4; }
.first-run-graphics button.first-run-theme-option--active { border-color: var(--accent, #7aa2ff); color: var(--text, #e8edf7); background: color-mix(in srgb, var(--accent, #7aa2ff) 12%, var(--panel, #101827)); }
.first-run-finish { align-items: start; }
.first-run-summary { display: grid; gap: 8px; padding: 12px; border: 1px solid var(--line, #293952); border-radius: var(--radius-control, 8px); background: color-mix(in srgb, var(--panel, #101827) 80%, transparent); }
.first-run-summary > span { display: flex; min-width: 0; align-items: center; gap: 8px; overflow-wrap: anywhere; color: var(--muted, #a8b5c9); font-size: 12px; }
.first-run-summary > span svg { flex: 0 0 14px; color: var(--success, #75dcae); }
.first-run-card-footer { display: flex; justify-content: space-between; gap: 10px; padding: 12px 22px; border-top: 1px solid var(--line, rgba(255,255,255,.1)); font-size: 11.5px; }
.first-run-tour-shell { position: fixed; z-index: 2500; inset: 0; pointer-events: none; }
.first-run-tour-highlight { position: fixed; z-index: 2500; border: 2px solid var(--accent, #8aabff); border-radius: var(--radius-control, 8px); box-shadow: 0 0 0 9999px rgba(4, 8, 15, .68); pointer-events: none; transition: top .18s ease, left .18s ease, width .18s ease, height .18s ease; }
.first-run-tour-scrim { z-index: 2500; pointer-events: none; }
.first-run-tour-card { position: fixed; z-index: 2501; right: 20px; bottom: 20px; display: grid; width: min(400px, calc(100vw - 32px)); max-height: min(420px, calc(100dvh - 32px)); gap: 12px; overflow: auto; padding: 16px; border: 1px solid var(--line-strong, #24334c); border-radius: var(--radius-card, 8px); color: var(--text, #e8edf7); background: var(--panel-raised, #121b2a); box-shadow: 0 20px 56px rgba(0,0,0,.5); pointer-events: auto; }
.first-run-tour-header { display: flex; align-items: center; justify-content: space-between; }
.first-run-tour-badge { display: inline-flex; align-items: center; gap: 7px; color: var(--accent, #9bb6ff); font-size: 12px; font-weight: 600; }
.first-run-tour-progress { display: flex; gap: 5px; }
.first-run-tour-progress > span { height: 3px; flex: 1; border-radius: 2px; background: var(--line, #293952); }
.first-run-tour-progress > .first-run-tour-progress--active { background: var(--accent, #7aa2ff); }
.first-run-tour-progress > .first-run-tour-progress--done { background: color-mix(in srgb, var(--accent, #7aa2ff) 55%, var(--line, #293952)); }
.first-run-tour-card h2 { font-size: 18px; }
.first-run-tour-content { display: grid; gap: 10px; }
.first-run-tour-content > p { margin: 0; color: var(--muted, #a0adc1); font-size: 12.5px; line-height: 1.6; }
.first-run-tour-hint { display: flex; align-items: flex-start; gap: 8px; padding: 9px 10px; border: 1px solid var(--line, #293952); border-radius: var(--radius-control, 8px); color: var(--muted, #a7b3c7); background: color-mix(in srgb, var(--panel, #101827) 78%, transparent); font-size: 11.5px; line-height: 1.5; }
.first-run-tour-hint svg { flex: 0 0 15px; color: var(--accent, #9bb6ff); }
.first-run-tour-actions { display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 7px; }
.first-run-tour-actions .button { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; }
.first-run-tour-skip { color: var(--muted, #95a3b9); }
.first-run-fade-enter-active, .first-run-fade-leave-active { transition: opacity .16s ease; }
.first-run-fade-enter-from, .first-run-fade-leave-to { opacity: 0; }
.first-run-step-enter-active, .first-run-step-leave-active { transition: opacity .16s ease, transform .16s cubic-bezier(.2,.7,.2,1); }
.first-run-step-enter-from { opacity: 0; transform: translateX(10px); }
.first-run-step-leave-to { opacity: 0; transform: translateX(-10px); }
@keyframes first-run-spin { to { transform: rotate(360deg); } }

@media (prefers-reduced-motion: reduce) {
  .first-run-fade-enter-active, .first-run-fade-leave-active, .first-run-step-enter-active, .first-run-step-leave-active, .first-run-tour-highlight { transition-duration: .01ms !important; }
  .first-run-step-enter-from, .first-run-step-leave-to { transform: none; }
  .first-run-spinner { animation: none; }
}

@media (max-width: 600px) {
  .first-run-scrim { padding: 10px; }
  .first-run-card { max-height: calc(100dvh - 20px); }
  .first-run-header { padding: 12px 14px; }
  .first-run-content { gap: 12px; padding: 18px 15px 20px; }
  .first-run-welcome, .first-run-finish { min-height: 350px; }
  .first-run-card-footer { padding: 10px 14px; }
  .first-run-theme, .first-run-graphics { grid-template-columns: 1fr; }
  .first-run-tour-card { right: 12px; bottom: 12px; width: calc(100vw - 24px); padding: 14px; }
  .first-run-tour-actions { justify-content: flex-start; }
  .first-run-tour-skip { margin-right: auto; }
}
</style>
