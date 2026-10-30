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
    let _ = render::page("```rust\nx\n```\n", false, Theme::Default, false, &wiki); // load syntaxes
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    eprintln!("threads: {threads}");
    for mb in [1, 5] {
        let md = sample_note(mb * 1_000_000);
        let runs = if mb == 1 { 5 } else { 3 };
        // Opening the note: nothing highlighted yet.
        let cold = render::forget_highlighting;
        let (full, html) = best(runs, cold, || {
            render::page(&md, false, Theme::Default, false, &wiki)
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
            render::page(&md, false, Theme::Default, false, &wiki);
        };
        let (rerender, _) = best(runs, shown, || {
            render::page(&edited, false, Theme::Default, false, &wiki)
        });
        // The preview: the first page rendered per block, then a keystroke patching it.
        let style = Style {
            dark: false,
            theme: Theme::Default,
            width: crate::state::TextWidth::Medium,
            web_images: false,
            hard_breaks: false,
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
            patched == render::page(&edited, false, Theme::Default, false, &wiki),
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
            let edited_html = render::page(&edited, true, Theme::Paper, false, &wiki);
            std::fs::write(dir.join(format!("{mb}mb-edited.html")), edited_html)
                .expect("write page");
        }
    }
}

/// CPU time of the whole process so far (all threads, the renderer's too), from `/proc`.
fn cpu_time() -> Duration {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // After the command name in parentheses: state is field 3, utime / stime fields 14 / 15.
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .map_or("", |(_, rest)| rest)
        .split_whitespace()
        .collect();
    let ticks: u64 = [11, 12]
        .iter()
        .filter_map(|&i| fields.get(i)?.parse::<u64>().ok())
        .sum();
    Duration::from_millis(ticks * 10)
}

/// Typing into a 5 MB note in a shown editor, with Focus Mode off, on, then off again:
/// keystrokes in a paragraph in the middle of the note, each followed by the main loop until
/// the next frame is drawn (wall time per keystroke, and the process's CPU time); also turning
/// Focus Mode on and a cursor jump to another paragraph. Run like `large_notes`
/// (`focus_typing` instead), with a display.
#[test]
#[ignore]
fn focus_typing() {
    use sourceview5::prelude::*;
    if gtk::init().is_err() {
        eprintln!("no display");
        return;
    }
    let md = sample_note(5_000_000);
    let buffer = sourceview5::Buffer::new(None);
    buffer.set_language(
        sourceview5::LanguageManager::default()
            .language("markdown")
            .as_ref(),
    );
    buffer.set_text(&md);
    let focus = crate::focus::Focus::attach(&buffer);
    focus.set_colours(None, false);
    let view = sourceview5::View::with_buffer(&buffer);
    let scroll = gtk::ScrolledWindow::builder().child(&view).build();
    let window = gtk::Window::builder()
        .default_width(900)
        .default_height(600)
        .child(&scroll)
        .build();
    window.present();
    let context = gtk::glib::MainContext::default();
    let painted = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let settle = |budget: Duration| {
        let start = Instant::now();
        while context.pending() && start.elapsed() < budget {
            context.iteration(false);
        }
    };
    // Shown and highlighted, the first layout done.
    let start = Instant::now();
    while window.frame_clock().is_none() && start.elapsed() < Duration::from_secs(10) {
        context.iteration(true);
    }
    let clock = window.frame_clock().expect("shown");
    let p = std::rc::Rc::clone(&painted);
    clock.connect_after_paint(move |_| p.set(p.get() + 1));
    let middle = md[..md.len() / 2].rfind("\nParagraph ").expect("paragraph") + 11;
    let place = |offset: usize| {
        let at = buffer.iter_at_offset(md[..offset].chars().count() as i32);
        buffer.place_cursor(&at);
        view.scroll_to_iter(&mut at.clone(), 0.0, true, 0.0, 0.5);
    };
    let drawn = || {
        let frame = painted.get();
        let start = Instant::now();
        while painted.get() == frame && start.elapsed() < Duration::from_secs(5) {
            context.iteration(true);
        }
    };
    place(middle);
    settle(Duration::from_secs(120));
    drawn();
    let keys = 300;
    let typing = |label: &str| {
        let (start, cpu) = (Instant::now(), cpu_time());
        let mut worst = Duration::ZERO;
        for _ in 0..keys {
            let key = Instant::now();
            buffer.insert_at_cursor("x");
            drawn();
            worst = worst.max(key.elapsed());
        }
        let (took, cpu) = (start.elapsed(), cpu_time() - cpu);
        eprintln!(
            "{label}: {keys} keystrokes, {:.1} ms each to the next frame (worst {}), \
CPU {:.2} ms per keystroke",
            took.as_secs_f64() * 1000.0 / keys as f64,
            ms(worst),
            cpu.as_secs_f64() * 1000.0 / keys as f64
        );
    };
    typing("focus mode off");
    let start = Instant::now();
    focus.set_on(true);
    focus.update();
    eprintln!("turn focus mode on: {}", ms(start.elapsed()));
    drawn();
    typing("focus mode on");
    let start = Instant::now();
    place(md[..middle].rfind("\nParagraph ").expect("paragraph") + 11);
    focus.update();
    eprintln!("cursor to another paragraph: {}", ms(start.elapsed()));
    drawn();
    focus.set_on(false);
    drawn();
    typing("focus mode off again");
    window.destroy();
}
