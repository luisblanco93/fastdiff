"""Offline checks for maintainer version edits and rollback."""

import importlib.util
from pathlib import Path
import re
import shutil
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


bump = load("bump_version", ROOT / "scripts/bump-version.py")
packager = load("package_release", ROOT / "scripts/package-release.py")


class VersionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for relative in (
            "Cargo.toml", "Cargo.lock", "crates",
        ):
            source, target = ROOT / relative, self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            if source.is_dir():
                # Cargo metadata is enough; keep fixtures independent of application source.
                for manifest in source.glob("*/Cargo.toml"):
                    destination = target / manifest.relative_to(source)
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(manifest, destination)
            else:
                shutil.copyfile(source, target)
        self.old = packager.release_version(self.root, None)
        major, minor, patch_version = map(int, self.old.split("."))
        self.new = f"{major}.{minor}.{patch_version + 1}"

    def snapshot(self):
        return {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}

    def test_bump_preserves_dependency_versions_and_formatting(self):
        lock = self.root / "Cargo.lock"
        extra = f'\n[[package]]\nname = "unrelated-fixture"\nversion = "{self.old}"\n'
        lock.write_text(lock.read_text() + extra, encoding="utf-8")
        before = self.snapshot()
        self.assertEqual(set(bump.prepare_edits(self.root, self.new)), {
            self.root / "Cargo.toml", self.root / "Cargo.lock",
        })
        self.assertTrue(bump.bump_version(self.root, self.new))
        self.assertEqual(packager.release_version(self.root, f"v{self.new}"), self.new)
        self.assertTrue(lock.read_text().endswith(extra))
        for path, original in before.items():
            updated = path.read_bytes()
            if path != lock:
                self.assertEqual(updated.replace(self.new.encode(), self.old.encode()), original)
        after = self.snapshot()
        with patch.object(Path, "write_bytes", side_effect=AssertionError("no writes")):
            self.assertFalse(bump.bump_version(self.root, self.new))
        self.assertEqual(self.snapshot(), after)

    def test_invalid_and_lower_versions_do_not_write(self):
        before = self.snapshot()
        for version in ("v0.1.3", "01.2.3", "0.1.3-beta", "0.1", "0.0.0"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                bump.bump_version(self.root, version)
            self.assertEqual(self.snapshot(), before)

    def test_crlf_and_commented_toml_headers_are_preserved(self):
        cargo = self.root / "Cargo.toml"
        original = cargo.read_bytes().replace(
            b"[workspace.package]", b"[workspace.package] # release metadata"
        ).replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
        cargo.write_bytes(original)
        self.assertTrue(bump.bump_version(self.root, self.new))
        self.assertEqual(
            cargo.read_bytes(), original.replace(self.old.encode(), self.new.encode(), 1)
        )
        self.assertEqual(packager.release_version(self.root, None), self.new)

    def test_inconsistent_lockfile_fails_before_writing(self):
        path = self.root / "Cargo.lock"
        original = path.read_bytes()
        for newline in ("\n", "\r\n"):
            with self.subTest(newline=newline):
                text = original.decode("utf-8").replace("\r\n", "\n").replace("\n", newline)
                for name in bump.packager.workspace_packages(self.root):
                    marker = re.compile(
                        rf'(?m)(^name = "{re.escape(name)}"\r?\nversion = "){re.escape(self.old)}("\r?$)'
                    )
                    if marker.search(text):
                        text = marker.sub(r'\g<1>999.0.0\g<2>', text, count=1)
                        break
                else:
                    self.fail("no local workspace package found in Cargo.lock")
                path.write_bytes(text.encode("utf-8"))
                before = self.snapshot()
                with self.assertRaisesRegex(ValueError, "Version mismatch"):
                    bump.bump_version(self.root, self.new)
                self.assertEqual(self.snapshot(), before)
                with self.assertRaisesRegex(ValueError, "Version mismatch"):
                    packager.release_version(self.root, None)
                self.assertEqual(self.snapshot(), before)
                path.write_bytes(original)

    def test_failed_write_restores_every_original(self):
        before = self.snapshot()
        write = Path.write_bytes
        calls = 0

        def fail_once(path, data):
            nonlocal calls
            calls += 1
            if calls == 2:
                write(path, b"partial write")
                raise OSError("fixture write failure")
            return write(path, data)

        with patch.object(Path, "write_bytes", fail_once):
            with self.assertRaisesRegex(OSError, "fixture write failure"):
                bump.bump_version(self.root, self.new)
        self.assertEqual(self.snapshot(), before)


if __name__ == "__main__":
    unittest.main()
