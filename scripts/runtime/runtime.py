#!/usr/bin/env python3
"""Install the latest stable fastdiff release or launch the installed app."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import re
import shutil
import shlex
import stat
import subprocess
import sys
import tarfile
import tempfile
import uuid
import zipfile
from urllib.request import Request, urlopen
from pathlib import Path, PurePosixPath, PureWindowsPath

REPOSITORY = "luisblanco93/fastdiff"
# Releases before the fastdiff rename installed under this name; setup removes them.
LEGACY_NAME = "diffcode"
LEGACY_PLATFORMS = ("linux", "wsl", "windows")
LEGACY_DIRECTORIES = {
    "versions": re.compile(r"\d+\.\d+\.\d+(?:-[\w.-]+)?-(?:linux|windows)-[0-9a-f]{32}"),
    "runtimes": re.compile(r"\d+\.\d+\.\d+(?:-[\w.-]+)?-[0-9a-f]{32}"),
}
# A WSL setup writes only Windows packages (no receipt, runtimes, or command) into the Windows root.
LEGACY_WINDOWS_PACKAGES = {
    "versions": re.compile(r"\d+\.\d+\.\d+(?:-[\w.-]+)?-windows-[0-9a-f]{32}"),
}


def launch_phase_start() -> float | None:
    if not os.environ.get("FASTDIFF_LAUNCH_TRACE"):
        return None
    import time
    return time.perf_counter()


def record_launch_phase(phase: str, started: float | None, *, started_unix_ms: float | None = None) -> None:
    trace = os.environ.get("FASTDIFF_LAUNCH_TRACE")
    if started is None or not trace:
        return
    import time
    row = {
        "schema": 1, "sample_id": os.environ.get("FASTDIFF_LAUNCH_SAMPLE_ID", ""),
        "phase": phase, "pid": os.getpid(), "duration_ms": (time.perf_counter() - started) * 1000,
    }
    if started_unix_ms is not None:
        row["started_unix_ms"] = started_unix_ms
    try:
        with open(trace, "a", encoding="utf-8") as output:
            output.write(json.dumps(row) + "\n")
    except (OSError, ValueError):
        # Optional profiling must not break launch; the harness rejects missing phases.
        pass


def read_json(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError(f"Expected an object in {path}")
    return value


def version_key(version: str) -> tuple[int, ...]:
    if not isinstance(version, str) or not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", version):
        raise ValueError("Release version must be stable major.minor.patch")
    return tuple(int(part) for part in version.split("."))


def installed_version(root: Path) -> str:
    started = launch_phase_start()
    try:
        version = read_json(root / "receipt.json").get("version")
        version_key(version)
        return version
    finally:
        record_launch_phase("receipt_version", started)


def fetch_json(url: str) -> dict:
    with urlopen(Request(url, headers={"User-Agent": "fastdiff-installer"}), timeout=60) as response:
        return json.load(response)


def download_file(url: str, destination: Path) -> None:
    with urlopen(Request(url, headers={"User-Agent": "fastdiff-installer"}), timeout=60) as source:
        with destination.open("xb") as output:
            shutil.copyfileobj(source, output)


def source_revision() -> str:
    metadata = fetch_json(f"https://api.github.com/repos/{REPOSITORY}/commits/main")
    revision = metadata.get("sha") if isinstance(metadata, dict) else None
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("main has no valid source revision")
    return revision


def latest_version() -> str:
    metadata = fetch_json(f"https://api.github.com/repos/{REPOSITORY}/releases/latest")
    if (not isinstance(metadata, dict) or metadata.get("draft") is not False
            or metadata.get("prerelease") is not False):
        raise ValueError("Latest release must be published and stable")
    tag = metadata.get("tag_name")
    if not isinstance(tag, str) or not tag.startswith("v"):
        raise ValueError("Latest release has no stable version tag")
    version_key(tag[1:])
    return tag[1:]


def reject_downgrade(root: Path, version: str) -> None:
    version_key(version)
    try:
        current = installed_version(root)
    except (FileNotFoundError, ValueError, TypeError):
        return  # Staged setup repairs missing or malformed receipts.
    if version_key(current) > version_key(version):
        raise ValueError(f"Refusing to downgrade fastdiff to {version}")


def update(host: str, root: Path) -> None:
    check_command_owner(command_path(host, root), root, host)
    revision = source_revision()
    with tempfile.TemporaryDirectory(prefix="fastdiff-update-") as temporary:
        installer = Path(temporary) / "install.py"
        download_file(f"https://raw.githubusercontent.com/{REPOSITORY}/{revision}/install.py", installer)
        subprocess.run([sys.executable, str(installer), "--revision", revision,
                        "--root", str(root)], check=True)


def host_platform() -> str:
    if platform.machine().lower() not in {"x86_64", "amd64"}:
        raise ValueError("fastdiff currently supports x64 Linux, Windows, and WSL2")
    system = platform.system()
    if system == "Windows":
        return "windows"
    if system == "Linux":
        if os.environ.get("WSL_DISTRO_NAME") or "microsoft" in platform.release().lower():
            if "wsl2" not in platform.release().lower():
                raise ValueError("fastdiff requires WSL2; WSL1 is not supported")
            return "wsl"
        return "linux"
    raise ValueError("fastdiff currently supports x64 Linux, Windows, and WSL2")


def data_root(host: str) -> Path:
    if host == "windows":
        local = os.environ.get("LOCALAPPDATA")
        if not local:
            raise ValueError("LOCALAPPDATA is unavailable")
        base = Path(local)
    else:
        base = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local" / "share")))
    if not base.is_absolute():
        raise ValueError("The application data directory must be an absolute path")
    return base / "fastdiff"


def wsl_windows_root() -> Path:
    output = subprocess.run(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
         "[Console]::OutputEncoding = [Text.UTF8Encoding]::new(); "
         "[Environment]::GetFolderPath('LocalApplicationData')"],
        check=True, capture_output=True, text=True, encoding="utf-8",
    ).stdout.strip()
    windows_path = PureWindowsPath(output)
    if not windows_path.is_absolute() or not re.fullmatch(r"[A-Za-z]:", windows_path.drive):
        raise ValueError("Cannot locate Windows-local LocalAppData from WSL")
    translated = subprocess.run(
        ["wslpath", "-u", str(windows_path / "fastdiff")],
        check=True, capture_output=True, text=True, encoding="utf-8",
    ).stdout.strip()
    result = Path(translated)
    if not result.is_absolute():
        raise ValueError("wslpath did not return an absolute Windows-local path")
    return result


def archive_name(version: str, target: str) -> str:
    extension = "tar.gz" if target == "linux" else "zip"
    return f"fastdiff-{version}-{target}-x64.{extension}"


def download_assets(version: str, names: list[str], stage: Path) -> None:
    version_key(version)
    base = f"https://github.com/{REPOSITORY}/releases/download/v{version}"
    for name in ["SHA256SUMS", *names]:
        if Path(name).name != name or not re.fullmatch(r"[A-Za-z0-9_.-]+", name):
            raise ValueError(f"Invalid release asset name: {name}")
    for name in ["SHA256SUMS", *names]:
        download_file(f"{base}/{name}", stage / name)


def verify_checksums(stage: Path, names: list[str]) -> None:
    entries = {}
    for line in (stage / "SHA256SUMS").read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](.+)", line)
        if not match:
            raise ValueError("Malformed SHA256SUMS entry")
        digest, name = match.groups()
        if name in entries:
            raise ValueError(f"Duplicate checksum entry: {name}")
        entries[name] = digest.lower()
    for name in names:
        if name not in entries:
            raise ValueError(f"Missing checksum for {name}")
        with (stage / name).open("rb") as archive:
            actual = hashlib.file_digest(archive, "sha256").hexdigest()
        if actual != entries[name]:
            raise ValueError(f"SHA-256 verification failed for {name}")


def archive_path(name: str, package: str) -> PurePosixPath:
    path = PurePosixPath(name)
    if (not path.parts or path.is_absolute() or ".." in path.parts
            or any(character in name for character in '\\:<>"|?*')
            or any(ord(character) < 32 for character in name)
            or path.parts[0] != package):
        raise ValueError(f"Unsafe archive path: {name}")
    for part in path.parts:
        if part.endswith((".", " ")) or re.fullmatch(r"(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\..*)?", part, re.I):
            raise ValueError(f"Unsafe Windows archive path: {name}")
    return path


def extract_archive(archive: Path, destination: Path, package: str) -> Path:
    seen = set()

    def output_path(name: str) -> Path:
        relative = archive_path(name, package)
        key = str(relative).casefold()
        if key in seen:
            raise ValueError(f"Duplicate archive path: {name}")
        seen.add(key)
        return destination.joinpath(*relative.parts)

    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as source:
            for member in source.infolist():
                target = output_path(member.filename)
                mode = member.external_attr >> 16
                if stat.S_ISLNK(mode) or stat.S_IFMT(mode) not in {0, stat.S_IFREG, stat.S_IFDIR}:
                    raise ValueError(f"Unsupported archive entry: {member.filename}")
                if member.is_dir():
                    target.mkdir(parents=True, exist_ok=True)
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with source.open(member) as reader, target.open("xb") as writer:
                        shutil.copyfileobj(reader, writer)
    else:
        with tarfile.open(archive, "r:gz") as source:
            for member in source:
                target = output_path(member.name)
                if member.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                elif member.isfile():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with source.extractfile(member) as reader, target.open("xb") as writer:
                        shutil.copyfileobj(reader, writer)
                else:
                    raise ValueError(f"Unsupported archive entry: {member.name}")
    return destination / package


def validate_package(package: Path, version: str, target: str) -> dict:
    metadata = read_json(package / "build-info.json")
    if metadata.get("version") != version or metadata.get("platform") != f"{target}-x64":
        raise ValueError(f"Release version or platform mismatch in {package}")
    revision = metadata.get("revision")
    if not isinstance(revision, str) or not revision or revision == "unversioned":
        raise ValueError("The release has no source revision for pairing")
    binaries = ["bin/fastdiff", "bin/fastdiff-gui"] if target == "linux" else [
        "bin/fastdiff.exe", "bin/fastdiff-gui.exe", "wsl/fastdiff"]
    for relative in binaries:
        path = package / relative
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"Missing release executable: {path}")
    return metadata


def load_install(root: Path, version: str, host: str) -> dict:
    receipt = read_json(root / "receipt.json")
    if receipt.get("version") != version or receipt.get("platform") != host:
        raise ValueError("The installed app does not match this version or platform")
    names = ["launcher", "gui"] + (["windows_gui"] if host == "wsl" else [])
    paths = {name: Path(receipt.get(name, "")) for name in names}
    for path in paths.values():
        if not path.is_absolute() or not path.is_file():
            raise ValueError(f"Installed executable is unavailable: {path}")
    target = "windows" if host == "windows" else "linux"
    extension = ".exe" if target == "windows" else ""
    package = paths["launcher"].parent.parent
    if paths["launcher"] != package / "bin" / f"fastdiff{extension}" or paths["gui"] != package / "bin" / f"fastdiff-gui{extension}":
        raise ValueError("The receipt does not identify a paired installation")
    metadata = validate_package(package, version, target)
    if host == "wsl":
        windows_package = paths["windows_gui"].parent.parent
        if paths["windows_gui"] != windows_package / "bin" / "fastdiff-gui.exe":
            raise ValueError("The receipt does not identify a Windows GUI")
        windows = validate_package(windows_package, version, "windows")
        if windows["revision"] != metadata["revision"]:
            raise ValueError("Linux and Windows release revisions do not match")
    return receipt


def install_package(archive: Path, root: Path, version: str, target: str) -> Path:
    root.mkdir(parents=True, exist_ok=True)
    package_name = f"fastdiff-{version}-{target}-x64"
    # Each attempt has a fresh destination so an interrupted setup cannot damage the old app.
    with tempfile.TemporaryDirectory(prefix=".stage-", dir=root) as temporary:
        package = extract_archive(archive, Path(temporary), package_name)
        metadata = package / "build-info.json"
        if not metadata.exists():
            helper = package / ("bin/fastdiff" if target == "linux" else "wsl/fastdiff")
            metadata.write_text(json.dumps({
                "version": version, "platform": f"{target}-x64",
                "revision": "sha256:" + hashlib.sha256(helper.read_bytes()).hexdigest(),
            }) + "\n", encoding="utf-8")
        validate_package(package, version, target)
        if target == "linux":
            for binary in ("fastdiff", "fastdiff-gui"):
                (package / "bin" / binary).chmod(0o755)
        destination = root / "versions" / f"{version}-{target}-{uuid.uuid4().hex}"
        destination.parent.mkdir(parents=True, exist_ok=True)
        package.rename(destination)
    return destination


def write_receipt(root: Path, receipt: dict) -> None:
    temporary = root / f".receipt-{uuid.uuid4().hex}.json"
    try:
        temporary.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
        temporary.replace(root / "receipt.json")
    finally:
        temporary.unlink(missing_ok=True)


def command_path(host: str, root: Path) -> Path:
    return root / "bin" / "dff.cmd" if host == "windows" else Path.home() / ".local" / "bin" / "dff"


def command_marker(root: Path, host: str, name: str = "fastdiff") -> str:
    prefix = "rem" if host == "windows" else "#"
    owner = hashlib.sha256(str(root).encode("utf-8")).hexdigest()
    return f"{prefix} {name}-managed-command: {owner}"


def legacy_root(root: Path) -> Path:
    return root.with_name(LEGACY_NAME)


def check_command_owner(path: Path, root: Path, host: str, *, replace_legacy: bool = False) -> None:
    owners = {command_marker(root, host)}
    if replace_legacy:
        # Linux and WSL share ~/.local/bin/dff with the pre-rename install; installing takes it over.
        owners.add(command_marker(legacy_root(root), host, LEGACY_NAME))
    if path.is_symlink() or (path.exists() and (
            not path.is_file() or owners.isdisjoint(path.read_text(encoding="utf-8").splitlines()))):
        raise ValueError(f"Refusing to overwrite user-owned command {path}; move it before rerunning the installer.")


def stage_command(root: Path, version: str, host: str) -> tuple[dict, Path]:
    directory = root / "runtimes" / f"{version}-{uuid.uuid4().hex}"
    scripts = directory / "scripts"
    scripts.mkdir(parents=True)
    (directory / "runtime.json").write_text(json.dumps({"version": version}), encoding="utf-8")
    for name in ("runtime.py", "command.py", "lifecycle.py"):
        shutil.copyfile(Path(__file__).with_name(name), scripts / name)
    python = str(Path(sys.executable).resolve())
    command = str(scripts / "command.py")
    marker = command_marker(root, host)
    if host == "windows":
        # CMD expands percent signs even inside quotes; double them in batch literals.
        executable, entry = (value.replace("%", "%%") for value in (python, command))
        body = (f'@echo off\nsetlocal EnableDelayedExpansion\nsetlocal DisableDelayedExpansion\n{marker}\n'
                'for /f "tokens=2 delims=:" %%G in (\'chcp\') do set "fastdiff_codepage=%%G"\n'
                'chcp 65001 >nul\n'
                # Leave the batch context before returning: uninstall may have deleted this file.
                f'(\n"{executable}" "{entry}" %*\n'
                'endlocal\nfor %%E in (!errorlevel!) do (\n'
                'chcp %fastdiff_codepage% >nul\n'
                '(goto) 2>nul & "%COMSPEC%" /d /c exit %%E\n)\n)\n')
    else:
        body = f'#!/bin/sh\n{marker}\nexec {shlex.quote(python)} {shlex.quote(command)} "$@"\n'
    path = command_path(host, root)
    path.parent.mkdir(parents=True, exist_ok=True)
    pending = path.with_name(f".dff-{uuid.uuid4().hex}.tmp")
    pending.write_text(body, encoding="utf-8")
    if host != "windows":
        pending.chmod(0o755)
    return {"python": python, "command": command}, pending


def add_windows_path(directory: Path) -> None:
    import winreg

    with winreg.CreateKey(winreg.HKEY_CURRENT_USER, "Environment") as key:
        try:
            value, kind = winreg.QueryValueEx(key, "Path")
        except FileNotFoundError:
            value, kind = "", winreg.REG_EXPAND_SZ
        entries = value.split(";")
        normalized = os.path.normcase(os.path.normpath(str(directory)))
        if any(os.path.normcase(os.path.normpath(os.path.expandvars(entry))) == normalized
               for entry in entries if entry):
            return
        separator = "" if not value or value.endswith(";") else ";"
        winreg.SetValueEx(key, "Path", 0, kind, value + separator + str(directory))
    notify_windows_environment()


def notify_windows_environment() -> None:
    import ctypes
    from ctypes import wintypes

    send = ctypes.windll.user32.SendMessageTimeoutW
    send.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM,
                     wintypes.UINT, wintypes.UINT, ctypes.POINTER(ctypes.c_size_t)]
    send.restype = wintypes.LPARAM
    environment = ctypes.c_wchar_p("Environment")
    result = ctypes.c_size_t()
    if not send(0xFFFF, 0x001A, 0, ctypes.cast(environment, ctypes.c_void_p).value,
                2, 1000, ctypes.byref(result)):
        print("User PATH updated; sign out and back in if new terminals cannot find dff.")


def remove_windows_path(directory: Path) -> None:
    import winreg

    with winreg.CreateKey(winreg.HKEY_CURRENT_USER, "Environment") as key:
        try:
            value, kind = winreg.QueryValueEx(key, "Path")
        except FileNotFoundError:
            return
        entries = value.split(";")
        remaining = [entry for entry in entries if entry != str(directory)]
        if remaining == entries:
            return
        winreg.SetValueEx(key, "Path", 0, kind, ";".join(remaining))
    notify_windows_environment()


def remove_empty(directory: Path) -> None:
    if directory.is_dir() and not any(directory.iterdir()):
        directory.rmdir()


def is_redirected(status: os.stat_result) -> bool:
    """True for symlinks and, because Path.is_symlink() misses them on Python 3.11, Windows junctions."""
    return stat.S_ISLNK(status.st_mode) or bool(getattr(status, "st_file_attributes", 0) & 0x400)


def check_path(path: Path) -> None:
    """Reject redirected paths and paths owned by another user."""
    status = path.lstat()
    if is_redirected(status):
        raise ValueError(f"Refusing to remove a symlink or junction: {path}")
    if hasattr(os, "getuid") and status.st_uid != os.getuid():
        raise ValueError(f"Refusing to remove a user-owned path belonging to another user: {path}")


def check_ancestors(path: Path) -> None:
    for ancestor in (path, *path.parents):
        if ancestor.exists() or ancestor.is_symlink():
            if is_redirected(ancestor.lstat()):
                raise ValueError(f"Refusing to remove through a symlink or junction: {ancestor}")
    if path.exists():
        check_path(path)


def check_tree(directory: Path) -> None:
    check_path(directory)
    if not directory.is_dir():
        raise ValueError(f"Managed directory is not a directory: {directory}")
    for child in directory.iterdir():
        check_path(child)
        if child.is_dir():
            check_tree(child)
        elif not child.is_file():
            raise ValueError(f"Unsupported managed file: {child}")


def warn_legacy(legacy: Path, error: object) -> None:
    print(f"Could not remove the previous {LEGACY_NAME} installation at {legacy}: {error}. "
          "Resolve this and rerun the installer.", file=sys.stderr)


def legacy_is_removable(legacy: Path, host: str, packages_only: bool) -> bool:
    """Decide whether this environment may clean legacy; redirected or foreign-owned paths raise."""
    check_ancestors(legacy)
    receipt = legacy / "receipt.json"
    if not os.path.lexists(receipt):
        return packages_only  # A WSL-only install writes Windows packages without a receipt.
    if packages_only:
        return False  # A receipt in the Windows root is a native install its own installer cleans.
    check_path(receipt)
    if not receipt.is_file():
        return False
    recorded = read_json(receipt).get("platform")
    # A receipt from another environment is never cleaned from here.
    return recorded in LEGACY_PLATFORMS and (recorded == "windows") == (host == "windows")


def remove_legacy_entries(legacy: Path, groups: dict[str, re.Pattern[str]]) -> bool:
    """Remove the matching package folders; return False when a redirected path was skipped."""
    complete = True
    for group, pattern in groups.items():
        parent = legacy / group
        if not os.path.lexists(parent):
            continue
        try:
            check_path(parent)
            if not parent.is_dir():
                continue
        except ValueError as error:
            warn_legacy(legacy, error)
            complete = False
            continue
        for directory in parent.iterdir():
            if not pattern.fullmatch(directory.name):
                continue
            try:
                check_path(directory)
                if not directory.is_dir():
                    continue
                check_tree(directory)  # Nothing inside may be a link, junction, or another user's file.
            except ValueError as error:
                warn_legacy(legacy, error)
                complete = False
                continue
            shutil.rmtree(directory)
        remove_empty(parent)
    return complete


def remove_legacy_command(legacy: Path, host: str) -> bool:
    """Remove the legacy Windows command and PATH entry; return False when it was skipped."""
    command = command_path("windows", legacy)
    if host != "windows" or not os.path.lexists(command):
        return True
    try:
        check_path(command.parent)
        check_path(command)
    except ValueError as error:
        warn_legacy(legacy, error)
        return False
    if command.is_file() and command_marker(
            legacy, host, LEGACY_NAME) in command.read_text(encoding="utf-8").splitlines():
        remove_windows_path(command.parent)
        command.unlink()
        remove_empty(command.parent)
    return True


def remove_legacy_install(root: Path, host: str, *, packages_only: bool = False) -> None:
    """Remove the managed diffcode installation that sits next to a fastdiff data root.

    A managed install is identified by its receipt, and only when that receipt belongs to the
    environment doing the cleanup. With packages_only (the Windows root seen from WSL, which a
    WSL-only install populates without a receipt), only Windows package folders are removed, and
    only while no receipt exists: a receipt there is a native Windows install that its own
    installer must clean up. Redirected paths (symlinks, junctions) are never traversed.
    """
    legacy = legacy_root(root)
    if legacy == root or not os.path.lexists(legacy):
        return  # Only a separate directory is ever removed.
    if not legacy_is_removable(legacy, host, packages_only):
        return
    complete = remove_legacy_entries(legacy, LEGACY_WINDOWS_PACKAGES if packages_only else LEGACY_DIRECTORIES)
    if packages_only:
        remove_empty(legacy)
        return
    complete = remove_legacy_command(legacy, host) and complete
    if not complete:
        return  # Skipped paths keep the receipt so the next installer run warns and retries.
    # The receipt goes last so an interrupted cleanup is retried by the next installer run.
    (legacy / "receipt.json").unlink()
    remove_empty(legacy)


def remove_legacy_installs(host: str, roots: list[Path], windows_roots: tuple[Path, ...] = ()) -> None:
    """Clean each data root; windows_roots are the Windows-side roots a WSL setup shares."""
    attempts = [(directory, False) for directory in roots] + [(directory, True) for directory in windows_roots]
    for directory, packages_only in attempts:
        try:
            remove_legacy_install(directory, host, packages_only=packages_only)
        except (OSError, ValueError) as error:
            warn_legacy(legacy_root(directory), error)


def activate_command(root: Path, version: str, host: str, receipt: dict) -> None:
    path = command_path(host, root)
    check_command_owner(path, root, host, replace_legacy=True)
    previous = path.read_bytes() if path.exists() else None
    fields, pending = stage_command(root, version, host)
    published = {**receipt, **fields}
    activated = False
    try:
        if host == "windows":
            add_windows_path(path.parent)
        # The stable entry follows the receipt, so it can also launch the previous app.
        check_command_owner(path, root, host, replace_legacy=True)
        pending.replace(path)
        activated = True
        write_receipt(root, published)
    finally:
        pending.unlink(missing_ok=True)
        try:
            active = read_json(root / "receipt.json").get("command") == fields["command"]
        except (OSError, ValueError):
            active = False
        if activated and not active:
            if previous is None:
                path.unlink(missing_ok=True)
            else:
                rollback = path.with_name(f".dff-{uuid.uuid4().hex}.tmp")
                rollback.write_bytes(previous)
                if host != "windows":
                    rollback.chmod(0o755)
                rollback.replace(path)
    if host == "windows":
        print(f"Installed {path}; open a new terminal to refresh PATH")
    elif previous is None and str(path.parent) not in os.environ.get("PATH", "").split(os.pathsep):
        print(f"Installed {path}. Activate in this terminal: export PATH={shlex.quote(str(path.parent))}:\"$PATH\"")


def setup(version: str, host: str, root: Path) -> None:
    check_command_owner(command_path(host, root), root, host, replace_legacy=True)
    reject_downgrade(root, version)
    try:
        existing = load_install(root, version, host)
    except (OSError, ValueError, TypeError):
        pass  # A missing or stale receipt requires a fresh staged installation.
    else:
        activate_command(root, version, host, existing)
        # A receipt-paired Windows package lives at <windows root>/versions/<package>/bin/<exe>.
        windows_roots = (Path(existing["windows_gui"]).parents[3],) if host == "wsl" else ()
        remove_legacy_installs(host, [root], windows_roots)
        print(f"fastdiff {version} is already installed")
        return
    targets = ["linux", "windows"] if host == "wsl" else [host]
    roots = {"linux": root, "windows": wsl_windows_root() if host == "wsl" else root}
    names = [archive_name(version, target) for target in targets]
    root.mkdir(parents=True, exist_ok=True)
    installed = []
    with tempfile.TemporaryDirectory(prefix=".download-", dir=root) as temporary:
        stage = Path(temporary)
        download_assets(version, names, stage)
        verify_checksums(stage, names)
        for target, name in zip(targets, names):
            installed.append(install_package(stage / name, roots[target], version, target))
        launcher = installed[0] / "bin" / ("fastdiff.exe" if host == "windows" else "fastdiff")
        gui = launcher.with_name("fastdiff-gui.exe" if host == "windows" else "fastdiff-gui")
        receipt = {"version": version, "platform": host, "launcher": str(launcher), "gui": str(gui)}
        if host == "wsl":
            linux = validate_package(installed[0], version, "linux")
            windows = validate_package(installed[1], version, "windows")
            if linux["revision"] != windows["revision"]:
                raise ValueError("Linux and Windows release revisions do not match")
            receipt = {**receipt, "windows_gui": str(installed[1] / "bin" / "fastdiff-gui.exe")}
        # Preserve version folders even on failure: the receipt may already reference them.
        activate_command(root, version, host, receipt)
    remove_legacy_installs(host, [root], (roots["windows"],) if host == "wsl" else ())
    print(f"Installed fastdiff {version}; invoke dff to open Git changes")


def launch(version: str, host: str, root: Path, arguments: list[str]) -> int:
    started = launch_phase_start()
    try:
        receipt = load_install(root, version, host)
    except (OSError, ValueError, TypeError) as error:
        raise ValueError(f"fastdiff installation is unavailable ({error}). Rerun the installer.") from error
    finally:
        record_launch_phase("receipt_validation", started)
    environment = {**os.environ, "FASTDIFF_GUI_EXE": receipt["gui"]}
    if host == "wsl":
        environment = {**environment, "FASTDIFF_WINDOWS_EXE": receipt["windows_gui"]}
    return subprocess.run([receipt["launcher"], *arguments], env=environment).returncode
