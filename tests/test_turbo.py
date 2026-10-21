from __future__ import annotations

import hashlib
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher import turbo


class TurboBatchTests(unittest.TestCase):
    def test_batch_matches_hashlib_when_binary_available(self) -> None:
        # Собранный бинарник в песочнице лежит рядом с ресурсами бэкенда.
        candidates = [
            Path(__file__).resolve().parents[1] / "frontend/src-tauri/resources/backend/nivora-turbo",
        ]
        binary = next((path for path in candidates if path.is_file()), None)
        if binary is None:
            self.skipTest("nivora-turbo не собран (scripts/build_turbo.sh)")
        with tempfile.TemporaryDirectory() as directory:
            files = []
            for index, size in enumerate([0, 1, 63, 64, 65, 4096, 1 << 20]):
                path = Path(directory) / f"chunk-{index}.bin"
                path.write_bytes(bytes((index * 31 + 7) % 256 for _ in range(size)))
                files.append(path)
            with patch.dict(os.environ, {"NIVORA_TURBO_BIN": str(binary)}, clear=False):
                turbo._cached_path = None
                turbo._cached_resolved = False
                try:
                    batch = turbo.sha256_batch(files)
                finally:
                    turbo._cached_path = None
                    turbo._cached_resolved = False
            self.assertIsNotNone(batch)
            assert batch is not None
            for path in files:
                digest, size = batch[path]
                self.assertEqual(digest, hashlib.sha256(path.read_bytes()).hexdigest())
                self.assertEqual(size, path.stat().st_size)

    def test_missing_binary_falls_back_to_none(self) -> None:
        with patch.dict(os.environ, {"NIVORA_TURBO_BIN": "/nonexistent/nivora-turbo"}, clear=False):
            turbo._cached_path = None
            turbo._cached_resolved = False
            try:
                with patch.object(turbo.shutil, "which", return_value=None):
                    self.assertIsNone(turbo.turbo_binary())
            finally:
                turbo._cached_path = None
                turbo._cached_resolved = False

    def test_tiny_batches_skip_subprocess(self) -> None:
        with patch.object(turbo, "turbo_binary", return_value="/bin/true"):
            self.assertIsNone(turbo.sha256_batch([Path("/etc/hostname")] * 3))

    def test_failed_run_reports_none(self) -> None:
        # Бинарник есть, но рухнул (код 1) — пачка целиком откатывается.
        with patch.object(turbo, "turbo_binary", return_value="/bin/false"):
            self.assertIsNone(turbo.sha256_batch([Path("/etc/hostname")] * 5))


if __name__ == "__main__":
    unittest.main()
