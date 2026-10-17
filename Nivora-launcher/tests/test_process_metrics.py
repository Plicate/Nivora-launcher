from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.metrics import _process_sample


class ProcessMetricsTests(unittest.TestCase):
    def test_sample_includes_sandbox_children(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            proc_root = Path(directory)
            _write_process(proc_root, 101, 1, 10, 5, 11, [102])
            _write_process(proc_root, 102, 101, 20, 10, 22, [103])
            _write_process(proc_root, 103, 102, 30, 5, 33, [])

            with patch("nivora_launcher.metrics.os.sysconf", return_value=100):
                sample = _process_sample(101, proc_root=proc_root)

        self.assertIsNotNone(sample)
        self.assertAlmostEqual(sample[0], 0.8)
        self.assertEqual(sample[1], 66 * 1024)

    def test_missing_root_process_is_not_reported_as_running(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            with patch("nivora_launcher.metrics.os.sysconf", return_value=100):
                self.assertIsNone(_process_sample(101, proc_root=Path(directory)))


def _write_process(
    proc_root: Path,
    pid: int,
    parent_pid: int,
    user_ticks: int,
    system_ticks: int,
    resident_kib: int,
    children: list[int],
) -> None:
    process = proc_root / str(pid)
    task = process / "task" / str(pid)
    task.mkdir(parents=True)
    fields = ["S", str(parent_pid), *("0" for _ in range(9)), str(user_ticks), str(system_ticks)]
    (process / "stat").write_text(f"{pid} (java (wrapper)) {' '.join(fields)}\n", encoding="utf-8")
    (process / "status").write_text(f"Name:\tjava\nVmRSS:\t{resident_kib} kB\n", encoding="utf-8")
    (task / "children").write_text(" ".join(str(child) for child in children), encoding="ascii")


if __name__ == "__main__":
    unittest.main()
