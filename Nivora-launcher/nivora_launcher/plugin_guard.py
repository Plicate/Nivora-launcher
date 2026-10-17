"""Host-owned rate/data budgets and redacted, rotating operation audit."""
from __future__ import annotations
import json
import os
import re
import stat
import threading
import time
from collections import deque
from pathlib import Path
from .errors import LauncherError

MAX_CALLS = 360
MAX_BYTES_PER_MINUTE = 8 * 1024 * 1024
MAX_PAYLOAD = 4 * 1024 * 1024
MAX_AUDIT_BYTES = 4 * 1024 * 1024

class PluginOperationGuard:
    def __init__(self):
        self.lock = threading.RLock()
        self.windows = {}

    def consume(self, plugin_id: str, payload: dict, *, bucket: str = 'rpc', counts_as_call: bool = True):
        if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,79}', plugin_id):
            raise LauncherError('Invalid plugin identity')
        try:
            data = json.dumps(payload, ensure_ascii=False, allow_nan=False).encode('utf-8')
        except (ValueError, TypeError, RecursionError) as exc:
            raise LauncherError('Plugin request is not bounded JSON') from exc
        if len(data) > MAX_PAYLOAD:
            raise LauncherError('Plugin request exceeds the payload limit')
        now = time.monotonic()
        identity = (plugin_id, bucket)
        with self.lock:
            if identity not in self.windows and len(self.windows) >= 256:
                # Never evict an active budget to let callers reset their limit.
                stale = [key for key, entries in self.windows.items() if not entries or now - entries[-1][0] >= 60]
                for key in stale: self.windows.pop(key, None)
                if len(self.windows) >= 256: raise LauncherError('Plugin budget registry is full')
            values = self.windows.setdefault(identity, deque())
            while values and now - values[0][0] >= 60: values.popleft()
            if (counts_as_call and sum(item[2] for item in values) >= MAX_CALLS) or sum(item[1] for item in values) + len(data) > MAX_BYTES_PER_MINUTE:
                raise LauncherError('Plugin rate/data limit exceeded; retry after the one-minute window')
            values.append((now, len(data), int(counts_as_call)))
        return len(data)

    def usage(self, plugin_id: str, *, bucket: str = 'rpc'):
        now = time.monotonic()
        with self.lock:
            values = self.windows.get((plugin_id, bucket), ())
            active = [item for item in values if now - item[0] < 60]
            return {'calls': sum(item[2] for item in active), 'bytes': sum(item[1] for item in active), 'callLimit': MAX_CALLS,
                    'byteLimit': MAX_BYTES_PER_MINUTE, 'windowSeconds': 60, 'enforced': True}

    def audit(self, root: Path, plugin_id: str, operation: str, args: dict, outcome: str, elapsed_ms: float):
        # Deliberately never store values: URLs, headers, bodies, cookies, tokens,
        # chat content and file content can all contain credentials.
        keys = sorted(str(key)[:80] for key in args)[:64]
        entry = {'at': time.time(), 'plugin': plugin_id[:64], 'operation': operation[:128],
                 'argumentKeys': keys, 'outcome': outcome, 'elapsedMs': round(elapsed_ms, 2)}
        from .plugins import _PLUGIN_V2_EVENTS, _PLUGIN_V3_EVENTS
        event = args.get('event')
        listener = args.get('listener_id')
        if isinstance(event, str) and event in _PLUGIN_V2_EVENTS | _PLUGIN_V3_EVENTS: entry['event'] = event
        if isinstance(listener, str) and re.fullmatch(r'listener-[0-9]{1,16}', listener): entry['listenerId'] = listener
        encoded = (json.dumps(entry, ensure_ascii=False) + '\n').encode('utf-8')
        with self.lock:
            if root.is_symlink(): raise LauncherError('Audit directory cannot be a link')
            root.mkdir(mode=0o700, parents=True, exist_ok=True)
            if os.name != 'nt': root.chmod(0o700)
            path = root / 'plugin-operations.jsonl'
            if path.is_symlink(): raise LauncherError('Audit file cannot be a link')
            if path.exists() and path.stat().st_size > MAX_AUDIT_BYTES:
                backup = root / 'plugin-operations.previous.jsonl'
                if backup.is_symlink(): raise LauncherError('Audit backup cannot be a link')
                os.replace(path, backup)
            fd = os.open(path, os.O_APPEND | os.O_WRONLY | os.O_CREAT | getattr(os, 'O_NOFOLLOW', 0) | getattr(os, 'O_NONBLOCK', 0), 0o600)
            try:
                if not stat.S_ISREG(os.fstat(fd).st_mode): raise LauncherError('Audit destination is not a regular file')
                if hasattr(os, 'fchmod'): os.fchmod(fd, 0o600)
                os.write(fd, encoded)
            finally: os.close(fd)

guard = PluginOperationGuard()
