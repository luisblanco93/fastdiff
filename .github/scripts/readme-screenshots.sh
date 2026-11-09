#!/usr/bin/env bash
# Opens the release app on the demo repository and saves one window screenshot per README image.
# usage: readme-screenshots.sh <demo-repository> <output-dir>
set -u
demo="$1"
out="$2"
mkdir -p "$out"
log="$out/session.log"
app="$GITHUB_WORKSPACE/target/release/fastdiff"
theme=(--color-theme fastdiff)

keys() {
    osascript -e "tell application \"System Events\" to $1" >>"$log" 2>&1 \
        || echo "osascript failed: $1" >>"$log"
}

# The CoreGraphics id of the app's main window, so the capture holds the window alone.
window_id() {
    osascript -l JavaScript -e '
        ObjC.import("CoreGraphics");
        const windows = ObjC.deepUnwrap(ObjC.castRefToObject(
            $.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, $.kCGNullWindowID)));
        const main = windows.find(w => w.kCGWindowOwnerName === "fastdiff-gui" && w.kCGWindowLayer === 0);
        main ? String(main.kCGWindowNumber) : "";
    ' 2>>"$log"
}

snap() {
    local name="$1" id
    id="$(window_id)"
    if [ -n "$id" ]; then
        screencapture -x -o -l "$id" "$out/$name.png" >>"$log" 2>&1
    else
        echo "no window id for $name; capturing the screen" >>"$log"
        screencapture -x "$out/$name-screen.png" >>"$log" 2>&1
    fi || echo "screencapture $name failed" >>"$log"
}

# Each shot starts from a fresh window so earlier key presses cannot leak into it.
launch() {
    pkill -f fastdiff-gui 2>/dev/null
    sleep 1
    echo "launch: $*" >>"$log"
    (cd "$demo" && "$app" "$@" "${theme[@]}" >>"$out/app.stdout" 2>>"$out/app.stderr") &
    sleep 8
    keys 'set frontmost of (first process whose name is "fastdiff-gui") to true'
    sleep 1
}

launch . --file src/api.rs
snap review

keys 'keystroke "5" using command down'
sleep 2
snap reflog

launch . --view unpushed --file src/store.rs
snap unpushed

launch . --file src/store.rs
keys 'keystroke "f" using command down'
sleep 1
keys 'keystroke "due"'
sleep 2
snap search

launch . --file src/api.rs
keys 'key code 122'
sleep 2
snap shortcuts

launch README.md
sleep 2
snap markdown

pkill -f fastdiff-gui 2>/dev/null
ls -l "$out" >>"$log"
