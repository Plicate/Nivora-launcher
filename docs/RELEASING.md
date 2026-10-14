# Как выпустить релиз с самообновлением

Фид апдейтера: `https://github.com/vedrooou0/Nivora-launcher/releases/latest/download/latest.json`
(прописан в `frontend/src-tauri/tauri.conf.json` → `plugins.updater`).
Публичный ключ апдейтера уже вшит в конфиг; приватный — только у мейнтейнера.

## Разовая настройка репозитория

1. GitHub → **Settings → Secrets and variables → Actions** → New secret:
   - `TAURI_SIGNING_PRIVATE_KEY` — **содержимое** файла приватного ключа
     (`nivora-updater-private.key`, base64-блок целиком);
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — пароль ключа.
2. Убедитесь, что workflow `.github/workflows/release.yml` в репозитории
   (он собирает Windows NSIS/MSI и Linux AppImage/deb/rpm, подписывает их
   и публикует `latest.json` в Release).

Приватный ключ и пароль терять нельзя: без них выпустить обновление,
которое примут уже установленные копии лаунчера, будет невозможно.
Ключ не коммитится и не попадает в артефакты сборки.

## Выпуск версии

```bash
# 1. Поднять версию (все три места синхронно)
#    frontend/package.json, frontend/src-tauri/tauri.conf.json,
#    nivora_launcher/constants.py (APP_VERSION) + BUILD_STAMP
# 2. Закоммитить и запушить
git add -A && git commit -m "release: 1.0.1" && git push
# 3. Тег запускает релизную сборку
git tag v1.0.1 && git push origin v1.0.1
```

Workflow соберёт пакеты, подпишет их, создаст Release с `latest.json`.
Готово: установленные лаунчеры увидят версию через
Настройки → Самообновление.

## Замечания

- **Linux**: автообновление работает только для AppImage — `latest.json`
  ссылается на AppImage; deb/rpm обновляются ручной переустановкой.
- Первый «нулевой» релиз (например, `v1.0.0`) стоит выпустить сразу после
  настройки секретов, чтобы фид перестал отдавать 404 и кнопка
  «Проверить обновления» оживала с честным ответом.
- Если ключ скомпрометирован: сгенерировать новую пару
  (`npm run tauri -- signer generate -w …`), заменить `pubkey` в
  `tauri.conf.json`, поднять мажорную версию — старые сборки обновятся
  на неё, подписанные старым ключом пакеты больше не принимаются.
