"""Static checks that only a tagged run can write a draft release."""

from pathlib import Path
import re
import unittest

WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/build.yml"
TAG_ONLY = "if: startsWith(github.ref, 'refs/tags/v')"


def jobs(text: str) -> dict[str, str]:
    body = text.split("\njobs:\n", 1)[1]
    parts = re.split(r"(?m)^  ([a-z][a-z-]*):\n", body)
    return dict(zip(parts[1::2], parts[2::2]))


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        self.text = WORKFLOW.read_text(encoding="utf-8")
        self.jobs = jobs(self.text)

    def test_default_token_is_read_only(self):
        header = self.text.split("\njobs:\n", 1)[0]
        self.assertRegex(header, r"(?m)^permissions:\n  contents: read$")

    def test_only_the_tag_gated_draft_job_can_write(self):
        writers = [name for name, body in self.jobs.items() if "contents: write" in body]
        self.assertEqual(writers, ["draft-release"])
        self.assertIn(TAG_ONLY, self.jobs["draft-release"])
        self.assertRegex(self.jobs["draft-release"], r"(?m)^    needs: release-assets$")

    def test_draft_job_does_not_cancel_or_publish(self):
        body = self.jobs["draft-release"]
        self.assertIn("cancel-in-progress: false", body)
        self.assertIn("group: release-${{ github.ref }}", body)
        for forbidden in ("--draft=false", "draft=false", "gh release edit", "--latest"):
            self.assertNotIn(forbidden, body)

    def test_metadata_guard_runs_before_toolchain_and_builds(self):
        body = self.jobs["linux"]
        guard = body.index("scripts/release-guard.py")
        self.assertLess(guard, body.index("dtolnay/rust-toolchain"))
        self.assertLess(guard, body.index("cargo build"))
        self.assertIn(TAG_ONLY, body[guard - 120:guard])

    def test_tag_checkout_fetches_history_for_the_main_ancestry_check(self):
        self.assertIn("fetch-depth: ${{ startsWith(github.ref, 'refs/tags/v') && '0' || '1' }}",
                      self.jobs["linux"])


if __name__ == "__main__":
    unittest.main()
