"""Behavioral tests for benchmark evidence validation and process arguments."""

import math
import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest
from unittest.mock import MagicMock, patch

import benchmark
from benchmark import (WindowsSampler, await_gui, launch_sample,
                       parse_report, percentile, summarize, wsl_command)
import benchmark_telemetry as telemetry
from benchmark_telemetry import Sample, content_ready, gui_identity, read_report


BUILD = {"version": "0.1.7", "revision": None, "profile": "release", "protocol": 5}
PLATFORM = {"os": "linux", "arch": "x86_64"}


def report_v2(**overrides):
    report = {"schema": 2, "pid": 123, "started_unix_ms": 1100,
              "first_frame_ms": 40, "first_diff_ms": 90,
              "ui_ready_ms": 40, "requested_content_ms": 90,
              "build": BUILD, "platform": PLATFORM,
              "measured_frames": 3, "max_frame_cpu_ms": 2.0, "frame_cpu_total_ms": 5.0}
    return {**report, **overrides}


def sample(frame=100.0, diff=200.0, ack=10.0, rss=1024, bound=True, resources=None):
    return Sample(frame, diff, ack, rss, ack_ms=ack, ui_ready_ms=frame,
                  requested_content_ms=diff, readiness_identity_bound=bound,
                  telemetry_schema=2 if bound else 1, build=BUILD, platform=PLATFORM,
                  resources=resources)


class BenchmarkEvidenceTests(unittest.TestCase):
    def test_windows_sampling_reuses_one_owned_process(self) -> None:
        import io
        process = MagicMock()
        process.stdout = io.StringIO('ready\n1024\nnull\n')
        process.stdin = io.StringIO()
        with patch('benchmark.subprocess.Popen', return_value=process) as spawn:
            sampler = WindowsSampler()
            self.assertEqual(sampler.query(7), (True, 1024))
            self.assertEqual(sampler.query(9), (False, 0))
            self.assertEqual(process.stdin.getvalue(), '7\n9\n')
            sampler.close()
            spawn.assert_called_once()
            process.wait.assert_called_once()

    def test_broken_sampler_stdin_still_reaps_owned_process(self) -> None:
        import io
        process = MagicMock()
        process.stdout = io.StringIO('ready\n')
        process.stdin.close.side_effect = BrokenPipeError('peer exited')
        with patch('benchmark.subprocess.Popen', return_value=process):
            sampler = WindowsSampler()
            sampler.close()
            process.wait.assert_called_once()

    def test_p95_uses_nearest_rank_over_all_thirty_launches(self) -> None:
        self.assertEqual(percentile(tuple(range(1, 31)), 0.95), 29)

    def test_empty_percentile_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            percentile((), 0.95)

    def test_launch_offset_includes_bridge_time(self) -> None:
        sample = parse_report(
            {"schema": 1, "pid": 123, "started_unix_ms": 1100,
             "first_frame_ms": 40, "first_diff_ms": 90},
            launch_unix_ms=1000, launcher_ms=10, idle_rss_bytes=1024,
        )
        self.assertEqual(sample.first_frame_ms, 140)
        self.assertEqual(sample.first_diff_ms, 190)
        self.assertEqual(sample.launcher_ms, 10)

    def test_incomplete_report_cannot_be_counted_as_a_success(self) -> None:
        with self.assertRaises(ValueError):
            parse_report(
                {"schema": 1, "pid": 123, "started_unix_ms": 1000,
                 "first_frame_ms": 20},
                launch_unix_ms=1000, launcher_ms=10, idle_rss_bytes=1024,
            )

    def test_nonfinite_negative_and_out_of_order_timings_rejected(self) -> None:
        for frame, diff in ((-1, 10), (math.nan, 10), (20, 10)):
            with self.subTest(frame=frame, diff=diff):
                with self.assertRaises(ValueError):
                    parse_report(
                        {"schema": 1, "pid": 123, "started_unix_ms": 1000,
                         "first_frame_ms": frame, "first_diff_ms": diff},
                        launch_unix_ms=1000, launcher_ms=10,
                        idle_rss_bytes=1024,
                    )

    def test_wsl_user_paths_are_arguments_not_shell_source(self) -> None:
        command = wsl_command(
            "Ubuntu", "/home/u/bin/fastdiff", "/home/u/a $(bad)",
            "a file.rs", "/mnt/c/a b/report.json", 3000, "C:\\a b\\gui.exe",
        )
        self.assertIn("/home/u/a $(bad)", command)
        self.assertIn("a file.rs", command)
        self.assertIn("FASTDIFF_BENCH_REPORT=/mnt/c/a b/report.json", command)
        self.assertNotIn("sh", command)
        self.assertNotIn("-c", command)

    def test_rejected_stale_telemetry_never_authorizes_process_cleanup(self) -> None:
        args = argparse.Namespace(launcher="viewer", repository="/repo", file="a.rs",
                                  windows_from_wsl=False, wsl_distro=None, timeout=10)
        process = MagicMock(returncode=0)
        process.communicate.return_value = (None, b"")
        process.poll.return_value = 0
        report = {"schema": 1, "pid": 321, "started_unix_ms": 1}
        with patch("benchmark.subprocess.Popen", return_value=process), \
                patch("benchmark.await_gui", side_effect=ValueError("stale report")), \
                patch("benchmark.read_report", return_value=report), \
                patch("benchmark.os.kill") as stop:
            with self.assertRaises(ValueError):
                launch_sample(args, Path("unused-report"))
            stop.assert_not_called()

    @unittest.skipIf(os.name == "nt", "Unix executable fixture")
    def test_launcher_timing_does_not_wait_for_gui_inherited_stderr(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            launcher = Path(directory) / "launcher"
            launcher.write_text(
                f"#!{sys.executable}\nimport subprocess,sys\n"
                "subprocess.Popen([sys.executable,'-c','import time; time.sleep(1)'], "
                "stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)\n",
                encoding="utf-8",
            )
            launcher.chmod(0o700)
            args = argparse.Namespace(launcher=str(launcher), repository=directory,
                                      file="a.rs", windows_from_wsl=False,
                                      wsl_distro=None, timeout=10)
            with patch("benchmark.await_gui", return_value=None), \
                    patch("benchmark.read_report", return_value=None):
                started = time.monotonic()
                launch_sample(args, Path(directory) / "report.json")
                self.assertLess(time.monotonic() - started, 0.5)


class SchemaTwoTelemetryTests(unittest.TestCase):
    def parse(self, report, **kwargs):
        return parse_report(report, launch_unix_ms=1000, launcher_ms=10,
                            idle_rss_bytes=1024, **kwargs)

    def test_schema_2_uses_identity_bound_readiness_and_records_build(self) -> None:
        result = self.parse(report_v2(first_diff_ms=70, requested_content_ms=95))
        self.assertEqual(result.ui_ready_ms, 140)
        self.assertEqual(result.requested_content_ms, 195)
        self.assertEqual(result.first_frame_ms, result.ui_ready_ms)
        self.assertEqual(result.first_diff_ms, result.requested_content_ms)
        self.assertEqual(result.ack_ms, 10)
        self.assertTrue(result.readiness_identity_bound)
        self.assertEqual(result.build, BUILD)
        self.assertEqual(result.platform, PLATFORM)
        self.assertEqual(result.telemetry_schema, 2)

    def test_frame_totals_are_recorded_when_reported(self) -> None:
        result = self.parse(report_v2())
        self.assertEqual((result.measured_frames, result.frame_cpu_total_ms,
                          result.max_frame_cpu_ms), (3, 5.0, 2.0))
        with self.assertRaises(ValueError):
            self.parse(report_v2(frame_cpu_total_ms=-1))

    def test_schema_1_falls_back_and_is_labeled_not_identity_bound(self) -> None:
        result = self.parse({"schema": 1, "pid": 123, "started_unix_ms": 1100,
                             "first_frame_ms": 40, "first_diff_ms": 90})
        self.assertEqual((result.ui_ready_ms, result.requested_content_ms), (140, 190))
        self.assertFalse(result.readiness_identity_bound)
        self.assertIsNone(result.build)
        self.assertEqual(result.telemetry_schema, 1)

    def test_schema_2_false_success_is_rejected(self) -> None:
        cases = {
            "missing requested content": {"requested_content_ms": None},
            "negative requested content": {"requested_content_ms": -1},
            "non-finite requested content": {"requested_content_ms": math.nan},
            "content precedes ui ready": {"ui_ready_ms": 80, "requested_content_ms": 50},
            "missing ui ready": {"ui_ready_ms": None},
            "bad build": {"build": {"version": "", "profile": "release", "protocol": 5}},
            "bad platform": {"platform": {"os": "linux"}},
        }
        for name, overrides in cases.items():
            with self.subTest(name), self.assertRaises(ValueError):
                self.parse(report_v2(**overrides))
        with self.assertRaises(ValueError):
            self.parse({**report_v2(), "schema": 3})

    def test_wait_condition_never_accepts_first_diff_alone_on_schema_2(self) -> None:
        self.assertTrue(content_ready({"schema": 1, "first_diff_ms": 5}))
        self.assertFalse(content_ready({"schema": 1, "first_diff_ms": None}))
        self.assertTrue(content_ready(report_v2()))
        self.assertFalse(content_ready(report_v2(first_diff_ms=None, requested_content_ms=None)))
        with self.assertRaises(ValueError):
            content_ready(report_v2(requested_content_ms=None))

    def test_build_identity_must_not_change_inside_a_run(self) -> None:
        rows = [{"build": BUILD, "platform": PLATFORM, "telemetry_schema": 2}]
        identity, error = gui_identity(rows + rows)
        self.assertIsNone(error)
        self.assertEqual(identity["gui_build"], BUILD)
        self.assertEqual(identity["telemetry_schemas"], [2])
        changed = [{**rows[0], "build": {**BUILD, "version": "0.1.8"}}]
        self.assertIsNotNone(gui_identity(rows + changed)[1])


class MixedAndPolledIdentityTests(unittest.TestCase):
    def row(self, schema=2, build=BUILD, poll=None):
        return {"telemetry_schema": schema, "build": build if schema == 2 else None,
                "platform": PLATFORM, "control_poll_ms": poll}

    def test_mixed_schemas_in_one_run_fail_the_run(self) -> None:
        identity, error = gui_identity([self.row(2), self.row(2), self.row(1)])
        self.assertIn("mixed GUI telemetry schemas", error)
        self.assertEqual(identity["telemetry_schemas"], [1, 2])
        self.assertIsNone(gui_identity([self.row(1), self.row(1)])[1])
        self.assertIsNone(gui_identity([self.row(2), self.row(2)])[1])

    def test_a_schema_2_sample_without_build_next_to_one_with_build_fails(self) -> None:
        error = gui_identity([self.row(2), self.row(2, build=None)])[1]
        self.assertIn("mixed GUI telemetry schemas or build identity", error)

    def test_null_revision_is_still_tolerated(self) -> None:
        self.assertIsNone(BUILD["revision"])
        build = {**BUILD, "revision": None}
        self.assertIsNone(gui_identity([self.row(2, build=build)] * 3)[1])

    def test_samples_without_telemetry_schema_do_not_trigger_the_mix_check(self) -> None:
        self.assertIsNone(gui_identity([{"request_to_render_ms": 1.0}] * 2)[1])

    def test_control_poll_interval_is_recorded_once_per_run(self) -> None:
        identity, error = gui_identity([self.row(poll=250.0)] * 3)
        self.assertIsNone(error)
        self.assertEqual(identity["gui_control_poll_ms"], 250.0)
        self.assertIsNone(gui_identity([self.row()] * 2)[0]["gui_control_poll_ms"])

    def test_changed_or_partly_missing_control_poll_interval_fails_the_run(self) -> None:
        self.assertIn("control poll", gui_identity([self.row(poll=250.0), self.row(poll=25.0)])[1])
        self.assertIn("control poll", gui_identity([self.row(poll=250.0), self.row()])[1])

    def test_sample_parser_records_the_reported_poll_interval(self) -> None:
        parsed = parse_report(report_v2(control_poll_ms=250), launch_unix_ms=1000,
                              launcher_ms=10, idle_rss_bytes=1024)
        self.assertEqual(parsed.control_poll_ms, 250.0)
        self.assertIsNone(parse_report(report_v2(), launch_unix_ms=1000, launcher_ms=10,
                                       idle_rss_bytes=1024).control_poll_ms)
        for bad in (0, -1, "250", True, math.nan):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                parse_report(report_v2(control_poll_ms=bad), launch_unix_ms=1000,
                             launcher_ms=10, idle_rss_bytes=1024)

    def test_main_writes_the_poll_interval_into_the_identity_block(self) -> None:
        polled = Sample(100.0, 200.0, 10.0, 1024, ack_ms=10.0, ui_ready_ms=100.0,
                        requested_content_ms=200.0, readiness_identity_bound=True,
                        telemetry_schema=2, build=BUILD, platform=PLATFORM, control_poll_ms=250.0)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "result.json"
            argv = ["benchmark.py", "--launcher", "x", "--repository", directory,
                    "--file", "a.rs", "--hardware", "t", "--output", str(output)]
            with patch.object(sys, "argv", argv), \
                    patch("benchmark.launch_sample", return_value=polled), \
                    contextlib.redirect_stdout(io.StringIO()):
                benchmark.main()
            result = json.loads(output.read_text())
        self.assertEqual(result["gui_control_poll_ms"], 250.0)
        self.assertEqual(result["samples"][0]["control_poll_ms"], 250.0)


class ReportReadRetryTests(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.path = Path(directory.name) / "report.json"
        self.path.write_text(json.dumps({"schema": 2, "pid": 7}), encoding="utf-8")
        patcher = patch("benchmark_telemetry.time.sleep")
        self.sleep = patcher.start()
        self.addCleanup(patcher.stop)

    def flaky_open(self, failures, error):
        real = Path.open
        state = {"left": failures, "calls": 0}

        def fake_open(path, *args, **kwargs):
            state["calls"] += 1
            if state["left"] > 0:
                state["left"] -= 1
                raise error
            return real(path, *args, **kwargs)
        return patch.object(Path, "open", fake_open), state

    def test_a_single_permission_error_is_retried_not_fatal(self) -> None:
        fake, state = self.flaky_open(1, PermissionError(13, "report is being replaced"))
        with fake:
            self.assertEqual(read_report(self.path), {"schema": 2, "pid": 7})
        self.assertEqual(state["calls"], 2)
        self.sleep.assert_called_once()

    def test_windows_sharing_violation_is_retried_too(self) -> None:
        # Not a PermissionError by errno: only the Windows error code marks it transient.
        violation = OSError(5, "The process cannot access the file")
        self.assertNotIsInstance(violation, PermissionError)
        self.assertFalse(telemetry.is_transient_read_error(violation))
        violation.winerror = 32  # Populated by the OS layer on Windows.
        self.assertTrue(telemetry.is_transient_read_error(violation))
        fake, state = self.flaky_open(2, violation)
        with fake:
            self.assertEqual(read_report(self.path)["pid"], 7)
        self.assertEqual(state["calls"], 3)

    def test_a_report_that_stays_locked_fails_with_its_cause_after_a_bounded_wait(self) -> None:
        fake, state = self.flaky_open(10_000, PermissionError(13, "locked"))
        with fake, self.assertRaises(PermissionError):
            read_report(self.path)
        self.assertEqual(state["calls"], telemetry.TRANSIENT_READ_ATTEMPTS)

    def test_other_os_errors_are_not_retried(self) -> None:
        fake, state = self.flaky_open(1, OSError(5, "I/O error"))
        with fake, self.assertRaises(OSError):
            read_report(self.path)
        self.assertEqual(state["calls"], 1)
        self.sleep.assert_not_called()

    def test_missing_and_partial_reports_still_read_as_not_yet_written(self) -> None:
        self.assertIsNone(read_report(self.path.with_name("absent.json")))
        self.path.write_text('{"schema": 2,', encoding="utf-8")
        self.assertIsNone(read_report(self.path))


class AwaitGuiResourceTests(unittest.TestCase):
    class Probe:
        def __init__(self) -> None:
            self.calls = []

        def begin(self, pid):
            self.calls.append(("begin", pid))
            return "before"

        def finish(self, before, pid, rss):
            self.calls.append(("finish", before, pid, rss))
            return {"gui_only": {"rss_bytes": rss}, "total": {"rss_bytes": rss}}

    def wait(self, telemetry, probe=None):
        with patch("benchmark.read_report", return_value=telemetry), \
                patch("benchmark.time.sleep"), \
                patch("benchmark.process_info",
                      side_effect=[(True, 4096), (False, 0)]):
            return await_gui(Path("report"), 1000, 10, benchmark.time.monotonic() + 5,
                             False, probe)

    def test_resources_wrap_the_idle_window_and_keep_gui_only_rss(self) -> None:
        probe = self.Probe()
        result = self.wait(report_v2(), probe)
        self.assertEqual(probe.calls, [("begin", 123), ("finish", "before", 123, 4096)])
        self.assertEqual(result.idle_rss_bytes, 4096)
        self.assertEqual(result.resources["gui_only"]["rss_bytes"], 4096)

    def test_without_a_probe_resources_are_absent_not_zero(self) -> None:
        self.assertIsNone(self.wait(report_v2()).resources)

    def test_schema_2_without_requested_content_fails_the_run(self) -> None:
        with self.assertRaises(ValueError):
            self.wait(report_v2(requested_content_ms=None))

    def test_stale_gui_is_rejected_before_any_pid_query(self) -> None:
        with patch("benchmark.read_report", return_value=report_v2(started_unix_ms=1)), \
                patch("benchmark.process_info") as query, \
                self.assertRaises(ValueError):
            await_gui(Path("report"), 1000, 10, benchmark.time.monotonic() + 5)
        query.assert_not_called()


class SummaryAndResultTests(unittest.TestCase):
    def test_summary_keeps_schema_1_keys_and_adds_schema_2_percentiles(self) -> None:
        samples = tuple(sample(frame=100 + n, diff=200 + n, ack=5 + n) for n in range(30))
        summary = summarize(samples, windows=False)
        for key in ("first_frame_p95_ms", "first_diff_p95_ms", "max_idle_rss_bytes", "targets_met"):
            self.assertIn(key, summary)
        self.assertEqual(summary["first_frame_p95_ms"], 128)
        self.assertEqual(summary["ui_ready_p95_ms"], 128)
        self.assertEqual(summary["requested_content_p95_ms"], 228)
        self.assertEqual(summary["ack_p95_ms"], 33)
        self.assertTrue(summary["readiness_identity_bound"])
        self.assertEqual(set(summary["targets_met"]), {"first_frame", "first_diff", "idle_rss"})
        self.assertEqual(summary["resources"]["measured_runs"], 0)

    def test_unbound_samples_flag_the_summary(self) -> None:
        summary = summarize((sample(bound=False),) * 30, windows=False)
        self.assertFalse(summary["readiness_identity_bound"])

    def test_main_writes_schema_2_result_with_all_schema_1_keys(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "result.json"
            argv = ["benchmark.py", "--launcher", "/bin/dff", "--repository", directory,
                    "--file", "a.rs", "--hardware", "test", "--output", str(output),
                    "--software", "rev abc"]
            with patch.object(sys, "argv", argv), \
                    patch("benchmark.launch_sample", return_value=sample()), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(benchmark.main(), 0)
            result = json.loads(output.read_text())
        self.assertEqual(result["schema"], 2)
        for key in ("hardware", "platform", "python", "wsl_distro", "launcher",
                    "windows_from_wsl", "repository", "file", "requested_runs",
                    "completed_runs", "failure", "samples", "summary"):
            self.assertIn(key, result)
        self.assertEqual(result["software"], "rev abc")
        self.assertEqual(result["gui_build"], BUILD)
        self.assertEqual(result["gui_platform"], PLATFORM)
        self.assertEqual(result["environment"]["launcher"], "/bin/dff")
        row = result["samples"][0]
        for key in ("first_frame_ms", "first_diff_ms", "launcher_ms", "idle_rss_bytes",
                    "ack_ms", "ui_ready_ms", "requested_content_ms", "build", "platform"):
            self.assertIn(key, row)
        self.assertEqual(result["completed_runs"], 30)

    def test_main_fails_when_the_gui_build_changes_between_runs(self) -> None:
        other = Sample(100.0, 200.0, 10.0, 1024, ack_ms=10.0, ui_ready_ms=100.0,
                       requested_content_ms=200.0, readiness_identity_bound=True,
                       telemetry_schema=2, build={**BUILD, "version": "9"}, platform=PLATFORM)
        runs = [sample()] * 29 + [other]
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "result.json"
            argv = ["benchmark.py", "--launcher", "x", "--repository", directory,
                    "--file", "a.rs", "--hardware", "t", "--output", str(output)]
            with patch.object(sys, "argv", argv), \
                    patch("benchmark.launch_sample", side_effect=runs), \
                    contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(benchmark.main(), 1)
            result = json.loads(output.read_text())
        self.assertIn("build", result["failure"])
        self.assertIsNone(result["summary"])


class ArgumentValidationTests(unittest.TestCase):
    def parse(self, *extra):
        argv = ["benchmark.py", "--launcher", "x", "--repository", "/repo", "--file", "a.rs",
                "--hardware", "t", "--output", str(Path(tempfile.gettempdir()) / "r.json"),
                *extra]
        stderr = io.StringIO()
        with patch.object(sys, "argv", argv), contextlib.redirect_stderr(stderr):
            try:
                return benchmark.arguments(), ""
            except SystemExit:
                return None, stderr.getvalue()

    def test_software_is_optional_for_plain_runs_and_required_for_profiles(self) -> None:
        self.assertEqual(self.parse("--software", "rev")[0].software, "rev")
        self.assertIsNone(self.parse()[0].software)
        for flag in (["--launch-profile"], ["--scenario", "open"]):
            args, error = self.parse(*flag)
            self.assertIsNone(args)
            self.assertIn("--software", error)

    def test_scenario_option_combinations_are_validated(self) -> None:
        base = ["--software", "rev"]
        self.assertIn("--second-file", self.parse(*base, "--scenario", "navigate_aba")[1])
        self.assertEqual(self.parse(*base, "--scenario", "navigate_aba",
                                    "--second-file", "b.rs")[0].second_file, "b.rs")
        self.assertIn("--second-file", self.parse(*base, "--scenario", "open",
                                                  "--second-file", "b.rs")[1])
        self.assertIn("--launch-profile", self.parse(*base, "--scenario", "open",
                                                     "--launch-profile")[1])
        self.assertIn("--views", self.parse(*base, "--scenario", "open", "--views", "all,staged")[1])
        self.assertEqual(self.parse(*base, "--scenario", "switch_view",
                                    "--views", "all,staged")[0].views, ("all", "staged"))
        self.assertIn("--views", self.parse(*base, "--scenario", "switch_view",
                                            "--views", "all,bogus")[1])
        self.assertIn("30 runs", self.parse(*base, "--scenario", "open", "--runs", "5")[1])
        self.assertIn("--launch-profile or --scenario", self.parse("--case", "warm")[1])


if __name__ == "__main__":
    unittest.main()
