#!/usr/bin/env bash
# Regenerate the screenshots in docs/screenshots/ (README, store, website):
#   ./dev.sh packaging/make-screenshots.sh
# Uses a throwaway folder of sample notes and a temporary state, so no personal data ends up in
# an image. The app runs on Xvfb (screen = window = 1280x800, no window manager), like
# scripts/smoke.sh.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo build -q
BIN="${CARGO_TARGET_DIR:-target}/debug/markdownmate"
OUT="$PWD/docs/screenshots"
mkdir -p "$OUT"
TMP=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$TMP"' EXIT

# ── sample notes ──
N="$TMP/Notes"
mkdir -p "$N/Projects" "$N/Journal" "$N/Recipes/assets"
cat > "$N/Projects/Website Redesign.md" <<'EOF'
# Website Redesign

Launch the new site before the **spring release**. Design notes live in [[Design System]],
the API side in [[API Notes]]. #work #project

## Milestones

| Phase      | Owner | Due        | Status      |
|------------|-------|------------|-------------|
| Wireframes | Linh  | 2026-11-04 | Done        |
| Visual     | Sam   | 2026-11-18 | In progress |
| Build      | Minh  | 2026-12-02 | Not started |

## To do

- [x] Collect feedback from the support team
- [x] Pick a type scale and colour palette
- [ ] Write copy for the landing page
- [ ] Set up redirects for old URLs
  - [ ] `/blog/*` → `/notes/*`
  - [ ] `/download` → `/get`

## Open questions

1. Do we keep the dark hero image?
2. Should pricing move to its own page?

> Ship small, ship often. Everything else is a draft.
EOF
cat > "$N/Projects/API Notes.md" <<'EOF'
# API Notes

The sync endpoint returns notes changed since a cursor. #work

```mermaid
sequenceDiagram
    App->>Server: GET /notes?since=cursor
    Server-->>App: changed notes + new cursor
    App->>App: merge and save
```

A busy server answers 429: retry after $t_n = \min(60, 2^n)$ seconds.

## Fetch changes

```python
def changed_since(cursor: str) -> list[dict]:
    """Return notes changed after `cursor`."""
    r = session.get("/notes", params={"since": cursor})
    r.raise_for_status()
    return r.json()["notes"]
```

## Status codes

| Code | Meaning              | Retry? |
|------|----------------------|--------|
| 200  | OK                   | –      |
| 304  | Nothing changed      | No     |
| 429  | Too many requests    | Yes    |
| 503  | Maintenance          | Yes    |

## Parsing in Rust

```rust
#[derive(Debug, Deserialize)]
struct Note {
    id: u64,
    title: String,
    tags: Vec<String>,
}

fn titles(notes: &[Note]) -> Vec<&str> {
    notes.iter().map(|n| n.title.as_str()).collect()
}
```

See also [[Website Redesign]].
EOF
cat > "$N/Projects/Sync Design.md" <<'EOF'
# Sync Design

How two devices agree on a note. Related: [[API Notes]].

## Flow

```mermaid
flowchart LR
    Edit[Edit on laptop] --> Push[Push change]
    Push --> Check{Newer on server?}
    Check -- No --> Save[Save]
    Check -- Yes --> Merge[Merge both]
    Merge --> Save
```

## Retry budget

With a first wait $t_0$ and doubling after each failure, the total wait after $n$ retries is

$$
T_n = \sum_{k=0}^{n-1} t_0 \, 2^k = t_0 \left(2^n - 1\right)
$$

so five retries from one second add up to $T_5 = 31$ s.
EOF
cat > "$N/Projects/Design System.md" <<'EOF'
# Design System

Colours, type scale and spacing for the new site. #project #design

- Primary: `#3584e4`
- Body text: 17 px, line height 1.5
- Spacing unit: 8 px
EOF
cat > "$N/Journal/2026-10-19.md" <<'EOF'
# Sunday, 19 October

Long walk by the river in the morning. Finished *The Pragmatic Programmer*. #reading

- [x] Groceries
- [x] Call Mum
- [ ] Plan next week
EOF
cat > "$N/Journal/2026-10-20.md" <<'EOF'
# Monday, 20 October

Kick-off for the [[Website Redesign]]. Good energy in the room. #work

**Tomorrow:** review the wireframes with Linh.
EOF
cat > "$N/Recipes/Pho Bo.md" <<'EOF'
# Phở Bò

A slow Sunday soup: clear, fragrant broth, rice noodles and thin slices of beef. #recipe

![A bowl of phở](assets/pho.png)

## Ingredients

| Ingredient        | Amount  |
|-------------------|---------|
| Beef bones        | 2 kg    |
| Rice noodles      | 400 g   |
| Star anise        | 4       |
| Cinnamon stick    | 1       |
| Onion and ginger  | 1 each, charred |

## Steps

1. Blanch the bones, rinse, then simmer for **6 hours**.
2. Toast the spices and add them for the last hour.
3. Season with fish sauce and a little rock sugar.
4. Pour the hot broth over noodles and raw beef.

> Serve with basil, lime and bean sprouts on the side.
EOF
cat > "$N/Recipes/Banana Bread.md" <<'EOF'
# Banana Bread

Three very ripe bananas, one bowl, one hour. #recipe

- [ ] Buy walnuts
EOF
cat > "$N/Reading List.md" <<'EOF'
# Reading List

- [x] The Pragmatic Programmer
- [ ] A Philosophy of Software Design
- [ ] Designing Data-Intensive Applications

#reading
EOF
cat > "$N/Inbox.md" <<'EOF'
# Inbox

- Idea: weekly review template
- Try the new coffee place on Nguyễn Huệ
EOF
# A small "photo" for the recipe: a bowl on a warm background (generated, no external file).
python3 - "$N/Recipes/assets/pho.png" <<'EOF'
import struct, sys, zlib
w, h = 560, 200
rows = []
for y in range(h):
    row = bytearray([0])
    for x in range(w):
        dx, dy = (x - w / 2) / 150, (y - 70) / 120
        if dy > 0 and dx * dx + dy * dy < 1:  # bowl
            c = (245, 245, 240) if dx * dx + dy * dy > 0.8 else (196, 120, 60)
        elif abs(dy) < 0.06 and abs(dx) < 1:  # broth surface
            c = (214, 150, 80)
        else:  # background gradient
            c = (250 - y // 8, 225 - y // 6, 180 - y // 5)
        row += bytes(c)
    rows.append(bytes(row))
def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(b"".join(rows), 9)) + chunk(b"IEND", b"")
open(sys.argv[1], "wb").write(png)
EOF

# ── display and session ──
export DISPLAY=:98
Xvfb :98 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
sleep 1
export XDG_CONFIG_HOME="$TMP/config" XDG_DATA_HOME="$TMP/data" XDG_CACHE_HOME="$TMP/cache"
export XDG_RUNTIME_DIR="$TMP/run"
mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME/markdownmate" "$XDG_CONFIG_HOME/gtk-4.0"
chmod 700 "$XDG_RUNTIME_DIR"
# Ubuntu's UI font, as on a stock Ubuntu desktop.
printf '[Settings]\ngtk-font-name=Ubuntu 11\n' > "$XDG_CONFIG_HOME/gtk-4.0/settings.ini"
# The container (root, no user namespaces) cannot run WebKit's bubblewrap sandbox.
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
export NO_AT_BRIDGE=1 GTK_A11Y=none

fail=0
# shot <file> <note> <state lines> [act function]: start the app, size the window to the screen,
# run the act (clicks/keys), screenshot, quit.
shot() {
    local file=$1 note=$2 mode=$3 act=${4:-}
    printf 'folder=%s\nnote=%s\nfont=Ubuntu Mono 13\n%s\n' "$N" "$N/$note" "$mode" \
        > "$XDG_CONFIG_HOME/markdownmate/state"
    dbus-run-session -- "$BIN" > "$TMP/log" 2>&1 &
    local pid=$!
    sleep 4
    timeout 5 xdotool search --sync --onlyvisible --class markdownmate \
        windowmove %@ 0 0 windowsize %@ 1280 800 windowactivate --sync %@ 2>/dev/null || true
    sleep 2
    # Open the folder of the note in the sidebar (folders start collapsed).
    case $note in */*) xdotool mousemove 80 "$(folder_y "${note%%/*}")" click 1 mousemove 640 28; sleep 1 ;; esac
    [ -n "$act" ] && "$act"
    sleep 1
    scrot -o "$OUT/$file"
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    pkill -f -- "$BIN" 2>/dev/null || true
    for _ in $(seq 50); do pgrep -f -- "$BIN" >/dev/null || break; sleep 0.1; done
    if grep -E 'CRITICAL|panicked|size_allocate|WARNING' "$TMP/log"; then
        echo "FAIL $file: warning in the app log"; fail=1
    else
        echo "ok   docs/screenshots/$file"
    fi
}
# Sidebar rows (folders first, sorted): Journal, Projects, Recipes.
folder_y() { case $1 in Journal) echo 76 ;; Projects) echo 114 ;; Recipes) echo 152 ;; esac; }

act_quick() { xdotool key ctrl+p; sleep 1; xdotool type 'no'; sleep 1; }
act_tags() { xdotool mousemove 80 768 click 1 mousemove 640 28; sleep 1; }

act_diagrams() { sleep 3; }

shot 1-editor.png "Projects/Website Redesign.md" "$(printf 'mode=edit\nformat_bar=true')"
shot 2-side-by-side.png "Projects/API Notes.md" mode=split act_diagrams
shot 3-preview.png "Recipes/Pho Bo.md" mode=preview
shot 4-open-quickly.png "Projects/Website Redesign.md" mode=edit act_quick
shot 5-tags.png "Journal/2026-10-20.md" mode=split act_tags
ADW_DEBUG_COLOR_SCHEME=prefer-dark shot 6-dark.png "Projects/API Notes.md" mode=split act_diagrams
shot 7-themes.png "Recipes/Pho Bo.md" "$(printf 'mode=split\ntheme=solarized')"
shot 8-math-diagrams.png "Projects/Sync Design.md" mode=preview act_diagrams
exit $fail
