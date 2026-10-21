# Статус переноса Python → нативные языки (полиглот)

Python-бэкенд (`nivora_launcher/`) — это «воркер» с JSON-протоколом операций
`{operation, ...поля}`. Нативный слой забирает операции без интерпретатора:
операция добавляется в `NATIVE_OPS` и `handle()` в `rust_engine/mod.rs` —
Python-воркер для неё больше не поднимается.

## Полиглотная политика (выбор языка под задачу)

Язык выбирается по природе задачи, а не «всё в один»:

| Язык | Где живёт | Что берёт на себя | Почему именно он |
|---|---|---|---|
| **Rust** | `frontend/src-tauri/src/rust_engine/` | оркестрация операций, состояние, процессы, файловые операции, криптография (ed25519/HMAC), TLS-клиент | движок и так Rust-крейт внутри Tauri: in-process-вызов без запуска процесса, нулевой оверхед |
| **C++** | `cpp-turbo/` (`nivora-turbo`) | SIMD SHA-256 (`sha256rnds2`), батч-скан jar-централов, горячие циклы сканирования | ручной контроль над SIMD/памятью в чисто CPU-ных циклах; один процесс на пачку файлов |
| **Go** | `go-modcat/` (`nivora-modcat`) | каталог Modrinth, конкурентные сетевые запросы, батч-проверки обновлений | горутины + net/http: сотни параллельных запросов пишутся в десятках строк, статический бинарник без зависимостей |
| **Java** | `java/ModScan.java` | JVM-интроспекция: разбор class-файлов, требуемая версия Java по major version | jar и class-файлы — родная среда JVM; снаружи это не посчитать |

Правило: Rust-движок всегда оркестрирует; тяжёлая работа делегируется
хелперам на C++/Go/Java, когда она CPU-ная (SIMD), сетево-конкурентная или
JVM-специфичная. Запуск JVM ради одной операции (~200 мс на старт) —
антипаттерн, поэтому одноразовые операции не выносятся в Java.

## Правила порта (сложившиеся в проекте)

1. Один Python-модуль → один файл своего языка (`rust_engine/*.rs`,
   subcommand в `cpp-turbo/nivora-turbo.cpp`, cmd в `go-modcat/main.go`);
   имена функций повторяют Python (`build_recording_args`, `pick_encoder`, …).
2. Сообщения об ошибках — русские, дословно как в Python (фронтенд и тесты
   на них опираются).
3. Операция добавляется в `NATIVE_OPS` + match в `handle()`; allowlist в
   `lib.rs` уже содержит имена (проверяет тест
   `rust_native_operations_are_allowlisted_...`).
4. Python-код воркера НЕ удаляется: он остаётся фолбэком для сборок без
   нативного слоя и умирает последним вместе с `gui_service.py`.
5. События UI — через `events::emit_status_event(...)` (эквивалент
   `scoped_emit` воркера).
6. Тесты: юнит-тесты чистых функций + интеграционный цикл через `handle()`
   с подменой внешних бинарников shell-скриптами (или явными корнями через
   `ConfigStore::from_paths` — без мутации env).

## Готово

| Python-модуль | Строк | Нативный файл | Операции |
|---|---|---|---|
| errors.py | 8 | — (контракт ошибок = `Result<Value, String>`) | — |
| operations.py | 56 | operations.rs | отмена операций |
| release_check.py | 58 | release_check.rs | check_remote_release |
| constants.py | 61 | constants.rs | — |
| plugin_reload.py | 66 | plugin_reload.rs | plugin_dev_changes |
| turbo.py | 81 | **turbo.rs** → C++ `nivora-turbo` | — (пакетное SHA-256; потребитель — cloud_storage) |
| process_guard.py | 83 | process_guard.rs | plugin_api_quota_usage |
| plugin_guard.py | 84 | plugin_guard.rs | — |
| portable_fs.py | 120 | portable_fs.rs | — |
| loaders.py | 127 | loaders.rs | loader_choices |
| settings_profiles.py | 129 | profiles.rs | settings_profile_* |
| platform_support.py | 133 | platform_support.rs | — |
| crash_reports.py | 164 | crash_reports.rs | — |
| metadata.py | 168 | metadata.rs | versions |
| modpack_export.py | 181 | modpack_export.rs | export_modpack |
| bedrock.py | 222 | bedrock.rs | bedrock_* |
| mod_dependencies.py | 232 | mod_dependencies.rs | mod_dependency_graph, mod_removal_impact |
| upnp.py | 240 | upnp.rs | — |
| skins.py | 285 | skins.rs | skins_list, skins_import, skins_export, skin_profile, apply_skin, clear_skin, apply_cape, clear_cape, open_skins_folder, skins_folder |
| recorder.py | 293 | recorder.rs | recording_start, recording_stop, recording_status |
| gui_cloud_ops.py | 114 | cloud.rs (вместе с cloud_storage.py) | cloud_status, cloud_configure, cloud_test, cloud_list, cloud_sync, cloud_restore, cloud_delete, cloud_disconnect |
| server_traffic.py | 298 | server_traffic.rs | — |
| world_import.py | 304 | world_import.rs | world_import_path |
| world_backups.py | 313 | backups.rs | world_backup_* |
| shader_analysis.py | 324 | shader_analysis.rs | shader_analyze |
| server_network.py | 350 | server_network.rs | server_network_* |
| metrics.py | 444 | metrics.rs | system_metrics |
| runtime.py | 460 | runtime.rs | — |
| snapshots.py | 516 | snapshots.rs | server_snapshot_* |
| themes.py | 580 | themes.rs | theme_* |
| instance_files.py | 668 | instance_files.rs | instance_file_*, instance_log_* |
| plugin_security.py | 809 | plugin_security.rs | plugin_security_* |
| mods.py (каталог) | 825 | mods.rs + Go `nivora-modcat` | mods_search, mods_details, mods_check_updates |
| optimizer.py | 828 | optimization.rs (+ Java `ModScan.java`) | optimization_analyze |
| instances.py | 835 | instances.rs | instance_groups, *_instance_group, delete/duplicate_instance, external_instances_scan |
| net.py | 985 | net.rs | — (HttpClient движка) |
| accounts.py | 1033 | accounts.rs | create_offline_account, delete/move/set_default_account, login_* |
| plugin_catalogs.py | 1296 | plugin_catalogs.rs + modcat.rs | каталоги плагинов |
| config.py | 1392 | config.rs | save_settings |
| installer.py | 1415 | installer.rs | install |
| server.py | 1508 | server.rs | server_start/stop/restart/command/log/status, server_core_versions |
| cloud_storage.py | 782 | cloud.rs (вместе с gui_cloud_ops.py) | — (WebDAV-клиент, манифест, sync/restore) |
| github_plugins.py | 920 | github_plugins.rs | github_plugin_* (поиск/детали/установка/OAuth device flow/публикация) |
| plugins.py | 2127 | plugins.rs | plugin_list |
| instance_workspace.py | 888 | **instance_workspace.rs** (этот запрос) | instance_details, save_instance_settings, remove_instance_asset, toggle_mod, import_asset, mods_remove_files |
| — (новое, без Python-прототипа) | — | **shortcuts.rs** | instance_shortcut_status/create/remove, shortcut_take_launch_request (ярлыки .lnk/.app/.desktop, `--launch-instance`) |
| — (новое, без Python-прототипа) | — | **mod_list.rs** | instance_mod_list_export (markdown/csv/txt, манифесты через nivora-turbo scan-jars) |
| — (новое) | — | smoke.rs (src/) | `--smoke-test`: дымовой прогон упакованного приложения через тот же мост |

Примечания к портам:

- cloud: подпись манифеста — HMAC-SHA256 (своя реализация на sha2, сверена
  с RFC 4231); пароль WebDAV — libsecret/Credential Manager; редиректы —
  только тот же origin и https; `put_file` читает файл в память (≤512 МиБ);
  фолбэк `cloud_remote_path` — дефолт движка `nivora-launcher`.
- github_plugins: OAuth device flow, установка релизных ZIP с проверкой
  манифеста/подписи (`validate_plugin_archive`), секреты через
  `secret-tool`/`windows_keyring`.
- instance_workspace: NBT-ридер (`level.dat`/`servers.dat`, gzip через
  flate2 с лимитами и дословными сообщениями), метаданные ресурсов из
  zip-манифестов (fabric/quilt/mcmod/pack.mcmeta) и sidecar-файлов,
  `.disabled`-переключение модов, миграция security-ключей из легаси-файла
  настроек в защищённое хранилище (политика v4). `installed_projects`/
  `remove_installed_project` уже портированы и ждут потребителя — порт
  modpacks.py. Для тестов без мутации env добавлен `ConfigStore::from_paths`.

## Очередь (язык выбран по политике выше)

| Python-модуль | Строк | Язык | Обоснование |
|---|---|---|---|
| launcher.py | 1070 | Rust | порождение JVM-процесса, env, classpath, супервизия — родная среда `std::process` |
| shader_benchmark.py | 266 | Rust | сэмплирование CPU/GPU/RAM по procfs вне JVM (`_process_sample` уже в metrics.rs); манипуляция options.txt — текст; портировать парой с launcher.py |
| security.py | 854 | Rust | OS-проверки песочниц, метаданные ФС |
| plugin_dev.py | 758 | Rust | ed25519/zip уже в plugin_security.rs; JVM-стартап на операцию медленнее |
| modpacks.py | 1582 | Go + Rust | конкурентные скачивания .mrpack/каталогов — горутины в `nivora-modcat` (новый cmd); оркестрация/протокол — Rust |
| auto_repair.py | 1637 | C++ + Rust | глубокий скан jar-централов (packages/mixins/loaders — расширение `scan-jars`) в `nivora-turbo`; правила diagnose/apply — Rust |
| mod_audit.py | 1882 | C++ + Rust | `scan_sources` (паттерн-матчинг по .jasm — самый горячий цикл), `_cpu_benchmark`, `_secure_wipe` — в `nivora-turbo`; сессии/протокол — Rust |
| windows_sandbox.py | 656 | Rust (Windows-only) | AppContainer-хелпер; независимо от остальных |
| cli.py | 562 | — (умрёт с воркером) | CLI-фронтенд воркера, desktop не зависит |
| gui_service.py | 4228 | — (последний) | сам воркер — удаляется, когда всё выше нативно |

## Проверка после каждого порта

```bash
cd frontend/src-tauri
cargo check --lib            # сборка
cargo test --lib             # все тесты движка
cargo clippy --lib           # без новых замечаний в портированных файлах
# хелперы:
bash scripts/build_turbo.sh  # C++ nivora-turbo
cd go-modcat && go vet && go build  # Go nivora-modcat
```
