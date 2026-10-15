"""Offline checks for binary release checksums and version validation."""

import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "package_release", ROOT / "scripts/package-release.py"
)
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


class PackageTests(unittest.TestCase):
    def test_only_binary_archives_are_checksummed_and_remain_unchanged(self):
        version = packager.release_version(ROOT, None)
        with tempfile.TemporaryDirectory(prefix="fastdiff release ") as directory:
            output = Path(directory)
            names = [f"fastdiff-{version}-linux-x64.tar.gz", f"fastdiff-{version}-macos-arm64.tar.gz",
                     f"fastdiff-{version}-windows-x64.zip"]
            contents = [b"offline linux fixture", b"offline macos fixture", b"offline windows fixture"]
            for name, data in zip(names, contents):
                (output / name).write_bytes(data)
            checksums = packager.package(ROOT, output, f"v{version}")
            self.assertEqual(checksums, output / "SHA256SUMS")
            self.assertEqual(
                checksums.read_text(encoding="utf-8"),
                "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n"
                        for name, data in zip(names, contents)),
            )
            self.assertEqual({path.name for path in output.iterdir()}, {*names, "SHA256SUMS"})
            self.assertEqual([(output / name).read_bytes() for name in names], contents)

    def test_wrong_tag_fails_before_writing_checksums(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "does not match"):
                packager.package(ROOT, Path(directory), "v999.0.0")
            self.assertFalse(list(Path(directory).iterdir()))

    def test_missing_any_binary_archive_fails_before_writing(self):
        version = packager.release_version(ROOT, None)
        names = [f"fastdiff-{version}-linux-x64.tar.gz", f"fastdiff-{version}-macos-arm64.tar.gz",
                 f"fastdiff-{version}-windows-x64.zip"]
        for present in [None, *names]:
            with self.subTest(present=present), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                if present is not None:
                    (output / present).write_bytes(b"offline fixture")
                before = {path.name: path.read_bytes() for path in output.iterdir()}
                with self.assertRaisesRegex(ValueError, "Missing paired app archive"):
                    packager.package(ROOT, output)
                self.assertEqual({path.name: path.read_bytes() for path in output.iterdir()}, before)


if __name__ == "__main__":
    unittest.main()
