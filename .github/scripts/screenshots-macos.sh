#!/usr/bin/env bash
# usage: screenshots-macos.sh <repo-dir> <shots-dir>
# Drives the release build on the demo repository and saves one window crop per README screenshot.
repo="$1"; shots="$2"
log="$shots/session.log"
gui="$GITHUB_WORKSPACE/target/release/fastdiff"
mkdir -p "$shots"

keys() { osascript -e "tell application \"System Events\" to $1" >>"$log" 2>&1 || echo "osascript failed: $1" >>"$log"; }

launch() {
  pkill -f fastdiff-gui 2>/dev/null; sleep 2
  (cd "$repo" && "$gui" . --theme "$@" >>"$shots/app.stdout" 2>>"$shots/app.stderr" &)
  sleep 9
  keys 'set frontmost of (first process whose name is "fastdiff-gui") to true'; sleep 1
}

snap() {
  local bounds
  bounds=$(osascript -e 'tell application "System Events" to tell process "fastdiff-gui" to get {position, size} of window 1' 2>>"$log" | tr -d ' ')
  echo "$1 window bounds: $bounds" >>"$log"
  screencapture -x "$shots/full-$1.png" >>"$log" 2>&1
  screencapture -x -R"$bounds" "$shots/$1.png" >>"$log" 2>&1 || echo "screencapture $1 failed" >>"$log"
}

# All view, main.rs selected: Staged and Unstaged sections with the changed-file tree.
launch --file src/main.rs
snap review

# Reflog panel under the file tree (Cmd+5).
keys 'keystroke "5" using command down'; sleep 2
snap reflog
keys 'keystroke "5" using command down'; sleep 1

# Search across the diff (Cmd+F).
keys 'keystroke "f" using command down'; sleep 1
keys 'keystroke "route"'; sleep 2
snap search
keys 'key code 53'; sleep 1

# Keyboard shortcut reference (F1).
keys 'key code 53'; sleep 1
keys 'key code 122'; sleep 2
snap shortcuts

# Unpushed view on orders.rs needs a fresh window.
launch --view unpushed --file src/routes/orders.rs
snap unpushed

pgrep -fl fastdiff-gui >>"$log" 2>&1 || echo "no fastdiff-gui process at end" >>"$log"
