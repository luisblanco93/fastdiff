"""Remove only the managed installation in the current environment."""

from pathlib import Path
import os
import re
import shutil
import sys

import runtime

VERSION = r"\d+\.\d+\.\d+(?:-[\w.-]+)?"


def check_path(path: Path) -> None:
    """Reject redirected paths, including junctions on Python 3.11."""
    runtime.check_path(path)


def check_ancestors(path: Path) -> None:
    runtime.check_ancestors(path)


def check_tree(directory: Path) -> None:
    runtime.check_tree(directory)


def managed_directories(root: Path, host: str) -> list[Path]:
    target = runtime.host_target(host)
    directories = []
    for group in ("versions", "runtimes"):
        parent = root / group
        if not parent.exists() and not parent.is_symlink():
            continue
        check_path(parent)
        for directory in parent.iterdir():
            pattern = rf"({VERSION})-{target}-[0-9a-f]{{32}}" if group == "versions" else rf"({VERSION})-[0-9a-f]{{32}}"
            match = re.fullmatch(pattern, directory.name)
            if not match:
                continue
            check_tree(directory)
            version = match.group(1)
            if group == "versions":
                runtime.validate_package(directory, version, target)
            else:
                metadata_path = directory / "runtime.json"
                if not metadata_path.exists():
                    # Older installations used plugin metadata; read it only for removal.
                    metadata_path = directory / ".codex-plugin" / "plugin.json"
                metadata = runtime.read_json(metadata_path)
                if metadata.get("version") != version:
                    raise ValueError(f"Managed runtime version mismatch: {directory}")
                if not all((directory / "scripts" / name).is_file() for name in ("runtime.py", "command.py")):
                    raise ValueError(f"Managed runtime is incomplete: {directory}")
            directories.append(directory)
    return directories


def state_directories(root: Path, host: str) -> list[Path]:
    """The Windows IPC state directory (instance lock and endpoint state) that outlives the app.

    A redirected, foreign-owned, or unexpected tree is left in place with a warning.
    """
    state = root / "run"
    if host != "windows" or not os.path.lexists(state):
        return []
    try:
        check_tree(state)
    except ValueError as error:
        print(f"fastdiff: leaving {state} in place: {error}", file=sys.stderr)
        return []
    return [state]


def uninstall(host: str, root: Path) -> None:
    if host not in {"linux", "macos", "windows", "wsl"} or not root.is_absolute():
        raise ValueError("Uninstall requires the current environment's absolute installation root")
    check_ancestors(root)
    receipt_path = root / "receipt.json"
    check_path(receipt_path)
    receipt = runtime.read_json(receipt_path)
    if receipt.get("platform") != host:
        raise ValueError("The installation receipt belongs to another environment")
    runtime.installed_version(root)
    command = runtime.command_path(host, root)
    check_ancestors(command.parent)
    if command.exists() or command.is_symlink():
        check_path(command)
    runtime.check_command_owner(command, root, host)
    directories = managed_directories(root, host)
    state = state_directories(root, host)
    # Validate every removal target before deleting anything; receipt paths are never targets.
    # The state goes first so a still-running app fails the uninstall before packages are removed.
    for directory in [*state, *directories]:
        shutil.rmtree(directory)
    command.unlink(missing_ok=True)
    receipt_path.unlink()
    if host == "windows":
        runtime.remove_windows_path(root / "bin")
        runtime.remove_empty(root / "bin")
    for group in ("versions", "runtimes"):
        runtime.remove_empty(root / group)
    runtime.remove_empty(root)
    print("Uninstalled fastdiff from this environment")
