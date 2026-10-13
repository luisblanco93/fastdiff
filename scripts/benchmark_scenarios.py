"""Step-labelled end-to-end scenarios over the marked scratch fixture.

A plan is data (cold or warm, an unmeasured seed, ordered measured steps). `drive`
executes it through injected callables, so sequencing, trigger ordering, and
failure handling are unit-testable without a GUI. `run_scenario` binds the real
launcher, fixture, and clock.

Every requested file must be changed (modified or untracked) when requested: the
GUI opens an unchanged file as a full document and completes no sample. The marker
edit written before each request satisfies that for a scratch file.

An edit-trigger sample is taken only after the GUI has acknowledged its control.
A GUI that did not arm within the arm wait fails the run for that sample; its
`armed_before_edit=false` is kept in the failure record, never pooled into p50/p95.
"""

from __future__ import annotations

import argparse
import json
import math
import subprocess
import sys
import tempfile
import time
import uuid
from dataclasses import asdict, dataclass
from functools import partial
from pathlib import Path
from typing import Any, Callable, Sequence

from benchmark_telemetry import (CONTROL_POLL_SECONDS, CONTROL_WAIT_INTERVALS, RESULT_SCHEMA,
                                 gui_identity, harness_identity, percentile, read_report,
                                 request_sample)

SCENARIOS = ("open", "reopen", "navigate_aba", "switch_view", "save_update")
VIEWS = ("all", "unstaged", "staged", "unpushed")
DEFAULT_VIEWS = ("all", "unstaged")
# The GUI polls its control file every CONTROL_POLL_SECONDS once a sample is published,
# so the edit control is seen after up to one interval and acknowledged on the next
# report write. Wait for many intervals: an unarmed sample fails the run.
ARM_WAIT_SECONDS = CONTROL_WAIT_INTERVALS * CONTROL_POLL_SECONDS
ARM_POLL_SECONDS = 0.025
MARKER_PREFIX = "lit12-"
MEASUREMENT_ERRORS = (OSError, ValueError, RuntimeError, subprocess.SubprocessError)


TRIGGERS = ("open", "edit")


@dataclass(frozen=True)
class Step:
    label: str
    file: str
    view: str | None = None
    trigger: str = "open"  # "open": launcher request; "edit": harness edits the open file

    def __post_init__(self) -> None:
        # The GUI rejects a control with an unknown view or trigger as a sample.
        if self.view is not None and self.view not in VIEWS:
            raise ValueError(f"step {self.label}: view must be one of {', '.join(VIEWS)}")
        if self.trigger not in TRIGGERS:
            raise ValueError(f"step {self.label}: trigger must be one of {', '.join(TRIGGERS)}")


@dataclass(frozen=True)
class ScenarioPlan:
    name: str
    cold: bool  # True: a fresh GUI per run; False: one window opened by `seed`
    seed: Step | None
    steps: tuple[Step, ...]


def parse_views(value: str) -> tuple[str, str]:
    names = tuple(part.strip() for part in value.split(","))
    if len(names) != 2 or names[0] == names[1] or any(name not in VIEWS for name in names):
        raise ValueError(f"--views needs two different views from {', '.join(VIEWS)}")
    return names[0], names[1]


def build_plan(name: str, file: str, second_file: str | None = None,
               views: tuple[str, str] = DEFAULT_VIEWS) -> ScenarioPlan:
    seed = Step("seed", file)
    if name == "open":
        return ScenarioPlan(name, True, None, (Step("open", file),))
    if name == "reopen":
        return ScenarioPlan(name, False, seed, (Step("reopen", file),))
    if name == "navigate_aba":
        if not second_file or second_file == file:
            raise ValueError("navigate_aba needs a second marked file different from the first")
        return ScenarioPlan(name, False, seed,
                            (Step("a_to_b", second_file), Step("b_to_a", file)))
    if name == "switch_view":
        first, second = views
        return ScenarioPlan(name, False, Step("seed", file, first),
                            (Step(f"view_{second}", file, second), Step(f"view_{first}", file, first)))
    if name == "save_update":
        return ScenarioPlan(name, False, seed, (Step("save_update", file, trigger="edit"),))
    raise ValueError(f"unknown scenario: {name}")


def wall_ms() -> float:
    return time.time_ns() / 1_000_000


class EditNotArmed(RuntimeError):
    """The GUI did not acknowledge the edit control before the file was edited.

    `detail` is merged into the failure record for diagnosis.
    """

    def __init__(self, message: str, detail: dict) -> None:
        super().__init__(message)
        self.detail = detail


def arm_wait_seconds(report: Path) -> float:
    """At least ARM_WAIT_SECONDS, and CONTROL_WAIT_INTERVALS of the GUI's own poll interval."""
    try:
        reported = (read_report(report) or {}).get("control_poll_ms")
    except (OSError, ValueError):
        reported = None
    if type(reported) in (int, float) and math.isfinite(reported) and reported > 0:
        return max(ARM_WAIT_SECONDS, CONTROL_WAIT_INTERVALS * reported / 1000)
    return ARM_WAIT_SECONDS


def wait_armed(report: Path, sample_id: str, deadline: float) -> bool:
    """True once the GUI reports it accepted this edit control."""
    while time.monotonic() < deadline:
        telemetry = read_report(report)
        request = telemetry.get("request") if telemetry else None
        if isinstance(request, dict) and request.get("sample_id") == sample_id:
            return True
        time.sleep(ARM_POLL_SECONDS)
    return False


def run_edit(args: Any, report: Path, expected: dict, write_edit: Callable[[], None],
             pid: int | None, *, control: Callable[[Path, dict], None],
             armed: Callable[[Path, str, float], bool], render: Callable[..., dict],
             now_ms: Callable[[], float] = wall_ms, probe: Any = None) -> dict:
    """Edit the open file after arming the GUI; latency is render minus trigger.

    The control is written first so the GUI is watching the selected tab. A GUI
    that has not acknowledged it within the arm wait fails the sample before the
    file is touched: an edit it may have missed would measure nothing, and a late
    arm would only lengthen the latency, so it is never pooled. Samples keep
    `armed_before_edit` (true) as the record that arming was verified.
    """
    deadline = time.monotonic() + args.timeout
    control_ms = now_ms()
    control(report, expected)
    wait = arm_wait_seconds(report)
    acknowledged = armed(report, expected["sample_id"],
                         min(deadline, time.monotonic() + wait))
    if not acknowledged:
        raise EditNotArmed(
            f"GUI did not acknowledge the edit control {expected['sample_id']} within "
            f"{wait:.1f} s; the sample was not taken (armed_before_edit=false)",
            {"armed_before_edit": False, "arm_wait_seconds": wait,
             "control_unix_ms": control_ms})
    trigger_ms = now_ms()
    write_edit()
    rendered = render(report, expected, control_ms, None, deadline, args.windows_from_wsl,
                      pid, origin_ms=trigger_ms, probe=probe)
    sample = request_sample(rendered, None, expected, None, trigger_ms, {}, [])
    return {**sample, "trigger_unix_ms": trigger_ms, "control_unix_ms": control_ms,
            "armed_before_edit": acknowledged}


@dataclass(frozen=True)
class Hooks:
    request: Callable[..., dict]
    edit: Callable[..., dict]
    write_marker: Callable[[str, str], None]
    new_id: Callable[[], str]
    close_window: Callable[[Path, int | None, float, float, bool], None]
    wait_exit: Callable[[int, float, bool], None]
    now_ms: Callable[[], float] = wall_ms
    progress: Callable[[str], None] = partial(print, flush=True)


@dataclass(frozen=True)
class ScenarioRun:
    samples: tuple[dict, ...]
    warmup: dict | None
    failures: tuple[dict, ...]
    completed_runs: int


def expectation(step: Step, repository: str, sample_id: str) -> dict:
    expected = {"schema": 1, "sample_id": sample_id, "repository": repository,
                "file": step.file, "marker": MARKER_PREFIX + sample_id}
    if step.view is not None:
        expected["view"] = step.view
    if step.trigger != "open":
        expected["trigger"] = step.trigger
    return expected


def step_args(args: Any, step: Step) -> Any:
    """A copy of the run arguments aimed at this step's file and view."""
    return argparse.Namespace(**{**vars(args), "file": step.file, "view": step.view})


def execute_step(plan_step: Step, args: Any, report: Path, runtime: Path, expected: dict,
                 exit_ms: int, pid: int | None, hooks: Hooks) -> dict:
    aimed = step_args(args, plan_step)
    if plan_step.trigger == "edit":
        return hooks.edit(aimed, report, expected,
                          lambda: hooks.write_marker(plan_step.file, expected["marker"]), pid)
    hooks.write_marker(plan_step.file, expected["marker"])
    return hooks.request(aimed, report, runtime, expected, exit_ms, pid)


def drive(plan: ScenarioPlan, args: Any, root: Path, hooks: Hooks, exit_ms: int) -> ScenarioRun:
    """Run the plan `args.runs` times; stop at the first failed step."""
    samples: list[dict] = []
    failures: list[dict] = []
    warmup: dict | None = None
    pid: int | None = None
    completed = 0
    report, runtime = root / "warm.json", root / "ipc-warm"
    seed_started = hooks.now_ms()
    try:
        if plan.seed is not None:
            seed = expectation(plan.seed, args.repository, hooks.new_id())
            try:
                hooks.write_marker(plan.seed.file, seed["marker"])
                warmup = hooks.request(step_args(args, plan.seed), report, runtime, seed,
                                       exit_ms, None)
            except MEASUREMENT_ERRORS as error:
                failures.append({"run": "seed", "step": plan.seed.label,
                                 "sample_id": seed["sample_id"], "error": str(error)})
                return ScenarioRun((), None, tuple(failures), 0)
            pid = warmup["pid"]
        for run in range(args.runs):
            if plan.cold:
                report, runtime = root / f"cold-{run}.json", root / f"ipc-{run}"
            for index, step in enumerate(plan.steps):
                expected = expectation(step, args.repository, hooks.new_id())
                try:
                    sample = execute_step(step, args, report, runtime, expected, exit_ms, pid, hooks)
                    if plan.cold:
                        hooks.wait_exit(sample["pid"], args.timeout, args.windows_from_wsl)
                except MEASUREMENT_ERRORS as error:
                    failures.append({"run": run + 1, "step": step.label,
                                     "sample_id": expected["sample_id"], "error": str(error),
                                     **getattr(error, "detail", {})})
                    return ScenarioRun(tuple(samples), warmup, tuple(failures), completed)
                samples.append({**sample, "scenario": plan.name, "step": step.label,
                                "step_index": index, "run": run + 1})
                hooks.progress(f"{run + 1}/{args.runs} {step.label}: "
                               f"{sample['request_to_render_ms']:.1f} ms")
            completed += 1
    finally:
        if plan.seed is not None:
            hooks.close_window(report, pid, seed_started, args.timeout, args.windows_from_wsl)
    return ScenarioRun(tuple(samples), warmup, tuple(failures), completed)


def step_summary(plan: ScenarioPlan, samples: Sequence[dict]) -> dict:
    """Nearest-rank p50/p95 request-to-render per labelled step and for the scenario."""
    def stats(values: tuple[float, ...]) -> dict:
        return {"samples": len(values), "p50_ms": percentile(values, 0.5),
                "p95_ms": percentile(values, 0.95)}
    steps = {}
    for step in plan.steps:
        values = tuple(s["request_to_render_ms"] for s in samples if s["step"] == step.label)
        if values:
            steps[step.label] = {**stats(values), "file": step.file, "view": step.view,
                                 "trigger": step.trigger}
    pooled = tuple(s["request_to_render_ms"] for s in samples)
    return {"scenario": {"name": plan.name, **(stats(pooled) if pooled else {})}, "steps": steps}


def scenario_document(args: Any, plan: ScenarioPlan, outcome: ScenarioRun,
                      failures: list[dict], summary: dict | None, identity: dict) -> dict:
    return {"schema": RESULT_SCHEMA, "scenario": args.scenario, "plan": asdict(plan),
            "hardware": args.hardware, "software": args.software,
            "wsl_distro": args.wsl_distro, "windows_from_wsl": args.windows_from_wsl,
            "launcher": args.launcher, "repository": args.repository, "file": args.file,
            "second_file": args.second_file, "views": list(args.views),
            "environment": harness_identity(args), **identity,
            "case": "cold" if plan.cold else "warm", "command_kind": args.command_kind,
            "trace_phases": args.trace_phases, "requested_runs": args.runs,
            "completed_runs": outcome.completed_runs, "failure": failures or None,
            "samples": list(outcome.samples), "warmup": outcome.warmup, "summary": summary}


def run_scenario(args: Any) -> int:
    from contextlib import ExitStack
    import launch_profile as profile
    from benchmark import exit_budget_ms, open_windows_sampler, probe_for_args

    plan = build_plan(args.scenario, args.file, args.second_file, args.views)
    outcome = ScenarioRun((), None, (), 0)
    failures: list[dict] = []
    try:
        with ExitStack() as cleanup:
            root = Path(cleanup.enter_context(tempfile.TemporaryDirectory(
                prefix="fastdiff-scenario-", dir=args.report_directory)))
            sampler, sampler_error = open_windows_sampler(args, cleanup)
            args.resource_probe = probe_for_args(args, sampler, sampler_error)
            repository = profile.fixture_repository(args)
            names = dict.fromkeys(step.file for step in (plan.seed, *plan.steps) if step)
            fixtures = {name: cleanup.enter_context(profile.MarkerFixture(repository, name))
                        for name in names}
            hooks = Hooks(
                request=profile.run_request,
                edit=partial(run_edit, control=profile.atomic_control, armed=wait_armed,
                             render=profile.await_request, probe=args.resource_probe),
                write_marker=lambda name, marker: fixtures[name].write(marker),
                new_id=lambda: uuid.uuid4().hex,
                close_window=profile.close_warm_window, wait_exit=profile.wait_for_exit)
            exit_ms = (exit_budget_ms(args.resource_probe) if plan.cold
                       else int((args.runs * len(plan.steps) + 2) * args.timeout * 1000))
            outcome = drive(plan, args, root, hooks, exit_ms)
    except MEASUREMENT_ERRORS as error:
        failures.append({"run": "setup/cleanup", "error": str(error)})
    failures = failures + list(outcome.failures)
    rows = list(outcome.samples)
    identity, identity_error = gui_identity(rows + ([outcome.warmup] if outcome.warmup else []))
    if identity_error:
        failures.append({"run": "identity", "error": identity_error})
    summary = None
    if rows and not failures:
        summary = {**profile.summarize_requests(rows, plan.cold), **step_summary(plan, rows)}
    document = scenario_document(args, plan, outcome, failures, summary, identity)
    args.output.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    if failures:
        print(f"Scenario incomplete: {failures}", file=sys.stderr)
        return 1
    print(json.dumps(summary, indent=2))
    return 0
