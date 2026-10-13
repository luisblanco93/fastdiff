"""Telemetry validation shared by the benchmark drivers.

Accepts GUI report schema 1 and 2. Schema 2 adds identity-bound readiness
(`ui_ready_ms`, `requested_content_ms`), `build`, and `platform`; its readiness
must be complete and ordered, or the sample is rejected instead of counted.

A report may also carry a top-level `control_poll_ms`: how often the GUI polls its
control file once a sample is published (it polls faster only while one is armed).
Harness waits for the GUI to see a control therefore span several of those
intervals (`CONTROL_POLL_SECONDS`, `CONTROL_WAIT_INTERVALS`).
"""

from __future__ import annotations

import json
import math
import platform
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Iterable

MAX_REPORT_BYTES = 64 * 1024
SUPPORTED_SCHEMAS = (1, 2)
RESULT_SCHEMA = 2
BUILD_TEXT_FIELDS = ("version", "profile")
PLATFORM_FIELDS = ("os", "arch")
FRAME_FIELDS = ("frames", "frame_cpu_total_ms", "max_frame_cpu_ms")
# The GUI polls its control file this often once a sample is published; harness
# waits for it to see a control are sized from these, never from a shorter guess.
CONTROL_POLL_SECONDS = 0.25
CONTROL_WAIT_INTERVALS = 12
# The GUI replaces its report while the harness reads it; Windows reports that as a
# permission or sharing error for a moment. A bounded retry rides it out.
TRANSIENT_READ_ATTEMPTS = 20
TRANSIENT_READ_SLEEP_SECONDS = 0.01
SHARING_VIOLATION_WINERRORS = (32, 33)


@dataclass(frozen=True)
class Sample:
    """One launch measurement; schema-1 fields keep their historical meaning."""
    first_frame_ms: float
    first_diff_ms: float
    launcher_ms: float
    idle_rss_bytes: int
    ack_ms: float | None = None
    ui_ready_ms: float | None = None
    requested_content_ms: float | None = None
    readiness_identity_bound: bool = False
    telemetry_schema: int = 1
    build: dict[str, Any] | None = None
    platform: dict[str, str] | None = None
    resources: dict[str, Any] | None = None
    measured_frames: float | None = None
    frame_cpu_total_ms: float | None = None
    max_frame_cpu_ms: float | None = None
    control_poll_ms: float | None = None


def percentile(values: tuple[float, ...], proportion: float) -> float:
    if not values or not 0 < proportion <= 1:
        raise ValueError("percentile needs samples and a proportion in (0, 1]")
    return sorted(values)[math.ceil(len(values) * proportion) - 1]


def positive_number(report: dict[str, Any], field: str) -> float:
    value = report.get(field)
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ValueError(f"missing or invalid telemetry field: {field}")
    if not math.isfinite(value) or value < 0:
        raise ValueError(f"nonfinite or negative telemetry field: {field}")
    return float(value)


def is_transient_read_error(error: OSError) -> bool:
    """A permission or sharing error the GUI's own report write can cause."""
    return isinstance(error, PermissionError) or (
        getattr(error, "winerror", None) in SHARING_VIOLATION_WINERRORS)


def read_report(path: Path) -> dict[str, Any] | None:
    """The GUI report, or None while it is absent or half written.

    A permission or sharing error is retried a bounded number of times, then raised:
    a report that stays locked fails the run with its real cause.
    """
    for attempt in range(1, TRANSIENT_READ_ATTEMPTS + 1):
        try:
            if path.stat().st_size > MAX_REPORT_BYTES:
                raise ValueError("GUI telemetry exceeded 64 KiB")
            content = json.loads(path.read_text(encoding="utf-8"))
            break
        except (FileNotFoundError, json.JSONDecodeError):
            return None  # The GUI may still be writing its initial report.
        except OSError as error:
            if not is_transient_read_error(error) or attempt == TRANSIENT_READ_ATTEMPTS:
                raise
            time.sleep(TRANSIENT_READ_SLEEP_SECONDS)
    if not isinstance(content, dict):
        raise ValueError("GUI telemetry must be a JSON object")
    return content


def control_poll_ms(report: dict[str, Any]) -> float | None:
    """The GUI's reported control polling interval; None when it does not say."""
    if report.get("control_poll_ms") is None:
        return None
    value = positive_number(report, "control_poll_ms")
    if value <= 0:
        raise ValueError("nonpositive telemetry field: control_poll_ms")
    return value


def reported_schema(report: dict[str, Any]) -> int:
    schema = report.get("schema")
    if type(schema) is not int or schema not in SUPPORTED_SCHEMAS:
        raise ValueError("unsupported GUI telemetry schema")
    return schema


def readiness(report: dict[str, Any], schema: int) -> tuple[float, float, bool]:
    """Return GUI-relative (ui_ready_ms, requested_content_ms, identity_bound)."""
    if schema == 1:
        return (positive_number(report, "first_frame_ms"),
                positive_number(report, "first_diff_ms"), False)
    ui_ready = positive_number(report, "ui_ready_ms")
    content = positive_number(report, "requested_content_ms")
    if content < ui_ready:
        raise ValueError("requested content time precedes UI readiness")
    return ui_ready, content, True


def content_ready(report: dict[str, Any]) -> bool:
    """True once the report holds complete readiness; raise on an unsound report."""
    schema = report.get("schema")
    if type(schema) is int and schema == 2:
        if report.get("requested_content_ms") is not None:
            return True
        if report.get("first_diff_ms") is not None:
            raise ValueError(
                "schema 2 telemetry reported first_diff_ms without requested_content_ms")
        return False
    return report.get("first_diff_ms") is not None


def parse_build(report: dict[str, Any]) -> dict[str, Any] | None:
    """GUI-reported build identity; absent on schema 1, validated when present."""
    build = report.get("build")
    if build is None:
        return None
    if not isinstance(build, dict):
        raise ValueError("invalid telemetry field: build")
    for field in BUILD_TEXT_FIELDS:
        if not isinstance(build.get(field), str) or not build[field]:
            raise ValueError(f"missing or invalid telemetry field: build.{field}")
    protocol = build.get("protocol")
    if type(protocol) is not int or protocol < 0:
        raise ValueError("missing or invalid telemetry field: build.protocol")
    revision = build.get("revision")
    if revision is not None and not isinstance(revision, str):
        raise ValueError("invalid telemetry field: build.revision")
    return {"version": build["version"], "revision": revision,
            "profile": build["profile"], "protocol": protocol}


def parse_platform(report: dict[str, Any]) -> dict[str, str] | None:
    value = report.get("platform")
    if value is None:
        return None
    if not isinstance(value, dict) or any(
            not isinstance(value.get(field), str) or not value[field]
            for field in PLATFORM_FIELDS):
        raise ValueError("invalid telemetry field: platform")
    return {field: value[field] for field in PLATFORM_FIELDS}


def parse_report(
    report: dict[str, Any], launch_unix_ms: float, launcher_ms: float,
    idle_rss_bytes: int, resources: dict[str, Any] | None = None,
) -> Sample:
    schema = reported_schema(report)
    if type(report.get("pid")) is not int:
        raise ValueError("GUI pid must be a positive integer")
    pid = positive_number(report, "pid")
    started_ms = positive_number(report, "started_unix_ms")
    frame_ms, diff_ms, bound = readiness(report, schema)
    if pid <= 0 or not pid.is_integer() or diff_ms < frame_ms:
        raise ValueError("invalid pid or first-diff time preceding first-frame")
    if started_ms < launch_unix_ms - 5:
        raise ValueError("GUI predates launch; close existing viewer windows")
    if idle_rss_bytes <= 0:
        raise ValueError("native idle GUI resident memory was not measured")
    startup_ms = max(0, started_ms - launch_unix_ms)
    ui_ready, requested = startup_ms + frame_ms, startup_ms + diff_ms
    return Sample(
        ui_ready, requested, launcher_ms, idle_rss_bytes,
        ack_ms=launcher_ms, ui_ready_ms=ui_ready, requested_content_ms=requested,
        readiness_identity_bound=bound, telemetry_schema=schema,
        build=parse_build(report), platform=parse_platform(report), resources=resources,
        control_poll_ms=control_poll_ms(report),
        **{field: positive_number(report, field) if report.get(field) is not None else None
           for field in ("measured_frames", "frame_cpu_total_ms", "max_frame_cpu_ms")})


def request_frame_metrics(request: dict[str, Any], schema: int) -> dict[str, float | None]:
    """Per-request frame work. Final once rendered_unix_ms is published (schema 2)."""
    if schema < 2:
        return {field: None for field in FRAME_FIELDS}
    return {field: positive_number(request, field) for field in FRAME_FIELDS}


def request_sample(sample: dict, startup_ui_ready_ms: float | None, expected: dict,
                   command: list[str] | None, launch_ms: float, phases: dict, rows: list) -> dict:
    """Normalize one rendered request into the schema-2 sample layout."""
    latency = sample["request_to_render_ms"]
    return {**sample, "first_frame_ms": startup_ui_ready_ms, "first_diff_ms": latency,
            "ack_ms": sample["launcher_ms"], "ui_ready_ms": startup_ui_ready_ms,
            # The harness marker, not GUI readiness alone, binds this content.
            "requested_content_ms": latency, "readiness_identity_bound": True,
            "sample_id": expected["sample_id"], "trigger": expected.get("trigger", "open"),
            "view": expected.get("view"), "command": command,
            "launch_unix_ms": launch_ms, "phases_ms": phases, "phase_rows": rows}


def harness_identity(args: Any) -> dict[str, Any]:
    """Who measured: software label, interpreter, host, and launcher under test."""
    return {
        "software": getattr(args, "software", None),
        "python": sys.version,
        "python_implementation": platform.python_implementation(),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "launcher": getattr(args, "launcher", None),
        "wsl_distro": getattr(args, "wsl_distro", None),
        "windows_from_wsl": bool(getattr(args, "windows_from_wsl", False)),
    }


def gui_identity(samples: Iterable[dict[str, Any]]) -> tuple[dict[str, Any], str | None]:
    """Collapse per-sample GUI identity; the error is set if it changed mid-run.

    One run must come from one kind of GUI: the same build, the same telemetry
    schema (every sample with or without `build`), and one control poll interval.
    A null `build.revision` is tolerated; a missing or mixed `build` is not.
    """
    builds: list[dict[str, Any]] = []
    platforms: list[dict[str, str]] = []
    schemas: set[int] = set()
    shapes: set[tuple[int, bool]] = set()
    polls: list[float | None] = []
    for sample in samples:
        if sample.get("build") is not None:
            builds = builds + [sample["build"]]
        if sample.get("platform") is not None:
            platforms = platforms + [sample["platform"]]
        schema = sample.get("telemetry_schema")
        if type(schema) is int:
            schemas = schemas | {schema}
            shapes = shapes | {(schema, sample.get("build") is not None)}
            polls = polls + [sample.get("control_poll_ms")]
    error = None
    if any(build != builds[0] for build in builds) or any(
            item != platforms[0] for item in platforms):
        error = "GUI build or platform identity changed during the benchmark"
    elif len(shapes) > 1:
        error = ("mixed GUI telemetry schemas or build identity within one run: "
                 f"{sorted(shapes)} as (schema, has build)")
    elif len({poll for poll in polls if poll is not None}) > 1 or (
            any(poll is None for poll in polls) and any(poll is not None for poll in polls)):
        error = "GUI control poll interval changed or was missing for some samples"
    reported = next((poll for poll in polls if poll is not None), None)
    return ({"gui_build": builds[0] if builds else None,
             "gui_platform": platforms[0] if platforms else None,
             "gui_control_poll_ms": reported,
             "telemetry_schemas": sorted(schemas)}, error)
