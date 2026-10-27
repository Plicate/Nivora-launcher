import { check as checkForAppUpdate } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import type { Ref } from 'vue'
import type { PluginUiTone } from '../../../plugins/api'
import { confirmDialog } from './dialogs'
import { bridge } from './bridge'

export function createLauncherUpdates(deps: {
  updaterBusy: Ref<boolean>
  updaterStatus: Ref<string>
  updaterVersion: Ref<string>
  updaterNotes: Ref<string>
  updaterProgressDownloaded: Ref<number>
  updaterProgressTotal: Ref<number>
  updaterManualUrl: Ref<string>
  showToast: (message: string, tone?: PluginUiTone) => void
  errorMessage: (error: unknown) => string
}) {
  const {
    updaterBusy,
    updaterStatus,
    updaterVersion,
    updaterNotes,
    updaterProgressDownloaded,
    updaterProgressTotal,
    updaterManualUrl,
    showToast,
    errorMessage,
  } = deps
  let pendingAppUpdate: any = null

function inDesktopTauri(): boolean {
  return typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

async function checkRemoteReleaseFallback(): Promise<boolean> {
  try {
    const release = await bridge.checkRemoteRelease()
    if (!release.update_available) return false
    pendingAppUpdate = null
    updaterManualUrl.value = release.release_url
    updaterVersion.value = release.latest_version
    updaterNotes.value = release.notes || ''
    updaterStatus.value = `Доступна версия ${release.latest_version}. Автообновление для неё недоступно — скачайте установщик со страницы релизов.`
    showToast(`Найдена версия ${release.latest_version}`)
    return true
  } catch {
    return false
  }
}

async function checkLauncherUpdates(showToastOnNoUpdate = true) {
  if (!inDesktopTauri()) {
    if (showToastOnNoUpdate) showToast('Проверка обновлений доступна только в desktop launcher')
    return false
  }
  updaterBusy.value = true
  updaterProgressDownloaded.value = 0
  updaterProgressTotal.value = 0
  try {
    const update = await checkForAppUpdate()
    if (!update) {
      pendingAppUpdate = null
      updaterVersion.value = ''
      updaterNotes.value = ''
      if (await checkRemoteReleaseFallback()) return true
      updaterManualUrl.value = ''
      updaterStatus.value = 'Установлена последняя доступная версия.'
      if (showToastOnNoUpdate) showToast('Обновлений нет')
      return false
    }
    pendingAppUpdate = update
    updaterManualUrl.value = ''
    updaterVersion.value = String(update.version || '')
    updaterNotes.value = String(update.body || '')
    updaterStatus.value = `Доступна версия ${updaterVersion.value || 'новее текущей'}.`
    showToast(`Найдена версия ${updaterVersion.value || 'новее текущей'}`)
    return true
  } catch (error) {
    // без сервера обновлений не показываем пользователю необработанную ошибку
    const raw = errorMessage(error)
    if (await checkRemoteReleaseFallback()) return true
    if (/example\.invalid|dns|resolve|failed to fetch|endpoint/i.test(raw)) {
      updaterStatus.value = 'Самообновление в этой сборке не настроено — свежие версии выходят на странице проекта.'
      if (showToastOnNoUpdate) showToast('Самообновление недоступно в этой сборке')
    } else {
      updaterStatus.value = raw
      if (showToastOnNoUpdate) showToast(raw)
    }
    return false
  } finally {
    updaterBusy.value = false
  }
}

async function installLauncherUpdate() {
  if (!pendingAppUpdate) {
    if (updaterManualUrl.value) {
      if (!(await confirmDialog(`Открыть страницу загрузки версии ${updaterVersion.value || 'новее текущей'}?`, {
        title: 'Самообновление Nivora Launcher',
        detail: 'Для этой версии ещё нет автоматического обновления — установщик лежит на странице релизов GitHub.',
        confirmLabel: 'Открыть страницу',
      }))) return
      try {
        await bridge.openExternal(updaterManualUrl.value)
      } catch (error) {
        showToast(errorMessage(error), 'danger')
      }
      return
    }
    const found = await checkLauncherUpdates(false)
    if (!found || !pendingAppUpdate) return
  }
  const version = updaterVersion.value || 'новая версия'
  if (!(await confirmDialog(`Установить обновление ${version}?`, {
    title: 'Самообновление Nivora Launcher',
    detail: updaterNotes.value || 'После установки приложение перезапустится автоматически.',
    confirmLabel: 'Установить',
  }))) return
  updaterBusy.value = true
  updaterStatus.value = `Загружаю ${version}…`
  updaterProgressDownloaded.value = 0
  updaterProgressTotal.value = 0
  try {
    await pendingAppUpdate.downloadAndInstall((event: any) => {
      const phase = String(event?.event || '')
      if (phase === 'Started') {
        updaterProgressTotal.value = Number(event?.data?.contentLength || 0)
        updaterProgressDownloaded.value = 0
        updaterStatus.value = `Загрузка ${version} началась.`
      } else if (phase === 'Progress') {
        updaterProgressDownloaded.value += Number(event?.data?.chunkLength || 0)
        updaterStatus.value = updaterProgressTotal.value > 0
          ? `Скачано ${Math.min(updaterProgressDownloaded.value, updaterProgressTotal.value)} из ${updaterProgressTotal.value} байт.`
          : 'Скачиваю обновление…'
      } else if (phase === 'Finished') {
        updaterStatus.value = 'Пакет обновления скачан, устанавливаю…'
      }
    })
    updaterStatus.value = 'Обновление установлено. Перезапускаю лаунчер…'
    showToast('Обновление установлено, перезапуск…')
    await relaunch()
  } catch (error) {
    updaterStatus.value = errorMessage(error)
    showToast(errorMessage(error))
  } finally {
    updaterBusy.value = false
  }
}



  return { inDesktopTauri, checkLauncherUpdates, installLauncherUpdate }
}
