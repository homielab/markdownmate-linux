//! Timing of large notes (1 MB and 5 MB): opening one in the editor (buffer + Markdown
//! highlighting), the preview's full render, the re-render after a keystroke, and the same two
//! as the preview does them per top-level block (`incremental`).
//! Ignored by default; how to run it: `docs/PERFORMANCE.md`.

use std::time::{Duration, Instant};

use crate::incremental::{self, Style, Update};
use crate::render;
use crate::state::Theme;

/// A deterministic note of at least `size` bytes: sections with headings, inline styling,
/// web / note / wiki links, tags, task and nested lists, a table, a quote, math and fenced
/// code in several languages (and one without a language).
pub(crate) fn sample_note(size: usize) -> String {
    let mut md = String::from("# Large note\n\n");
    let mut i = 0;
    while md.len() < size {
        i += 1;
        md += &format!(
            "## Section {i} #topic/{tag}\n\n\
Paragraph {i} with *emphasis*, **bold**, ~~struck~~, `inline code`, a [web link](https://example.com/{i}), \
a [note link](Notes/Note%20{i}.md), [[Wiki {i}]] and [[Other#Heading|alias]]. Inline math $x_{i}^2 + \\sqrt{{y}}$.\n\n\
- item one #tag{tag}\n- [ ] open task {i}\n- [x] done task\n  1. nested\n  2. nested again\n\n\
| Name | Value | Note |\n|:-----|------:|:----:|\n| a{i} | {i} | first |\n| b{i} | {j} | second |\n\n\
> A quote about section {i}.\n\n\
```rust\nfn section_{i}() -> Vec<u32> {{\n    let v: Vec<u32> = (0..{i}).map(|x| x * 2).collect();\n    println!(\"{{v:?}}\");\n    v\n}}\n```\n\n\
```python\ndef f{i}(x):\n    \"\"\"Squares of odd numbers.\"\"\"\n    return [n ** 2 for n in range(x) if n % 2]\n```\n\n\
```javascript\nconst f{i} = (a, b) => a.map(x => x + b).filter(Boolean);\n```\n\n\
```\nplain block {i}\n```\n\n\
$$\n\\sum_{{k=1}}^{{{i}}} k = \\frac{{n(n+1)}}{{2}}\n$$\n\n",
            tag = i % 7,
            j = i * 3,
        );
    }
    md
}

/// Fastest of `runs` runs of `f`, each after `setup` (untimed); the fastest is the least
/// disturbed by other work on the machine.
fn best<T>(runs: usize, mut setup: impl FnMut(), mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut best = None;
    let mut out = None;
    for _ in 0..runs {
        setup();
        let start = Instant::now();
        let value = f();
        let took = start.elapsed();
        if best.is_none_or(|b| took < b) {
            best = Some(took);
        }
        out = Some(value);
    }
    (best.expect("runs > 0"), out.expect("runs > 0"))
}

fn ms(d: Duration) -> String {
    format!("{:.0} ms", d.as_secs_f64() * 1000.0)
}

/// Loads `md` into a GtkSourceView buffer with the Markdown language and highlights all of
/// it, as opening the note does (the view highlights the visible part first, the rest in
/// idle time). None without a display.
fn buffer_load(md: &str) -> Option<Duration> {
    use sourceview5::prelude::*;
    gtk::init().ok()?;
    let language = sourceview5::LanguageManager::default().language("markdown");
    let (took, _) = best(
        3,
        || {},
        || {
            let buffer = sourceview5::Buffer::new(None);
            buffer.set_language(language.as_ref());
            buffer.set_text(md);
            let (start, end) = buffer.bounds();
            buffer.ensure_highlight(&start, &end);
            buffer
        },
    );
    Some(took)
}

/// `xvfb-run -a cargo test --release large_notes -- --ignored --nocapture --test-threads=1`
/// (Xvfb for the editor's timing); with `MM_PERF_OUT=<dir>` it also writes each note's
/// preview page there, to compare the HTML of two builds byte for byte.
#[test]
#[ignore]
fn large_notes() {
    let wiki = |t: &str| {
        t.len()
            .is_multiple_of(2)
            .then(|| std::path::PathBuf::from(format!("/n/{t}.md")))
    };
    let _ = render::page("```rust\nx\n```\n", false, Theme::Default, &wiki); // load syntaxes
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    eprintln!("threads: {threads}");
    for mb in [1, 5] {
        let md = sample_note(mb * 1_000_000);
        let runs = if mb == 1 { 5 } else { 3 };
        // Opening the note: nothing highlighted yet.
        let cold = render::forget_highlighting;
        let (full, html) = best(runs, cold, || {
            render::page(&md, false, Theme::Default, &wiki)
        });
        let (plain, _) = best(runs, cold, || {
            render::body_html(&md, &wiki, false, true, true)
        });
        // A keystroke in a code block in the middle of the note, after the note's preview:
        // the whole note is rendered again.
        let mut edited = md.clone();
        let mid = md[..md.len() / 2].rfind("fn section_").expect("code") + 3;
        edited.insert(mid, 'x');
        let shown = || {
            render::page(&md, false, Theme::Default, &wiki);
        };
        let (rerender, _) = best(runs, shown, || {
            render::page(&edited, false, Theme::Default, &wiki)
        });
        // The preview: the first page rendered per block, then a keystroke patching it.
        let style = Style {
            dark: false,
            theme: Theme::Default,
            width: crate::state::TextWidth::Full,
            links: 0,
        };
        let (blocks, _) = best(runs, cold, || incremental::render(None, &md, style, &wiki));
        let state = std::cell::Cell::new(None);
        let (patch, (after, update)) = best(
            runs,
            || state.set(incremental::render(None, &md, style, &wiki).0),
            || incremental::render(state.take(), &edited, style, &wiki),
        );
        let Update::Patch(patch_out) = update else {
            panic!("a keystroke in a code block is a patch");
        };
        let patched = after.expect("blocks").page();
        assert!(
            patched == render::page(&edited, false, Theme::Default, &wiki),
            "the patched page is the whole render"
        );
        let sent: usize = patch_out.edits.iter().map(|e| e.html.len()).sum();
        let load = buffer_load(&md).map_or_else(|| "no display".into(), ms);
        eprintln!(
            "{mb} MB ({} KB, {} lines): open in editor {load}; full render {} \
(without code highlighting {}); re-render after a keystroke {}; page {} KB; \
per block: first render {}, update after the keystroke {} (patch {} KB)",
            md.len() / 1024,
            md.lines().count(),
            ms(full),
            ms(plain),
            ms(rerender),
            html.len() / 1024,
            ms(blocks),
            ms(patch),
            sent.div_ceil(1024),
        );
        if let Some(dir) = std::env::var_os("MM_PERF_OUT") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).expect("output dir");
            std::fs::write(dir.join(format!("{mb}mb.html")), &html).expect("write page");
            render::forget_highlighting();
            let edited_html = render::page(&edited, true, Theme::Paper, &wiki);
            std::fs::write(dir.join(format!("{mb}mb-edited.html")), edited_html)
                .expect("write page");
        }
    }
}
