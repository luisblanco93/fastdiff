"""Focused checks for correlated cold/warm launch measurements."""
import argparse
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import launch_profile as profile


class LaunchProfileTests(unittest.TestCase):
    def expectation(self):
        return {"schema": 1, "sample_id": "new", "repository": "/repo", "file": "a.rs", "marker": "unique"}

    def report(self):
        return {"schema": 1, "pid": 42, "started_unix_ms": 500,
                "request": {**self.expectation(), "received_unix_ms": 1001,
                            "rendered_unix_ms": 1100, "content_ready_ms": 50, "render_ms": 49}}

    def test_warm_completion_requires_same_window_and_current_content(self):
        result = profile.parse_request(self.report(), self.expectation(), 1000, expected_pid=42)
        self.assertEqual(result["request_to_render_ms"], 100)
        for field, value in (("sample_id", "old"), ("repository", "/other"),
                             ("file", "other.rs"), ("marker", "previous")):
            report = self.report()
            report["request"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                profile.parse_request(report, self.expectation(), 1000, expected_pid=42)
        with self.assertRaises(ValueError):
            profile.parse_request(self.report(), self.expectation(), 1000, expected_pid=99)

    def test_partial_stale_and_nonfinite_telemetry_rejected(self):
        for field, value in (("rendered_unix_ms", None), ("rendered_unix_ms", 999),
                             ("content_ready_ms", float("nan")), ("render_ms", -1)):
            report = self.report()
            report["request"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                profile.parse_request(report, self.expectation(), 1000)

    def test_phase_parser_correlates_and_keeps_raw_rows(self):
        row = {"schema": 1, "sample_id": "new", "pid": 42, "phase": "python_imports",
               "duration_ms": 4, "started_unix_ms": 1002}
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.jsonl"
            path.write_text(json.dumps(row) + "\n")
            phases, raw = profile.parse_phases(path, "new", 1000)
            self.assertEqual(phases, {"python_imports": 4, "python_startup": 2})
            self.assertEqual(raw, [row])
            for invalid in ({**row, "sample_id": "old"}, {**row, "duration_ms": -1}):
                path.write_text(json.dumps(invalid) + "\n")
                with self.assertRaises(ValueError):
                    profile.parse_phases(path, "new", 1000)

    def test_literal_arguments_and_disabled_trace(self):
        args = argparse.Namespace(launcher="/bin/dff", repository="/repo $(bad)", file="a b.rs",
                                  windows_from_wsl=False, wsl_distro=None, trace_phases=False)
        command, env = profile.launch_command(args, Path("report.json"), Path("runtime"),
                                              "sample", 10000)
        self.assertIn("/repo $(bad)", command)
        self.assertIn("a b.rs", command)
        self.assertNotIn("sh", command)
        self.assertNotIn("FASTDIFF_LAUNCH_TRACE", env)
        self.assertNotIn("FASTDIFF_LAUNCH_SAMPLE_ID", env)

    def test_fixture_is_restored_and_external_edits_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "a.rs"
            original = b'FASTDIFF_LAUNCH_MARKER\n'
            path.write_bytes(original)
            with profile.MarkerFixture(Path(directory), "a.rs") as fixture:
                fixture.write("first")
                self.assertEqual(path.read_bytes(), b"first\n")
                fixture.write("second")
                self.assertEqual(path.read_bytes(), b"second\n")
            self.assertEqual(path.read_bytes(), original)
            with self.assertRaises(RuntimeError):
                with profile.MarkerFixture(Path(directory), "a.rs") as fixture:
                    fixture.write("third")
                    path.write_bytes(b"external edit\n")
            self.assertEqual(path.read_bytes(), b"external edit\n")

    def test_fixture_requires_explicit_first_line_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "a.rs"
            path.write_bytes(b"user content\n")
            with self.assertRaises(ValueError):
                profile.MarkerFixture(Path(directory), "a.rs")
            with self.assertRaises(ValueError):
                profile.MarkerFixture(Path(directory), "../outside")

    def test_wait_ignores_previous_sample_until_current_marker_is_rendered(self):
        current = self.report()
        old = {**current, "request": {**current["request"], "sample_id": "old"}}
        partial = {**current, "request": {**current["request"], "rendered_unix_ms": None}}
        with patch("launch_profile.read_report", side_effect=[old, partial, current]), \
                patch("launch_profile.time.sleep"), \
                patch("launch_profile.process_info", return_value=(True, 1024)):
            result = profile.await_request(Path("report"), self.expectation(), 1000, 10,
                                           profile.time.monotonic() + 5, False, 42)
        self.assertEqual(result["request_to_render_ms"], 100)

    def test_repeated_warm_requests_use_one_pid_and_different_markers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = root / "a.rs"
            fixture.write_bytes(b"FASTDIFF_LAUNCH_MARKER\n")
            args = argparse.Namespace(repository=directory, file="a.rs", case="warm",
                                      runs=3, timeout=10, windows_from_wsl=False,
                                      wsl_distro=None, report_directory=None,
                                      launcher="dff", hardware="test", software="test",
                                      command_kind="managed", trace_phases=False,
                                      output=root / "results.json")
            observed = []
            def request(args, report, runtime, expected, exit_ms, expected_pid=None):
                observed.append((expected, runtime, expected_pid))
                self.assertIn(expected["marker"].encode(), fixture.read_bytes())
                return {"pid": 42, "first_frame_ms": None, "request_to_render_ms": 20,
                        "launcher_ms": 5, "content_ready_ms": 10, "render_ms": 5,
                        "idle_rss_bytes": 1024, "phases_ms": {}}
            with patch("launch_profile.run_request", side_effect=request), \
                    patch("launch_profile.wait_for_exit") as close:
                self.assertEqual(profile.run(args), 0)
            self.assertEqual(len({item[0]["marker"] for item in observed}), 4)
            self.assertEqual(len({item[1] for item in observed}), 1)
            self.assertEqual([item[2] for item in observed], [None, 42, 42, 42])
            close.assert_called_once_with(42, 10, False)
            self.assertEqual(fixture.read_bytes(), b"FASTDIFF_LAUNCH_MARKER\n")
            result = json.loads(args.output.read_text())
            self.assertEqual(result["completed_runs"], 3)
            self.assertEqual(result["summary"]["first_frame_p95_ms"], None)

    def test_failed_warm_seed_still_requests_graceful_close(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.rs").write_bytes(b"FASTDIFF_LAUNCH_MARKER\n")
            args = argparse.Namespace(repository=directory, file="a.rs", case="warm",
                                      runs=3, timeout=10, windows_from_wsl=False,
                                      wsl_distro=None, report_directory=None,
                                      launcher="dff", hardware="test", software="test",
                                      command_kind="managed", trace_phases=False,
                                      output=root / "results.json")
            controls = []
            original_control = profile.atomic_control
            def control(report, value):
                controls.append(value)
                original_control(report, value)
            def failed_seed(args, report, runtime, expected, exit_ms):
                report.write_text(json.dumps({"schema": 1, "pid": 42,
                                              "started_unix_ms": profile.time.time_ns() / 1_000_000}))
                raise ValueError("missing phases")
            with patch("launch_profile.run_request", side_effect=failed_seed), \
                    patch("launch_profile.atomic_control", side_effect=control), \
                    patch("launch_profile.wait_for_exit") as close:
                self.assertEqual(profile.run(args), 1)
            self.assertIn({"schema": 1, "close": True}, controls)
            close.assert_called_once_with(42, 10, False)
            def malformed_seed(args, report, runtime, expected, exit_ms):
                report.write_text("[]")
                raise ValueError("malformed telemetry")
            with patch("launch_profile.run_request", side_effect=malformed_seed), \
                    patch("launch_profile.time.sleep") as grace:
                self.assertEqual(profile.run(args), 1)
            grace.assert_called_once_with(profile.CLOSE_GRACE_SECONDS)

    def test_windows_batch_paths_with_parentheses_are_rejected(self):
        args = argparse.Namespace(launcher="C:/tools/dff.cmd", repository="C:/repo(test)",
                                  file="a.rs", windows_from_wsl=False, wsl_distro=None,
                                  trace_phases=False)
        report, runtime = Path("report.json"), Path("runtime")
        launcher = Path(args.launcher)
        with patch("launch_profile.os.name", "nt"), \
                patch("launch_profile.Path", return_value=launcher), self.assertRaises(ValueError):
            profile.launch_command(args, report, runtime, "sample", 10000)


class SchemaTwoRequestTests(unittest.TestCase):
    BUILD = {"version": "0.1.7", "revision": "abc", "profile": "release", "protocol": 5}
    PLATFORM = {"os": "linux", "arch": "x86_64"}

    def expectation(self, **extra):
        return {"schema": 1, "sample_id": "new", "repository": "/repo", "file": "a.rs",
                "marker": "unique", **extra}

    def report(self, request=None, **top):
        request = {"sample_id": "new", "repository": "/repo", "file": "a.rs", "marker": "unique",
                   "received_unix_ms": 1001, "rendered_unix_ms": 1100, "content_ready_ms": 50,
                   "render_ms": 49, "view": "all", "trigger": "open", "frames": 4,
                   "frame_cpu_total_ms": 6.5, "max_frame_cpu_ms": 3.0, **(request or {})}
        return {"schema": 2, "pid": 42, "started_unix_ms": 500, "ui_ready_ms": 40,
                "requested_content_ms": 90, "build": self.BUILD, "platform": self.PLATFORM,
                "request": request, **top}

    def test_schema_2_request_reports_frames_build_and_confirmed_view(self):
        result = profile.parse_request(self.report(), self.expectation(view="all"), 1000, 42)
        self.assertEqual(result["request_to_render_ms"], 100)
        self.assertEqual(result["telemetry_schema"], 2)
        self.assertEqual(result["frames"], 4)
        self.assertEqual(result["frame_cpu_total_ms"], 6.5)
        self.assertEqual(result["max_frame_cpu_ms"], 3.0)
        self.assertEqual(result["build"], self.BUILD)
        self.assertTrue(result["view_verified"])

    def test_schema_2_false_success_is_rejected(self):
        cases = {
            "missing requested content": {"requested_content_ms": None},
            "content precedes ui ready": {"ui_ready_ms": 80, "requested_content_ms": 50},
            "negative content": {"requested_content_ms": -5},
            "missing ui ready": {"ui_ready_ms": None},
        }
        for name, top in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                profile.parse_request(self.report(**top), self.expectation(), 1000, 42)
        for name, request in {"empty request": {"sample_id": None},
                              "other sample": {"sample_id": "old"},
                              "other file": {"file": "b.rs"}}.items():
            with self.subTest(name), self.assertRaises(ValueError):
                profile.parse_request(self.report(request), self.expectation(), 1000, 42)
        with self.assertRaises(ValueError):
            profile.parse_request(self.report(), self.expectation(), 1000, 99)
        with self.assertRaises(ValueError):
            profile.parse_request({**self.report(), "request": None}, self.expectation(), 1000)

    def test_trigger_and_view_must_match_what_was_requested(self):
        with self.assertRaises(ValueError):
            profile.parse_request(self.report(), self.expectation(trigger="edit"), 1000, 42)
        edit = self.report({"trigger": "edit"})
        self.assertEqual(profile.parse_request(
            edit, self.expectation(trigger="edit"), 1000, 42)["request_to_render_ms"], 100)
        with self.assertRaises(ValueError):
            profile.parse_request(self.report({"view": "staged"}),
                                  self.expectation(view="unstaged"), 1000, 42)
        missing = self.report()
        del missing["request"]["view"]
        with self.assertRaises(ValueError):
            profile.parse_request(missing, self.expectation(view="all"), 1000, 42)

    def test_schema_2_frame_metrics_are_required_once_rendered(self):
        for field in ("frames", "frame_cpu_total_ms", "max_frame_cpu_ms"):
            report = self.report()
            del report["request"][field]
            with self.subTest(field=field), self.assertRaises(ValueError):
                profile.parse_request(report, self.expectation(), 1000, 42)

    def test_schema_1_view_cannot_be_confirmed_and_says_so(self):
        legacy = {"schema": 1, "pid": 42, "started_unix_ms": 500,
                  "request": {**self.expectation(), "received_unix_ms": 1001,
                              "rendered_unix_ms": 1100, "content_ready_ms": 50, "render_ms": 49}}
        result = profile.parse_request(legacy, self.expectation(view="staged"), 1000, 42)
        self.assertFalse(result["view_verified"])
        self.assertIsNone(result["build"])
        self.assertIsNone(result["frames"])

    def test_edit_latency_runs_from_the_trigger_not_the_control_write(self):
        report = self.report({"trigger": "edit", "received_unix_ms": 1001,
                              "rendered_unix_ms": 1180})
        result = profile.parse_request(report, self.expectation(trigger="edit"), 1000, 42,
                                       origin_ms=1100)
        self.assertEqual(result["request_to_render_ms"], 80)
        stale = self.report({"trigger": "edit", "received_unix_ms": 900, "rendered_unix_ms": 1180})
        with self.assertRaises(ValueError):
            profile.parse_request(stale, self.expectation(trigger="edit"), 1000, 42, origin_ms=1100)
        early = self.report({"trigger": "edit", "rendered_unix_ms": 1050})
        with self.assertRaises(ValueError):
            profile.parse_request(early, self.expectation(trigger="edit"), 1000, 42, origin_ms=1100)

    def test_wait_never_accepts_an_unrelated_or_empty_request(self):
        for request in ({}, None, {"sample_id": "old", "rendered_unix_ms": 1100}):
            report = {**self.report(), "request": request}
            with self.subTest(request=request), \
                    patch("launch_profile.read_report", return_value=report), \
                    patch("launch_profile.time.sleep"), \
                    patch("launch_profile.process_info", return_value=(True, 1024)):
                with self.assertRaises(RuntimeError):
                    profile.await_request(Path("report"), self.expectation(), 1000, 10,
                                          profile.time.monotonic() + 0.05, False, 42)

    def test_probe_brackets_the_idle_window_for_each_request(self):
        class Probe:
            events = []
            def begin(self, pid):
                self.events.append(("begin", pid))
                return "before"
            def finish(self, before, pid, rss):
                self.events.append(("finish", before, pid, rss))
                return {"total": {"rss_bytes": rss}}
        probe = Probe()
        with patch("launch_profile.read_report", return_value=self.report()), \
                patch("launch_profile.time.sleep"), \
                patch("launch_profile.process_info", return_value=(True, 2048)):
            result = profile.await_request(Path("report"), self.expectation(), 1000, 10,
                                           profile.time.monotonic() + 5, False, 42, probe=probe)
        self.assertEqual(probe.events, [("begin", 42), ("finish", "before", 42, 2048)])
        self.assertEqual(result["resources"], {"total": {"rss_bytes": 2048}})
        self.assertEqual(result["idle_rss_bytes"], 2048)

    def test_view_argument_is_passed_before_the_repository(self):
        args = argparse.Namespace(launcher="/bin/dff", repository="/repo", file="a.rs",
                                  windows_from_wsl=False, wsl_distro=None, trace_phases=False,
                                  view="staged")
        command, _ = profile.launch_command(args, Path("report.json"), Path("runtime"), "s", 1000)
        self.assertEqual(command[command.index("--view") + 1], "staged")
        self.assertLess(command.index("--view"), command.index("--"))
        args.view = None
        self.assertNotIn("--view", profile.launch_command(
            args, Path("report.json"), Path("runtime"), "s", 1000)[0])

    def test_request_sample_uses_schema_2_names_and_keeps_schema_1_aliases(self):
        from benchmark_telemetry import request_sample
        sample = {"pid": 42, "request_to_render_ms": 100.0, "launcher_ms": 7.0,
                  "content_ready_ms": 50.0, "render_ms": 49.0, "idle_rss_bytes": 2048}
        row = request_sample(sample, 140.0, self.expectation(view="staged"), ["dff"], 1000.0, {}, [])
        self.assertEqual((row["first_frame_ms"], row["ui_ready_ms"]), (140.0, 140.0))
        self.assertEqual((row["first_diff_ms"], row["requested_content_ms"]), (100.0, 100.0))
        self.assertEqual(row["ack_ms"], 7.0)
        self.assertTrue(row["readiness_identity_bound"])
        self.assertEqual(row["view"], "staged")
        self.assertEqual(row["trigger"], "open")

    def test_summary_handles_samples_without_a_launcher_or_resources(self):
        rows = [{"request_to_render_ms": 100.0 + n, "launcher_ms": None, "content_ready_ms": 5.0,
                 "render_ms": 3.0, "idle_rss_bytes": 1024, "phases_ms": {},
                 "first_frame_ms": None} for n in range(3)]
        summary = profile.summarize_requests(rows, cold=False)
        self.assertIsNone(summary["launcher_ms"])
        self.assertIsNone(summary["ack_p95_ms"])
        self.assertEqual(summary["requested_content_p95_ms"], 102.0)
        self.assertEqual(summary["first_diff_p95_ms"], 102.0)
        self.assertEqual(summary["resources"]["measured_runs"], 0)


if __name__ == "__main__":
    unittest.main()
