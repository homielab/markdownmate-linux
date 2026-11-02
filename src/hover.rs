//! Hover preview of `[[wiki links]]` in the editor: a tooltip with the linked note's name and
//! its first lines as plain Markdown. `excerpt` picks the lines (pure, tested); `Hover` keeps
//! the last lookup and reads closed notes off the main thread, only as far as needed.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib, pango};

use crate::editor::Editor;
use crate::{lists, render};

/// Non-empty lines shown at most.
pub const MAX_LINES: usize = 12;
/// Characters shown at most (line breaks and the `…` ending a cut line count).
pub const MAX_CHARS: usize = 600;
/// Bytes kept of one line read from disk; the rest of a longer line is skipped. Holds more
/// than `MAX_CHARS` characters, so a cut line always fills the excerpt.
const MAX_LINE_BYTES: u64 = 4096;
/// Lines looked at for the end of a front matter; none by then → the `---` is note text.
const MAX_FRONT_MATTER: usize = 1000;
/// Paragraph lines kept as the possible text of a Setext heading.
const MAX_PARAGRAPH: usize = 20;

/// The first lines of a note shown on hover: front matter skipped, empty lines left out, at
/// most `MAX_LINES` lines and `MAX_CHARS` characters. With `heading` (a `[[Note#Heading]]`
/// fragment, matched as `render::heading_at` does) it starts at that heading's line; a
/// heading the note lacks gives its top. A block (`^id`, `blocks`) starts at its first line;
/// the whole note is read to find it. Takes from `lines` only what it needs.
pub fn excerpt(lines: impl Iterator<Item = String>, heading: Option<&str>) -> String {
    if let Some(id) = heading.and_then(crate::blocks::block_ref) {
        let lines: Vec<String> = lines.collect();
        let start = crate::blocks::find(&lines.join("\n"), id).map(|b| *b.lines.start());
        let Some(start) = start else {
            return excerpt(lines.into_iter(), None);
        };
        let mut from = Collector::default();
        for line in &lines[start - 1..] {
            if from.push(line) {
                break;
            }
        }
        return from.text;
    }
    let mut lines = skip_front_matter(lines);
    let mut top = Collector::default();
    let Some(want) = heading.map(str::trim).filter(|h| !h.is_empty()) else {
        for line in lines {
            if top.push(&line) {
                break;
            }
        }
        return top.text;
    };
    let wanted = [want.to_string(), comrak::Anchorizer::new().anchorize(want)];
    let mut ids = comrak::Anchorizer::new();
    let mut fence = None;
    // The paragraph so far (the text of a Setext heading if an underline follows).
    let mut para: VecDeque<String> = VecDeque::new();
    while let Some(line) = lines.next() {
        top.push(&line);
        let in_code = fence.is_some();
        if lists::fence_step(&mut fence, &line) || in_code || line.trim().is_empty() {
            para.clear();
            continue;
        }
        let heading = if is_atx(&line) {
            render::heading_text(&line).map(|text| (text, Vec::new()))
        } else if is_underline(&line) && !para.is_empty() {
            setext(&para, &line)
        } else {
            None
        };
        let Some((text, above)) = heading else {
            if para.len() == MAX_PARAGRAPH {
                para.pop_front();
            }
            para.push_back(line);
            continue;
        };
        para.clear();
        if !wanted.contains(&ids.anchorize(&text)) {
            continue;
        }
        let mut from = Collector::default();
        let full = above.iter().chain([&line]).any(|l| from.push(l));
        if !full {
            for line in lines {
                if from.push(&line) {
                    break;
                }
            }
        }
        return from.text;
    }
    top.text
}

/// `lines` without the YAML front matter that opens them (`---` on line 1 up to the next
/// `---` line, as `lists::front_matter_lines`).
fn skip_front_matter(mut lines: impl Iterator<Item = String>) -> impl Iterator<Item = String> {
    let mut kept = Vec::new();
    if let Some(first) = lines.next() {
        let opens = first.trim_end() == "---";
        kept.push(first);
        if opens {
            while kept.len() <= MAX_FRONT_MATTER {
                let Some(line) = lines.next() else { break };
                let closes = line.trim_end() == "---";
                kept.push(line);
                if closes {
                    kept.clear();
                    break;
                }
            }
        }
    }
    kept.into_iter().chain(lines)
}

/// The Setext heading `underline` ends after the paragraph lines `para`: its text and its
/// lines above the underline. The whole paragraph is its text; if that does not parse as one
/// heading (a list item before, say), the last line alone.
fn setext(para: &VecDeque<String>, underline: &str) -> Option<(String, Vec<String>)> {
    let whole: Vec<String> = para.iter().cloned().collect();
    let last = para.back().cloned().into_iter().collect::<Vec<_>>();
    [whole, last].into_iter().find_map(|above| {
        let src = format!("{}\n{underline}", above.join("\n"));
        render::heading_text(&src).map(|text| (text, above))
    })
}

/// `# Heading` (up to 3 spaces before, 1–6 `#`, then a space or the line end).
fn is_atx(line: &str) -> bool {
    let t = line.trim_start_matches(' ');
    let hashes = t.len() - t.trim_start_matches('#').len();
    line.len() - t.len() <= 3
        && (1..=6).contains(&hashes)
        && t[hashes..]
            .chars()
            .next()
            .is_none_or(|c| c == ' ' || c == '\t')
}

/// A Setext underline: `===` or `---` (up to 3 spaces before).
fn is_underline(line: &str) -> bool {
    let t = line.trim_end();
    let s = t.trim_start_matches(' ');
    t.len() - s.len() <= 3
        && !s.is_empty()
        && (s.bytes().all(|b| b == b'=') || s.bytes().all(|b| b == b'-'))
}

/// The excerpt being built.
#[derive(Default)]
struct Collector {
    text: String,
    lines: usize,
    chars: usize,
    full: bool,
}

impl Collector {
    /// Adds `line` unless it is empty; true once nothing more fits.
    fn push(&mut self, line: &str) -> bool {
        let line = line.trim_end();
        if self.full || line.trim_start().is_empty() {
            return self.full;
        }
        if !self.text.is_empty() {
            self.text.push('\n');
            self.chars += 1;
        }
        let room = MAX_CHARS.saturating_sub(self.chars);
        let len = line.chars().count();
        if len > room {
            self.text.extend(line.chars().take(room.saturating_sub(1)));
            self.text.truncate(self.text.trim_end().len());
            self.text.push('…');
            self.full = true;
            return true;
        }
        self.text.push_str(line);
        self.chars += len;
        self.lines += 1;
        self.full = self.lines == MAX_LINES || self.chars >= MAX_CHARS;
        self.full
    }
}

/// The lines of `reader` without their `\n` / `\r\n`, each cut to `MAX_LINE_BYTES`, read one
/// at a time: stopping early leaves the rest unread.
pub fn lines(reader: impl BufRead) -> impl Iterator<Item = String> {
    let mut reader = reader;
    let mut skip = false;
    std::iter::from_fn(move || {
        if std::mem::take(&mut skip) {
            reader.skip_until(b'\n').ok()?;
        }
        let mut buf = Vec::new();
        let n = reader
            .by_ref()
            .take(MAX_LINE_BYTES)
            .read_until(b'\n', &mut buf)
            .ok()?;
        if n == 0 {
            return None;
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
            if buf.last() == Some(&b'\r') {
                buf.pop();
            }
        } else {
            skip = n as u64 == MAX_LINE_BYTES;
        }
        Some(String::from_utf8_lossy(&buf).into_owned())
    })
}

/// `excerpt` of the note at `path`, read from disk only as far as needed.
pub fn read(path: &Path, heading: Option<&str>) -> io::Result<String> {
    Ok(excerpt(lines(BufReader::new(File::open(path)?)), heading))
}

/// What the tooltip shows for a wiki link.
#[derive(Clone, Debug, PartialEq)]
pub struct Peek {
    /// The note's file name without extension.
    pub name: String,
    pub text: String,
}

/// A wiki link as looked up: its name as written and the note it was written in.
#[derive(Clone, PartialEq)]
struct Key {
    name: String,
    note: Option<PathBuf>,
}

enum State {
    /// A closed note being read.
    Reading,
    /// None: a missing note, or one that could not be read. `open`: taken from the buffer.
    Ready { peek: Option<Peek>, open: bool },
}

/// The last wiki link looked up for one editor and what it gave. Kept while the pointer stays
/// on links, so the tooltip (queried on every pointer move) does not read again.
#[derive(Default)]
pub(crate) struct Hover {
    last: RefCell<Option<(Key, State)>>,
}

impl Hover {
    /// The preview for wiki link `name` (as written, `Note#Heading` or `#Heading`) in
    /// `editor`'s note. A closed note not read yet → None; reading starts, and the tooltip
    /// is queried again when it is done.
    pub(crate) fn peek(self: &Rc<Self>, editor: &Editor, name: &str) -> Option<Peek> {
        let key = Key {
            name: name.to_string(),
            note: editor.path.borrow().clone(),
        };
        if let Some((k, state)) = &*self.last.borrow()
            && *k == key
        {
            return match state {
                State::Reading => None,
                State::Ready { peek, .. } => peek.clone(),
            };
        }
        let state = self.look_up(editor, &key);
        let peek = match &state {
            State::Ready { peek, .. } => peek.clone(),
            State::Reading => None,
        };
        self.last.replace(Some((key, state)));
        peek
    }

    fn look_up(self: &Rc<Self>, editor: &Editor, key: &Key) -> State {
        let (note, heading) = match key.name.split_once('#') {
            Some((note, heading)) => (note, Some(heading.to_string())),
            None => (key.name.as_str(), None),
        };
        let path = if note.trim().is_empty() {
            key.note.clone()
        } else {
            let notes = editor.note_list.borrow().clone();
            let aliases = editor.aliases.borrow().clone();
            let root = editor.root.borrow().clone().unwrap_or_default();
            render::resolve_wiki_with(note, &notes, &aliases, &root, key.note.as_deref()).cloned()
        };
        let Some(path) = path else {
            return State::Ready {
                peek: None,
                open: false,
            };
        };
        let name = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        if key.note.as_ref() == Some(&path) {
            let text = excerpt(buffer_lines(editor.buffer.upcast_ref()), heading.as_deref());
            return State::Ready {
                peek: Some(Peek { name, text }),
                open: true,
            };
        }
        let hover = Rc::downgrade(self);
        let view = editor.view.clone();
        let key = key.clone();
        glib::spawn_future_local(async move {
            let text = gio::spawn_blocking(move || read(&path, heading.as_deref()).ok())
                .await
                .ok()
                .flatten();
            let Some(hover) = hover.upgrade() else {
                return;
            };
            let mut last = hover.last.borrow_mut();
            if let Some((k, state)) = last.as_mut()
                && *k == key
            {
                *state = State::Ready {
                    peek: text.map(|text| Peek { name, text }),
                    open: false,
                };
                drop(last);
                view.trigger_tooltip_query();
            }
        });
        State::Reading
    }

    /// The pointer left the links: the next hover looks up (and reads) again.
    pub(crate) fn forget(&self) {
        self.last.take();
    }

    /// The open note changed: a preview taken from it is stale.
    pub(crate) fn text_changed(&self) {
        let stale = matches!(
            &*self.last.borrow(),
            Some((_, State::Ready { open: true, .. }))
        );
        if stale {
            self.forget();
        }
    }
}

/// The lines of `buffer`, read one at a time.
fn buffer_lines(buffer: &gtk::TextBuffer) -> impl Iterator<Item = String> + '_ {
    (0..buffer.line_count()).map_while(move |n| {
        let start = buffer.iter_at_line(n)?;
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        Some(buffer.text(&start, &end, true).into())
    })
}

/// The tooltip's content: the name in bold, then the lines as plain text.
pub(crate) fn widget(peek: &Peek) -> gtk::Widget {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let title = gtk::Label::new(Some(&peek.name));
    title.add_css_class("heading");
    title.set_xalign(0.0);
    title.set_ellipsize(pango::EllipsizeMode::End);
    title.set_max_width_chars(60);
    content.append(&title);
    if !peek.text.is_empty() {
        let body = gtk::Label::new(Some(&peek.text));
        body.set_xalign(0.0);
        body.set_wrap(true);
        body.set_wrap_mode(pango::WrapMode::WordChar);
        body.set_max_width_chars(60);
        content.append(&body);
    }
    content.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Cursor;

    fn ex(text: &str, heading: Option<&str>) -> String {
        excerpt(text.lines().map(String::from), heading)
    }

    #[test]
    fn skips_front_matter_and_empty_lines() {
        let md = "---\ntitle: x\ntags: [a]\n---\n\n# Note\n\nFirst line.  \n\n- item\n";
        assert_eq!(ex(md, None), "# Note\nFirst line.\n- item");
        // No closing `---`: not front matter.
        assert_eq!(ex("---\nplain\n", None), "---\nplain");
        // `---` later in the note is text.
        assert_eq!(ex("a\n---\nb", None), "a\n---\nb");
        assert_eq!(ex("", None), "");
        assert_eq!(ex("---\nk: v\n---\n", None), "");
    }

    #[test]
    fn starts_at_a_block() {
        let md = "---\nk: v\n---\n\nIntro.\n\n- one\n- two ^b\n  - child\n\nend ^e\n";
        assert_eq!(ex(md, Some("^b")), "- two ^b\n  - child\nend ^e");
        assert_eq!(ex(md, Some("^e")), "end ^e");
        // An unknown block: the note's top.
        assert_eq!(ex(md, Some("^nope")), ex(md, None));
    }

    #[test]
    fn line_and_character_limits() {
        let md: String = (1..=20).map(|i| format!("line {i}\n\n")).collect();
        let out = ex(&md, None);
        assert_eq!(out.lines().count(), MAX_LINES);
        assert!(out.ends_with("line 12"));

        let long = "word ".repeat(200);
        let out = ex(&format!("short\n{long}\nnext"), None);
        assert_eq!(out.chars().count(), MAX_CHARS);
        assert!(out.starts_with("short\nword"));
        assert!(out.ends_with("wor…"));

        let line = "é".repeat(100);
        let md = [line.as_str(); 10].join("\n");
        let out = ex(&md, None);
        assert_eq!(out.chars().count(), MAX_CHARS);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn starts_at_the_heading() {
        let md = "---\ntitle: t\n---\nIntro\n\n## Setup\n\nInstall it.\n\n## Use\nRun it.\n";
        assert_eq!(
            ex(md, Some("Setup")),
            "## Setup\nInstall it.\n## Use\nRun it."
        );
        assert_eq!(ex(md, Some("use")), "## Use\nRun it.");
        // Missing heading: the top of the note.
        assert_eq!(
            ex(md, Some("Nope")),
            "Intro\n## Setup\nInstall it.\n## Use\nRun it."
        );
        assert_eq!(ex(md, Some("")), ex(md, None));
    }

    #[test]
    fn heading_rules_match_the_preview() {
        // Repeated titles by id; inline markup dropped; Setext (one or more lines); not in code.
        let md = "# Notes\na\n# Notes\nb\n## **Bold** title\nc\n\nSet up\n---\nd\n\n\
                  one\ntwo\n===\n```\n# Hidden\n```\n# Hidden\ne\n";
        let targets = render::heading_targets(md);
        let fragments: Vec<&str> = targets.iter().map(|t| t.fragment.as_str()).collect();
        assert_eq!(
            fragments,
            [
                "Notes",
                "notes-1",
                "Bold title",
                "Set up",
                "one two",
                "Hidden"
            ]
        );
        // Starts on the line `heading_at` finds (a Setext heading: its first text line).
        for want in fragments.iter().copied().chain(["bold-title", "one-two"]) {
            let (line, _) = render::heading_at(md, want).unwrap();
            let first = ex(md, Some(want)).lines().next().unwrap().to_string();
            assert_eq!(md.lines().nth(line - 1).unwrap(), first, "{want}");
        }
        assert_eq!(
            ex(md, Some("Set up")),
            "Set up\n---\nd\none\ntwo\n===\n```\n# Hidden\n```\n# Hidden\ne"
        );
        assert_eq!(ex(md, Some("Hidden")), "# Hidden\ne");
        // `#tag` is no heading; neither is a list item then `---`, but a line after that is.
        assert_eq!(ex("#tag\n- x\n---\n# tag\nz", Some("tag")), "# tag\nz");
        assert_eq!(ex("- x\n---\ny", Some("x")), "- x\n---\ny");
        assert_eq!(ex("- x\n---\ny\n===\nz", Some("y")), "y\n===\nz");
    }

    #[test]
    fn crlf_and_long_lines_from_a_reader() {
        let read = |s: &str| excerpt(lines(Cursor::new(s.as_bytes().to_vec())), None);
        assert_eq!(read("---\r\nk: v\r\n---\r\nA\r\n\r\nB\r\n"), "A\nB");
        let long = format!("{}\n# H\nafter\n", "x".repeat(10_000));
        let all: Vec<String> = lines(Cursor::new(long.into_bytes())).collect();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].len(), MAX_LINE_BYTES as usize);
        assert_eq!(all[1], "# H");
    }

    /// Counts the bytes read through it.
    struct Counting<'a, R>(R, &'a Cell<usize>);

    impl<R: Read> Read for Counting<'_, R> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.0.read(buf)?;
            self.1.set(self.1.get() + n);
            Ok(n)
        }
    }

    #[test]
    fn large_note_is_read_only_as_far_as_needed() {
        let mut md = String::from("---\ntitle: big\n---\n# Big\n\n## Middle\nm\n");
        while md.len() < 5_000_000 {
            md.push_str("Some paragraph text that goes on for a while, line after line.\n");
        }
        md.push_str("## End\nlast\n");
        let read = Cell::new(0);
        let out = excerpt(
            lines(BufReader::new(Counting(Cursor::new(md.as_bytes()), &read))),
            Some("Middle"),
        );
        assert!(out.starts_with("## Middle\nm\nSome paragraph"));
        assert!(read.get() <= 16 * 1024, "read {} bytes", read.get());

        // A heading near the end: read up to it, then only the lines shown.
        read.set(0);
        let out = excerpt(
            lines(BufReader::new(Counting(Cursor::new(md.as_bytes()), &read))),
            Some("End"),
        );
        assert_eq!(out, "## End\nlast");
        assert_eq!(read.get(), md.len());

        // One 5 MB line: only its start is read.
        let one = "y".repeat(5_000_000);
        read.set(0);
        let out = excerpt(
            lines(BufReader::new(Counting(Cursor::new(one.as_bytes()), &read))),
            None,
        );
        assert_eq!(out.chars().count(), MAX_CHARS);
        assert!(read.get() <= 16 * 1024, "read {} bytes", read.get());
    }

    #[test]
    fn reads_a_file() {
        let dir = std::env::temp_dir().join(format!("mm-hover-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        struct Remove(PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _guard = Remove(dir.clone());
        let path = dir.join("Note.md");
        std::fs::write(&path, "---\na: b\n---\nHello\n# Part\nthere\n").unwrap();
        assert_eq!(read(&path, None).unwrap(), "Hello\n# Part\nthere");
        assert_eq!(read(&path, Some("Part")).unwrap(), "# Part\nthere");
        assert!(read(&dir.join("missing.md"), None).is_err());
    }
}
