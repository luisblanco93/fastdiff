#!/usr/bin/env python3
"""Install the latest compiled fastdiff release and the terminal dff command.

A previous diffcode installation is removed and its dff command is taken over.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from urllib.request import Request, urlopen

REPOSITORY = "luisblanco93/fastdiff"
LEGACY_ROOT_NAME = "diffcode"
CORE_FILES = (
    "scripts/runtime/runtime.py",
    "scripts/runtime/command.py",
    "scripts/runtime/lifecycle.py",
)


def source_revision() -> str:
    request = Request(f"https://api.github.com/repos/{REPOSITORY}/commits/main",
                      headers={"User-Agent": "fastdiff-installer"})
    with urlopen(request, timeout=60) as response:
        metadata = json.load(response)
    revision = metadata.get("sha") if isinstance(metadata, dict) else None
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("main has no valid source revision")
    return revision


def download_file(url: str, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    request = Request(url, headers={"User-Agent": "fastdiff-installer"})
    with urlopen(request, timeout=60) as source, destination.open("xb") as output:
        shutil.copyfileobj(source, output)


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def install(revision: str, root: Path | None) -> None:
    with tempfile.TemporaryDirectory(prefix="fastdiff-install-") as temporary:
        stage = Path(temporary)
        for relative in CORE_FILES:
            download_file(f"https://raw.githubusercontent.com/{REPOSITORY}/{revision}/{relative}",
                          stage / relative)
        runtime = load_module("fastdiff_installer_runtime", stage / "scripts/runtime/runtime.py")
        platform = runtime.host_platform()
        destination = root if root is not None else runtime.data_root(platform)
        runtime.setup(runtime.latest_version(), platform, destination)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--revision", help=argparse.SUPPRESS)
    parser.add_argument("--root", type=Path, help=argparse.SUPPRESS)
    options = parser.parse_args(argv)
    try:
        if sys.version_info < (3, 11):
            raise ValueError("fastdiff requires Python 3.11 or newer")
        root = options.root
        if root is not None and root.is_absolute() and root.name == LEGACY_ROOT_NAME:
            root = root.with_name("fastdiff")  # a pre-rename `dff --update` passes its own root
        if root is not None and (not root.is_absolute() or root.name != "fastdiff"):
            raise ValueError("Installer requires an absolute fastdiff data root")
        revision = options.revision if options.revision is not None else source_revision()
        if not re.fullmatch(r"[0-9a-f]{40}", revision):
            raise ValueError("Installer requires a full source commit")
        install(revision, root)
        return 0
    except (OSError, ValueError, TypeError, subprocess.SubprocessError,
            tarfile.TarError, zipfile.BadZipFile) as error:
        print(f"fastdiff: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
