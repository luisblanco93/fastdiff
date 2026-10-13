"""Offline checks for release-impact evidence, using real temporary Git history."""

from datetime import datetime, timezone
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("release_evidence", ROOT / "scripts/release-evidence.py")
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)

REPO = "owner/fastdiff"
NOW = datetime(2026, 10, 20, 12, 0, tzinfo=timezone.utc)
IDENTITY = ["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
            "-c", "commit.gpgsign=false"]


def make_gh(releases=(), fail=None):
    def gh(args):
        if fail:
            raise ValueError(fail)
        if args == ["api", f"repos/{REPO}"]:
            return "{}"
        return json.dumps(list(releases))
    return gh


def release(tag, **overrides):
    return {"tag_name": tag, "draft": False, "prerelease": False} | overrides


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.counter = 0
        self.git("init", "-b", "main")
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "1.0.0"\n')
        self.commit("chore: initial import", "README.md", day=1)

    def git(self, *args, day=None):
        env = os.environ.copy()
        if day is not None:
            stamp = f"2026-10-{day:02d}T10:00:00+00:00"
            env |= {"GIT_AUTHOR_DATE": stamp, "GIT_COMMITTER_DATE": stamp}
        result = subprocess.run(["git", *IDENTITY, "-C", str(self.root), *args],
                                capture_output=True, text=True, env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def commit(self, message, path="crates/core/src/lib.rs", day=2, body=""):
        self.counter += 1
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(f"change {self.counter}\n")
        self.git("add", "-A")
        args = ["commit", "-m", message, *(["-m", body] if body else [])]
        self.git(*args, day=day)
        return self.git("rev-parse", "HEAD")

    def publish_baseline(self, tag="v1.0.0"):
        self.git("tag", "-a", tag, "-m", tag)

    def assess(self, releases=(), candidate="HEAD", fail=None, now=NOW):
        return evidence.assess(self.root, REPO, make_gh(releases, fail), candidate, "main", now)

    def test_no_published_release_recommends_the_initial_release(self):
        result = self.assess()
        self.assertEqual(
            (result["recommendation"], result["proposed_version"], result["timing"], result["baseline"]),
            ("initial", "1.0.0", "release now", None),
        )

    def test_api_failure_is_blocked_and_never_read_as_no_releases(self):
        result = self.assess(fail="HTTP 404: Not Found")
        self.assertEqual((result["recommendation"], result["timing"]), ("unknown", "blocked"))
        self.assertIn("unavailable", result["blocked"][0])

    def test_baseline_ignores_drafts_prereleases_and_newer_tags(self):
        self.publish_baseline("v1.0.0")
        self.commit("fix: handle empty repository", day=18)
        self.git("tag", "-a", "v1.1.0", "-m", "unpublished tag")
        releases = [release("v1.0.0"), release("v1.1.0", draft=True),
                    release("v1.2.0-rc.1", prerelease=True), release("v0.9.0")]
        result = self.assess(releases)
        self.assertEqual(result["baseline"]["tag"], "v1.0.0")
        self.assertEqual((result["recommendation"], result["proposed_version"]), ("patch", "1.0.1"))
        self.assertEqual(len(result["integrated_changes"]), 1)

    def test_highest_bump_wins_and_breaking_marker_is_major(self):
        self.publish_baseline()
        self.commit("fix: one", day=18)
        self.commit("feat: two", day=18)
        self.assertEqual(self.assess([release("v1.0.0")])["proposed_version"], "1.1.0")
        self.commit("feat!: three", day=18)
        result = self.assess([release("v1.0.0")])
        self.assertEqual((result["recommendation"], result["proposed_version"]), ("major", "2.0.0"))

    def test_docs_skill_installer_and_ci_changes_need_no_app_release(self):
        self.publish_baseline()
        self.commit("docs: usage", "docs/usage.md", day=10)
        self.commit("fix: skill wording", "skills/fastdiff/SKILL.md", day=10)
        self.commit("fix: installer path", "scripts/runtime/update.py", day=10)
        self.commit("ci: cache", ".github/workflows/build.yml", day=10)
        result = self.assess([release("v1.0.0")])
        self.assertEqual((result["recommendation"], result["timing"], result["relevant_count"]),
                         ("none", "accumulate", 0))
        self.assertEqual(set(result["surfaces"]), {"docs", "skill", "installer/runtime", "internal"})

    def test_five_changes_or_seven_days_make_a_release_due(self):
        self.publish_baseline()
        for index in range(4):
            self.commit(f"fix: change {index}", day=19)
        self.assertEqual(self.assess([release("v1.0.0")])["timing"], "accumulate")
        self.commit("fix: fifth", day=19)
        self.assertEqual(self.assess([release("v1.0.0")])["timing"], "release now")

    def test_age_threshold_uses_the_oldest_relevant_change(self):
        self.publish_baseline()
        self.commit("fix: old", day=12)
        self.assertEqual(self.assess([release("v1.0.0")], now=datetime(2026, 10, 19, 9, 59, tzinfo=timezone.utc))["timing"],
                         "accumulate")
        self.assertEqual(self.assess([release("v1.0.0")], now=datetime(2026, 10, 19, 10, 0, tzinfo=timezone.utc))["timing"],
                         "release now")

    def test_pull_request_commits_count_once(self):
        self.publish_baseline()
        self.git("checkout", "-b", "feature")
        for index in range(3):
            self.commit(f"fix: part {index}", day=18)
        self.git("checkout", "main")
        self.git("merge", "--no-ff", "feature", "-m", "Merge pull request #1 from owner/feature",
                 "-m", "fix: grouped pull request", day=18)
        result = self.assess([release("v1.0.0")])
        self.assertEqual(len(result["integrated_changes"]), 1)
        self.assertEqual(result["integrated_changes"][0]["subject"], "fix: grouped pull request")

    def test_reverted_pairs_release_preparation_and_bookkeeping_do_not_count(self):
        self.publish_baseline()
        target = self.commit("feat: experiment", day=18)
        self.git("revert", "--no-edit", target, day=18)
        self.commit("chore: prepare v1.0.1", "Cargo.lock", day=18)
        self.git("checkout", "-b", "topic")
        self.commit("fix: topic", day=18)
        self.git("checkout", "main")
        self.git("merge", "--no-ff", "topic", "-m", "Merge branch 'main' into topic", day=18)
        result = self.assess([release("v1.0.0")])
        reasons = sorted(item["reason"].split(" by ")[0] for item in result["excluded"])
        self.assertEqual(reasons, ["merge bookkeeping", "release preparation",
                                   "reverted", "reverts another pending change"])
        self.assertEqual(result["integrated_changes"], [])

    def test_reverting_a_revert_makes_the_change_live_again(self):
        self.publish_baseline()
        original = self.commit("feat: experiment", day=18)
        self.git("revert", "--no-edit", original, day=18)
        revert = self.git("rev-parse", "HEAD")
        self.git("revert", "--no-edit", revert, day=18)
        result = self.assess([release("v1.0.0")])
        self.assertEqual(len(result["integrated_changes"]), 1)
        self.assertEqual(len(result["excluded"]), 2)
        self.assertEqual(result["recommendation"], "patch")

    def test_unmerged_branch_is_a_preview_only(self):
        self.publish_baseline()
        self.commit("fix: integrated", day=18)
        self.git("checkout", "-b", "topic")
        self.commit("feat!: unmerged", day=18)
        result = self.assess([release("v1.0.0")], candidate="topic")
        self.assertEqual((result["recommendation"], result["preview_recommendation"]), ("patch", "major"))
        self.assertEqual(len(result["integrated_changes"]), 1)
        self.assertEqual(len(result["preview_changes"]), 1)

    def test_missing_baseline_tag_and_unrelated_baseline_are_blocked(self):
        result = self.assess([release("v1.0.0")])
        self.assertIn("not available locally", result["blocked"][0])
        self.git("checkout", "--orphan", "unrelated")
        self.commit("chore: unrelated root", "other.txt", day=3)
        self.publish_baseline()
        self.git("checkout", "main")
        result = self.assess([release("v1.0.0")])
        self.assertIn("not an ancestor", result["blocked"][0])

    def test_shallow_clone_is_blocked(self):
        self.commit("fix: second", day=3)
        clone = Path(self.temp.name) / "clone"
        subprocess.run(["git", "clone", "--depth", "1", self.root.as_uri(), str(clone)],
                       check=True, capture_output=True)
        result = evidence.assess(clone, REPO, make_gh(), "HEAD", "origin/main", NOW)
        self.assertIn("shallow", result["blocked"][0])

    def test_dependency_updates_count_as_patch_changes(self):
        self.publish_baseline()
        self.commit("chore(deps): update serde", "Cargo.lock", day=18)
        result = self.assess([release("v1.0.0")])
        self.assertEqual((result["recommendation"], result["relevant_count"]), ("patch", 1))

    def test_paths_with_spaces_and_unicode_are_classified_correctly(self):
        self.publish_baseline()
        self.commit("fix: unicode fixture", "crates/core/tests/caf\u00e9 file.rs", day=18)
        self.assertEqual(self.assess([release("v1.0.0")])["recommendation"], "patch")

    def test_blocked_markdown_never_reads_as_no_release(self):
        result = self.assess(fail="HTTP 401: Bad credentials")
        self.assertEqual(result["baseline_state"], "unknown")
        text = evidence.render_markdown(result)
        self.assertIn("Baseline release and commit: unknown (blocked", text)
        self.assertNotIn("none published", text)
        self.assertIn("Affected delivery surface: unknown (blocked)", text)

    def test_initial_proposal_follows_the_workspace_version(self):
        self.assertEqual(self.assess()["baseline_state"], "none")
        (self.root / "Cargo.toml").write_text('[workspace.package]\nversion = "1.0.1"\n')
        self.assertEqual(self.assess()["proposed_version"], "1.0.1")

    def test_markdown_follows_the_assessment_template(self):
        self.publish_baseline()
        self.commit("fix: report", day=18)
        text = evidence.render_markdown(self.assess([release("v1.0.0")]))
        for heading in ("Baseline release and commit:", "Candidate commit:", "Recommendation: patch",
                        "Proposed version: 1.0.1", "Timing: accumulate", "Validation evidence and gaps:"):
            self.assertIn(heading, text)


if __name__ == "__main__":
    unittest.main()
