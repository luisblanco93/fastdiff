#!/usr/bin/env bash
# Opens the release app on the demo repository and saves one window screenshot per README image.
# usage: readme-screenshots.sh <demo-repository> <output-dir>
set -u
demo="$1"
out="$2"
mkdir -p "$out"
log="$out/session.log"
app="$GITHUB_WORKSPACE/target/release/fastdiff"
# Material file icons and the colored reflog in every shot. No --color-theme: a session theme
# would override the theme picked from the menu at the end.
theme=(--theme)

keys() {
    osascript -e "tell application \"System Events\" to $1" >>"$log" 2>&1 \
        || echo "osascript failed: $1" >>"$log"
}

# "id x y width height" of the app's main window in screen points; the id lets the capture
# hold the window alone.
window_info() {
    osascript -l JavaScript -e '
        ObjC.import("CoreGraphics");
        const windows = ObjC.deepUnwrap(ObjC.castRefToObject(
            $.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, $.kCGNullWindowID)));
        const main = windows.find(w => w.kCGWindowOwnerName === "fastdiff-gui" && w.kCGWindowLayer === 0);
        const b = main ? main.kCGWindowBounds : null;
        main ? [main.kCGWindowNumber, b.X, b.Y, b.Width, b.Height].map(Math.round).join(" ") : "";
    ' 2>>"$log"
}

window_id() { window_info | cut -d" " -f1; }

# Moves the real pointer to a screen point, and clicks there when the first argument is "click".
mouse() {
    echo "mouse: $*" >>"$log"
    osascript -l JavaScript - "$@" >>"$log" 2>&1 <<'JS' || echo "mouse failed: $*" >>"$log"
ObjC.import("CoreGraphics");
function run(argv) {
    const point = { x: Number(argv[1]), y: Number(argv[2]) };
    const post = type => $.CGEventPost($.kCGHIDEventTap,
        $.CGEventCreateMouseEvent(null, type, point, $.kCGMouseButtonLeft));
    post($.kCGEventMouseMoved);
    if (argv[0] === "click") {
        delay(0.2);
        post($.kCGEventLeftMouseDown);
        delay(0.1);
        post($.kCGEventLeftMouseUp);
    }
}
JS
}

# Center "x y" of the accessibility element with this name, from the app's AccessKit tree.
element_center() {
    osascript - "$1" 2>>"$log" <<'AS'
on run argv
    set wanted to item 1 of argv
    tell application "System Events" to tell process "fastdiff-gui"
        repeat with candidate in (entire contents of window 1)
            try
                if (name of candidate as text) is wanted or (description of candidate as text) is wanted then
                    set {x, y} to position of candidate
                    set {w, h} to size of candidate
                    return ((x + w / 2) as integer as text) & " " & ((y + h / 2) as integer as text)
                end if
            end try
        end repeat
    end tell
    return ""
end run
AS
}

# Clicks the named control, or the window-relative fallback point when it is not in the tree.
# usage: press <name> <move|click> <x from window right> <y from window top>
press() {
    local name="$1" action="$2" point
    point="$(element_center "$name")"
    if [ -z "$point" ]; then
        read -r _ wx wy ww _ <<<"$(window_info)"
        point="$((wx + ww - $3)) $((wy + $4))"
        echo "no element named $name; using $point" >>"$log"
    fi
    # shellcheck disable=SC2086
    mouse "$action" $point
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

# pick <theme name> <fallback y from window top>: opens the Theme dropdown and clicks the theme.
pick() {
    press "Theme ⏷" click 86 46
    sleep 2
    press "$1" click 150 "$2"
    sleep 1
    mouse move 5 5
    sleep 2
}

# Hero: the diff with the reflog panel open.
launch . --file src/api.rs
keys 'keystroke "5" using command down'
sleep 2
snap reflog

launch . --file src/store.rs
keys 'keystroke "f" using command down'
sleep 1
keys 'keystroke "due"'
sleep 2
snap search

launch README.md
sleep 2
snap markdown

# Last, because a theme picked from the menu is saved and later launches would use it.
launch . --file src/api.rs
keys 'keystroke "5" using command down'
sleep 2
pick "Catppuccin Mocha" 195
snap theme-catppuccin
pick "GitHub Light" 239
snap theme-github-light

pkill -f fastdiff-gui 2>/dev/null
ls -l "$out" >>"$log"
