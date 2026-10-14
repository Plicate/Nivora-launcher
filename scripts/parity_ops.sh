#!/usr/bin/env bash
# Живой паритет пакета ops: jvm_presets_list, launch_history, cache_stats.
# Прогоняет эталонные функции и rust-движок на одних данных и сравнивает
# ответы структурно (cache_prune не сравнивается: он удаляет файлы, его
# поведение покрыто cargo-тестами с теми же правилами выбора).
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
FIXTURE="$(mktemp -d /tmp/nivora-parity-ops-XXXXXX)"

GAME="$FIXTURE/game"
INSTANCE="$GAME/instances/My-World"
CACHE="$FIXTURE/cache/downloads"
mkdir -p "$INSTANCE" "$CACHE/nested"

# История запусков: валидные объекты, мусорные строки, числа, null.
python3 - "$INSTANCE" <<'PY'
import json
import sys
from pathlib import Path

instance = Path(sys.argv[1])
history = [
    {"launch_id": "launch-1", "started_at": 1000.5, "loader": "fabric", "preset": "balanced", "memory": "4096MiB", "mc_version": "1.20.4", "exit_code": 0, "elapsed_seconds": 63.2, "time_to_window_seconds": 8.4},
    "повреждённая строка",
    42,
    None,
    {"launch_id": "launch-2", "started_at": 2000.0, "exit_code": None},
]
(instance / ".nivora-launcher-launch-history.json").write_text(json.dumps(history), encoding="utf-8")
PY

# Кеш: файлы разного размера, вложенность, мусорные расширения.
printf 'x%.0s' {1..1024} > "$CACHE/assets.json"
printf 'x%.0s' {1..2048} > "$CACHE/nested/lib.cache"
printf 'jar' > "$CACHE/client.jar"
printf '{}' > "$CACHE/meta.json"

# --- Python-сторона: те же функции, что питали вырезанные ветки ---
python3 - > "$FIXTURE/python-presets.json" <<'PY'
import json
from nivora_launcher.optimizer import JVM_PRESETS
print(json.dumps({"presets": list(JVM_PRESETS.values())}, ensure_ascii=False, indent=2))
PY

python3 - "$INSTANCE" > "$FIXTURE/python-history.json" <<'PY'
import json
import sys
from pathlib import Path
from nivora_launcher.optimizer import read_launch_history

print(json.dumps({"history": read_launch_history(Path(sys.argv[1]))}, ensure_ascii=False, indent=2))
PY

python3 - "$CACHE" > "$FIXTURE/python-cache.json" <<'PY'
import json
import sys
from pathlib import Path
from nivora_launcher.optimizer import cache_stats

print(json.dumps({"cache": cache_stats(Path(sys.argv[1]))}, ensure_ascii=False, indent=2))
PY

# --- Rust-сторона ---
export NIVORA_PARITY_PRESETS_OUT="$FIXTURE/rust-presets.json"
export NIVORA_PARITY_HISTORY_GAME="$GAME"
export NIVORA_PARITY_HISTORY_NAME="My World"
export NIVORA_PARITY_HISTORY_OUT="$FIXTURE/rust-history.json"
export NIVORA_PARITY_CACHE_DIR="$CACHE"
export NIVORA_PARITY_CACHE_OUT="$FIXTURE/rust-cache.json"
(cd "$ROOT/frontend/src-tauri" && "$HOME/.cargo/bin/cargo" test live_parity_ops_fixture -- --nocapture --quiet)

# --- Сравнение ---
python3 - "$FIXTURE" <<'PY'
import json
import sys

fixture = sys.argv[1]
failures = []

def load(name):
    with open(f"{fixture}/{name}", encoding="utf-8") as handle:
        return json.load(handle)

def compare(label, expected, actual):
    if expected != actual:
        failures.append(label)
        print(f"  {label}:\n    python = {json.dumps(expected, ensure_ascii=False)[:200]}\n    rust   = {json.dumps(actual, ensure_ascii=False)[:200]}")

compare("jvm_presets_list", load("python-presets.json"), load("rust-presets.json"))
compare("launch_history", load("python-history.json"), load("rust-history.json"))
py_cache = load("python-cache.json")["cache"]
rust_cache = load("rust-cache.json")["cache"]
for key in ("total_bytes", "file_count", "total_mib"):
    compare(f"cache_stats.{key}", py_cache[key], rust_cache[key])

if failures:
    print(f"ПАРИТЕТ НАРУШЕН: {failures}")
    sys.exit(1)
print("ПАРИТЕТ OK: jvm_presets_list, launch_history, cache_stats совпадают с Python")
print(f"  пресетов: {len(load('rust-presets.json')['presets'])} · записей истории: {len(load('rust-history.json')['history'])} · файлов кеша: {py_cache['file_count']}")
PY

# --- Домены: темы, профили настроек, справочники (r31) ---
DOMAIN="$FIXTURE/domain"
mkdir -p "$DOMAIN"

# Тема-образец и ZIP-архивы собирает эталонная реализация.
python3 - "$DOMAIN" <<'PY'
import io, json, sys, zipfile
from pathlib import Path

domain = Path(sys.argv[1])
theme = {
    "schema_version": 1, "id": "parity-theme", "name": "Parity Theme",
    "mode": "dark", "accent": "#4F7CFF", "accent_strong": "#3B5FD9",
    "background": "#0B0F1A", "panel": "#141B2E", "panel_soft": "#1B2338",
    "density": "comfortable", "sidebar_mode": "expanded",
}
metadata = {"author": "Паритет", "description": "Тема для сверки", "version": "1.0.0+meta", "creators": ["Первый", "Второй", "Первый"]}
(domain / "theme-save.json").write_text(json.dumps({"theme": theme, "metadata": metadata, "replace": False}), encoding="utf-8")
(domain / "theme-export.json").write_text(json.dumps({"theme_id": "parity-theme"}), encoding="utf-8")

profile = {"format": "nivora-settings-profile/1", "name": "Паритет!", "created_at": 100.0, "updated_at": 200.0,
           "settings": {"theme": "dark", "memory": 4096, "java_path": ""}}
(domain / "profile-import.json").write_text(json.dumps({"json": json.dumps(profile, ensure_ascii=False), "name": "  Свой  "}), encoding="utf-8")
(domain / "profile-export.json").write_text(json.dumps({"name": "Свой"}), encoding="utf-8")
PY

# --- Python-сторона: те же функции, что питали вырезанные ветки ---
python3 - "$DOMAIN" > "$FIXTURE/python-domain.json" <<'PY'
import json, sys
from pathlib import Path
sys.path.insert(0, ".")
from nivora_launcher.themes import ThemeStore
from nivora_launcher.settings_profiles import import_profile, list_profiles, export_profile
from nivora_launcher.server import core_options
from nivora_launcher.security import sandbox_status
from nivora_launcher.runtime import discover_java_paths, java_version

class Paths:
    def __init__(self, data, config):
        self.data_home = data
        self.config_home = config

class Cfg:
    def __init__(self, data, config, java_path=""):
        self.paths = Paths(data, config)
        self.settings = type("S", (), {"java_path": java_path})()

domain = Path(sys.argv[1])
report = {}

def result(fn):
    try:
        return fn()
    except Exception as exc:
        return {"__error__": str(exc)}

store_py = ThemeStore(Cfg(domain / "data-py", domain / "config-py"))
theme = json.loads((domain / "theme-save.json").read_text())["theme"]
metadata = json.loads((domain / "theme-save.json").read_text())["metadata"]
report["theme-save"] = result(lambda: {"theme": store_py.save_theme(theme, metadata), "themes": store_py.list()})
report["theme-list"] = result(lambda: {"themes": store_py.list()})
name, archive = store_py.export_archive("parity-theme")
import base64
(domain / "theme-import.json").write_text(json.dumps({"data": base64.b64encode(archive).decode("ascii"), "replace": True}), encoding="utf-8")
buf = io.BytesIO(archive) if False else None
import io, zipfile
with zipfile.ZipFile(io.BytesIO(archive)) as zf:
    report["theme-export"] = {"file_name": name, "zip": {info.filename: zf.read(info.filename).decode() for info in zf.infolist()}}
(domain / "theme.zip").write_bytes(archive)
report["theme-import"] = result(lambda: {"theme": store_py.import_archive(archive, replace=True), "themes": store_py.list()})

cfg = Cfg(domain / "data-py", domain / "config-rs")
report["profile-import"] = result(lambda: import_profile(cfg, (domain / "profile-import.json").read_text() and json.loads((domain / "profile-import.json").read_text())["json"], name="  Свой  "))
report["profiles-list"] = result(lambda: {"profiles": list_profiles(cfg)})
report["profile-export"] = result(lambda: export_profile(cfg, "Свой"))
report["server-cores"] = {"cores": core_options()}
options = [{"value": "auto", "label": "Автоматически", "meta": "Mojang runtime → системная Java"}]
seen = {"auto"}
for path in discover_java_paths():
    value = str(path)
    if value in seen:
        continue
    seen.add(value)
    version = java_version(path)
    options.append({"value": value, "label": f"Java {version}" if version else path.parent.parent.name, "meta": value})
report["java-options"] = {"options": options}
report["security-status"] = sandbox_status()
print(json.dumps(report, ensure_ascii=False, indent=2))
PY

# --- Rust-сторона доменов: корни через XDG направляем в fixture ---
mkdir -p "$DOMAIN/xdg-config" "$DOMAIN/xdg-data"
printf '{"java_path": "/opt/absent/java"}' > "$DOMAIN/xdg-config/settings.json"
(
  export XDG_CONFIG_HOME="$DOMAIN/xdg-config"
  export XDG_DATA_HOME="$DOMAIN/xdg-data"
  export NIVORA_PARITY_DOMAIN_ROOT="$DOMAIN"
  export NIVORA_PARITY_DOMAIN_OUT="$FIXTURE/rust-domain.json"
  cd "$ROOT/frontend/src-tauri" && "$HOME/.cargo/bin/cargo" test live_parity_domain_fixture -- --nocapture --quiet
)

python3 - "$FIXTURE" <<'PY'
import json, sys

fixture = sys.argv[1]
py = json.load(open(f"{fixture}/python-domain.json", encoding="utf-8"))
rs = json.load(open(f"{fixture}/rust-domain.json", encoding="utf-8"))
failures = []

def compare(label, expected, actual):
    if expected != actual:
        failures.append(label)
        print(f"  {label}:\n    python = {json.dumps(expected, ensure_ascii=False)[:260]}\n    rust   = {json.dumps(actual, ensure_ascii=False)[:260]}")

compare("theme-save", py["theme-save"], rs["theme-save"])
compare("theme-list", py["theme-list"], rs["theme-list"])
if "zip" in py["theme-export"] and "zip" in rs["theme-export"]:
    for side in (py, rs):
        parts = side["theme-export"]["zip"]
        side["theme-export"]["zip"] = {k: (json.loads(v) if k == "theme.json" else v) for k, v in parts.items()}
compare("theme-export", py["theme-export"], rs["theme-export"])
compare("theme-import", py["theme-import"], rs["theme-import"])

# Профили: метки времени «сейчас» различаются миллисекундами — гасим их,
# а json экспорта разбираем в объект (форматирование не часть контракта).
for side in (py, rs):
    for key in ("profile-import", "profiles-list"):
        for entry in side.get(key, {}).get("profiles", []):
            entry["updated_at"] = 0
    if isinstance(side.get("profile-export", {}).get("json"), str):
        side["profile-export"]["json"] = json.loads(side["profile-export"]["json"])
    if isinstance(side.get("profile-export", {}).get("json"), dict):
        side["profile-export"]["json"]["updated_at"] = 0
compare("profile-import", py["profile-import"], rs["profile-import"])
compare("profiles-list", py["profiles-list"], rs["profiles-list"])
compare("profile-export", py["profile-export"], rs["profile-export"])
compare("server-cores", py["server-cores"], rs["server-cores"])
compare("java-options", py["java-options"], rs["java-options"])
# Адрес bwrap и текст ошибки пробы зависят от машины — сравниваем факты.
for side in (py, rs):
    side["security-status"] = {k: v for k, v in side["security-status"].items() if k not in ("bubblewrap_path", "strict_probe_error")}
compare("security-status", py["security-status"], rs["security-status"])

if failures:
    print(f"ПАРИТЕТ ДОМЕНОВ НАРУШЕН: {failures}")
    sys.exit(1)
print("ПАРИТЕТ ДОМЕНОВ OK: темы, профили, server_cores, java_options, security_status совпадают с Python")
PY

# --- Домены: резервные копии миров и аккаунты (r32) ---
STATE="$FIXTURE/state"
for side in py rs; do
    GAME_SIDE="$STATE/game-$side"
    mkdir -p "$STATE/xdg-$side-config" "$STATE/xdg-$side-data" "$STATE/xdg-$side-cache"
    python3 - "$GAME_SIDE" "$STATE/xdg-$side-config" <<'PY'
import json, sys
from pathlib import Path

game, config = Path(sys.argv[1]), Path(sys.argv[2])
saves = game / "instances" / "Alpha" / "saves"
files = {
    "New World/level.dat": "мир данные",
    "New World/sub/deep.txt": "вложенный",
    "Second World/x.dat": "второй мир",
}
for relative, payload in files.items():
    target = saves / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(payload, encoding="utf-8")
settings = {
    "game_dir": str(game),
    "backup_schedule": {"enabled": True, "interval_hours": 999, "keep": 3, "destination": "weird"},
}
config_dir = config / "nivora-launcher"
config_dir.mkdir(parents=True, exist_ok=True)
(config_dir / "settings.json").write_text(json.dumps(settings, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
done

# --- Python-сторона: реальные хранилища через XDG-корня фикстуры ---
XDG_CONFIG_HOME="$STATE/xdg-py-config" XDG_DATA_HOME="$STATE/xdg-py-data" XDG_CACHE_HOME="$STATE/xdg-py-cache" \
PYTHONPATH="$ROOT" python3 - > "$FIXTURE/python-state.json" <<'PY'
import json, sys, time
from pathlib import Path

from nivora_launcher.accounts import AccountStore, OfflineDebugAccount, microsoft_login_status
from nivora_launcher.config import ConfigStore
from nivora_launcher.instances import InstanceStore
from nivora_launcher.world_backups import WorldBackupStore

config = ConfigStore()
instances = InstanceStore(config)
backups = WorldBackupStore(config, instances)
accounts = AccountStore(config)
report = {}

def result(fn):
    try:
        return fn()
    except Exception as exc:
        return {"__error__": str(exc)}

def strip_id(value):
    return value[16:] if isinstance(value, str) else value

def normalize(value):
    if not isinstance(value, dict):
        return value
    value = dict(value)
    if isinstance(value.get("backup"), dict):
        value["backup"] = dict(value["backup"])
        value["backup"]["backup_id"] = strip_id(value["backup"].get("backup_id"))
        value["backup"]["size_bytes"] = value["backup"].get("size_bytes", 0) > 0
        if isinstance(value["backup"].get("backups"), list):
            value["backup"]["backups"] = [normalize_entry(entry) for entry in value["backup"]["backups"]]
    if isinstance(value.get("backups"), list):
        value["backups"] = [normalize_entry(entry) for entry in value["backups"]]
    if isinstance(value.get("schedule"), dict):
        value["schedule"] = dict(value["schedule"], last_run=0)
    return value

def normalize_entry(entry):
    return {"id": strip_id(entry.get("id")), "created_at": 0, "size_bytes": entry.get("size_bytes", 0) > 0, "destination": entry.get("destination")}

created = backups.create("Alpha", destination="local", reason="manual")
report["backup-create"] = normalize({"backup": created, "schedule": backups.schedule("Alpha")})
backup_id = created.get("backup_id", "")
report["backup-list"] = normalize({"backups": backups.list("Alpha"), "schedule": backups.schedule("Alpha")})
report["schedule-set"] = normalize(result(lambda: {"schedule": backups.set_schedule("Alpha", {"interval_hours": 48, "keep": 2, "destination": "local"}, override=True)}))
report["schedule-get"] = normalize(result(lambda: {"schedule": backups.schedule("Alpha")}))

world = Path(config.game_dir) / "instances" / "Alpha" / "saves" / "New World" / "level.dat"
world.write_text("сломано", encoding="utf-8")
report["backup-restore"] = normalize(result(lambda: backups.restore("Alpha", backup_id)))
report["restored-content"] = world.read_text(encoding="utf-8")
report["backup-delete"] = normalize(result(lambda: backups.delete("Alpha", backup_id)))

def account_public(value):
    return {
        "account_id": value.account_id, "kind": value.kind, "username": value.username,
        "alias": value.alias, "display_name": value.alias or value.username,
        "expires_at": value.expires_at, "limit_seconds": value.limit_seconds,
        "used_seconds": value.used_seconds, "is_default": value.is_default,
    }

report["account-create"] = result(lambda: {"account": account_public(OfflineDebugAccount(config, accounts).create("  Игрок_77!  "))})
accounts_path = Path(config.paths.accounts_file)
records = json.loads(accounts_path.read_text(encoding="utf-8"))
records.append({"account_id": "beta", "kind": "offline", "username": "Beta", "alias": "Бета", "expires_at": 0, "xuid": "", "used_seconds": 0, "limit_seconds": 0, "is_default": False})
accounts_path.write_text(json.dumps(records, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
report["account-default"] = result(lambda: {"account": account_public(accounts.set_default("beta")), "accounts": [account_public(value) for value in accounts.all()]})
settings_value = json.loads(Path(config.paths.settings_file).read_text(encoding="utf-8"))
report["settings-default-account"] = settings_value.get("default_account_id")
report["account-move"] = result(lambda: {"accounts": [account_public(value) for value in accounts.move("beta", "up")]})
report["account-delete"] = result(lambda: {"removed": accounts.remove("offline-debug")})
report["accounts-file"] = accounts_path.read_text(encoding="utf-8")

status_path = Path(config.paths.cache_home) / "microsoft-login.json"
status_path.parent.mkdir(parents=True, exist_ok=True)
status_path.write_text(json.dumps({"status": "device_code", "code": "XYZ 123"}), encoding="utf-8")
report["login-status"] = microsoft_login_status(config)
print(json.dumps(report, ensure_ascii=False, indent=2))
PY

# --- Rust-сторона: те же XDG-корня, тот же сценарий ---
(
  export XDG_CONFIG_HOME="$STATE/xdg-rs-config"
  export XDG_DATA_HOME="$STATE/xdg-rs-data"
  export XDG_CACHE_HOME="$STATE/xdg-rs-cache"
  export NIVORA_PARITY_STATE_OUT="$FIXTURE/rust-state.json"
  cd "$ROOT/frontend/src-tauri" && "$HOME/.cargo/bin/cargo" test live_parity_backup_fixture -- --nocapture --quiet
)

python3 - "$FIXTURE" <<'PY'
import json, sys

fixture = sys.argv[1]
py = json.load(open(f"{fixture}/python-state.json", encoding="utf-8"))
rs = json.load(open(f"{fixture}/rust-state.json", encoding="utf-8"))
failures = []

def compare(label, expected, actual):
    if expected != actual:
        failures.append(label)
        print(f"  {label}:\n    python = {json.dumps(expected, ensure_ascii=False)[:260]}\n    rust   = {json.dumps(actual, ensure_ascii=False)[:260]}")

for key in ("backup-create", "backup-list", "schedule-set", "schedule-get", "backup-restore", "restored-content", "backup-delete",
            "account-create", "account-default", "settings-default-account", "account-move", "account-delete",
            "accounts-file", "login-status"):
    compare(key, py.get(key), rs.get(key))

if failures:
    print(f"ПАРИТЕТ БЭКАПОВ/АККАУНТОВ НАРУШЕН: {failures}")
    sys.exit(1)
print("ПАРИТЕТ БЭКАПОВ/АККАУНТОВ OK: world_backup (list/create/restore/delete/schedule) и accounts (create/move/default/delete/status) совпадают с Python")
PY

echo "fixture: $FIXTURE"
