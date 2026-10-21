// окно авто-починки — одно на всё приложение: краш может прийти и из
import { reactive } from 'vue'
import { bridge } from './bridge'
import type { AutoRepairReport } from '../types'

export const autoRepairState = reactive({
  report: null as AutoRepairReport | null,
})

/** Показать окно, если в отчёте есть что предложить. */
export function offerAutoRepair(report?: AutoRepairReport | null): boolean {
  if (!report || report.disabled || !report.issues?.length) return false
  autoRepairState.report = report
  return true
}

/**
 * Запуск упал ещё до выхода игры (исключение вместо кода выхода) — просим
 * бэкенд разобрать текст ошибки и лог сессии. Сбой диагностики не важнее
 * исходной ошибки запуска, поэтому он молча игнорируется.
 */
export async function diagnoseLaunchFailure(instanceName: string, error: string, exitCode?: number | null): Promise<boolean> {
  try {
    const report = await bridge.autoRepairDiagnose({ instance_name: instanceName, error, exit_code: exitCode ?? null })
    return offerAutoRepair(report)
  } catch {
    return false
  }
}

/**
 * Ручная проверка из рабочей области: окно открывается даже без проблем.
 * С кодом выхода (кнопка в плашке краша) учитывается и лог последней сессии.
 */
export async function diagnoseManually(instanceName: string, exitCode?: number | null): Promise<AutoRepairReport> {
  const report = await bridge.autoRepairDiagnose({ instance_name: instanceName, manual: true, exit_code: exitCode ?? null })
  autoRepairState.report = report
  return report
}

export function closeAutoRepair() {
  autoRepairState.report = null
}
