#!/usr/bin/env python3
"""Fold the results of the workflow's jobs into one pass/fail.

Reads `toJSON(needs)` from the NEEDS_JSON environment variable. Every job must
succeed; a job may be skipped only when it is tag-only and the run is not for a
release tag. A failed, cancelled, or unexpectedly skipped job fails the result.
"""

import argparse
import json
import os
import sys

TAG_ONLY_JOBS = frozenset({"release-assets", "draft-release"})


def problems(needs: dict, is_tag: bool) -> list[str]:
    if not needs:
        return ["no job results were provided"]
    found = []
    for name, info in sorted(needs.items()):
        result = info.get("result")
        if result == "success":
            continue
        if result == "skipped" and name in TAG_ONLY_JOBS and not is_tag:
            continue
        found.append(f"{name}: {result}")
    return found


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag-run", choices=("true", "false"), required=True,
                        help="whether this run is for a release tag")
    args = parser.parse_args(argv)
    try:
        needs = json.loads(os.environ["NEEDS_JSON"])
    except (KeyError, json.JSONDecodeError) as error:
        print(f"NEEDS_JSON is missing or invalid: {error}", file=sys.stderr)
        return 2
    found = problems(needs, args.tag_run == "true")
    for line in found:
        print(f"not successful: {line}", file=sys.stderr)
    if not found:
        print(f"all {len(needs)} required jobs succeeded or were legitimately skipped")
    return 1 if found else 0


if __name__ == "__main__":
    raise SystemExit(main())
