# Releasing fastdiff

The single maintainer reference for versioning, release timing, and the release procedure. Users never need the GitHub CLI; it appears here only in maintainer tooling. Installation and updating are covered in [distribution](distribution.md), recorded checks in [validation](validation.md), and user-facing changes in the [changelog](../CHANGELOG.md).

## Version policy

Tags are stable `vMAJOR.MINOR.PATCH` only, following [Semantic Versioning](https://semver.org/). Apply the highest bump required by any unreleased change. The number of commits decides *when to consider* a release, never the increment.

| Change | Bump |
| --- | --- |
| Backward-compatible bug fix, performance improvement, or binary dependency/security fix | Patch |
| Backward-compatible user-facing feature or documented deprecation | Minor |
| Breaking change to a supported user-facing contract | Major |
| Documentation, tests, CI, or internal changes with no shipped behavior change | No app release |

**Supported contract:** CLI commands and options, documented `FASTDIFF_*` environment variables, installation and update behavior, supported platforms, and persisted user data. Internal implementation details are not a contract. The helper protocol is versioned independently (see [helper compatibility](architecture.md#helper-boundary)); changing it does not by itself require a major release when matched components keep the user-facing contract.

**Installer, runtime, and skill** changes ship from `main` without an app release, because `install.py` and `scripts/runtime/` are fetched from `main`. They must stay compatible with the latest published stable app. An incompatible change needs a coordinated migration and release review, not an independent push that breaks existing installations.

## When to release

Prepare a release when any of these holds, counting from the **latest published stable release** (not the newest tag or a draft):

- A complete user-facing feature is ready.
- A critical security, startup, installation, or correctness fix is ready.
- At least **five relevant integrated changes** are pending.
- Relevant changes have been unpublished for **seven days**.

Count logical changes integrated into `main`: a merged or squash-merged pull request counts once, direct commits (and each commit of a rebase-merged pull request) count individually, and merge bookkeeping, release preparation, and changes reverted before publication do not count. Unmerged branches get a preview assessment but never count toward cadence. Only changes touching the app (`crates/`, `Cargo.*` including dependency updates, packaging) are relevant to an app release.

With no published release, the recommendation is the **initial release**, subject to validation. A failed or unauthorized GitHub lookup is never read as "no previous releases"; it blocks the assessment.

## Release-impact assessment

An assessment separates "a release is warranted" from "this commit is ready to publish". Collect the evidence read-only, then judge compatibility:

```bash
git fetch --tags origin
python3 scripts/release-evidence.py --format markdown   # or --format json
```

The collector (`scripts/release-evidence.py`, needs `git` and an authenticated `gh`) reports the baseline release, integrated and previewed changes, delivery surface, a recommendation hint, and timing. Conventional commit prefixes are hints, not proof of compatibility; a person or review agent must still read the changes. It exits 2 with an explicit blocked result for a shallow clone, missing baseline tag, a baseline that is not an ancestor of `main`, or unavailable GitHub metadata. It runs no tests and claims no check passed.

Assess once per unchanged candidate commit and baseline. An assessor never bumps versions, edits the changelog, creates or moves tags, pushes, or publishes.

## Procedure

Every step that writes outside the working tree (push, tag, publish) needs the maintainer's explicit authorization for that step; approval of one does not extend to the next.

1. **Assess** with the collector above and agree the version.
2. **Prepare** on a branch: `python3 scripts/bump-version.py X.Y.Z`, then move the `Unreleased` entries into a new `## [X.Y.Z] - YYYY-MM-DD` changelog section, add migration notes where needed, and open a release-preparation pull request. The release notes are that section, not a commit list.
3. **Validate locally** with the CI toolchain (Rust 1.90, Python 3.11) before pushing:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   python3 -m unittest discover -s scripts -p 'test_*.py'
   cargo build --release --locked --workspace
   ```
   Run the packaging and native checks on Windows itself (`./scripts/test-package-windows.ps1`, `./scripts/package-windows.ps1`); a Linux pass does not verify Windows.
4. **Integrate**: merge the pull request. CI does not run for pull requests or merges to `main`; the local checks above are the gate, and the tagged CI run below is the release gate. Approving a pull request is not validation of the release: the full set in step 3 and the tagged run must cover the exact commit that is tagged, and evidence from an earlier commit does not carry over (see [validation](validation.md#developer-validation)).
5. **Tag** the merged commit with a new annotated tag, then push it:
   ```bash
   git tag -a vX.Y.Z -m "fastdiff X.Y.Z" <commit>
   git push origin vX.Y.Z
   ```
6. **CI builds a draft.** The tagged run first rejects a tag that differs from the workspace or lockfile version, has no non-empty changelog section, or is not on `main`. It then builds both platforms, creates `SHA256SUMS`, and the `draft-release` job verifies exactly three assets (Linux archive, Windows archive, `SHA256SUMS`), their checksums, archive contents, and byte-identical paired WSL helpers. It creates a GitHub Release with `draft=true` and `prerelease=false`, recording the commit, CI run, validation limits, and changelog notes. Only that job has `contents: write`; it is serialized per tag, resumes a matching incomplete draft on re-run, and refuses a published release or conflicting assets.
7. **Inspect the draft**: confirm the commit, the CI run, the asset list, and record the local and platform smoke results in [validation](validation.md) using its template.
   ```bash
   gh api "repos/OWNER/REPO/releases" --jq '.[] | select(.tag_name=="vX.Y.Z") | {draft, prerelease, assets: [.assets[].name]}'
   gh run download RUN_ID --name release-assets   # the same files CI attached
   ```
8. **Publish** only after separate approval, by publishing the draft in the GitHub UI or `gh api -X PATCH repos/OWNER/REPO/releases/ID -F draft=false`. The installer and `dff --update` use the latest stable release, so installation becomes available at this step, not at the tag.

### Recovery

- **Never move or recreate the tag of a published release and never overwrite published assets.** The installer's downgrade protection depends on published versions staying immutable.
- **A version that was never published may be re-tagged.** If tagged CI fails before a draft exists, or a draft is rejected during inspection, no installer can have seen that version. Record the exact command, platform, revision, and run link, fix the cause on `main`, delete the draft release if one exists, delete the tag locally and on `origin`, and tag the fixed commit with the same version. Each tag deletion and push needs the maintainer's explicit authorization, like any tag push. Do not bump the version only because an unpublished attempt failed.
- Re-running the failed jobs of the same run is fine; it does not change the tag.
- A defect in a published release is fixed by a new version.

## First release (1.0.0)

The history of this repository starts at `1.0.0`. Earlier `diffcode` evidence, tags, and validation records do not apply to it. Before publishing `1.0.0`, run and record Linux, native Windows, and Windows-to-WSL smoke checks on the exact release candidate. Existing installations that still follow the previous repository update from there until the user reruns the installer from this repository once; there is no automatic redirect.
