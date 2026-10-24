#!/usr/bin/env bash
# Headless GUI smoke test, run inside the container: ./dev.sh scripts/smoke.sh
# Starts the app on Xvfb with a sample note folder, takes screenshots into ./screenshots/,
# and fails if the app dies or prints a panic / GTK CRITICAL / size_allocate warning (layout bug).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -q
cargo build -q --example drag_files
BIN="${CARGO_TARGET_DIR:-target}/debug/markdownmate"
OUT="$PWD/screenshots"
mkdir -p "$OUT"
TMP=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$TMP"' EXIT

# Sample notes: a few .md files, a subfolder, an image the note embeds.
NOTES="$TMP/notes"
mkdir -p "$NOTES/Projects/Ideas"
cat > "$NOTES/Welcome.md" <<'EOF'
# Welcome to MarkdownMate

Some **bold**, some *italic* and `code`.

- [ ] a to-do
- [x] done item
1. first
2. second

> A quote

| Col A | Col B |
|-------|-------|
| 1     | 2     |

```rust
// coloured by syntect
fn main() { let n = 42; println!("<b>{n}</b>"); }
```

![dot](dot.png)
EOF
# Long enough to scroll the preview (side-by-side run).
for i in $(seq 1 60); do echo "Paragraph $i"; echo; done >> "$NOTES/Welcome.md"
echo "# Plan" > "$NOTES/Projects/Plan.md"
echo "# Idea" > "$NOTES/Projects/Ideas/Idea.md"
printf '# Top\n\ntext\n\n```\n# not a heading\n```\n\n## Second\n\nbody\n' > "$NOTES/Outline.md"
printf '# [Go to Plan](Projects/../Projects/Plan.md#x)\n\n[Outside](../x.md)\n' > "$NOTES/Links.md"
printf '# Target\n\nbody\n' > "$NOTES/Target.md"
printf 'intro\n\nSee [[Target|the target]] here\n\n`[[Target]]` in code\n' > "$NOTES/Ref.md"
printf '# Ref2\n[t](../Target.md)\n' > "$NOTES/Projects/Ref2.md"
printf '# [[idea|Open the idea note]]\n\n[[Nowhere|missing]] and `[[code]]`\n' > "$NOTES/Wiki.md"
# Links to headings (#116): within the note and to a heading of another note.
{ echo '# [Tới phần cuối](#phần-cuối)'; echo; for i in $(seq 1 40); do echo "Đoạn $i"; echo; done
  echo '## Phần cuối'; echo; for i in $(seq 1 20); do echo "Sau $i"; echo; done; } > "$NOTES/Zanchor.md"
echo '# [[ZanchorB#Mục hai]]' > "$NOTES/ZanchorLink.md"
{ echo '# B'; echo; for i in $(seq 1 40); do echo "Dòng $i"; echo; done; echo '## Mục hai'; echo; for i in $(seq 1 20); do echo "Sau $i"; echo; done; } > "$NOTES/ZanchorB.md"
printf '# Todo\n\nParagraph <b>not bold</b> & escaped\n' > "$NOTES/Todo.md"
printf -- '- [ ] first\n- [ ] second\n' > "$NOTES/Tasks.md"
printf '```sh\necho "copied code"\nls -la\n```\n\ntext\n' > "$NOTES/Code.md"
printf '# Themes\n\nText with a [link](https://x) and #tag.\n\n- [ ] open\n- [x] done\n\n```rust\n// comment\nfn main() { let s = "str"; }\n```\n' > "$NOTES/Themes.md"
printf '# Tags\n\nPlan #work/project and #việc-làm, not `#code` or #1.\n\nAlso #work, colour #fff.\n' > "$NOTES/Tags.md"
printf 'Note for #work\n' > "$NOTES/Projects/Tagged.md"
echo '# Term' > "$NOTES/Term.md"
printf 'Việt\nviệt nam\nother VIỆT\n' > "$NOTES/Search.md"
printf 'one\ntwo\n' > "$NOTES/Reload.md"
printf '| Tên | b |\n|:-:|--|\n| Nguyễn Văn | 1 |' > "$NOTES/Table.md"
printf '# Math\n\nInline $x^2 + \\sqrt{y}$ and a price: $5 and $10.\n\n$$\\sum_{i=1}^{n} \\frac{a_i}{b}$$\n\nBroken: $\\frac{a$\n\n```math\n\\int_0^1 x\\,dx = \\frac{1}{2}\n```\n' > "$NOTES/Math.md"
printf '![a big red picture for the hover tooltip](big.svg)\n![a link to an image that does not exist](nope.png)\n![a web image that is never downloaded](https://example.com/a.png)\n' > "$NOTES/Image.md"
printf '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="480"><rect width="640" height="480" fill="red"/></svg>' > "$NOTES/big.svg"
# 1x1 red PNG.
base64 -d > "$NOTES/dot.png" <<<'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg=='

export DISPLAY=:99
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
sleep 1
export XDG_CONFIG_HOME="$TMP/config" XDG_DATA_HOME="$TMP/data" XDG_CACHE_HOME="$TMP/cache"
export XDG_RUNTIME_DIR="$TMP/run"
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
# Smoke test only: the container (root, no user namespaces) cannot run WebKit's bubblewrap sandbox.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export NO_AT_BRIDGE=1 GTK_A11Y=none

fail=0
# wait_for <seconds> <command...>: retries the command until it succeeds or time runs out
# (a fixed sleep flakes when the machine is busy).
wait_for() {
    local end=$((SECONDS + $1)); shift
    until "$@"; do [ "$SECONDS" -lt "$end" ] || return 1; sleep 0.2; done
}
# run <name> <state mode line> [xdotool keys...]: start the app, screenshot, check it.
# A function act_<name>, if defined, runs after the first screenshot.
run() {
    local name=$1 mode=$2
    shift 2
    mkdir -p "$XDG_CONFIG_HOME/markdownmate"
    # KEEP_STATE=1: khởi động lại với state lần chạy trước để lại.
    [ -n "${KEEP_STATE:-}" ] || printf 'folder=%s\nnote=%s\n%s\n' "$NOTES" "${NOTE:-$NOTES/Welcome.md}" "$mode" \
        > "$XDG_CONFIG_HOME/markdownmate/state"
    local log="$OUT/$name.log"
    dbus-run-session -- "$BIN" > "$log" 2>&1 &
    local pid=$! bad=0
    sleep 5
    scrot -o "$OUT/$name.png"
    if [ $# -gt 0 ]; then
        timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
        xdotool key "$@"
        sleep 2
        scrot -o "$OUT/$name-keys.png"
    fi
    if declare -F "act_$name" >/dev/null; then
        "act_$name"
    fi
    if ! kill -0 "$pid" 2>/dev/null; then
        echo "FAIL $name: app exited early"; bad=1
    fi
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    # Killing dbus-run-session may leave the app's window up; the next test must not see it.
    pkill -f -- "$BIN" 2>/dev/null || true
    # The app saves on SIGTERM (note + state): let it finish before the next test writes state.
    for _ in $(seq 50); do pgrep -f -- "$BIN" >/dev/null || break; sleep 0.1; done
    sleep 0.5
    if grep -E 'CRITICAL|panicked|size_allocate' "$log"; then
        echo "FAIL $name: CRITICAL/panic/size_allocate warning in $log"; bad=1
    fi
    [ $bad = 0 ] && echo "ok   $name -> screenshots/$name.png" || fail=1
}

# Side by side, Sync Scrolling off: scroll the preview, type in the editor; the re-rendered
# preview should keep its scroll position and show the typed text. The re-render swaps the page
# in place: split-rerender.png (just after it) must not show the preview's top either.
# The window opens at 900x600 in the top-left corner: preview right, editor middle.
act_split() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 750 400 click --repeat 15 --delay 50 5
    sleep 1
    scrot -o "$OUT/split-scrolled.png"
    xdotool mousemove 400 150 click 1 key ctrl+Home type 'TYPED '
    sleep 0.4
    scrot -o "$OUT/split-rerender.png"
    sleep 1.6
    scrot -o "$OUT/split-typed.png"
}

# Scroll sync (on by default): Ctrl+End in the editor -> the preview shows its end too
# ("Paragraph 60" at the bottom of both panes in split-sync.png).
act_sync() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+End
    sleep 1
    scrot -o "$OUT/split-sync.png"
    # Preview -> editor (#96): scrolling the preview up scrolls the editor to the same place
    # (split-sync-back.png: both panes show the same paragraphs, not the end); the cursor
    # stays at the end, so typing scrolls the editor back down there (split-sync-cursor.png).
    xdotool mousemove 750 400 click --repeat 10 --delay 50 4
    sleep 1
    scrot -o "$OUT/split-sync-back.png"
    # Keys in the preview (#110): click an empty spot right of the text (no link/checkbox),
    # Home then Page Down twice -> the editor shows the same paragraphs (split-sync-keys.png).
    xdotool mousemove 820 300 click 1 key Home
    sleep 1
    xdotool key Next Next
    sleep 1
    scrot -o "$OUT/split-sync-keys.png"
    # Dragging the preview's scrollbar: End puts its thumb at the bottom, drag it up to the
    # middle -> the editor follows (split-sync-drag.png: middle paragraphs, not the end).
    xdotool key End
    sleep 1
    xdotool mousemove 889 560 sleep 0.5 mousedown 1 mousemove 889 450 sleep 0.2 \
        mousemove 889 330 sleep 0.2 mouseup 1
    sleep 1
    scrot -o "$OUT/split-sync-drag.png"
    # Back to the editor without moving its cursor (Ctrl+3: Side by side again focuses it).
    xdotool key ctrl+3
    sleep 0.5
    xdotool type 'Z'
    sleep 1
    scrot -o "$OUT/split-sync-cursor.png"
}

# Find in Folder: search, then open a match (starting in Preview: it must switch to the editor).
act_find() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key shift+ctrl+f
    sleep 1
    xdotool type 'paragraph'
    sleep 2
    scrot -o "$OUT/find.png"
    xdotool key Down Down Return
    sleep 2
    scrot -o "$OUT/find-opened.png"
}

# Ctrl+B / Ctrl+I / Ctrl+K on the word at the cursor; the auto-saved note must show the result.
act_format() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+End Return type 'word'
    xdotool key ctrl+b ctrl+i ctrl+k
    xdotool type 'https://x'
    sleep 2
    scrot -o "$OUT/format.png"
    grep -qF '***[word](https://x)***' "$NOTES/Welcome.md" || { echo "FAIL format: note not formatted"; bad=1; }
}

# Table: Tab past the last cell adds a row, Enter adds one, Enter on the empty last row leaves
# the table (one undo step); every step realigns the columns (Vietnamese, `:-:` centred).
act_table() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+End Tab type 'x'
    xdotool key Tab; xdotool type 'y'
    xdotool key Return Return
    sleep 2
    grep -qxF '| Nguyễn Văn | 1   |' "$NOTES/Table.md" || { echo "FAIL table: columns not aligned"; bad=1; }
    grep -qF '|     |' "$NOTES/Table.md" && { echo "FAIL table: empty last row not removed"; bad=1; }
    xdotool key ctrl+z
    sleep 2
    grep -qxF '|            |     |' "$NOTES/Table.md" || { echo "FAIL table: leaving the table is not one undo step"; bad=1; }
    xdotool key Return; xdotool type 'after'
    sleep 2
    scrot -o "$OUT/table.png"
    local want
    want=$(printf '|    Tên     | b   |\n| :--------: | --- |\n| Nguyễn Văn | 1   |\n|     x      | y   |\nafter')
    [ "$(cat "$NOTES/Table.md")" = "$want" ] || { echo "FAIL table: got"; cat "$NOTES/Table.md"; bad=1; }
    # Insert Table (Shift+Ctrl+T) under "after": blank line, empty 3-column table, the first
    # header cell selected; Tab moves to the second one (realigned on the next Tab, not on typing).
    xdotool key shift+ctrl+t; xdotool type 'k'
    xdotool key Tab; xdotool type 'v'
    sleep 2
    scrot -o "$OUT/table-insert.png"
    local base=$want
    want=$(printf '%s\n\n| k   | v    |     |\n| --- | --- | --- |\n|     |     |     |\n|     |     |     |' "$want")
    [ "$(cat "$NOTES/Table.md")" = "$want" ] || { echo "FAIL table insert: got"; cat "$NOTES/Table.md"; bad=1; }
    # In a `> ` quote (#126): Enter leaves the table above, every inserted line keeps `> `,
    # Tab moves to the next cell.
    xdotool key ctrl+End Return; xdotool type '> q'
    xdotool key shift+ctrl+t; xdotool type 'k'
    xdotool key Tab; xdotool type 'v'
    sleep 2
    want=$(printf '%s\n\n| k   | v   |     |\n| --- | --- | --- |\n|     |     |     |\n> q\n>\n> | k   | v    |     |\n> | --- | --- | --- |\n> |     |     |     |\n> |     |     |     |' "$base")
    [ "$(cat "$NOTES/Table.md")" = "$want" ] || { echo "FAIL table insert in quote: got"; cat "$NOTES/Table.md"; bad=1; }
}

# Preview link to another note: clicking it opens that note (window title becomes Plan).
act_link() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 340 122 click 1
    sleep 2
    scrot -o "$OUT/link.png"
    xdotool search --onlyvisible --class markdownmate getwindowname %@ | grep -q '^Plan.md' || { echo "FAIL link: Plan not opened"; bad=1; }
}

# Wiki link [[idea]] (case-insensitive name) opens Projects/Ideas/Idea.md.
act_wiki() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 340 122 click 1
    sleep 2
    scrot -o "$OUT/wiki.png"
    xdotool search --onlyvisible --class markdownmate getwindowname %@ | grep -q '^Idea.md' || { echo "FAIL wiki: Idea not opened"; bad=1; }
}

# Ctrl+click on the wiki link [[idea|…]] in the editor (first line) opens Projects/Ideas/Idea.md.
act_editor_wiki() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 340 70 keydown ctrl click 1 keyup ctrl
    sleep 2
    scrot -o "$OUT/editor_wiki.png"
    # Not `| grep -q` (pipefail: xdotool may get SIGPIPE when it lists several windows).
    grep -q '^Idea.md' <<<"$(xdotool search --onlyvisible --class markdownmate getwindowname %@)" || { echo "FAIL editor_wiki: Idea not opened"; bad=1; }
}

# Preview link to a heading of the note (#116): clicking `[…](#phần-cuối)` scrolls the preview
# so "Phần cuối" is at the top (anchor.png).
act_anchor() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 340 122 click 1
    sleep 1
    scrot -o "$OUT/anchor.png"
    grep -q '^Zanchor.md' <<<"$(xdotool search --onlyvisible --class markdownmate getwindowname %@)" || { echo "FAIL anchor: left the note"; bad=1; }
}
# `[[ZanchorB#Mục hai]]` opens ZanchorB with "Mục hai" at the top of the preview (anchor_note.png)
# and the cursor on that heading (saved in state on quit).
act_anchor_note() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 340 122 click 1
    sleep 2
    scrot -o "$OUT/anchor_note.png"
    grep -q '^ZanchorB.md' <<<"$(xdotool search --onlyvisible --class markdownmate getwindowname %@)" || { echo "FAIL anchor_note: ZanchorB not opened"; bad=1; }
}
# anchor_cursor <step> <note> <heading line>: after the run, the state saved on quit has the
# cursor of <note> on that heading.
anchor_cursor() {
    local line
    line=$(grep -nx -- "$3" "$NOTES/$2" | cut -d: -f1)
    grep -q "^pos=$line:1:$NOTES/$2\$" "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL $1: cursor not on the heading"; grep '^pos=' "$XDG_CONFIG_HOME/markdownmate/state"; fail=1; }
}
# Ctrl+click in the editor (#131): `[…](#phần-cuối)` puts the cursor on "## Phần cuối" in the
# same note, `[[ZanchorB#Mục hai]]` opens ZanchorB with the cursor on "## Mục hai".
act_editor_anchor() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 340 70 keydown ctrl click 1 keyup ctrl
    sleep 1
    scrot -o "$OUT/editor_anchor.png"
    grep -q '^Zanchor.md' <<<"$(xdotool search --onlyvisible --class markdownmate getwindowname %@)" || { echo "FAIL editor_anchor: left the note"; bad=1; }
}
act_editor_anchor_note() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 340 70 keydown ctrl click 1 keyup ctrl
    title_is() { grep -q '^ZanchorB.md' <<<"$(xdotool search --onlyvisible --class markdownmate getwindowname %@)"; }
    wait_for 5 title_is || { echo "FAIL editor_anchor_note: ZanchorB not opened"; bad=1; }
    sleep 0.5
    scrot -o "$OUT/editor_anchor_note.png"
}
# Side by side (scroll sync on, #131): clicking `[…](#phần-cuối)` in the preview scrolls it so
# "Phần cuối" is at the top, and the editor follows (split_anchor.png: both panes there).
act_split_anchor() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 620 122 click 1
    sleep 1.5
    scrot -o "$OUT/split_anchor.png"
    grep -q '^Zanchor.md' <<<"$(xdotool search --onlyvisible --class markdownmate getwindowname %@)" || { echo "FAIL split_anchor: left the note"; bad=1; }
}

# Hovering an image link in the editor shows a thumbnail tooltip (its own X window); a missing
# image or a web URL shows none.
act_hover() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    local n
    for y in 88 106; do
        xdotool mousemove 330 $y mousemove 340 $y
        sleep 2
        n=$(xdotool search --onlyvisible --class markdownmate | wc -l)
        [ "$n" = 1 ] || { echo "FAIL hover: tooltip on a missing/web image (y=$y)"; bad=1; }
    done
    xdotool mousemove 330 70 mousemove 340 70
    sleep 2
    scrot -o "$OUT/hover.png"
    n=$(xdotool search --onlyvisible --class markdownmate | wc -l)
    [ "$n" = 2 ] || { echo "FAIL hover: no image tooltip"; bad=1; }
}

# Clicking a task checkbox in the preview ticks it in the note (auto-saved).
act_task() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 278 104 click 1
    sleep 2
    scrot -o "$OUT/task.png"
    [ "$(head -1 "$NOTES/Tasks.md")" = '- [x] first' ] || { echo "FAIL task: checkbox not ticked"; bad=1; }
}

# Copy button on a code block in the preview (shown on hover) puts the block's code on the clipboard.
act_codecopy() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 500 90
    sleep 1
    scrot -o "$OUT/codecopy-hover.png"
    xdotool mousemove 840 105 click 1
    sleep 1
    scrot -o "$OUT/codecopy.png"
    [ "$(timeout 5 xclip -selection clipboard -o 2>/dev/null)" = "$(printf 'echo "copied code"\nls -la')" ] \
        || { echo "FAIL codecopy: code not on clipboard"; bad=1; }
}

# Go to Line from Preview: Ctrl+L 3:6 switches to the editor, cursor before `**bold**`.
act_goto() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key ctrl+l
    sleep 1
    xdotool type '3:6'
    xdotool key Return
    sleep 1
    xdotool type 'X'
    sleep 2
    scrot -o "$OUT/goto.png"
    grep -qF 'Some X**bold**' "$NOTES/Welcome.md" || { echo "FAIL goto: cursor not at 3:6"; bad=1; }
}

# Find/replace in the note, starting in Preview: Ctrl+F switches to the editor; Esc back to it.
# The selected `Việt` becomes the search text (case-insensitive: 3 results); Enter selects the
# 2nd (`việt`), Esc returns to the editor so `X` replaces it. Ctrl+H, `Nam`, Ctrl+Enter =
# Replace All; one Ctrl+Z undoes all of it.
act_search() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key ctrl+f
    sleep 1
    xdotool key Escape ctrl+Home shift+End ctrl+f
    sleep 1
    xdotool key Return
    sleep 1
    scrot -o "$OUT/search.png"
    xdotool key Escape type 'X'
    sleep 2
    [ "$(sed -n 2p "$NOTES/Search.md")" = 'X nam' ] || { echo "FAIL search: Enter/Esc did not select the 2nd match"; bad=1; }
    xdotool key ctrl+h
    sleep 1
    xdotool type 'Nam'
    xdotool key ctrl+Return
    sleep 1
    scrot -o "$OUT/search-replace.png"
    sleep 1
    [ "$(cat "$NOTES/Search.md")" = "$(printf 'Nam\nX nam\nother Nam')" ] || { echo "FAIL search: Replace All"; bad=1; }
    xdotool key Escape ctrl+z
    sleep 2
    [ "$(cat "$NOTES/Search.md")" = "$(printf 'Việt\nX nam\nother VIỆT')" ] || { echo "FAIL search: Replace All is not one undo step"; bad=1; }
}

# Paste an image (clipboard holds only image/png): saved as assets/image.png, linked in the note.
act_paste() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xclip -quiet -selection clipboard -t image/png -i "$NOTES/dot.png" >/dev/null 2>&1 &
    sleep 1
    xdotool mousemove 400 150 click 1 key ctrl+End Return ctrl+v
    sleep 2
    scrot -o "$OUT/paste.png"
    head -c 4 "$NOTES/assets/image.png" 2>/dev/null | grep -q PNG || { echo "FAIL paste: assets/image.png missing"; bad=1; }
    grep -qF '![](assets/image.png)' "$NOTES/Welcome.md" || { echo "FAIL paste: link not in note"; bad=1; }
}

# Paste a URL over a selection -> [text](url); `*` twice over a selection -> bold, Ctrl+Z undoes
# one wrap.
act_wrap() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    printf 'https://x.y/việt' | xclip -quiet -selection clipboard -i >/dev/null 2>&1 &
    sleep 1
    xdotool mousemove 400 150 click 1 key ctrl+End Return type 'site'
    xdotool key shift+Home ctrl+v
    sleep 1
    xdotool key End Return type 'bold'
    xdotool key shift+Home asterisk asterisk
    sleep 2
    scrot -o "$OUT/wrap.png"
    grep -qF '[site](https://x.y/việt)' "$NOTES/Welcome.md" || { echo "FAIL wrap: URL not pasted as link"; bad=1; }
    grep -qxF '**bold**' "$NOTES/Welcome.md" || { echo "FAIL wrap: ** not wrapped"; bad=1; }
    xdotool key ctrl+z
    sleep 2
    grep -qxF '*bold*' "$NOTES/Welcome.md" || { echo "FAIL wrap: wrap is not one undo step"; bad=1; }
    # Not a URL: pasted as usual over the selection (replaced, one undo step), multi-line too.
    printf 'chữ mới' | xclip -quiet -selection clipboard -i >/dev/null 2>&1 &
    sleep 1
    xdotool key End Return type 'old'
    xdotool key shift+Home ctrl+v
    sleep 2
    grep -qxF 'chữ mới' "$NOTES/Welcome.md" || { echo "FAIL wrap: text not pasted over the selection"; bad=1; }
    xdotool key ctrl+z
    sleep 2
    grep -qxF 'old' "$NOTES/Welcome.md" || { echo "FAIL wrap: paste is not one undo step"; bad=1; }
    printf 'dòng một\ndòng hai' | xclip -quiet -selection clipboard -i >/dev/null 2>&1 &
    sleep 1
    xdotool key shift+Home ctrl+v
    sleep 2
    [ "$(tail -n 2 "$NOTES/Welcome.md")" = "$(printf 'dòng một\ndòng hai')" ] || { echo "FAIL wrap: multi-line paste"; bad=1; }
}

# Rich paste: clipboard text/html -> Markdown (one undo step); Shift+Ctrl+V pastes plain text
# (xclip answers the text target with the same bytes, so that is the HTML source, unconverted).
act_rich() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    printf '<h2>Rich head</h2><p>A <b>bold</b> <a href="https://x.y/">link</a></p><ul><li>one<ul><li>inner</li></ul></li></ul><script>alert(1)</script>' \
        | xclip -quiet -selection clipboard -t text/html -i >/dev/null 2>&1 &
    sleep 1
    xdotool mousemove 400 150 click 1 key ctrl+End Return Return ctrl+v
    sleep 2
    scrot -o "$OUT/rich.png"
    [ "$(tail -n 6 "$NOTES/Welcome.md")" = "$(printf '## Rich head\n\nA **bold** [link](https://x.y/)\n\n- one\n  - inner')" ] \
        || { echo "FAIL rich: HTML not pasted as Markdown"; bad=1; }
    xdotool key ctrl+z
    sleep 2
    grep -qF 'Rich head' "$NOTES/Welcome.md" && { echo "FAIL rich: paste is not one undo step"; bad=1; }
    xdotool key shift+ctrl+v
    sleep 2
    grep -qF '<h2>Rich head</h2>' "$NOTES/Welcome.md" || { echo "FAIL rich: Shift+Ctrl+V converted HTML"; bad=1; }
    printf '**raw** <b>x</b>' | xclip -quiet -selection clipboard -i >/dev/null 2>&1 &
    sleep 1
    xdotool key Return shift+ctrl+v
    sleep 2
    grep -qxF '**raw** <b>x</b>' "$NOTES/Welcome.md" || { echo "FAIL rich: Shift+Ctrl+V plain text"; bad=1; }
}

# Ctrl+R with unsaved edits asks first: Escape (Cancel) keeps them and auto-save resumes; then
# another app rewrites the note while the editor has unsaved edits, Ctrl+R -> Reload shows the
# disk text and nothing is saved over it (no conflict copy, checked at the end).
act_reload() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+End type 'kept' && xdotool key ctrl+r
    sleep 1
    scrot -o "$OUT/reload-ask.png"
    xdotool key Escape
    sleep 2
    grep -qx 'kept' "$NOTES/Reload.md" || { echo "FAIL reload: Cancel lost the edits"; bad=1; }
    xdotool type 'lost' && xdotool key ctrl+r
    printf 'from disk\nline 2\n' > "$NOTES/Reload.md"
    sleep 1
    xdotool key Tab space
    sleep 2
    xdotool type 'X'
    sleep 2
    scrot -o "$OUT/reload.png"
    [ "$(cat "$NOTES/Reload.md")" = "$(printf 'from disk\nline 2\nX')" ] || { echo "FAIL reload: $(cat "$NOTES/Reload.md")"; bad=1; }
}

# Typing `[[we` pops up note names; Enter inserts the first one and closes the link. Then
# `#wo` pops up tags (most notes first); Enter completes it to `#work`.
act_complete() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+End Return type '[[we'
    sleep 1
    scrot -o "$OUT/complete.png"
    xdotool key Return
    xdotool type ' done'
    sleep 2
    grep -qF '[[Welcome]] done' "$NOTES/Welcome.md" || { echo "FAIL complete: [[Welcome]] not inserted"; bad=1; }
    xdotool type ' #wo'
    sleep 1
    scrot -o "$OUT/complete-tag.png"
    xdotool key Return
    xdotool type ' tag'
    sleep 2
    grep -qF '[[Welcome]] done #work tag' "$NOTES/Welcome.md" || { echo "FAIL complete: #work not inserted"; bad=1; }
}

# Outline from Preview: Alt+Ctrl+O focuses the first heading; Down + Enter jumps to `## Second`
# (the `#` inside the code block is not listed) in the editor, where `Second` stays highlighted.
# Ctrl+Home moves the highlight to `Top` while the editor keeps focus (typing lands on line 1).
act_outline() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key alt+ctrl+o
    sleep 1
    scrot -o "$OUT/outline-panel.png"
    xdotool key Down Return
    sleep 1
    xdotool key End
    xdotool type ' X'
    sleep 2
    scrot -o "$OUT/outline.png"
    grep -qxF '## Second X' "$NOTES/Outline.md" || { echo "FAIL outline: cursor not at ## Second"; bad=1; }
    xdotool key ctrl+Home
    sleep 1
    scrot -o "$OUT/outline-follow.png"
    xdotool type 'Y'
    sleep 2
    grep -qxF 'Y# Top' "$NOTES/Outline.md" || { echo "FAIL outline: editor lost focus"; bad=1; }
    # Right-click the (now first) heading `Second X` (#123): Copy Link puts a wiki link on the clipboard.
    xdotool mousemove 740 75; sleep 0.5; xdotool click 3
    sleep 1
    scrot -o "$OUT/outline-menu.png"
    xdotool mousemove 705 101 click 1 # Copy Link
    sleep 1
    [ "$(timeout 5 xclip -selection clipboard -o 2>/dev/null)" = '[[Outline#Second X]]' ] \
        || { echo "FAIL outline: link not on clipboard"; bad=1; }
}

# Backlinks under the outline: Ref.md (line 3; the code span on line 5 is not listed) and
# Projects/Ref2.md (line 2, listed first: folders come first). Tab from the outline into the
# list, Enter opens Ref2.md at line 2.
act_backlinks() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key alt+ctrl+o
    sleep 2
    scrot -o "$OUT/backlinks-panel.png"
    xdotool key Tab Return
    sleep 1
    xdotool type 'X'
    sleep 2
    scrot -o "$OUT/backlinks-open.png"
    grep -qx 'X\[t\](../Target.md)' "$NOTES/Projects/Ref2.md" \
        || { echo "FAIL backlinks: Ref2.md not opened at line 2"; bad=1; }
}

# Tags: the sidebar lists #việc-làm, #work (2 notes), #project under it. Clicking #work lists
# its notes; Enter opens the first (Projects/Tagged.md), then typing goes into it.
act_tags() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 60 530 click 1
    sleep 1
    scrot -o "$OUT/tags-notes.png"
    xdotool key Return
    sleep 1
    xdotool key ctrl+1
    xdotool type 'Z'
    sleep 2
    grep -qxF 'ZNote for #work' "$NOTES/Projects/Tagged.md" || { echo "FAIL tags: tagged note not opened"; bad=1; }
}

# Bookmarks: the header button opens the dialog, Enter bookmarks the open folder (saved in
# state); Alt+2 = a bookmark whose folder is gone (toast only), Alt+1 opens Projects; both
# folders end up in `recent=` (newest first) and the dialog shows a Recent list.
act_bookmarks() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 68 28 click 1
    sleep 1
    scrot -o "$OUT/bookmarks-dialog.png"
    xdotool key Return
    sleep 1
    grep -qxF "bookmark=$NOTES" "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL bookmarks: folder not bookmarked"; bad=1; }
    xdotool key alt+2
    sleep 1
    xdotool key alt+1
    sleep 2
    scrot -o "$OUT/bookmarks-open.png"
    grep -qxF "folder=$NOTES/Projects" "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL bookmarks: Alt+1 did not open Projects"; bad=1; }
    # Recent folders: newest first, then the dialog lists the other one under "Recent".
    [ "$(grep '^recent=' "$XDG_CONFIG_HOME/markdownmate/state" | head -2 | tr '\n' ' ')" = "recent=$NOTES/Projects recent=$NOTES " ] \
        || { echo "FAIL bookmarks: recent folders not saved newest first"; bad=1; }
    xdotool mousemove 68 28 click 1
    sleep 1
    scrot -o "$OUT/bookmarks-recent.png"
    xdotool key Escape
}

# Print (Edit, Preview, Side by side): Shift+Ctrl+P renders the note off-screen and opens GTK's
# print dialog; with no CUPS the only printer is Print to File (PDF, `output.pdf` in the app's
# working directory by default). No window manager, so the dialog sits at the top-left: click the
# printer row, then Print.
act_print() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key shift+ctrl+p
    sleep 3
    xdotool mousemove 85 96 click 1
    sleep 1
    scrot -o "$OUT/$name-dialog.png"
    xdotool mousemove 591 478 click 1
    sleep 4
    head -c 5 output.pdf 2>/dev/null | grep -q '%PDF' || { echo "FAIL $name: output.pdf not written"; bad=1; }
    rm -f output.pdf
}
act_print_preview() { act_print; }
act_print_split() { act_print; }

# Export as HTML: Shift+Ctrl+E opens the save dialog with Wiki.html next to the note; Enter saves.
act_export() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key shift+ctrl+e
    sleep 2
    scrot -o "$OUT/export-dialog.png"
    xdotool key Return
    sleep 2
    grep -qF '<style>' "$NOTES/Wiki.html" 2>/dev/null || { echo "FAIL export: Wiki.html not written"; bad=1; }
    grep -qF 'href="Projects/Ideas/Idea.md"' "$NOTES/Wiki.html" 2>/dev/null || { echo "FAIL export: wiki link not relative"; bad=1; }
    ! grep -qF '<script' "$NOTES/Wiki.html" 2>/dev/null || { echo "FAIL export: script in Wiki.html"; bad=1; }
}

# Copy as HTML: Alt+Shift+Ctrl+C puts the whole note (no selection) as text/html + Markdown.
act_copy() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key alt+shift+ctrl+c
    sleep 1
    scrot -o "$OUT/copy.png"
    local html plain
    html=$(timeout 5 xclip -selection clipboard -t text/html -o 2>/dev/null || true)
    plain=$(timeout 5 xclip -selection clipboard -t UTF8_STRING -o 2>/dev/null || true)
    grep -qF 'href="Projects/Ideas/Idea.md"' <<<"$html" || { echo "FAIL copy: no relative wiki link in text/html"; bad=1; }
    ! grep -qE '<script|<style|mm-missing:|/tmp/' <<<"$html" || { echo "FAIL copy: unsafe text/html"; bad=1; }
    grep -qF '[[Nowhere|missing]]' <<<"$plain" || { echo "FAIL copy: no Markdown in text/plain"; bad=1; }
}

# action <name>: activates window action win.<name> over the app's own D-Bus session (run() gives
# it one), not through the main menu: its GtkPopover logs Gdk-CRITICAL under Xvfb.
action() {
    local pid addr
    pid=$(pgrep -f -- "^$BIN" | head -1)
    addr=$(tr '\0' '\n' < "/proc/$pid/environ" | sed -n 's/^DBUS_SESSION_BUS_ADDRESS=//p')
    DBUS_SESSION_BUS_ADDRESS=$addr gdbus call --session --dest com.homielab.MarkdownMate \
        --object-path /com/homielab/MarkdownMate/window/1 \
        --method org.gtk.Actions.Activate "$1" '[]' '{}' > /dev/null
}

# Export as PDF (main menu): the save dialog suggests Wiki.pdf next to the note; Enter prints it
# there through Print to File, no print dialog.
act_export_pdf() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    action export-pdf
    sleep 2
    scrot -o "$OUT/export_pdf-dialog.png"
    xdotool key Return
    sleep 4
    head -c 5 "$NOTES/Wiki.pdf" 2>/dev/null | grep -q '%PDF' || { echo "FAIL export_pdf: Wiki.pdf not written"; bad=1; }
    rm -f "$NOTES/Wiki.pdf"
}

# Open File (main menu): a note outside the open folder opens with its folder (checked after the
# run, in the state saved on exit).
act_open_file() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    action open-file
    sleep 2
    # Pasted, not typed: the location entry's autocompletion mangles typed paths.
    printf '%s' "$TMP/other/Other.md" | xclip -quiet -selection clipboard -i >/dev/null 2>&1 &
    sleep 1
    xdotool key slash
    sleep 1
    xdotool key ctrl+a ctrl+v
    sleep 1
    scrot -o "$OUT/open_file-dialog.png"
    xdotool key Return
    sleep 3
}

# About and Keyboard Shortcuts (main menu) open without warnings.
act_about() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    action about
    sleep 2
    scrot -o "$OUT/about-dialog.png"
    xdotool key Escape
    sleep 1
    action show-help-overlay
    sleep 2
    scrot -o "$OUT/about-shortcuts.png"
    xdotool key Escape
}

run edit preview=false ctrl+p
# SIGTERM < 1s after typing (before auto-save): run() kills the app right after this act.
act_sigterm() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key ctrl+End type 'killed'
}
NOTE="$NOTES/Term.md" run sigterm mode=edit
grep -q 'killed' "$NOTES/Term.md" || { echo "FAIL sigterm: typed text not saved"; fail=1; }
# Old state file (before side by side): preview=true must still open Preview.
run preview preview=true
run split "$(printf 'mode=split\nsync_scroll=false')"
run sync mode=split
# Large note in Side by side: opening it (#108) and the re-render after typing (#103) run off
# the main thread (debug build: seconds for this ~250KB note), so typing goes on and is saved.
act_big() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+Home type 'BIG '
    sleep 0.5
    xdotool type 'TYPED '
    sleep 8
    scrot -o "$OUT/big-typed.png"
}
for i in $(seq 400); do
    printf '## Part %s #tag\n\nText $x^%s$ and [[Welcome]].\n\n- [ ] task\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn f(x: u32) -> u32 { x * %s }\n```\n\n```python\ndef g(x):\n    return [i for i in range(x)]\n```\n\n$$\n\\frac{a}{%s}\n$$\n\n' "$i" "$i" "$i" "$i"
done > "$NOTES/Big.md"
for _ in 1 2 3; do cat "$NOTES/Big.md"; done > "$NOTES/Big2.md" && mv "$NOTES/Big2.md" "$NOTES/Big.md"
NOTE="$NOTES/Big.md" run big mode=split
grep -q '^BIG TYPED ## Part 1 ' "$NOTES/Big.md" || { echo "FAIL big: typed text not saved"; fail=1; }
rm "$NOTES/Big.md"
# The note ends with an empty line: the cursor (Ctrl+End) types Z on its own last line.
[ "$(tail -n1 "$NOTES/Welcome.md")" = Z ] || { echo "FAIL sync: scrolling the preview moved the editor cursor"; fail=1; }
sed -i '$d' "$NOTES/Welcome.md"
run find mode=preview
run goto mode=preview
NOTE="$NOTES/Search.md" run search mode=preview
run format mode=edit
# Proportional editor font: table lines still show in monospace (#104).
NOTE="$NOTES/Table.md" run table "$(printf 'mode=edit\nfont=Ubuntu 14')"
run paste mode=edit
run wrap mode=edit
run rich mode=edit
NOTE="$NOTES/Reload.md" run reload mode=edit
run complete mode=edit
NOTE="$NOTES/Links.md" run link mode=preview
NOTE="$NOTES/Wiki.md" run wiki mode=preview
NOTE="$NOTES/Wiki.md" run editor_wiki mode=edit
NOTE="$NOTES/Zanchor.md" run anchor mode=preview
NOTE="$NOTES/ZanchorLink.md" run anchor_note mode=preview
anchor_cursor anchor_note ZanchorB.md '## Mục hai'
NOTE="$NOTES/Zanchor.md" run editor_anchor mode=edit
anchor_cursor editor_anchor Zanchor.md '## Phần cuối'
NOTE="$NOTES/ZanchorLink.md" run editor_anchor_note mode=edit
anchor_cursor editor_anchor_note ZanchorB.md '## Mục hai'
NOTE="$NOTES/Zanchor.md" run split_anchor mode=split
NOTE="$NOTES/Image.md" run hover mode=edit
NOTE="$NOTES/Wiki.md" run export mode=preview
NOTE="$NOTES/Tasks.md" run task mode=preview
NOTE="$NOTES/Wiki.md" run copy mode=preview
NOTE="$NOTES/Code.md" run codecopy mode=preview
# Formulas as MathML (screenshot): inline, block, a price left as text, bad LaTeX as code.
NOTE="$NOTES/Math.md" run math mode=preview
# Formulas coloured in the editor (screenshot): not the price, not code.
NOTE="$NOTES/Math.md" run math_edit mode=edit
NOTE="$NOTES/Wiki.md" run export_pdf mode=preview
mkdir -p "$TMP/other" && echo '# Other' > "$TMP/other/Other.md"
run open_file mode=edit
grep -qx "folder=$TMP/other" "$XDG_CONFIG_HOME/markdownmate/state" && grep -qx "note=$TMP/other/Other.md" "$XDG_CONFIG_HOME/markdownmate/state" \
    || { echo "FAIL open_file: Other.md not opened in its folder"; fail=1; }
run about mode=edit
run print mode=edit
run print_preview mode=preview
run print_split mode=split
NOTE="$NOTES/Outline.md" run outline mode=preview
NOTE="$NOTES/Target.md" run backlinks mode=edit
NOTE="$NOTES/Tags.md" run tags mode=split
# pref <row title>: opens Preferences (Ctrl+,) and focuses that row through the dialog's own
# search (Ctrl+F, the title, Tab to the first result, Space jumps to the real row; Enter does
# not), so no Tab counting when Preferences gains rows.
pref() {
    xdotool key ctrl+comma
    sleep 2
    xdotool key ctrl+f
    sleep 1
    xdotool type "$1"
    sleep 1
    xdotool key Tab space
    sleep 1
}

# Tags off in the state file: no Tags panel, no tag colour/chips. Preferences: the Tags switch
# turns them back on at once and is saved.
act_tags_off() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    pref 'Tags'
    xdotool key space
    sleep 2
    scrot -o "$OUT/tags_off-prefs.png"
    xdotool key Escape
    sleep 2
    scrot -o "$OUT/tags_off-on.png"
    grep -qx 'tags=true' "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL tags_off: Tags switch not saved"; bad=1; }
}
NOTE="$NOTES/Tags.md" run tags_off "$(printf 'mode=split\ntags=false')"
# Preferences from the state file (line numbers, no wrap, 2-space list indent, bold 14pt font);
# Tab on a list item indents 2 spaces; Ctrl+, opens the dialog.
act_prefs() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+End Return type -- '- a'
    xdotool key Return
    xdotool type 'b'
    xdotool key Tab
    sleep 2
    grep -qx '  - b' "$NOTES/Outline.md" || { echo "FAIL prefs: list not indented 2 spaces"; bad=1; }
    pref 'Show Line Numbers'
    scrot -o "$OUT/prefs-dialog.png"
    xdotool key space
    sleep 1
    scrot -o "$OUT/prefs-toggled.png"
    grep -qx 'line_numbers=false' "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL prefs: switch not saved"; bad=1; }
}
NOTE="$NOTES/Outline.md" run prefs "$(printf 'mode=edit\nline_numbers=true\nwrap=false\nindent=2\nfont=Monospace Bold 14')"
# Typewriter scrolling on in the state file: Go to Line 40 of the long Welcome note puts it in
# the middle of the editor (typewriter.png); Preferences ▸ Typewriter Scrolling off is saved.
act_typewriter() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key ctrl+l; sleep 1; xdotool type '40'; xdotool key Return
    sleep 2
    scrot -o "$OUT/typewriter.png"
    pref 'Typewriter Scrolling'
    xdotool key space
    sleep 1
    grep -qx 'typewriter=false' "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL typewriter: switch not saved"; bad=1; }
}
run typewriter "$(printf 'mode=split\ntypewriter=true')"
# Zoom: Ctrl+= twice and Ctrl+- once from 120% (state) -> 130% saved; screenshot shows it.
act_zoom() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+equal ctrl+equal ctrl+minus
    sleep 2
    scrot -o "$OUT/zoom.png"
    grep -qx 'zoom=130' "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL zoom: zoom=130 not saved"; bad=1; }
}
run zoom "$(printf 'mode=split\nzoom=120')"
# Themes: editor scheme (Paper/Graphite from packaging/styles) + preview colours, checkbox.
for theme in solarized paper graphite; do
    NOTE="$NOTES/Themes.md" run "theme_$theme" "$(printf 'mode=split\ntheme=%s' "$theme")"
    ! grep -F 'style scheme' "$OUT/theme_$theme.log" || { echo "FAIL theme_$theme: scheme not found"; fail=1; }
done
# Style: Dark from the state file (window, editor, preview dark); Preferences ▸ Style back
# to System is saved at once.
act_style() {
    grep -qx 'style=dark' "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL style: style=dark not kept"; bad=1; }
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    pref 'Style'
    xdotool key space
    sleep 1
    xdotool key Up Up Return
    sleep 2
    scrot -o "$OUT/style-system.png"
    grep -qx 'style=system' "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL style: style=system not saved"; bad=1; }
}
NOTE="$NOTES/Themes.md" run style "$(printf 'mode=split\nstyle=dark')"
# Drag and drop in the sidebar: drag the open note Code.md (row 2) onto the Projects folder
# (row 1); it moves there and the editor follows it (typing lands in Projects/Code.md). Hovering
# a folder while dragging opens it, so Projects is now open: Ideas row 2, Code.md row 3. Then
# Code.md onto Ideas, where a Code.md exists: refused, both files unchanged. move-hover.png:
# the Projects row is highlighted while hovering (not checked).
drag() { # drag <from y> <to y> [shot] in the sidebar; shot: screenshot name while hovering
    xdotool mousemove 60 "$1" mousedown 1
    # Small steps: GTK starts the drag only after the pointer passes the drag threshold.
    for y in $(seq "$1" -5 "$2"); do xdotool mousemove 62 "$y"; sleep 0.1; done
    sleep 0.5; xdotool mousemove 63 "$2"; sleep 0.5
    [ -n "${3:-}" ] && scrot -o "$OUT/$3.png"
    xdotool mouseup 1
    sleep 2
}
act_move() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    drag 113 75 move-hover # Projects row highlighted as the drop target
    scrot -o "$OUT/move-done.png"
    wait_for 5 test -e "$NOTES/Projects/Code.md" && [ ! -e "$NOTES/Code.md" ] || { echo "FAIL move: Code.md not moved into Projects"; bad=1; }
    xdotool mousemove 400 150 click 1 key ctrl+Home type 'M'
    wait_for 5 grep -qx 'M```sh' "$NOTES/Projects/Code.md" || { echo "FAIL move: editor did not follow the moved note"; bad=1; }
    echo inner > "$NOTES/Projects/Ideas/Code.md"
    drag 151 113
    scrot -o "$OUT/move-taken.png"
    [ "$(cat "$NOTES/Projects/Ideas/Code.md")" = inner ] && head -1 "$NOTES/Projects/Code.md" | grep -q '^M' \
        || { echo "FAIL move: existing Code.md replaced"; bad=1; }
    rm -f "$NOTES/Projects/Ideas/Code.md"
}
NOTE="$NOTES/Code.md" run move mode=edit
# Drop files from another app (#118): examples/drag_files is the "other app". Drop Out.md +
# a .txt on a root note row (Code.md, row 2) -> Out.md copied as Out 2.md (Out.md exists, kept),
# the .txt skipped (toast, drop-skip.png). Then drop Single.md on the Projects row -> copied
# into Projects and opened (typing `S` lands in it); the outside files stay.
drop_files() { # drop_files <to y> <shot> <files...>: drag from the helper window onto the sidebar
    local y=$1 shot=$2; shift 2
    "${CARGO_TARGET_DIR:-target}/debug/examples/drag_files" "$@" &
    local helper=$!
    local win; win=$(timeout 5 xdotool search --sync --onlyvisible --name '^drag-files$' | head -1)
    xdotool windowmove "$win" 900 500; sleep 1
    xdotool mousemove 1000 600 mousedown 1
    for x in $(seq 1000 -40 60); do xdotool mousemove "$x" "$((y + (x - 60) / 2))"; sleep 0.05; done
    sleep 0.5; xdotool mousemove 62 "$y"; sleep 0.5
    scrot -o "$OUT/$shot-hover.png"
    xdotool mouseup 1; sleep 2
    scrot -o "$OUT/$shot.png"
    kill "$helper" 2>/dev/null || true; wait "$helper" 2>/dev/null || true
}
act_dropin() {
    mkdir -p "$TMP/outside"
    echo outside > "$TMP/outside/Out.md"; echo txt > "$TMP/outside/skip.txt"; echo single > "$TMP/outside/Single.md"
    echo keep > "$NOTES/Out.md"
    drop_files 113 drop-skip "$TMP/outside/Out.md" "$TMP/outside/skip.txt"
    [ "$(cat "$NOTES/Out.md")" = keep ] && [ "$(cat "$NOTES/Out 2.md" 2>/dev/null)" = outside ] \
        || { echo "FAIL dropin: Out.md not copied as Out 2.md"; bad=1; }
    [ ! -e "$NOTES/skip.txt" ] || { echo "FAIL dropin: .txt copied"; bad=1; }
    drop_files 75 drop-single "$TMP/outside/Single.md"
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+Home type 'S'
    sleep 2
    head -1 "$NOTES/Projects/Single.md" 2>/dev/null | grep -q '^S' || { echo "FAIL dropin: Single.md not copied into Projects and opened"; bad=1; }
    [ "$(cat "$TMP/outside/Single.md")" = single ] || { echo "FAIL dropin: source changed"; bad=1; }
    rm -f "$NOTES/Out.md" "$NOTES/Out 2.md" "$NOTES/Projects/Single.md"
}
run dropin mode=edit
# Open Quickly ô trống (#113): mở Welcome rồi Todo, Ctrl+P, Enter -> về Welcome (gõ `Q` vào đầu).
act_quick_recent() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key ctrl+p; sleep 1; xdotool type Todo; sleep 1; xdotool key Return; sleep 1
    xdotool key ctrl+p; sleep 1
    scrot -o "$OUT/quick-recent.png"
    xdotool key Return; sleep 1
    xdotool type 'Q'
    sleep 2
    head -1 "$NOTES/Welcome.md" | grep -q '^Q' || { echo "FAIL quick_recent: Enter did not go back to Welcome"; bad=1; }
}
run quick_recent mode=edit
sed -i '1s/^Q//' "$NOTES/Welcome.md"
# Vị trí con trỏ theo note: Ctrl+L 40, sang Todo rồi về Welcome (Open Quickly) -> con trỏ vẫn
# dòng 40 (gõ `Z`); bấm mở rộng Projects. Khởi động lại (state giữ nguyên) -> gõ `Y` vẫn ở
# dòng 40 (sau `Z`), Projects còn mở rộng (`expanded=` trong state, ảnh pos-restart.png).
act_pos() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool key ctrl+l; sleep 1; xdotool type '40'; xdotool key Return; sleep 1
    for n in Todo Welcome; do
        xdotool key ctrl+p; sleep 1; xdotool type "$n"; sleep 1; xdotool key Return; sleep 1
    done
    xdotool type 'Z'
    xdotool mousemove 60 75 click 1
    sleep 2
    [ "$(sed -n 40p "$NOTES/Welcome.md" | head -c1)" = Z ] || { echo "FAIL pos: cursor not back on line 40"; bad=1; }
}
act_pos_restart() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 300 type 'Y'
    sleep 2
    [ "$(sed -n 40p "$NOTES/Welcome.md" | head -c2)" = ZY ] || { echo "FAIL pos_restart: cursor not restored after restart"; bad=1; }
    grep -qxF "expanded=$NOTES/Projects" "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL pos_restart: expanded folder not saved"; bad=1; }
}
run pos mode=edit
KEEP_STATE=1 run pos_restart mode=edit
run bookmarks "$(printf 'mode=edit\nbookmark=%s\nbookmark=%s' "$NOTES/Projects" "$TMP/gone")"
# Sidebar right-click on Projects (#109): Copy Path puts its absolute path on the clipboard;
# Show in Files must not kill the app (the container may have no file manager: a toast at most).
act_ctxmenu() {
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    # Pointer moved first: a right-click arriving with the first motion event was sometimes missed.
    xdotool mousemove 60 75; sleep 0.5; xdotool click 3
    sleep 1
    scrot -o "$OUT/ctxmenu-open.png"
    xdotool mousemove 79 237 click 1 # Copy Path
    sleep 1
    [ "$(timeout 5 xclip -selection clipboard -o 2>/dev/null)" = "$NOTES/Projects" ] \
        || { echo "FAIL ctxmenu: path not on clipboard"; bad=1; }
    xdotool mousemove 60 75; sleep 0.5; xdotool click 3
    sleep 1
    xdotool mousemove 79 203 click 1 # Show in Files
    sleep 2
    scrot -o "$OUT/ctxmenu.png"
}
run ctxmenu mode=edit
# Web process killed (#82): the app stays up and toasts "Preview stopped" with Reload; typing
# re-renders the preview in a new web process.
act_webcrash() {
    pkill -f WebKitWebProcess || { echo "FAIL webcrash: no web process"; bad=1; }
    sleep 2
    scrot -o "$OUT/webcrash.png"
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate windowactivate --sync 2>/dev/null || true
    xdotool mousemove 400 150 click 1 key ctrl+Home type 'X'
    sleep 3
    pgrep -f WebKitWebProcess > /dev/null || { echo "FAIL webcrash: preview not restarted"; bad=1; }
}
run webcrash mode=split
# WebKit's sandbox blocked (#82): the container has no user namespaces, like Ubuntu 24.04 without
# the AppArmor profile, where WebKit used to abort the app. Now: a toast, back to Edit.
act_nosandbox() {
    grep -qx 'mode=edit' "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL nosandbox: not back to Edit"; bad=1; }
}
unset WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS
run nosandbox mode=split
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
# URL scheme: the first link starts the app (cold start), later ones run a second instance that
# forwards them to it over D-Bus (one session bus for both).
mkdir -p "$TMP/marked" && echo '# Marked' > "$TMP/marked/m.md"
printf 'folder=%s\nnote=%s\nmode=preview\nbookmark=%s\n' "$NOTES" "$NOTES/Welcome.md" "$TMP/marked" \
    > "$XDG_CONFIG_HOME/markdownmate/state"
cat > "$TMP/url.sh" <<EOF
"$BIN" 'markdownmate://append?path=Todo.md&text=Cold+%C4%91' > "$OUT/url.log" 2>&1 &
sleep 5
for link in 'append?path=Todo.md&text=Warm%20two' 'create?title=Idea&text=Hi+there' \
    'create?title=Idea&text=Again' 'append?path=../x.md&text=no' 'append?path=New.md&text=no' \
    'open?path=$TMP/marked/m.md'; do
    timeout 10 "$BIN" "markdownmate://\$link" >> "$OUT/url.log" 2>&1
    sleep 2
done
scrot -o "$OUT/url.png"
kill %1
EOF
dbus-run-session -- bash "$TMP/url.sh"
bad=0
[ "$(tail -2 "$NOTES/Todo.md")" = "$(printf 'Cold đ\nWarm two')" ] || { echo "FAIL url: append"; bad=1; }
[ "$(cat "$NOTES/Idea.md")" = "Hi there" ] && [ "$(cat "$NOTES/Idea 2.md")" = "Again" ] || { echo "FAIL url: create"; bad=1; }
[ ! -e "$NOTES/New.md" ] && [ ! -e "$TMP/x.md" ] || { echo "FAIL url: unsafe link wrote a file"; bad=1; }
grep -qx "folder=$TMP/marked" "$XDG_CONFIG_HOME/markdownmate/state" || { echo "FAIL url: bookmarked note not opened"; bad=1; }
if grep -E 'CRITICAL|panicked|size_allocate' "$OUT/url.log"; then echo "FAIL url: warning in url.log"; bad=1; fi
[ $bad = 0 ] && echo "ok   url -> screenshots/url.png" || fail=1
if ls "$NOTES"/*conflict* "$NOTES"/*/*conflict* 2>/dev/null; then echo "FAIL: conflict copy written"; fail=1; fi
exit $fail
