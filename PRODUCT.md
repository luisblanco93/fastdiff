# fastdiff


## Platform

Native desktop: Windows x64 and Linux x64, including WSL2 workflows.

fastdiff uses a shared Rust egui/eframe interface. It is not a web or mobile app. Preserve the actual supported platforms rather than applying web or mobile guidance.

## Users

Developers reviewing Git changes and text files on Windows or Linux, often while working with a coding agent. Users need to understand changes quickly and navigate large codebases without losing their place.

## Product Purpose

Provide fast, clear, read-only review of source changes and full text. Success means finding relevant files and hunks, understanding their contents, and returning to development with little interruption.

## Positioning

A compact desktop viewer that opens from a terminal or coding agent, accepts the request, and lets the conversation continue while the user reviews. It combines working-tree, staged, and unpushed comparisons with standalone text viewing.

## Operating Context

- Launch with `dff` from the active repository, or pass a repository or text-file path.
- Opening another repository adds a tab in the existing app.
- WSL opens the Windows GUI by default; repository and document reads stay in the Linux helper. Native Linux viewing is also supported.
- Ordinary launch and review stay offline. Unpushed comparisons use local upstream refs and never fetch.
- Files refresh as they change. Users navigate through the file tree, filename filter, text search, keyboard shortcuts, files, and hunks.

## Capabilities and Constraints

The collapsible HEAD Reflog shows local events independently of the selected diff; arrow keys scroll source panes. See [usage](docs/usage.md) for controls and theme options, and [helper compatibility](docs/architecture.md#helper-boundary) for binary pairing.

- Git views: All, Unstaged, Staged, and Unpushed. All includes pre-existing changes and nonignored untracked files.
- Side-by-side and unified diffs, collapsible context, repository tabs, and searchable full-file viewing.
- Source selection and copying, complete long-line horizontal scrolling, and adjustable font size.
- Review is read-only: no editing, staging, reverting, checkpoints, chat connections, or review approvals. Git cannot attribute changes to a particular agent.
- Binary files, conflicts, unsupported encodings, and oversized content receive explanatory states. Automatic loading is bounded at 10 MiB per file and 100,000 rows or document lines.
- App installation, updates, and uninstall manage the app and terminal command. The coding host separately manages the canonical repository skill.
- Launcher, GUI, and helper binaries must be compatible and paired.

## Product Principles

1. Make review fast and clear; avoid interruptions and unnecessary steps.
2. Help users navigate large codebases and change sets while keeping their location and comparison understandable.
3. Preserve source files and repository state; reviewing and copying never alter them.
4. Keep the terminal-to-desktop workflow lightweight, with actionable loading and launch errors.

## Evidence on Hand

- [README](README.md): public product scope and launch workflow.
- [Usage](docs/usage.md): review controls, shortcuts, loading limits, and error states.
- [Architecture](docs/architecture.md): process boundaries, offline behavior, and WSL routing.
- [Distribution](docs/distribution.md): supported platforms and application/skill lifecycle.
- [Performance](docs/performance.md) and [validation](docs/validation.md): documented targets, measured results, and verification limits. Treat those as evidence for their recorded builds and conditions, not guarantees for future changes.

## Open Decisions

No product-specific accessibility standard or additional audience has been confirmed. Preserve existing keyboard navigation, selection/copy behavior, and font-size controls; do not treat the absence of a named standard as permission to remove them.
