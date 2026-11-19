import { unzipSync, strFromU8 } from 'fflate'
import type { RuntimePluginManifest } from '../plugins/runtimeManifest'

const MAX_ZIP_BYTES = 30 * 1024 * 1024
const MAX_SOURCE_BYTES = 1024 * 1024

export type ThemePluginPreviewResult = { manifest: RuntimePluginManifest; source: string; name: string }

/** разбор архива плагина для предпросмотра в студии тем */
export function parseThemePluginZip(dataUrl: string): ThemePluginPreviewResult {
  const separator = dataUrl.indexOf(',')
  const base64 = separator >= 0 ? dataUrl.slice(separator + 1) : ''
  if (!base64) throw new Error('Файл пустой.')
  const binary = atob(base64)
  if (binary.length > MAX_ZIP_BYTES) throw new Error('Архив больше 30 МБ.')
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
  let files: Record<string, Uint8Array>
  try {
    files = unzipSync(bytes)
  } catch {
    throw new Error('Не удалось прочитать ZIP-архив.')
  }
  const manifestBytes = files['plugin.json']
  if (!manifestBytes) throw new Error('В архиве нет plugin.json.')
  let manifest: unknown
  try {
    manifest = JSON.parse(strFromU8(manifestBytes))
  } catch {
    throw new Error('plugin.json не читается.')
  }
  if (!manifest || typeof manifest !== 'object' || Array.isArray(manifest)) throw new Error('Манифест плагина некорректен.')
  const record = manifest as Record<string, unknown>
  const id = typeof record.id === 'string' ? record.id : ''
  const name = typeof record.name === 'string' ? record.name : ''
  if (!id || !/^[a-z\d][a-z\d._-]{1,63}$/i.test(id)) throw new Error('В манифесте нет корректного id.')
  if (!name) throw new Error('В манифесте нет имени плагина.')
  if (String(record.api_version ?? '') !== '3') throw new Error('Плагин собран под старую версию API; нужен api_version 3.')
  const entrypoint = record.entrypoint
  if (typeof entrypoint !== 'string' || !entrypoint.endsWith('.js')) throw new Error('В манифесте нет входного JS-файла.')
  const sourceBytes = files[entrypoint]
  if (!sourceBytes) throw new Error(`В архиве нет файла ${entrypoint}.`)
  if (sourceBytes.length > MAX_SOURCE_BYTES) throw new Error('Код плагина больше 1 МБ.')
  const permissions = record.permissions
  if (!Array.isArray(permissions) || permissions.some((entry) => typeof entry !== 'string')) throw new Error('Список разрешений в манифесте некорректен.')
  const allowed = new Set(['theme:effects', 'theme:tokens', 'theme:particles', 'theme:fonts', 'theme:wallpaper'])
  const unsupported = permissions.filter((entry) => !allowed.has(String(entry)))
  if (unsupported.length) {
    throw new Error(`Плагин требует разрешений вне темы (${unsupported.slice(0, 3).join(', ')}); в предпросмотре доступны только плагины для тем.`)
  }
  return {
    manifest: { ...record, id, name, entrypoint } as unknown as RuntimePluginManifest,
    source: strFromU8(sourceBytes),
    name,
  }
}
