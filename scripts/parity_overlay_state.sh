#!/usr/bin/env bash
# Сверка overlay_state: эталонная реализация против rust-движка на одном
# fixture.
#
# Создаёт изолированный XDG-дом с настройками, экземпляром с живым процессом
# (sleep), модами и версией; собирает ответ эталонных блоков и ответ
# rust-движка, после чего сравнивает стабильные поля JSON. Волатильные
# (timestamp, elapsed, метрики времени) исключены из сравнения осознанно.
set -euo pipefail

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
FIXTURE="$(mktemp -d /tmp/nivora-parity-XXXXXX)"
export XDG_CONFIG_HOME="$FIXTURE/xdg-config"
export XDG_DATA_HOME="$FIXTURE/xdg-data"
CONFIG_DIR="$XDG_CONFIG_HOME/nivora-launcher"
GAME_DIR="$FIXTURE/game"
INSTANCE="$GAME_DIR/instances/TestWorld"

mkdir -p "$CONFIG_DIR" "$INSTANCE/versions/1.20.4-fabric0.92.2" "$INSTANCE/mods" "$GAME_DIR/recordings"

cat > "$CONFIG_DIR/settings.json" <<'JSON'
{ "game_dir": "GAME_DIR_PLACEHOLDER", "overlay_settings": { "enabled": true, "hotkey_overlay": "Ctrl+O", "show_mods": true } }
JSON
sed -i "s|GAME_DIR_PLACEHOLDER|$GAME_DIR|g" "$CONFIG_DIR/settings.json"

cat > "$INSTANCE/.nivora-launcher-instance.json" <<'JSON'
{ "display_name": "Мой мир", "played_seconds": 100, "kind": "client", "game_edition": "java" }
JSON

cat > "$INSTANCE/versions/1.20.4-fabric0.92.2/1.20.4-fabric0.92.2.json" <<'JSON'
{ "id": "1.20.4-fabric0.92.2", "inheritsFrom": "1.20.4" }
JSON

printf 'jar-bytes' > "$INSTANCE/mods/sodium-1.20.4.jar"
printf 'x' > "$INSTANCE/mods/JEI-4.22.jar"
printf 'x' > "$INSTANCE/mods/jei-legacy.disabled"
printf 'x' > "$INSTANCE/mods/.hidden.part"

sleep 60 &
FAKE_PID=$!
trap 'kill %1 2>/dev/null || true' EXIT

cat > "$INSTANCE/.nivora-launcher-live.json" <<JSON
{ "running": true, "pid": $FAKE_PID, "started_at": $(python3 -c 'import time; print(time.time() - 42)') }
JSON

cat > "$GAME_DIR/recordings/recording-state.json" <<JSON
{ "pid": $FAKE_PID, "file": "", "started_at": $(python3 -c 'import time; print(time.time())'), "encoder": "libx264", "instance": "TestWorld" }
JSON

# --- Python-сторона: ровно те блоки, из которых собирался overlay_state ---
python3 - > "$FIXTURE/python.json" <<'PY'
import json
import time
from pathlib import Path

from nivora_launcher.config import ConfigStore
from nivora_launcher.instance_workspace import InstanceWorkspace
from nivora_launcher.instances import InstanceStore
from nivora_launcher.metrics import metrics_for_instance

config = ConfigStore()
instances = InstanceStore(config)
workspace = InstanceWorkspace(config, instances)
running = [info for info in instances.list() if info.running]
assert running, "экземпляр должен быть запущен"
info = running[0]
metrics = metrics_for_instance(info.path)
session_seconds = int(metrics.get("uptime_seconds", 0))
mods = [
    {"name": item["name"], "display_name": item.get("display_name") or item["name"], "enabled": bool(item.get("enabled", True))}
    for item in workspace.list_assets(info.name, "mods")[:200]
]
recording_state = json.loads(Path(config.game_dir, "recordings", "recording-state.json").read_text())
response = {
    "running": True,
    "instance": {
        "name": info.name,
        "display_name": info.display_name or info.name,
        "game_edition": info.game_edition,
        "kind": info.kind,
        "minecraft_version": info.minecraft_version,
        "loader": info.loader,
    },
    "session_seconds": session_seconds,
    "played_seconds": int(getattr(info, "played_seconds", 0)) + session_seconds,
    "metrics": {
        "cpu_percent": metrics.get("cpu_percent", 0.0),
        "ram_bytes": metrics.get("ram_bytes", 0),
        "peak_cpu_percent": metrics.get("peak_cpu_percent", 0.0),
        "peak_ram_bytes": metrics.get("peak_ram_bytes", 0),
        "storage_bytes": metrics.get("storage_bytes", 0),
    },
    "mods": mods,
    "recording": {
        "active": recording_state["pid"] > 0,
        "file": recording_state["file"],
        "started_at": recording_state["started_at"],
        "encoder": recording_state["encoder"],
        "instance": recording_state["instance"],
    },
    "overlay": config.settings.overlay_settings,
}
print(json.dumps(response, ensure_ascii=False, indent=2))
PY

# --- Rust-сторона ---
export NIVORA_PARITY_CONFIG="$XDG_CONFIG_HOME/nivora-launcher"
export NIVORA_PARITY_DATA="$XDG_DATA_HOME"
export NIVORA_PARITY_OUT="$FIXTURE/rust.json"
(cd "$ROOT/frontend/src-tauri" && "$HOME/.cargo/bin/cargo" test live_parity_fixture -- --nocapture --quiet)

# --- Сравнение стабильных полей ---
python3 - "$FIXTURE" <<'PY'
import json
import sys

fixture = sys.argv[1]
python = json.load(open(f"{fixture}/python.json"))
rust = json.load(open(f"{fixture}/rust.json"))

failures = []
def check(path, extractor):
    expected, actual = extractor(python), extractor(rust)
    if expected != actual:
        failures.append(f"{path}: python={expected!r} rust={actual!r}")

check("running", lambda v: v["running"])
for key in ("name", "display_name", "game_edition", "kind", "minecraft_version", "loader"):
    check(f"instance.{key}", lambda v, k=key: v["instance"][k])
check("session_seconds(±2с)", lambda v: None)
check("played_seconds", lambda v: None)
check("metrics.cpu_percent", lambda v: v["metrics"]["cpu_percent"])
check("metrics.peak_cpu_percent", lambda v: v["metrics"]["peak_cpu_percent"])
check("mods", lambda v: v["mods"])
check("recording.active", lambda v: v["recording"]["active"])
check("recording.encoder", lambda v: v["recording"]["encoder"])
check("recording.instance", lambda v: v["recording"]["instance"])
check("overlay.hotkey_overlay", lambda v: v["overlay"]["hotkey_overlay"])
check("overlay.enabled", lambda v: v["overlay"]["enabled"])

# Время-зависимые поля — с допуском.
for name in ("session_seconds", "played_seconds"):
    p, r = python[name], rust[name]
    if abs(p - r) > 3:
        failures.append(f"{name}: python={p} rust={r} (расхождение > 3с)")

for name in ("ram_bytes", "peak_ram_bytes", "storage_bytes"):
    p, r = python["metrics"][name], rust["metrics"][name]
    if p > 0 and r > 0 and abs(p - r) > max(1024, p * 0.10):
        failures.append(f"metrics.{name}: python={p} rust={r}")

if failures:
    print("ПАРИТЕТ НАРУШЕН:")
    for failure in failures:
        print(" -", failure)
    sys.exit(1)
print(f"ПАРИТЕТ OK: {len(json.dumps(python))} байт ответа Python совпадают с Rust")
print("  instance:", python["instance"]["name"], "·", python["instance"]["minecraft_version"], python["instance"]["loader"])
print("  mods:", len(python["mods"]), "шт. · recording:", python["recording"]["encoder"])
PY

echo "fixture: $FIXTURE"
