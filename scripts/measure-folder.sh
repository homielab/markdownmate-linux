#!/usr/bin/env bash
# Launch time and memory with a large folder, run inside the container:
#   ./dev.sh scripts/measure-folder.sh [notes, default 10000] [runs, default 3]
# Generates the folder in a temp dir (examples/generate_notes.rs), then starts the release
# build on Xvfb `runs` times with the folder open, every subfolder expanded and
# `Area 0/Topic 0/Note 0.md` reopened, and prints the app's timing log (MARKDOWNMATE_TIMING,
# ms after main started), its memory after it settles, after the Outline/Backlinks panel is
# shown and after a switch to another note, and the tag re-index and Backlinks after a note
# changes on disk. SORT=modified (or created) sorts the sidebar by that date instead of by
# name, and also times moving the changed note to the top. MM_BIN=<path> launches that build
# instead of building this checkout (an A/B against another version; see docs/PERFORMANCE.md).
# Everything it starts and writes is removed on exit. See docs/PERFORMANCE.md.
set -euo pipefail
cd "$(dirname "$0")/.."

COUNT=${1:-10000}
RUNS=${2:-3}
[ -n "${MM_BIN:-}" ] || cargo build -q --release
cargo build -q --release --example generate_notes
TARGET="${CARGO_TARGET_DIR:-target}/release"
BIN="${MM_BIN:-$TARGET/markdownmate}"
TMP=$(mktemp -d)
trap 'pkill -f -- "$BIN" 2>/dev/null || true; kill $(jobs -p) 2>/dev/null || true;
      [ -n "${DBUS_PID:-}" ] && kill "$DBUS_PID" 2>/dev/null; rm -rf "$TMP"' EXIT

NOTES="$TMP/notes"
"$TARGET/examples/generate_notes" "$NOTES" "$COUNT"
echo "on disk: $(du -sh "$NOTES" | cut -f1), $(find "$NOTES" -type d | wc -l) folders"

export DISPLAY=:99
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
sleep 1
export XDG_CONFIG_HOME="$TMP/config" XDG_DATA_HOME="$TMP/data" XDG_CACHE_HOME="$TMP/cache"
export XDG_RUNTIME_DIR="$TMP/run"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME/markdownmate" && chmod 700 "$XDG_RUNTIME_DIR"
# The container (root, no user namespaces) cannot run WebKit's bubblewrap sandbox.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export NO_AT_BRIDGE=1 GTK_A11Y=none MARKDOWNMATE_TIMING=1
# One session bus for the app and the second instance that forwards a markdownmate:// link.
read -r DBUS_SESSION_BUS_ADDRESS DBUS_PID < <(dbus-daemon --session --fork --print-address=1 --print-pid=1 | paste -sd' ')
export DBUS_SESSION_BUS_ADDRESS

wait_for() {
    local end=$((SECONDS + $1)); shift
    until "$@"; do [ "$SECONDS" -lt "$end" ] || return 1; sleep 0.2; done
}
# Resident memory (MB) of the app and of its WebKit processes.
rss() {
    local app web=0 p
    app=$(awk '/^VmRSS/ {print $2}' "/proc/$1/status")
    for p in $(pgrep -f 'WebKit(Web|Network)Process' || true); do
        web=$((web + $(awk '/^VmRSS/ {print $2}' "/proc/$p/status" 2>/dev/null || echo 0)))
    done
    echo "app $((app / 1024)) MB, WebKit $((web / 1024)) MB"
}
logged() { grep -q -- "$2" "$1"; }
# The newest log line matching $2, without the prefix.
last() { grep -- "$2" "$1" | tail -1 | sed 's/^markdownmate-timing: //'; }

# Path of note $1 as examples/generate_notes.rs places it.
note_path() { echo "Area $(($1 % 20))/Topic $(($1 / 20 % 10))/Note $1.md"; }
first=$(note_path 0)
other=$(note_path $((COUNT * 1065 / 10000)))
changed=$(note_path $((COUNT * 1989 / 10000)))
for run in $(seq 1 "$RUNS"); do
    {
        printf 'folder=%s\nnote=%s\nmode=edit\nsidebar_sort=%s\n' "$NOTES" "$NOTES/$first" "${SORT:-name}"
        find "$NOTES" -mindepth 1 -type d | sed 's/^/expanded=/'
    } > "$XDG_CONFIG_HOME/markdownmate/state"
    log="$TMP/run$run.log"
    "$BIN" > "$log" 2>&1 &
    pid=$!
    echo "--- run $run"
    wait_for 120 logged "$log" "tags indexed" || echo "no tags index after 120 s"
    sleep 3
    grep '^markdownmate-timing' "$log" | sed 's/^markdownmate-timing: /  /'
    echo "  memory after launch: $(rss $pid)"

    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync >/dev/null 2>&1 || true
    xdotool key alt+ctrl+o
    wait_for 120 logged "$log" "backlinks listed" || echo "no backlinks after 120 s"
    echo "  outline shown: $(last "$log" "backlinks listed")"
    sleep 2
    echo "  memory with the outline: $(rss $pid)"

    n=$(grep -c "backlinks listed" "$log")
    "$BIN" "markdownmate://open?path=${other// /%20}" >/dev/null 2>&1
    wait_for 120 sh -c "[ \$(grep -c 'backlinks listed' '$log') -gt $n ]" || echo "no backlinks after the switch"
    echo "  note switch: $(last "$log" "note opened"), $(last "$log" "backlinks listed")"
    sleep 3
    echo "  memory after the switch: $(rss $pid)"

    n=$(grep -c "tags indexed" "$log")
    echo "Changed on disk #tagnew" >> "$NOTES/$changed"
    wait_for 60 sh -c "[ \$(grep -c 'tags indexed' '$log') -gt $n ]" || echo "no tag re-index"
    echo "  note changed on disk: $(last "$log" "tags indexed"), $(last "$log" "backlinks listed")${SORT:+, $(last "$log" "note re-sorted")}"
    sleep 1
    echo "  memory at the end: $(rss $pid)"

    if grep -E 'panicked|CRITICAL' "$log"; then echo "  errors in the log above"; fi
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    pkill -f -- "$BIN" 2>/dev/null || true
    sleep 1
done
