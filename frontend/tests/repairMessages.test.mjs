import test from 'node:test'
import assert from 'node:assert/strict'
import { repairErrorSummary } from '../src/services/repairMessages.ts'
test('repair errors are understandable and retain generic fallback', () => {
  assert.match(repairErrorSummary('Connection timed out'), /интернет/)
  assert.match(repairErrorSummary('SHA1 mismatch'), /проверку/)
  assert.match(repairErrorSummary('Операция остановлена пользователем'), /остановлена/)
  assert.match(repairErrorSummary('unknown technical detail'), /Подробности/)
  assert.match(repairErrorSummary('permission denied: mods/a.jar'), /права на папку/)
  assert.match(repairErrorSummary('Нет доступа к файлу'), /права на папку/)
  assert.doesNotMatch(repairErrorSummary('Сервер доступно обновлён, версия 2.1'), /права на папку/)
})
