---
name: fastdiff
description: A flat, dark, compact desktop review bench where source code is the only loud thing.
colors:
  slate-ink: "#181B21"
  deep-moss: "#1B372A"
  dried-wine: "#422328"
  tide-blue: "#374E73"
  paper-mist: "#D5DCE6"
  gutter-fog: "#757F8F"
  periwinkle: "#93B4EF"
  orchid: "#C197E6"
  sage: "#A9CC94"
  lichen: "#91AA9B"
  amber: "#E5B171"
  coral: "#EB928A"
  signal-red: "#F57878"
  mint: "#6ECD91"
  steel: "#91B4D7"
  brass: "#BE9A4A"
  lime: "#58EA51"
  chrome-panel: "#1B1B1B"
  chrome-control: "#3C3C3C"
  chrome-hover: "#464646"
  chrome-text: "#8C8C8C"
  chrome-button-text: "#B4B4B4"
  chrome-strong: "#FFFFFF"
  chrome-selection: "#005C80"
  chrome-focus: "#C0DEFF"
typography:
  body:
    fontFamily: "Ubuntu-Light, sans-serif"
    fontSize: "12.5px"
    fontWeight: 300
  label:
    fontFamily: "Ubuntu-Light, sans-serif"
    fontSize: "9px"
    fontWeight: 300
  heading:
    fontFamily: "Ubuntu-Light, sans-serif"
    fontSize: "18px"
    fontWeight: 300
  source:
    fontFamily: "Hack, monospace"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: "18px"
rounded:
  control: "2px"
  focus: "2px"
  row: "3px"
  tab: "4px"
  window: "6px"
spacing:
  hairline: "1px"
  item-x: "8px"
  item-y: "3px"
  tab-x: "8px"
  tab-y: "4px"
  tree-indent: "16px"
  control-height: "24px"
  gutter: "64px"
components:
  tab:
    backgroundColor: "transparent"
    textColor: "{colors.chrome-text}"
    rounded: "{rounded.tab}"
    padding: "4px 8px"
    height: "24px"
  tab-active:
    backgroundColor: "{colors.chrome-control}"
    textColor: "{colors.chrome-strong}"
    rounded: "{rounded.tab}"
    padding: "4px 8px"
    height: "24px"
  toolbar-button:
    backgroundColor: "{colors.chrome-control}"
    textColor: "{colors.chrome-button-text}"
    rounded: "{rounded.control}"
    height: "18px"
  toolbar-button-selected:
    backgroundColor: "{colors.chrome-selection}"
    textColor: "{colors.chrome-focus}"
    rounded: "{rounded.control}"
    height: "18px"
  tree-row:
    backgroundColor: "transparent"
    textColor: "{colors.chrome-button-text}"
    rounded: "{rounded.row}"
    height: "24px"
  tree-row-selected:
    backgroundColor: "{colors.chrome-selection}"
    textColor: "{colors.chrome-focus}"
    rounded: "{rounded.row}"
    height: "24px"
  diff-row-context:
    backgroundColor: "{colors.slate-ink}"
    textColor: "{colors.paper-mist}"
    typography: "{typography.source}"
  diff-row-addition:
    backgroundColor: "{colors.deep-moss}"
    textColor: "{colors.paper-mist}"
    typography: "{typography.source}"
  diff-row-deletion:
    backgroundColor: "{colors.dried-wine}"
    textColor: "{colors.paper-mist}"
    typography: "{typography.source}"
  diff-row-match:
    backgroundColor: "{colors.tide-blue}"
    textColor: "{colors.paper-mist}"
    typography: "{typography.source}"
---

# Design System: fastdiff

## Overview

**Creative North Star: "The Reading Bench"**

fastdiff is a dark, flat workbench built for one job: reading source changes and returning to development. The chrome is neutral gray and recedes; the only saturated surfaces are the diff rows, a few status badges, and syntax tokens. Nothing decorates the frame, because the code is the subject.

The voice is sharp, neutral, and fast. Controls are small and exact (18px buttons, 24px rows), spacing is tight, and every state reads at a glance: selected, hovered, focused, matched. The interface answers immediately and then gets out of the way, matching the terminal-to-desktop workflow it serves.

It rejects IDE-style chrome: no activity bars, icon rails, or nested panel stacks. A review tool earns its density through alignment and restraint, not through feature-shaped furniture.

**Key Characteristics:**
- Flat panels separated by 1px hairlines; no shadows on resting surfaces.
- One dark theme; meaning carried by diff background tint and +/− markers, never by chrome color.
- Monospace is reserved for source, paths, and shortcut bindings.
- Compact 24px interactive rows with keyboard focus rings on every control.
- One blue fill for selection and search matches in content.

## Colors

A neutral gray chrome frame around a cool-slate code surface, with desaturated moss and wine tints for change and a small set of soft syntax hues. Chrome values are inherited from egui's dark preset (`Visuals::dark()`); the app overrides none of them.

### Primary
- **Tide Blue** (#374E73): The single content-level accent. Fills selected source ranges and search-match rows, in both panes and the unified view.

### Secondary
- **Periwinkle** (#93B4EF): Hunk and file header text. Marks structure inside the diff without a background.

### Tertiary
- **Deep Moss** (#1B372A): Addition rows. A dark green tint, readable under syntax color.
- **Dried Wine** (#422328): Deletion rows. A dark red tint, paired with the − marker in unified view.
- **Mint** (#6ECD91): Untracked (`+`) badge in the file tree.
- **Steel** (#91B4D7): Staged / unstaged (`S`, `U`, `S+U`) badges.
- **Signal Red** (#F57878): Conflict (`!`) badge.
- **Coral** (#EB928A): Error messages (load failures, reflog errors).
- **Amber** (#E5B171): Warnings such as the 100,000-row render limit; also the number token in source.

### Syntax
- **Orchid** (#C197E6): Keywords.
- **Sage** (#A9CC94): Strings.
- **Lichen** (#91AA9B): Comments, kept readable on every row background.
- **Amber** (#E5B171): Numbers.

### Neutral
- **Slate Ink** (#181B21): Context rows and the code surface.
- **Paper Mist** (#D5DCE6): Source text.
- **Gutter Fog** (#757F8F): Line numbers and diff markers.
- **Chrome Panel** (#1B1B1B): Panels, toolbar, status bar.
- **Chrome Control** (#3C3C3C) / **Chrome Hover** (#464646): Control fill at rest and on hover; also hairlines and the active-tab fill.
- **Chrome Text** (#8C8C8C) / **Chrome Button Text** (#B4B4B4) / **Chrome Strong** (#FFFFFF): Labels and status text, button and tree-row text, and emphasized text.
- **Chrome Selection** (#005C80) / **Chrome Focus** (#C0DEFF): Selected control or tree row fill, and the focus ring and active-tab underline.

### Optional theme palette
- **Brass** (#BE9A4A): Reflog commit IDs, only with `--theme`.
- **Lime** (#58EA51): Reflog branch names, only with `--theme`.

### Named Rules
**The Diff-Only Hue Rule.** Saturated hue belongs to diff rows, badges, and syntax. Chrome stays neutral gray so change is the loudest thing on screen.

**The One Selection Blue Rule.** Source selection and search matches share Tide Blue. A second highlight color never competes with it.

## Typography

**UI Font:** Ubuntu-Light (egui `default_fonts`, with system fallback)
**Source Font:** Hack (egui `default_fonts` monospace)

**Character:** A light, quiet sans for chrome paired with a sturdy monospace for code. The sans stays small so the source column carries the visual weight.

### Hierarchy
- **Source** (400, 13px default, 18px row): Diff and document text. User-adjustable with Ctrl+- / Ctrl++; row height is font size + 5.
- **Body** (300, 12.5px): Buttons, tabs, tree labels, status text, notices.
- **Heading** (300, 18px): Reserved by egui; not used in the main shell.
- **Label** (300, 9px): Small hints, match counts, duplicate-tab folder names, key hints.
- **Path / shortcut** (monospace, text-style size): The selected file path above the content and bindings in Help.

### Named Rules
**The Monospace Is Source Rule.** Monospace marks source, paths, and key bindings. Buttons, tabs, and status text stay proportional.

**The Two Scales Rule.** UI scale (0.75×–2.0×, UI − / UI +) and source font size are independent controls and stay independent.

## Layout

A single-window shell: tab strip (tabs left, window-wide UI scale and Help right) and toolbar on top, a resizable left sidebar, a central diff or document, and a status bar below. The window opens at 1100×720 and will not shrink below 640×400.

- **Sidebar:** 260px by default, 140px minimum, capped at 40% of window width. Holds the filename filter, the changed-file tree, and the collapsible Reflog (40% of sidebar height when open, drag-resizable).
- **Rhythm:** interactive rows are 24px (tabs, tree rows, tab close targets); Reflog rows are 22px; item spacing is egui's 8×3.
- **Diff:** both panes share one vertical scroll position; each pane scrolls long lines horizontally with a fixed 64px gutter. Below 640px of diff width side-by-side becomes unified; it returns at 680px.
- **Context folding:** unchanged runs longer than 8 lines collapse to 3 lines at each edge plus a fold row.
- **Toolbar rows:** one row from 620px of width; below that, search and Full file / Git Diff on the first row and the Git views on the second. egui cannot wrap widget groups, so the break is an explicit threshold.
- **Path row:** above the content, the selected path (truncated) at the left, Side by side | Unified and Copy at the right. It is always shown.
- **Density:** compact by default; controls are reflowed, never hidden.

## Elevation & Depth

Flat by default. Depth comes from 1px hairlines between regions and from tonal fills on state (hover, selection, match). Resting surfaces never cast shadows; the only shadows are egui's defaults on floating elements: the Help window, menus, and tooltips.

### Named Rules
**The Flat-By-Default Rule.** Surfaces are flat at rest. Hairlines separate regions; shadow appears only on floating elements.

## Shapes

Near-square. Controls and focus rings use a 2px radius, tree rows 3px, window and menus 6px. Tabs round only their top corners (4px) and sit on a 2px underline when active. Icons are drawn as simple outlines (folder, document, disclosure triangle, branch glyph in the status bar only); with `--theme` they become bundled Material icons.

## Components

Compact and utilitarian: small, unadorned, every state legible.

### Tabs
- **Shape:** top corners 4px, flat bottom, 24px label height with 8×4px padding.
- **Identity:** an 11px drawn icon before the name: folder for a repository, document for a file. Tabs never show the branch (names can be very long); the branch stays in the status bar.
- **Default:** transparent, Chrome Text label, truncated between 48px and 180px. Tabs with the same name add as many parent folders as it takes to tell them apart, in small weak text capped at 140px.
- **Active:** Chrome Control fill, Chrome Strong label, Chrome Focus icon, 2px Chrome Focus underline.
- **Close:** a drawn ✕ inside a 24px target, always present; weak on inactive tabs until hovered, then brightens. Focus draws a 2px-radius ring.
- **Window controls:** UI −, zoom percentage, UI +, and Help sit at the right end of the tab row.

### Toolbar buttons
- **Shape:** 2px radius. Plain buttons (Copy, search arrows) have a Chrome Control fill; the Full file / Git Diff, Git view and layout choices are frameless until selected.
- **Selected (Full file / Git Diff / All / Unstaged / Staged / Unpushed / Side by side / Unified):** Chrome Selection fill with Chrome Focus label.
- **Disabled:** dimmed; Git filters are disabled in Full file mode.
- **Search arrows:** Previous / Next match, 20px, drawn as up/down chevrons (egui's default fonts have no arrow glyphs) and flush against the search field with a 1px gap.
- **Small buttons:** UI − / UI +, Help use egui's small style (about 15px high).

### File tree rows
- **Style:** 24px rows, 16px indent per level, disclosure triangle for folders, icon, label truncated with an ellipsis, right-aligned status badge.
- **Hover / selected:** 3px-radius fill (Chrome Hover, then Chrome Selection with Chrome Focus text).
- **Badges:** `!` Signal Red, `+` Mint, `S` / `U` / `S+U` Steel.

### Diff rows
- **Gutter:** 64px, Gutter Fog, line number plus `+` / `−` marker in unified view.
- **Fills:** Deep Moss for additions, Dried Wine for deletions, Slate Ink for context, Tide Blue for search matches.
- **Headers:** Periwinkle monospace text, no background.
- **Fold row:** an expandable stand-in for hidden context.

### Inputs
- **Style:** egui text edit; the toolbar search is 150px wide, the filename filter fills the sidebar width with a drawn magnifier.
- **Focus:** Chrome Focus ring.

### Status bar
- **Style:** small text; branch glyph and name (max 360px or half the window), spinner while loading, changed-file count and root, key hints only at 900px or wider.

## Do's and Don'ts

### Do:
- **Do** keep chrome neutral and let diff tints and syntax carry all color.
- **Do** pair addition and deletion tints with the `+` / `−` marker in the unified view.
- **Do** keep interactive rows at 24px and give every control a visible keyboard focus ring.
- **Do** keep selection and search matches on Tide Blue (#374E73).
- **Do** keep UI scale and source font size as separate controls.

### Don't:
- **Don't** import IDE-style chrome: activity bars, icon rails, or nested panel stacks.
- **Don't** add shadows to resting surfaces; reserve them for Help, menus, and tooltips.
- **Don't** use saturated color in chrome that could be mistaken for diff meaning.
- **Don't** set proportional text in monospace or source text in the UI font.
