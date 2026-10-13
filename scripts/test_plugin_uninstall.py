"""Offline environment-local uninstall checks."""

import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
from contextlib import nullcontext
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

import test_plugin_command as command_tests
from test_plugin_runtime import fake_download, runtime, VERSION

MODULE = Path(__file__).resolve().parents[1] / "scripts/runtime/lifecycle.py"
SPEC = importlib.util.spec_from_file_location("fastdiff_lifecycle", MODULE)
lifecycle = importlib.util.module_from_spec(SPEC)
sys.path.insert(0, str(MODULE.parent))
SPEC.loader.exec_module(lifecycle)


class UninstallTests(unittest.TestCase):
    install = command_tests.InstalledCommandTests.install

    def setUp(self):
        command_tests.InstalledCommandTests.setUp(self)
        self.module_runtime = patch.object(lifecycle, "runtime", runtime)
        self.module_runtime.start()
        self.addCleanup(self.module_runtime.stop)

    def test_removes_owned_install_and_preserves_unrelated_files(self):
        self.install()
        unrelated = self.root / "notes.txt"
        unrelated.write_text("keep")
        other_version = self.root / "versions" / "my-app"
        other_version.mkdir()
        lifecycle.uninstall("linux", self.root)
        self.assertEqual(unrelated.read_text(), "keep")
        self.assertTrue(other_version.is_dir())
        self.assertFalse(self.wrapper.exists())
        self.assertFalse((self.root / "receipt.json").exists())
        self.assertFalse((self.root / "runtimes").exists())

    def test_legacy_runtime_metadata_remains_readable_for_uninstall(self):
        receipt = self.install()
        directory = Path(receipt["command"]).parents[1]
        legacy = directory / ".codex-plugin" / "plugin.json"
        legacy.parent.mkdir()
        (directory / "runtime.json").replace(legacy)
        lifecycle.uninstall("linux", self.root)
        self.assertFalse(directory.exists())
        self.assertFalse(self.wrapper.exists())

    def test_invalid_generic_runtime_metadata_does_not_fall_back_to_legacy(self):
        receipt = self.install()
        directory = Path(receipt["command"]).parents[1]
        legacy = directory / ".codex-plugin" / "plugin.json"
        legacy.parent.mkdir()
        legacy.write_text(json.dumps({"version": VERSION}))
        (directory / "runtime.json").write_text(json.dumps({"version": "9.9.9"}))
        with self.assertRaisesRegex(ValueError, "runtime version mismatch"):
            lifecycle.uninstall("linux", self.root)
        self.assertTrue(directory.exists())
        self.assertTrue(self.wrapper.exists())
        self.assertTrue((self.root / "receipt.json").exists())

    def test_receipt_paths_do_not_control_removal(self):
        receipt = self.install()
        outside = self.root.parent / "keep"
        outside.mkdir()
        (outside / "binary").write_text("keep")
        runtime.write_receipt(self.root, {**receipt, "launcher": str(outside / "binary"),
                                        "command": str(outside / "binary")})
        lifecycle.uninstall("linux", self.root)
        self.assertEqual((outside / "binary").read_text(), "keep")

    def test_wsl_uninstall_never_changes_windows_install(self):
        windows = self.root.parent / "windows"
        with patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "wsl_windows_root", return_value=windows):
            runtime.setup(VERSION, "wsl", self.root)
        before = sorted(str(path) for path in windows.rglob("*"))
        with patch.object(runtime, "wsl_windows_root", side_effect=AssertionError("Windows access")), \
                patch.object(runtime, "remove_windows_path", side_effect=AssertionError("Windows PATH")):
            lifecycle.uninstall("wsl", self.root)
        self.assertEqual(sorted(str(path) for path in windows.rglob("*")), before)

    def test_conflicts_and_linked_roots_fail_before_removal(self):
        self.install()
        self.wrapper.write_text("user-owned")
        with self.assertRaisesRegex(ValueError, "user-owned"):
            lifecycle.uninstall("linux", self.root)
        self.assertTrue((self.root / "receipt.json").is_file())
        if os.name != "nt":
            link = self.root.parent / "linked"
            link.symlink_to(self.root, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "link|junction"):
                lifecycle.uninstall("linux", link)

    @unittest.skipIf(os.name == "nt", "Creating symlinks requires Windows privileges")
    def test_real_entrypoint_preserves_linked_root_for_uninstall_checks(self):
        receipt = self.install()
        link = self.root.parent / "linked"
        link.symlink_to(self.root, target_is_directory=True)
        entry = link / Path(receipt["command"]).relative_to(self.root)
        output = subprocess.run([sys.executable, str(entry), "--uninstall"],
                                capture_output=True, text=True)
        self.assertEqual(output.returncode, 1)
        self.assertIn("symlink or junction", output.stderr)
        self.assertTrue(self.wrapper.is_file())
        self.assertTrue((self.root / "receipt.json").is_file())

    @unittest.skipUnless(os.name == "nt", "Windows wrapper requires CMD and the Windows registry")
    def test_real_windows_wrapper_can_uninstall_itself(self):
        # Use the real root-local command location; setup only stages fixture executables.
        with patch.object(runtime, "command_path", side_effect=lambda host, root: root / "bin" / "dff.cmd"), \
                patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "add_windows_path"):
            runtime.setup(VERSION, "windows", self.root)
        wrapper = self.root / "bin" / "dff.cmd"
        command_line = subprocess.list2cmdline([str(wrapper), "--uninstall"])
        output = subprocess.run(
            f'"{os.environ["COMSPEC"]}" /d /s /c "{command_line}"',
            capture_output=True, text=True,
        )
        self.assertEqual(output.returncode, 0, output.stderr)
        self.assertIn("Uninstalled fastdiff", output.stdout)
        self.assertFalse(wrapper.exists())
        self.assertFalse(self.root.exists())

    def test_nested_symlink_is_rejected_without_deleting_install(self):
        self.install()
        if os.name == "nt":
            self.skipTest("Creating symlinks requires Windows privileges")
        directory = next((self.root / "versions").iterdir())
        (directory / "linked").symlink_to(self.root.parent, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "link|junction"):
            lifecycle.uninstall("linux", self.root)
        self.assertTrue(self.wrapper.exists())

    def test_conflicting_metadata_fails_before_removal(self):
        self.install()
        directory = next((self.root / "versions").iterdir())
        metadata = directory / "build-info.json"
        data = runtime.read_json(metadata)
        metadata.write_text(json.dumps({**data, "version": "9.9.9"}))
        with self.assertRaisesRegex(ValueError, "mismatch"):
            lifecycle.uninstall("linux", self.root)
        self.assertTrue(self.wrapper.exists())
        self.assertTrue((self.root / "receipt.json").exists())

    def test_reparse_point_is_rejected_on_python_311(self):
        attributes = SimpleNamespace(st_file_attributes=0x400, st_mode=0o40755, st_uid=0)
        with patch.object(Path, "lstat", return_value=attributes):
            with self.assertRaisesRegex(ValueError, "link|junction"):
                lifecycle.check_path(self.root)

    def test_windows_path_only_removes_exact_entry(self):
        directory = self.root / "bin"
        original = f"first;{directory};{directory}/other;last;"
        registry = SimpleNamespace(
            HKEY_CURRENT_USER=1, REG_EXPAND_SZ=2,
            CreateKey=Mock(side_effect=lambda *_: nullcontext("key")),
            QueryValueEx=Mock(return_value=(original, 2)), SetValueEx=Mock(),
        )
        with patch.dict(sys.modules, {"winreg": registry}), \
                patch.object(runtime, "notify_windows_environment") as notify:
            runtime.remove_windows_path(directory)
        registry.SetValueEx.assert_called_once_with("key", "Path", 0, 2, f"first;{directory}/other;last;")
        notify.assert_called_once()

    def windows_install(self):
        """Install the fixture package as the Windows host, with the command under the root."""
        with patch.object(runtime, "command_path", side_effect=lambda host, root: root / "bin" / "dff.cmd"), \
                patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "add_windows_path"):
            runtime.setup(VERSION, "windows", self.root)

    def windows_uninstall(self):
        with patch.object(runtime, "command_path", side_effect=lambda host, root: root / "bin" / "dff.cmd"), \
                patch.object(runtime, "remove_windows_path") as remove_path:
            lifecycle.uninstall("windows", self.root)
        return remove_path

    def make_ipc_state(self) -> Path:
        state = self.root / "run"
        state.mkdir()
        (state / "instance.lock").write_bytes(b"")
        (state / "endpoint.json").write_text("{}", encoding="utf-8")
        return state

    def test_windows_uninstall_removes_ipc_state_left_by_the_app(self):
        self.windows_install()
        self.make_ipc_state()
        self.windows_uninstall()
        self.assertFalse(self.root.exists())

    def test_windows_uninstall_leaves_redirected_ipc_state_in_place(self):
        self.windows_install()
        state = self.make_ipc_state()
        real_lstat = Path.lstat

        def lstat(path, *args, **kwargs):
            status = real_lstat(path, *args, **kwargs)
            if path == state:
                return SimpleNamespace(st_mode=status.st_mode, st_uid=status.st_uid, st_file_attributes=0x400)
            return status

        with patch.object(Path, "lstat", lstat), patch("sys.stderr", new_callable=io.StringIO) as output:
            self.windows_uninstall()
        self.assertIn("symlink or junction", output.getvalue())
        self.assertEqual(sorted(path.name for path in state.iterdir()), ["endpoint.json", "instance.lock"])
        self.assertEqual([path.name for path in self.root.iterdir()], ["run"])

    def test_non_windows_uninstall_never_removes_a_run_directory(self):
        self.install()
        user_directory = self.root / "run"
        user_directory.mkdir()
        (user_directory / "notes.txt").write_text("keep")
        lifecycle.uninstall("linux", self.root)
        self.assertEqual((user_directory / "notes.txt").read_text(), "keep")

    def test_locked_files_report_failure(self):
        self.install()
        with patch.object(lifecycle.shutil, "rmtree", side_effect=PermissionError("Close fastdiff")):
            with self.assertRaises(PermissionError):
                lifecycle.uninstall("linux", self.root)
        self.assertTrue(self.wrapper.exists())


if __name__ == "__main__":
    unittest.main()
