"""Checks for the final CI result that folds every job into one status."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ci-result.py"
spec = importlib.util.spec_from_file_location("ci_result", SCRIPT)
ci_result = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci_result)


def results(**jobs: str) -> dict:
    return {name.replace("_", "-"): {"result": value} for name, value in jobs.items()}


class ProblemTests(unittest.TestCase):
    def test_all_success_passes(self):
        needs = results(linux="success", windows="success", release_assets="success")
        self.assertEqual(ci_result.problems(needs, is_tag=True), [])

    def test_failure_and_cancellation_fail(self):
        for result in ("failure", "cancelled"):
            with self.subTest(result=result):
                needs = results(linux="success", windows=result)
                self.assertEqual(ci_result.problems(needs, is_tag=False), [f"windows: {result}"])

    def test_tag_only_jobs_may_be_skipped_off_tag(self):
        needs = results(linux="success", windows="success", release_assets="skipped",
                        draft_release="skipped")
        self.assertEqual(ci_result.problems(needs, is_tag=False), [])

    def test_tag_only_jobs_must_run_on_a_tag(self):
        needs = results(linux="success", release_assets="skipped")
        self.assertEqual(ci_result.problems(needs, is_tag=True), ["release-assets: skipped"])

    def test_a_skipped_verification_job_is_never_acceptable(self):
        needs = results(linux="success", windows="skipped")
        for is_tag in (True, False):
            with self.subTest(is_tag=is_tag):
                self.assertEqual(ci_result.problems(needs, is_tag), ["windows: skipped"])

    def test_missing_results_fail(self):
        self.assertTrue(ci_result.problems({}, is_tag=False))


class CommandTests(unittest.TestCase):
    def run_script(self, needs, tag_run="false", env_extra=None):
        env = {k: v for k, v in os.environ.items() if k != "NEEDS_JSON"}
        if needs is not None:
            env["NEEDS_JSON"] = needs if isinstance(needs, str) else json.dumps(needs)
        env.update(env_extra or {})
        return subprocess.run([sys.executable, str(SCRIPT), "--tag-run", tag_run],
                              capture_output=True, text=True, env=env)

    def test_exit_codes(self):
        self.assertEqual(self.run_script(results(linux="success")).returncode, 0)
        failed = self.run_script(results(linux="failure"))
        self.assertEqual(failed.returncode, 1)
        self.assertIn("linux: failure", failed.stderr)

    def test_missing_or_invalid_input_is_an_error_not_a_pass(self):
        self.assertEqual(self.run_script(None).returncode, 2)
        self.assertEqual(self.run_script("not json").returncode, 2)


if __name__ == "__main__":
    unittest.main()
