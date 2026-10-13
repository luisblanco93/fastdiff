# Validation

This page indexes validation evidence for fastdiff. Record evidence for a release in the release's own section below; never reuse evidence from another version.

## Current release

### Release 1.0.0

Recorded 2026-10-13 from revision `170e81d65650d81dcc65e00069330460217996d9`. Tagged CI run: [38480276396](https://github.com/luisblanco93/fastdiff/actions/runs/38480276396), dispatched on the `v1.0.0` tag after the push-triggered run was deleted.

#### Checks run
- CI on Ubuntu 22.04 and Windows (Rust 1.90, Python 3.11): fmt, clippy `-D warnings`, workspace tests, coverage, release build, and packaging passed. The `draft-release` job verified three assets, their checksums, archive contents, and byte-identical paired WSL helpers.
- Downloaded the three release assets and verified both archives against `SHA256SUMS` with `sha256sum -c`.
- Linux x86_64 archive on WSL2 Ubuntu: `fastdiff --version` printed `fastdiff 1.0.0 (helper protocol 5)` and `--help` ran.
- Windows x86_64 archive on native Windows 11: `fastdiff.exe --version` printed `fastdiff 1.0.0 (helper protocol 5)` and `--help` ran.
- Paired WSL helper (`wsl/fastdiff` from the Windows archive): same SHA-256 under Windows and WSL, and `--version` printed `fastdiff 1.0.0 (helper protocol 5)`.
- Native Windows startup: 30 cold launches on the LIT-26 tree, median first diff 933 ms (see the README and [benchmark-windows-native.json](benchmark-windows-native.json)).

#### Artifact verification
| Archive | Bytes | SHA-256 |
|---|---:|---|
| `fastdiff-1.0.0-linux-x64.tar.gz` | 5,471,173 | `07743c7a0908289796af90851d8fe2d1c62f8840007defb8e06a815bb89995d9` |
| `fastdiff-1.0.0-windows-x64.zip` | 3,990,627 | `cc2b26dab8a0d79dafb2127d0f1f307aee30cc13b6c724d719ef320a24b92e7e` |

#### Limits
- No interactive window was opened from the release archives on Linux, native Windows, or Windows-to-WSL; the native-window smoke checks ran in CI only. A Windows window opening a WSL repository through the paired helper was not exercised on the release build.
- The Windows startup benchmark used the LIT-26 tree before the final merge and CI fix, not the tagged binary, and is a single run.
- Linux and WSL startup numbers were not re-measured for 1.0.0.

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
