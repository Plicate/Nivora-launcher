export interface FirstRunExperienceState {
  setupCompleted: boolean
  tourCompleted: boolean
}

export const FIRST_RUN_SETUP_STAGES = [
  { phase: 'language', label: 'Язык интерфейса' },
  { phase: 'welcome', label: 'Приветствие' },
  { phase: 'account', label: 'Добавляем аккаунт' },
  { phase: 'basics', label: 'Базовая настройка' },
  { phase: 'finish', label: 'Завершение' },
] as const

export type FirstRunSetupPhase = typeof FIRST_RUN_SETUP_STAGES[number]['phase']
export type FirstRunPhase = FirstRunSetupPhase | 'tour'

export interface FirstRunStorage {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

export const FIRST_RUN_EXPERIENCE_KEY = 'nivora-launcher:first-run-experience:v1'

const emptyState: FirstRunExperienceState = {
  setupCompleted: false,
  tourCompleted: false,
}

function resolveStorage(storage?: FirstRunStorage): FirstRunStorage | undefined {
  if (storage) return storage
  try {
    return typeof window === 'undefined' ? undefined : window.localStorage
  } catch {
    return undefined
  }
}

export function readFirstRunExperience(storage?: FirstRunStorage): FirstRunExperienceState {
  try {
    const raw = resolveStorage(storage)?.getItem(FIRST_RUN_EXPERIENCE_KEY)
    if (!raw) return { ...emptyState }
    const value: unknown = JSON.parse(raw)
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return { ...emptyState }
    const record = value as Record<string, unknown>
    return {
      setupCompleted: record.version === 1 && record.setupCompleted === true,
      tourCompleted: record.version === 1 && record.tourCompleted === true,
    }
  } catch {
    return { ...emptyState }
  }
}

export function initialFirstRunPhase(state: FirstRunExperienceState): FirstRunPhase | null {
  // язык — самый первый шаг первоначальной настройки: до приветствия,
  // чтобы весь визард сразу был на языке пользователя.
  if (!state.setupCompleted) return 'language'
  if (!state.tourCompleted) return 'tour'
  return null
}

export function writeFirstRunExperience(
  state: FirstRunExperienceState,
  storage?: FirstRunStorage,
): boolean {
  try {
    const target = resolveStorage(storage)
    if (!target) return false
    target.setItem(FIRST_RUN_EXPERIENCE_KEY, JSON.stringify({
      version: 1,
      setupCompleted: state.setupCompleted === true,
      tourCompleted: state.tourCompleted === true,
    }))
    return true
  } catch {
    return false
  }
}
