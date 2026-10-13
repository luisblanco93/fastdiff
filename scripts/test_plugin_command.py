"""Offline stable-command checks; no downloads or desktop required."""

import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from contextlib import nullcontext
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from test_plugin_runtime import fake_download, runtime, VERSION


class InstalledCommandTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="fastdiff command ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / "data" / "fastdiff"
        self.wrapper = Path(temporary.name) / "bin" / "dff"
        location = patch.object(runtime, "command_path", return_value=self.wrapper)
        location.start()
        self.addCleanup(location.stop)

    def install(self, version=VERSION):
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(version, "linux", self.root)
        return runtime.read_json(self.root / "receipt.json")

    def entry(self):
        receipt = self.install()
        spec = importlib.util.spec_from_file_location("installed_command", receipt["command"])
        entry = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(entry)
        return entry

    def test_staged_runtime_has_generic_metadata_without_host_files(self):
        receipt = self.install()
        directory = Path(receipt["command"]).parents[1]
        self.assertEqual(runtime.read_json(directory / "runtime.json"), {"version": VERSION})
        self.assertEqual({path.name for path in directory.iterdir()}, {"runtime.json", "scripts"})
        self.assertEqual({path.name for path in (directory / "scripts").iterdir()},
                         {"runtime.py", "command.py", "lifecycle.py"})

    def test_missing_install_reports_installer_repair_guidance(self):
        entry = self.entry()
        (self.root / "receipt.json").unlink()
        with patch("sys.stderr", new_callable=io.StringIO) as output:
            self.assertEqual(entry.main([]), 1)
        self.assertIn("Rerun the installer", output.getvalue())
        self.assertNotIn("dff-setup", output.getvalue())

    def test_help_and_version_are_offline(self):
        entry = self.entry()
        for flag in ("--help", "--h", "-h", "--version", "--v"):
            with self.subTest(flag=flag), patch("sys.stdout", new_callable=io.StringIO) as output, \
                    patch.object(entry.runtime, "host_platform", side_effect=AssertionError("offline")), \
                    patch.object(entry.runtime, "launch", side_effect=AssertionError("no GUI")):
                self.assertEqual(entry.main([flag]), 0)
                self.assertIn(VERSION if flag in ("--version", "--v") else "--uninstall", output.getvalue())

    def test_lifecycle_dispatch_and_invalid_combinations(self):
        entry = self.entry()
        for flag, module, method in (("--update", entry.runtime, "update"),
                                     ("--uninstall", entry.lifecycle, "uninstall")):
            with self.subTest(flag=flag), patch.object(entry.runtime, "host_platform", return_value="linux"), \
                    patch.object(module, method) as action:
                self.assertEqual(entry.main([flag]), 0)
                action.assert_called_once_with("linux", self.root)
            for extra in (["repo"], ["--file", "x"], ["--view", "all"], ["--native"], ["--theme"], ["--version"]):
                with self.subTest(flag=flag, extra=extra), self.assertRaises(SystemExit) as error:
                    entry.main([flag, *extra])
                self.assertEqual(error.exception.code, 2)

    def test_lifecycle_words_remain_repository_names(self):
        entry = self.entry()
        for repository in ("update", "version", "uninstall"):
            with patch.object(entry.runtime, "host_platform", return_value="linux"), \
                    patch.object(entry.runtime, "launch", return_value=0) as launch:
                self.assertEqual(entry.main([repository]), 0)
                self.assertEqual(launch.call_args.args[-1], ["--", repository])

    def test_document_and_unpushed_arguments_are_forwarded(self):
        entry = self.entry()
        with patch.object(entry.runtime, "host_platform", return_value="linux"), \
                patch.object(entry.runtime, "launch", return_value=0) as launch:
            self.assertEqual(entry.main(["notes with spaces.txt", "--view", "unpushed"]), 0)
            self.assertEqual(launch.call_args.args[-1],
                             ["--view", "unpushed", "--", "notes with spaces.txt"])

    def test_theme_is_forwarded_before_repository_separator(self):
        entry = self.entry()
        for host in ("linux", "wsl", "windows"):
            with self.subTest(host=host), \
                    patch.object(entry.runtime, "host_platform", return_value=host), \
                    patch.object(entry.runtime, "launch", return_value=0) as launch:
                self.assertEqual(entry.main(["repo with spaces", "--theme"]), 0)
                self.assertEqual(launch.call_args.args[-1], ["--theme", "--", "repo with spaces"])

    @unittest.skipIf(os.name == "nt", "POSIX wrapper requires a POSIX shell")
    def test_real_wrapper_version_and_help_are_offline(self):
        self.install()
        for flag, expected in (("--v", VERSION), ("--h", "--uninstall")):
            with self.subTest(flag=flag):
                output = subprocess.run([str(self.wrapper), flag], check=True, capture_output=True, text=True)
                self.assertIn(expected, output.stdout)

    def test_conflicting_command_preserved_before_download(self):
        self.wrapper.parent.mkdir()
        self.wrapper.write_text("#!/bin/sh\necho mine\n")
        with patch.object(runtime, "download_assets") as download:
            with self.assertRaisesRegex(ValueError, "user-owned"):
                runtime.setup(VERSION, "linux", self.root)
        download.assert_not_called()
        self.assertIn("echo mine", self.wrapper.read_text())

    @unittest.skipIf(os.name == "nt", "POSIX wrapper requires a POSIX shell")
    def test_wrapper_forwards_literal_arguments_from_staged_runtime(self):
        receipt = self.install()
        command = Path(receipt["command"])
        self.assertTrue(command.is_file())
        self.assertEqual(receipt["python"], str(Path(runtime.sys.executable).resolve()))
        # Replace only the staged entry point to observe the wrapper's exact argv.
        command.write_text("import json, sys\nprint(json.dumps(sys.argv[1:]))\n")
        arguments = ["/repo with spaces", "--file", "a;$(echo nope).rs", "--view", "staged"]
        output = subprocess.run([str(self.wrapper), *arguments], check=True, capture_output=True, text=True)
        self.assertEqual(json.loads(output.stdout), arguments)

    @unittest.skipUnless(os.name == "nt", "Windows wrapper requires CMD")
    def test_windows_wrapper_forwards_literal_arguments(self):
        fields, pending = runtime.stage_command(self.root, VERSION, "windows")
        command = Path(fields["command"])
        command.write_text("import json, sys\nprint(json.dumps(sys.argv[1:]))\n")
        wrapper = self.wrapper.with_suffix(".cmd")
        pending.replace(wrapper)
        arguments = ["repo with spaces", "--file", "a & 100% !literal!.rs", "--view", "staged"]
        command_line = subprocess.list2cmdline([str(wrapper), *arguments])
        output = subprocess.run(
            f'"{os.environ["COMSPEC"]}" /d /s /c "{command_line}"',
            check=True, capture_output=True, text=True,
        )
        self.assertEqual(json.loads(output.stdout), arguments)

    @unittest.skipUnless(os.name == "nt", "Windows wrapper requires CMD")
    def test_windows_wrapper_preserves_exit_code_after_self_deletion(self):
        wrapper = self.wrapper.with_suffix(".cmd")
        for exit_code in (0, 37):
            with self.subTest(exit_code=exit_code):
                fields, pending = runtime.stage_command(self.root, VERSION, "windows")
                Path(fields["command"]).write_text(
                    f"from pathlib import Path\nimport sys\nPath({str(wrapper)!r}).unlink()\nsys.exit({exit_code})\n"
                )
                pending.replace(wrapper)
                caller = self.root / "caller.cmd"
                caller.write_text(f'@echo off\ncall "{wrapper}"\n'
                                  'set "child_status=%errorlevel%"\n'
                                  'echo continued:%child_status%\nexit /b %child_status%\n')
                command_line = subprocess.list2cmdline([str(caller)])
                output = subprocess.run(
                    f'"{os.environ["COMSPEC"]}" /d /s /c "{command_line}"',
                    capture_output=True, text=True, cwd=self.root,
                )
                self.assertEqual(output.returncode, exit_code, output.stderr)
                self.assertIn(f"continued:{exit_code}", output.stdout)
                self.assertEqual(output.stderr, "")
                self.assertFalse(wrapper.exists())

    def test_failed_activation_keeps_previous_wrapper_and_receipt(self):
        self.install()
        wrapper = self.wrapper.read_bytes()
        receipt = (self.root / "receipt.json").read_bytes()
        with patch.object(runtime, "download_assets", side_effect=fake_download), \
                patch.object(runtime, "write_receipt", side_effect=OSError("interrupted")):
            with self.assertRaises(OSError):
                runtime.setup("0.2.0", "linux", self.root)
        self.assertEqual(self.wrapper.read_bytes(), wrapper)
        self.assertEqual((self.root / "receipt.json").read_bytes(), receipt)

    def test_stable_entry_uses_receipt_version_not_runtime_metadata(self):
        receipt = self.install()
        metadata = Path(receipt["command"]).parents[1] / "runtime.json"
        metadata.write_text(json.dumps({"version": "9.9.9"}))
        spec = importlib.util.spec_from_file_location("installed_command", receipt["command"])
        entry = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(entry)
        with patch.object(entry.runtime, "load_install", wraps=runtime.load_install) as load, \
                patch.object(entry.runtime.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
                patch.object(entry.runtime, "host_platform", return_value="linux"):
            self.assertEqual(entry.main(["--view", "unstaged"]), 0)
        self.assertEqual(load.call_args.args[1], VERSION)
        (self.root / "receipt.json").write_text('{}')
        self.assertEqual(entry.main([]), 1)

    def test_windows_wrapper_escapes_paths_and_preserves_user_path(self):
        root = self.root / "José & 100% ! data"
        fields, pending = runtime.stage_command(root, VERSION, "windows")
        body = pending.read_text(encoding="utf-8")
        self.assertIn("setlocal DisableDelayedExpansion", body)
        self.assertIn("chcp 65001 >nul", body)
        self.assertIn(fields["command"].replace("%", "%%"), body)
        self.assertNotIn(str(root), runtime.command_marker(root, "windows"))
        self.assertIn("%*", body)
        pending.unlink()

        registry = SimpleNamespace(
            HKEY_CURRENT_USER=1, REG_EXPAND_SZ=2,
            CreateKey=Mock(side_effect=lambda *_: nullcontext("key")),
            QueryValueEx=Mock(return_value=("user-first;user-second", 2)),
            SetValueEx=Mock(),
        )
        with patch.dict(sys.modules, {"winreg": registry}), \
                patch.object(runtime, "notify_windows_environment") as notify:
            runtime.add_windows_path(self.wrapper.parent)
            expected = "user-first;user-second;" + str(self.wrapper.parent)
            registry.SetValueEx.assert_called_once_with("key", "Path", 0, 2, expected)
            notify.assert_called_once()
            registry.QueryValueEx.return_value = (expected, 2)
            runtime.add_windows_path(self.wrapper.parent)
            self.assertEqual(registry.SetValueEx.call_count, 1)


if __name__ == "__main__":
    unittest.main()
