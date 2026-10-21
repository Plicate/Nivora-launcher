"""Дымовой тест упакованного Python-бэкенда: JSON-протокол stdin/stdout.

Сборочный glue для CI и build-скриптов: запускает собранный бинарник так же,
как его запускает Tauri, отправляет локальные операции воркера и проверяет
конверт ответа. Использование: smoke_backend.py <бэкенд> [аргументы...].
"""

import json
import os
import subprocess
import sys
import tempfile

OPERATIONS = ["server_plugin_catalogs", "plugin_security_status", "instance_groups"]


def main(argv: list[str]) -> int:
    if not argv:
        print("использование: smoke_backend.py BACKEND [АРГУМЕНТЫ...]", file=sys.stderr)
        return 2
    with tempfile.TemporaryDirectory() as home:
        environment = dict(os.environ)
        for key in ("HOME", "XDG_DATA_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA"):
            environment[key] = os.path.join(home, key.lower())
            os.makedirs(environment[key], exist_ok=True)
        environment["PYTHONUTF8"] = "1"
        environment.setdefault("PYTHONPATH", os.getcwd())
        payload = "".join(json.dumps({"operation": name, "operation_id": f"smoke-{index}"}) + "\n" for index, name in enumerate(OPERATIONS))
        completed = subprocess.run(
            argv, input=payload, capture_output=True, text=True, encoding="utf-8", timeout=180, env=environment
        )
    responses = [json.loads(line) for line in completed.stdout.splitlines() if line.strip() and '"event"' not in line[:12]]
    if len(responses) != len(OPERATIONS):
        print(f"ожидалось {len(OPERATIONS)} ответов, получено {len(responses)}\nstderr:\n{completed.stderr[-4000:]}", file=sys.stderr)
        return 1
    failed = 0
    for name, response in zip(OPERATIONS, responses):
        ok = isinstance(response, dict) and "ok" in response
        print(f"{'OK' if ok else 'ОШИБКА'}: {name}: ok={response.get('ok')} error={response.get('error', '')!s:.200}")
        failed += not ok
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
