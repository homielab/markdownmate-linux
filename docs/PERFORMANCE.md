# Performance with large notes and folders

Large notes first; a folder of 10,000 notes: [below](#a-folder-of-10000-notes).

`src/perf.rs` has an ignored test that times what makes a multi-megabyte note slow:

- **open in editor**: loading the text into the GtkSourceView buffer and highlighting all of
  its Markdown (the editor does the visible part first and the rest in idle time);
- **full render**: the preview page (`render::page`) of a note seen for the first time;
- **without code highlighting**: the same body with syntect turned off, to show its share;
- **re-render after a keystroke**: the whole page again after one character typed into a
  code block in the middle of the note (what the preview did before it patched pages, and
  still does for notes it cannot cut, below);
- **per block: first render / update after the keystroke**: what the preview does now
  (`incremental`): the note rendered per top-level block, then the same keystroke updating
  only the blocks it touched, plus the size of the HTML sent to the page. The test checks
  that the patched page is byte-identical to the whole render.

The notes are generated (`perf::sample_note`): 1 MB and 5 MB of sections with headings, inline
styling, web / note / wiki links, tags, task and nested lists, a table, a quote, math and
fenced code in Rust, Python, JavaScript and without a language. Each time is the fastest of
3–5 runs.

## Run it

In the build container (`./dev.sh`), with a virtual display for the editor timing:

```sh
./dev.sh sh -c 'export DISPLAY=:99; Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 & sleep 1;
  cargo test --release large_notes -- --ignored --nocapture --test-threads=1'
```

On Ubuntu, `xvfb-run -a cargo test --release large_notes -- --ignored --nocapture
--test-threads=1` (or plain `cargo test …` in a desktop session). Without a display the editor
timing says "no display". It takes one to five minutes.

## Check that a change keeps the HTML

Set `MM_PERF_OUT` to a folder to also write each note's preview page there (`1mb.html`,
`5mb.html`, and `*-edited.html` after the keystroke, in the dark Paper theme). Run it once
on the old build and once on the new one, then compare:

```sh
./dev.sh sh -c '… MM_PERF_OUT=/target/perf-before cargo test --release large_notes -- --ignored'
# change the code
./dev.sh sh -c '… MM_PERF_OUT=/target/perf-after cargo test --release large_notes -- --ignored'
./dev.sh sh -c 'for f in 1mb 1mb-edited 5mb 5mb-edited; do cmp /target/perf-before/$f.html /target/perf-after/$f.html; done'
```

(`/target` is the `markdownmate-target` volume, so the pages stay out of the repo.)

## Results

Apple Silicon, 12 cores, Docker (OrbStack), release build. The two columns ran back to back
on a busy machine; on a quiet one the full render before the change took 0.8 s (1 MB) and
8.6 s (5 MB).

| Note | Step | Before | After |
|------|------|-------:|------:|
| 1 MB (50,914 lines) | open in editor | 0.3 s | 0.5 s (unchanged code; noise) |
| | full render | 4.2 s | 0.9 s |
| | without code highlighting | 0.08 s | 0.14 s |
| | re-render after a keystroke | 2.0 s | 0.23 s |
| 5 MB (250,563 lines) | open in editor | 1.0 s | 2.9 s (unchanged code; noise) |
| | full render | 23.0 s | 5.4 s |
| | without code highlighting | 0.9 s | 0.9 s |
| | re-render after a keystroke | 20.8 s | 1.4 s |

Code highlighting (syntect) was 90–97 % of the render. It now runs before comrak writes the
page: every code block with a known language is highlighted on all cores at once, and the
blocks of the last render are kept (by language and code), so a re-render highlights only
the code blocks that changed. The rendered HTML is byte-identical.

### Patching only the changed blocks

Same machine, quiet, release build, one run (2026-10-25). "Before" is the whole page rendered
again and swapped in (the previous preview, still measured as *re-render after a
keystroke*); "after" is the update the preview now makes.

| Note | Step | Before | After |
|------|------|-------:|------:|
| 1 MB | first preview of the note | 0.84 s | 0.89 s |
| | update after a keystroke | 0.12 s | 0.003 s (5 KB of HTML sent instead of 14.6 MB) |
| 5 MB | first preview of the note | 3.7 s | 1.6 s |
| | update after a keystroke | 0.41 s | 0.013 s (5 KB of HTML sent instead of 72 MB) |

The note is cut between top-level blocks where a blank line separates them and nothing runs
on (open list, fence, indented code, HTML block); each part renders on its own to exactly its
part of the whole page (on all cores for the first preview). After an edit only the parts it
touched are rendered again (with one part of context; more if a fence or list now runs on),
and the page replaces just their DOM nodes and moves the source lines of the nodes after them
(scroll sync, checkboxes, Copy buttons). Formulas and diagrams outside the edit are not
touched. Repeated heading titles are renumbered over the whole note. Notes with footnotes or
reference link definitions, and edits that may move the end of front matter, render whole
as before. The times above are the Rust side; applying a 5 KB patch in the page is a few DOM
operations, while swapping in a 72 MB body was not even part of the old number.

`src/incremental.rs` has tests that replay random edits and check every patched page against
the whole render (`SEED=<n> cargo test random_edits` for other edits); `perf::large_notes`
checks it too.

### Typing with Focus Mode

`perf::focus_typing` (ignored; run it like `large_notes`, with `focus_typing` instead) types
300 characters into a paragraph in the middle of the 5 MB note in a shown editor, waiting for
each frame to be drawn, with Focus Mode off, on, then off again; it also times turning Focus
Mode on and a cursor jump to another paragraph. In the container (Xvfb, GTK on llvmpipe,
2026-10-31):

| Focus Mode | to the next frame, per keystroke | CPU per keystroke |
|---|---|---|
| off | 39.6 ms | 10.6 ms |
| on | 33.8 ms | 13.2 ms |
| off again | 42.3 ms | 11.9 ms |

Turning it on takes under 1 ms, moving to another paragraph 1 ms: nothing in the note is
tagged; the view draws a veil over the visible lines outside the paragraph (two rectangles a
frame), and the paragraph is read at most 1,000 lines each way. The frame times are software
rendering's; the difference is within the noise.

### Typing with spell check

`perf::spell_typing` (ignored; run it like `large_notes`, with `spell_typing` instead; needs the
`hunspell-en-us` dictionary, in the container) types 300 characters of prose with a misspelled
word into a paragraph in the middle of the 5 MB note in a shown editor, waiting for each frame:
with spell check off, on while libspelling still checks the rest of the note, and on once the
whole note is checked. In the container (Xvfb, GTK on llvmpipe, 2026-10-31):

| Spell check | to the next frame, per keystroke | CPU per keystroke |
|---|---|---|
| off | 26.2 ms (worst 103 ms) | 18.9 ms |
| on, note being checked | 23.9 ms (worst 109 ms) | 19.8 ms |
| on, note checked | 30.5 ms (worst 106 ms) | 12.8 ms |

Typing is as fast with it on: libspelling checks only the words that change, and runs the first
check of the note in slices of a few milliseconds between frames. That first check goes from the
top down at about 6 KB/s: 825 s for the 5 MB note (96 s of CPU), so about 3 minutes per MB and
a few seconds for a typical note. Until it gets there, words far down a very large note are not
underlined, typed ones included. Turning it off takes 36 ms (the underline removed from the
whole note).

libspelling 0.2 (Ubuntu 24.04) cannot check the visible part first, so the app does not
spell-check a note over 1 MB (`spell::LARGE_NOTE`, bytes on disk) when it opens it: the status
bar says *Spell check is off for notes over 1 MB*, the Preferences switch stays on, and
right-click ▸ Check Spelling checks that note anyway. A note just under the limit takes about
3 minutes. The app opening a 5 MB note of prose (debug build, Xvfb in the container, 2026-10-31), CPU used
in the 20 s after it is shown (`spell_check=true`):

| 5 MB note | background check | CPU in the 20 s after opening | typing |
|---|---|---|---|
| before (checked) | ~825 s (96 s of CPU) | 660 ms | the "on" rows above |
| after (held off) | none | 30 ms | the "off" row above |

A small note: 40 ms before, 20 ms after, underlines as before.

## A folder of 10,000 notes

`scripts/measure-folder.sh` times launch, the sidebar, the Tags list and Backlinks with a large
folder, and the app's memory. It writes the folder with `examples/generate_notes.rs` (the same
on every run): 10,000 notes, 13.6 MB (40 MB on disk), 20 areas × 10 topics (220 folders, 50
notes each). Each note has a heading, 1–3 tags out of 170 plus a nested `#areaN/topicM` tag,
2–6 sections with sentences, lists, tasks and quotes, a wiki link to a random note per section,
a fenced code block holding a `#tag` and a `[[link]]` that must be ignored, and, one time in
three, a relative link to a note of its folder.

```sh
./dev.sh scripts/measure-folder.sh          # 10,000 notes, 3 launches; or: <notes> <launches>
./dev.sh env SORT=modified scripts/measure-folder.sh   # sidebar by Date Modified (or created)
```

Each launch reopens the folder with every subfolder expanded and `Area 0/Topic 0/Note 0.md` in
the editor, then shows the Outline (Backlinks under it, Alt+Ctrl+O), switches to another note
through a `markdownmate://open` link, and appends a tag to a third note on disk (the Tags list
is indexed again, Backlinks listed again). The times come from the app itself: with `MARKDOWNMATE_TIMING` set it
prints `markdownmate-timing: <ms since main started> <what>` lines to stderr (`src/timing.rs`):
*window shown* (first frame), *note opened*, *folder scanned* (the background folder walk; its
duration runs from the start of the scan until the app takes the result), *sidebar filled*
(the sidebar's list is built), *sidebar drawn* (first frame showing it), *tags indexed*, *links
indexed* and *backlinks listed* (background jobs, with their own duration). Memory is the resident size (`VmRSS`) of the app and of its WebKit processes. The
script runs on Xvfb inside the container and removes the folder, the session and every process
it started when it ends, also on failure. It takes about a minute.

### Results

Apple Silicon, 12 cores, Docker (OrbStack), release build, 2026-10-25. Before: one launch; after:
three (the first launch of each is the slowest, files not yet cached).

| Step | Before | After |
|------|-------:|------:|
| window shown (after main started) | 1.47 s | 0.34–1.03 s |
| folder scanned (background) | 0.43 s | 0.09–0.31 s |
| sidebar filled, 10,000 notes, all folders open | 1.90 s | 0.42–1.35 s |
| tags indexed (background, every note read) | 0.51 s | 0.17–0.26 s |
| **Backlinks of a note** (background, every note read) | **94.6 s** | **0.28–0.40 s** |
| Backlinks after switching note | over 120 s | 0.22–0.37 s |
| tags indexed again after a note changed on disk | (not reached) | 0.12–0.17 s |
| memory after launch: app + WebKit | 339 + 44 MB | 300–343 + 45 MB |
| memory after the outline, a note switch and a change | — | 304–341 + 37–45 MB |

Link index (same machine and folder, 2026-10-25, three launches; before = the table's "After"):

| Step | Before | After |
|------|-------:|------:|
| links indexed at folder open (background, every note read once) | — | 0.24–0.26 s |
| **Backlinks of a note** (Outline shown after launch) | **0.28–0.40 s** | **1–2 ms** |
| Backlinks after switching note | 0.22–0.37 s | 1 ms |
| Backlinks again after another note changed on disk | (not refreshed) | 1–2 ms |

Open Quickly heading search (2026-11-02, `quick::tests::time_headings`, release build in the
dev container, 10,000 generated notes with 49,916 headings). The link index also keeps each
note's headings, so a `#` query reads no file; it costs time once, in the background index
build at folder open.

| Step | Before | After |
|------|-------:|------:|
| link index built (every note read once) | 0.23–0.25 s | 0.47 s |
| headings copied from the index (first `#` query of a dialog) | — | 6 ms |
| `#n` (50 rows) / `#zzqx` (no match) | — | 7 ms / 2 ms |
| name query `n` / `zzqx`, for comparison | 3 ms / 2 ms | 3 ms / 2 ms |

Sidebar sort orders (same machine and folder, 2026-10-26, two launches each; before = no sort
setting). Dates come from the `stat` the scan already makes for every note, so sorting by them
costs nothing measurable; a note changed on disk moves to its place in under 1 ms.

| Step | Before | Name | Date Modified | Date Created |
|------|-------:|-----:|--------------:|-------------:|
| folder scanned (background) | 52–219 ms | 37–206 ms | 33–215 ms | 66–188 ms |
| sidebar filled (after main started) | 217–672 ms | 213–642 ms | 220–721 ms | 264–570 ms |
| note re-sorted after a change on disk | — | — | 0 ms | 0 ms |
| memory after launch: app + WebKit | 300–343 + 45 MB | 324–368 + 44–45 MB |

Sidebar at launch (same machine and folder, 2026-10-26, three launches, all folders open, Name;
Date Modified / Created two launches each). Before = the build with sort orders; times after
main started.

| Step | Before | After |
|------|-------:|------:|
| folder walk itself, notes + folders to watch (background) | 19–22 ms | 7–8 ms (Date Modified 20 ms) |
| folder scanned | 186–572 ms | 114–127 ms |
| **sidebar filled** | **197–584 ms** | **124–138 ms** (Date Modified / Created 129–137 ms) |
| sidebar drawn (first frame with it) | 208–594 ms | 179–410 ms, = window shown |
| window shown | 161–392 ms | 179–410 ms |

The walk of 10,000 notes took only about 20 ms; the sidebar waited for the main thread. The scan
started only after `window.present()`, which itself takes 70–270 ms (GTK sets up the window and
its GL renderer), and its result then waited until the first frame was drawn, so the sidebar
came one or more frames after the window. The session's folder (or the one given on the command
line) is now scanned on its own thread from the start of building the window; when the window
is ready the scan is done, the note is opened and the sidebar is built before `present()`, so
the first frame already shows it (about 12 ms more before the first frame). A scan that is not
done by then (a slow or cold disk) fills the sidebar when it ends, as before. Under Name the
walk no longer reads each note's metadata: the folder listing already says it is a plain file
(symlinks are still followed, date orders still read the dates). Opening another folder,
rescans and the expanded-folder restore are unchanged.

Launch and the sidebar were already fine: the folder is walked on a background thread and the
sidebar builds plain list items with widgets only for the visible rows. The cost was Backlinks:
every wiki link of every note was resolved by comparing it with every note of the folder
(about 40,000 links × 10,000 notes, each comparison building lower-case strings), so the panel
stayed empty for a minute and a half after each note switch while a core was busy. Wiki links
are now resolved through an index of the notes by name (`render::WikiIndex`): a link only
compares the notes that have its name, with the same result as before (a test checks the index
against the plain search). Link updates after a sidebar rename or move resolve their links the
same way.

Backlinks then still read every note on each note switch. Now the app keeps every note's links
in memory (`backlinks::LinkIndex`): when a folder opens, a background thread reads each note
once and keeps the lines holding links (number, snippet, wiki names, Markdown link files) and a
map from each linked note to the lines linking to it, wiki links resolved through
`render::WikiIndex`. A note saved by the app or changed on disk (the folder watcher's events) is
read again on its own; when the note list changes (a note created, renamed, moved or deleted)
notes new to the list are read and every link is resolved again from memory, since a new or
renamed note can change where `[[Name]]` points. Backlinks of a note are then a lookup. A test
compares the index with the full scan after edits, a rename, a move, a new and a deleted note.

Memory doesn't grow with the folder: with 20 notes instead of 10,000 the app uses the same
(328 MB after launch). Most of it is GTK's GL renderer, which in the container draws with Mesa's
software rasteriser: with `GSK_RENDERER=cairo` (`./dev.sh sh -c 'GSK_RENDERER=cairo
scripts/measure-folder.sh 10000 1'`) the app uses 183 MB with 10,000 notes and 195 MB with 20. Breakdown and
what the app trims: [Memory](#memory).

### Find in Folder with Regex

Searching the whole folder for a query that matches nothing (every note read and searched to the
end; best of 5, files in the page cache, release build, one thread):

| query | time |
|---|---|
| plain `zzqx` | 163 ms |
| Regex `zzqx` | 41 ms |
| Regex `zz\w*qx` | 40 ms |
| Regex `\bzz\d+\b` | 49 ms |

A pattern is faster than plain text because plain text without Match Case lowercases every line
char by char, while the `regex` crate matches case-insensitively without copying. A common word
stops at the first 500 matching lines (under 5 ms either way). Run it with
`./dev.sh sh -c 'cargo run -q --release --example generate_notes -- /tmp/notes 10000 && MM_FIND_NOTES=/tmp/notes cargo test --release find::tests::time_regex -- --ignored --nocapture'`.

### Export Folder as HTML

Exporting the 10,000 generated notes (every note rendered with code highlighting, its links
rewritten and its page written, the folders searched for `assets`, plus `index.html`; release
build, 8 threads on a 12-core machine, files in the page cache): 0.75 s. The window only polls the progress counter, so it
stays responsive, and Cancel stops the threads before their next note. Notes with diagrams or
formulas are drawn afterwards in the off-screen WebView one at a time, like Export as HTML (the
generated folder has none). Run it with
`./dev.sh sh -c 'cargo run -q --release --example generate_notes -- /tmp/notes 10000 && MM_SITE_NOTES=/tmp/notes cargo test --release site::tests::time_export -- --ignored --nocapture'`.

## Memory

Resident size (`VmRSS`, `/proc/<pid>/smaps` summed per mapping) with the folder of 20 notes,
release build, Docker (OrbStack) on Apple Silicon, Xvfb, 2026-10-26. The container has no GPU:
GTK and WebKit draw with OpenGL on Mesa's software rasterizer (llvmpipe), which is most of the
memory and not the app's.

| Launched in Edit | App | of which | WebKit processes |
|------------------|----:|----------|-----------------:|
| default (GTK's `ngl` renderer) | 288–334 MB | heap 123–167 MB, `libLLVM` 57 MB, WebKit libraries 26 MB, Mesa 12 MB, GTK 8 MB | network 45 MB |
| `GSK_RENDERER=gl` | 287 MB | heap 122 MB, `libLLVM` 57 MB | network 45 MB |
| `GSK_RENDERER=cairo` | 186–191 MB | heap 40 MB, `libLLVM` 47 MB | network 45 MB |

heaptrack: the heap peaks at 191 MB with `ngl` and 65 MB with cairo, so about 125 MB of the
app's heap and the 57 MB of LLVM (llvmpipe's shader compiler) are GTK's GL renderer on a
software GPU. With a real GPU the driver's share differs (textures live in video memory).
Forcing cairo for everyone is not measured safe (slower drawing, no GPU scrolling on real
hardware), so the app leaves the renderer to GTK; on a machine without a GPU,
`GSK_RENDERER=cairo markdownmate` saves about 140 MB.

| Launched in Preview | App | WebKit processes |
|---------------------|----:|-----------------:|
| default | 317–320 MB | 336–355 MB (web process: heap 53 MB, other anonymous 92 MB, `libLLVM` 57 MB, WebKit 46 MB) |
| `WEBKIT_DISABLE_COMPOSITING_MODE=1` | 324 MB | web process ≈ 120 MB |

WebKit's accelerated compositing on llvmpipe costs about 150 MB in the web process; also a
renderer cost, left to WebKit (on a GPU it scrolls and zooms the page smoothly).

What the app controls, and does now:

- **The preview's WebView is made the first time the preview is shown** (Ctrl+2 / Ctrl+3, or a
  session reopened in Preview / Side by side), not at launch: before, the hidden WebView
  started WebKit's network process (45 MB) and loaded WebKit in the app even for a session spent
  in Edit.
- **The off-screen page of Print, Export as PDF and Export as HTML is dropped once done**: before,
  it and its web process (a whole renderer, ≈ 120–300 MB) stayed until the next print.
- **WebKit keeps the smallest caches** (`CacheModel::DocumentViewer`): one local page at a time,
  never back or forward.
- Already on demand before: the syntax set (first highlighted code block), KaTeX and Mermaid
  (only installed in pages with a formula / diagram).

Before / after, same container, `scripts/measure-folder.sh` (launched in Edit, 2 launches each,
with `WEBKIT_DISABLE_DMABUF_RENDERER=1`, the app's default):

| Memory after launch, app + WebKit | Before | After |
|-----------------------------------|-------:|------:|
| 20 notes | 279–323 + 45 MB | 266–309 + 0 MB |
| 10,000 notes | 323–362 + 44–45 MB | 309–351 + 0 MB |

About 55–60 MB less in all for a session spent in Edit: the network process (45 MB) and
≈ 13 MB of WebKit in the app. Once the preview is shown both versions use the same (app
≈ 316 MB, WebKit ≈ 263 MB). The first preview is not slower: a note with a code block, a
formula and a Mermaid diagram, reopened in Preview, is drawn 830–850 ms after the mode is set
before and 760–860 ms after; switching from Edit with Ctrl+2 takes 240–1150 ms before
(median ≈ 900 ms, 8 launches) and 270–970 ms after (median ≈ 850 ms): the WebView takes a few
ms to make, the web process starts on the first page in both.

`MARKDOWNMATE_TIMING` also logs `mode <Edit|Preview|Split>`, `preview loaded`, `formulas typeset`
and `diagrams drawn`, to time the first preview of a note.

## Further ideas

- Debounce preview refreshes by note size, so typing in a 5 MB note renders less often.
- Highlight code blocks lazily in the page (plain code first, highlighted as it scrolls into
  view).
