# Validation

This page indexes validation evidence for fastdiff. Record evidence for a release in the release's own section below; never reuse evidence from another version.

## Current release

### Release 1.3.0

Recorded 2026-10-27 from revision `a8b3e9f35faa52a6dac843688728453b9e5dfa8b`. Tagged CI run: [40581234761](https://github.com/luisblanco93/fastdiff/actions/runs/40581234761), dispatched by the `v1.3.0` tag push.

Two failures of the local gate led to test-only fixes on `main` before the release PR: the idle-document branch test failed under load in the default-parallelism Linux run (#37, LIT-37) and a settings path test used Linux fixture paths that are not absolute on Windows (#38, LIT-38). Evidence below is for the commit above after both fixes.

#### Checks run
- CI on Ubuntu 22.04, macOS 14 (Apple Silicon), and Windows (Rust 1.90, Python 3.11): fmt, Clippy `-D warnings`, workspace and script tests, release builds, native window smoke checks, and packaging passed (`linux`, `macos`, `windows`, `windows-package`, `release-assets`, `draft-release`, `result` all succeeded). The `draft-release` job created the draft (commit `a8b3e9f`, four assets) and verified checksums, archive contents, and byte-identical paired WSL helpers.
- Local gate on Linux/WSL2 at `8a4c85d` (the release commit before the squash merge; Rust 1.90.0, Python 3.11): fmt, Clippy `--locked -D warnings`, `cargo test --workspace --locked` (`fastdiff-gui` 354 passed, 2 ignored), `cargo build --release --locked --workspace`, and 298 Python tests (3 skipped).
- Local gate on native Windows (MSVC, Rust 1.90) at `8a4c85d`: fmt, Clippy `--locked -D warnings`, `cargo test --workspace --locked -- --test-threads=2` (`fastdiff-gui` 345 passed, 1 ignored), release build, and `scripts/test-package-windows.ps1`.
- Downloaded the three archives and `SHA256SUMS` from the published release and verified them with `sha256sum -c`; checked each archive's file list (binaries plus `licenses/material-icons-LICENSE` and `licenses/color-themes-NOTICE`; the Windows archive also has `wsl/fastdiff`). The `wsl/fastdiff` helper in the Windows archive has the same SHA-256 as the Linux `bin/fastdiff` (`fe8bd4a5709de0ccb5743d04633fff8e50ecbb28c45fb41d789ca827e3c40efc`).
- Linux x86_64 archive on WSL2 Ubuntu: `fastdiff --version` printed `fastdiff 1.3.0 (helper protocol 5)` and `--help` listed `--color-theme`.
- A debug build of the same source (before the version bump) was opened on native Linux and, with its matching WSL helper (protocol 5), on native Windows; the Theme menu, hover preview, and the Default theme rename were tried by hand.

#### Artifact verification
| Archive | Bytes | SHA-256 |
|---|---:|---|
| `fastdiff-1.3.0-linux-x64.tar.gz` | 5,564,216 | `72403778d2e5c9d5271f402e356162568ba6f0052b216e56a9d5b889df994a57` |
| `fastdiff-1.3.0-macos-arm64.tar.gz` | 3,559,658 | `1daace3763ca8e78bd3cfc2790d980c37762dc81ff91a8ddbd85a44ef33192e3` |
| `fastdiff-1.3.0-windows-x64.zip` | 4,096,773 | `6b0be12cb291da9c01f5f4409e4e78ce2e76e62bee73a1301d2b1e5f1bae2214` |

#### Limits
- No interactive window was opened from the release archives themselves; the hand checks used debug builds of the same source. The macOS archive was not run on a Mac, and the macOS binaries are not signed or notarized.
- Follow-system theme switching needs a platform that reports the system theme (Windows, macOS). It was covered by unit tests with a simulated system theme, not by changing the OS appearance on a real Windows or macOS machine, and it is unavailable on Linux X11 and Wayland.
- Performance was not measured. Theme lookups, settings writes, and the extra function and type highlight spans add small per-frame and per-line work; no before and after numbers exist.
- The 1.3.0 idle-document branch test failure under load (#37) was not reproduced on an idle machine; the fix removes a redundant fixed time window, and the test still fails when Git is polled on every tick.
- Release 1.2.0 has no section on this page.

### Release 1.1.0

Recorded 2026-10-15 from revision `704591def29393a112c1f3efdbbfff1cce415c22`. Tagged CI run: [38816468381](https://github.com/luisblanco93/fastdiff/actions/runs/38816468381), dispatched by the `v1.1.0` tag push.

The first `v1.1.0` tag (revision `107274ce12a334d016872bc297377b4226c1b6b1`, run [38798728033](https://github.com/luisblanco93/fastdiff/actions/runs/38798728033)) was never published. It was deleted and re-created on the commit above to include #24, #25, and #26 (Unpushed without an upstream, LIT-32), as allowed for unpublished versions in [releasing](releasing.md). Evidence below is for the re-created tag only.

#### Checks run
- CI on Ubuntu 22.04, macOS 14 (Apple Silicon), and Windows (Rust 1.90, Python 3.11): fmt, Clippy `-D warnings`, workspace and script tests, coverage, release builds, native window smoke checks, and packaging passed in the concurrent layout. `windows-package` packaged the Windows binaries with the Linux helper from the same run. The `draft-release` job created the draft (commit `704591d`, four assets), and the `result` job passed.
- Local gate on Linux/WSL2 at `704591d` (Rust 1.90.0, Python 3.11.15): fmt, Clippy `--locked -D warnings`, 493 Rust tests passed (2 ignored), locked release build, 296 Python tests passed (3 skipped), and `release-guard.py --tag v1.1.0` reported consistent metadata.
- Downloaded the three archives and `SHA256SUMS` from the run and verified them with `sha256sum -c`; checked each archive's file list; the `wsl/fastdiff` helper in the Windows archive is byte-identical to the Linux `bin/fastdiff`.
- Linux x86_64 archive on WSL2 Ubuntu: `fastdiff --version` printed `fastdiff 1.1.0 (helper protocol 5)` and `--help` ran.
- LIT-32: a debug build of the same change opened natively on Linux on a repository with a never-pushed branch, before the merge; the tagged archives were not opened on such a repository.
- macOS: screenshots from CI builds of earlier source (hosted `macos-14`, not the tagged archive) showed the window rendering a repository, dark chrome and title bar, `Cmd` shortcuts and the help list, live refresh of an open file, and a CJK file name drawn with real characters ([38788844832](https://github.com/luisblanco93/fastdiff/actions/runs/38788844832)).

#### Artifact verification
| Archive | Bytes | SHA-256 |
|---|---:|---|
| `fastdiff-1.1.0-linux-x64.tar.gz` | 5,506,494 | `5ae652cb09c271d6cb86bbe8d7c1275f955f1ff2f4b9e91891ed59ed87a5244b` |
| `fastdiff-1.1.0-macos-arm64.tar.gz` | 3,495,880 | `23894fe751dc08ef49bcfde9e8d7417d6590ef6c20bdb9256ab582f5b4ebad17` |
| `fastdiff-1.1.0-windows-x64.zip` | 4,034,026 | `ec2e2043053d061168233b91e58b38f6712de44cecee0e3eed6c317f94422532` |

#### Limits
- The macOS archive itself was not run on a Mac. The screenshots are from CI builds of earlier source, taken through scripted keystrokes, and the binaries are not signed or notarized.
- The LIT-32 Unpushed fallback was not exercised on native Windows, macOS, or Windows-to-WSL; its per-refresh cost on branches without an upstream was not measured.
- No interactive window was opened from the Windows archive, and Windows-to-WSL was not exercised on this build.
- The 10-repetition worker test check on native Windows was not run, so LIT-21 and LIT-31 stability on Windows is not proven. The dark native title bar and the Windows CJK font candidates are unverified there.
- Linux CJK font candidates were not exercised; no CJK font was present on the test host.
- CJK fallback startup cost was reasoned from lazy loading, not benchmarked.

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

## Developer validation

One policy for every contributor and coding agent (Codex, Claude Code, or a person). It uses only the shell commands below; it does not depend on any agent's hooks, commands, memory, or session context. Local agent files such as `AGENTS.md` and `CLAUDE.md` are untracked here (see `.gitignore`), so a local copy should link to this section rather than restate it.

### What to run, and when

CI runs only for `v*` tags and manual dispatch, so the local checks are the gate for pull requests and `main`. Run the cheapest check that covers the change while editing, and the full set at the delivery boundaries.

| Moment | Run |
| -- | -- |
| While editing | Focused tests for the touched module and its consumers (commands below). Not the full gate after each edit. |
| Before pushing a pull request | Format, Clippy, `cargo test --workspace --locked`, and Python tests |
| Distribution-sensitive change (Cargo/toolchain/dependencies, assets, native startup or rendering, `install.py`, `scripts/runtime/`, packaging, workflows, their tests) | The above plus the packaging and native-smoke checks on the affected platform; a Linux pass does not verify Windows |
| Release candidate | The exact candidate commit through the full local set and the tagged CI run ([releasing](releasing.md)) |
| Performance measurement | Explicit opt-in only ([performance](performance.md)); never a gate |

Ambiguous impact defaults to the broader check.

### Focused commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test -p fastdiff-core --locked                        # core library and tests/repository.rs
cargo test -p fastdiff-cli --locked --test protocol         # helper protocol; also: ipc, launcher, launch_process
cargo test -p fastdiff-gui --locked worker::                # worker, cache, watcher, reflog, recovery
cargo test -p fastdiff-gui --locked app::                   # app state, sidebar, shortcuts
python3 -m pip install pyyaml==6.0.3                      # once; test_release_workflow parses the workflow as YAML
python3 -m unittest scripts.test_release_workflow           # any scripts/test_*.py module
python3 -m unittest discover -s scripts -p 'test_*.py'      # all Python tests
```

Windows runs use `--test-threads=2` in CI (`cargo test --workspace --locked -- --test-threads=2`). A worker test that fails only under the default parallelism is a load-sensitivity defect to repair with a progress-based wait, not a reason to raise a deadline (LIT-21, LIT-31).

### Rules

1. Decide whether a failure is an application defect or a test/harness defect before changing an assertion. A production defect gets its own scoped fix; its test is not weakened.
2. Never resolve a failure with `#[ignore]`, `continue-on-error`, retry-until-green, or a blanket timeout increase.
3. Keep path-location equivalence (short/long names, canonical prefixes, external Git directories) apart from textual contracts (raw Git bytes, serialized values, forwarded arguments). Normalize only the former.
4. Each new test protects an identifiable risk; reuse existing coverage where it is sufficient. Do not spawn Git or processes for pure logic, but keep real integrations at the boundaries.
5. Use explicit started/blocked/released/completed handshakes instead of sleeps. A watchdog timeout detects hangs, is not a performance budget, and reports the last state or event seen.
6. Keep focused performance invariants such as avoiding extra Git work on unchanged state. Do not delete all call-count assertions, and do not assert incidental call order or command spelling. Assert an exact count only when the number is the contract (0 means no work; at least 1 means the value must be queried again after a change). State every other count as an upper bound with a message, so an optimization does not fail the suite.
7. Report the commands actually run, with versions and platform, and name every platform or check that was not run.
8. When two agents work on one branch, agree who owns it and hand off with the commit, changed files, checks run, blockers, and the next step. Do not overwrite the other's work or repeat costly checks on an unchanged candidate.

### Suite protection map

Suite-level map. When a test is deleted or merged, record the protected risk and the remaining executable protection in the pull request. The `Arc::ptr_eq` and `as_ptr` assertions in `crates/fastdiff-gui/src/app/*_tests.rs` and `sidebar/tests.rs` are deliberate: they protect the guarantee that a refresh with unchanged data keeps the existing snapshot instead of rebuilding it.

| Suite | Main risks protected |
| -- | -- |
| `crates/fastdiff-core/tests/repository.rs`, `src/repository/*_tests.rs` | Read-only Git behavior, correct diffs and status, filter safety, raw path handling, size and binary limits |
| `crates/fastdiff-cli/tests/{protocol,ipc}.rs` | Helper protocol compatibility, request/response shape, stale-result rejection |
| `crates/fastdiff-cli/tests/{launcher,launch_process}.rs` | Launch, argument forwarding, process cleanup |
| `crates/fastdiff-gui/src/worker/*_tests.rs` | Cancellation and process cleanup, stale-result rejection, cache correctness, watcher and reflog refresh, recovery after restart |
| `crates/fastdiff-gui/src/app/*_tests.rs` | Selection and refresh behavior, responsive layout, shortcuts, snapshot stability |
| `scripts/test_{installer,plugin_*,launch_*}.py` | Installer and update safety, runtime launch, uninstall |
| `scripts/test_release_*.py`, `test_bump_version.py`, `test_app_package.py` | Tag/version/changelog/ancestry guard, draft-release controls, checksums, archive contents, paired helper |

## CI pipeline shape (LIT-27)

Runs only for `v*` tags and manual dispatch.

- **Concurrent verification:** `linux` and `windows` run format, Clippy, Rust and Python tests, a release build, and the native smoke test independently. `windows-package` needs both and is the only place the Windows archive is built, from the `windows` binaries and the `linux` helper of the same run.
- **Final result:** the `result` job always runs and fails on any failed, cancelled, or unexpectedly skipped job (`scripts/ci-result.py`). `release-assets` and `draft-release` may be skipped only on manual runs. Add every new job to its `needs`.
- **Caches** hold dependency inputs only: the Cargo registry and Git sources with `target/debug`, and the pinned `cargo-llvm-cov` tool with its build directory. Keys are separated by platform, architecture, toolchain, and `Cargo.lock`/`Cargo.toml` hashes. A tag run cannot restore another tag's cache but can restore the default branch's, so a natural tag run is cold unless a manual dispatch on `main` seeded the cache; the caches mainly help re-runs and manual runs. `target/release` and `dist/` are never cached, so shipped binaries are always built from the candidate in that run.
- **Timing** (manual dispatches on `main` at `040f824`, one cold then one warm; single samples, so runner variance applies):

  | | Cold ([38777925584](https://github.com/luisblanco93/fastdiff/actions/runs/38777925584)) | Warm ([38779139454](https://github.com/luisblanco93/fastdiff/actions/runs/38779139454)) |
  | -- | --: | --: |
  | Whole workflow | 8m07s | 6m05s |
  | `linux` job | 6m07s | 4m35s |
  | `windows` job | 7m32s | 5m25s |
  | `macos` job (not cached) | 4m35s | 3m14s |
  | `windows-package` | 21s | 28s |
  | Linux: Clippy / Rust tests / coverage tool install / coverage | 41s / 79s / 47s / 17s | 5s / 29s / skipped / 13s |
  | Windows: Clippy / Rust tests | 65s / 152s | 6s / 85s |
  | Release build, Linux / Windows (never cached) | 115s / 141s | 144s / 105s |

  The 1.0.0 tagged run (38480276396) ran Linux then Windows in sequence for about 13 minutes of verification. The cache saves roughly 30-40 s of Clippy and 45-70 s of tests per platform and skips the 47 s coverage-tool install; release builds are unchanged by design. macOS has no cache yet, so its 1m21s difference is runner variance, which bounds how much of the other differences can be trusted.

## CI failure classification (LIT-27)

CI runs only for `v*` tags and manual dispatch (#5, #7); local checks are the PR and `main` gate (`docs/releasing.md`). Each failing step in `build.yml` is its own named step (format, Clippy, Rust tests, Python tests, packaging validation, smoke), and both smoke steps print and upload stdout, stderr, and the report on failure.

Classification of the failures reported in LIT-27 for the predecessor repository (26 failed attempts out of 42, 17 distinct commits). The counts come from that issue and cannot be reproduced from this repository. They are run-level counts, not distinct bugs.

| Class | Attempts | Status | Current evidence |
| -- | -- | -- | -- |
| Rust/Python test failures | 15 | Resolved in code; Windows stability not yet measured | Windows path fixes landed (for example #10); worker waits use the progress watchdog and handshakes (#13, #14); the 10-repetition native Windows check has not been run |
| Native-window smoke failures | 5 | Diagnosable; no failure since | Smoke steps retain stdout, stderr, and the report; the 1.1.0 tagged run passed them |
| Formatting | 4 | Resolved as a diagnosis problem | Runs in tagged CI as its own step; local gate per `docs/releasing.md`; the historical failures predate the tag-only trigger |
| Clippy | 1 | Same as formatting | Named step |
| Coverage stage | 1 | No change needed | Thresholds unchanged (80% lines for core and CLI, Linux only); the stage is separate and passed in the 1.1.0 tagged run |

Incident in this repository: the `v1.0.1` tag run [38666695292](https://github.com/luisblanco93/fastdiff/actions/runs/38666695292) failed on `helper_operation_metadata_is_shared_for_mixed_all` (`crates/fastdiff-cli/tests/protocol.rs`), a Git call-count assertion broken by the LIT-29 change (#9), and cost a retag (#12). Status: fixed in #12; the remaining count assertions in `protocol.rs` now state upper bounds, with exact counts kept only where the number is the contract.

## Historical evidence (before this repository)

The records below come from the project's earlier development history, when the product shipped as `diffcode`. They are retained for context and **do not validate any release of this repository**.

- **0.1.7** (2026-10-10): CI on Linux and Windows passed tests, release builds, native window smoke checks, and packaging. The recorded local run reported 247 Linux and 221 native Windows MSVC Rust tests, Python suites of 81 cases per platform, core line coverage of 96.08% and CLI coverage of 80.27%, and protocol 4 Windows-to-WSL hello/status/branch/shutdown checks. Archives were below the 40 MiB compressed target.
- **Startup benchmarks** (2026-10-06, before 0.1.7): [Linux](benchmark-linux.json) and [Windows/WSL](benchmark-wsl-windows.json). Each completed 30 launches and met its startup and memory targets. Hardware, raw samples, and summaries are in those files. They measure that earlier build, not the current one.
- **Other measurements:** [LIT-1 refresh](benchmark-lit1-headless.json) and [LIT-4 merge](benchmark-lit4-merge.json) reports, described in [performance](performance.md).

See [helper compatibility](architecture.md#helper-boundary) before combining binaries from different builds, and [performance](performance.md) for targets and how to take a new measurement.
