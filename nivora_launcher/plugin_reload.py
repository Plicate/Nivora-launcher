"""Developer reload requests. Never bypass package integrity or execution trust."""
from __future__ import annotations
import hashlib
import json
import re
import time
from itertools import islice
from pathlib import Path
from .config import ConfigStore, _atomic_json_write
from .errors import LauncherError
from .plugins import PluginStore


def _requests(config):
    path = config.paths.data_home / 'plugin-dev-reload.json'
    if path.is_symlink(): raise LauncherError('Unsafe developer reload file')
    if not path.exists(): return {}
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 32768:
        raise LauncherError('Unsafe developer reload file')
    data = json.loads(path.read_text(encoding='utf-8'))
    if not isinstance(data, dict) or len(data) > 128: raise LauncherError('Invalid reload registry')
    if any(not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,79}', key) or type(value) is not int or not 0 < value < 2**63 for key, value in data.items()):
        raise LauncherError('Invalid reload identity or timestamp')
    return data


def request_reload(plugin_id: str, config: ConfigStore | None = None):
    config = config or ConfigStore()
    if not config.settings.developer_mode: raise LauncherError('Plugin reload requires developer mode')
    store = PluginStore(config)
    # An explicit reload is not an implicit grant to run edited or unsigned code.
    store.runtime_source(plugin_id)
    requests = _requests(config)
    if len(requests) >= 128 and plugin_id not in requests: raise LauncherError('Reload registry is full')
    requests[plugin_id] = time.time_ns()
    _atomic_json_write(config.paths.data_home / 'plugin-dev-reload.json', requests, private=True)
    return requests[plugin_id]


def changes(config: ConfigStore):
    if not config.settings.developer_mode: return {'enabled': False, 'generation': '', 'requests': {}}
    store = PluginStore(config)
    entries = []
    # Entry scripts are single-file bundles; no arbitrary recursive directory walk.
    for root in islice(store.root.iterdir(), 128):
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,79}', root.name) or not root.is_dir(): continue
        item = {'id': root.name}
        if root.is_symlink(): raise LauncherError('Cannot watch a linked plugin')
        manifest_path = root / 'plugin.json'
        if manifest_path.is_symlink() or not manifest_path.is_file() or manifest_path.stat().st_size > 256 * 1024:
            raise LauncherError('Cannot watch an unsafe manifest')
        try: manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
        except (ValueError, UnicodeError) as exc: raise LauncherError('Cannot watch an invalid manifest') from exc
        entrypoint = manifest.get('entrypoint', 'main.js') if isinstance(manifest, dict) else 'main.js'
        if not isinstance(entrypoint, str) or not entrypoint or len(entrypoint) > 256 or '\\' in entrypoint or ':' in entrypoint or Path(entrypoint).is_absolute() or '..' in Path(entrypoint).parts:
            raise LauncherError('Unsafe plugin entrypoint')
        candidate = root
        for part in Path(entrypoint).parts:
            candidate = candidate / part
            if candidate.is_symlink(): raise LauncherError('Cannot watch a linked entrypoint')
        for name in ('plugin.json', entrypoint):
            path = root / name
            if path.is_symlink(): raise LauncherError('Cannot watch a linked entrypoint')
            try: metadata = path.stat()
            except FileNotFoundError: continue
            entries.append((item['id'], name, metadata.st_mtime_ns, metadata.st_size))
    requests = _requests(config)
    fingerprint = hashlib.sha256(json.dumps([entries, requests], sort_keys=True).encode()).hexdigest()
    return {'enabled': True, 'generation': fingerprint, 'requests': requests}
