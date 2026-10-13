"""Offline checks for release tag, changelog, and branch validation."""

import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release_guard", ROOT / "scripts/release-guard.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)

IDENTITY = ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "-c", "commit.gpgsign=false"]


class ChangelogTests(unittest.TestCase):
    TEXT = (
        "# Changelog\n\n## [Unreleased]\n\n- Pending work\n\n"
        "## [1.0.0] - 2026-10-13\n\n- First release\n\n### Notes\n\nMore.\n\n"
        "## [0.9.0]\n\n"
    )

    def test_returns_body_until_next_version_heading(self):
        self.assertEqual(
            guard.changelog_section(self.TEXT, "1.0.0"),
            "- First release\n\n### Notes\n\nMore.",
        )

    def test_unreleased_never_satisfies_a_version(self):
        with self.assertRaisesRegex(ValueError, "no section for 2.0.0"):
            guard.changelog_section(self.TEXT, "2.0.0")

    def test_empty_and_duplicate_sections_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "empty"):
            guard.changelog_section(self.TEXT, "0.9.0")
        with self.assertRaisesRegex(ValueError, "more than one"):
            guard.changelog_section(self.TEXT + "## [1.0.0]\n\n- Again\n", "1.0.0")


class ReleaseCheckTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for name in ("Cargo.toml", "Cargo.lock"):
            shutil.copyfile(ROOT / name, self.root / name)
        for manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            target = self.root / manifest.relative_to(ROOT)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(manifest, target)
        self.version = guard.packager.release_version(self.root, None)
        (self.root / "CHANGELOG.md").write_text(
            f"## [Unreleased]\n\n## [{self.version}] - 2026-10-13\n\n- Release notes\n", encoding="utf-8"
        )
        self.git("init", "-b", "main")
        self.commit("release candidate")
        self.git("update-ref", "refs/remotes/origin/main", "HEAD")

    def git(self, *args):
        result = subprocess.run(["git", *IDENTITY, "-C", str(self.root), *args],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "--allow-empty", "-m", message)
        return self.git("rev-parse", "HEAD")

    def check(self, tag=None, ref="HEAD"):
        return guard.check_release(self.root, tag or f"v{self.version}", ref, "origin/main")

    def test_consistent_release_returns_notes(self):
        self.assertEqual(self.check(), "- Release notes")

    def test_tag_must_be_stable_and_match_the_workspace_version(self):
        for tag in ("v999.0.0", f"v{self.version}-rc.1", self.version, "vfoo"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                self.check(tag)

    def test_missing_changelog_or_section_is_rejected(self):
        changelog = self.root / "CHANGELOG.md"
        changelog.write_text("## [Unreleased]\n\n- Pending\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "no section"):
            self.check()
        changelog.unlink()
        with self.assertRaisesRegex(ValueError, "CHANGELOG.md is missing"):
            self.check()

    def test_commit_outside_main_is_rejected(self):
        self.git("checkout", "-b", "topic")
        sha = self.commit("unmerged work")
        with self.assertRaisesRegex(ValueError, "not part of origin/main"):
            self.check(ref=sha)

    def test_unknown_ref_is_an_error_not_a_pass(self):
        with self.assertRaisesRegex(ValueError, "Cannot compare"):
            self.check(ref="0" * 40)


if __name__ == "__main__":
    unittest.main()
