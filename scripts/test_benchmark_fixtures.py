"""Fixture generator checks; sizes stay tiny and nothing touches the checkout."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import benchmark_fixtures as fixtures
from benchmark_fixtures import CHECKOUT_ROOT, MARKER, Shape, check_destination, generate


def git(repository, *arguments):
    return subprocess.run(["git", *arguments], cwd=repository, check=True, capture_output=True,
                          text=True).stdout


class DestinationSafetyTests(unittest.TestCase):
    def test_refuses_the_checkout_and_everything_inside_it(self):
        for candidate in (CHECKOUT_ROOT, CHECKOUT_ROOT / "target" / "fixture",
                          CHECKOUT_ROOT / "scripts" / ".." / "docs" / "new"):
            with self.subTest(candidate=str(candidate)), self.assertRaises(ValueError):
                check_destination(candidate)
        self.assertFalse((CHECKOUT_ROOT / "target" / "fixture").exists())

    def test_checkout_root_can_be_injected_and_nothing_is_created_on_refusal(self):
        with tempfile.TemporaryDirectory() as directory:
            fake_checkout = Path(directory) / "checkout"
            fake_checkout.mkdir()
            target = fake_checkout / "out"
            with self.assertRaises(ValueError):
                generate(Shape(1, 3), target, checkout_root=fake_checkout)
            self.assertFalse(target.exists())

    def test_refuses_non_empty_directories_files_and_git_work_trees(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            busy = root / "busy"
            busy.mkdir()
            (busy / "keep.txt").write_text("user data")
            with self.assertRaises(ValueError):
                generate(Shape(1, 3), busy, checkout_root=root / "elsewhere")
            self.assertEqual((busy / "keep.txt").read_text(), "user data")
            plain = root / "file"
            plain.write_text("x")
            with self.assertRaises(ValueError):
                check_destination(plain, root / "elsewhere")
            work_tree = root / "work"
            work_tree.mkdir()
            git(work_tree, "init", "-q")
            with self.assertRaises(ValueError):
                check_destination(work_tree / "nested", root / "elsewhere")

    def test_accepts_an_empty_existing_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / "empty"
            destination.mkdir()
            self.assertEqual(check_destination(destination, CHECKOUT_ROOT), destination.resolve())


class GitEnvironmentTests(unittest.TestCase):
    POLLUTION = {"GIT_DIR": "/nonexistent/elsewhere/.git", "GIT_WORK_TREE": "/nonexistent/tree",
                 "GIT_INDEX_FILE": "/nonexistent/index", "GIT_OBJECT_DIRECTORY": "/nonexistent/o",
                 "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.worktree",
                 "GIT_CONFIG_VALUE_0": "/nonexistent", "GIT_CEILING_DIRECTORIES": "/",
                 "git_namespace": "lowercase", "GIT_AUTHOR_NAME": "Someone Else"}

    def test_no_inherited_git_variable_reaches_the_child(self):
        base = {"PATH": "/usr/bin", "HOME": "/home/u", "LANG": "C", **self.POLLUTION}
        env = fixtures.git_environment(base)
        for name in env:
            if name.upper().startswith("GIT_"):
                self.assertIn(name, {"GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_GLOBAL",
                                     "GIT_TERMINAL_PROMPT"}, name)
        self.assertEqual((env["PATH"], env["HOME"], env["LANG"]), ("/usr/bin", "/home/u", "C"))
        self.assertEqual(env["GIT_CONFIG_NOSYSTEM"], "1")
        self.assertEqual(env["GIT_CONFIG_GLOBAL"], os.devnull)
        self.assertEqual(env["GIT_TERMINAL_PROMPT"], "0")
        self.assertNotIn("git_namespace", env)

    def test_polluted_environment_cannot_redirect_the_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            other = Path(directory) / "other"
            other.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=other, check=True, capture_output=True)
            target = Path(directory) / "fixture"
            pollution = {**self.POLLUTION, "GIT_DIR": str(other / ".git"),
                         "GIT_WORK_TREE": str(other), "GIT_INDEX_FILE": str(other / ".git" / "index")}
            with patch.dict(os.environ, pollution):
                fixture = generate(Shape(2, 5), target, checkout_root=CHECKOUT_ROOT)
            self.assertTrue((target / ".git").is_dir())
            self.assertEqual(fixture.total_changed_files, 4)
            clean = {key: value for key, value in os.environ.items()
                     if not key.upper().startswith("GIT_")}
            tracked = subprocess.run(["git", "ls-files"], cwd=target, env=clean, check=True,
                                     capture_output=True, text=True).stdout.splitlines()
            self.assertEqual(len(tracked), 4)
            other_log = subprocess.run(["git", "log", "--oneline"], cwd=other, env=clean,
                                       capture_output=True, text=True)
            self.assertNotEqual(other_log.returncode, 0)  # Still an unborn repository.
            self.assertEqual(
                subprocess.run(["git", "ls-files"], cwd=other, env=clean, check=True,
                               capture_output=True, text=True).stdout, "")


class GenerationTests(unittest.TestCase):
    def generate(self, shape, name="repo"):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        destination = Path(directory.name) / name
        return destination, generate(shape, destination, checkout_root=CHECKOUT_ROOT)

    def test_counts_markers_and_modified_working_tree(self):
        destination, fixture = self.generate(Shape(changed_files=7, rows_per_file=25))
        self.assertEqual(fixture.changed_files, 7)
        self.assertEqual(fixture.total_changed_files, 9)
        tracked = git(destination, "ls-files").splitlines()
        self.assertEqual(len(tracked), 9)
        status = git(destination, "status", "--porcelain").splitlines()
        self.assertEqual(len(status), 9)
        self.assertTrue(all(line.startswith(" M") for line in status))  # Modified, not clean.
        self.assertEqual(len(git(destination, "log", "--oneline").splitlines()), 1)
        for name in fixture.marker_files:
            lines = (destination / name).read_text().splitlines()
            self.assertEqual(lines[0], MARKER)
            self.assertEqual(sum(MARKER in line for line in lines), 1)
            self.assertNotIn(MARKER, git(destination, "show", f"HEAD:{name}"))

    def test_marker_files_work_with_the_launch_profile_fixture(self):
        import launch_profile
        destination, fixture = self.generate(Shape(1, 3))
        with launch_profile.MarkerFixture(destination, fixture.marker_files[0]) as marker:
            marker.write("lit12-abc")
            self.assertIn("lit12-abc", (destination / fixture.marker_files[0]).read_text())
        self.assertTrue((destination / fixture.marker_files[0]).read_text().startswith(MARKER))

    def test_large_file_shape_has_one_changed_file_with_requested_rows(self):
        destination, fixture = self.generate(Shape(changed_files=1, rows_per_file=1000))
        changed = [name for name in git(destination, "ls-files").splitlines()
                   if name.startswith("src/")]
        self.assertEqual(len(changed), 1)
        self.assertEqual(len((destination / changed[0]).read_text().splitlines()), 1000)
        self.assertEqual(fixture.rows_per_file, 1000)

    def test_marked_changed_file_is_a_valid_marker_target_and_stays_modified(self):
        shape = Shape(changed_files=1, rows_per_file=50, marked_changed_files=1)
        destination, fixture = self.generate(shape)
        (large,) = [name for name in git(destination, "ls-files").splitlines()
                    if name.startswith("src/")]
        self.assertIn(large, fixture.marker_files)
        lines = (destination / large).read_text().splitlines()
        self.assertEqual(len(lines), 50)
        self.assertEqual(lines[0], MARKER)  # First line, once.
        self.assertEqual(sum(MARKER in line for line in lines), 1)
        self.assertNotIn(MARKER, git(destination, "show", f"HEAD:{large}"))
        self.assertIn(f" M {large}", git(destination, "status", "--porcelain").splitlines())
        # Counted once: the generated file plus the two standalone marker files.
        self.assertEqual(fixture.total_changed_files, 3)
        self.assertEqual(len(git(destination, "status", "--porcelain").splitlines()), 3)
        import launch_profile
        with launch_profile.MarkerFixture(destination, large) as marker:
            marker.write("lit12-abc")
            changed = (destination / large).read_text()
            self.assertTrue(changed.startswith("lit12-abc\n"))
            self.assertIn(f" M {large}", git(destination, "status", "--porcelain").splitlines())
        self.assertTrue((destination / large).read_text().startswith(MARKER))

    def test_unmarked_changed_files_keep_no_marker(self):
        destination, fixture = self.generate(Shape(changed_files=2, rows_per_file=12))
        self.assertEqual(fixture.marker_files, fixtures.MARKER_FILES)
        for name in git(destination, "ls-files").splitlines():
            if name.startswith("src/"):
                self.assertNotIn(MARKER, (destination / name).read_text())

    def test_marking_more_files_than_exist_is_rejected_before_touching_disk(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "repo"
            with self.assertRaises(ValueError):
                generate(Shape(1, 5, marked_changed_files=2), target, checkout_root=CHECKOUT_ROOT)
            self.assertFalse(target.exists())

    def test_content_is_deterministic(self):
        first, _ = self.generate(Shape(3, 12), "one")
        second, _ = self.generate(Shape(3, 12), "two")
        self.assertEqual(git(first, "rev-parse", "HEAD^{tree}"),
                         git(second, "rev-parse", "HEAD^{tree}"))
        self.assertEqual(git(first, "diff"), git(second, "diff"))

    def test_git_is_invoked_without_a_shell_and_with_local_identity(self):
        calls = []
        real = subprocess.run

        def spy(command, *args, **kwargs):
            calls.append((command, kwargs))
            return real(command, *args, **kwargs)
        with tempfile.TemporaryDirectory() as directory, \
                patch("benchmark_fixtures.subprocess.run", side_effect=spy):
            generate(Shape(1, 3), Path(directory) / "repo", checkout_root=CHECKOUT_ROOT)
        self.assertGreaterEqual(len(calls), 3)
        for command, kwargs in calls:
            self.assertIsInstance(command, list)
            self.assertEqual(command[0], "git")
            self.assertFalse(kwargs.get("shell", False))
            self.assertIn("user.name=fastdiff Benchmark", command)
            self.assertEqual(kwargs["env"]["GIT_CONFIG_NOSYSTEM"], "1")

    def test_invalid_shapes_are_rejected_before_touching_disk(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "repo"
            for shape in (Shape(0, 5), Shape(1, 0)):
                with self.subTest(shape=shape), self.assertRaises(ValueError):
                    generate(shape, target, checkout_root=CHECKOUT_ROOT)
            self.assertFalse(target.exists())

    def test_presets_cover_the_documented_shapes(self):
        self.assertEqual(fixtures.PRESETS["small"].changed_files, 2)
        self.assertEqual(fixtures.PRESETS["files-1k"].changed_files, 1000)
        self.assertEqual(fixtures.PRESETS["files-10k"].changed_files, 10000)
        # The one 100,000-row changed file must be usable as --file, so it is marked.
        self.assertEqual(fixtures.PRESETS["rows-100k"], Shape(1, 100000, marked_changed_files=1))
        for name in ("small", "files-1k", "files-10k"):
            self.assertEqual(fixtures.PRESETS[name].marked_changed_files, 0)


if __name__ == "__main__":
    unittest.main()
