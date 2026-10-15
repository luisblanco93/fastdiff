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
6. Keep focused performance invariants such as avoiding extra Git work on unchanged state. Do not delete all call-count assertions, and do not assert incidental call order or command spelling.
7. Report the commands actually run, with versions and platform, and name every platform or check that was not run.
8. When two agents work on one branch, agree who owns it and hand off with the commit, changed files, checks run, blockers, and the next step. Do not overwrite the other's work or repeat costly checks on an unchanged candidate.

### Suite protection map

Initial suite-level map, to be refined as tests are consolidated (LIT-27 PR 3). When a test is deleted or merged, record the protected risk and the remaining executable protection in the pull request.

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
- **Caches** hold dependency inputs only: the Cargo registry and Git sources with `target/debug`, and the pinned `cargo-llvm-cov` tool with its build directory. Keys are separated by platform, architecture, toolchain, and `Cargo.lock`/`Cargo.toml` hashes. `target/release` and `dist/` are never cached, so shipped binaries are always built from the candidate in that run.
- **Timing:** the last measured successful tagged run (1.0.0, [38480276396](https://github.com/luisblanco93/fastdiff/actions/runs/38480276396)) took about 6m12s on Linux and 6m49s on Windows, run in sequence. Cold and warm timings for this layout are not yet measured and are recorded after the next natural tag or manual run.

## CI failure classification (LIT-27)

CI runs only for `v*` tags and manual dispatch (#5, #7); local checks are the PR and `main` gate (`docs/releasing.md`). Each failing step in `build.yml` is its own named step (format, Clippy, Rust tests, Python tests, packaging validation, smoke), and both smoke steps print and upload stdout, stderr, and the report on failure.

Classification of the failures reported in LIT-27 for the predecessor repository (26 failed attempts out of 42, 17 distinct commits). The counts come from that issue and cannot be reproduced from this repository. They are run-level counts, not distinct bugs.

| Class | Attempts | Status | Current evidence |
| -- | -- | -- | -- |
| Rust/Python test failures | 15 | Partly resolved; worker timing is pending | Windows path fixes landed (for example #10); worker-wait load sensitivity is tracked in LIT-21 and LIT-31 |
| Native-window smoke failures | 5 | Diagnosable; cause not re-evaluated | Smoke steps retain stdout, stderr, and the report |
| Formatting | 4 | Resolved as a diagnosis problem | Runs in tagged CI as its own step; local gate per `docs/releasing.md`; the historical failures predate the tag-only trigger |
| Clippy | 1 | Same as formatting | Named step |
| Coverage stage | 1 | Pending | Thresholds unchanged (80% lines for core and CLI, Linux only); the stage was already separate |

Incident in this repository: the `v1.0.1` tag run [38666695292](https://github.com/luisblanco93/fastdiff/actions/runs/38666695292) failed on `helper_operation_metadata_is_shared_for_mixed_all` (`crates/fastdiff-cli/tests/protocol.rs`), a Git call-count assertion broken by the LIT-29 change (#9), and cost a retag (#12). Status: fixed in #12; the incidental-count class is pending in LIT-27 PR 3.

## Historical evidence (before this repository)

The records below come from the project's earlier development history, when the product shipped as `diffcode`. They are retained for context and **do not validate any release of this repository**.

- **0.1.7** (2026-10-10): CI on Linux and Windows passed tests, release builds, native window smoke checks, and packaging. The recorded local run reported 247 Linux and 221 native Windows MSVC Rust tests, Python suites of 81 cases per platform, core line coverage of 96.08% and CLI coverage of 80.27%, and protocol 4 Windows-to-WSL hello/status/branch/shutdown checks. Archives were below the 40 MiB compressed target.
- **Startup benchmarks** (2026-10-06, before 0.1.7): [Linux](benchmark-linux.json) and [Windows/WSL](benchmark-wsl-windows.json). Each completed 30 launches and met its startup and memory targets. Hardware, raw samples, and summaries are in those files. They measure that earlier build, not the current one.
- **Other measurements:** [LIT-1 refresh](benchmark-lit1-headless.json) and [LIT-4 merge](benchmark-lit4-merge.json) reports, described in [performance](performance.md).

See [helper compatibility](architecture.md#helper-boundary) before combining binaries from different builds, and [performance](performance.md) for targets and how to take a new measurement.
