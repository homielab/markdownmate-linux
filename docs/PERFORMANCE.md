# Performance with large notes

`src/perf.rs` has an ignored test that times what makes a multi-megabyte note slow:

- **open in editor**: loading the text into the GtkSourceView buffer and highlighting all of
  its Markdown (the editor does the visible part first and the rest in idle time);
- **full render**: the preview page (`render::page`) of a note seen for the first time;
- **without code highlighting**: the same body with syntect turned off, to show its share;
- **re-render after a keystroke**: the page again after one character typed into a code
  block in the middle of the note (the preview renders the whole note again and swaps the
  page in place).

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

## Further ideas

- Re-render only the top-level blocks around the edit and patch the page's DOM instead of
  replacing the whole body (the 5 MB page is 73 MB of HTML, most of it highlighting spans).
- Debounce preview refreshes by note size, so typing in a 5 MB note renders less often.
- Highlight code blocks lazily in the page (plain code first, highlighted as it scrolls into
  view).
