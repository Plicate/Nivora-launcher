/** Короткие подсказки для пользователя; исходная ошибка остаётся в подробностях. */
export function repairErrorSummary(message: string): string {
  const text = message.toLowerCase()
  if (/остановлена пользователем|cancelled|canceled|aborted/.test(text)) return 'Починка остановлена. Уже выполненные изменения сохранятся.'
  if (/не предлагает java|не удалось определить подходящую версию java|не указана подходящая версия java/.test(text)) return 'Не удалось подобрать Java для этой сборки. Установите подходящую версию вручную.'
  if (/sha1|checksum|контрольн|хеш|hash mismatch/.test(text)) return 'Загруженный файл не прошёл проверку. Попробуйте ещё раз.'
  if (/network|timeout|timed out|connection|dns|сеть|интернет|соединени|загрузить|download|http error/.test(text)) return 'Не удалось загрузить нужные файлы. Проверьте интернет и повторите попытку.'
  if (/permission denied|access denied|нет доступа|доступ запрещ|нет прав\b|недостаточно прав|os error 13/.test(text)) return 'Нет доступа к файлам. Проверьте права на папку лаунчера.'
  if (/no space|недостаточно места|os error 28/.test(text)) return 'Недостаточно места на диске. Освободите место и повторите попытку.'
  return 'Не удалось применить исправление. Подробности можно открыть ниже.'
}
