# Measuring release performance

Release targets:

| Measure | Linux x64 | Native Windows from WSL2 |
|---|---:|---:|
| Cold process launch to first interactive frame, p95 | ≤500 ms | ≤1,000 ms |
| Cold process launch to first selected diff, p95 | ≤1,000 ms | ≤1,500 ms |
| Small-repository idle GUI RSS | ≤150 MiB | ≤150 MiB |
| Compressed platform package | ≤40 MiB | ≤40 MiB |
| Navigation UI stall | ≤50 ms | ≤50 ms |

Measure release binaries over at least 30 fresh process launches, with WSL already running. Close existing viewer windows first. Cold here means a new process; this script does not evict the operating-system disk cache. Record CPU, RAM, disk, OS, desktop/GPU, Rust version, repository size, Git version, and WSL version/distribution.

The benchmark uses opt-in GUI telemetry and automatic window closure. It includes CLI/bridge startup in each elapsed time rather than timing only the launcher acknowledgement. It rejects missing first-frame/diff telemetry, reports launcher exit time separately, and samples native GUI resident memory while the window is idle. Linux measurements pass the launcher's `--native` option to select Linux even inside WSL. Run against a repository with a small, changed text file:

```bash
python3 scripts/benchmark.py \
  --launcher ./target/release/fastdiff \
  --repository /path/to/benchmark-repository \
  --file src/main.rs \
  --hardware 'CPU; RAM; disk; GPU; desktop' \
  --runs 30 --output ./benchmark-linux.json
```

For native Windows, run Python on Windows and give native paths. For WSL → Windows, run the driver from Windows with `--wsl-distro` and the absolute Linux launcher/repository paths:

```powershell
python scripts/benchmark.py --launcher /home/you/.local/bin/fastdiff `
  --repository /home/you/benchmark-repository --file src/main.rs `
  --wsl-distro Ubuntu --hardware 'CPU; RAM; disk; GPU; WSL2' `
  --runs 30 --output benchmark-wsl.json
```

Configure `FASTDIFF_WINDOWS_EXE` in the environment used for the WSL benchmark; the driver forwards it when set. WSL is expected to be running already. The driver passes the report path and exit timing through `WSLENV` so the Windows GUI writes telemetry to a Windows-accessible temporary directory.

You can also run the driver inside WSL. Use a Windows-accessible report directory: a Windows-mounted drive or a WSL directory accessible through its Windows share. This mode queries the native Windows process's resident memory through PowerShell; it does not mistake the Linux launcher's memory for GUI memory:

```bash
python3 scripts/benchmark.py --launcher "$HOME/.local/bin/fastdiff" \
  --repository /home/you/benchmark-repository --file src/main.rs \
  --windows-from-wsl --report-directory /mnt/c/Users/you/AppData/Local/Temp \
  --hardware 'CPU; RAM; disk; GPU; WSL2' --runs 30 --output benchmark-wsl.json
```

Keep the JSON result as evidence. It uses the nearest-rank p95 and reports raw samples, incomplete runs, idle RSS, and target outcomes. A launch failure fails the benchmark instead of being silently excluded. First-frame telemetry indicates the first completed UI frame, not a compositor-present acknowledgement. Since LIT-12, `first_diff_ms` is bound to the requested repository, file, and view; see [Readiness definitions](#readiness-definitions).

Check clipboard, refresh, navigation responsiveness, and complete shutdown separately on both native platforms. When telemetry is enabled, the GUI also records `measured_frames` and `max_frame_cpu_ms` after the first diff loads. It uses eframe's [frame CPU duration](https://docs.rs/eframe/0.33.3/eframe/struct.IntegrationInfo.html): UI work plus rendering, excluding vsync waits. These values are written on shutdown. They measure main-thread frame work during the exercised interactions, rather than keyboard-to-compositor latency. They are not inferred from startup measurements.

**Historical evidence, pre-0.1.7.** The unchanged [Linux baseline](benchmark-linux.json) and [Windows/WSL baseline](benchmark-wsl-windows.json) were recorded on 2026-10-06 for the original viewer, before 0.1.7. They used a two-file fixture and the legacy readiness signal, which could also fire on empty or unrelated states. They are schema 1 results, carry no build identity, and describe neither current builds nor warm reopening, file navigation, or interactive refresh. Do not extrapolate them to 0.1.7 or later. Each completed 30 launches and passed startup and memory targets. Their launcher and fixture paths are historical and were removed during artifact cleanup. Hardware and raw samples remain in the JSON files. No fresh 0.1.7 benchmark was run; current release checks and archive sizes are in [validation](validation.md).

## Readiness definitions and end-to-end scenarios (LIT-12)

LIT-12 adds instrumentation, a scenario harness, and fixtures. It records no new measurements: running the matrix below needs an explicit user command, and no result here is a baseline or speedup claim.

### Readiness definitions

GUI telemetry is report schema 2 (the control file stays schema 1) and keeps every schema 1 field. It separates three moments:

| Field | Meaning | Source |
|---|---|---|
| `ack_ms` | Launcher acceptance (the launcher process returned successfully) | Harness, `launcher_ms` |
| `ui_ready_ms` | First completed UI frame; same value as `first_frame_ms` | GUI |
| `requested_content_ms` | The requested repository, file, and view is the selected tab's loaded diff and has been shown; same value as `first_diff_ms`, which is a deprecated alias for one release | GUI |

`requested_content_ms` completes only when the selected tab's repository, file, and view match the active request, a diff presentation exists, and the tab is neither loading nor in an error state. Empty states, errors, loading states, unrelated tab or reflog events, stale or duplicate sample IDs, and the full-document fallback for an unchanged file do not complete a sample. Fixtures therefore leave their target files modified or untracked. Reports also carry `build` (version, optional `FASTDIFF_BUILD_REVISION`, profile, protocol) and `platform` (OS, architecture); the harness records its own environment and fails a run whose GUI build or telemetry schema changes midway. `FASTDIFF_BUILD_REVISION` is read at compile time and is `null` unless the build sets it, so identify the exact source revision in `--software`. A report that still has schema 1 is labelled `readiness_identity_bound: false`. Under schema 2, `first_diff_ms` is the identity-bound value, which is stricter than the schema 1 signal, so the unchanged `targets_met.first_diff` threshold is applied to a narrower measurement; the report-level `measured_frames` and `max_frame_cpu_ms` also reset each time a new sample is accepted.

While a sample is armed and unpublished, the GUI repaints and polls its control file every 25 ms. Once published, it polls every 250 ms (`control_poll_ms` in the report) so the next warm request, edit trigger, or close is noticed. This polling is part of the measured window: idle CPU and report-level frame counts in a scenario or `--launch-profile` run include those wake-ups, and the harness waits up to three seconds for the GUI to arm an edit trigger. Compare them only with runs that use the same `control_poll_ms`. Legacy runs with no control file have no polling.

### Scenarios

`scripts/benchmark.py --scenario NAME` requires `--software` and a marked scratch file as in the launch-profiling recipe below. Each step edits the marker before its timer starts.

| Scenario | Steps |
|---|---|
| `open` | Request with the application closed (cold) |
| `reopen` | Request with the application already open (warm) |
| `navigate_aba` | File A, then B, then A in one window (`--second-file`) |
| `switch_view` | Same file in two views (`--views all,unstaged` by default) |
| `save_update` | Edit the open file; the harness waits for the GUI to acknowledge arming, then edits, and latency runs from its `trigger_unix_ms` |

Results report p50 and nearest-rank p95 per labelled step and for the scenario. Views are `all`, `unstaged`, `staged`, and `unpushed`. A `save_update` sample whose arming is not acknowledged before the edit fails the run instead of being pooled. Within one sample, `content_ready_ms` and `render_ms` are less reliable if an automatic refresh lands between arming and the edit; the marker still prevents a false completion.

### Resources and fixtures

Each sample reports `gui_only` and `total` resource blocks: RSS, idle CPU percent over the idle window, process count, and an explicit `unavailable` list. `total` covers the GUI process tree, plus the WSL helper and child Git when measurable. Totals are all-or-nothing: if any component is unavailable, `total.rss_bytes`, `process_count`, and `idle_cpu_percent` are `null`, and what was measured is kept in `partial_lower_bound`. `complete` is true only when nothing was unavailable and no measured process exited inside the window; `incomplete_reasons` names each case. Summaries compute `total` statistics over complete runs only and report `complete_runs` against `measured_runs`. On Linux, CPU that exited children spent is taken from the reaping process's counters. On Windows an exited process's later CPU is not counted, and a process that starts and exits between the two readings cannot be detected. The WSL helper is matched by `--helper <repository>` and must have started after the probe, so a helper another user starts later for the same repository is indistinguishable. `idle_rss_bytes` keeps its GUI-only meaning. The GUI also reports per-sample `frames`, `frame_cpu_total_ms`, and `max_frame_cpu_ms` (frame work from acceptance to the rendered frame); launch-profile summaries aggregate them as median, p95, and max.

`scripts/benchmark_fixtures.py --kind small|files-1k|files-10k|rows-100k --destination DIR` creates deterministic repositories (two files, 1,000 or 10,000 changed files, one 100,000-row file). The `rows-100k` changed file carries the `FASTDIFF_LAUNCH_MARKER` on its first line so scenarios can drive it. Git runs with `GIT_*` variables stripped. The generator refuses destinations inside the checkout or a non-empty directory. It is opt-in and is never part of default tests.

The measurement matrix (native Linux, native Windows, and WSL → Windows; application closed and open; small and large change sets) remains unrun. The Windows process-tree query and WSL helper discovery have been unit-tested only with fakes.


## Automatic refresh and inactive-tab work (LIT-1)

Startup/RSS baselines do not measure periodic repository work. For each source revision, use the same three small Git repositories, each with one already-modified `main.rs`; open one tab and then three tabs with the last tab selected. After five seconds of settling, sample each case for 60 seconds. Count Git invocations separately for each repository. Record app and child Git CPU separately, and collect at least 30 repeated edits to the already-modified selected file with raw samples and nearest-rank p95 latency.

The ignored `app::refresh_measurement::lit1_refresh_measurement` test uses the existing `egui_kittest` harness with real native repository workers. It calls the real app polling/rendering path every 50 ms during idle sampling and every 10 ms while waiting for edits. An edit completes when the edited marker appears in the generated egui text shapes. It records Linux `/proc/self/stat` user/system and reaped-child CPU ticks; divide by `getconf CLK_TCK` to obtain CPU seconds. This is headless app-plus-worker evidence: fixed-cadence harness CPU is not native GUI CPU, and generated frame output is not a compositor-present acknowledgement. There is no WSL helper in this test.

Use the retained local toolchain or an equivalent matching Rust 1.90 toolchain. This recipe creates temporary fixtures and a Git counting wrapper; it leaves the installed app and user Git configuration alone:

```bash
measurement_dir=$(mktemp -d)
mkdir -p "$measurement_dir/fixture" "$measurement_dir/git-shim"
for index in 0 1 2; do
  repo="$measurement_dir/fixture/repo-$index"
  /usr/bin/git init -q -b main "$repo"
  printf 'fn main() { println!("original"); }\n' > "$repo/main.rs"
  /usr/bin/git -C "$repo" add main.rs
  /usr/bin/git -C "$repo" -c user.name='LIT-1 measurement' \
    -c user.email=lit1@example.invalid commit -qm Fixture
  printf 'fn main() { println!("edit-000"); }\n' > "$repo/main.rs"
done
cat > "$measurement_dir/git-shim/git" <<'SH'
#!/bin/bash
printf '%s\t%s\t%s\n' "$EPOCHREALTIME" "$PWD" "$*" >> "$LIT1_GIT_LOG"
exec /usr/bin/git "$@"
SH
chmod +x "$measurement_dir/git-shim/git"
CARGO_HOME="$PWD/target/local-tools/fastdiff-cargo" \
RUSTUP_HOME="$PWD/target/local-tools/fastdiff-rustup" \
LIT1_FIXTURE="$measurement_dir/fixture" \
LIT1_REPORT="$measurement_dir/refresh.json" \
LIT1_GIT_LOG="$measurement_dir/git.log" \
PATH="$measurement_dir/git-shim:$PATH" \
target/local-tools/fastdiff-cargo/bin/cargo test --locked --offline \
  -p fastdiff-gui --lib app::refresh_measurement::lit1_refresh_measurement \
  -- --ignored --exact --nocapture --test-threads=1
```

Keep the report and Git log. Reset `main.rs` in all three fixtures to `edit-000` before the next revision's sample, and keep the wrapper, fixture paths, frame cadence, build profile, and environment identical. The wrapper adds Bash execution overhead equally to both revisions. Avoid concurrent heavy builds during CPU/latency samples. For native desktop and Windows-to-WSL evidence, repeat the 60-second cases with actual GUI/helper processes and verify edits, activation/loading, and shutdown on those platforms; the headless result does not establish that bridge or desktop behavior.


The [LIT-1 headless before/after evidence](benchmark-lit1-headless.json) was recorded on 2026-10-10 using the same WSL2 host, fixture paths, wrapper, test build profile, frame cadence, and successful-log counting logic. Pre-change production source was snapshotted before implementation. The after harness fails if its Git log is unreadable; both captured logs were readable and nonzero. Separate harness/source hashes and the diagnostic-only difference are preserved in the JSON.

| 60-second idle case | Before Git calls | After Git calls | Before headless app / child Git CPU | After headless app / child Git CPU |
|---|---:|---:|---:|---:|
| One tab | 270 | 270 | 3.13 s / 0.86 s | 3.06 s / 0.78 s |
| Three tabs, third selected | 810 | 270 | 3.79 s / 2.59 s | 3.43 s / 0.79 s |

The three-tab sample reduced Git subprocesses by 66.7%; each inactive repository went from 270 calls to zero, while the selected repository retained its 270 calls. The one-tab sample was unchanged. Each phase completed 30 same-size repeated edits to an already-modified file. Edit-to-generated-headless-frame p95 was 400.13 ms before and 322.08 ms after. These CPU and latency values are raw samples, not general speedup claims: necessary compiler gates overlapped the baseline host, and the fixed-cadence headless loop adds its own CPU work. This evidence does not measure native GUI/helper CPU or native Windows-to-WSL refresh latency.


Fresh debug binaries from the settled source also passed functional smoke checks on native Linux under WSLg/X11 and native Windows MSVC through its matching protocol 4 WSL helper, using isolated IPC runtimes. Complete-window screenshots showed two repeated edits to an already-modified selected file on each platform, three repository tabs, and the latest edit after reactivating an inactive repository (existing-request reopen on Linux; real mouse tab selection on Windows). Native Windows selected-tab closure and normal GUI/helper shutdown passed; Linux accepted normal window closure. The native earlier-tab closure case and loading-state preservation are covered by the headless UI tests; the native activation spinner was not captured reliably. Source hashes, verified markers, and exact native check limits are included in the evidence JSON. Screenshots and native reports remain locally in `target/lit1-measurement/`.

Native before/after GUI/helper CPU samples and native edit-to-visible latency samples remain outstanding. The native functional smokes do not fill those performance gaps or establish release startup p95 targets.

## Indexed changed-file merge (LIT-4)

The [LIT-4 merge benchmark](benchmark-lit4-merge.json) was recorded on 2026-10-11 on an AMD Ryzen 5 3600 under WSL2 using Rust 1.90.0 and `rustc -O`. It compares the exact pre-change linear-search function with the indexed function, using the production model struct layouts. It measures only the merge, excluding fixture generation/cloning, output checks/drop, Git subprocesses, and GUI work. Each variant had one warmup and five timed samples. Full output equality and first-occurrence order were asserted for every timed fixture.

Distinct records have 32-byte raw destination paths split between working and committed inputs. Overlapping inputs contain one working and one committed record per destination. Fixtures include synthetic flags, old paths, and committed comparisons; the focused production tests separately cover metadata precedence, conflicts, and rename semantics.

| Distinct destinations | Input case | Old median | Indexed median | Speedup | Added peak requested heap |
|---:|---|---:|---:|---:|---:|
| 1,000 | Distinct records | 1.916 ms | 0.166 ms | 11.5× | 0.124 MiB |
| 10,000 | Distinct records | 193.818 ms | 1.798 ms | 107.8× | 0.821 MiB |
| 50,000 | Distinct records | 6,411.313 ms | 10.189 ms | 629.2× | 3.588 MiB |
| 1,000 | Overlapping inputs | 3.936 ms | 0.368 ms | 10.7× | 0.124 MiB |
| 10,000 | Overlapping inputs | 521.829 ms | 3.715 ms | 140.5× | 0.821 MiB |
| 50,000 | Overlapping inputs | 27,856.183 ms | 27.904 ms | 998.3× | 3.588 MiB |

Memory was measured separately with a counting allocator forwarding to `System`, so allocation tracking did not affect the timing samples. The added peak is the difference between the two variants' peak live requested heap bytes above their input baselines; baseline subtraction removes process argument size differences. It includes the hash table and cloned destination paths. At 50,000 destinations with 128-byte paths, the additional peak was 8.166 MiB in both input cases. These memory-only cases asserted output length. All eight cases had zero old/new difference in net live requested heap after consuming the inputs: the index is released before the function returns.

These results demonstrate improved merge scaling on these fixtures. Requested live heap excludes allocator bookkeeping, retained arenas, internal reallocation copies, stack, and OS resident memory. Hash-table capacity steps and path lengths affect the peak; these measurements do not establish a fixed cost per destination. There are no confidence intervals or end-to-end refresh, native Windows, or GUI latency measurements.

The JSON retains raw samples, environment details, measured-source hashes/snapshots, and reproduction sources. To reproduce with the retained toolchain from the repository root:

```bash
python3 - <<'PY'
import json
from pathlib import Path
report = json.loads(Path('docs/benchmark-lit4-merge.json').read_text())
output = Path('target/lit4-measurement')
output.mkdir(parents=True, exist_ok=True)
for name, source in report['reproduction_sources'].items():
    (output / name).write_text(source)
PY
target/local-tools/fastdiff-rustup/toolchains/1.90.0-x86_64-unknown-linux-gnu/bin/rustc \
  --edition=2024 -O target/lit4-measurement/harness.rs -o target/lit4-measurement/timing
target/local-tools/fastdiff-rustup/toolchains/1.90.0-x86_64-unknown-linux-gnu/bin/rustc \
  --edition=2024 -O --cfg memory_measurement target/lit4-measurement/harness.rs \
  -o target/lit4-measurement/memory
python3 target/lit4-measurement/run.py
```

## Managed and direct launch profiling (LIT-6)

LIT-6 now has opt-in profiling support; no LIT-6 performance samples have been collected. The existing results above do not establish managed-command overhead or warm-request latency. Benchmark execution, coverage, full suites, and native platform smoke checks require a separate explicit user command. The issue remains open until measurements and a ranked recommendation are recorded.

Use `scripts/benchmark.py --launch-profile` to compare the installed `dff` entry point with its paired direct Rust launcher. This mode keeps the existing driver and output schema, adds `--case cold|warm`, `--command-kind direct|managed`, and `--trace-phases`, and requires `--software` to identify matching binary revisions, build profiles, Git/Rust/WSL versions, and desktop/GPU configuration. Record CPU/RAM/disk in `--hardware`. Use matching instrumented release binaries and staged runtime scripts from the same revision in isolated installations. Historical 0.1.7 binaries lack the new request telemetry; they cannot produce these samples. Do not update the user's installation to prepare a measurement.

Create a dedicated small scratch repository with one tracked text file. Put the placeholder `FASTDIFF_LAUNCH_MARKER` exactly once on its first line, then commit it and leave the file modified. For example, this recipe creates a new fixture rather than editing the application checkout:

```bash
fixture_dir=$(mktemp -d)
git init -q -b main "$fixture_dir"
printf '// FASTDIFF_LAUNCH_MARKER\nfn main() {}\n' > "$fixture_dir/main.rs"
git -C "$fixture_dir" add main.rs
git -C "$fixture_dir" -c user.name='LIT-6 fixture' \
  -c user.email=lit6@example.invalid commit -qm Fixture
printf '// FASTDIFF_LAUNCH_MARKER\nfn main() { println!("changed"); }\n' > "$fixture_dir/main.rs"
```

The profiling flag authorizes temporary marker replacements in this explicitly marked scratch file. The driver restores its original bytes on exit, checks for external edits before each replacement and restoration, and preserves conflicting external edits with a reported failure. Keep the fixture idle during measurement. Put the marker near the start of a short line so it fits within the source pane. Pass a repository-relative file path with `/` separators; repository paths must identify the repository root. Each request carries a unique sample ID and marker through an opt-in control file beside the report, without changing helper protocol 4.

For Linux, repeat this command for each combination of `direct|managed` and `cold|warm`, changing `--launcher` between the isolated direct binary and staged `dff` wrapper:

```bash
python3 scripts/benchmark.py --launch-profile \
  --launcher /path/to/isolated/bin/dff --command-kind managed --case warm \
  --repository "$fixture_dir" --file main.rs \
  --hardware 'CPU; RAM; disk; GPU; desktop' \
  --software 'binary revision; release profile; Rust; Git; OS/WSL versions' \
  --runs 30 --output ./lit6-linux-managed-warm.json
```

Linux profiling passes `--native`, including under WSL. Native Windows uses Windows Python, native repository/executable paths, and the staged `dff.cmd` wrapper for managed samples. CMD wrapper paths containing expansion/control metacharacters are rejected; direct executable and POSIX invocations retain literal argument handling. For WSL → Windows, add `--windows-from-wsl --report-directory /mnt/c/Users/you/AppData/Local/Temp` when running this same recipe inside WSL. A Windows-hosted driver can instead use `--wsl-distro Ubuntu` with absolute Linux launcher/repository paths; it edits the scratch file through the WSL share. Use matching Windows GUI and Linux helper binaries. Direct WSL runs also require `FASTDIFF_WINDOWS_EXE` to identify that isolated Windows GUI; managed `dff` uses its validated receipt. The launcher propagates report and isolated IPC paths across WSL with `WSLENV` path conversion. Existing installed viewer windows are left alone.

Cold means a fresh GUI process with OS caches retained. Each cold sample uses a separate IPC directory and waits for automatic GUI exit before the next sample. Warm means one existing isolated GUI with WSL already running: an uncounted seed request starts it, then all 30 measured requests must render in that same PID. The driver changes the marker before starting each timer, waits for correlated repository/file/marker telemetry from a completed generated GUI frame, and measures native idle RSS after one second. A close control shuts down the warm GUI; an automatic exit timer remains as a fallback. The driver never kills a PID supplied by telemetry. Launcher acknowledgement alone and earlier reports cannot count as success. Generated frame completion is not a compositor-present acknowledgement.

The JSON retains successful raw samples, GUI reports, executed argument lists, warmup, failures, median, and nearest-rank p95. A failed or incomplete run fails the command rather than dropping the sample. Existing `first_frame_ms`, `first_diff_ms`, `launcher_ms`, and `idle_rss_bytes` sample fields remain. Warm samples have `first_frame_ms: null` because startup's first frame is not a new request's frame; `first_diff_ms` and `request_to_render_ms` both measure the requested marker's completion. Legacy cold benchmarking without `--launch-profile` keeps its target checks, but its `first_diff_ms` is now identity-bound (LIT-12) and its result is schema 2 with the schema 1 keys retained. Profiling exit status indicates complete measurement collection, not a latency target pass.

Collect phase traces separately by repeating each case with `--trace-phases` and a separate output file. Normal profiling explicitly clears inherited launch-trace variables. Phase runs retain the correlated JSONL rows and phase median/p95 values alongside end-to-end samples; compare trace-enabled and ordinary end-to-end distributions to identify instrumentation overhead. Phases are monotonic elapsed measurements within each process:

| Phase | Boundary |
|---|---|
| `python_startup` | Driver process start timestamp to Python entry timestamp; includes wrapper/pre-entry startup |
| `python_imports` | Python entry to completion of existing runtime/maintenance imports |
| `receipt_version` | Initial installed-version receipt read and version check |
| `receipt_validation` | Full paired-installation and package validation before Rust launch |
| `repository_validation` | Rust path canonicalization and repository discovery |
| `forwarding_probe` | Native IPC attempt when no instance accepts the request |
| `process_start` | GUI/WSL bridge spawn call, excluding subsequent startup/acceptance |
| `forwarding_acceptance` | Existing-instance IPC acceptance, or spawned process's ready acknowledgement |
| `content_ready_ms` | GUI acceptance to prepared selected document |
| `render_ms` | Prepared selected document to generated frame containing the visible marker |

Python startup and request-to-render timestamps require aligned system clocks across WSL/Windows. Phase values do not sum to end-to-end latency: argument parsing, Python-to-Rust startup, executable/helper discovery, IPC queueing, rendering scheduling, and overlapping work remain outside or overlap these boundaries. A missing required phase invalidates a detailed sample; optional trace write failures preserve ordinary launch/error behavior.

Focused implementation evidence: new harness tests first failed because profiling support did not exist, then passed with current-marker validation, literal arguments, malformed phase rejection, scratch restoration, and repeated warm-request simulation. The simulation exposed and then verified the fix for close-control cleanup order. Review regressions also failed then passed for failed/malformed warm seeds, CMD parentheses, and rejected-seed idle close polling. Incomplete cold samples rely on the four-second exit timer. Runtime/CLI and GUI telemetry tests exercised disabled instrumentation and request reset/completion. Focused Windows Python checks passed for profiling, tracing, and managed commands, including the existing native CMD wrapper tests; the Windows command suite skipped its two POSIX-only cases. These tests are functional evidence, not performance measurements or native GUI/bridge verification.

Focused test targets passed during implementation; equivalent reproduction commands:

```text
PYTHONPATH=scripts python3 -m unittest scripts/test_launch_profile.py scripts/test_benchmark.py
PYTHONPATH=scripts python3 -m unittest scripts/test_launch_trace.py scripts/test_plugin_runtime.py scripts/test_plugin_command.py
cargo test --locked --offline -p fastdiff-cli --test launch_process
cargo test --locked --offline -p fastdiff-gui --lib benchmark
cargo test --locked --offline -p fastdiff-gui --lib tracked_marker_requires_a_visible_source_cell
cargo test --locked --offline -p fastdiff-gui --lib rejected_sample_keeps_polling_control_until_idle_close
python.exe -m unittest discover -s scripts -p test_launch_profile.py
python.exe -m unittest discover -s scripts -p test_launch_trace.py
python.exe -m unittest discover -s scripts -p test_plugin_command.py
```

Rust commands used the retained local toolchain described above; local IPC tests needed sandbox escalation. Formatting and scoped whitespace checks passed. Coverage and full-workspace/E2E/platform smoke runs were not performed.

Pending measurement matrix: Linux, native Windows, and WSL → Windows, each with managed/direct × cold/warm × ordinary/detailed runs, with at least 30 successful samples per case. Preserve exact binary hashes/revisions, commands, platform versions, raw data, and failures. Assess maintenance import laziness only after its measured share is known. The lightweight Windows CLI bridge remains a follow-up experiment: the current launcher still invokes the Windows GUI bridge and has no transport redesign. No optimization ranking or speedup is justified yet.

## Bounded prepared-diff navigation cache (LIT-7)

LIT-7 adds a worker-owned LRU with four entries and 16 MiB of accounted storage per tab. Every navigation still checks availability and reads/parses fresh Git output. Reuse requires the complete `View`/`FileChange` key and equality of the entire fresh `DiffDocument`; the existing current-document shortcut also remains. Accepted status snapshots prune obsolete keys for their view. Cache values are immutable prepared documents; scroll, selection, folds, and search state remain outside it. Standalone full-file loading retains its existing path. Public interfaces and helper protocol 4 are unchanged.

Storage accounting includes reserved cache slots, raw key paths and comparison IDs, source/patch strings and vector capacities, highlights, context flags, and display-line arrays. Shared prepared-document and display-line allocations are counted once, using saturating arithmetic. Oversized documents bypass admission and remain displayable. The budget excludes active documents outside the cache, temporary Git output/preparation, allocator overhead, and other tabs, so it is not a process RSS ceiling.

No LIT-7 timings or RSS measurements have been collected. Correctness tests establish freshness and bounded retention; they do not establish a navigation speedup. LIT-7 stays open pending measurements, and LIT-6's deferred measurements remain deferred.

An ignored measurement test uses the existing scratch-repository fixture, fresh native backend reads, and the actual preparation/cache code. Run it only after an explicit benchmark command from the user. With the retained local toolchain, the reproduction command is:

```bash
CARGO_HOME="$PWD/target/local-tools/fastdiff-cargo" \
PATH="$PWD/target/local-tools/fastdiff-rustup/toolchains/1.90.0-x86_64-unknown-linux-gnu/bin:$PATH" \
cargo test --locked --offline -p fastdiff-gui --lib \
  measure_prepared_diff_cache_navigation -- --ignored --nocapture --test-threads=1
```

Record the emitted JSON with the exact source revision/dirty patch, build profile, Rust/Git versions, OS/WSL version, and CPU/RAM/disk details. Compare disabled and default caching over 30 A → B → A cycles for 400-line and 4,000-line files, then exercise eight-file churn and oversized admission with a 1 KiB test-only limit. Retain every raw navigation/backend/preparation time, median/nearest-rank p95, cache/current reuse counts, accounted retained bytes, and process RSS (unavailable platforms report null). Disabled runs precede enabled runs in the same process; Git/OS warming and allocator retention can bias timing and RSS, which is sampled after each navigation. Inspect raw samples and memory retention before drawing conclusions. The harness measures worker-level backend/preparation work, excluding UI scheduling/rendering and launch/IPC overhead; it is not end-to-end user-visible latency evidence. Debug-profile evidence alone must not be presented as a release-performance result.

Focused TDD evidence: the new real-worker A → B → A test first failed at original-`Arc` reuse, then passed with cache admission enabled. The cache and real-worker tests passed for LRU promotion/replacement, full-key/full-document equality, byte/entry limits, oversized bypass, cancellation, status pruning, restored timestamps, staging/views/refs, renames/deletions/raw paths, backend errors, and search of a cached document. Existing worker tests and four UI/render regressions passed, preserving stale-event rejection and scroll/folding state. Reproduction targets (without measurement execution):

```text
cargo test --locked --offline -p fastdiff-gui --lib worker::cache
cargo test --locked --offline -p fastdiff-gui --lib worker::tests
cargo test --locked --offline -p fastdiff-gui --lib stale_diff_result_does_not_replace_selected_file
cargo test --locked --offline -p fastdiff-gui --lib reactivating_tab_rejects_previous_diff_and_reflog_results
cargo test --locked --offline -p fastdiff-gui --lib automatic_refresh_preserves_selected_comparison_and_scroll
cargo test --locked --offline -p fastdiff-gui --lib refresh_preserves_expansion_layout_and_horizontal_state
```

These targets used the retained Cargo home/toolchain above. Coverage collection, full-workspace suites, E2E, and native Linux/Windows/WSL GUI smoke checks remain deferred; no current coverage percentage or performance gain is established.


## Watcher noise and paged reflog (LIT-9)

Current development filters native watcher events and loads reflog rows in pages of 15. It retains recursive registration, the 250 ms debounce, native two-second freshness checks, and WSL polling. Initial registration costs and OS watch counts are unchanged. No LIT-9 startup, refresh, or large-tree/reflog performance measurements have been collected.

When measurement is explicitly requested, extend the LIT-1 Git-counting recipe above with identical fixtures and builds before/after LIT-9:

1. Record cold startup separately for small and large directory trees, with Reflog closed and open. Count registrations and Git subprocesses separately from launcher acceptance and the first requested diff frame.
2. Run 60-second settled native cases idle, while repeatedly writing ignored build outputs, and while editing tracked files. Include an explicitly opened ignored document. Count `check-ignore`, status/diff, snapshot-layout, and `reflog show` commands separately; record raw edit-to-visible samples. Repeat WSL cases to confirm retained polling and helper request costs.
3. Generate controlled histories at 0, 15, 16, 30, and many thousands of entries, including repeated resets. Open Reflog, scroll to exhaustion, wait idle, change a ref decoration, append an entry, and expire history. Record request offsets/output bytes, loaded-row counts/order, reset behavior, and time to requested rows.
4. Repeat with linked worktrees and separate Git directories, atomic saves, missing/recreated document parents, and failed watcher coverage. Observe registrations/notifications directly so periodic fallback cannot mask broken watching.

Preserve revisions, binary hashes, fixture sizes, Git/Rust/OS/WSL versions, hardware, commands, raw samples and failures. Metadata scans scale with ref count; fingerprints provide practical consistency rather than transactional isolation. Layouts without reliable fingerprints reread the loaded prefix and may have higher repeated-read cost. Benchmarks, coverage, full suites and platform smoke checks remain deferred. Performance measurements are optional follow-up work and do not block LIT-9 implementation completion after focused correctness tests and review; no measured speedup is established.

## Direct file opens and idle polling (LIT-34)

A standalone document (`fastdiff <file>`) now builds its filesystem watcher after the first content is shown, as a repository review already did (LIT-26), and re-reads the document as soon as the watcher exists so an edit made in between is not left to the two-second poll. While a document is open, the branch and Git status are refreshed on events, explicit refreshes and every 10 seconds instead of every poll (WSL and a failed watcher raise no events, so there Git runs every 10 seconds); the file itself is still re-read every two seconds. Full-file preparation builds its single view directly, UTF-8 decoding reuses the read buffer, and the fallback-font scan borrows file names.

`scripts/benchmark.py --document` opens `--file` (an absolute path) as a standalone document instead of a file of `--repository`; it is only valid for the plain native launch benchmark. Compare identical fixtures: the same file outside Git, in a small repository, and in a repository with thousands of directories.

Recorded 2026-10-19, release builds, 30 cold opens per case (a few cases were shorter, noted below), sequential rather than interleaved before/after runs. Time to requested content, median:

| Case | Before | After | Same file outside Git |
| --- | ---: | ---: | ---: |
| Linux (WSL2/WSLg), repository with 6,000 directories | 233 ms | 92 ms | 88 ms |
| Linux, small repository | 144 ms (13 samples) | 94 ms | 88 ms |
| Windows native, repository with 6,000 directories | 581 ms | 347 ms | 274 ms |

First frame is unchanged (about 72 ms on Linux, 180 ms on Windows). On Windows about 73 ms remains above the outside-Git case; the Git discovery before the window reports its root and the branch lookup before the first read are still synchronous.

Steady-state idle CPU on Linux with a document open (30 s window after 6 s, GUI process plus reaped Git children, benchmark telemetry enabled in all runs so absolute values are inflated):

| Case | Before | After |
| --- | ---: | ---: |
| Outside Git | 2.6% | 2.5% |
| Small repository | 6.0% | 3.2% |
| Repository with 6,000 directories | 16.3% | 5.1% |

Measured and left unchanged:

- **Full-document preparation and decoding:** about 17 ms (9%) faster at the median on an 88,000-line 8.6 MiB file on Linux (195 ms with 16 samples before, 178 ms after); no difference for small files. Peak memory was not compared.
- **First fallback-font install (Windows, `msyh.ttc`, 19.7 MB):** about 5 ms to read the file, 5 ms to install it and 3 ms for the first frame afterwards, so well under a perceptible pause with a warm file cache. Moving the read off the UI thread is not warranted on this evidence. A cold disk, other platforms, and the glyph scan of a large covered-accent document (a cost that stays on every change) were not measured.
- **Managed `dff` imports:** importing the runtime costs about 21 ms, 13 ms of it `urllib.request`, against about 10 ms for a bare interpreter. Making the maintenance imports lazy would save at most that, so it was not done.

Not measured: macOS, interactive stalls from the font install, Windows idle CPU, and the interleaved before/after comparison the README method asks for. The repository-review path was not changed; its native Windows README row was re-recorded separately (README, `docs/benchmark-windows-native-2026-10-19.json`).
