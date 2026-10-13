"""Scenario sequencing checks with injected launch, clock, and fixture callables."""

import argparse
import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import benchmark_scenarios as scenarios
from benchmark_scenarios import Hooks, Step, build_plan, drive, run_edit, step_summary


class Fakes:
    """Records every harness action in order and answers with canned samples."""

    def __init__(self, fail_on_request=None, latency=10.0):
        self.events = []
        self.requests = []
        self.edits = []
        self.exits = []
        self.closed = []
        self.fail_on_request = fail_on_request
        self.latency = latency
        self.counter = 0
        self.clock = 1000.0

    def new_id(self):
        self.counter += 1
        return f"id{self.counter}"

    def write_marker(self, name, marker):
        self.events.append(("marker", name, marker))

    def request(self, args, report, runtime, expected, exit_ms, pid):
        self.requests.append({"file": args.file, "view": args.view, "expected": expected,
                              "report": report, "runtime": runtime, "pid": pid})
        self.events.append(("request", args.file, args.view, expected["file"]))
        if self.fail_on_request == len(self.requests):
            raise RuntimeError("no current repository/file/marker render telemetry")
        return {"pid": 42, "request_to_render_ms": self.latency + len(self.requests),
                "launcher_ms": 5.0, "idle_rss_bytes": 1024, "phases_ms": {}}

    def edit(self, args, report, expected, write_edit, pid):
        self.events.append(("edit-start", expected["sample_id"]))
        write_edit()
        self.edits.append({"expected": expected, "pid": pid, "report": report})
        return {"pid": 42, "request_to_render_ms": 30.0, "launcher_ms": None,
                "idle_rss_bytes": 1024, "phases_ms": {}, "trigger": "edit"}

    def close(self, report, pid, started, timeout, windows):
        self.closed.append((report, pid, timeout, windows))

    def wait_exit(self, pid, timeout, windows):
        self.exits.append(pid)

    def now(self):
        self.clock += 1
        return self.clock

    def hooks(self):
        return Hooks(request=self.request, edit=self.edit, write_marker=self.write_marker,
                     new_id=self.new_id, close_window=self.close, wait_exit=self.wait_exit,
                     now_ms=self.now, progress=lambda _message: None)


def args(**overrides):
    values = {"repository": "/repo", "file": "a.rs", "runs": 2, "timeout": 10,
              "windows_from_wsl": False, "view": None}
    return argparse.Namespace(**{**values, **overrides})


class PlanTests(unittest.TestCase):
    def test_every_plan_names_its_steps(self):
        self.assertEqual([s.label for s in build_plan("open", "a").steps], ["open"])
        self.assertTrue(build_plan("open", "a").cold)
        self.assertFalse(build_plan("reopen", "a").cold)
        aba = build_plan("navigate_aba", "a", "b")
        self.assertEqual([(s.label, s.file) for s in aba.steps], [("a_to_b", "b"), ("b_to_a", "a")])
        views = build_plan("switch_view", "a", views=("all", "staged"))
        self.assertEqual([(s.label, s.view) for s in views.steps],
                         [("view_staged", "staged"), ("view_all", "all")])
        self.assertEqual(views.seed.view, "all")
        save = build_plan("save_update", "a")
        self.assertEqual(save.steps[0].trigger, "edit")

    def test_invalid_plans_are_rejected(self):
        for name, second in (("navigate_aba", None), ("navigate_aba", "a"), ("nope", None)):
            with self.subTest(name=name, second=second), self.assertRaises(ValueError):
                build_plan(name, "a", second)
        self.assertEqual(scenarios.parse_views("all, unstaged"), ("all", "unstaged"))
        for bad in ("all", "all,all", "all,bogus", "all,unstaged,staged"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                scenarios.parse_views(bad)


class ControlValidationTests(unittest.TestCase):
    def test_steps_reject_values_the_gui_would_reject(self):
        with self.assertRaises(ValueError):
            Step("bad", "a.rs", view="everything")
        with self.assertRaises(ValueError):
            Step("bad", "a.rs", trigger="save")
        self.assertEqual(Step("ok", "a.rs", "unpushed", "edit").view, "unpushed")

    def test_control_is_schema_1_with_only_view_and_edit_trigger_extras(self):
        plain = scenarios.expectation(Step("s", "a.rs"), "/repo", "id")
        self.assertEqual(plain, {"schema": 1, "sample_id": "id", "repository": "/repo",
                                 "file": "a.rs", "marker": "lit12-id"})
        full = scenarios.expectation(Step("s", "a.rs", "staged", "edit"), "/repo", "id")
        self.assertEqual((full["view"], full["trigger"], full["schema"]), ("staged", "edit", 1))


class DriveTests(unittest.TestCase):
    def drive(self, plan, fakes=None, **argument_overrides):
        fakes = fakes or Fakes()
        with tempfile.TemporaryDirectory() as directory:
            outcome = drive(plan, args(**argument_overrides), Path(directory), fakes.hooks(), 9000)
        return fakes, outcome

    def test_navigate_aba_requests_a_then_b_then_a_in_one_window(self):
        fakes, outcome = self.drive(build_plan("navigate_aba", "a.rs", "b.rs"))
        files = [r["file"] for r in fakes.requests]
        self.assertEqual(files, ["a.rs", "b.rs", "a.rs", "b.rs", "a.rs"])  # seed, then 2 runs
        self.assertTrue(all(r["expected"]["file"] == r["file"] for r in fakes.requests))
        self.assertEqual([r["pid"] for r in fakes.requests], [None, 42, 42, 42, 42])
        self.assertEqual(len({r["runtime"] for r in fakes.requests}), 1)
        self.assertEqual([(s["run"], s["step"]) for s in outcome.samples],
                         [(1, "a_to_b"), (1, "b_to_a"), (2, "a_to_b"), (2, "b_to_a")])
        self.assertEqual(outcome.completed_runs, 2)
        self.assertEqual(len(fakes.closed), 1)
        self.assertEqual(fakes.closed[0][1], 42)
        self.assertEqual(len({r["expected"]["marker"] for r in fakes.requests}), 5)

    def test_marker_is_written_before_each_launcher_request(self):
        fakes, _ = self.drive(build_plan("reopen", "a.rs"), runs=1)
        kinds = [event[0] for event in fakes.events]
        self.assertEqual(kinds, ["marker", "request", "marker", "request"])
        self.assertEqual(fakes.events[0][2], fakes.requests[0]["expected"]["marker"])

    def test_switch_view_passes_each_view_to_launcher_and_control(self):
        fakes, outcome = self.drive(build_plan("switch_view", "a.rs", views=("all", "unstaged")),
                                    runs=1)
        self.assertEqual([(r["view"], r["expected"].get("view")) for r in fakes.requests],
                         [("all", "all"), ("unstaged", "unstaged"), ("all", "all")])
        self.assertEqual([s["step"] for s in outcome.samples], ["view_unstaged", "view_all"])

    def test_save_update_edits_after_the_edit_hook_starts_and_sends_no_launcher_request(self):
        fakes, outcome = self.drive(build_plan("save_update", "a.rs"), runs=2)
        self.assertEqual(len(fakes.requests), 1)  # Only the seed opens the window.
        self.assertEqual(len(fakes.edits), 2)
        self.assertEqual(fakes.edits[0]["expected"]["trigger"], "edit")
        self.assertEqual(fakes.edits[0]["pid"], 42)
        first_edit = next(i for i, e in enumerate(fakes.events) if e[0] == "edit-start")
        self.assertEqual(fakes.events[first_edit + 1][0], "marker")  # Edit happens inside the hook.
        self.assertEqual(fakes.events[first_edit + 1][2], fakes.edits[0]["expected"]["marker"])
        self.assertEqual({s["trigger"] for s in outcome.samples}, {"edit"})

    def test_open_is_cold_with_a_new_window_and_report_per_run(self):
        fakes, outcome = self.drive(build_plan("open", "a.rs"), runs=3)
        self.assertEqual([r["pid"] for r in fakes.requests], [None, None, None])
        self.assertEqual(len({r["report"] for r in fakes.requests}), 3)
        self.assertEqual(len({r["runtime"] for r in fakes.requests}), 3)
        self.assertEqual(fakes.exits, [42, 42, 42])
        self.assertEqual(fakes.closed, [])
        self.assertEqual(outcome.completed_runs, 3)

    def test_failure_stops_the_run_keeps_earlier_samples_and_closes_the_window(self):
        fakes = Fakes(fail_on_request=4)  # seed, a_to_b, b_to_a, then run 2 a_to_b fails
        fakes_, outcome = self.drive(build_plan("navigate_aba", "a.rs", "b.rs"), fakes)
        self.assertEqual(len(outcome.samples), 2)
        self.assertEqual(outcome.completed_runs, 1)
        self.assertEqual(outcome.failures[0]["run"], 2)
        self.assertEqual(outcome.failures[0]["step"], "a_to_b")
        self.assertEqual(len(fakes_.closed), 1)

    def test_an_unarmed_edit_ends_the_run_without_pooling_the_sample(self):
        fakes = Fakes()

        def unarmed(args, report, expected, write_edit, pid):
            raise scenarios.EditNotArmed("GUI did not acknowledge the edit control",
                                         {"armed_before_edit": False, "arm_wait_seconds": 3.0})
        hooks = fakes.hooks()
        hooks = Hooks(request=hooks.request, edit=unarmed, write_marker=hooks.write_marker,
                      new_id=hooks.new_id, close_window=hooks.close_window,
                      wait_exit=hooks.wait_exit, now_ms=hooks.now_ms, progress=hooks.progress)
        with tempfile.TemporaryDirectory() as directory:
            outcome = drive(build_plan("save_update", "a.rs"), args(runs=3), Path(directory),
                            hooks, 9000)
        self.assertEqual(outcome.samples, ())
        self.assertEqual(outcome.completed_runs, 0)
        self.assertEqual(len(outcome.failures), 1)
        failure = outcome.failures[0]
        self.assertEqual((failure["run"], failure["step"]), (1, "save_update"))
        self.assertIs(failure["armed_before_edit"], False)  # Kept for diagnosis.
        self.assertIn("did not acknowledge", failure["error"])
        self.assertEqual(len(fakes.closed), 1)

    def test_seed_failure_is_reported_and_still_closes(self):
        fakes = Fakes(fail_on_request=1)
        _, outcome = self.drive(build_plan("reopen", "a.rs"), fakes)
        self.assertEqual(outcome.failures[0]["run"], "seed")
        self.assertEqual(outcome.samples, ())
        self.assertEqual(len(fakes.closed), 1)
        self.assertIsNone(fakes.closed[0][1])

    def test_per_step_and_scenario_percentiles_are_labelled(self):
        fakes, outcome = self.drive(build_plan("navigate_aba", "a.rs", "b.rs"), runs=3)
        summary = step_summary(build_plan("navigate_aba", "a.rs", "b.rs"), list(outcome.samples))
        self.assertEqual(set(summary["steps"]), {"a_to_b", "b_to_a"})
        self.assertEqual(summary["steps"]["a_to_b"]["samples"], 3)
        self.assertEqual(summary["steps"]["a_to_b"]["file"], "b.rs")
        self.assertEqual(summary["scenario"]["samples"], 6)
        self.assertLessEqual(summary["scenario"]["p50_ms"], summary["scenario"]["p95_ms"])


class EditTriggerTests(unittest.TestCase):
    def run_edit(self, events, rendered=None, armed=True):
        clock = iter([5000.0, 5003.0])

        def render(report, expected, launch_ms, launcher_ms, deadline, windows, pid,
                   origin_ms=None, probe=None):
            events.append(("render", launch_ms, origin_ms, launcher_ms))
            return rendered or {"pid": 42, "request_to_render_ms": 80.0, "launcher_ms": None,
                                "content_ready_ms": 5.0, "render_ms": 4.0,
                                "idle_rss_bytes": 1024, "resources": None, "telemetry": {}}
        expected = {"schema": 1, "sample_id": "s1", "repository": "/repo", "file": "a.rs",
                    "marker": "m1", "trigger": "edit"}
        return run_edit(
            argparse.Namespace(timeout=10, windows_from_wsl=False), Path("report.json"), expected,
            lambda: events.append(("edit",)), 42,
            control=lambda report, value: events.append(("control", value["trigger"])),
            armed=lambda report, sample_id, deadline: events.append(("armed", sample_id)) or armed,
            render=render, now_ms=lambda: next(clock))

    def test_control_is_written_before_the_file_edit_and_trigger_time_is_recorded(self):
        events = []
        sample = self.run_edit(events)
        self.assertEqual([e[0] for e in events], ["control", "armed", "edit", "render"])
        self.assertEqual(sample["trigger_unix_ms"], 5003.0)
        self.assertEqual(sample["control_unix_ms"], 5000.0)
        self.assertEqual(events[-1], ("render", 5000.0, 5003.0, None))
        self.assertEqual(sample["trigger"], "edit")
        self.assertTrue(sample["armed_before_edit"])
        self.assertIsNone(sample["ack_ms"])
        self.assertEqual(sample["requested_content_ms"], 80.0)
        self.assertEqual(sample["launch_unix_ms"], 5003.0)

    def test_an_unarmed_gui_fails_the_sample_before_the_file_is_edited(self):
        events = []
        with self.assertRaises(RuntimeError) as caught:
            self.run_edit(events, armed=False)
        self.assertIsInstance(caught.exception, scenarios.EditNotArmed)
        self.assertIn("armed_before_edit=false", str(caught.exception))
        self.assertIn("s1", str(caught.exception))
        self.assertEqual([e[0] for e in events], ["control", "armed"])  # No edit, no render.
        self.assertFalse(caught.exception.detail["armed_before_edit"])

    def test_arm_wait_spans_several_gui_poll_intervals(self):
        self.assertGreaterEqual(scenarios.ARM_WAIT_SECONDS, 3 * scenarios.CONTROL_POLL_SECONDS)
        waits = []
        events = []
        expected = {"schema": 1, "sample_id": "s1", "repository": "/repo", "file": "a.rs",
                    "marker": "m1", "trigger": "edit"}
        with patch("benchmark_scenarios.read_report", return_value={"control_poll_ms": 1000}):
            self.assertEqual(scenarios.arm_wait_seconds(Path("r")),
                             scenarios.CONTROL_WAIT_INTERVALS * 1.0)
            run_edit(argparse.Namespace(timeout=60, windows_from_wsl=False), Path("r"), expected,
                     lambda: None, 42, control=lambda *_: None,
                     armed=lambda report, sample_id, deadline: waits.append(
                         deadline - scenarios.time.monotonic()) or True,
                     render=lambda *a, **k: {"pid": 42, "request_to_render_ms": 1.0,
                                             "launcher_ms": None},
                     now_ms=lambda: 5000.0)
        self.assertGreater(waits[0], 3 * 1.0)  # Several of the GUI's own 1 s intervals.
        for report in (None, {}, {"control_poll_ms": -5}, {"control_poll_ms": "fast"},
                       {"control_poll_ms": True}, {"control_poll_ms": float("nan")}):
            with self.subTest(report=report), \
                    patch("benchmark_scenarios.read_report", return_value=report):
                self.assertEqual(scenarios.arm_wait_seconds(Path("r")), scenarios.ARM_WAIT_SECONDS)
        with patch("benchmark_scenarios.read_report", side_effect=ValueError("not an object")):
            self.assertEqual(scenarios.arm_wait_seconds(Path("r")), scenarios.ARM_WAIT_SECONDS)
        with patch("benchmark_scenarios.read_report", return_value={"control_poll_ms": 250}):
            self.assertEqual(scenarios.arm_wait_seconds(Path("r")), scenarios.ARM_WAIT_SECONDS)

    def test_wait_armed_recognises_only_the_current_sample(self):
        report = {"request": {"sample_id": "other"}}
        with patch("benchmark_scenarios.read_report", side_effect=[None, report,
                                                                    {"request": {"sample_id": "s1"}}]), \
                patch("benchmark_scenarios.time.sleep"):
            self.assertTrue(scenarios.wait_armed(Path("r"), "s1",
                                                 scenarios.time.monotonic() + 5))
        with patch("benchmark_scenarios.read_report", return_value=report), \
                patch("benchmark_scenarios.time.sleep"):
            self.assertFalse(scenarios.wait_armed(Path("r"), "s1",
                                                  scenarios.time.monotonic() + 0.02))


class RunScenarioTests(unittest.TestCase):
    def test_result_document_is_schema_2_with_step_summaries_and_restored_fixture(self):
        import launch_profile as profile
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("a.rs", "b.rs"):
                (root / name).write_bytes(b"FASTDIFF_LAUNCH_MARKER\nbody\n")
            output = root / "result.json"
            namespace = argparse.Namespace(
                scenario="navigate_aba", launcher="dff", repository=directory, file="a.rs",
                second_file="b.rs", views=("all", "unstaged"), hardware="test",
                software="rev", output=output, runs=2, timeout=10, windows_from_wsl=False,
                wsl_distro=None, report_directory=None, command_kind="direct",
                trace_phases=False)
            seen = []

            def request(args, report, runtime, expected, exit_ms, pid=None):
                seen.append((args.file, expected["file"], pid))
                self.assertIn(expected["marker"].encode(), (root / expected["file"]).read_bytes())
                return {"pid": 42, "request_to_render_ms": 20.0, "launcher_ms": 4.0,
                        "content_ready_ms": 9.0, "render_ms": 3.0, "idle_rss_bytes": 2048,
                        "phases_ms": {}, "first_frame_ms": None, "telemetry_schema": 2,
                        "build": {"version": "1", "revision": None, "profile": "release",
                                  "protocol": 5},
                        "platform": {"os": "linux", "arch": "x86_64"}, "resources": None,
                        "control_poll_ms": 250.0}
            with patch("launch_profile.run_request", side_effect=request), \
                    patch("launch_profile.close_warm_window") as close, \
                    patch("launch_profile.wait_for_exit"), \
                    contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(scenarios.run_scenario(namespace), 0)
            result = json.loads(output.read_text())
            self.assertEqual((root / "a.rs").read_bytes(), b"FASTDIFF_LAUNCH_MARKER\nbody\n")
            self.assertEqual((root / "b.rs").read_bytes(), b"FASTDIFF_LAUNCH_MARKER\nbody\n")
        self.assertEqual(result["schema"], 2)
        self.assertEqual(result["scenario"], "navigate_aba")
        self.assertEqual(result["completed_runs"], 2)
        self.assertEqual(set(result["summary"]["steps"]), {"a_to_b", "b_to_a"})
        self.assertIn("p95_ms", result["summary"]["scenario"])
        self.assertEqual(result["gui_build"]["version"], "1")
        self.assertEqual(result["gui_control_poll_ms"], 250.0)
        self.assertEqual(result["software"], "rev")
        self.assertEqual([s["step"] for s in result["samples"]],
                         ["a_to_b", "b_to_a", "a_to_b", "b_to_a"])
        self.assertEqual([item[0] for item in seen], ["a.rs", "b.rs", "a.rs", "b.rs", "a.rs"])
        close.assert_called_once()
        self.assertIsNone(result["failure"])


if __name__ == "__main__":
    unittest.main()
