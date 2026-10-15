# Changelog

User-facing changes to fastdiff. Release notes are taken from the matching version section, so write each entry for someone installing or updating the app. Versioning and release rules are in [docs/releasing.md](docs/releasing.md).

## [Unreleased]

### Added

- A macOS (Apple Silicon) build: a `fastdiff-<version>-macos-arm64.tar.gz` release archive that the installer sets up as `dff`. Shortcuts use <kbd>Cmd</kbd> instead of <kbd>Ctrl</kbd> on macOS.

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
