"""Offline checks; run with python3 -m unittest discover -s scripts -p test_plugin_runtime.py."""

import hashlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from contextlib import contextmanager
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

MODULE = Path(__file__).resolve().parents[1] / "scripts/runtime/runtime.py"
SPEC = importlib.util.spec_from_file_location("fastdiff_runtime", MODULE)
runtime = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runtime)
VERSION = "0.1.1"


def make_package(parent: Path, target: str, revision: str = "abc123", version: str = VERSION) -> Path:
    package = parent / f"fastdiff-{version}-{target}-x64"
    package.mkdir(parents=True)
    (package / "build-info.json").write_text(json.dumps({
        "version": version, "platform": f"{target}-x64", "revision": revision,
    }))
    names = ["bin/fastdiff", "bin/fastdiff-gui"] if target == "linux" else [
        "bin/fastdiff.exe", "bin/fastdiff-gui.exe", "wsl/fastdiff"]
    for name in names:
        path = package / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"binary")
    return package


def make_legacy_install(root: Path, version: str = "0.1.0", platform: str = "linux", receipt: bool = True) -> Path:
    """Create a managed diffcode data root, as installed before the fastdiff rename, beside root.

    A WSL-only install leaves the Windows root with only Windows packages and no receipt: pass
    platform="windows", receipt=False for that shape.
    """
    legacy = root.with_name("diffcode")
    target = "windows" if platform == "windows" else "linux"
    suffix = ".exe" if target == "windows" else ""
    package = legacy / "versions" / f"{version}-{target}-{'a' * 32}"
    for name in (f"bin/diffcode{suffix}", f"bin/diffcode-gui{suffix}"):
        (package / name).parent.mkdir(parents=True, exist_ok=True)
        (package / name).write_bytes(b"legacy binary")
    if receipt:
        scripts = legacy / "runtimes" / f"{version}-{'b' * 32}" / "scripts"
        scripts.mkdir(parents=True, exist_ok=True)
        (scripts / "command.py").write_text("legacy command", encoding="utf-8")
        (legacy / "receipt.json").write_text(json.dumps({
            "version": version, "platform": platform, "launcher": str(package / f"bin/diffcode{suffix}"),
        }))
    return legacy


def snapshot(directory: Path) -> dict[str, bytes | None]:
    """Map every relative path under directory to its bytes (None for directories)."""
    return {str(path.relative_to(directory)): path.read_bytes() if path.is_file() else None
            for path in sorted(directory.rglob("*"))}


def fake_download(version: str, names: list[str], stage: Path, windows_revision: str = "abc123", binary_only: bool = True) -> None:
    checksums = []
    for name in names:
        target = "linux" if name.endswith("tar.gz") else "windows"
        package = make_package(stage / "input" / target, target,
                               windows_revision if target == "windows" else "abc123", version)
        if binary_only:
            (package / "build-info.json").unlink()
            if target == "windows" and windows_revision != "abc123":
                (package / "wsl/fastdiff").write_bytes(windows_revision.encode())
        archive = stage / name
        if target == "linux":
            with tarfile.open(archive, "w:gz") as writer:
                writer.add(package, arcname=package.name)
        else:
            with zipfile.ZipFile(archive, "w") as writer:
                for path in package.rglob("*"):
                    if path.is_file():
                        writer.write(path, arcname=str(path.relative_to(package.parent)))
        checksums.append(f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {name}")
    (stage / "SHA256SUMS").write_text("\n".join(checksums) + "\n")


class PluginRuntimeTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory(prefix="fastdiff test ")
        self.addCleanup(temporary.cleanup)
        self.parent = Path(temporary.name)
        self.root = self.parent / "linux data" / "fastdiff"
        location = patch.object(runtime, "command_path", return_value=self.parent / "bin" / "dff")
        location.start()
        self.addCleanup(location.stop)

    @contextmanager
    def reparse_points(self, *paths: Path):
        """Make lstat report a Windows reparse point (junction) for paths, as Python 3.11 does."""
        real_lstat = Path.lstat
        redirected = set(paths)

        def lstat(path, *args, **kwargs):
            status = real_lstat(path, *args, **kwargs)
            if path in redirected:
                return SimpleNamespace(st_mode=status.st_mode, st_uid=status.st_uid, st_file_attributes=0x400)
            return status

        with patch.object(Path, "lstat", lstat):
            yield

    def test_platform_selection_and_unsupported_hosts(self) -> None:
        for system, release, env, expected in [
            ("Linux", "6.8.0", {}, "linux"),
            ("Windows", "11", {}, "windows"),
            ("Linux", "6.6.87.2-microsoft-standard-WSL2", {"WSL_DISTRO_NAME": "Ubuntu"}, "wsl"),
        ]:
            with self.subTest(system=system), patch.dict(os.environ, env, clear=True), \
                    patch.object(runtime.platform, "machine", return_value="AMD64"), \
                    patch.object(runtime.platform, "system", return_value=system), \
                    patch.object(runtime.platform, "release", return_value=release):
                self.assertEqual(runtime.host_platform(), expected)
        with patch.object(runtime.platform, "machine", return_value="aarch64"):
            with self.assertRaisesRegex(ValueError, "x64"):
                runtime.host_platform()
        with patch.object(runtime.platform, "machine", return_value="x86_64"), \
                patch.object(runtime.platform, "system", return_value="Darwin"):
            with self.assertRaises(ValueError):
                runtime.host_platform()

    def test_entry_point_traces_imports(self) -> None:
        fields, pending = runtime.stage_command(self.root, VERSION, "linux")
        pending.unlink()
        trace = self.parent / "entry.jsonl"
        base = {key: value for key, value in os.environ.items()
                if not key.startswith("FASTDIFF_")}
        result = subprocess.run(
            [sys.executable, fields["command"], "--help"], capture_output=True, text=True,
            env={**base, "FASTDIFF_LAUNCH_TRACE": str(trace), "FASTDIFF_LAUNCH_SAMPLE_ID": "entry"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stderr, "")
        row = json.loads(trace.read_text())
        self.assertEqual((row["phase"], row["sample_id"]), ("python_imports", "entry"))

    def test_bad_checksum_never_publishes_receipt(self) -> None:
        def corrupt(version, names, stage):
            fake_download(version, names, stage)
            (stage / names[0]).write_bytes(b"corrupt")
        with patch.object(runtime, "download_assets", side_effect=corrupt):
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                runtime.setup(VERSION, "linux", self.root)
        self.assertFalse((self.root / "receipt.json").exists())

    def test_exact_checksum_entry_is_required(self) -> None:
        stage = self.parent / "assets"
        stage.mkdir()
        fake_download(VERSION, [runtime.archive_name(VERSION, "linux")], stage)
        with self.assertRaisesRegex(ValueError, "Missing checksum"):
            runtime.verify_checksums(stage, ["other.tar.gz"])

    def test_version_mismatch_is_rejected(self) -> None:
        package = make_package(self.parent, "linux")
        with self.assertRaisesRegex(ValueError, "version or platform mismatch"):
            runtime.validate_package(package, "0.2.0", "linux")

    def test_binary_only_release_generates_local_pairing_metadata(self) -> None:
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(VERSION, "linux", self.root)
        receipt = runtime.load_install(self.root, VERSION, "linux")
        package = Path(receipt["launcher"]).parent.parent
        metadata = runtime.read_json(package / "build-info.json")
        self.assertEqual(metadata["revision"], "sha256:" + hashlib.sha256(b"binary").hexdigest())

    def test_legacy_release_metadata_is_preserved(self) -> None:
        def legacy(version, names, stage):
            fake_download(version, names, stage, binary_only=False)
        with patch.object(runtime, "download_assets", side_effect=legacy):
            runtime.setup(VERSION, "linux", self.root)
        receipt = runtime.load_install(self.root, VERSION, "linux")
        metadata = runtime.read_json(Path(receipt["launcher"]).parent.parent / "build-info.json")
        self.assertEqual(metadata["revision"], "abc123")

    def test_interrupted_setup_preserves_previous_install(self) -> None:
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(VERSION, "linux", self.root)
        before = (self.root / "receipt.json").read_bytes()
        with patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "write_receipt", side_effect=OSError("interrupted")):
            with self.assertRaisesRegex(OSError, "interrupted"):
                runtime.setup("0.2.0", "linux", self.root)
        self.assertEqual((self.root / "receipt.json").read_bytes(), before)
        self.assertEqual(runtime.load_install(self.root, VERSION, "linux")["version"], VERSION)

    def test_error_after_receipt_publication_preserves_new_install(self) -> None:
        original_write = runtime.write_receipt

        def publish_then_fail(root, receipt):
            original_write(root, receipt)
            raise OSError("interrupted after publication")

        with patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "write_receipt", side_effect=publish_then_fail):
            with self.assertRaisesRegex(OSError, "after publication"):
                runtime.setup(VERSION, "linux", self.root)
        receipt = runtime.load_install(self.root, VERSION, "linux")
        self.assertTrue(Path(receipt["launcher"]).is_file())
        self.assertTrue(Path(receipt["gui"]).is_file())

    def test_repeated_setup_does_not_download_again(self) -> None:
        with patch.object(runtime, "download_assets", side_effect=fake_download) as download:
            runtime.setup(VERSION, "linux", self.root)
            runtime.setup(VERSION, "linux", self.root)
        self.assertEqual(download.call_count, 1)

    def test_setup_removes_legacy_install_and_takes_over_its_command(self) -> None:
        legacy = make_legacy_install(self.root)
        command = self.parent / "bin" / "dff"
        command.parent.mkdir()
        command.write_text(f"#!/bin/sh\n{runtime.command_marker(legacy, 'linux', 'diffcode')}\nexec legacy\n")
        unrelated = legacy / "notes.txt"
        unrelated.write_text("keep")
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(VERSION, "linux", self.root)
        self.assertEqual([path.name for path in legacy.iterdir()], ["notes.txt"])
        self.assertIn(runtime.command_marker(self.root, "linux"), command.read_text().splitlines())
        self.assertEqual(runtime.load_install(self.root, VERSION, "linux")["version"], VERSION)

    def test_legacy_cleanup_ignores_unmanaged_and_failing_directories(self) -> None:
        legacy = make_legacy_install(self.root)
        (legacy / "receipt.json").unlink()
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(VERSION, "linux", self.root)
        self.assertEqual(sorted(path.name for path in legacy.iterdir()), ["runtimes", "versions"])
        make_legacy_install(self.root)
        with patch.object(runtime.shutil, "rmtree", side_effect=PermissionError("locked")), \
                patch("sys.stderr", new_callable=io.StringIO) as output:
            runtime.remove_legacy_installs("linux", [self.root])
        self.assertIn("Could not remove the previous diffcode installation", output.getvalue())
        self.assertTrue((legacy / "receipt.json").is_file(), "receipt stays so the next installer run retries")

    def test_legacy_cleanup_ignores_receipts_from_another_environment(self) -> None:
        for host, platform in [("linux", "windows"), ("wsl", "windows"), ("windows", "linux"),
                               ("windows", "wsl"), ("linux", "unknown")]:
            with self.subTest(host=host, platform=platform):
                legacy = make_legacy_install(self.root, platform=platform)
                before = snapshot(legacy)
                runtime.remove_legacy_installs(host, [self.root])
                self.assertEqual(snapshot(legacy), before)
                shutil.rmtree(legacy)

    def test_legacy_cleanup_never_traverses_junctions(self) -> None:
        for position in ("root", "versions", "runtimes", "package", "file inside package"):
            with self.subTest(position=position):
                legacy = make_legacy_install(self.root)
                package = next((legacy / "versions").iterdir())
                other = legacy / "versions" / f"0.0.9-linux-{'c' * 32}"
                (other / "bin").mkdir(parents=True)
                (other / "bin" / "diffcode").write_bytes(b"other legacy binary")
                redirected = {"root": legacy, "versions": legacy / "versions", "runtimes": legacy / "runtimes",
                              "package": package, "file inside package": package / "bin" / "diffcode"}[position]
                removed = {"versions": legacy / "runtimes" / f"0.1.0-{'b' * 32}", "runtimes": package,
                           "package": other, "file inside package": other}.get(position)
                before_all = snapshot(legacy)
                before = snapshot(redirected) if redirected.is_dir() else redirected.read_bytes()
                with self.reparse_points(redirected), patch("sys.stderr", new_callable=io.StringIO) as output:
                    runtime.remove_legacy_installs("linux", [self.root])
                self.assertEqual(snapshot(redirected) if redirected.is_dir() else redirected.read_bytes(), before)
                self.assertIn("symlink or junction", output.getvalue())
                self.assertIn(str(redirected), output.getvalue())
                self.assertTrue((legacy / "receipt.json").is_file(), "receipt stays so the next run retries")
                if removed is None:
                    self.assertEqual(snapshot(legacy), before_all)
                else:
                    self.assertFalse(removed.exists())
                shutil.rmtree(legacy)

    def test_legacy_cleanup_ignores_paths_owned_by_another_user(self) -> None:
        legacy = make_legacy_install(self.root)
        before = snapshot(legacy)
        with patch.object(runtime.os, "getuid", return_value=os.stat(legacy).st_uid + 1, create=True), \
                patch("sys.stderr", new_callable=io.StringIO) as output:
            runtime.remove_legacy_installs("linux", [self.root])
        self.assertEqual(snapshot(legacy), before)
        self.assertIn("another user", output.getvalue())

    def test_legacy_windows_command_behind_a_junction_is_left_alone(self) -> None:
        legacy = make_legacy_install(self.root, platform="windows")
        command = legacy / "bin" / "dff.cmd"
        command.parent.mkdir()
        command.write_text(f"@echo off\n{runtime.command_marker(legacy, 'windows', 'diffcode')}\n")
        before = snapshot(command.parent)
        with self.reparse_points(command.parent), \
                patch.object(runtime, "command_path", side_effect=lambda host, root: root / "bin" / "dff.cmd"), \
                patch.object(runtime, "remove_windows_path") as remove_path, \
                patch("sys.stderr", new_callable=io.StringIO) as output:
            runtime.remove_legacy_installs("windows", [self.root])
        remove_path.assert_not_called()
        self.assertEqual(snapshot(command.parent), before)
        self.assertIn("symlink or junction", output.getvalue())
        self.assertTrue((legacy / "receipt.json").is_file())

    def test_legacy_command_is_refused_without_the_takeover_marker(self) -> None:
        command = self.parent / "bin" / "dff"
        command.parent.mkdir()
        command.write_text("#!/bin/sh\n# diffcode-managed-command: " + "0" * 64 + "\n")
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            with self.assertRaisesRegex(ValueError, "user-owned"):
                runtime.setup(VERSION, "linux", self.root)

    def test_wsl_setup_removes_legacy_windows_packages_on_repeat_runs(self) -> None:
        windows_root = self.parent / "Windows LocalAppData" / "fastdiff"
        legacy = make_legacy_install(windows_root, platform="windows", receipt=False)
        self.assertFalse((legacy / "receipt.json").exists(), "a WSL-only install writes no Windows receipt")
        with patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "wsl_windows_root", return_value=windows_root):
            runtime.setup(VERSION, "wsl", self.root)
            self.assertFalse(legacy.exists())
            make_legacy_install(windows_root, platform="windows", receipt=False)
            runtime.setup(VERSION, "wsl", self.root)
        self.assertFalse(legacy.exists())

    def test_wsl_cleanup_of_the_windows_root_is_limited_to_windows_packages(self) -> None:
        windows_root = self.parent / "Windows LocalAppData" / "fastdiff"
        legacy = make_legacy_install(windows_root, platform="windows", receipt=False)
        kept = [legacy / "versions" / f"0.1.0-linux-{'c' * 32}", legacy / "versions" / "user data",
                legacy / "versions" / f"0.1.0-windows-{'d' * 31}", legacy / "runtimes" / f"0.1.0-{'b' * 32}"]
        for directory in kept:
            directory.mkdir(parents=True)
        command = legacy / "bin" / "dff.cmd"
        command.parent.mkdir()
        command.write_text(f"@echo off\n{runtime.command_marker(legacy, 'windows', 'diffcode')}\n")
        runtime.remove_legacy_installs("wsl", [], (windows_root,))
        self.assertEqual(sorted(path.name for path in (legacy / "versions").iterdir()),
                         sorted(path.name for path in kept if path.parent.name == "versions"))
        self.assertTrue(all(directory.is_dir() for directory in kept))
        self.assertTrue(command.is_file())

    def test_wsl_setup_leaves_native_windows_legacy_install_untouched(self) -> None:
        windows_root = self.parent / "Windows LocalAppData" / "fastdiff"
        native = make_legacy_install(windows_root, platform="windows")
        command = native / "bin" / "dff.cmd"
        command.parent.mkdir()
        command.write_text(f"@echo off\n{runtime.command_marker(native, 'windows', 'diffcode')}\n")
        linux = make_legacy_install(self.root, platform="wsl")
        before = snapshot(native)
        with patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "wsl_windows_root", return_value=windows_root), \
                patch.object(runtime, "remove_windows_path") as remove_path:
            runtime.setup(VERSION, "wsl", self.root)
            self.assertEqual(snapshot(native), before)
            self.assertFalse(linux.exists(), "the WSL legacy install itself is still removed")
            runtime.setup(VERSION, "wsl", self.root)
        self.assertEqual(snapshot(native), before)
        remove_path.assert_not_called()

    def test_windows_setup_removes_legacy_command_and_path_entry(self) -> None:
        legacy = make_legacy_install(self.root, platform="windows")
        command = legacy / "bin" / "dff.cmd"
        command.parent.mkdir()
        command.write_text(f"@echo off\n{runtime.command_marker(legacy, 'windows', 'diffcode')}\n")
        with patch.object(runtime, "command_path", side_effect=lambda host, root: root / "bin" / "dff.cmd"), \
                patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "add_windows_path"), \
                patch.object(runtime, "remove_windows_path") as remove_path:
            runtime.setup(VERSION, "windows", self.root)
        remove_path.assert_called_once_with(command.parent)
        self.assertFalse(legacy.exists())
        self.assertTrue((self.root / "bin" / "dff.cmd").is_file())

    def test_launch_passes_arguments_with_spaces_without_shell(self) -> None:
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(VERSION, "linux", self.root)
        arguments = ["--file", "src/a file;$(touch x).rs", "--view", "staged", "--", "/repo with spaces"]
        with patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            self.assertEqual(runtime.launch(VERSION, "linux", self.root, arguments), 0)
        call = run.call_args
        receipt = runtime.load_install(self.root, VERSION, "linux")
        self.assertEqual(call.args[0], [receipt["launcher"], *arguments])
        self.assertEqual(call.kwargs["env"]["FASTDIFF_GUI_EXE"], receipt["gui"])
        self.assertNotIn("shell", call.kwargs)

    def test_wsl_pairing_and_windows_gui_override(self) -> None:
        windows_root = self.parent / "Windows LocalAppData" / "fastdiff"
        with patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "wsl_windows_root", return_value=windows_root):
            runtime.setup(VERSION, "wsl", self.root)
        receipt = runtime.load_install(self.root, VERSION, "wsl")
        self.assertTrue(Path(receipt["windows_gui"]).is_relative_to(windows_root))
        with patch.object(runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            runtime.launch(VERSION, "wsl", self.root, ["--native", "--", "repo"])
        self.assertEqual(run.call_args.kwargs["env"]["FASTDIFF_WINDOWS_EXE"], receipt["windows_gui"])

    def test_mismatched_wsl_revision_keeps_previous_receipt(self) -> None:
        windows_root = self.parent / "Windows data" / "fastdiff"
        with patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "wsl_windows_root", return_value=windows_root):
            runtime.setup(VERSION, "wsl", self.root)
        before = (self.root / "receipt.json").read_bytes()
        def mismatch(version, names, stage):
            fake_download(version, names, stage, windows_revision="different")
        with patch.object(runtime, "download_assets", side_effect=mismatch), \
                patch.object(runtime, "wsl_windows_root", return_value=windows_root):
            with self.assertRaisesRegex(ValueError, "revisions do not match"):
                runtime.setup("0.2.0", "wsl", self.root)
        self.assertEqual((self.root / "receipt.json").read_bytes(), before)

    def test_archives_reject_traversal_and_links(self) -> None:
        package = f"fastdiff-{VERSION}-linux-x64"
        for name, symbolic in [(f"{package}/../escape", False), (f"{package}/bin/fastdiff", True)]:
            archive = self.parent / "unsafe.tar.gz"
            with tarfile.open(archive, "w:gz") as writer:
                member = tarfile.TarInfo(name)
                if symbolic:
                    member.type = tarfile.SYMTYPE
                    member.linkname = "/tmp/escape"
                    writer.addfile(member)
                else:
                    member.size = 1
                    writer.addfile(member, io.BytesIO(b"x"))
            with self.subTest(name=name), self.assertRaises(ValueError):
                runtime.extract_archive(archive, self.parent / "extract", package)


if __name__ == "__main__":
    unittest.main()
