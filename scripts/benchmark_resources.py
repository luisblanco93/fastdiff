"""Idle resource totals for the GUI process, its descendants, and the WSL helper.

Every component is measured or named in `unavailable`; an unmeasured component is
never reported as zero. `gui_only` is the GUI process alone; `total` adds its
descendant processes and, for a Windows GUI backed by WSL, the helper tree.

Rules for `total`:

* All or nothing. `rss_bytes`, `process_count`, and `idle_cpu_percent` are null
  whenever any component is unavailable; a partial sum is never presented as the
  total. What was measured stays visible as `partial_lower_bound`, a diagnostic
  that is never aggregated.
* `complete` is true only when nothing was unavailable and the window was stable:
  no measured process exited inside it. `incomplete_reasons` names every case.
  `summarize_resources` computes the `total` statistics over complete runs only.
* CPU of exited processes. Linux reports `cutime`+`cstime`, the CPU of children a
  process has already waited for, so children that exit inside the window are
  counted when a live process in the tree reaped them. Windows has no such
  counter: the CPU an exited process used after the first reading is not measured
  (named in `incomplete_reasons`), and processes that start and exit between the
  two readings cannot be detected at all. An exited process is credited to its
  nearest live ancestor in the tree; one reparented out of the tree before it
  exited looks the same, so such a reading can come out low or unavailable, and
  it is always flagged as exited.
* The WSL helper is found by `--helper <repository>` among processes that started
  after the probe was created, so a helper the user already had open for the same
  repository is not counted. A helper someone else starts for that repository
  during the run is indistinguishable and would still be counted.
"""

from __future__ import annotations

import math
import os
import re
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Iterable, Sequence

from benchmark_telemetry import percentile

HELPER_FLAG = "--helper"
DEFAULT_PROC_ROOT = Path("/proc")
WSL_TIMEOUT_SECONDS = 20
PROC_READ_BYTES = 8192
WINDOWS_100NS_PER_SECOND = 10_000_000
# Slack between the table read and the harness clock when ordering process starts.
HELPER_AGE_TOLERANCE_SECONDS = 1.0
# CPU differences below this are counter rounding, not work.
CPU_EPSILON_SECONDS = 1e-6
# find + head need no shell; head -v labels every file so one run returns the table
# and /proc/uptime, which turns each start time into an age on the same clock.
WSL_TABLE_COMMAND = [
    "find", "/proc", "-maxdepth", "2",
    "(", "-path", "/proc/uptime", "-o", "-path", "/proc/[0-9]*/stat",
    "-o", "-path", "/proc/[0-9]*/cmdline", ")", "-type", "f",
    "-exec", "head", "-v", "-c", str(PROC_READ_BYTES), "{}", "+",
]
_HEADER = re.compile(rb"^==> /proc/(?:(\d+)/(stat|cmdline)|(uptime)) <==\n", re.MULTILINE)


class ResourceUnavailable(Exception):
    """A component could not be measured; the message becomes its reason."""


@dataclass(frozen=True)
class ProcessSample:
    pid: int
    parent_pid: int
    rss_bytes: int
    cpu_seconds: float
    # CPU of children this process already waited for (Linux cutime+cstime);
    # None where the platform has no such counter (Windows).
    reaped_cpu_seconds: float | None = None
    # Seconds after boot at which the process started (Linux starttime).
    start_seconds: float | None = None


@dataclass(frozen=True)
class LinuxProcess:
    sample: ProcessSample
    args: tuple[str, ...] = ()
    age_seconds: float | None = None  # Time since start, on the table's own clock.


@dataclass(frozen=True)
class TreeReading:
    processes: tuple[ProcessSample, ...]
    unreadable_pids: tuple[int, ...] = ()


@dataclass(frozen=True)
class ComponentReading:
    at: float | None
    tree: TreeReading | None
    reason: str | None = None


@dataclass(frozen=True)
class Reading:
    gui: ComponentReading
    helper: ComponentReading | None


TreeSource = Callable[[int], TreeReading]
HelperSource = Callable[[], TreeReading]
MEASUREMENT_ERRORS = (OSError, RuntimeError, ValueError, subprocess.SubprocessError,
                      ResourceUnavailable)


def parse_stat(text: str, clock_ticks: int, page_size: int) -> ProcessSample | None:
    """Parse /proc/<pid>/stat; None for a zombie or dead process."""
    start, end = text.find("("), text.rfind(")")
    fields = text[end + 1:].split() if start > 0 and end > start else []
    if len(fields) < 22:
        raise ValueError("malformed process stat")
    try:
        pid, parent = int(text[:start]), int(fields[1])
        ticks = int(fields[11]) + int(fields[12])
        reaped_ticks = int(fields[13]) + int(fields[14])
        started_ticks = int(fields[19])
        rss_pages = int(fields[21])
    except ValueError:
        raise ValueError("malformed process stat") from None
    if fields[0] in ("Z", "X"):
        return None
    return ProcessSample(pid, parent, max(0, rss_pages) * page_size, ticks / clock_ticks,
                         max(0, reaped_ticks) / clock_ticks, max(0, started_ticks) / clock_ticks)


def parse_cmdline(raw: bytes) -> tuple[str, ...]:
    return tuple(part.decode("utf-8", "replace") for part in raw.split(b"\0") if part)


def parse_uptime(text: str) -> float:
    """Seconds since boot from /proc/uptime."""
    fields = text.split()
    try:
        value = float(fields[0])
    except (IndexError, ValueError):
        raise ValueError("malformed /proc/uptime") from None
    if not math.isfinite(value) or value < 0:
        raise ValueError("malformed /proc/uptime")
    return value


def _age(uptime: float | None, sample: ProcessSample) -> float | None:
    if uptime is None or sample.start_seconds is None:
        return None
    return max(0.0, uptime - sample.start_seconds)


def read_proc_table(proc_root: Path, clock_ticks: int, page_size: int,
                    with_args: bool = False, uptime: float | None = None
                    ) -> tuple[LinuxProcess, ...]:
    table: list[LinuxProcess] = []
    for entry in proc_root.iterdir():
        if not entry.name.isdigit():
            continue
        try:
            sample = parse_stat((entry / "stat").read_text(encoding="utf-8", errors="replace"),
                                clock_ticks, page_size)
            args = parse_cmdline((entry / "cmdline").read_bytes()) if with_args else ()
        except (FileNotFoundError, ProcessLookupError):
            continue  # The process exited while the table was being read.
        if sample is not None:
            table.append(LinuxProcess(sample, args, _age(uptime, sample)))
    return tuple(table)


def subtree(table: Sequence[LinuxProcess], roots: Iterable[int]) -> tuple[ProcessSample, ...]:
    """The roots present in `table` plus all of their descendants, ordered by pid."""
    by_pid = {entry.sample.pid: entry.sample for entry in table}
    children: dict[int, list[int]] = {}
    for entry in table:
        children.setdefault(entry.sample.parent_pid, []).append(entry.sample.pid)
    found: dict[int, ProcessSample] = {}
    pending = [pid for pid in roots if pid in by_pid]
    while pending:
        pid = pending.pop()
        if pid not in found:
            found[pid] = by_pid[pid]
            pending.extend(children.get(pid, ()))
    return tuple(sorted(found.values(), key=lambda sample: sample.pid))


def is_helper(args: Sequence[str], repository: str) -> bool:
    """True for `<helper> --helper <repository> ...`, the process the GUI spawns."""
    return HELPER_FLAG in args and args.index(HELPER_FLAG) + 1 < len(args) \
        and args[args.index(HELPER_FLAG) + 1] == repository


def linux_tree_source(proc_root: Path = DEFAULT_PROC_ROOT) -> TreeSource:
    def read(pid: int) -> TreeReading:
        table = read_proc_table(proc_root, os.sysconf("SC_CLK_TCK"), os.sysconf("SC_PAGE_SIZE"))
        processes = subtree(table, [pid])
        if not any(sample.pid == pid for sample in processes):
            raise ResourceUnavailable(f"GUI process {pid} is not listed in {proc_root}")
        return TreeReading(processes)
    return read


def local_table_reader(proc_root: Path = DEFAULT_PROC_ROOT) -> Callable[[], tuple[LinuxProcess, ...]]:
    def read() -> tuple[LinuxProcess, ...]:
        uptime = parse_uptime((proc_root / "uptime").read_text(encoding="ascii", errors="replace"))
        return read_proc_table(proc_root, os.sysconf("SC_CLK_TCK"),
                               os.sysconf("SC_PAGE_SIZE"), with_args=True, uptime=uptime)
    return read


def parse_wsl_table(data: bytes, clock_ticks: int, page_size: int) -> tuple[LinuxProcess, ...]:
    stats: dict[int, str] = {}
    cmdlines: dict[int, tuple[str, ...]] = {}
    uptime: float | None = None
    matches = list(_HEADER.finditer(data))
    for index, match in enumerate(matches):
        end = matches[index + 1].start() if index + 1 < len(matches) else len(data)
        body = data[match.end():end].rstrip(b"\n")
        pid, kind, is_uptime = match.groups()
        if is_uptime:
            uptime = parse_uptime(body.decode("ascii", "replace"))
        elif kind == b"stat":
            stats[int(pid)] = body.decode("utf-8", "replace")
        else:
            cmdlines[int(pid)] = parse_cmdline(body)
    table: list[LinuxProcess] = []
    for pid, text in stats.items():
        sample = parse_stat(text, clock_ticks, page_size)
        if sample is not None:
            table.append(LinuxProcess(sample, cmdlines.get(pid, ()), _age(uptime, sample)))
    if not table:
        raise ResourceUnavailable("WSL process table was empty")
    return tuple(table)


def run_wsl(command: list[str]) -> bytes:
    done = subprocess.run(command, capture_output=True, timeout=WSL_TIMEOUT_SECONDS, check=False)
    if not done.stdout:
        detail = done.stderr.decode("utf-8", "replace").strip()[:200]
        raise ResourceUnavailable(f"wsl.exe query produced no output (exit {done.returncode}): {detail}")
    return done.stdout


def wsl_table_reader(distribution: str, run: Callable[[list[str]], bytes] = run_wsl
                     ) -> Callable[[], tuple[LinuxProcess, ...]]:
    prefix = ["wsl.exe", "--distribution", distribution, "--exec"]
    constants: dict[str, int] = {}

    def constant(name: str) -> int:
        text = run(prefix + ["getconf", name]).decode("ascii", "replace").strip()
        if not text.isdigit() or int(text) <= 0:
            raise ResourceUnavailable(f"getconf {name} returned {text!r}")
        return int(text)

    def read() -> tuple[LinuxProcess, ...]:
        if not constants:
            constants.update(ticks=constant("CLK_TCK"), page=constant("PAGESIZE"))
        return parse_wsl_table(run(prefix + WSL_TABLE_COMMAND), constants["ticks"], constants["page"])
    return read


def helper_tree_source(read_table: Callable[[], Sequence[LinuxProcess]], repository: str,
                       elapsed: Callable[[], float] | None = None) -> HelperSource:
    """Helper tree for `repository`; with `elapsed`, only helpers started within it.

    `elapsed` is the harness time since the probe was created. A helper whose age
    exceeds it existed before the benchmark began and is not the GUI's helper.
    """
    def read() -> TreeReading:
        table = read_table()
        candidates = [entry for entry in table if is_helper(entry.args, repository)]
        earlier: list[int] = []
        if elapsed is not None and candidates:
            if any(entry.age_seconds is None for entry in candidates):
                raise ResourceUnavailable(
                    f"start time of the WSL helper for {repository} is unknown; "
                    "it cannot be told apart from an earlier helper")
            limit = elapsed() + HELPER_AGE_TOLERANCE_SECONDS
            earlier = [entry.sample.pid for entry in candidates
                       if entry.age_seconds is not None and entry.age_seconds > limit]
            candidates = [entry for entry in candidates if entry.sample.pid not in earlier]
        if not candidates:
            if earlier:
                raise ResourceUnavailable(
                    f"only WSL helper processes for {repository} that started before the "
                    f"benchmark exist: {sorted(earlier)}")
            raise ResourceUnavailable(f"no WSL helper process for {repository} is running")
        return TreeReading(subtree(table, [entry.sample.pid for entry in candidates]))
    return read


def windows_tree_source(sampler: Any) -> TreeSource:
    """Adapt WindowsSampler.tree() rows to TreeReading."""
    def read(pid: int) -> TreeReading:
        processes: list[ProcessSample] = []
        unreadable: list[int] = []
        for row_pid, parent, working_set, cpu_100ns in sampler.tree(pid):
            if working_set < 0:
                unreadable.append(row_pid)
            else:
                processes.append(ProcessSample(row_pid, parent, working_set,
                                               cpu_100ns / WINDOWS_100NS_PER_SECOND))
        if not any(sample.pid == pid for sample in processes):
            raise ResourceUnavailable(f"Windows process {pid} was not found or is unreadable")
        return TreeReading(tuple(processes), tuple(unreadable))
    return read


def unavailable_tree(reason: str) -> TreeSource:
    def read(_pid: int) -> TreeReading:
        raise ResourceUnavailable(reason)
    return read


def _read_component(source: Callable[[], TreeReading], clock: Callable[[], float]) -> ComponentReading:
    started = clock()
    try:
        tree = source()
    except MEASUREMENT_ERRORS as error:
        return ComponentReading(None, None, str(error) or type(error).__name__)
    return ComponentReading((started + clock()) / 2, tree)


@dataclass(frozen=True)
class CpuWindow:
    percent: float
    started: tuple[int, ...]  # Selected processes absent from the first reading.
    exited: tuple[int, ...]  # Selected processes absent from the second reading.
    uncounted: tuple[int, ...]  # Exited processes whose later CPU nothing reaped.
    exit_cpu_seconds: float  # CPU used inside the window by children that exited.


def _always(_pid: int) -> bool:
    return True


def _never(_pid: int) -> bool:
    return False


def _advance(pid: int, label: str, start: float, end: float) -> float:
    if end < start:
        raise ResourceUnavailable(f"{label} counter of process {pid} went backwards")
    return end - start


def _reaper(gone: ProcessSample, prior: dict[int, ProcessSample],
            current: dict[int, ProcessSample]) -> ProcessSample | None:
    """The live process that received `gone`'s CPU when it was waited for.

    Walks up through ancestors that also exited: a reaped process hands its own
    reaped-children CPU to its parent on the way.
    """
    parent = gone.parent_pid
    for _ in range(len(prior)):
        if parent in current:
            return current[parent]
        if parent not in prior:
            return None
        parent = prior[parent].parent_pid
    return None


def _cpu_window(before: ComponentReading, after: ComponentReading,
                own: Callable[[int], bool], kids: Callable[[int], bool],
                require: int | None = None) -> CpuWindow:
    """CPU used between two readings, as a percentage of the wall time between them.

    `own` selects processes whose own CPU counts; `kids` selects processes whose
    reaped-children CPU counts. A reaped child's whole lifetime CPU lands in its
    parent's counter, so the part already visible in the first reading is removed.
    """
    assert before.tree is not None and after.tree is not None
    assert before.at is not None and after.at is not None
    wall = after.at - before.at
    if wall <= 0:
        raise ResourceUnavailable("idle window has no positive duration")
    prior = {sample.pid: sample for sample in before.tree.processes}
    current = {sample.pid: sample for sample in after.tree.processes}
    if require is not None and (require not in prior or require not in current):
        raise ResourceUnavailable(f"process {require} was not present in both readings")
    skipped = set(before.tree.unreadable_pids)
    consumed = reaped = 0.0
    for pid, now in current.items():
        if pid in skipped:
            continue
        then = prior.get(pid)  # None: the process started inside the window.
        if own(pid):
            consumed += _advance(pid, "CPU", then.cpu_seconds if then else 0.0, now.cpu_seconds)
        if kids(pid) and now.reaped_cpu_seconds is not None:
            gained = _advance(pid, "reaped-children CPU",
                              (then.reaped_cpu_seconds or 0.0) if then else 0.0,
                              now.reaped_cpu_seconds)
            consumed += gained
            reaped += gained
    exited = tuple(sorted(pid for pid in prior if own(pid) and pid not in current))
    uncounted: list[int] = []
    already_seen = 0.0
    for pid in exited:
        gone = prior[pid]
        reaper = _reaper(gone, prior, current)
        if reaper is None or not kids(reaper.pid) or reaper.reaped_cpu_seconds is None:
            uncounted.append(pid)
            continue
        already_seen += gone.cpu_seconds + (gone.reaped_cpu_seconds or 0.0)
    consumed -= already_seen
    exit_cpu = reaped - already_seen
    if consumed < -CPU_EPSILON_SECONDS or exit_cpu < -CPU_EPSILON_SECONDS:
        raise ResourceUnavailable("CPU of exited processes could not be attributed")
    started = tuple(sorted(pid for pid in current
                           if own(pid) and pid not in prior and pid not in skipped))
    return CpuWindow(100.0 * max(0.0, consumed) / wall, started, exited, tuple(uncounted),
                     max(0.0, exit_cpu))


def _exit_reason(window: CpuWindow) -> str | None:
    if not window.exited and window.exit_cpu_seconds <= CPU_EPSILON_SECONDS:
        return None
    reasons: list[str] = []
    if window.exited:
        reasons.append(f"processes exited during the window: {list(window.exited)}")
    if window.exit_cpu_seconds > CPU_EPSILON_SECONDS:
        reasons.append(f"exited children used {window.exit_cpu_seconds:.3f} CPU s, "
                       "counted from reaped-children counters")
    if window.uncounted:
        reasons.append(f"CPU of {list(window.uncounted)} after the first reading is not counted")
    return "; ".join(reasons)


def _both_trees(before: ComponentReading | None, after: ComponentReading | None
                ) -> tuple[bool, str | None]:
    if before is None or after is None:
        return False, "component was not read"
    if before.tree is None or after.tree is None:
        return False, before.reason or after.reason or "component was not read"
    return True, None


@dataclass(frozen=True)
class Parts:
    """What one component contributes to the total."""
    cpu: dict[str, float]
    rss_bytes: int
    process_count: int
    unavailable: dict[str, str]  # Part name -> why it could not be measured.
    incomplete: dict[str, str]  # Part name -> why its window was not stable.
    notes: tuple[str, ...] = ()


def _window_parts(name: str, window: CpuWindow) -> tuple[dict[str, str], tuple[str, ...]]:
    reason = _exit_reason(window)
    notes = (f"{name} processes started during the window: {list(window.started)}",) \
        if window.started else ()
    return ({name: reason} if reason else {}), notes


def _gui_parts(before: ComponentReading, after: ComponentReading, pid: int) -> Parts:
    """GUI root CPU, then every other process of its tree."""
    usable, reason = _both_trees(before, after)
    if not usable or after.tree is None:
        why = reason or "unavailable"
        return Parts({}, 0, 0, {"gui_cpu": why, "gui_descendants": why}, {})
    cpu: dict[str, float] = {}
    unavailable: dict[str, str] = {}
    incomplete: dict[str, str] = {}
    notes: tuple[str, ...] = ()
    try:
        cpu["gui_cpu"] = _cpu_window(before, after, lambda p: p == pid, _never, require=pid).percent
    except ResourceUnavailable as error:
        unavailable["gui_cpu"] = str(error)
    try:
        window = _cpu_window(before, after, lambda p: p != pid, _always)
        cpu["gui_descendants"] = window.percent
        incomplete, notes = _window_parts("gui_descendants", window)
    except ResourceUnavailable as error:
        unavailable["gui_descendants"] = str(error)
    others = [s for s in after.tree.processes if s.pid != pid]
    if after.tree.unreadable_pids:
        unavailable["gui_descendants"] = f"unreadable processes: {list(after.tree.unreadable_pids)}"
    cpu = {name: value for name, value in cpu.items() if name not in unavailable}
    return Parts(cpu, sum(s.rss_bytes for s in others), len(others), unavailable, incomplete, notes)


def _helper_parts(before: ComponentReading | None, after: ComponentReading | None) -> Parts:
    """The WSL helper tree, including its Git children."""
    usable, reason = _both_trees(before, after)
    if not usable or before is None or after is None or after.tree is None:
        return Parts({}, 0, 0, {"wsl_helper": reason or "unavailable"}, {})
    processes = after.tree.processes
    try:
        window = _cpu_window(before, after, _always, _always)
    except ResourceUnavailable as error:
        return Parts({}, sum(s.rss_bytes for s in processes), len(processes),
                     {"wsl_helper": str(error)}, {})
    incomplete, notes = _window_parts("wsl_helper", window)
    return Parts({"wsl_helper": window.percent}, sum(s.rss_bytes for s in processes),
                 len(processes), {}, incomplete, notes)


def aggregate(before: Reading, after: Reading, pid: int, gui_rss_bytes: int) -> dict[str, Any]:
    """Combine two readings into the gui_only and total blocks."""
    parts = [_gui_parts(before.gui, after.gui, pid)]
    components = ["gui", "gui_descendants"]
    if before.helper is not None or after.helper is not None:
        components.append("wsl_helper")
        parts.append(_helper_parts(before.helper, after.helper))
    cpu = {name: value for part in parts for name, value in part.cpu.items()}
    unavailable = {name: why for part in parts for name, why in part.unavailable.items()}
    incomplete = {name: why for part in parts for name, why in part.incomplete.items()}
    rss_bytes = gui_rss_bytes + sum(part.rss_bytes for part in parts)
    process_count = 1 + sum(part.process_count for part in parts)
    cpu_percent = sum(cpu.values()) if cpu else None
    gui_only = {
        "rss_bytes": gui_rss_bytes, "idle_cpu_percent": cpu.get("gui_cpu"), "process_count": 1,
        "components": ["gui"], "unavailable": ["gui_cpu"] if "gui_cpu" in unavailable else [],
        "unavailable_reasons": {k: v for k, v in unavailable.items() if k == "gui_cpu"},
    }
    whole = not unavailable
    total: dict[str, Any] = {
        "rss_bytes": rss_bytes if whole else None,
        "idle_cpu_percent": cpu_percent if whole else None,
        "process_count": process_count if whole else None,
        "components": components,
        "unavailable": sorted(unavailable), "unavailable_reasons": dict(sorted(unavailable.items())),
        "incomplete_reasons": dict(sorted(incomplete.items())),
        "complete": whole and not incomplete,
        "notes": [note for part in parts for note in part.notes],
    }
    if not whole:
        total["partial_lower_bound"] = {"rss_bytes": rss_bytes, "process_count": process_count,
                                        "idle_cpu_percent": cpu_percent}
    return {"gui_only": gui_only, "total": total}


class ResourceProbe:
    """Two readings around the idle window, aggregated into resource totals."""

    def __init__(self, gui_tree: TreeSource, helper_tree: HelperSource | None = None,
                 clock: Callable[[], float] = time.monotonic) -> None:
        self.gui_tree = gui_tree
        self.helper_tree = helper_tree
        self.clock = clock

    def begin(self, pid: int) -> Reading:
        gui = _read_component(lambda: self.gui_tree(pid), self.clock)
        helper = _read_component(self.helper_tree, self.clock) if self.helper_tree else None
        return Reading(gui, helper)

    def finish(self, before: Reading, pid: int, gui_rss_bytes: int) -> dict[str, Any]:
        return aggregate(before, self.begin(pid), pid, gui_rss_bytes)


def probe_for(*, windows_gui: bool, repository: str, sampler: Any | None = None,
              sampler_error: str | None = None, wsl_distro: str | None = None,
              windows_from_wsl: bool = False, proc_root: Path = DEFAULT_PROC_ROOT,
              wsl_run: Callable[[list[str]], bytes] = run_wsl,
              clock: Callable[[], float] = time.monotonic) -> ResourceProbe:
    """Choose sources: Windows tree via the persistent sampler, Linux via /proc.

    The probe is created before the harness launches anything, so a helper older
    than the probe is not the one the GUI spawned.
    """
    if windows_gui:
        gui = (windows_tree_source(sampler) if sampler is not None else
               unavailable_tree(sampler_error or "Windows process sampler is not running"))
    else:
        gui = linux_tree_source(proc_root)
    created = clock()

    def elapsed() -> float:
        return clock() - created

    helper: HelperSource | None = None
    if wsl_distro:
        helper = helper_tree_source(wsl_table_reader(wsl_distro, wsl_run), repository, elapsed)
    elif windows_from_wsl:
        helper = helper_tree_source(local_table_reader(proc_root), repository, elapsed)
    return ResourceProbe(gui, helper, clock)


def _stats(parts: Sequence[dict[str, Any]]) -> dict[str, Any]:
    """Maxima and p95 over resource blocks; a null field is skipped, never zero."""
    cpu = tuple(part["idle_cpu_percent"] for part in parts if part["idle_cpu_percent"] is not None)
    rss = [part["rss_bytes"] for part in parts if part["rss_bytes"] is not None]
    count = [part["process_count"] for part in parts if part["process_count"] is not None]
    return {"max_rss_bytes": max(rss) if rss else None,
            "max_process_count": max(count) if count else None,
            "p95_idle_cpu_percent": percentile(cpu, 0.95) if cpu else None,
            "cpu_measured_runs": len(cpu),
            "unavailable": sorted({item for part in parts for item in part["unavailable"]})}


def summarize_resources(blocks: Sequence[dict[str, Any] | None]) -> dict[str, Any]:
    """Distribution of per-run resource blocks; unmeasured runs stay visible.

    `gui_only` covers every measured run. `total` covers complete runs only, so one
    run that lost a component or saw processes exit cannot lower a maximum or p95;
    with no complete run its statistics are null.
    """
    measured = [block for block in blocks if block]
    summary: dict[str, Any] = {"runs": len(blocks), "measured_runs": len(measured)}
    if not measured:
        return {**summary, "unavailable": ["resources"]}
    totals = [block["total"] for block in measured]
    complete = [total for total in totals if total["complete"]]
    return {**summary,
            "gui_only": _stats([block["gui_only"] for block in measured]),
            "total": {**_stats(complete),
                      "complete_runs": len(complete), "measured_runs": len(measured),
                      # Everything that went missing or unstable, whether or not the run counted.
                      "unavailable": sorted({item for total in totals
                                             for item in total["unavailable"]}),
                      "incomplete": sorted({item for total in totals
                                            for item in total.get("incomplete_reasons", {})})}}
