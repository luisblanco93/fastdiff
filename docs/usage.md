# Reviewing changes

After [installation](distribution.md#install-fastdiff), run `dff` in the active repository, or pass a repository directory or text file. Manual app archives expose `fastdiff`, which accepts the same launch arguments. File arguments open full text, including clean and ignored files or files outside Git. `--file` selects a repository-relative filename; an unchanged selected file opens as full text. `--view` accepts `all`, `unstaged`, `staged`, or `unpushed`.

Any coding agent can use the same [fastdiff skill](../skills/fastdiff/SKILL.md) to install or launch the viewer through its shell tool.

```bash
dff . --file 'src/a file.rs' --view unstaged
```

Use `dff . --theme` (or `fastdiff . --theme` with a manual archive) to enable Material icons in the directory tree and gold commit IDs (`#BE9A4A`) and green branch names (`#58EA51`) in the Reflog section. Icons apply to every tab in the current window and stay enabled until that window closes; subsequent launches into that window without `--theme` do not turn them off. No preference is saved, so a new window without `--theme` uses the default icons. The icons are bundled locally in the executable; enabling them requires no downloads. Unmatched files keep the generic document icon.

**All** includes unpushed commits, staged changes (HEAD → index), unstaged changes (index → working tree), and nonignored untracked additions. Each destination path appears once, with separate comparisons ordered Unpushed, Staged, then Unstaged/Untracked. **Unpushed** compares the merge base of the configured upstream and HEAD to HEAD, showing the cumulative net change across local commits. It uses locally stored refs and never fetches. Ahead, behind, and diverged branches therefore show only the local side's changes since their common ancestor. Missing upstream, detached/unborn HEAD, and unavailable common history receive explanatory states; All still shows working changes.

Use `dff <path>` to open a text file. Relative paths resolve from the terminal's current directory. **Full file / Git Diff** switches between current disk contents and Git review when a repository is available. Git filters and Ctrl+1–4 are disabled in Full file; switch to Git Diff to use them. Changing Git filters keeps Git Diff and the repository tree visible, retains the selected file when it matches, and otherwise selects the first changed file. An empty filter shows “No changes in this view.” Deleted files remain available as diffs. Documents have one pane with line numbers, selection/copy, horizontal scrolling, and **Search document** across the entire loaded text, including long lines. Documents do not fold context or permanently truncate line previews.

Repository-backed tabs show the current branch at the left of the bottom status bar; hover it for the full name. Detached checkouts show **Detached HEAD**. The collapsible **Reflog** panel below the directory tree (Ctrl+5) shows the local HEAD reflog, newest first, including commits, branch switches, resets, and branch renames. Each row has three aligned fields without column headers: the entry’s abbreviated commit SHA, current Git ref decorations in parentheses, and `HEAD@{n}: action`. Decorations are empty when no current ref points at the entry’s commit; they do not identify the branch at the time of the event. With `--theme`, SHAs are gold (`#BE9A4A`) and decorations are green (`#58EA51`). Detached HEAD still shows its reflog; an unborn HEAD or empty reflog shows **No reflog entries**. The panel reads local Git data and never fetches.

Expand or collapse Reflog by clicking its header or pressing Ctrl+5. The directory tree and selected diff stay visible, with separate scrolling for files and reflog entries. Rows are informational and do not open file changes. Search, diff filters, and file navigation remain available while the panel is open. Automatic refresh and F5 update both the current view and the expanded reflog.

Text loading supports UTF-8, UTF-8 BOM, and BOM-marked UTF-16. Binary content, unsupported encodings, read failures, and files exceeding 10 MiB or 100,000 lines produce an error. Git patches have an additional 2 MiB preview limit (see the automatic limits below). Atomic saves reload the requested path; disappearance displays an error in the same tab.

Repository and document tabs occupy a separate scrollable strip above the tools. An underline identifies the active tab; the close control stays inside its tab. Newly selected tabs scroll into view. Select a file from the expandable changed-file tree. Its path stays above the content; hover the path to read it in full when space is limited. Click a folder or its arrow to expand or collapse it. Filtering reveals matching files with their parent folders; clearing the filter restores your previous expansion. File navigation reveals the selected file even if its folder was collapsed. The toolbar groups search, Full file/Git Diff, Git views, Copy, and layout controls. Hover All to see which changes it includes. Empty views suggest switching to All, checking the repository, or refreshing. Copy offers the path and the selected comparison’s patch or source lines, with a comparison selector when several are available; shortcut notes below describe what source includes. Unified / Side by side switches the diff layout; the layout status reflects automatic narrow-window changes. UI − / UI + scales the whole interface, including navigation and status text; Ctrl+- / Ctrl++ independently adjusts source text. Help remains available in the toolbar. Help opens a scrollable shortcut reference and names the selected comparison used for whole-comparison copy commands. Escape closes Help.

Drag to select multiple source lines in either pane. Selection stays in the pane where the drag began, including when crossing the divider. Drag above or below the viewport to scroll, double-click to select the entire source line, or Shift-click to extend the selection. Right-click preserves the pane's existing selection and offers Copy selection and Select all. Ctrl+C copies source text with line breaks, excluding gutters, headers, diff markers, and the opposite pane. Ctrl+A selects all available source in the active comparison and pane, including folded context; unified mode follows its displayed source order. Selection survives scrolling and font changes and clears when the document, file, comparison, or layout changes.

Collapsed context can be expanded to inspect nearby unchanged lines. The diff panes fit the available window width and share one aligned vertical scroll position. Each pane scrolls long source lines horizontally with fixed line numbers. In Git Diff, a line longer than 4,096 bytes shows a preview ending in “… [long line; copy source for full text]”; Copy source returns the full line. Full file mode scrolls through complete lines. Below 640 logical pixels of diff width, side-by-side temporarily becomes unified; at 680 pixels or wider it returns to side-by-side. The explicit unified preference remains active until toggled off.

| Shortcut | Action |
|---|---|
| Ctrl+F | Focus diff search |
| Ctrl+Alt+F | Focus filename filter |
| F3 / Shift+F3 | Next / previous search match |
| Enter / Shift+Enter in diff search | Next / previous match |
| Ctrl+C / Ctrl+A | Copy selection / select all in active pane |
| F1 / Escape | Open / close Help |
| F5 | Refresh |
| Alt+Up / Alt+Down | Previous / next file |
| Ctrl+Up / Ctrl+Down | Previous / next hunk |
| Up / Down / Left / Right | Scroll active document / diff pane |
| Ctrl+- / Ctrl++ | Decrease / increase font size |
| Ctrl+1 / Ctrl+2 / Ctrl+3 / Ctrl+4 | All / Unstaged / Staged / Unpushed |
| Ctrl+5 | Expand / collapse Reflog panel |
| Ctrl+Shift+U | Toggle unified layout |
| Ctrl+Shift+K | Collapse context |
| Ctrl+Shift+C | Copy file path |
| Ctrl+Alt+C | Copy selected comparison's patch |
| Ctrl+Alt+Shift+C | Copy selected comparison's source |
| Ctrl+Alt+Left / Ctrl+Alt+Right | Previous / next copy comparison |

These are application defaults. Text fields retain normal editing shortcuts. Whole-comparison source copying uses the comparison's new source lines, or its old source lines when no new source is available. These are the lines in the selected comparison (changed lines plus its surrounding context), not necessarily the entire file; open Full file to copy a whole file. Long lines are copied in full. Choose the copy comparison with Ctrl+Alt+Left/Right; Help shows its label.

Click or hover over a document / diff pane to scroll it with the arrow keys. Drag the top edge of the expanded Reflog section to adjust its height; collapsing and reopening it preserves that height.

Changes refresh automatically. Native Linux and Windows tabs use filesystem notifications (250 ms debounce) plus two-second freshness checks; WSL tabs have no filesystem notifications and rely on the two-second checks and F5. Inactive tabs pause automatic refresh. Scroll is retained when the same file remains available; source selection clears when the document reloads. A binary, conflict, or loading-limit summary explains why text is unavailable. The automatic limits are 10 MiB per file, 2 MiB per Git patch preview, and 100,000 rows.

Opening another repository adds a tab. Closing the application ends owned helpers. The launcher does not wait for review feedback; returning to the agent conversation needs no approval or message exchange.

## Troubleshooting

- **Git unavailable:** standalone document viewing works without Git. For Git review, install Git in the environment that owns the repository and ensure it is on PATH.
- **No changes:** check the selected view and repository path. Unpushed shows net changes against the last fetched upstream ref; ignored files are excluded from Git review but can be opened as documents.
- **Not a Git repository:** pass a directory inside an initialized Git repository. For a repository target, the launcher validates Git before opening or forwarding the request and prints a discovery error in the terminal. When the launcher returns, the app has accepted the request; the requested diff may still be loading.
- **No Linux display:** launch from a graphical session or use the native Windows bridge under WSL.
- **Prefer the Linux window in WSL:** launch with `dff . --native` from a graphical Linux/WSLg session.
- **WSL launch failure:** follow the [installation troubleshooting](distribution.md#troubleshooting). Managed setup installs the matching helper and Windows app; manual archives use the helper steps in that guide.
- **Helper mismatch/disconnect:** install GUI and helper from the same build and launch again. An incompatible helper cannot safely serve the GUI.
