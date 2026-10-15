"""Structural checks that only a tagged run can write a draft release.

The workflow is parsed as YAML, so formatting, indentation, and key order do not
affect these tests; only the permissions, conditions, dependencies, and step
order that the release controls rely on do. Requires PyYAML (a development/CI
dependency, not part of the shipped runtime).
"""

from pathlib import Path
import re
import unittest

import yaml

WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/build.yml"
TAG_ONLY = "startsWith(github.ref, 'refs/tags/v')"
PUBLISH_MARKERS = ("--draft=false", "draft=false", "gh release edit", "--latest")
NAMED_CHECKS = ("Check formatting", "Clippy", "Rust tests", "Python tests")


def load() -> dict:
    return yaml.safe_load(WORKFLOW.read_text(encoding="utf-8"))


def triggers(workflow: dict) -> dict:
    # YAML 1.1 reads the bare key `on` as the boolean True.
    return workflow["on"] if "on" in workflow else workflow[True]


def needs(job: dict) -> list[str]:
    value = job.get("needs", [])
    return [value] if isinstance(value, str) else list(value)


def ancestors(jobs: dict, name: str) -> set[str]:
    seen: set[str] = set()
    pending = needs(jobs[name])
    while pending:
        current = pending.pop()
        if current not in seen:
            seen.add(current)
            pending.extend(needs(jobs[current]))
    return seen


def step_index(steps: list[dict], predicate) -> int:
    for index, step in enumerate(steps):
        if predicate(step):
            return index
    raise AssertionError("no matching step")


def run_text(job: dict) -> str:
    return "\n".join(step.get("run", "") for step in job["steps"])


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        self.workflow = load()
        self.jobs = self.workflow["jobs"]

    def test_runs_only_for_release_tags_and_manual_dispatch(self):
        on = triggers(self.workflow)
        self.assertEqual(set(on), {"push", "workflow_dispatch"})
        self.assertEqual(on["push"], {"tags": ["v*"]})

    def test_default_token_is_read_only(self):
        self.assertEqual(self.workflow["permissions"], {"contents": "read"})

    def test_only_the_tag_gated_draft_job_can_write(self):
        writers = [
            name
            for name, job in self.jobs.items()
            if "write" in (job.get("permissions") or {}).values()
        ]
        self.assertEqual(writers, ["draft-release"])
        draft = self.jobs["draft-release"]
        self.assertEqual(draft["if"], TAG_ONLY)
        self.assertEqual(needs(draft), ["release-assets"])

    def test_draft_job_does_not_cancel_or_publish(self):
        draft = self.jobs["draft-release"]
        self.assertEqual(
            draft["concurrency"],
            {"group": "release-${{ github.ref }}", "cancel-in-progress": False},
        )
        for marker in PUBLISH_MARKERS:
            self.assertNotIn(marker, run_text(draft))

    def test_no_job_publishes_a_release(self):
        for name, job in self.jobs.items():
            for marker in PUBLISH_MARKERS:
                self.assertNotIn(marker, run_text(job), f"{name} contains {marker}")

    def test_assets_and_draft_depend_on_both_platform_builds(self):
        for name in ("release-assets", "draft-release"):
            self.assertTrue(
                {"linux", "windows"} <= ancestors(self.jobs, name),
                f"{name} must transitively need linux and windows",
            )
        self.assertEqual(self.jobs["release-assets"]["if"], TAG_ONLY)

    def test_platform_verification_runs_concurrently(self):
        self.assertNotIn("linux", ancestors(self.jobs, "windows"))
        self.assertNotIn("windows", ancestors(self.jobs, "linux"))

    def test_only_windows_packaging_waits_for_the_linux_helper(self):
        self.assertEqual(set(needs(self.jobs["windows-package"])), {"linux", "windows"})
        uses_helper = [
            name
            for name, job in self.jobs.items()
            if any(
                (step.get("with") or {}).get("name") == "wsl-helper"
                for step in job["steps"]
                if step.get("uses", "").startswith("actions/download-artifact")
            )
        ]
        self.assertEqual(uses_helper, ["windows-package"])

    def test_final_result_always_runs_and_covers_every_job(self):
        result = self.jobs["result"]
        self.assertEqual(result["if"], "always()")
        self.assertEqual(set(needs(result)), set(self.jobs) - {"result"})
        self.assertIn("scripts/ci-result.py", run_text(result))
        self.assertIn("toJSON(needs)", str(result["steps"]))

    def test_caches_are_pinned_and_never_hold_release_outputs(self):
        caches = [
            (name, step)
            for name, job in self.jobs.items()
            for step in job["steps"]
            if step.get("uses", "").startswith("actions/cache@")
        ]
        self.assertTrue(caches, "expected dependency caches")
        for name, step in caches:
            with self.subTest(job=name, step=step.get("name")):
                self.assertRegex(step["uses"], r"^actions/cache@[0-9a-f]{40}\b")
                paths = step["with"]["path"].split()
                for path in paths:
                    self.assertFalse(path.startswith(("target/release", "dist")), path)
                key = step["with"]["key"]
                self.assertIn("runner.os", key)
                self.assertIn("rust1.90.0", key)
                self.assertIn("hashFiles(", key)

    def test_release_build_is_never_restored_from_cache(self):
        for name in ("linux", "windows"):
            steps = self.jobs[name]["steps"]
            build = step_index(steps, lambda s: "cargo build --release" in s.get("run", ""))
            restored = [
                s
                for s in steps[:build]
                if s.get("uses", "").startswith("actions/cache@")
                and "release" in s["with"]["path"]
            ]
            self.assertEqual(restored, [])

    def test_metadata_guard_runs_before_toolchain_and_builds(self):
        steps = self.jobs["linux"]["steps"]
        guard = step_index(steps, lambda s: "release-guard.py" in s.get("run", ""))
        toolchain = step_index(steps, lambda s: "dtolnay/rust-toolchain" in s.get("uses", ""))
        build = step_index(steps, lambda s: "cargo build" in s.get("run", ""))
        self.assertLess(guard, toolchain)
        self.assertLess(guard, build)
        self.assertEqual(steps[guard]["if"], TAG_ONLY)

    def test_macos_assets_are_built_checked_and_released(self):
        macos = self.jobs["macos"]
        self.assertEqual(macos["runs-on"], "macos-14")
        self.assertIn("scripts/package-unix.sh", run_text(macos))
        self.assertIn("verify_tar_archive", run_text(macos))
        release = self.jobs["release-assets"]
        self.assertEqual(set(needs(release)), {"linux", "macos", "windows"})
        downloaded = [
            step["with"]["name"]
            for step in release["steps"]
            if step.get("uses", "").startswith("actions/download-artifact")
        ]
        self.assertIn("macos-arm64", downloaded)
        uploaded = next(
            step["with"]["path"]
            for step in release["steps"]
            if step.get("uses", "").startswith("actions/upload-artifact")
        )
        self.assertIn("dist/fastdiff-*-macos-arm64.tar.gz", uploaded)

    def test_tag_checkout_fetches_history_for_the_main_ancestry_check(self):
        checkout = next(
            s for s in self.jobs["linux"]["steps"] if s.get("uses", "").startswith("actions/checkout")
        )
        self.assertEqual(
            checkout["with"]["fetch-depth"],
            "${{ startsWith(github.ref, 'refs/tags/v') && '0' || '1' }}",
        )

    def test_each_platform_reports_named_check_steps(self):
        for name in ("linux", "windows"):
            names = [s.get("name") for s in self.jobs[name]["steps"]]
            for check in NAMED_CHECKS:
                self.assertIn(check, names, f"{name} lacks a '{check}' step")

    def test_smoke_diagnostics_are_uploaded_on_failure(self):
        for name in ("linux", "windows"):
            steps = self.jobs[name]["steps"]
            smoke = step_index(steps, lambda s: s.get("name", "").startswith("Native") and s["name"].endswith("smoke test"))
            upload = steps[smoke + 1]
            self.assertEqual(upload.get("if"), "failure()")
            self.assertTrue(upload["uses"].startswith("actions/upload-artifact@"))


if __name__ == "__main__":
    unittest.main()
