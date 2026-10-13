#!/usr/bin/env python3
"""Reject a release tag whose metadata, changelog, or commit is not ready."""

import argparse
import importlib.util
from pathlib import Path
import re
import subprocess

spec = importlib.util.spec_from_file_location(
    "package_release", Path(__file__).with_name("package-release.py")
)
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)

TAG = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
HEADING = re.compile(r"^## \[(?P<name>[^\]]+)\](?: - \d{4}-\d{2}-\d{2})?[ \t]*$")


def changelog_section(text: str, version: str) -> str:
    """Return the non-empty body of the `## [version]` changelog section."""
    bodies, current = [], None
    for line in text.splitlines():
        if line.startswith("## "):
            heading = HEADING.match(line)
            current = [] if heading and heading["name"] == version else None
            if current is not None:
                bodies.append(current)
        elif current is not None:
            current.append(line)
    if not bodies:
        raise ValueError(f"CHANGELOG.md has no section for {version}")
    if len(bodies) > 1:
        raise ValueError(f"CHANGELOG.md has more than one section for {version}")
    body = "\n".join(bodies[0]).strip()
    if not body:
        raise ValueError(f"CHANGELOG.md section for {version} is empty")
    return body


def git_is_ancestor(root: Path, commit: str, branch: str) -> bool:
    result = subprocess.run(
        ["git", "-C", str(root), "merge-base", "--is-ancestor", commit, branch],
        capture_output=True, text=True,
    )
    if result.returncode == 0:
        return True
    if result.returncode == 1:
        return False
    raise ValueError(f"Cannot compare {commit} with {branch}: {result.stderr.strip()}")


def check_release(root: Path, tag: str, ref: str, main_ref: str) -> str:
    """Validate a release candidate and return its changelog notes."""
    if not TAG.fullmatch(tag):
        raise ValueError(f"Tag {tag} must be a stable vMAJOR.MINOR.PATCH version")
    version = packager.release_version(root, tag)
    changelog = root / "CHANGELOG.md"
    if not changelog.is_file():
        raise ValueError("CHANGELOG.md is missing")
    notes = changelog_section(changelog.read_text(encoding="utf-8"), version)
    if not git_is_ancestor(root, ref, main_ref):
        raise ValueError(f"Tagged commit {ref} is not part of {main_ref}")
    return notes


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True, help="Release tag, for example v1.0.0")
    parser.add_argument("--ref", default="HEAD", help="Commit the tag points at")
    parser.add_argument("--main-ref", default="origin/main", help="Branch the commit must belong to")
    parser.add_argument("--notes-file", type=Path, help="Write the changelog notes here")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    try:
        notes = check_release(root, args.tag, args.ref, args.main_ref)
        if args.notes_file:
            args.notes_file.write_text(notes + "\n", encoding="utf-8")
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"Release check failed: {error}\n")
    print(f"Release metadata for {args.tag} is consistent")


if __name__ == "__main__":
    main()
