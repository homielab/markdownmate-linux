//! The preview of a note rendered per top-level block, so an edit re-renders only the blocks
//! it touched and the page patches only their DOM nodes (pure Rust, no GTK).
//!
//! The note is cut between top-level blocks where a blank line separates them and nothing
//! before can continue into what follows (no open list, fence, indented code or HTML block):
//! each part ("chunk") then renders standalone (`render::render_body` with its first line) to
//! exactly its part of the whole note's HTML. Heading ids are the exception: a repeated title
//! gets `-1`, `-2`, … counted from the note's start, so they are set again over the whole note
//! after each update (`Preview::set_ids`). So is a `[TOC]` block, which lists the headings of
//! the whole note: it is filled again whenever that list changes.
//!
//! After an edit, the chunks the changed lines touch are parsed again with one chunk of context
//! on each side; if the cut before or after them no longer holds (a fence or list now runs on),
//! the range grows until both hold. Its new chunks replace the old ones; later chunks only
//! move by the number of lines added or removed (their HTML is shifted when it is needed again;
//! the page shifts its own nodes' line numbers, `Patch::delta`).
//!
//! Notes this cannot cut safely render whole, as before (`render::page`): footnotes (numbered
//! over the whole note, listed at its end), link reference definitions (used anywhere), a lone
//! carriage return as a line break, or the private-use chars `render` marks its AST with. So
//! does an edit that may move the end of front matter.
//! The page built from the chunks is identical to `render::page` (tests below and
//! `perf::large_notes`).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::render;
use crate::state::{TextWidth, Theme};

/// Resolves a wiki link's target (`render::resolve_wiki`); shared by the threads of a render.
pub(crate) type Wiki<'a> = dyn Fn(&str) -> Option<PathBuf> + Sync + 'a;

/// What a rendered chunk depends on besides its text: light/dark, the theme, the text width,
/// whether web images load (the page's CSP), whether every newline is a line break, and the notes wiki links resolve against (a fingerprint of the list, root and current note).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Style {
    pub dark: bool,
    pub theme: Theme,
    pub width: TextWidth,
    pub web_images: bool,
    pub hard_breaks: bool,
    pub links: u64,
}

/// A rendered chunk of the note.
struct Chunk {
    /// Its first line (1-based) and that line's byte offset in the note.
    line: usize,
    byte: usize,
    /// The first line `html` was rendered (or last shifted) for; differs from `line` after
    /// lines were added or removed above it.
    at: usize,
    html: String,
    /// `render::Rendered::headings`, and the ids `html` gives those headings.
    headings: Vec<String>,
    ids: Vec<String>,
    /// `render::Rendered::titles`.
    titles: Vec<(u8, String)>,
    /// With a `[TOC]` block: `html` before `render::fill_toc` (same lines and ids as `html`).
    template: Option<String>,
    /// The top-level DOM nodes `html` makes in the page's body.
    nodes: usize,
}

/// The note shown in the preview, as chunks.
pub(crate) struct Preview {
    style: Style,
    text: String,
    chunks: Vec<Chunk>,
    /// The chunks the last update replaced (`Key`):
    /// typing a fence and closing it again gets the code after it back without rendering.
    spare: HashMap<Key, Chunk>,
    /// The table of contents the `[TOC]` blocks show (`render::toc_list`).
    toc: String,
}

/// A chunk by what its HTML depends on: whether it is the note's first chunk, the line after
/// it (none at the note's end), and its text.
type Key = (bool, Option<String>, String);

/// How the shown page changes.
pub(crate) enum Update {
    /// A whole new page (first render, another style, or a note that cannot be cut).
    Page(String),
    /// Some of the body's nodes replaced.
    Patch(Patch),
}

/// Changes to the body of a page showing the previous render: first every top-level node from
/// index `shift` on moves by `delta` source lines (`data-sourcepos`, task and Copy links),
/// then each edit (indices in the old body, ascending, not overlapping) replaces `remove`
/// nodes from `at` with `html`. The body has `nodes` nodes before and `after` after.
#[derive(Debug, Default)]
pub(crate) struct Patch {
    pub nodes: usize,
    pub after: usize,
    pub shift: usize,
    pub delta: isize,
    pub edits: Vec<Edit>,
}

#[derive(Debug)]
pub(crate) struct Edit {
    pub at: usize,
    pub remove: usize,
    pub html: String,
}

impl Patch {
    /// Whether the new nodes may hold formulas or diagrams to draw.
    pub(crate) fn has_math(&self) -> bool {
        self.edits.iter().any(|e| render::has_math(&e.html))
    }

    pub(crate) fn has_diagrams(&self) -> bool {
        self.edits.iter().any(|e| render::has_diagrams(&e.html))
    }
}

/// More chunks than this whose heading ids changed (a repeated title added near the top of a
/// note with many of them): the whole page is swapped instead.
const MAX_ID_EDITS: usize = 200;

/// Renders `md` for the preview, building on `previous` (the render the page shows) when it
/// can. Returns the state for the next render (None when `md` renders whole) and the change.
pub(crate) fn render(
    previous: Option<Preview>,
    md: &str,
    style: Style,
    wiki: &Wiki,
) -> (Option<Preview>, Update) {
    if let Some(previous) = previous.filter(|p| p.style == style)
        && cuttable(md)
        && let Some(done) = previous.update(md, wiki)
    {
        return done;
    }
    match Preview::new(md, style, wiki) {
        Some(preview) => {
            let page = preview.page();
            (Some(preview), Update::Page(page))
        }
        None => (
            None,
            Update::Page(render::preview_page(
                style.dark,
                style.theme,
                Some(style.width),
                style.web_images,
                &render::body_html(md, wiki, true, true, true, true),
            )),
        ),
    }
}

/// Whether `md` may be cut into chunks (module doc).
fn cuttable(md: &str) -> bool {
    if md.contains("[^") || md.chars().any(render::is_marker) {
        return false;
    }
    let bytes = md.as_bytes();
    if bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'\r' && bytes.get(i + 1) != Some(&b'\n'))
    {
        return false;
    }
    // `[label]: destination`, anywhere (in a list or quote, after a line break inside the
    // label): any `]:` but the end of a wiki link (`[[Note]]: text`), whose label would hold
    // brackets.
    let mut rest = md;
    while let Some(i) = rest.find("]:") {
        let before = &rest[..i];
        if !before.ends_with(']') || before.ends_with("\\]") {
            return false;
        }
        rest = &rest[i + 2..];
    }
    true
}

/// A top-level block of a parse: its first and last line (1-based, in the note) and whether
/// it is raw HTML (shown escaped, as text: no element starts its chunk).
struct Block {
    start: usize,
    end: usize,
    html: bool,
}

/// The top-level blocks of `text`, which starts at line `first_line` of the note.
fn blocks(text: &str, first_line: usize) -> Vec<Block> {
    use comrak::nodes::NodeValue;
    let mut options = render::options();
    if first_line > 1 {
        options.extension.front_matter_delimiter = None;
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, text, &options);
    root.children()
        .map(|node| {
            let data = node.data();
            let (mut start, mut end) = (data.sourcepos.start.line, data.sourcepos.end.line);
            if let NodeValue::FrontMatter(front) = &data.value {
                // Its delimiters and the blank lines after it.
                start = 1;
                end = end.max(front.matches('\n').count());
            }
            Block {
                start: start + first_line - 1,
                end: end.max(start) + first_line - 1,
                html: matches!(data.value, NodeValue::HtmlBlock(_)),
            }
        })
        .collect()
}

/// Byte offsets of the lines of `text` (the first is 0).
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// Lines of a part of the note: `text` starting at line `first_line`.
struct Lines<'a> {
    text: &'a str,
    first_line: usize,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str, first_line: usize) -> Self {
        Lines {
            text,
            first_line,
            starts: line_starts(text),
        }
    }

    /// Byte offset in `text` of 1-based note line `line` (the end after the last line).
    fn byte(&self, line: usize) -> usize {
        self.starts
            .get(line - self.first_line)
            .copied()
            .unwrap_or(self.text.len())
    }

    fn is_blank(&self, line: usize) -> bool {
        let start = self.byte(line);
        let end = self.byte(line + 1);
        self.text[start..end].trim().is_empty()
    }

    /// Whether the note may be cut before `line`: the line before is blank and no block runs
    /// across it.
    fn cut_holds(&self, blocks: &[Block], line: usize) -> bool {
        line > self.first_line
            && self.is_blank(line - 1)
            && !blocks.iter().any(|b| b.start < line && b.end >= line)
    }

    /// The lines from `from` to `to` (exclusive) cut into chunks: their first lines, `from`
    /// first. A cut goes before a block after a blank line that is not raw HTML.
    fn cuts(&self, blocks: &[Block], from: usize, to: usize) -> Vec<usize> {
        let mut cuts = vec![from];
        for pair in blocks.windows(2) {
            let (before, block) = (&pair[0], &pair[1]);
            if block.start > from
                && block.start < to
                && !block.html
                && block.start > before.end
                && self.is_blank(block.start - 1)
            {
                cuts.push(block.start);
            }
        }
        cuts
    }
}

/// A chunk to render: its text, first line and byte offset in the note, and the note's line
/// after it (None at the note's end).
struct Part<'a> {
    text: &'a str,
    line: usize,
    byte: usize,
    next: Option<&'a str>,
}

impl Part<'_> {
    fn key(&self) -> Key {
        (
            self.line == 1,
            self.next.map(str::to_string),
            self.text.to_string(),
        )
    }
}

/// Renders `parts`, taking those `spare` has (shifted to their line); on all cores when there
/// are many. None if a chunk's HTML could not be cut into nodes (`top_nodes`).
fn render_parts(
    parts: &[Part],
    wiki: &Wiki,
    spare: &mut HashMap<Key, Chunk>,
) -> Option<Vec<Chunk>> {
    let mut done: Vec<Option<Chunk>> = parts
        .iter()
        .map(|p| {
            let mut chunk = spare.remove(&p.key())?;
            chunk.line = p.line;
            chunk.byte = p.byte;
            Some(chunk)
        })
        .collect();
    let todo: Vec<usize> = (0..parts.len()).filter(|&i| done[i].is_none()).collect();
    let one = |i: usize| render_chunk(&parts[i], wiki);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(todo.len() / 8)
        .max(1);
    if threads == 1 {
        for &i in &todo {
            done[i] = Some(one(i)?);
        }
    } else {
        let todo = &todo;
        let results: Vec<Vec<(usize, Option<Chunk>)>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    scope.spawn(move || {
                        todo.iter()
                            .skip(t)
                            .step_by(threads)
                            .map(|&i| (i, one(i)))
                            .collect()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("render thread"))
                .collect()
        });
        for (i, chunk) in results.into_iter().flatten() {
            done[i] = Some(chunk?);
        }
    }
    done.into_iter().collect()
}

fn render_chunk(part: &Part, wiki: &Wiki) -> Option<Chunk> {
    let body = render::Body {
        rich: true,
        tasks: true,
        sourcepos: true,
        toc: true,
        first_line: part.line,
        cached: false,
        next: part.next.is_some(),
    };
    // Where the blocks at the chunk's end end depends on what closes them (the next block's
    // first line, or the note's end): parsed with that line, as in the whole note.
    let text = match part.next {
        Some(next) => std::borrow::Cow::Owned(format!("{}{next}", part.text)),
        None => part.text.into(),
    };
    let rendered = render::render_body(&text, wiki, &body)?;
    // A later chunk's first node must be an element: text would merge with the text node
    // ending the chunk before it.
    if part.line > 1 && !rendered.html.is_empty() && !rendered.html.starts_with('<') {
        return None;
    }
    let nodes = top_nodes(&rendered.html)?;
    let ids = heading_ids(&rendered.html);
    if ids.len() != rendered.headings.len() {
        return None;
    }
    Some(Chunk {
        line: part.line,
        byte: part.byte,
        at: part.line,
        template: rendered.toc.then(|| rendered.html.clone()),
        html: rendered.html,
        headings: rendered.headings,
        ids,
        titles: rendered.titles,
        nodes,
    })
}

impl Chunk {
    /// Moves its HTML to its line (`shift`).
    fn shift_to_line(&mut self) {
        let delta = self.line as isize - self.at as isize;
        if delta == 0 {
            return;
        }
        self.html = shift(&self.html, delta).into_owned();
        if let Some(template) = &mut self.template {
            *template = shift(template, delta).into_owned();
        }
        self.at = self.line;
    }
}

impl Preview {
    /// `md` rendered as chunks; None if it cannot be cut (`cuttable`) or a chunk's HTML is not
    /// as expected.
    fn new(md: &str, style: Style, wiki: &Wiki) -> Option<Self> {
        if !cuttable(md) {
            return None;
        }
        let lines = Lines::new(md, 1);
        let blocks = blocks(md, 1);
        let end = lines.starts.len() + 1;
        let cuts = lines.cuts(&blocks, 1, end);
        let parts = parts(&lines, &cuts, end, 0, true);
        let chunks = render_parts(&parts, wiki, &mut HashMap::new())?;
        let mut preview = Preview {
            style,
            text: md.to_string(),
            chunks,
            spare: HashMap::new(),
            toc: String::new(),
        };
        preview.set_ids(0..preview.chunks.len())?;
        Some(preview)
    }

    /// The whole page (as `render::page` of the text).
    pub(crate) fn page(&self) -> String {
        let mut body = String::with_capacity(self.chunks.iter().map(|c| c.html.len()).sum());
        for chunk in &self.chunks {
            body.push_str(&shift(&chunk.html, chunk.line as isize - chunk.at as isize));
        }
        render::preview_page(
            self.style.dark,
            self.style.theme,
            Some(self.style.width),
            self.style.web_images,
            &body,
        )
    }

    /// The index of the chunk holding 1-based `line`.
    fn chunk_at(&self, line: usize) -> usize {
        self.chunks.partition_point(|c| c.line <= line).max(1) - 1
    }

    /// Updates the chunks to `md` (`cuttable`), or None when it is better rendered whole.
    fn update(mut self, md: &str, wiki: &Wiki) -> Option<(Option<Preview>, Update)> {
        let old = std::mem::take(&mut self.text);
        if old == md {
            self.text = old;
            let nodes = self.chunks.iter().map(|c| c.nodes).sum();
            let patch = Patch {
                nodes,
                after: nodes,
                shift: nodes,
                ..Patch::default()
            };
            return Some((Some(self), Update::Patch(patch)));
        }
        // The changed bytes: `old[prefix..old.len() - suffix]` became
        // `md[prefix..md.len() - suffix]`.
        let (a, b) = (old.as_bytes(), md.as_bytes());
        let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
        let max_suffix = a.len().min(b.len()) - prefix;
        let suffix = a
            .iter()
            .rev()
            .zip(b.iter().rev())
            .take(max_suffix)
            .take_while(|(x, y)| x == y)
            .count();
        let (old_end, new_end) = (a.len() - suffix, b.len() - suffix);
        let newlines = |s: &[u8]| s.iter().filter(|&&c| c == b'\n').count() as isize;
        let delta = newlines(&b[prefix..new_end]) - newlines(&a[prefix..old_end]);
        let byte_delta = b.len() as isize - a.len() as isize;
        // The (old) lines the change touches, through the chunks holding them.
        let line_of = |pos: usize| {
            let chunk = &self.chunks[self.chunks.partition_point(|c| c.byte <= pos).max(1) - 1];
            chunk.line + newlines(&a[chunk.byte..pos]) as usize
        };
        // Through the line of the first unchanged byte: a change ending at a line start may
        // have joined that line to the one before.
        let (first, last) = (line_of(prefix), line_of(old_end));
        let (mut lo, mut hi) = (self.chunk_at(first), self.chunk_at(last));
        // A chunk was parsed with the line after it (`Part::next`).
        if lo > 0 && first == self.chunks[lo].line {
            lo -= 1;
        }
        // Front matter runs from a first line `---` to the next such line anywhere in the note:
        // a change to the first chunk or to a line with `---` renders whole.
        let front = |t: &str| t.trim_start_matches('\u{feff}').starts_with("---");
        if front(&old) || front(md) {
            let touches = |t: &str, from: usize, to: usize| {
                let start = t.as_bytes()[..from]
                    .iter()
                    .rposition(|&c| c == b'\n')
                    .map_or(0, |i| i + 1);
                let end = t.as_bytes()[to..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(t.len(), |i| to + i);
                t[start..end].contains("---")
            };
            if lo == 0 || touches(&old, prefix, old_end) || touches(md, prefix, new_end) {
                return None;
            }
        }
        let count = self.chunks.len();
        // Where chunk `i` starts in `md`: chunks after the change moved.
        let changed_to = hi;
        let moved = |c: &Chunk, i: usize| {
            if i > changed_to {
                (
                    (c.line as isize + delta) as usize,
                    (c.byte as isize + byte_delta) as usize,
                )
            } else {
                (c.line, c.byte)
            }
        };
        // Grow [lo, hi] until the cuts around it hold in `md`.
        let mut step = 1;
        let (to_line, region) = loop {
            let before = lo.saturating_sub(1);
            let after = (hi + 2).min(count); // first chunk past the parsed text
            let (start_line, start_byte) = (self.chunks[before].line, self.chunks[before].byte);
            let end_byte = match self.chunks.get(after) {
                Some(c) => moved(c, after).1,
                None => md.len(),
            };
            let text = &md[start_byte..end_byte];
            let lines = Lines::new(text, start_line);
            let parsed = blocks(text, start_line);
            let from_line = self.chunks[lo].line;
            let to_line = self.chunks.get(hi + 1).map(|c| moved(c, hi + 1).0);
            let lo_holds = lo == 0 || lines.cut_holds(&parsed, from_line);
            let hi_holds = to_line.is_none_or(|l| lines.cut_holds(&parsed, l));
            if lo_holds && hi_holds {
                let end = to_line.unwrap_or(start_line + lines.starts.len());
                let cuts = lines.cuts(&parsed, from_line, end);
                break (end, (start_byte, text.to_string(), cuts));
            }
            if !lo_holds {
                lo = lo.saturating_sub(step);
            }
            if !hi_holds {
                hi = (hi + step).min(count - 1);
            }
            step *= 2;
        };
        let (start_byte, text, cuts) = region;
        let lines = Lines::new(&text, self.chunks[lo.saturating_sub(1)].line);
        let at_end = hi + 1 == count;
        let parts = parts(&lines, &cuts, to_line, start_byte, at_end);
        // The nodes the replaced chunks had in the page.
        let node_at: Vec<usize> = std::iter::once(0)
            .chain(self.chunks.iter().scan(0, |n, c| {
                *n += c.nodes;
                Some(*n)
            }))
            .collect();
        let nodes = node_at[count];
        // The replaced chunks, by text, for this update and the next.
        let mut spare = std::mem::take(&mut self.spare);
        let keys: Vec<Key> = (lo..=hi)
            .map(|i| {
                let end = self.chunks.get(i + 1).map_or(old.len(), |c| c.byte);
                let next = self.chunks.get(i + 1).map(|c| {
                    let line_end = old[end..].find('\n').map_or(old.len(), |n| end + n + 1);
                    old[c.byte..line_end].to_string()
                });
                (i == 0, next, old[self.chunks[i].byte..end].to_string())
            })
            .collect();
        for (key, chunk) in keys.iter().cloned().zip(self.chunks.drain(lo..=hi)) {
            spare.insert(key, chunk);
        }
        let mut fresh = render_parts(&parts, wiki, &mut spare)?;
        for chunk in &mut fresh {
            chunk.shift_to_line();
        }
        self.spare = keys
            .into_iter()
            .filter_map(|k| spare.remove_entry(&k))
            .collect();
        let added = fresh.len();
        for chunk in &mut self.chunks[lo..] {
            chunk.line = (chunk.line as isize + delta) as usize;
            chunk.byte = (chunk.byte as isize + byte_delta) as usize;
        }
        self.chunks.splice(lo..lo, fresh);
        self.text = md.to_string();
        let changed = self.set_ids(lo..lo + added)?;
        if changed.len() > MAX_ID_EDITS {
            let page = self.page();
            return Some((Some(self), Update::Page(page)));
        }
        let mut edits = vec![Edit {
            at: node_at[lo],
            remove: node_at[hi + 1] - node_at[lo],
            html: self.chunks[lo..lo + added]
                .iter()
                .map(|c| c.html.as_str())
                .collect(),
        }];
        // Another chunk whose heading ids or table of contents changed: rendered for its line
        // now. `changed` is ascending and skips the new chunks, so the edits stay in order.
        for i in changed {
            let old_index = if i < lo { i } else { i - added + (hi + 1 - lo) };
            let chunk = &mut self.chunks[i];
            chunk.shift_to_line();
            let edit = Edit {
                at: node_at[old_index],
                remove: node_at[old_index + 1] - node_at[old_index],
                html: chunk.html.clone(),
            };
            if i < lo {
                edits.insert(edits.len() - 1, edit);
            } else {
                edits.push(edit);
            }
        }
        let after = self.chunks.iter().map(|c| c.nodes).sum();
        let patch = Patch {
            nodes,
            after,
            shift: node_at[hi + 1],
            delta,
            edits,
        };
        Some((Some(self), Update::Patch(patch)))
    }

    /// Sets every heading id as a render of the whole note gives it, and fills the `[TOC]`
    /// blocks with its headings; returns, in order, the chunks outside `fresh` whose HTML
    /// changed (those in `fresh` are new: their HTML is sent anyway). None if a filled chunk's
    /// HTML cannot be cut into nodes.
    fn set_ids(&mut self, fresh: std::ops::Range<usize>) -> Option<Vec<usize>> {
        let mut anchorizer = comrak::Anchorizer::new();
        let mut changed = Vec::new();
        for (i, chunk) in self.chunks.iter_mut().enumerate() {
            let ids: Vec<String> = chunk
                .headings
                .iter()
                .map(|h| anchorizer.anchorize(h))
                .collect();
            if ids != chunk.ids {
                chunk.html = replace_ids(&chunk.html, &ids);
                if let Some(template) = &mut chunk.template {
                    *template = replace_ids(template, &ids);
                }
                chunk.ids = ids;
                if !fresh.contains(&i) {
                    changed.push(i);
                }
            }
        }
        if self.chunks.iter().all(|c| c.template.is_none()) {
            return Some(changed);
        }
        let toc = render::toc_list(self.chunks.iter().flat_map(|c| {
            c.titles
                .iter()
                .zip(&c.ids)
                .map(|((level, title), id)| (*level, title.as_str(), id.as_str()))
        }));
        let all = toc != self.toc;
        self.toc = toc;
        for (i, chunk) in self.chunks.iter_mut().enumerate() {
            let Some(template) = &chunk.template else {
                continue;
            };
            let touched = changed.contains(&i);
            if all || touched || fresh.contains(&i) {
                chunk.html = render::fill_toc(template, &self.toc);
                chunk.nodes = top_nodes(&chunk.html)?;
                if !touched && !fresh.contains(&i) {
                    changed.push(i);
                }
            }
        }
        changed.sort_unstable();
        Some(changed)
    }
}

/// The chunks of `lines` cut at `cuts` up to line `end` (exclusive); `base` is the byte
/// offset of `lines` in the note; `at_end`: `end` is the note's end.
fn parts<'a>(
    lines: &Lines<'a>,
    cuts: &[usize],
    end: usize,
    base: usize,
    at_end: bool,
) -> Vec<Part<'a>> {
    cuts.iter()
        .enumerate()
        .map(|(i, &line)| {
            let next = cuts.get(i + 1).copied().unwrap_or(end);
            let (start, stop) = (lines.byte(line), lines.byte(next));
            Part {
                text: &lines.text[start..stop],
                line,
                byte: base + start,
                next: (!at_end || i + 1 < cuts.len())
                    .then(|| &lines.text[stop..lines.byte(next + 1)]),
            }
        })
        .collect()
}

/// `html` with its source lines moved by `delta`: `data-sourcepos` and the task and Copy
/// links (`render`), as the page's patch script does to its nodes.
fn shift(html: &str, delta: isize) -> std::borrow::Cow<'_, str> {
    if delta == 0 {
        return html.into();
    }
    let add = |n: &str| (n.parse::<isize>().expect("line number") + delta).to_string();
    let mut out = String::with_capacity(html.len() + html.len() / 64);
    let mut rest = html;
    const POS: &str = "data-sourcepos=\"";
    const TASK: &str = "<a class=\"task\" href=\"mm-task:";
    const COPY: &str = "<a class=\"copy\" href=\"mm-copy:";
    loop {
        let next = [POS, TASK, COPY]
            .iter()
            .filter_map(|p| rest.find(p).map(|i| (i, *p)))
            .min();
        let Some((i, pattern)) = next else {
            break;
        };
        let value = i + pattern.len();
        out.push_str(&rest[..value]);
        let end = value + rest[value..].find('"').expect("quoted");
        let v = &rest[value..end];
        if pattern == POS {
            // `L:C-L:C`
            let (start, stop) = v.split_once('-').expect("range");
            let (l1, c1) = start.split_once(':').expect("line:column");
            let (l2, c2) = stop.split_once(':').expect("line:column");
            out.push_str(&format!("{}:{c1}-{}:{c2}", add(l1), add(l2)));
        } else {
            out.push_str(&add(v));
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out.into()
}

/// The ids of the headings (`<h1>`…`<h6>`) in `html`, in order.
fn heading_ids(html: &str) -> Vec<String> {
    heading_id_ranges(html)
        .into_iter()
        .map(|r| html[r].to_string())
        .collect()
}

/// Where each heading's id value is in `html`. The note's text is escaped, so `<h` + digit
/// only starts a heading.
fn heading_id_ranges(html: &str) -> Vec<std::ops::Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(i) = html[from..].find("<h").map(|i| from + i) {
        from = i + 2;
        let level = html.as_bytes().get(i + 2).copied().unwrap_or(0);
        let next = html.as_bytes().get(i + 3).copied().unwrap_or(0);
        if !(b'1'..=b'6').contains(&level) || !(next == b' ' || next == b'>') {
            continue;
        }
        let tag_end = i + html[i..].find('>').unwrap_or(html.len() - i);
        if let Some(id) = html[i..tag_end].find(" id=\"") {
            let start = i + id + 5;
            let end = start + html[start..].find('"').unwrap_or(0);
            found.push(start..end);
        }
    }
    found
}

/// `html` with its headings' ids set to `ids` (as many as `heading_ids` finds).
fn replace_ids(html: &str, ids: &[String]) -> String {
    let mut out = String::with_capacity(html.len());
    let mut done = 0;
    for (range, id) in heading_id_ranges(html).into_iter().zip(ids) {
        out.push_str(&html[done..range.start]);
        out.push_str(id);
        done = range.end;
    }
    out.push_str(&html[done..]);
    out
}

/// How many nodes `html` makes as children of the page's body (`node_starts`).
fn top_nodes(html: &str) -> Option<usize> {
    node_starts(html).map(|starts| starts.len())
}

/// Where each node `html` makes as a child of the page's body starts: top-level elements and
/// runs of text between them. None if the tags don't balance. Only for `render`'s own output,
/// where text and attribute values are escaped and void elements end in `/>`.
fn node_starts(html: &str) -> Option<Vec<usize>> {
    const VOID: [&str; 6] = ["br", "hr", "img", "input", "wbr", "col"];
    let bytes = html.as_bytes();
    let (mut depth, mut nodes, mut in_text) = (0usize, Vec::new(), false);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<'
            && bytes
                .get(i + 1)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'/')
        {
            let close = bytes[i + 1] == b'/';
            // The tag's end, skipping quoted attribute values.
            let mut j = i + 1;
            let mut quote = None;
            while j < bytes.len() {
                match (bytes[j], quote) {
                    (b'"' | b'\'', None) => quote = Some(bytes[j]),
                    (q, Some(open)) if q == open => quote = None,
                    (b'>', None) => break,
                    _ => {}
                }
                j += 1;
            }
            if j >= bytes.len() {
                return None;
            }
            if close {
                depth = depth.checked_sub(1)?;
            } else {
                if depth == 0 {
                    nodes.push(i);
                    in_text = false;
                }
                let name_end = html[i + 1..j]
                    .find(|c: char| c.is_ascii_whitespace() || c == '/')
                    .map_or(j, |k| i + 1 + k);
                let name = &html[i + 1..name_end];
                let void =
                    bytes[j - 1] == b'/' || VOID.contains(&name.to_ascii_lowercase().as_str());
                if !void {
                    depth += 1;
                }
            }
            i = j + 1;
        } else {
            if depth == 0 && !in_text {
                nodes.push(i);
                in_text = true;
            }
            i += 1;
        }
    }
    (depth == 0).then_some(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STYLE: Style = Style {
        dark: false,
        theme: Theme::Default,
        width: TextWidth::Medium,
        web_images: false,
        hard_breaks: false,
        links: 0,
    };

    fn wiki(t: &str) -> Option<PathBuf> {
        t.starts_with('N')
            .then(|| PathBuf::from(format!("/n/{t}.md")))
    }

    #[test]
    fn web_images_switch_renders_a_new_page() {
        let md = "# A\n\n![w](https://x.org/w.png)\n";
        let (state, _) = render(None, md, STYLE, &wiki);
        let on = Style {
            web_images: true,
            ..STYLE
        };
        let (_, update) = render(state, md, on, &wiki);
        let Update::Page(html) = update else {
            panic!("the CSP only changes with a new page");
        };
        assert!(html.contains("http: https:"), "{html}");
    }

    /// The body of a page.
    fn body(page: &str) -> &str {
        let start = page.find("<body>").expect("body") + 6;
        let end = page.rfind("</body>").expect("body end");
        &page[start..end]
    }

    /// The body's nodes, as `node_starts` cuts them.
    fn nodes(body: &str) -> Vec<String> {
        let starts = node_starts(body).expect("balanced");
        starts
            .iter()
            .enumerate()
            .map(|(i, &s)| body[s..starts.get(i + 1).copied().unwrap_or(body.len())].to_string())
            .collect()
    }

    /// What the page's patch script does, on the body's nodes.
    fn apply(dom: &mut Vec<String>, patch: &Patch) {
        assert_eq!(dom.len(), patch.nodes, "nodes before the patch");
        for node in dom.iter_mut().skip(patch.shift) {
            *node = shift(node, patch.delta).into_owned();
        }
        for edit in patch.edits.iter().rev() {
            let new = if edit.html.is_empty() {
                vec![]
            } else {
                nodes(&edit.html)
            };
            dom.splice(edit.at..edit.at + edit.remove, new);
        }
        assert_eq!(dom.len(), patch.after, "nodes after the patch");
    }

    /// Renders `texts` one after the other as the preview would, checking each page (built
    /// from the chunks, and patched) against a whole render. Returns how many were patches.
    fn check(texts: &[String]) -> usize {
        let mut state = None;
        let mut dom: Vec<String> = Vec::new();
        let mut patches = 0;
        for (n, md) in texts.iter().enumerate() {
            let (next, update) = render(state.take(), md, STYLE, &wiki);
            let whole = render::preview_page(
                false,
                Theme::Default,
                Some(STYLE.width),
                false,
                &render::body_html(md, &wiki, true, true, true, true),
            );
            match update {
                Update::Page(page) => {
                    assert_eq!(page, whole, "page {n}");
                    dom = nodes(body(&page));
                }
                Update::Patch(patch) => {
                    apply(&mut dom, &patch);
                    patches += 1;
                    assert_eq!(
                        dom.concat(),
                        body(&whole),
                        "patched page {n} after {:?}",
                        texts[n - 1]
                    );
                    // No text node merges with the next: the page has these nodes.
                    assert_eq!(nodes(&dom.concat()).len(), dom.len(), "nodes {n}");
                }
            }
            if let Some(preview) = &next {
                assert_eq!(preview.page(), whole, "chunks of page {n}");
            }
            state = next;
        }
        patches
    }

    const NOTE: &str = "---\ntitle: Test\ntags: [a]\n---\n\n# Title\n\nIntro with *em*, [[Note]], \
[[Missing]], #tag and $x^2$.\n\n## Same\n\ntext\n\n## Same\n\n- [ ] one\n- [x] two\n\n  more\n\n\
- three\n\n1. a\n2. b\n\n> quote\n> more\n\n```rust\nfn main() {}\n```\n\n```mermaid\ngraph TD\n  A-->B\n```\n\n\
    indented\n    code\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n\n<div>\nraw\n</div>\n\nafter html\n\n\
## Sum $a+b$\n\n$$\n\\sum_i i\n$$\n\n```math\nx = 1\n```\n\n---\n\n## Same\n\nSetext\n===\n\nend\n";

    #[test]
    fn chunks_render_as_the_whole_note() {
        for md in [
            NOTE,
            "",
            "\n\n",
            "one line",
            "# a\n\n# a\n\n# a-1\n\n# a\n",
            "x\n\n\n\n",
        ] {
            check(&[md.to_string()]);
        }
        check(&[crate::perf::sample_note(30_000)]);
    }

    #[test]
    fn notes_that_render_whole() {
        for md in [
            "a[^1]\n\n[^1]: note\n",
            "[a]\n\n[a]: /url\n",
            "a\rb\n",
            "x\u{E000}\n",
        ] {
            let (state, _) = render(None, md, STYLE, &wiki);
            assert!(state.is_none(), "{md:?}");
        }
        assert!(cuttable("[[Note]]: a definition list of sorts\n"));
    }

    #[test]
    fn typing_patches_only_the_block() {
        let md = crate::perf::sample_note(20_000);
        let mid = md[..md.len() / 2].rfind("fn section_").unwrap() + 3;
        let mut edited = md.clone();
        edited.insert(mid, 'x');
        let (state, _) = render(None, &md, STYLE, &wiki);
        let (_, update) = render(state, &edited, STYLE, &wiki);
        let Update::Patch(patch) = update else {
            panic!("not a patch");
        };
        assert_eq!(patch.edits.len(), 1);
        assert_eq!(patch.edits[0].remove, 2); // the code block and its newline
        assert_eq!(patch.delta, 0);
        assert!(check(&[md, edited]) == 1);
    }

    #[test]
    fn editing_inside_an_alert() {
        let md = "# A\n\ntext\n\n> [!NOTE]\n> one\n>\n> two\n\nend\n";
        let texts: Vec<String> = [
            md,
            "# A\n\ntext\n\n> [!NOTE]\n> one more\n>\n> two\n\nend\n",
            "# A\n\ntext\n\n> [!NOTE]\n> one more\n>\n> - two\n> - three\n\nend\n",
            "# A\n\ntext\n\n> [!WARNING]\n> one more\n>\n> - two\n> - three\n\nend\n",
            "# A\n\ntext\n\n> [!WARNING] x\n> one more\n>\n> - two\n> - three\n\nend\n",
            "# A\n\nnew\n\ntext\n\n> [!TIP]\n> one more\n\nend\n",
        ]
        .map(String::from)
        .to_vec();
        assert!(check(&texts) >= 4);
        let (state, _) = render(None, texts[0].as_str(), STYLE, &wiki);
        let (_, update) = render(state, &texts[1], STYLE, &wiki);
        let Update::Patch(patch) = update else {
            panic!("not a patch");
        };
        assert_eq!(patch.edits.len(), 1);
        assert!(patch.edits[0].html.contains("alert alert-note"));
    }

    #[test]
    fn table_of_contents_follows_the_headings() {
        let texts: Vec<String> = [
            "[TOC]\n\ntext\n",
            "[TOC]\n\ntext\n\n# One\n",
            "[TOC]\n\ntext\n\n# One\n\nmore\n\n## Two\n",
            "[TOC]\n\ntext\n\n# One\n\nmore\n\n## Twice\n",
            "# Top\n\n[TOC]\n\ntext\n\n# One\n\nmore\n\n## Twice\n",
            "# Top\n\n[TOC]\n\ntext\n\n# One\n\nmore\n\n## Twice\n\n[toc]\n",
            "# Top\n\n[TOC] x\n\ntext\n\n# One\n\nmore\n\n## Twice\n\n[toc]\n",
            "# Top\n\n[TOC]\n\ntext\n\n# One\n\nmore\n\nTwice\n",
            "[TOC]\n\ntext\n\nmore\n",
        ]
        .map(String::from)
        .to_vec();
        assert!(check(&texts) >= 6);
        // Renaming a heading patches it and the table of contents, nothing else.
        let md = "[TOC]\n\na\n\n# One\n\nb\n\n## Two\n\nc\n";
        let edited = md.replace("## Two", "## Three");
        let (state, _) = render(None, md, STYLE, &wiki);
        let (_, update) = render(state, &edited, STYLE, &wiki);
        let Update::Patch(patch) = update else {
            panic!("not a patch");
        };
        assert_eq!(patch.edits.len(), 2, "{patch:?}");
        assert!(patch.edits[0].html.contains("<nav class=\"toc\""));
        assert!(patch.edits[0].html.contains(">Three</a>"));
        assert!(patch.edits[1].html.contains("<h2"));
        // Text elsewhere: only that block.
        let edited = md.replace("\nb\n", "\nbb\n");
        let (state, _) = render(None, md, STYLE, &wiki);
        let (_, update) = render(state, &edited, STYLE, &wiki);
        let Update::Patch(patch) = update else {
            panic!("not a patch");
        };
        assert_eq!(patch.edits.len(), 1, "{patch:?}");
    }

    /// A small deterministic generator (no dependency for tests).
    struct Random(u64);

    impl Random {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as usize) % n.max(1)
        }
    }

    #[test]
    fn random_edits_patch_to_the_whole_render() {
        const SNIPPETS: [&str; 35] = [
            "* ",
            "+ ",
            "  - ",
            "\t",
            "***\n",
            "    - x\n",
            "[x]",
            "\n\n\n",
            "> - ",
            "x",
            "\n",
            "\n\n",
            "- ",
            "```",
            "```\n",
            "# ",
            "## Same\n",
            "    ",
            "|a|b|\n|-|-|\n",
            "$x$",
            "<div>\n",
            "> ",
            "1. ",
            "[[Note]]",
            " #tag",
            "- [ ] task\n",
            "~~~\n",
            "---\n",
            "  ",
            "\n===\n",
            "$$\n",
            "```rust\nlet a = 1;\n```\n",
            "<!--\n",
            "\n[TOC]\n",
            "[toc]",
        ];
        // `SEED=<n> cargo test random_edits` tries other edits.
        let seed = std::env::var("SEED").ok().and_then(|s| s.parse().ok());
        let mut random = Random(seed.unwrap_or(7));
        let starts = [NOTE.to_string(), crate::perf::sample_note(6_000)];
        let mut patches = 0;
        for start in starts {
            for _ in 0..60 {
                let mut md = start.clone();
                let mut texts = vec![md.clone()];
                for _ in 0..25 {
                    let mut at = random.below(md.len() + 1);
                    while !md.is_char_boundary(at) {
                        at -= 1;
                    }
                    if random.below(3) == 0 && !md.is_empty() {
                        let mut end = (at + 1 + random.below(30)).min(md.len());
                        while !md.is_char_boundary(end) {
                            end += 1;
                        }
                        md.replace_range(at..end, "");
                    } else {
                        md.insert_str(at, SNIPPETS[random.below(SNIPPETS.len())]);
                    }
                    texts.push(md.clone());
                }
                patches += check(&texts);
            }
        }
        assert!(patches > 100, "only {patches} patches");
    }

    #[test]
    fn shift_moves_lines() {
        let html = "<p data-sourcepos=\"3:1-4:2\">a</p>\n<a class=\"task\" href=\"mm-task:5\">\
<input /></a><div class=\"code\"><a class=\"copy\" href=\"mm-copy:7\">Copy</a></div>\
<a href=\"mm-task:5\">user</a>";
        assert_eq!(
            shift(html, 2),
            "<p data-sourcepos=\"5:1-6:2\">a</p>\n<a class=\"task\" href=\"mm-task:7\">\
<input /></a><div class=\"code\"><a class=\"copy\" href=\"mm-copy:9\">Copy</a></div>\
<a href=\"mm-task:5\">user</a>"
        );
        assert_eq!(shift(html, -1).matches("2:1-3:2").count(), 1);
    }

    #[test]
    fn node_counts() {
        assert_eq!(
            top_nodes("<p>a</p>\n<hr />\n<ul>\n<li>x</li>\n</ul>\n"),
            Some(6)
        );
        assert_eq!(top_nodes("&lt;div&gt;\n<p>a</p>"), Some(2));
        assert_eq!(top_nodes("<p>a"), None);
        assert_eq!(top_nodes(""), Some(0));
    }

    #[test]
    fn ids_follow_the_whole_note() {
        let md = "# A\n\n# A\n\n# B\n";
        let edited = "# A\n\n# B\n\n# A\n\n# B\n";
        check(&[md.to_string(), edited.to_string(), md.to_string()]);
    }
}
