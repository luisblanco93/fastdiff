# Changelog

User-facing changes to fastdiff. Release notes are taken from the matching version section, so write each entry for someone installing or updating the app. Versioning and release rules are in [docs/releasing.md](docs/releasing.md).

## [Unreleased]

## [1.3.1] - 2026-10-27

### Added

- The fastdiff app icon now appears on the window and taskbar, and on `fastdiff-gui.exe` in Windows Explorer.

## [1.3.0] - 2026-10-27

### Added

- Color themes: a **Theme** button (or Ctrl+K then Ctrl+T) offers Default, Dark Modern, One Dark Pro, Dracula, Catppuccin Mocha, and GitHub Light. Each theme sets the window, diff, and syntax colors; hovering a theme previews it and Esc restores the previous one. The menu can follow the system light or dark mode (Windows and macOS) and give the code viewer its own theme. `--color-theme NAME` sets the theme for one window without saving it. Function names and type names are now highlighted in code.
- The theme choice and the UI zoom are remembered between launches in `fastdiff/settings.json` in your config directory.

### Changed

- The launcher and the viewer must come from the same version: an older running viewer rejects launches that carry `--color-theme`.
- Release archives include `licenses/color-themes-NOTICE` with the licenses of the theme colors.

## [1.2.0] - 2026-10-19

### Changed

- The toolbar and tab strip are redesigned. Each tab shows a folder or document icon and, for duplicate names, its parent folder; the active tab is filled and every tab has a dimmed close control. The window-wide UI size, zoom and Help controls sit at the right end of the tab row. The toolbar keeps search and the Full file, Git Diff and Git views, and breaks into two rows below 620 px. A path row always shows the path with the Side by side and Unified toggle and Copy. Branch and path labels are selectable.
- Opening a file directly (`dff <file>`) inside a Git repository shows its content sooner: the file is shown before the repository is watched, which saves about 140 ms on Linux and 230 ms on Windows in a repository with thousands of directories. A file left open in a repository also uses less CPU, because the branch and Git status are no longer refreshed every two seconds while only the file is shown.
- Large text documents use less memory on Linux: memory freed while a document loads is returned to the system, so ten open 8 MB documents take about 16% less memory (about 90 MiB) and each additional open document costs about 34 MiB instead of 45 MiB.

## [1.1.0] - 2026-10-15

### Added

- A macOS (Apple Silicon) build: a `fastdiff-<version>-macos-arm64.tar.gz` release archive that the installer sets up as `dff`. Shortcuts use <kbd>Cmd</kbd> instead of <kbd>Ctrl</kbd> on macOS. The macOS binaries are not signed or notarized.

### Changed

- Unpushed works on a branch without an upstream: it lists the commits that are not on another local or remote-tracking branch, and shows the whole history of a branch that has no other branch to compare with. Previously the view stayed empty with a "no configured upstream" notice. Nothing is fetched, so remote branches deleted on the server but not pruned locally still count as pushed.

### Fixed

- The window chrome, including the native title bar, stays dark when the system appearance is light, instead of a light frame around dark diff panes.
- File names and diff text in Chinese, Japanese, or Korean show the characters instead of empty boxes. A system CJK font is loaded only when such text appears, so other repositories pay no startup cost.

## [1.0.1] - 2026-10-15

### Fixed

- Full File no longer fails with "os error 123" on native Windows repositories.

### Changed

- Selecting a file starts its Git comparisons together instead of one after another, and a file you already viewed is shown immediately from cache while it is re-checked. Speedup has not been measured.

## [1.0.0] - 2026-10-13

First stable release of fastdiff, the read-only Git change viewer, published from this repository.

### Added

- A native Rust window for reviewing Git changes with virtualized rows and cached diffs, opened with the `dff` terminal command (`fastdiff` from manual archives).
- One **All** view listing each path once, with Unpushed, Staged, and Unstaged comparisons labeled separately, plus dedicated `unstaged`, `staged`, and `unpushed` views and untracked files.
- Side-by-side and unified layouts, changed-file tree with filter, hunk and file navigation, in-diff search, and a local reflog panel. <kbd>F1</kbd> lists the keyboard shortcuts.
- Automatic refresh when files change, and summaries instead of freezes for binary files, conflicts, and oversized changes.
- A searchable full-text view for any text file, including files outside Git.
- `--theme` for Material file icons and a colored reflog.
- Native Linux and Windows windows, with a paired helper for repositories that live in WSL.
- An installer that downloads verified release archives, pairs the Windows app with the WSL helper, and supports `dff --update` and `dff --uninstall`.
- A shared agent skill so coding agents can open the viewer for a user.

### Changed

- The product, binaries, and environment variables are named `fastdiff`, `fastdiff-gui`, and `FASTDIFF_*`. Releases before the rename shipped as `diffcode`; the installer removes legacy `diffcode` binaries.

### Migration

- Installations made before the project moved to `luisblanco93/fastdiff` still update from the previous repository. Rerun the installer from [docs/distribution.md](https://github.com/luisblanco93/fastdiff/blob/main/docs/distribution.md#install-fastdiff) once to switch; there is no automatic cross-repository redirect.
- Legacy `DIFFCODE_*` and `DIFFVIEWER_*` environment variables are not read. Rename them to `FASTDIFF_*`.
- Keep the launcher, GUI, and WSL helper from the same release; mismatched builds are rejected.
