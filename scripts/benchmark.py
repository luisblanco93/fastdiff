#!/usr/bin/env python3
"""Measure real native GUI startup using opt-in telemetry, never CLI return alone."""

from __future__ import annotations

import argparse
import json
import math
import os
import platform
import subprocess
import sys
import tempfile
import time
from dataclasses import asdict
from pathlib import Path
from typing import Any

from benchmark_resources import ResourceProbe, probe_for, summarize_resources
from benchmark_telemetry import (MAX_REPORT_BYTES, RESULT_SCHEMA, Sample, content_ready,
                                 gui_identity, harness_identity, parse_report, percentile,
                                 positive_number, read_report)
from benchmark_windows import WindowsSampler, windows_process_info

POLL_SECONDS = 0.025
IDLE_SECONDS = 1.0
EXIT_MS = 4000
# Resource readings wrap the idle window; keep the GUI alive for both of them.
PROBE_EXIT_MARGIN_MS = 3000
MIN_RUNS = 30


def wsl_command(
    distro: str, launcher: str, repository: str, filename: str,
    report: str, exit_ms: int, windows_exe: str | None,
) -> list[str]:
    assignments = [f"FASTDIFF_BENCH_REPORT={report}",
                   f"FASTDIFF_BENCH_EXIT_MS={exit_ms}"]
    if windows_exe:
        assignments = assignments + [f"FASTDIFF_WINDOWS_EXE={windows_exe}"]
    return ["wsl.exe", "--distribution", distro, "--exec", "env",
            *assignments, launcher, repository, "--file", filename]


_WINDOWS_SAMPLER: WindowsSampler | None = None


def wsl_windows_process_info(pid: int) -> tuple[bool, int]:
    if _WINDOWS_SAMPLER is None:
        raise RuntimeError("Windows memory sampler was not initialized")
    return _WINDOWS_SAMPLER.query(pid)


def process_info(pid: int, windows_from_wsl: bool = False) -> tuple[bool, int]:
    if windows_from_wsl:
        return wsl_windows_process_info(pid)
    if os.name == "nt":
        return windows_process_info(pid)
    try:
        status = Path(f"/proc/{pid}/status").read_text(encoding="ascii")
    except (FileNotFoundError, ProcessLookupError):
        # A process that exits while its status is read reports ESRCH, not ENOENT.
        return False, 0
    fields = dict(line.split(":", 1) for line in status.splitlines() if ":" in line)
    if fields.get("State", "").strip().startswith("Z"):
        return False, 0
    return True, int(fields.get("VmRSS", "0 kB").split()[0]) * 1024


def exit_budget_ms(probe: ResourceProbe | None) -> int:
    return EXIT_MS + (PROBE_EXIT_MARGIN_MS if probe is not None else 0)


def launch_arguments(args: argparse.Namespace) -> list[str]:
    """A repository review of `--file`, or with `--document` that file opened on its own."""
    if getattr(args, "document", False):
        return [args.launcher, args.file]
    return [args.launcher, args.repository, "--file", args.file]


def launch_sample(args: argparse.Namespace, report: Path,
                  probe: ResourceProbe | None = None) -> Sample:
    exit_ms = exit_budget_ms(probe)
    env = dict(os.environ, FASTDIFF_BENCH_REPORT=str(report),
               FASTDIFF_BENCH_EXIT_MS=str(exit_ms))
    command = launch_arguments(args)
    if os.name != "nt" and not args.windows_from_wsl:
        command = command + ["--native"]
    if args.wsl_distro:
        linux_report = subprocess.check_output(
            ["wsl.exe", "--distribution", args.wsl_distro, "--exec", "wslpath",
             "-u", str(report)], text=True, timeout=10,
        ).strip()
        command = wsl_command(args.wsl_distro, args.launcher, args.repository,
                              args.file, linux_report, exit_ms,
                              env.get("FASTDIFF_WINDOWS_EXE"))
    launched_unix_ms = time.time_ns() / 1_000_000
    started = time.monotonic()
    # A GUI may inherit this handle after the launcher exits. Waiting for a pipe's
    # EOF would measure GUI lifetime instead of launch acceptance.
    with tempfile.TemporaryFile() as diagnostics:
        launcher = subprocess.Popen(command, env=env, stdout=subprocess.DEVNULL,
                                    stderr=diagnostics)
        try:
            launcher.wait(timeout=args.timeout)
            launcher_ms = (time.monotonic() - started) * 1000
            if launcher.returncode:
                diagnostics.seek(0)
                raise RuntimeError(diagnostics.read(MAX_REPORT_BYTES).decode(errors="replace").strip())
            return await_gui(report, launched_unix_ms, launcher_ms,
                             started + args.timeout, args.windows_from_wsl, probe)
        finally:
            if launcher.poll() is None:
                launcher.kill()
                launcher.wait()
            # The GUI owns its automatic exit timer. Never terminate a process
            # using a PID supplied by a telemetry file, even when that file is invalid.


def await_gui(report: Path, launch_ms: float, launcher_ms: float,
              deadline: float, windows_from_wsl: bool = False,
              probe: ResourceProbe | None = None) -> Sample:
    while time.monotonic() < deadline:
        telemetry = read_report(report)
        if telemetry and content_ready(telemetry):
            # Validate all telemetry before querying any native PID.
            parse_report(telemetry, launch_ms, launcher_ms, 1)
            pid = int(positive_number(telemetry, "pid"))
            before = probe.begin(pid) if probe is not None else None
            time.sleep(IDLE_SECONDS)
            alive, rss = process_info(pid, windows_from_wsl)
            if not alive:
                raise RuntimeError("GUI exited before idle memory could be measured")
            resources = (probe.finish(before, pid, rss)
                         if probe is not None and before is not None else None)
            sample = parse_report(telemetry, launch_ms, launcher_ms, rss, resources)
            while time.monotonic() < deadline:
                if not process_info(pid, windows_from_wsl)[0]:
                    return sample
                time.sleep(POLL_SECONDS)
            raise RuntimeError("GUI did not exit after benchmark deadline")
        time.sleep(POLL_SECONDS)
    raise RuntimeError("no complete GUI first-frame/first-diff telemetry")


def optional_p95(values: tuple[float | None, ...]) -> float | None:
    present = tuple(value for value in values if value is not None)
    return percentile(present, 0.95) if present else None


def summarize(samples: tuple[Sample, ...], windows: bool) -> dict[str, Any]:
    frame = percentile(tuple(s.first_frame_ms for s in samples), 0.95)
    diff = percentile(tuple(s.first_diff_ms for s in samples), 0.95)
    rss = max(s.idle_rss_bytes for s in samples)
    return {"first_frame_p95_ms": frame, "first_diff_p95_ms": diff,
            "ui_ready_p95_ms": optional_p95(tuple(s.ui_ready_ms for s in samples)),
            "requested_content_p95_ms":
                optional_p95(tuple(s.requested_content_ms for s in samples)),
            "ack_p95_ms": optional_p95(tuple(s.ack_ms for s in samples)),
            "readiness_identity_bound": all(s.readiness_identity_bound for s in samples),
            "max_idle_rss_bytes": rss,
            "resources": summarize_resources([s.resources for s in samples]),
            "targets_met": {"first_frame": frame <= (1000 if windows else 500),
                            "first_diff": diff <= (1500 if windows else 1000),
                            "idle_rss": rss <= 150 * 1024 * 1024}}


def open_windows_sampler(args: argparse.Namespace, cleanup: Any
                         ) -> tuple[WindowsSampler | None, str | None]:
    """Start the persistent Windows sampler used for RSS and process trees.

    A WSL-driven run needs it for RSS, so a startup failure fails the run. A
    native Windows run already has RSS; only the tree measurement is lost.
    """
    global _WINDOWS_SAMPLER
    if args.windows_from_wsl:
        sampler = WindowsSampler()
    elif os.name == "nt":
        try:
            sampler = WindowsSampler()
        except (OSError, RuntimeError) as error:
            return None, f"Windows process sampler unavailable: {error}"
    else:
        return None, None
    _WINDOWS_SAMPLER = sampler
    cleanup.callback(sampler.close)
    return sampler, None


def probe_for_args(args: argparse.Namespace, sampler: WindowsSampler | None,
                   sampler_error: str | None) -> ResourceProbe:
    return probe_for(windows_gui=args.windows_from_wsl or os.name == "nt",
                     repository=args.repository, sampler=sampler,
                     sampler_error=sampler_error, wsl_distro=args.wsl_distro,
                     windows_from_wsl=args.windows_from_wsl)


def arguments() -> argparse.Namespace:
    from benchmark_scenarios import DEFAULT_VIEWS, SCENARIOS, parse_views
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--launcher", required=True)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--file", required=True)
    parser.add_argument("--hardware", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--wsl-distro")
    parser.add_argument("--windows-from-wsl", action="store_true")
    parser.add_argument("--report-directory", type=Path)
    parser.add_argument("--runs", type=int, default=MIN_RUNS)
    parser.add_argument("--timeout", type=float, default=15)
    parser.add_argument("--document", action="store_true",
                        help="Open --file as a standalone document (absolute path) instead of "
                             "as a file of --repository; --repository then only labels the run")
    parser.add_argument("--launch-profile", action="store_true",
                        help="Profile a marked scratch file; temporarily replaces its marker")
    parser.add_argument("--case", choices=["cold", "warm"], default="cold")
    parser.add_argument("--command-kind", choices=["direct", "managed"], default="direct")
    parser.add_argument("--trace-phases", action="store_true")
    parser.add_argument("--software", help="Matching binary revision/profile and tool/platform "
                                           "versions; recorded in the result, required for "
                                           "--launch-profile and --scenario")
    parser.add_argument("--scenario", choices=SCENARIOS,
                        help="Step-labelled end-to-end scenario on the marked scratch file: "
                             "open (app closed), reopen (app open), navigate_aba (file A, B, A; "
                             "needs --second-file), switch_view (uses --views), "
                             "save_update (edit the open file). Files are edited with a fresh "
                             "marker before each request, so they are changed when requested")
    parser.add_argument("--second-file", help="Second marked scratch file for navigate_aba")
    parser.add_argument("--views", default=",".join(DEFAULT_VIEWS),
                        help="Two comma-separated views for switch_view; each must show the "
                             "marked change (default: %(default)s)")
    args = parser.parse_args()
    profiling = args.launch_profile or args.scenario is not None
    if profiling and not args.software:
        parser.error("--launch-profile and --scenario require --software to identify the measured builds")
    if args.document and (profiling or args.wsl_distro or args.windows_from_wsl):
        parser.error("--document only applies to the plain native launch benchmark")
    if args.document and not Path(args.file).is_absolute():
        parser.error("--document needs an absolute --file path")
    if args.scenario and args.launch_profile:
        parser.error("--scenario already profiles the marked file; omit --launch-profile")
    if args.scenario and args.case != "cold":
        parser.error("--case is defined by --scenario (open is cold, the others are warm)")
    if not profiling and (args.case != "cold" or args.command_kind != "direct" or args.trace_phases):
        parser.error("warm/managed/phase measurements require --launch-profile or --scenario")
    if args.scenario == "navigate_aba" and not args.second_file:
        parser.error("--scenario navigate_aba requires --second-file")
    if args.second_file and args.scenario != "navigate_aba":
        parser.error("--second-file is only used by --scenario navigate_aba")
    if args.second_file and args.second_file == args.file:
        parser.error("--second-file must differ from --file")
    if args.views != ",".join(DEFAULT_VIEWS) and args.scenario != "switch_view":
        parser.error("--views is only used by --scenario switch_view")
    try:
        args.views = parse_views(args.views)
    except ValueError as error:
        parser.error(str(error))
    if not args.output.parent.is_dir():
        parser.error("output parent directory must exist")
    if args.runs < MIN_RUNS or not math.isfinite(args.timeout) or args.timeout <= 5:
        parser.error(f"use at least {MIN_RUNS} runs and a finite timeout above 5 seconds")
    if args.wsl_distro and os.name != "nt":
        parser.error("run the WSL benchmark driver on Windows to measure native RSS")
    if args.windows_from_wsl and (os.name == "nt" or not os.environ.get("WSL_DISTRO_NAME")):
        parser.error("--windows-from-wsl requires a Linux shell inside WSL")
    if args.windows_from_wsl and not args.report_directory:
        parser.error("--windows-from-wsl requires a Windows-accessible --report-directory")
    if os.name != "nt" and platform.system() != "Linux":
        parser.error("native RSS sampling supports Linux or Windows")
    return args


def main() -> int:
    args = arguments()
    if args.scenario:
        from benchmark_scenarios import run_scenario
        return run_scenario(args)
    if args.launch_profile:
        from launch_profile import run
        return run(args)
    samples: tuple[Sample, ...] = ()
    failure: str | None = None
    from contextlib import ExitStack
    with ExitStack() as cleanup, tempfile.TemporaryDirectory(
        prefix="fastdiff-benchmark-", dir=args.report_directory,
    ) as directory:
        sampler, sampler_error = open_windows_sampler(args, cleanup)
        probe = probe_for_args(args, sampler, sampler_error)
        for index in range(args.runs):
            try:
                sample = launch_sample(args, Path(directory) / f"{index}.json", probe)
            except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
                failure = f"run {index + 1}: {error}"
                break
            samples = samples + (sample,)
            print(f"{index + 1}/{args.runs}: frame {sample.first_frame_ms:.1f} ms, "
                  f"diff {sample.first_diff_ms:.1f} ms", flush=True)
    rows = [asdict(sample) for sample in samples]
    identity, identity_error = gui_identity(rows)
    failure = failure or identity_error
    result = {"schema": RESULT_SCHEMA, "hardware": args.hardware,
              "platform": platform.platform(), "python": sys.version,
              "software": args.software, "wsl_distro": args.wsl_distro,
              "launcher": args.launcher, "windows_from_wsl": args.windows_from_wsl,
              "repository": args.repository, "file": args.file,
              "environment": harness_identity(args), **identity,
              "requested_runs": args.runs, "completed_runs": len(samples),
              "failure": failure, "samples": rows,
              "summary": summarize(samples, os.name == "nt" or args.windows_from_wsl)
              if samples and not failure else None}
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    if failure:
        print(f"Benchmark failed: {failure}", file=sys.stderr)
        return 1
    print(json.dumps(result["summary"], indent=2))
    return 0 if all(result["summary"]["targets_met"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
