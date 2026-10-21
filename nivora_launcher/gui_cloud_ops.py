"""операции cloud_* (WebDAV-синхронизация) для gui-воркера."""
from __future__ import annotations

from typing import Any, Callable

from .cloud_storage import (
    WebDavStorage,
    delete_cloud_file,
    restore_cloud,
    save_cloud_password,
    sync_cloud,
)
from .config import ConfigStore

CLOUD_OPERATIONS = frozenset(
    {
        "cloud_status",
        "cloud_configure",
        "cloud_test",
        "cloud_list",
        "cloud_sync",
        "cloud_restore",
        "cloud_delete",
        "cloud_disconnect",
    }
)

def handle_cloud_operation(
    operation: str,
    request: dict[str, Any],
    config: ConfigStore,
    scoped_emit: Callable[[dict[str, Any]], None],
    settings_public: Callable[[ConfigStore], dict[str, Any]],
    status_public: Callable[[ConfigStore], dict[str, Any]],
) -> dict[str, Any] | None:
    """Обрабатывает cloud-операцию; None — операция не из этой группы."""
    if operation not in CLOUD_OPERATIONS:
        return None

    if operation == "cloud_status":
        return {"status": status_public(config)}

    if operation == "cloud_configure":
        next_url = str(request.get("url", "")).strip().rstrip("/")
        next_username = str(request.get("username", "")).strip()
        WebDavStorage._validated_url(next_url)
        credentials_changed = next_url != config.settings.cloud_url or next_username != config.settings.cloud_username
        values = {
            "cloud_url": next_url,
            "cloud_username": next_username,
            "cloud_remote_path": str(request.get("remote_path", "trial-launcher")),
            "cloud_include_accounts": bool(request.get("include_accounts", False)),
        }
        # remove the old key before changing its libsecret attributes. Otherwise
        # changing endpoint or username would leave an orphaned credential.
        if credentials_changed:
            save_cloud_password(config, "")
        config.set_values(values)
        password = str(request.get("password", ""))
        if password:
            save_cloud_password(config, password)
        elif credentials_changed:
            save_cloud_password(config, "")
        return {"status": status_public(config), "settings": settings_public(config)}

    if operation == "cloud_test":
        storage = WebDavStorage(config)
        files = storage.list_files()
        return {
            "status": status_public(config),
            "files": [value.as_dict() for value in files],
        }

    if operation == "cloud_list":
        storage = WebDavStorage(config)
        return {
            "status": status_public(config),
            "files": [value.as_dict() for value in storage.list_files()],
        }

    if operation == "cloud_sync":
        result = sync_cloud(config)
        storage = WebDavStorage(config)
        files = storage.list_files()
        scoped_emit({"kind": "status", "event": "cloud-synced", "message": "Файлы сохранены в облако."})
        return {
            "result": result,
            "status": status_public(config),
            "files": [value.as_dict() for value in files],
        }

    if operation == "cloud_restore":
        result = restore_cloud(
            config,
            confirmed=bool(request.get("confirmed", False)),
            manifest_id=str(request.get("manifest_id", "")),
        )
        if result.get("preview"):
            return {"result": result, "status": status_public(config)}
        scoped_emit({"kind": "status", "event": "cloud-restored", "message": "Файлы восстановлены из облака."})
        return {"result": result, "status": status_public(config)}

    if operation == "cloud_delete":
        delete_cloud_file(config, str(request.get("key", "")))
        storage = WebDavStorage(config)
        return {
            "status": status_public(config),
            "files": [value.as_dict() for value in storage.list_files()],
        }

    # cloud_disconnect
    save_cloud_password(config, "")
    config.set_values({"cloud_url": "", "cloud_username": "", "cloud_remote_path": "trial-launcher", "cloud_include_accounts": False, "cloud_last_sync": 0})
    return {"status": status_public(config), "settings": settings_public(config)}
