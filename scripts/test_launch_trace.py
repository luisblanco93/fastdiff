"""Focused launch tracing checks; no desktop, downloads, or benchmark run."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from test_plugin_runtime import make_package, runtime, VERSION


class LaunchTraceTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="fastdiff launch trace ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.trace = self.root / "launch.jsonl"
        package = make_package(self.root, "linux")
        self.receipt = {
            "version": VERSION, "platform": "linux",
            "launcher": str(package / "bin/fastdiff"),
            "gui": str(package / "bin/fastdiff-gui"),
        }
        (self.root / "receipt.json").write_text(json.dumps(self.receipt))

    def environment(self, trace: Path | None = None) -> dict[str, str]:
        return {
            "FASTDIFF_LAUNCH_TRACE": str(self.trace if trace is None else trace),
            "FASTDIFF_LAUNCH_SAMPLE_ID": 'sample "with quotes"',
        }

    def test_entry_imports_record_entry_timestamp_and_valid_json(self) -> None:
        wrapper = self.root / "dff"
        with patch.object(runtime, "command_path", return_value=wrapper):
            fields, pending = runtime.stage_command(self.root, VERSION, "linux")
        pending.unlink()
        started = time.time() * 1000
        result = subprocess.run(
            [sys.executable, fields["command"], "--help"],
            env={**os.environ, **self.environment()}, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        row = json.loads(self.trace.read_text())
        self.assertEqual(row["phase"], "python_imports")
        self.assertEqual(row["schema"], 1)
        self.assertEqual(row["sample_id"], 'sample "with quotes"')
        self.assertGreater(row["pid"], 0)
        self.assertGreaterEqual(row["duration_ms"], 0)
        self.assertGreaterEqual(row["started_unix_ms"], started)
        self.assertLessEqual(row["started_unix_ms"], time.time() * 1000)

    def test_receipt_validation_is_correlated_and_child_inherits_trace(self) -> None:
        with patch.dict(os.environ, self.environment()), \
                patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 37)) as run:
            self.assertEqual(runtime.launch(VERSION, "linux", self.root, ["--", "repo"]), 37)
        row = json.loads(self.trace.read_text())
        self.assertEqual(row["phase"], "receipt_validation")
        self.assertEqual(row["sample_id"], 'sample "with quotes"')
        self.assertEqual(row["pid"], os.getpid())
        self.assertGreaterEqual(row["duration_ms"], 0)
        self.assertEqual(run.call_args.kwargs["env"]["FASTDIFF_LAUNCH_TRACE"], str(self.trace))

    def test_receipt_version_read_has_its_own_phase(self) -> None:
        with patch.dict(os.environ, self.environment()):
            self.assertEqual(runtime.installed_version(self.root), VERSION)
        row = json.loads(self.trace.read_text())
        self.assertEqual(row["phase"], "receipt_version")

    def test_disabled_launch_does_not_read_the_clock_or_emit(self) -> None:
        with patch.dict(os.environ, {"FASTDIFF_LAUNCH_TRACE": ""}), \
                patch("time.perf_counter", side_effect=AssertionError("disabled timer")), \
                patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)):
            self.assertEqual(runtime.launch(VERSION, "linux", self.root, []), 0)
        self.assertFalse(self.trace.exists())

    def test_trace_write_failure_preserves_launch_and_receipt_errors(self) -> None:
        with patch.dict(os.environ, self.environment(self.root)), \
                patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)):
            self.assertEqual(runtime.launch(VERSION, "linux", self.root, []), 0)
            (self.root / "receipt.json").write_text("{}")
            with self.assertRaisesRegex(ValueError, "installation is unavailable"):
                runtime.launch(VERSION, "linux", self.root, [])


if __name__ == "__main__":
    unittest.main()
