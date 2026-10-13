#!/usr/bin/env python3
"""Opt-in generator for deterministic Git repositories used by benchmark runs.

Nothing here runs during tests or benchmarks unless invoked. Destinations are
validated before any write: never inside the fastdiff checkout or another Git
work tree, never a non-empty directory. Git runs without a shell, with identity
supplied through -c, system/global config disabled, and no network use. No GIT_*
variable is inherited from the caller (GIT_DIR, GIT_WORK_TREE, GIT_INDEX_FILE and
the like would redirect the fixture into another repository).

Every target file is left modified against the baseline commit, never committed
clean: a GUI repository request for an unchanged file falls back to the full
document view and completes no measurement. The marker files carry the
FASTDIFF_LAUNCH_MARKER first line that launch profiling replaces; a shape with
`marked_changed_files` puts the same single first-line marker on its leading
generated files too, so a large generated file is itself a valid `--file`.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Mapping

CHECKOUT_ROOT = Path(__file__).resolve().parents[1]
MARKER = "FASTDIFF_LAUNCH_MARKER"
MARKER_FILES = ("marker-a.txt", "marker-b.txt")
MARKER_ROWS = 20
CHANGE_STRIDE = 10  # Every tenth row of a changed file differs from the baseline.
FILES_PER_DIRECTORY = 100
GIT_IDENTITY = ["-c", "user.name=fastdiff Benchmark", "-c", "user.email=benchmark@fastdiff.invalid",
                "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"]


@dataclass(frozen=True)
class Shape:
    changed_files: int
    rows_per_file: int
    marked_changed_files: int = 0  # Leading generated files that carry the marker.


PRESETS = {
    "small": Shape(2, 20),
    "files-1k": Shape(1_000, 20),
    "files-10k": Shape(10_000, 20),
    # The one large file is the profiling target, so it must accept a marker.
    "rows-100k": Shape(1, 100_000, marked_changed_files=1),
}


@dataclass(frozen=True)
class Fixture:
    repository: str
    marker_files: tuple[str, ...]  # Every modified file whose first line is the marker.
    changed_files: int  # Generated files, including any that are marked.
    total_changed_files: int  # Everything modified in the working tree, counted once.
    rows_per_file: int


def check_destination(destination: Path, checkout_root: Path = CHECKOUT_ROOT) -> Path:
    """Return the resolved destination or raise before anything is created."""
    resolved = destination.expanduser().resolve()
    checkout = checkout_root.resolve()
    if resolved == checkout or resolved.is_relative_to(checkout):
        raise ValueError(f"refusing destination inside the fastdiff checkout: {resolved}")
    for ancestor in (resolved, *resolved.parents):
        if (ancestor / ".git").exists():
            raise ValueError(f"refusing destination inside a Git work tree: {ancestor}")
    if resolved.exists() and (not resolved.is_dir() or any(resolved.iterdir())):
        raise ValueError(f"refusing non-empty or non-directory destination: {resolved}")
    return resolved


def git_environment(base: Mapping[str, str] | None = None) -> dict[str, str]:
    """The caller's environment without any GIT_* variable, plus the settings we need.

    Variables are matched case-insensitively (Windows environments are).
    """
    inherited = os.environ if base is None else base
    clean = {name: value for name, value in inherited.items()
             if not name.upper().startswith("GIT_")}
    return {**clean, "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_TERMINAL_PROMPT": "0"}


def run_git(repository: Path, *arguments: str) -> None:
    command = ["git", *GIT_IDENTITY, *arguments]
    env = git_environment()
    try:
        subprocess.run(command, cwd=repository, env=env, check=True,
                       capture_output=True, text=True, timeout=600)
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f"git {arguments[0]} failed: {error.stderr.strip()}") from None


def baseline_text(index: int, rows: int) -> str:
    return "".join(f"file {index} row {row}\n" for row in range(rows))


def changed_text(index: int, rows: int, marked: bool = False) -> str:
    """Working-tree content; a marked file's first line is the marker, once."""
    text = "".join(
        f"file {index} row {row} changed\n" if row % CHANGE_STRIDE == 0 else f"file {index} row {row}\n"
        for row in range(rows))
    if not marked:
        return text
    first_line_end = text.index("\n") + 1  # Row 0 is replaced; baseline row 0 differs.
    return f"{MARKER}\n" + text[first_line_end:]


def marker_text(name: str) -> str:
    """Working-tree content: first line is the marker to be replaced."""
    return f"{MARKER}\n" + "".join(f"{name} row {row}\n" for row in range(1, MARKER_ROWS))


def marker_baseline_text(name: str) -> str:
    """Committed content: differs from the working tree and carries no marker."""
    return f"{name} baseline\n" + "".join(f"{name} row {row}\n" for row in range(1, MARKER_ROWS))


def changed_path(index: int) -> str:
    return f"src/d{index // FILES_PER_DIRECTORY:04d}/file-{index:06d}.txt"


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(text.encode("utf-8"))


def generate(shape: Shape, destination: Path, checkout_root: Path = CHECKOUT_ROOT,
             marker_files: tuple[str, ...] = MARKER_FILES) -> Fixture:
    """Create a baseline commit, then leave `shape.changed_files` files modified."""
    if shape.changed_files < 1 or shape.rows_per_file < 1:
        raise ValueError("a fixture needs at least one changed file with one row")
    if not 0 <= shape.marked_changed_files <= shape.changed_files:
        raise ValueError("marked_changed_files must be between 0 and changed_files")
    root = check_destination(destination, checkout_root)
    root.mkdir(parents=True, exist_ok=True)
    for name in marker_files:
        write_text(root / name, marker_baseline_text(name))
    for index in range(shape.changed_files):
        write_text(root / changed_path(index), baseline_text(index, shape.rows_per_file))
    run_git(root, "init", "-q")
    run_git(root, "add", "-A")
    run_git(root, "commit", "-q", "-m", "baseline")
    for name in marker_files:
        write_text(root / name, marker_text(name))
    for index in range(shape.changed_files):
        write_text(root / changed_path(index), changed_text(
            index, shape.rows_per_file, marked=index < shape.marked_changed_files))
    marked = tuple(changed_path(index) for index in range(shape.marked_changed_files))
    return Fixture(str(root), (*marker_files, *marked), shape.changed_files,
                   shape.changed_files + len(marker_files), shape.rows_per_file)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=sorted(PRESETS), required=True)
    parser.add_argument("--destination", type=Path, required=True,
                        help="New or empty directory outside the checkout")
    args = parser.parse_args()
    try:
        fixture = generate(PRESETS[args.kind], args.destination)
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"Fixture not created: {error}", file=sys.stderr)
        return 1
    print(json.dumps(asdict(fixture), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
