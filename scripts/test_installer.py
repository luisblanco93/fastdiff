"""Offline checks for the stable source installer; no live GitHub requests."""

import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from contextlib import contextmanager, redirect_stderr
from unittest.mock import Mock, patch

MODULE = Path(__file__).resolve().parents[1] / "install.py"
SPEC = importlib.util.spec_from_file_location("fastdiff_bootstrap", MODULE)
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)
REVISION = "a" * 40


class InstallerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="fastdiff installer test ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name) / "fastdiff"
        self.runtime = Mock()
        self.runtime.host_platform.return_value = "linux"
        self.runtime.latest_version.return_value = "0.1.3"
        self.runtime.data_root.return_value = self.root

    def test_default_install_resolves_main_once(self):
        with patch.object(installer, "source_revision", return_value=REVISION) as resolve, patch.object(installer, "install") as install:
            self.assertEqual(installer.main([]), 0)
        resolve.assert_called_once_with()
        install.assert_called_once_with(REVISION, None)

    def test_source_revision_uses_public_api_and_validates_commit(self):
        for metadata in [{"sha": REVISION}, {}, [], {"sha": "main"}, {"sha": 123}]:
            response = io.BytesIO(json.dumps(metadata).encode())
            with self.subTest(metadata=metadata), patch.object(installer, "urlopen", return_value=response) as request:
                if metadata == {"sha": REVISION}:
                    self.assertEqual(installer.source_revision(), REVISION)
                else:
                    with self.assertRaises(ValueError):
                        installer.source_revision()
            self.assertEqual(request.call_args.args[0].full_url, "https://api.github.com/repos/luisblanco93/fastdiff/commits/main")

    def test_download_creates_parents_and_preserves_existing_files(self):
        path = self.root / "scripts" / "runtime.py"
        url = f"https://raw.githubusercontent.com/{installer.REPOSITORY}/{REVISION}/runtime.py"
        with patch.object(installer, "urlopen", return_value=io.BytesIO(b"installer source")):
            installer.download_file(url, path)
        self.assertEqual(path.read_bytes(), b"installer source")
        with patch.object(installer, "urlopen", return_value=io.BytesIO(b"replacement")):
            with self.assertRaises(FileExistsError):
                installer.download_file(url, path)
        self.assertEqual(path.read_bytes(), b"installer source")

    def test_pinned_update_preserves_root_without_resolving_main_again(self):
        with patch.object(installer, "source_revision") as resolve, patch.object(installer, "install") as install:
            self.assertEqual(installer.main(["--revision", REVISION, "--root", str(self.root)]), 0)
        resolve.assert_not_called()
        install.assert_called_once_with(REVISION, self.root)

    def test_legacy_update_root_maps_to_the_fastdiff_sibling(self):
        legacy = self.root.parent / "diffcode"
        with patch.object(installer, "source_revision") as resolve, patch.object(installer, "install") as install:
            self.assertEqual(installer.main(["--revision", REVISION, "--root", str(legacy)]), 0)
        resolve.assert_not_called()
        install.assert_called_once_with(REVISION, self.root)

    def test_relative_legacy_root_is_still_rejected(self):
        with patch.object(installer, "install") as install, redirect_stderr(io.StringIO()):
            self.assertEqual(installer.main(["--revision", REVISION, "--root", "relative/diffcode"]), 1)
        install.assert_not_called()

    def test_invalid_root_and_revision_fail_before_installation(self):
        for arguments in [["--revision", "main"], ["--revision", "../escape"], ["--root", "relative/fastdiff"], ["--root", str(self.root.parent / "unrelated")]]:
            with self.subTest(arguments=arguments), patch.object(installer, "install") as install, patch.object(installer, "source_revision") as resolve, redirect_stderr(io.StringIO()):
                self.assertEqual(installer.main(arguments), 1)
            install.assert_not_called()
            resolve.assert_not_called()

    def test_downloads_only_generic_runtime_files_from_pinned_revision(self):
        with patch.object(installer, "download_file") as download, patch.object(installer, "load_module", return_value=self.runtime) as load:
            installer.install(REVISION, self.root)
        expected = ["scripts/runtime/runtime.py", "scripts/runtime/command.py", "scripts/runtime/lifecycle.py"]
        prefix = f"https://raw.githubusercontent.com/{installer.REPOSITORY}/{REVISION}/"
        self.assertEqual([call.args[0] for call in download.call_args_list], [prefix + path for path in expected])
        for call, relative in zip(download.call_args_list, expected):
            self.assertTrue(call.args[1].as_posix().endswith("/" + relative))
        self.runtime.setup.assert_called_once_with("0.1.3", "linux", self.root)
        self.runtime.data_root.assert_not_called()
        self.assertEqual(load.call_count, 1)
        self.assertTrue(load.call_args.args[1].as_posix().endswith("/scripts/runtime/runtime.py"))

    def test_removed_host_option_fails_before_any_download(self):
        with patch.object(installer, "download_file") as download, patch.object(installer, "source_revision", return_value=REVISION), redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit) as error:
                installer.main(["--host", "codex"])
        self.assertEqual(error.exception.code, 2)
        download.assert_not_called()

    def test_install_without_root_uses_platform_data_root(self):
        with patch.object(installer, "download_file"), patch.object(installer, "load_module", return_value=self.runtime):
            installer.install(REVISION, None)
        self.runtime.data_root.assert_called_once_with("linux")
        self.runtime.setup.assert_called_once_with("0.1.3", "linux", self.root)

    def test_source_download_failure_does_not_touch_existing_install(self):
        self.root.mkdir()
        receipt = self.root / "receipt.json"
        receipt.write_bytes(b"existing receipt")
        with patch.object(installer, "download_file", side_effect=OSError("download interrupted")), patch.object(installer, "load_module") as load:
            with self.assertRaisesRegex(OSError, "download interrupted"):
                installer.install(REVISION, self.root)
        load.assert_not_called()
        self.assertEqual(receipt.read_bytes(), b"existing receipt")

    def test_app_install_failure_is_reported(self):
        self.runtime.setup.side_effect = OSError("checksum failed")
        with patch.object(installer, "download_file"), patch.object(installer, "load_module", return_value=self.runtime) as load:
            with self.assertRaisesRegex(OSError, "checksum failed"):
                installer.install(REVISION, self.root)

    @contextmanager
    def local_sources(self):
        """Run the real installer against this checkout's runtime files; yields the loaded runtimes."""
        from test_plugin_runtime import VERSION, fake_download

        original_load = installer.load_module
        sources = MODULE.parent
        loaded = []

        def local_download(url, destination):
            relative = url.split(f"/{REVISION}/", 1)[1]
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(sources / relative, destination)

        def local_load(name, path):
            module = original_load(name, path)
            if name == "fastdiff_installer_runtime":
                module.host_platform = lambda: "windows" if os.name == "nt" else "linux"
                module.latest_version = lambda: VERSION
                module.download_assets = Mock(side_effect=fake_download)
                module.command_path = lambda host, root: root / "bin" / ("dff.cmd" if host == "windows" else "dff")
                module.add_windows_path = Mock()
                loaded.append(module)
            return module

        with patch.object(installer, "download_file", side_effect=local_download), patch.object(installer, "load_module", side_effect=local_load):
            yield loaded

    def test_real_installer_leaves_host_skill_untouched_and_refreshes_app(self):
        from test_plugin_runtime import VERSION

        existing_skill = self.root.parent / "agent skills" / "fastdiff" / "SKILL.md"
        existing_skill.parent.mkdir(parents=True)
        existing_skill.write_text("host-managed skill", encoding="utf-8")
        with self.local_sources() as loaded:
            installer.install(REVISION, self.root)
            installer.install(REVISION, self.root)
        self.assertEqual(loaded[0].download_assets.call_count, 1)
        loaded[1].download_assets.assert_not_called()
        self.assertEqual(existing_skill.read_text(encoding="utf-8"), "host-managed skill")
        receipt = json.loads((self.root / "receipt.json").read_text())
        self.assertNotIn("skills_dirs", receipt)
        host = "windows" if os.name == "nt" else "linux"
        command = loaded[1].command_path(host, self.root)
        argv = ["cmd.exe", "/c", str(command), "--version"] if os.name == "nt" else [str(command), "--version"]
        result = subprocess.run(argv, check=True, capture_output=True, text=True)
        self.assertEqual(result.stdout.strip(), VERSION)

    def test_install_removes_legacy_diffcode_binaries_and_runtimes(self):
        from test_plugin_runtime import make_legacy_install

        legacy = make_legacy_install(self.root, platform="windows" if os.name == "nt" else "linux")
        notes = legacy / "notes.txt"
        notes.write_text("keep", encoding="utf-8")
        with self.local_sources():
            installer.install(REVISION, self.root)
        self.assertEqual(sorted(path.name for path in legacy.iterdir()), ["notes.txt"])
        self.assertEqual(notes.read_text(encoding="utf-8"), "keep")
        self.assertTrue((self.root / "receipt.json").is_file())
        self.assertTrue(any((self.root / "versions").glob("*/bin/fastdiff*")))
        self.assertEqual([], [path for path in self.root.rglob("*") if path.name.startswith("diffcode")])


if __name__ == "__main__":
    unittest.main()
