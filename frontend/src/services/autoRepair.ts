import { reactive } from 'vue'
import { bridge } from './bridge'
import type { AutoRepairReport } from '../types'

export const autoRepairState = reactive({
  report: null as AutoRepairReport | null,
})

export function offerAutoRepair(report?: AutoRepairReport | null): boolean {
  if (!report || report.disabled || !report.issues?.length) return false
  autoRepairState.report = report
  return true
}

/** бэкенд анализирует ошибку и лог; сбой диагностики не заменяет исходную ошибку */
export async function diagnoseLaunchFailure(instanceName: string, error: string, exitCode?: number | null): Promise<boolean> {
  try {
    const report = await bridge.autoRepairDiagnose({ instance_name: instanceName, error, exit_code: exitCode ?? null })
    return offerAutoRepair(report)
  } catch {
    return false
  }
}

/** ручная проверка также учитывает код выхода и лог последней сессии */
export async function diagnoseManually(instanceName: string, exitCode?: number | null): Promise<AutoRepairReport> {
  const report = await bridge.autoRepairDiagnose({ instance_name: instanceName, manual: true, exit_code: exitCode ?? null })
  autoRepairState.report = report
  return report
}

export function closeAutoRepair() {
  autoRepairState.report = null
}
