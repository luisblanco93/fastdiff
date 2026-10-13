#!/usr/bin/env python3
"""Collect read-only Git and GitHub evidence for a release-impact assessment."""

import argparse
from dataclasses import dataclass
from datetime import datetime, timezone
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib

COUNT_THRESHOLD = 5
AGE_THRESHOLD_DAYS = 7
BUMP_ORDER = ("none", "patch", "minor", "major")
STABLE_TAG = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
CONVENTIONAL = re.compile(r"(?P<type>[a-z]+)(\([^)]*\))?(?P<bang>!)?:")
BOOKKEEPING = re.compile(r"Merge (remote-tracking )?branch '(origin/)?(main|master)'")
RELEASE_PREP = re.compile(r"(chore|release)(\([^)]*\))?: (prepare|bump|release) v?\d+\.\d+\.\d+")
REVERTS = re.compile(r"This reverts commit ([0-9a-f]{40})")
HINTS = {"feat": "minor", "fix": "patch", "perf": "patch"}
NO_RELEASE_TYPES = {"docs", "test", "ci", "chore", "refactor", "style", "build"}
DEPENDENCY_FILES = {"Cargo.toml", "Cargo.lock"}
APP_PREFIXES = ("crates/", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "scripts/package-")
RUNTIME_PREFIXES = ("install.py", "scripts/runtime/")
DOC_PREFIXES = ("docs/", "README.md", "CHANGELOG.md", "PRODUCT.md", "DESIGN.md", "LICENSE")


def load(name: str):
    path = Path(__file__).with_name(name)
    spec = importlib.util.spec_from_file_location(name.removesuffix(".py").replace("-", "_"), path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


drafter = load("release-draft.py")
packager = load("package-release.py")


class Blocked(Exception):
    """The assessment cannot be made from the available evidence."""


@dataclass(frozen=True)
class Change:
    sha: str
    date: datetime
    subject: str
    surfaces: tuple[str, ...]
    hint: str
    unclassified: bool


def git(root: Path, *args: str) -> str:
    try:
        result = subprocess.run(["git", "-C", str(root), *args], capture_output=True, text=True)
    except OSError as error:
        raise Blocked(f"Cannot run git: {error}") from error
    if result.returncode != 0:
        raise Blocked(f"git {' '.join(args[:2])} failed: {result.stderr.strip()}")
    return result.stdout


def is_ancestor(root: Path, ancestor: str, descendant: str) -> bool:
    result = subprocess.run(
        ["git", "-C", str(root), "merge-base", "--is-ancestor", ancestor, descendant],
        capture_output=True, text=True,
    )
    if result.returncode > 1:
        raise Blocked(f"Cannot compare {ancestor} with {descendant}: {result.stderr.strip()}")
    return result.returncode == 0


def derive_repo(root: Path) -> str:
    url = git(root, "remote", "get-url", "origin").strip()
    match = re.search(r"github\.com[:/]([^/\s]+/[^/\s]+?)(?:\.git)?/?$", url)
    if not match:
        raise Blocked(f"Cannot derive owner/name from origin {url!r}; pass --repo")
    return match[1]


def latest_stable_release(gh, repo: str) -> dict | None:
    """Return the highest published stable release; None only when access is proven."""
    try:
        gh(["api", f"repos/{repo}"])
        listing = drafter.decode_stream(gh(["api", "--paginate", f"repos/{repo}/releases?per_page=100"]))
    except ValueError as error:
        raise Blocked(f"GitHub release metadata unavailable: {error}") from error
    stable = [
        release for release in listing
        if not release.get("draft") and not release.get("prerelease")
        and STABLE_TAG.fullmatch(release["tag_name"])
    ]
    return max(stable, key=lambda r: tuple(map(int, r["tag_name"][1:].split(".")))) if stable else None


def surface_of(path: str) -> str:
    if path.startswith(APP_PREFIXES):
        return "app"
    if path.startswith(RUNTIME_PREFIXES):
        return "installer/runtime"
    if path.startswith("skills/"):
        return "skill"
    if path.startswith(DOC_PREFIXES):
        return "docs"
    return "internal"


def parse_change(root: Path, record: str) -> tuple[Change, str, str, list[str]]:
    sha, stamp, parents, subject, body = record.strip("\n").split("\x1f", 4)
    parent_list = parents.split()
    title = subject
    if len(parent_list) > 1:
        title = next((line.strip() for line in body.splitlines() if line.strip()), subject)
    if parent_list:
        paths = git(root, "diff", "--name-only", "-z", parent_list[0], sha).split("\0")
    else:
        paths = git(root, "show", "--format=", "--name-only", "-z", sha).split("\0")
    paths = [path for path in paths if path]
    surfaces = tuple(sorted({surface_of(path) for path in paths})) or ("internal",)
    match = CONVENTIONAL.match(title)
    breaking = bool(match and match["bang"]) or "BREAKING CHANGE" in body
    if breaking:
        hint = "major"
    elif match and match["type"] in HINTS:
        hint = HINTS[match["type"]]
    elif match and match["type"] in NO_RELEASE_TYPES:
        hint = "none"
    else:
        hint = "patch"
    if hint == "none" and any(Path(path).name in DEPENDENCY_FILES for path in paths):
        hint = "patch"  # dependency and security updates ship in the binary
    change = Change(
        sha, datetime.fromisoformat(stamp), title, surfaces, hint,
        unclassified=match is None and "app" in surfaces,
    )
    return change, subject, body, parent_list


def collect(root: Path, base: str | None, head: str) -> tuple[list[Change], list[dict]]:
    """First-parent history in base..head, with bookkeeping and reverted pairs excluded."""
    spec = f"{base}..{head}" if base else head
    log = git(root, "log", "--first-parent", "--reverse",
              "--format=%H%x1f%cI%x1f%P%x1f%s%x1f%b%x1e", spec)
    parsed = [parse_change(root, record) for record in log.split("\x1e") if record.strip()]
    kept, excluded, live = [], [], {}
    for change, subject, body, parents in parsed:
        reason = None
        if len(parents) > 1 and BOOKKEEPING.match(subject):
            reason = "merge bookkeeping"
        elif RELEASE_PREP.match(subject):
            reason = "release preparation"
        else:
            targets = [sha for sha in REVERTS.findall(body) if sha in live]
            for sha in targets:
                original = live.pop(sha)
                kept.remove(original)
                excluded.append({"sha": sha, "subject": original.subject,
                                 "reason": f"reverted by {change.sha[:12]}"})
            if targets:
                reason = "reverts another pending change"
        if reason:
            excluded.append({"sha": change.sha, "subject": change.subject, "reason": reason})
        else:
            kept.append(change)
            live[change.sha] = change
    return kept, excluded


def top_bump(changes: list[Change]) -> str:
    relevant = [c.hint for c in changes if "app" in c.surfaces]
    return max(relevant, key=BUMP_ORDER.index, default="none")


def bump_version(version: str, bump: str) -> str:
    major, minor, patch = map(int, version.split("."))
    return {"major": f"{major + 1}.0.0", "minor": f"{major}.{minor + 1}.0",
            "patch": f"{major}.{minor}.{patch + 1}"}[bump]


def describe(change: Change) -> dict:
    return {"sha": change.sha, "date": change.date.isoformat(), "subject": change.subject,
            "surfaces": list(change.surfaces), "hint": change.hint,
            "unclassified": change.unclassified}


def workspace_state(root: Path) -> dict:
    try:
        version = packager.release_version(root, None)
    except (OSError, ValueError, KeyError) as error:
        return {"workspace_version": None, "error": str(error)}
    changelog = root / "CHANGELOG.md"
    text = changelog.read_text(encoding="utf-8") if changelog.is_file() else ""
    return {"workspace_version": version,
            "changelog_present": changelog.is_file(),
            "changelog_unreleased_section": bool(re.search(r"(?m)^## \[Unreleased\]", text))}


def tag_commit(root: Path, tag: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}^{{commit}}"],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise Blocked(f"Baseline tag {tag} is not available locally; fetch tags")
    return result.stdout.strip()


def resolve_baseline(root: Path, gh, repo: str | None, main_ref: str) -> dict | None:
    release = latest_stable_release(gh, repo or derive_repo(root))
    if release is None:
        return None
    tag = release["tag_name"]
    commit = tag_commit(root, tag)
    if not is_ancestor(root, commit, main_ref):
        raise Blocked(f"Baseline {tag} is not an ancestor of {main_ref}")
    return {"tag": tag, "commit": commit}


def timing_for(relevant: list[Change], now: datetime) -> tuple[str, str]:
    age = (now - min(c.date for c in relevant)).days if relevant else 0
    summary = f"{len(relevant)} relevant integrated change(s); oldest pending {age} day(s) old"
    if len(relevant) >= COUNT_THRESHOLD or (relevant and age >= AGE_THRESHOLD_DAYS):
        return "release now", (f"{summary} (thresholds: {COUNT_THRESHOLD} changes or "
                               f"{AGE_THRESHOLD_DAYS} days).")
    return "accumulate", (f"{summary}; thresholds not reached. A complete feature or critical "
                          "fix may still justify releasing sooner.")


def initial_version(root: Path) -> str:
    """The workspace version when it is a stable 1.x or later version, else 1.0.0."""
    try:
        cargo = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
        version = cargo["workspace"]["package"]["version"]
    except (OSError, KeyError, tomllib.TOMLDecodeError):
        return "1.0.0"
    stable = STABLE_TAG.fullmatch(f"v{version}") and int(version.split(".")[0]) >= 1
    return version if stable else "1.0.0"


def assess(root: Path, repo: str | None, gh, candidate: str, main_ref: str, now: datetime) -> dict:
    result = {"candidate": None, "main_ref": main_ref, "baseline": None, "baseline_state": "unknown", "recommendation": "unknown",
              "proposed_version": None, "timing": "blocked", "reasons": [], "blocked": [],
              "integrated_changes": [], "excluded": [], "preview_changes": [],
              "version_metadata": workspace_state(root),
              "limits": ["Collector reads Git history and GitHub release metadata only; "
                         "it does not run tests, builds, or smoke checks."]}
    try:
        if git(root, "rev-parse", "--is-shallow-repository").strip() == "true":
            raise Blocked("The repository is a shallow clone; fetch full history first")
        result["candidate"] = git(root, "rev-parse", "--verify", f"{candidate}^{{commit}}").strip()
        git(root, "rev-parse", "--verify", f"{main_ref}^{{commit}}")
        baseline = resolve_baseline(root, gh, repo, main_ref)
        integrated, excluded = collect(root, baseline["commit"] if baseline else None, main_ref)
        preview = []
        if not is_ancestor(root, result["candidate"], main_ref):
            preview, _ = collect(root, main_ref, result["candidate"])
    except Blocked as error:
        result["blocked"].append(str(error))
        return result
    relevant = [c for c in integrated if "app" in c.surfaces and c.hint != "none"]
    result.update(
        baseline=baseline, baseline_state="found" if baseline else "none", excluded=excluded, relevant_count=len(relevant),
        integrated_changes=[describe(c) for c in integrated],
        preview_changes=[describe(c) for c in preview],
        surfaces=sorted({s for c in integrated for s in c.surfaces}),
        oldest_pending=min((c.date for c in relevant), default=None),
    )
    if result["oldest_pending"] is not None:
        result["oldest_pending"] = result["oldest_pending"].isoformat()
    if baseline is None:
        initial = initial_version(root)
        result.update(recommendation="initial", proposed_version=initial, timing="release now")
        result["reasons"].append("No published stable release exists; initial release, subject to validation.")
        return result
    bump = top_bump(integrated)
    result.update(
        recommendation=bump,
        proposed_version=None if bump == "none" else bump_version(baseline["tag"][1:], bump),
        preview_recommendation=top_bump([*integrated, *preview]),
    )
    if any(c.unclassified for c in relevant):
        result["reasons"].append("Some app changes lack a conventional prefix; verify their compatibility.")
    result["timing"], reason = timing_for(relevant, now)
    result["reasons"].append(reason)
    return result


def render_markdown(result: dict) -> str:
    baseline, blocked = result["baseline"], bool(result["blocked"])
    if blocked:
        baseline_line = "unknown (blocked: release history could not be established)"
    elif baseline:
        baseline_line = f"{baseline['tag']} {baseline['commit'][:12]}"
    else:
        baseline_line = "none published"
    lines = [
        f"Baseline release and commit: {baseline_line}",
        f"Candidate commit: {(result['candidate'] or 'unresolved')[:12]} (integration branch {result['main_ref']})",
        f"Relevant integrated changes / oldest pending change: {result.get('relevant_count', 0)} / "
        f"{result.get('oldest_pending') or 'n/a'}",
        "Affected delivery surface: " + ("unknown (blocked)" if blocked
                                         else ', '.join(result.get('surfaces', [])) or 'none'),
        f"Recommendation: {result['recommendation']}",
        f"Proposed version: {result['proposed_version'] or 'n/a'}",
        f"Timing: {result['timing']}",
        "Reason and user-visible changes:",
        *[f"- {reason}" for reason in result["reasons"] + result["blocked"]],
        *[f"- {c['sha'][:12]} [{', '.join(c['surfaces'])}] {c['hint']}: {c['subject']}"
          for c in result["integrated_changes"]],
        "Validation evidence and gaps:",
        *[f"- {limit}" for limit in result["limits"]],
        "Required approvals / next action: reviewer judgment on compatibility; maintainer approval "
        "before any version bump, tag, push, or publication.",
    ]
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", help="owner/name; derived from origin when omitted")
    parser.add_argument("--candidate", default="HEAD")
    parser.add_argument("--main-ref", default="origin/main")
    parser.add_argument("--format", choices=("json", "markdown"), default="markdown")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    result = assess(root, args.repo, drafter.run_gh, args.candidate, args.main_ref,
                    datetime.now(timezone.utc))
    sys.stdout.write(json.dumps(result, indent=2) + "\n" if args.format == "json"
                     else render_markdown(result))
    sys.exit(2 if result["blocked"] else 0)


if __name__ == "__main__":
    main()
