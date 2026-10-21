from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher.metrics import SessionEvents, _parse_pmon_row, _process_sample


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


class SessionEventsTests(unittest.TestCase):
    def test_events_and_mod_stats_extracted_from_game_log(self) -> None:
        events = SessionEvents()
        events.feed([
            "[13:20:01] [main/INFO]: Backend library: LWJGL version 3.3.3",
            "[13:20:04] [main/INFO] (FabricLoader) Loading 128 mods",
            "[13:20:20] [main/INFO] [minecraft/]: Sound engine started",
            "[13:20:21] [main/WARN] [create/] Pipeline disabled by config",
            "[13:20:22] [main/INFO] [create/] Contraption registered",
            "[13:20:23] [main/ERROR] [sodium/] Shader compile failed",
        ])
        snapshot = events.snapshot()
        titles = [item["title"] for item in snapshot["events"]]
        self.assertIn("LWJGL инициализирован", titles)
        self.assertIn("Загрузка модов", titles)
        self.assertIn("Звуковой движок запущен", titles)
        mods = {item["mod"]: item for item in snapshot["mods"]}
        self.assertEqual(mods["create"]["warnings"], 1)
        self.assertEqual(mods["create"]["lines"], 2)
        self.assertEqual(mods["sodium"]["errors"], 1)

    def test_feed_is_incremental_and_survives_tail_rotation(self) -> None:
        events = SessionEvents()
        events.feed(["[13:20:01] [main/INFO]: Backend library: LWJGL version 3.3.3"])
        events.feed([
            "[13:20:01] [main/INFO]: Backend library: LWJGL version 3.3.3",
            "[13:20:20] [main/INFO] [minecraft/]: Sound engine started",
        ])
        started = [event for event in events.snapshot()["events"] if event["title"] == "Звуковой движок запущен"]
        self.assertEqual(len(started), 1)

    def test_new_launch_keeps_marker_deduplication(self) -> None:
        events = SessionEvents()
        events.feed(["[13:20:01] [main/INFO]: Backend library: LWJGL version 3.3.3"])
        events.feed(["[14:00:01] [main/INFO]: Backend library: LWJGL version 3.3.3"])
        lwjgl = [event for event in events.snapshot()["events"] if event["title"] == "LWJGL инициализирован"]
        self.assertEqual(len(lwjgl), 1)

    def test_repeated_line_does_not_double_count(self) -> None:
        events = SessionEvents()
        line = "[13:20:21] [main/WARN] [create/] Pipeline disabled"
        events.feed([line])
        events.feed([line])
        events.feed([line])
        mods = {item["mod"]: item for item in events.snapshot()["mods"]}
        self.assertEqual(mods["create"]["warnings"], 1)


class GpuSampleTests(unittest.TestCase):
    def test_nvidia_pmon_row_parsed(self) -> None:
        sample = _parse_pmon_row("00000000  1234  G     45   512    0    0  javaw", 1234)
        self.assertEqual(sample, {"util_percent": 45.0, "memory_bytes": 512 * 1024 * 1024, "source": "nvidia"})

    def test_pmon_row_other_pid_ignored(self) -> None:
        self.assertIsNone(_parse_pmon_row("00000000  1234  G     45   512    0    0  javaw", 4321))

    def test_pmon_header_ignored(self) -> None:
        self.assertIsNone(_parse_pmon_row("# gpu        pid  type    sm   mem   enc   dec    command", 1234))


if __name__ == "__main__":
    unittest.main()
