"""Opt-in managed/direct cold and warm profiling for benchmark.py."""
from __future__ import annotations

import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import tempfile
import time
import uuid

from benchmark import (IDLE_SECONDS, POLL_SECONDS, exit_budget_ms,
                       open_windows_sampler, probe_for_args, process_info)
from benchmark_resources import summarize_resources
from benchmark_telemetry import (CONTROL_POLL_SECONDS, MAX_REPORT_BYTES, RESULT_SCHEMA,
                                 control_poll_ms, gui_identity, harness_identity, parse_build,
                                 parse_platform, parse_report, percentile, positive_number,
                                 read_report, readiness, reported_schema, request_frame_metrics,
                                 request_sample)

# An unreported GUI needs a few control polls to consume a close control, and never
# less than the idle window the harness already waits.
CLOSE_GRACE_SECONDS = max(IDLE_SECONDS, 4 * CONTROL_POLL_SECONDS)
FRAME_STAT_FIELDS = ("frames", "frame_cpu_total_ms", "max_frame_cpu_ms")
IDENTITY_KEYS = ("sample_id", "repository", "file", "marker")
MARKER = b"FASTDIFF_LAUNCH_MARKER"


class MarkerFixture:
    """Only edit an explicitly marked scratch file; preserve concurrent edits."""
    def __init__(self, repository: Path, filename: str) -> None:
        self.path = (repository / filename).resolve()
        if Path(filename).is_absolute() or not self.path.is_relative_to(repository.resolve()):
            raise ValueError("profile file must be inside the scratch repository")
        self.original = self.path.read_bytes()
        if self.original.count(MARKER) != 1 or MARKER not in self.original.split(b"\n", 1)[0]:
            raise ValueError("scratch file needs exactly one FASTDIFF_LAUNCH_MARKER on its first line")
        self.latest = self.original

    def __enter__(self) -> MarkerFixture:
        return self

    def write(self, marker: str) -> None:
        if self.path.read_bytes() != self.latest:
            raise RuntimeError("scratch file changed externally; preserving that edit")
        content = self.original.replace(MARKER, marker.encode("ascii"))
        self.path.write_bytes(content)
        self.latest = content

    def __exit__(self, *_: object) -> None:
        if self.path.read_bytes() != self.latest:
            raise RuntimeError("scratch file changed externally; preserving that edit")
        self.path.write_bytes(self.original)


def atomic_control(report: Path, value: dict) -> None:
    target = Path(str(report) + ".request.json")
    pending = target.with_name(target.name + ".tmp")
    pending.write_text(json.dumps(value), encoding="utf-8")
    pending.replace(target)


def check_request_intent(request: dict, expected: dict, schema: int) -> bool:
    """Match the trigger/view the GUI reports; True when the view was confirmed."""
    wanted = expected.get("trigger", "open")
    reported = request.get("trigger")
    if (reported is not None and reported != wanted) or (
            schema == 2 and reported is None and wanted != "open"):
        raise ValueError("telemetry trigger does not match the requested trigger")
    view = expected.get("view")
    if view is None:
        return True
    if schema == 2 and request.get("view") != view:
        raise ValueError("telemetry view does not match the requested view")
    return schema == 2


def parse_request(report: dict, expected: dict, launch_ms: float,
                  expected_pid: int | None = None, origin_ms: float | None = None) -> dict:
    """Validate one request sample; latency runs from `origin_ms` (default: launch)."""
    schema = reported_schema(report)
    pid = report.get("pid")
    if type(pid) is not int or pid <= 0 or (expected_pid is not None and pid != expected_pid):
        raise ValueError("request must render in the existing benchmark window")
    request = report.get("request")
    if not isinstance(request, dict) or any(request.get(key) != expected[key]
                                           for key in IDENTITY_KEYS):
        raise ValueError("telemetry does not match requested repository/file/content marker")
    view_verified = check_request_intent(request, expected, schema)
    if schema == 2:
        readiness(report, schema)  # Complete, finite, and ordered readiness is mandatory.
    received = positive_number(request, "received_unix_ms")
    rendered = positive_number(request, "rendered_unix_ms")
    content = positive_number(request, "content_ready_ms")
    render = positive_number(request, "render_ms")
    origin = launch_ms if origin_ms is None else origin_ms
    if received < launch_ms - 5 or rendered < max(received, origin):
        raise ValueError("stale or out-of-order request telemetry")
    return {"pid": pid, "request_to_render_ms": rendered - origin,
            "content_ready_ms": content, "render_ms": render,
            "telemetry_schema": schema, "view_verified": view_verified,
            "build": parse_build(report), "platform": parse_platform(report),
            "control_poll_ms": control_poll_ms(report),
            **request_frame_metrics(request, schema)}


def parse_phases(path: Path, sample_id: str, launch_ms: float) -> tuple[dict, list]:
    if path.stat().st_size > MAX_REPORT_BYTES:
        raise ValueError("launch trace exceeded 64 KiB")
    rows = [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]
    if not rows:
        raise ValueError("missing launch phase trace")
    phases: dict[str, float] = {}
    for row in rows:
        if (not isinstance(row, dict) or type(row.get("schema")) is not int or row["schema"] != 1
                or row.get("sample_id") != sample_id or type(row.get("pid")) is not int
                or row["pid"] <= 0 or not isinstance(row.get("phase"), str) or not row["phase"]):
            raise ValueError("invalid or uncorrelated launch phase")
        phase = row["phase"]
        phases[phase] = phases.get(phase, 0) + positive_number(row, "duration_ms")
        if phase == "python_imports":
            started = positive_number(row, "started_unix_ms")
            if started < launch_ms - 5:
                raise ValueError("Python entry predates launch")
            phases["python_startup"] = max(0, started - launch_ms)
    return phases, rows


def launch_command(args, report: Path, runtime: Path, sample_id: str,
                   exit_ms: int) -> tuple[list[str], dict]:
    env = {key: value for key, value in os.environ.items()
           if key not in {"FASTDIFF_LAUNCH_TRACE", "FASTDIFF_LAUNCH_SAMPLE_ID"}}
    env = {**env, "FASTDIFF_BENCH_REPORT": str(report),
           "FASTDIFF_BENCH_EXIT_MS": str(exit_ms), "FASTDIFF_RUNTIME_DIR": str(runtime)}
    if args.trace_phases:
        env = {**env, "FASTDIFF_LAUNCH_TRACE": str(report) + f".{sample_id}.jsonl",
               "FASTDIFF_LAUNCH_SAMPLE_ID": sample_id}
    command = [args.launcher, "--file", args.file]
    if getattr(args, "view", None):
        command += ["--view", args.view]
    if os.name != "nt" and not args.windows_from_wsl and not args.wsl_distro:
        command += ["--native"]
    command += ["--", args.repository]
    if os.name == "nt" and Path(args.launcher).suffix.lower() in {".cmd", ".bat"}:
        # Batch wrappers use CMD expansion even with shell=False.
        if any(any(character in argument for character in '&|<>^%!"()\r\n') for argument in command):
            raise ValueError("CMD wrapper profiling requires paths without CMD metacharacters")
    if args.wsl_distro:
        def linux_path(path: str) -> str:
            return subprocess.check_output(
                ["wsl.exe", "--distribution", args.wsl_distro, "--exec", "wslpath", "-u", path],
                text=True, timeout=10).strip()
        variables = {key: linux_path(value) if key in {
            "FASTDIFF_BENCH_REPORT", "FASTDIFF_RUNTIME_DIR", "FASTDIFF_LAUNCH_TRACE"}
            else value for key, value in env.items() if key.startswith("FASTDIFF_")}
        # Use env arguments, never a shell expression, across the distro boundary.
        command = ["wsl.exe", "--distribution", args.wsl_distro, "--exec", "env",
                   *(f"{key}={value}" for key, value in variables.items()), *command]
    return command, env


def await_request(report: Path, expected: dict, launch_ms: float, launcher_ms: float | None,
                  deadline: float, windows_from_wsl: bool,
                  expected_pid: int | None = None, origin_ms: float | None = None,
                  probe=None) -> dict:
    while time.monotonic() < deadline:
        telemetry = read_report(report)
        request = telemetry.get("request") if telemetry else None
        if (isinstance(request, dict) and request.get("sample_id") == expected["sample_id"]
                and request.get("rendered_unix_ms") is not None):
            result = parse_request(telemetry, expected, launch_ms, expected_pid, origin_ms)
            before = probe.begin(result["pid"]) if probe is not None else None
            time.sleep(IDLE_SECONDS)
            alive, rss = process_info(result["pid"], windows_from_wsl)
            if not alive or rss <= 0:
                raise RuntimeError("benchmark GUI exited before current content was observed")
            resources = (probe.finish(before, result["pid"], rss)
                         if probe is not None and before is not None else None)
            return {**result, "launcher_ms": launcher_ms, "idle_rss_bytes": rss,
                    "resources": resources, "telemetry": telemetry}
        time.sleep(POLL_SECONDS)
    raise RuntimeError("no current repository/file/marker render telemetry")


def wait_for_exit(pid: int, timeout: float, windows_from_wsl: bool) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not process_info(pid, windows_from_wsl)[0]:
            return
        time.sleep(POLL_SECONDS)
    raise RuntimeError("isolated benchmark GUI did not close; it retains an automatic exit timer")


def run_request(args, report: Path, runtime: Path, expected: dict,
                exit_ms: int, expected_pid: int | None = None) -> dict:
    command, env = launch_command(args, report, runtime, expected["sample_id"], exit_ms)
    atomic_control(report, expected)
    launch_ms = time.time_ns() / 1_000_000
    started = time.monotonic()
    with tempfile.TemporaryFile() as diagnostics:
        child = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL, stderr=diagnostics)
        try:
            child.wait(timeout=args.timeout)
            launcher_ms = (time.monotonic() - started) * 1000
            if child.returncode:
                diagnostics.seek(0)
                raise RuntimeError(diagnostics.read(MAX_REPORT_BYTES).decode(errors="replace").strip()
                                   or f"launcher exited {child.returncode}")
            sample = await_request(report, expected, launch_ms, launcher_ms,
                                   started + args.timeout, args.windows_from_wsl, expected_pid,
                                   probe=getattr(args, "resource_probe", None))
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
    phases, rows = ({}, [])
    if args.trace_phases:
        phases, rows = parse_phases(Path(env["FASTDIFF_LAUNCH_TRACE"]), expected["sample_id"], launch_ms)
        required = {"repository_validation", "forwarding_acceptance"}
        if expected_pid is None:
            required |= {"process_start"}
        if args.command_kind == "managed":
            required |= {"python_imports", "receipt_validation", "receipt_version"}
        if not required.issubset(phases):
            raise ValueError(f"incomplete phase trace: missing {sorted(required - phases.keys())}")
    ui_ready = None
    if expected_pid is None:
        ui_ready = parse_report(sample["telemetry"], launch_ms, launcher_ms,
                                sample["idle_rss_bytes"]).first_frame_ms
    return request_sample(sample, ui_ready, expected, command, launch_ms, phases, rows)


def optional_stats(samples: list[dict], field: str) -> dict | None:
    values = tuple(sample[field] for sample in samples if sample.get(field) is not None)
    if not values:
        return None
    return {"median_ms": statistics.median(values), "p95_ms": percentile(values, 0.95)}


def frame_stats(samples: list[dict], field: str) -> dict | None:
    """Median, p95, and max of one per-sample frame metric; None when no sample has it."""
    values = tuple(sample[field] for sample in samples if sample.get(field) is not None)
    if not values:
        return None
    return {"samples": len(values), "median": statistics.median(values),
            "p95": percentile(values, 0.95), "max": max(values)}


def summarize_requests(samples: list[dict], cold: bool) -> dict:
    result = {field: optional_stats(samples, field) for field in
              ("request_to_render_ms", "launcher_ms", "content_ready_ms", "render_ms")}
    # Frame work per request (schema 2 GUIs): a count, then CPU milliseconds.
    result.update({field: frame_stats(samples, field) for field in FRAME_STAT_FIELDS})
    frames = tuple(s["first_frame_ms"] for s in samples if s.get("first_frame_ms") is not None)
    result["first_frame_p95_ms"] = percentile(frames, 0.95) if cold and frames else None
    result["first_diff_p95_ms"] = result["request_to_render_ms"]["p95_ms"]
    result["ui_ready_p95_ms"] = result["first_frame_p95_ms"]
    result["requested_content_p95_ms"] = result["first_diff_p95_ms"]
    ack = optional_stats(samples, "launcher_ms")
    result["ack_p95_ms"] = ack["p95_ms"] if ack else None
    result["max_idle_rss_bytes"] = max(s["idle_rss_bytes"] for s in samples)
    result["resources"] = summarize_resources([s.get("resources") for s in samples])
    phase_names = sorted({phase for sample in samples for phase in sample["phases_ms"]})
    result["phases"] = {phase: {"median_ms": statistics.median(values), "p95_ms": percentile(values, .95),
                                "samples": len(values)}
                        for phase in phase_names
                        if (values := tuple(s["phases_ms"][phase] for s in samples if phase in s["phases_ms"]))}
    return result


def fixture_repository(args) -> Path:
    """Path used for marker edits; a Windows driver edits through the WSL share."""
    if args.wsl_distro:
        return Path(subprocess.check_output(
            ["wsl.exe", "--distribution", args.wsl_distro, "--exec", "wslpath", "-w", args.repository],
            text=True, timeout=10).strip())
    args.repository = str(Path(args.repository).resolve())
    return Path(args.repository)


def close_warm_window(report: Path, known_pid: int | None, seed_started_ms: float,
                      timeout: float, windows_from_wsl: bool) -> None:
    """Ask the isolated warm window to close and wait for its process to exit."""
    atomic_control(report, {"schema": 1, "close": True})
    measured_pid = known_pid
    if measured_pid is None:
        try:
            telemetry = read_report(report)
            if (telemetry and type(telemetry.get("schema")) is int and telemetry["schema"] in (1, 2)
                    and type(telemetry.get("pid")) is int and telemetry["pid"] > 0
                    and positive_number(telemetry, "started_unix_ms") >= seed_started_ms - 5):
                measured_pid = telemetry["pid"]
        except (OSError, ValueError):
            pass  # The close control still needs time to be consumed.
    if measured_pid is not None:
        wait_for_exit(measured_pid, timeout, windows_from_wsl)
    else:
        # Give an unreported GUI time to consume its close control.
        time.sleep(CLOSE_GRACE_SECONDS)


def run(args) -> int:
    from contextlib import ExitStack
    samples: list[dict] = []
    failures: list[dict] = []
    warmup = None
    cold = args.case == "cold"
    repository = fixture_repository(args)
    try:
        with ExitStack() as cleanup:
            directory = cleanup.enter_context(tempfile.TemporaryDirectory(
                prefix="fastdiff-launch-profile-", dir=args.report_directory))
            root = Path(directory)
            sampler, sampler_error = open_windows_sampler(args, cleanup)
            args.resource_probe = probe_for_args(args, sampler, sampler_error)
            fixture = cleanup.enter_context(MarkerFixture(repository, args.file))
            report = root / "warm.json"
            runtime = root / "ipc-warm"
            pid = None
            exit_ms = (exit_budget_ms(args.resource_probe) if cold
                       else int((args.runs + 2) * args.timeout * 1000))
            def expected() -> dict:
                sample_id = uuid.uuid4().hex
                marker = "lit6-" + sample_id
                fixture.write(marker)
                return {"schema": 1, "sample_id": sample_id, "repository": args.repository,
                        "file": args.file, "marker": marker}
            if not cold:
                seed_started_ms = time.time_ns() / 1_000_000
                cleanup.callback(lambda: close_warm_window(
                    report, pid, seed_started_ms, args.timeout, args.windows_from_wsl))
                warmup = run_request(args, report, runtime, expected(), exit_ms)
                pid = warmup["pid"]
            for index in range(args.runs):
                request = expected()
                if cold:
                    report, runtime = root / f"cold-{index}.json", root / f"ipc-{index}"
                try:
                    sample = run_request(args, report, runtime, request, exit_ms, pid)
                    if cold:
                        wait_for_exit(sample["pid"], args.timeout, args.windows_from_wsl)
                    samples.append(sample)
                except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
                    failures.append({"run": index + 1, "sample_id": request["sample_id"], "error": str(error)})
                    break
                print(f"{index + 1}/{args.runs}: marker rendered {sample['request_to_render_ms']:.1f} ms", flush=True)
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        failures.append({"run": "setup/cleanup", "error": str(error)})
    identity, identity_error = gui_identity(samples + ([warmup] if warmup else []))
    if identity_error:
        failures.append({"run": "identity", "error": identity_error})
    result = {"schema": RESULT_SCHEMA, "hardware": args.hardware, "platform": platform.platform(),
              "python": sys.version, "software": args.software, "wsl_distro": args.wsl_distro,
              "windows_from_wsl": args.windows_from_wsl, "launcher": args.launcher,
              "repository": args.repository, "file": args.file,
              "environment": harness_identity(args), **identity,
              "case": args.case, "command_kind": args.command_kind, "trace_phases": args.trace_phases,
              "requested_runs": args.runs, "completed_runs": len(samples), "failure": failures or None,
              "samples": samples, "warmup": warmup,
              "summary": summarize_requests(samples, cold) if samples and not failures else None}
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    if failures:
        print(f"Launch profiling incomplete: {failures}", file=sys.stderr)
        return 1
    print(json.dumps(result["summary"], indent=2))
    return 0
