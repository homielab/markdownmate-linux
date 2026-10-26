//! Focus Mode (main menu, Shift+Ctrl+J): the editor draws every line outside the cursor's
//! paragraph (`paragraph`) dimmed, with one text tag over the rest of the note.
//!
//! Display only: the text, undo, saving and the preview are untouched. The tag covers the
//! text before and after the focused paragraph; when the paragraph changes, only the text that
//! leaves it is tagged and only the new paragraph is untagged, so moving the cursor or typing
//! costs the paragraph, not the note. Text inserted outside the focused paragraph (Replace All,
//! undo, a note opened) is tagged as it comes in. The tag is the scheme's text colour, faded, so
//! every style scheme, light or dark, keeps its own look.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;

/// Lines read above and below the cursor's line, at most: the cost doesn't grow with the note.
pub(crate) const MAX_LINES: usize = 1000;

/// The opacity of dimmed text: about a tertiary label's contrast.
const FADED_ALPHA: f32 = 0.3;

/// The paragraph Focus Mode keeps at full contrast, as (first, last) line numbers (0-based,
/// inclusive), around line `line` of `count` lines (`text(n)`: line n without its line break).
///
/// It's the block of non-blank lines around the cursor (blocks are separated by blank lines, so
/// a table or a code block without blank lines is one paragraph), except that inside a list
/// each item is its own paragraph — the item's line and the lines below it up to the next item;
/// text above the first item, in the same block, is a paragraph too. On a blank line there is
/// none. At most `MAX_LINES` lines are read above and below. The same rule as the macOS app.
pub(crate) fn paragraph<S: AsRef<str>>(
    line: usize,
    count: usize,
    text: impl Fn(usize) -> S,
) -> Option<(usize, usize)> {
    if count == 0 {
        return None;
    }
    let line = line.min(count - 1);
    let current = text(line);
    if is_blank(current.as_ref()) {
        return None;
    }
    let mut first = line;
    if !is_list_item(current.as_ref()) {
        for _ in 0..MAX_LINES {
            if first == 0 {
                break;
            }
            let previous = text(first - 1);
            if is_blank(previous.as_ref()) {
                break;
            }
            first -= 1;
            if is_list_item(previous.as_ref()) {
                break;
            }
        }
    }
    let mut last = line;
    for _ in 0..MAX_LINES {
        if last + 1 >= count {
            break;
        }
        let next = text(last + 1);
        if is_blank(next.as_ref()) || is_list_item(next.as_ref()) {
            break;
        }
        last += 1;
    }
    Some((first, last))
}

/// Spaces, tabs and line breaks only (or nothing).
pub(crate) fn is_blank(line: &str) -> bool {
    line.chars()
        .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

/// A list item's first line: indentation, then `-`, `*` or `+`, or 1–9 digits and `.` or `)`,
/// then a space, a tab or the end of the line (`- [ ] task` included; `---` is not).
pub(crate) fn is_list_item(line: &str) -> bool {
    let rest = line.trim_start_matches([' ', '\t']);
    let rest = if let Some(rest) = rest.strip_prefix(['-', '*', '+']) {
        rest
    } else {
        let digits = rest.bytes().take(10).take_while(u8::is_ascii_digit).count();
        if !(1..=9).contains(&digits) {
            return false;
        }
        match rest[digits..].strip_prefix(['.', ')']) {
            Some(rest) => rest,
            None => return false,
        }
    };
    rest.chars()
        .next()
        .is_none_or(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

/// Focus Mode of one editor buffer.
pub(crate) struct Focus {
    buffer: sourceview5::Buffer,
    tag: gtk::TextTag,
    /// The focused paragraph as last tagged (moves with edits): everything outside it is
    /// tagged. Start: left gravity, end: right gravity, so typing at either end stays inside.
    start: gtk::TextMark,
    end: gtk::TextMark,
    on: Cell<bool>,
    /// An update is queued (`schedule`).
    pending: Cell<bool>,
}

impl Focus {
    /// Focus Mode for `buffer`, off; follows the cursor and edits once turned on.
    pub(crate) fn attach(buffer: &sourceview5::Buffer) -> Rc<Focus> {
        let tag = gtk::TextTag::new(None);
        buffer.tag_table().add(&tag);
        let at = buffer.start_iter();
        let focus = Rc::new(Focus {
            buffer: buffer.clone(),
            tag,
            start: buffer.create_mark(None, &at, true),
            end: buffer.create_mark(None, &at, false),
            on: Cell::new(false),
            pending: Cell::new(false),
        });
        let weak = Rc::downgrade(&focus);
        // After the text is in: `location` is then the end of the inserted text.
        buffer.connect_closure(
            "insert-text",
            true,
            glib::closure_local!(
                #[strong]
                weak,
                move |_buffer: sourceview5::Buffer,
                      location: gtk::TextIter,
                      text: String,
                      _len: i32| {
                    if let Some(focus) = weak.upgrade() {
                        focus.inserted(&location, text.chars().count() as i32);
                    }
                }
            ),
        );
        let w = weak.clone();
        buffer.connect_mark_set(move |buffer, _, mark| {
            if (*mark == buffer.get_insert() || *mark == buffer.selection_bound())
                && let Some(focus) = w.upgrade()
            {
                focus.schedule();
            }
        });
        buffer.connect_changed(move |_| {
            if let Some(focus) = weak.upgrade() {
                focus.schedule();
            }
        });
        focus
    }

    /// Turns Focus Mode on (dims all but the cursor's paragraph) or off (undims all).
    pub(crate) fn set_on(&self, on: bool) {
        if self.on.replace(on) == on {
            return;
        }
        let (start, end) = self.buffer.bounds();
        if on {
            // Above the tags made before it (tags, formulas), which set colours too.
            self.tag
                .set_priority(self.buffer.tag_table().size().saturating_sub(1));
            self.buffer.apply_tag(&self.tag, &start, &end);
            self.buffer.move_mark(&self.start, &start);
            self.buffer.move_mark(&self.end, &start);
            self.update();
        } else {
            self.buffer.remove_tag(&self.tag, &start, &end);
        }
    }

    /// The dimmed text's colour: the scheme's text colour (`dark`: white without one, else
    /// black), faded.
    pub(crate) fn set_colours(&self, scheme: Option<&sourceview5::StyleScheme>, dark: bool) {
        let mut colour = scheme
            .and_then(|s| s.style("text"))
            .and_then(|s| s.foreground())
            .and_then(|c| gtk::gdk::RGBA::parse(c.as_str()).ok())
            .unwrap_or(if dark {
                gtk::gdk::RGBA::WHITE
            } else {
                gtk::gdk::RGBA::BLACK
            });
        colour.set_alpha(FADED_ALPHA);
        self.tag.set_foreground_rgba(Some(&colour));
    }

    /// Updates once the current change is over (all of a Replace All, the cursor moved), but
    /// before the frame is drawn.
    fn schedule(self: &Rc<Self>) {
        if !self.on.get() || self.pending.replace(true) {
            return;
        }
        let weak: Weak<Focus> = Rc::downgrade(self);
        glib::idle_add_local_full(glib::Priority::DEFAULT, move || {
            if let Some(focus) = weak.upgrade() {
                focus.pending.set(false);
                focus.update();
            }
            glib::ControlFlow::Break
        });
    }

    /// Text inserted, ending at `end`, `chars` long: dimmed unless inside the focused
    /// paragraph (`update` sorts out the paragraph afterwards).
    fn inserted(&self, end: &gtk::TextIter, chars: i32) {
        if !self.on.get() || chars == 0 {
            return;
        }
        let stop = end.offset();
        let from = stop - chars;
        let first = self.buffer.iter_at_mark(&self.start).offset();
        let last = self.buffer.iter_at_mark(&self.end).offset();
        // An empty paragraph before (a blank line) has nothing to be inside of.
        if from >= first && stop <= last && last - first > chars {
            return;
        }
        let start = self.buffer.iter_at_offset(from);
        self.buffer.apply_tag(&self.tag, &start, end);
    }

    /// Dims what left the focused paragraph and undims the paragraph now at the cursor.
    pub(crate) fn update(&self) {
        if !self.on.get() {
            return;
        }
        let (new_start, new_end) = self.current();
        let old_start = self.buffer.iter_at_mark(&self.start);
        let old_end = self.buffer.iter_at_mark(&self.end);
        let (ns, ne) = (new_start.offset(), new_end.offset());
        let (os, oe) = (old_start.offset(), old_end.offset());
        if (os, oe) != (ns, ne) {
            for (a, b) in [(os, oe.min(ns)), (os.max(ne), oe)] {
                if a < b {
                    let (a, b) = (self.buffer.iter_at_offset(a), self.buffer.iter_at_offset(b));
                    self.buffer.apply_tag(&self.tag, &a, &b);
                }
            }
            self.buffer.move_mark(&self.start, &new_start);
            self.buffer.move_mark(&self.end, &new_end);
        }
        // Text pasted with the tag, or typed on what was a blank line.
        let mut toggle = new_start;
        if ns < ne
            && (new_start.has_tag(&self.tag)
                || (toggle.forward_to_tag_toggle(Some(&self.tag)) && toggle.offset() < ne))
        {
            self.buffer.remove_tag(&self.tag, &new_start, &new_end);
        }
    }

    /// The focused text now: the paragraphs at both ends of the selection (or the cursor's)
    /// and everything between, whole lines without the last line break; empty on a blank line.
    fn current(&self) -> (gtk::TextIter, gtk::TextIter) {
        let buffer = &self.buffer;
        let (a, b) = buffer.selection_bounds().unwrap_or_else(|| {
            let at = buffer.iter_at_mark(&buffer.get_insert());
            (at, at)
        });
        let count = buffer.line_count().max(1) as usize;
        let text = |n: usize| {
            let Some(start) = buffer.iter_at_line(n as i32) else {
                return glib::GString::new();
            };
            let mut end = start;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            start.slice(&end)
        };
        let lines = |at: &gtk::TextIter| paragraph(at.line() as usize, count, text);
        let (first, last) = match (lines(&a), lines(&b)) {
            (Some(p), Some(q)) => (p.0.min(q.0), p.1.max(q.1)),
            (Some(p), None) => (p.0, p.1.max(b.line() as usize)),
            (None, Some(q)) => (q.0.min(a.line() as usize), q.1),
            (None, None) if a.line() != b.line() => (a.line() as usize, b.line() as usize),
            (None, None) => return (a, a),
        };
        let start = buffer
            .iter_at_line(first as i32)
            .unwrap_or_else(|| buffer.start_iter());
        let mut end = buffer
            .iter_at_line(last as i32)
            .unwrap_or_else(|| buffer.end_iter());
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        (start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines as GtkTextBuffer splits them (`\n`, `\r\n`), without their line breaks.
    fn lines(text: &str) -> Vec<&str> {
        text.split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .collect()
    }

    /// The paragraph's text around line `line` of `text` (as the macOS tests: from its first
    /// line's start to its last line's end, without the final line break); "" for none.
    fn at_line(text: &str, line: usize) -> String {
        let all = lines(text);
        let Some((first, last)) = paragraph(line, all.len(), |n| all[n]) else {
            return String::new();
        };
        // Line starts in `text` (byte offsets).
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        let end = starts[last] + all[last].len();
        text[starts[first]..end].to_string()
    }

    /// The paragraph around the first occurrence of `marker`.
    fn at(text: &str, marker: &str) -> String {
        let offset = text.find(marker).expect(marker);
        at_line(text, text[..offset].matches('\n').count())
    }

    #[test]
    fn blank_lines_separate_paragraphs() {
        let text = "# Title\n\nFirst line\nsecond line\n\n\nThird\n";
        assert_eq!(at(text, "second"), "First line\nsecond line");
        assert_eq!(at(text, "First"), "First line\nsecond line");
        assert_eq!(at(text, "Title"), "# Title");
        assert_eq!(at(text, "Third"), "Third", "no final line break");
    }

    #[test]
    fn blank_line_has_no_paragraph() {
        let text = "One\n\n  \t\nTwo";
        assert_eq!(at_line(text, 1), "");
        assert_eq!(at_line(text, 2), "", "spaces and tabs only");
        assert_eq!(at_line("", 0), "");
        assert_eq!(paragraph(0, 0, |_| ""), None, "no lines");
    }

    #[test]
    fn ends_of_the_text() {
        let text = "Alpha\nbeta";
        assert_eq!(at_line(text, 0), text);
        assert_eq!(at_line(text, 1), text, "cursor on the last line");
        assert_eq!(at_line(text, 99), text, "clamped");
        assert_eq!(at_line("Alpha\n", 1), "", "the empty last line");
        assert_eq!(at_line("A\r\nB\r\n\r\nC", 0), "A\r\nB", "CRLF");
    }

    #[test]
    fn each_list_item_is_a_paragraph() {
        let text = "Shopping:\n- milk\n  semi-skimmed\n- [ ] bread\n* eggs\n+ tea\n1. first\n\
                    12) second\n    - nested\n\nAfter";
        assert_eq!(
            at(text, "Shopping"),
            "Shopping:",
            "text above the first item"
        );
        assert_eq!(at(text, "milk"), "- milk\n  semi-skimmed");
        assert_eq!(
            at(text, "semi"),
            "- milk\n  semi-skimmed",
            "an item's next lines"
        );
        assert_eq!(at(text, "[ ]"), "- [ ] bread");
        assert_eq!(at(text, "eggs"), "* eggs");
        assert_eq!(at(text, "tea"), "+ tea");
        assert_eq!(at(text, "first"), "1. first");
        assert_eq!(at(text, "second"), "12) second");
        assert_eq!(at(text, "nested"), "    - nested");
        assert_eq!(at(text, "After"), "After");
    }

    #[test]
    fn not_list_items() {
        assert!(is_list_item("-"));
        assert!(is_list_item("- a\n"));
        assert!(is_list_item("\t1.\ta"));
        assert!(!is_list_item("---"));
        assert!(!is_list_item("-a"));
        assert!(!is_list_item("**bold**"));
        assert!(!is_list_item("1.5 litres"));
        assert!(!is_list_item("1234567890. ten digits"));
        assert!(!is_list_item("2026"));
        assert!(!is_list_item(""));
    }

    #[test]
    fn table_is_one_paragraph() {
        let text = "Intro\n\n| A | B |\n| --- | --- |\n| 1 | 2 |\n\nEnd";
        assert_eq!(at(text, "| 1"), "| A | B |\n| --- | --- |\n| 1 | 2 |");
    }

    /// The editor's lines drawn dimmed, by their text, in order (blank lines left out); each
    /// line is dimmed as a whole or not at all.
    fn dimmed(focus: &Focus) -> Vec<String> {
        focus.update();
        let buffer = &focus.buffer;
        let mut out = Vec::new();
        for n in 0..buffer.line_count() {
            let start = buffer.iter_at_line(n).expect("line");
            let mut end = start;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            let text = start.slice(&end).to_string();
            let mut tagged = Vec::new();
            let mut at = start;
            while at < end {
                tagged.push(at.has_tag(&focus.tag));
                at.forward_char();
            }
            if text.trim().is_empty() {
                continue;
            }
            assert!(
                tagged.iter().all(|&t| t == tagged[0]),
                "{text}: part dimmed"
            );
            if tagged[0] {
                out.push(text);
            }
        }
        out
    }

    /// The macOS editor tests on a GtkSourceView buffer; skipped without a display.
    #[test]
    fn dims_all_but_the_cursor_paragraph() {
        if gtk::init().is_err() {
            return;
        }
        let text = "First paragraph\nstill first\n\nSecond paragraph\n\nThird paragraph";
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_text(text);
        buffer.place_cursor(&buffer.start_iter());
        let focus = Focus::attach(&buffer);
        focus.set_colours(None, false);
        assert!(dimmed(&focus).is_empty(), "off by default");
        focus.set_on(true);
        assert_eq!(dimmed(&focus), ["Second paragraph", "Third paragraph"]);

        let offset = |s: &str| text[..text.find(s).expect(s)].chars().count() as i32;
        let place = |at: i32| buffer.place_cursor(&buffer.iter_at_offset(at));
        place(offset("Second") + 3);
        assert_eq!(
            dimmed(&focus),
            ["First paragraph", "still first", "Third paragraph"]
        );
        place(offset("Third"));
        assert_eq!(
            dimmed(&focus),
            ["First paragraph", "still first", "Second paragraph"]
        );
        place(offset("\n\nThird") + 1);
        assert_eq!(dimmed(&focus).len(), 4, "a blank line: everything dimmed");
        buffer.select_range(
            &buffer.iter_at_offset(3),
            &buffer.iter_at_offset(offset("Second")),
        );
        assert_eq!(
            dimmed(&focus),
            ["Third paragraph"],
            "a selection: its paragraphs"
        );

        // Typing follows the paragraph.
        place(offset("Second"));
        buffer.insert_at_cursor("New ");
        assert_eq!(
            dimmed(&focus),
            ["First paragraph", "still first", "Third paragraph"]
        );
        buffer.insert_at_cursor("\n");
        assert_eq!(
            dimmed(&focus),
            ["First paragraph", "still first", "Third paragraph"],
            "a new line in it"
        );
        buffer.insert_at_cursor("\n");
        assert_eq!(
            dimmed(&focus),
            ["First paragraph", "still first", "New ", "Third paragraph"],
            "a blank line splits it: the cursor is in the second half"
        );
        // Text inserted elsewhere (Replace All, undo) is dimmed; typing on a blank line isn't
        // (here it joins the paragraphs above and below).
        buffer.insert(&mut buffer.start_iter(), "My ");
        assert_eq!(
            dimmed(&focus),
            [
                "My First paragraph",
                "still first",
                "New ",
                "Third paragraph"
            ]
        );
        let (start, end) = buffer.bounds();
        let now = buffer.text(&start, &end, true);
        place(now[..now.find("\n\nThird").expect("blank")].chars().count() as i32 + 1);
        buffer.insert_at_cursor("Typed");
        assert_eq!(
            dimmed(&focus),
            ["My First paragraph", "still first", "New "]
        );
        // Another note.
        buffer.set_text("One\n\nTwo");
        place(0);
        assert_eq!(dimmed(&focus), ["Two"]);

        // The schemes' text colours (named palette colours in the system ones), faded.
        let manager = sourceview5::StyleSchemeManager::default();
        for id in [
            "Adwaita",
            "Adwaita-dark",
            "solarized-light",
            "solarized-dark",
        ] {
            focus.set_colours(manager.scheme(id).as_ref(), false);
            let colour = focus.tag.foreground_rgba().expect("colour");
            assert!((colour.alpha() - FADED_ALPHA).abs() < 0.01, "{id}");
            // Not the fallback (black or white): the scheme's own colour.
            assert!(colour.red() > 0.01 && colour.red() < 0.99, "{id}");
        }

        focus.set_on(false);
        assert!(dimmed(&focus).is_empty());
        focus.set_on(true);
        assert_eq!(dimmed(&focus), ["Two"]);
    }

    #[test]
    fn long_block_is_read_only_near_the_cursor() {
        let count = 3 * MAX_LINES;
        let middle = count / 2;
        assert_eq!(
            paragraph(middle, count, |_| "line"),
            Some((middle - MAX_LINES, middle + MAX_LINES))
        );
        let read = Cell::new(0);
        paragraph(middle, count, |_| {
            read.set(read.get() + 1);
            "line"
        });
        assert!(read.get() <= 2 * MAX_LINES + 1);
    }
}
