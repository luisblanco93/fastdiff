"""Offline checks for stable release discovery and receipt-based launch."""

from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import test_plugin_runtime as runtime_tests
from test_plugin_runtime import VERSION, fake_download, runtime



class ReleaseUpdateTests(unittest.TestCase):
    setUp = runtime_tests.PluginRuntimeTests.setUp

    def test_setup_repairs_invalid_receipt_without_weakening_valid_downgrade_check(self):
        for contents in ('{"version":', '{}'):
            self.root.mkdir(parents=True, exist_ok=True)
            (self.root / "receipt.json").write_text(contents)
            with self.subTest(contents=contents), patch.object(runtime, "download_assets", side_effect=fake_download):
                runtime.setup(VERSION, "linux", self.root)
            self.assertEqual(runtime.load_install(self.root, VERSION, "linux")["version"], VERSION)

    def test_latest_release_requires_a_stable_published_tag(self):
        for metadata in [
            {"tag_name": "v0.1.3", "draft": False, "prerelease": False},
            {"tag_name": "v0.1.3-beta", "draft": False, "prerelease": False},
            {"tag_name": "v0.1.3", "draft": True, "prerelease": False},
            {"tag_name": "v0.1.3", "draft": False, "prerelease": True},
            [],
        ]:
            with self.subTest(metadata=metadata), patch.object(runtime, "fetch_json", return_value=metadata) as fetch:
                if metadata == {"tag_name": "v0.1.3", "draft": False, "prerelease": False}:
                    self.assertEqual(runtime.latest_version(), "0.1.3")
                    fetch.assert_called_once_with("https://api.github.com/repos/luisblanco93/fastdiff/releases/latest")
                else:
                    with self.assertRaises(ValueError):
                        runtime.latest_version()

    def test_source_revision_rejects_invalid_commit_metadata(self):
        revision = "a" * 40
        with patch.object(runtime, "fetch_json", return_value={"sha": revision}) as fetch:
            self.assertEqual(runtime.source_revision(), revision)
        fetch.assert_called_once_with("https://api.github.com/repos/luisblanco93/fastdiff/commits/main")
        for metadata in [{}, [], {"sha": "main"}, {"sha": "../escape"}, {"sha": 123}]:
            with self.subTest(metadata=metadata), patch.object(runtime, "fetch_json", return_value=metadata):
                with self.assertRaises(ValueError):
                    runtime.source_revision()

    def test_update_runs_pinned_current_installer_without_host_registration(self):
        revision = "a" * 40
        with patch.object(runtime, "source_revision", return_value=revision), patch.object(runtime, "download_file") as download, patch.object(runtime.subprocess, "run") as run:
            runtime.update("linux", self.root)
        installer = download.call_args.args[1]
        self.assertEqual(download.call_args.args[0], f"https://raw.githubusercontent.com/luisblanco93/fastdiff/{revision}/install.py")
        run.assert_called_once_with([runtime.sys.executable, str(installer), "--revision", revision, "--root", str(self.root)], check=True)

    def test_current_update_refreshes_runtime_without_downloading_binaries(self):
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(VERSION, "linux", self.root)
        previous = runtime.load_install(self.root, VERSION, "linux")["command"]
        with patch.object(runtime, "source_revision", return_value="a" * 40), patch.object(runtime, "download_file"), patch.object(runtime, "download_assets") as download, patch.object(runtime.subprocess, "run", side_effect=lambda *args, **kwargs: runtime.setup(VERSION, "linux", self.root)):
            runtime.update("linux", self.root)
        download.assert_not_called()
        current = runtime.load_install(self.root, VERSION, "linux")["command"]
        self.assertNotEqual(previous, current)
        self.assertTrue(Path(current).is_file())

    def test_setup_rejects_downgrade_before_downloading(self):
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup("0.2.0", "linux", self.root)
        before = (self.root / "receipt.json").read_bytes()
        with patch.object(runtime, "download_assets") as download:
            with self.assertRaisesRegex(ValueError, "downgrade"):
                runtime.setup(VERSION, "linux", self.root)
        download.assert_not_called()
        self.assertEqual((self.root / "receipt.json").read_bytes(), before)

    def test_failed_update_download_preserves_installation(self):
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(VERSION, "linux", self.root)
        before = (self.root / "receipt.json").read_bytes(), self.parent.joinpath("bin", "dff").read_bytes()
        with patch.object(runtime, "source_revision", return_value="a" * 40), patch.object(runtime, "download_file", side_effect=OSError("download interrupted")), patch.object(runtime.subprocess, "run") as run:
            with self.assertRaisesRegex(OSError, "download interrupted"):
                runtime.update("linux", self.root)
        run.assert_not_called()
        self.assertEqual((self.root / "receipt.json").read_bytes(), before[0])
        self.assertEqual(self.parent.joinpath("bin", "dff").read_bytes(), before[1])

    def test_update_installer_failure_preserves_previous_install(self):
        with patch.object(runtime, "download_assets", side_effect=fake_download):
            runtime.setup(VERSION, "linux", self.root)
        before = (self.root / "receipt.json").read_bytes(), self.parent.joinpath("bin", "dff").read_bytes()
        with patch.object(runtime, "source_revision", return_value="a" * 40), patch.object(runtime, "download_file"), patch.object(runtime.subprocess, "run", side_effect=subprocess.CalledProcessError(1, ["install.py"])):
            with self.assertRaises(subprocess.CalledProcessError):
                runtime.update("linux", self.root)
        self.assertEqual((self.root / "receipt.json").read_bytes(), before[0])
        self.assertEqual(self.parent.joinpath("bin", "dff").read_bytes(), before[1])

    def test_asset_download_uses_public_release_urls_without_subprocess(self):
        stage = self.parent / "assets"
        name = runtime.archive_name(VERSION, "linux")
        with patch.object(runtime, "download_file") as download, patch.object(runtime.subprocess, "run") as run:
            runtime.download_assets(VERSION, [name], stage)
        urls = [call.args[0] for call in download.call_args_list]
        self.assertCountEqual(urls, [f"https://github.com/luisblanco93/fastdiff/releases/download/v{VERSION}/{asset}" for asset in [name, "SHA256SUMS"]])
        self.assertCountEqual([call.args[1] for call in download.call_args_list], [stage / name, stage / "SHA256SUMS"])
        run.assert_not_called()

    def test_asset_download_rejects_unsafe_names(self):
        for name in ["../escape", "/absolute", "file?token", "..\\escape", ""]:
            with self.subTest(name=name), patch.object(runtime, "download_file") as download:
                with self.assertRaisesRegex(ValueError, "Invalid release asset"):
                    runtime.download_assets(VERSION, [name], self.parent)
            download.assert_not_called()



if __name__ == "__main__":
    unittest.main()
