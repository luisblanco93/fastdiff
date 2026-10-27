<h1 align="center">fastdiff</h1>

<p align="center">
  <b>See every change your AI coding agent makes.</b><br>
  Native. Read-only. Built with Rust.<br>
  Type <code>dff</code>, review the diff, get back to work.
</p>

<p align="center">
  ⚡ <b>Fast startup</b> &nbsp;&nbsp;&nbsp;&nbsp; 👁️ <b>Read-only</b> &nbsp;&nbsp;&nbsp;&nbsp; 🖥️ <b>Native desktop</b><br>
  <b>Install. Open. Review.</b><br>
  Windows · macOS · Linux
</p>

<p align="center">
  <a href="LICENSE-MIT"><img alt="License: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue"></a>
  <img alt="Rust 1.90+" src="https://img.shields.io/badge/rust-1.90%2B-orange">
  <img alt="Platforms: Linux, macOS, and Windows" src="https://img.shields.io/badge/platform-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows-lightgrey">
  <a href="https://github.com/luisblanco93/fastdiff/actions/workflows/build.yml"><img alt="CI" src="https://github.com/luisblanco93/fastdiff/actions/workflows/build.yml/badge.svg"></a>
</p>

<video src="https://github.com/user-attachments/assets/0ccf06d3-2c37-4ded-8359-641930450ed5" controls></video>

<p align="center">
  <img src="https://github.com/user-attachments/assets/28bc4848-8816-4b29-964a-41a7fff8ac61" alt="fastdiff showing a side-by-side diff with separate Staged and Unstaged sections and a changed-file tree" width="900">
</p>

> ## 🤖 Using a coding agent? Paste this one prompt.
>
> ```text
> Install fastdiff from https://github.com/luisblanco93/fastdiff — do NOT clone the repo,
> download a source archive, or build with Rust.
>
> 1. Read https://raw.githubusercontent.com/luisblanco93/fastdiff/main/docs/distribution.md
>    and run the Python one-liner under "Install fastdiff" for my platform
>    (Linux/WSL2, macOS on Apple Silicon, or Windows). It downloads the released binaries and installs `dff`.
> 2. Download https://raw.githubusercontent.com/luisblanco93/fastdiff/main/skills/fastdiff/SKILL.md
>    and save it as `fastdiff/SKILL.md` in your user-level skills directory
>    (Claude Code: ~/.claude/skills/fastdiff/SKILL.md, Codex/OpenCode: ~/.agents/skills/fastdiff/SKILL.md).
>    Never overwrite an existing file; report a conflict instead.
> 3. Reload your skills and apply any PATH guidance the installer prints.
>    Confirm with `command -v dff`.
> 4. Do NOT launch fastdiff yourself. Finish by telling me exactly how to open it:
>    - In this session, type `! dff` (opens the current repo; `! dff /path/to/repo` for another one).
>    - Or run `dff` from any terminal inside a Git repository.
>    - Useful variants: `dff . --view staged`, `dff . --view unpushed`, `dff notes.txt`.
>    - If `dff` isn't found, give me the PATH line the installer printed.
> ```
>
> Nothing is cloned: the installer fetches only a few raw files and the release archive, and checks it against `SHA256SUMS`.

| | Launch → first frame | Launch → first diff |
| --- | --- | --- |
| **Linux window** | **73 ms** median · 192 ms worst | **181 ms** median · 331 ms worst |
| **Windows window** (native, repository on NTFS) | **289 ms** median · 1,381 ms worst | **933 ms** median · 2,069 ms worst |

## Why fastdiff

- **Instant.** A native Rust window with virtualized rows and cached diffs. No Electron, no web view, no waiting on an IDE.
- **Everything that changed, in one place.** Unstaged, staged, unpushed commits, and untracked files, each labeled, plus the local reflog.
- **Built for coding agents.** `dff` returns as soon as the window accepts the request, so an agent can open the diff and carry on. A shared [skill](skills/fastdiff/SKILL.md) works with any agent that has a shell.
- **Read-only by design.** No staging, editing, or reverting. Opening a diff cannot change your repository.
- **Linux, macOS, and Windows.** Native windows on all three (Apple Silicon on macOS, from the first release that includes a macOS archive), with a paired helper for repositories that live in WSL.

## Install

fastdiff needs Python 3.11+ for the installer; the app itself needs no runtime, GitHub CLI, or Rust toolchain.

Releases are published. Install the latest one with the command for your platform in [docs/distribution.md](docs/distribution.md#install-fastdiff); it downloads the compiled release and installs the terminal command `dff`. Do not clone the repository or build from source to install.

## Use

```bash
dff                          # review the current repository (All changes)
dff /path/to/repository      # another repository opens in a new tab
dff . --view staged          # all | unstaged | staged | unpushed
dff . --file src/main.rs     # open one changed file
dff notes.txt                # searchable full-text view of any text file
dff . --theme                # Material file icons and colored reflog
dff . --color-theme dracula  # color theme for this window (see Theme menu)
```

**All** lists each path once and labels its Unpushed, Staged, and Unstaged comparisons separately. The default layout is side by side; switch to Unified next to the file path. Binary files, conflicts, and oversized changes get a summary instead of a freeze.

<table>
  <tr>
    <td width="50%"><img src="https://github.com/user-attachments/assets/90554af8-2cb0-4f60-8e65-b6eea511574b" alt="Unpushed view listing the net change of local commits"><br><sub><b>Unpushed.</b> The net change of your local commits against the last fetched upstream, or against your other branches when there is none.</sub></td>
    <td width="50%"><img src="https://github.com/user-attachments/assets/ce8c1127-2d77-415c-9844-5892fea1ab57" alt="Reflog panel below the file tree showing HEAD history"><br><sub><b>Reflog.</b> HEAD history under the file tree, <kbd>Ctrl</kbd>/<kbd>⌘</kbd>+<kbd>5</kbd>.</sub></td>
  </tr>
  <tr>
    <td width="50%"><img src="https://github.com/user-attachments/assets/24e1eb95-d3e7-4df8-b4ae-1985dcfb3968" alt="Search highlighting every match across the diff"><br><sub><b>Search.</b> <kbd>Ctrl</kbd>/<kbd>⌘</kbd>+<kbd>F</kbd> highlights every match; <kbd>F3</kbd> jumps between them.</sub></td>
    <td width="50%"><img src="https://github.com/user-attachments/assets/7ac92d63-5873-4343-a9cb-819ecca22047" alt="Keyboard shortcut reference opened with F1"><br><sub><b>Keyboard first.</b> <kbd>F1</kbd> lists every shortcut.</sub></td>
  </tr>
</table>

Files refresh automatically. Navigate with <kbd>Alt</kbd>+<kbd>↑</kbd>/<kbd>↓</kbd> (files) and <kbd>Ctrl</kbd>/<kbd>⌘</kbd>+<kbd>↑</kbd>/<kbd>↓</kbd> (hunks), filter the tree with <kbd>Ctrl</kbd>/<kbd>⌘</kbd>+<kbd>Alt</kbd>+<kbd>F</kbd>, and collapse unchanged context. Full controls and limits: [docs/usage.md](docs/usage.md).

## For AI agents: start here

1. **Understand the tool:** this README, then [docs/usage.md](docs/usage.md) for launch options and shortcuts, and [docs/architecture.md](docs/architecture.md) for the helper protocol and refresh model.
2. **Open fastdiff for a user:** follow [skills/fastdiff/SKILL.md](skills/fastdiff/SKILL.md). To install that skill into an agent, follow [docs/distribution.md](docs/distribution.md#agent-skill).
3. **Change code:** run the focused tests for the code you touched (checks are listed under [Contributing](#contributing)). Platform and install details are in [docs/distribution.md](docs/distribution.md); measurement method is in [docs/performance.md](docs/performance.md).

Ground rules: the viewer stays read-only, launcher, GUI, and WSL helper must come from the same build, never report a check you did not run, and do not tag, bump versions, or publish without explicit maintainer approval.

## Contributing

Bug reports and focused pull requests are welcome. fastdiff builds with Rust 1.90+ (`cargo build --release --locked --workspace`); run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, and `python3 -m unittest discover -s scripts -p 'test_*.py'` for the code you touch. [PRODUCT.md](PRODUCT.md) and [DESIGN.md](DESIGN.md) describe what fastdiff is and is not.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option. The bundled Material icons keep their own license, shipped with release archives.
