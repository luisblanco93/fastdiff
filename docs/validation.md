# Validation

This page indexes validation evidence for fastdiff. Record evidence for a release in the release's own section below; never reuse evidence from another version.

## Current release

No release has been published from this repository yet. The first release validation record goes here, using the template below.

### Template

```markdown
## Release X.Y.Z

Recorded YYYY-MM-DD from revision `<commit>`. Tagged CI run: <link>.

### Checks run
- Command, platform, toolchain versions, and result for each check actually executed.

### Artifact verification
| Archive | Bytes | SHA-256 |
|---|---:|---|

### Limits
- Every check that was not run, and what that leaves unproven.
```

Rules: report only checks that were executed; state versions and platforms; keep the raw output or its link; list what was skipped.

## Historical evidence (before this repository)

The records below come from the project's earlier development history, when the product shipped as `diffcode`. They are retained for context and **do not validate any release of this repository**.

- **0.1.7** (2026-10-10): CI on Linux and Windows passed tests, release builds, native window smoke checks, and packaging. The recorded local run reported 247 Linux and 221 native Windows MSVC Rust tests, Python suites of 81 cases per platform, core line coverage of 96.08% and CLI coverage of 80.27%, and protocol 4 Windows-to-WSL hello/status/branch/shutdown checks. Archives were below the 40 MiB compressed target.
- **Startup benchmarks** (2026-10-06, before 0.1.7): [Linux](benchmark-linux.json) and [Windows/WSL](benchmark-wsl-windows.json). Each completed 30 launches and met its startup and memory targets. Hardware, raw samples, and summaries are in those files. They measure that earlier build, not the current one.
- **Other measurements:** [LIT-1 refresh](benchmark-lit1-headless.json) and [LIT-4 merge](benchmark-lit4-merge.json) reports, described in [performance](performance.md).

See [helper compatibility](architecture.md#helper-boundary) before combining binaries from different builds, and [performance](performance.md) for targets and how to take a new measurement.
