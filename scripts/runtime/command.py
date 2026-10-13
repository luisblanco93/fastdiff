#!/usr/bin/env python3
"""Stable dff entry point: launch the app selected by the local receipt."""

import os

_launch_trace_enabled = os.environ.get("FASTDIFF_LAUNCH_TRACE")
if _launch_trace_enabled:
    import time
    _imports_started = time.perf_counter()
    _entry_unix_ms = time.time() * 1000

import argparse
from pathlib import Path
import subprocess
import sys
import tarfile
import zipfile

# Also supports loading the entry point in the offline checks.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import runtime
import lifecycle

if _launch_trace_enabled:
    runtime.record_launch_phase("python_imports", _imports_started, started_unix_ms=_entry_unix_ms)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, prog="dff", add_help=False)
    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--help", "--h", "-h", action="store_true", help="Show this help")
    actions.add_argument("--version", "--v", action="store_true", help="Print installed app version (offline)")
    actions.add_argument("--update", action="store_true", help="Install the latest stable release")
    actions.add_argument("--uninstall", action="store_true", help="Remove this environment's installation; close fastdiff first")
    parser.add_argument("repository", nargs="?", default=None, metavar="repository-or-file")
    parser.add_argument("--file")
    parser.add_argument("--view", choices=["all", "unstaged", "staged", "unpushed"])
    parser.add_argument("--native", action="store_true")
    parser.add_argument("--theme", action="store_true", help="Enable Material icons for this window")
    options = parser.parse_args(argv)
    action = options.help or options.version or options.update or options.uninstall
    if action and (options.repository is not None or options.file is not None
                   or options.view is not None or options.native or options.theme):
        parser.error("Lifecycle flags cannot be combined with a repository or launch options")
    if options.help:
        parser.print_help()
        return 0
    try:
        # Setup stages scripts beneath <data root>/runtimes/<version-id>/scripts.
        root = Path(__file__).absolute().parents[3]
        if options.version:
            print(runtime.installed_version(root))
            return 0
        host = runtime.host_platform()
        if options.update:
            runtime.update(host, root)
            return 0
        if options.uninstall:
            lifecycle.uninstall(host, root)
            return 0
        version = runtime.installed_version(root)
        arguments = []
        if options.file is not None:
            arguments.extend(["--file", options.file])
        if options.view is not None:
            arguments.extend(["--view", options.view])
        if options.native:
            arguments.append("--native")
        if options.theme:
            arguments.append("--theme")
        arguments.extend(["--", options.repository if options.repository is not None else str(Path.cwd())])
        return runtime.launch(version, host, root, arguments)
    except (OSError, ValueError, TypeError, subprocess.CalledProcessError, tarfile.TarError, zipfile.BadZipFile) as error:
        guidance = "Close fastdiff before uninstalling." if options.uninstall else "Rerun the installer to repair an installation."
        print(f"fastdiff: {error}. {guidance}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
