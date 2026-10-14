"""Разбор лога упавшего экземпляра: короткий вердикт и конкретные советы.

Паттерны сверяются с реальными сообщениями Fabric/Forge/NeoForge и ванильного
сервера; ничего не выдумываем — если совпадений нет, честно пишем «неизвестная
ошибка» и показываем выдержку лога.
"""
from __future__ import annotations

import re
from pathlib import Path
from typing import Any

# (заголовок, формат строки с деталями) для зависимостей загрузчиков
_FABRIC_REQUIRED = re.compile(r"mod\s+'([^']+)'\s+(\d[\w.\-+]*)?\s*(?:requires|depends)", re.IGNORECASE)
_FABRIC_INCOMPATIBLE = re.compile(r"Mod\s+([A-Za-z0-9_.\-]+)\s+(?:requires|incompatible)", re.IGNORECASE)
_MISSING_GENERAL = re.compile(r"(?:Missing or unsupported mandatory dependencies|missing dependency)[:\s]*(.*)", re.IGNORECASE)
_MOD_MENTION = re.compile(r"\bmod\s+'?([A-Za-z0-9_ .\-]{2,48})'?(\s+\S+)?\s+(?:requires|is incompatible|caused)", re.IGNORECASE)

MAX_EXCERPT_LINES = 24


def analyze_crash(
    lines: list[str],
    exit_code: int | None,
    *,
    instance_path: str | Path | None = None,
    memory: str = "",
    minecraft_version: str = "",
    loader: str = "",
) -> dict[str, Any]:
    text = "\n".join(lines[-400:])
    hints: list[dict[str, str]] = []
    suspected_mods: list[dict[str, str]] = []
    title = ""

    lowered = text.lower()

    if "outofmemoryerror" in lowered or "could not reserve enough space for object heap" in lowered:
        title = "Java не хватило памяти"
        hints.append({
            "kind": "memory",
            "title": "Увеличьте память экземпляра",
            "detail": f"Выделено сейчас: {memory or 'auto'}. Тяжёлым сборкам обычно нужно 4–8 ГБ."
            + (" Пресет Heavy Modpack тоже помогает GC." if loader != "vanilla" else ""),
        })

    if "unsupportedclassversionerror" in lowered or re.search(r"compiled by a more recent version of the java runtime", lowered):
        java_hint = "Игра требует более новую Java, чем выбранная."
        match = re.search(r"class file version (\d+)", lowered)
        if match:
            # 61=17, 65=21, 66=22
            major = int(match.group(1)) - 44
            if major >= 8:
                java_hint += f" Нужна Java {major} или новее."
        title = title or "Несовместимая версия Java"
        hints.append({"kind": "java", "title": "Смените Java", "detail": java_hint})

    if re.search(r"missing or unsupported mandatory dependencies|some of your mods are incompatible", lowered):
        title = title or "Конфликт модов"
        hints.append({
            "kind": "mods",
            "title": "Несовместимые или отсутствующие зависимости",
            "detail": "Загрузчик перечислил проблемные моды ниже. Отключите или удалите их и попробуйте снова.",
        })
        suspected_mods = _extract_mods(text)

    if re.search(r"failed to bind to port|maybe another server is running", lowered):
        title = title or "Порт сервера занят"
        hints.append({"kind": "port", "title": "Смените порт", "detail": "Порт уже используется другим процессом — измените server_port в настройках сервера."})

    if "you need to agree to the eula" in lowered:
        title = title or "Не принято EULA"
        hints.append({"kind": "eula", "title": "Примите EULA", "detail": "Откройте настройки сервера и отметьте согласие с EULA."})

    if re.search(r"no opengl context|could not create gl|glfw error|failed to create window|opengl.*not supported", lowered):
        title = title or "Проблема с графикой"
        hints.append({"kind": "gpu", "title": "Обновите драйвер видеокарты", "detail": "Minecraft не смог создать OpenGL-контекст. Также попробуйте переключить режим графики в настройках экземпляра."})

    if re.search(r"corrupted|failed to download|checksum mismatch", lowered):
        title = title or "Повреждённые файлы"
        hints.append({"kind": "files", "title": "Проверьте файлы", "detail": "Часть файлов не скачалась или повреждена. Переустановите версию или повторите загрузку."})

    hs_err = re.search(r"hs_err_pid\d+", lowered)
    if hs_err or "sigsegv" in lowered or "exception_access_violation" in lowered:
        title = title or "Аварийное завершение JVM"
        hints.append({
            "kind": "jvm",
            "title": "Упала сама Java, не Minecraft",
            "detail": "Уберите кастомные JVM-аргументы или смените пресет GC; при повторении обновите Java."
            + (" Отчёт JVM: " + hs_err.group(0) if hs_err else ""),
        })

    if not title:
        if exit_code:
            title = f"Экземпляр завершился с кодом {exit_code}"
        else:
            title = "Экземпляр завершился с ошибкой"
        hints.append({
            "kind": "generic",
            "title": "Явных признаков не найдено",
            "detail": "Посмотрите выдержку лога — обычно причина в последних 20 строках перед падением.",
        })

    return {
        "exit_code": exit_code,
        "title": title,
        "hints": hints,
        "suspected_mods": [mod for mod in suspected_mods if _match_mod_file(mod, instance_path)],
        "excerpt": _excerpt(lines),
    }


def _excerpt(lines: list[str]) -> list[str]:
    if not lines:
        return []
    tail = lines[-MAX_EXCERPT_LINES:]
    # последняя запись об ошибке часто в середине — оставляем и её контекст
    for index in range(len(lines) - 1, max(-1, len(lines) - MAX_EXCERPT_LINES * 4), -1):
        if re.search(r"error|exception|caused by", lines[index], re.IGNORECASE):
            window = lines[max(0, index - 4) : index + 8]
            return [*window, "…", *tail[-6:]] if window[-1] != lines[-1] else window
    return tail


def _extract_mods(text: str) -> list[dict[str, str]]:
    found: list[dict[str, str]] = []
    seen: set[str] = set()
    for line in text.splitlines():
        for pattern in (_FABRIC_REQUIRED, _FABRIC_INCOMPATIBLE, _MOD_MENTION, _MISSING_GENERAL):
            match = pattern.search(line)
            if not match:
                continue
            name = match.group(1).strip(" '\"")
            if len(name) < 2 or name.lower() in seen:
                continue
            seen.add(name.lower())
            found.append({"name": name, "reason": line.strip()[:300]})
    return found[:10]


def _match_mod_file(mod: dict[str, str], instance_path: str | Path | None) -> bool:
    if not instance_path:
        return True
    mods_dir = Path(instance_path) / "mods"
    if not mods_dir.is_dir():
        return True
    needle = mod["name"].lower().replace(" ", "")
    try:
        for entry in mods_dir.iterdir():
            if not entry.is_file():
                continue
            stem = entry.stem.lower().removesuffix(".disabled")
            if needle and (needle in stem or stem in needle):
                mod["file"] = entry.name
                return True
    except OSError:
        return True
    return False


def read_hs_err_report(instance_path: str | Path, limit: int = 40) -> list[str]:
    """Хвост последнего hs_err_pid*.log в каталоге экземпляра."""
    root = Path(instance_path)
    try:
        reports = sorted(root.glob("hs_err_pid*.log"), key=lambda p: p.stat().st_mtime, reverse=True)
    except OSError:
        return []
    if not reports:
        return []
    try:
        content = reports[0].read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    return content[:limit]
