//! Focus Mode (main menu, Shift+Ctrl+J): the editor draws every line outside the cursor's
//! paragraph (`paragraph`) dimmed.
//!
//! Display only: the text, undo, saving and the preview are untouched. The editor view (`View`)
//! draws a veil of the scheme's background over the visible lines above and below the focused
//! paragraph, so dimmed text keeps its syntax colours (headings, links, code), faded, in every
//! style scheme, light or dark. Nothing is tagged: moving the cursor or typing costs reading the
//! paragraph, not the note, and drawing costs two rectangles a frame.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};

/// Lines read above and below the cursor's line, at most: the cost doesn't grow with the note.
pub(crate) const MAX_LINES: usize = 1000;

/// The opacity dimmed text keeps: about a tertiary label's contrast.
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

/// Focus Mode of one editor view.
pub(crate) struct Focus {
    buffer: sourceview5::Buffer,
    view: glib::WeakRef<View>,
    /// The focused paragraph (moves with edits): every line outside it is dimmed. Start: left
    /// gravity, end: right gravity, so typing at either end stays inside.
    start: gtk::TextMark,
    end: gtk::TextMark,
    on: Cell<bool>,
    /// An update is queued (`schedule`).
    pending: Cell<bool>,
    /// Drawn over the dimmed lines: the scheme's background at `1 - FADED_ALPHA`.
    veil: Cell<gdk::RGBA>,
}

impl Focus {
    /// Focus Mode for `view`, off; follows the cursor and edits once turned on.
    pub(crate) fn attach(view: &View) -> Rc<Focus> {
        let buffer = view
            .buffer()
            .downcast::<sourceview5::Buffer>()
            .expect("the editor's buffer is a GtkSourceBuffer");
        let at = buffer.start_iter();
        let focus = Rc::new(Focus {
            start: buffer.create_mark(None, &at, true),
            end: buffer.create_mark(None, &at, false),
            buffer: buffer.clone(),
            view: view.downgrade(),
            on: Cell::new(false),
            pending: Cell::new(false),
            veil: Cell::new(gdk::RGBA::WHITE),
        });
        focus.set_colours(None, false);
        view.imp().focus.replace(Rc::downgrade(&focus));
        let weak = Rc::downgrade(&focus);
        buffer.connect_mark_set(move |buffer, _, mark| {
            if (*mark == buffer.get_insert() || *mark == buffer.selection_bound())
                && let Some(focus) = weak.upgrade()
            {
                focus.schedule();
            }
        });
        let weak = Rc::downgrade(&focus);
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
        self.update();
        self.redraw();
    }

    /// The veil's colour: the scheme's background (`dark`: a dark grey without one, else
    /// white), at the opacity that leaves text `FADED_ALPHA` of its contrast.
    pub(crate) fn set_colours(&self, scheme: Option<&sourceview5::StyleScheme>, dark: bool) {
        let mut colour = scheme
            .and_then(|s| s.style("text"))
            .and_then(|s| s.background())
            .and_then(|c| gdk::RGBA::parse(c.as_str()).ok())
            .unwrap_or(if dark {
                gdk::RGBA::new(0.12, 0.12, 0.12, 1.0)
            } else {
                gdk::RGBA::WHITE
            });
        colour.set_alpha(1.0 - FADED_ALPHA);
        self.veil.set(colour);
        self.redraw();
    }

    fn redraw(&self) {
        if let Some(view) = self.view.upgrade() {
            view.queue_draw();
        }
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

    /// Moves the focused paragraph to the cursor's.
    pub(crate) fn update(&self) {
        if !self.on.get() {
            return;
        }
        let (start, end) = self.current();
        let (old_start, old_end) = self.bright();
        if (old_start.offset(), old_end.offset()) != (start.offset(), end.offset()) {
            self.buffer.move_mark(&self.start, &start);
            self.buffer.move_mark(&self.end, &end);
            self.redraw();
        }
    }

    /// The focused paragraph as last updated; empty on a blank line.
    fn bright(&self) -> (gtk::TextIter, gtk::TextIter) {
        (
            self.buffer.iter_at_mark(&self.start),
            self.buffer.iter_at_mark(&self.end),
        )
    }

    /// Veils the visible lines outside the focused paragraph (on a blank line: outside the
    /// cursor's line, so the cursor stays bright). `snapshot` is in buffer coordinates.
    fn draw(&self, view: &gtk::TextView, snapshot: &gtk::Snapshot) {
        if !self.on.get() {
            return;
        }
        let visible = view.visible_rect();
        let (top, bottom) = (visible.y(), visible.y() + visible.height());
        let (start, end) = self.bright();
        let first = view.line_yrange(&start).0;
        let (y, height) = view.line_yrange(&end);
        let last = y + height;
        let veil = self.veil.get();
        for (a, b) in [(top, first.min(bottom)), (last.max(top), bottom)] {
            if a < b {
                let area = graphene::Rect::new(
                    visible.x() as f32,
                    a as f32,
                    visible.width() as f32,
                    (b - a) as f32,
                );
                snapshot.append_color(&veil, &area);
            }
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

glib::wrapper! {
    /// The editor's GtkSourceView, drawing Focus Mode's veil above the text.
    pub struct View(ObjectSubclass<imp::View>)
        @extends sourceview5::View, gtk::TextView, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Scrollable;
}

impl View {
    pub(crate) fn new(buffer: &sourceview5::Buffer) -> View {
        glib::Object::builder().property("buffer", buffer).build()
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct View {
        pub(super) focus: RefCell<Weak<Focus>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for View {
        const NAME: &'static str = "MarkdownMateEditorView";
        type Type = super::View;
        type ParentType = sourceview5::View;
    }

    impl ObjectImpl for View {}
    impl WidgetImpl for View {}

    impl TextViewImpl for View {
        fn snapshot_layer(&self, layer: gtk::TextViewLayer, snapshot: gtk::Snapshot) {
            self.parent_snapshot_layer(layer, snapshot.clone());
            if layer == gtk::TextViewLayer::AboveText
                && let Some(focus) = self.focus.borrow().upgrade()
            {
                focus.draw(self.obj().upcast_ref(), &snapshot);
            }
        }
    }

    impl sourceview5::subclass::prelude::ViewImpl for View {}
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

    /// The editor's lines drawn dimmed, by their text, in order (blank lines left out).
    fn dimmed(focus: &Focus) -> Vec<String> {
        focus.update();
        if !focus.on.get() {
            return Vec::new();
        }
        let buffer = &focus.buffer;
        let (start, end) = focus.bright();
        let (first, last) = (start.line(), end.line());
        let mut out = Vec::new();
        for n in (0..buffer.line_count()).filter(|n| *n < first || *n > last) {
            let start = buffer.iter_at_line(n).expect("line");
            let mut end = start;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            let text = start.slice(&end).to_string();
            if !text.trim().is_empty() {
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
        let view = View::new(&buffer);
        let focus = Focus::attach(&view);
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

        // The schemes' backgrounds (named palette colours in the system ones), as the veil.
        let manager = sourceview5::StyleSchemeManager::default();
        for id in [
            "Adwaita",
            "Adwaita-dark",
            "solarized-light",
            "solarized-dark",
        ] {
            focus.set_colours(manager.scheme(id).as_ref(), false);
            let colour = focus.veil.get();
            assert!((colour.alpha() - (1.0 - FADED_ALPHA)).abs() < 0.01, "{id}");
            // Not the fallback (white): the scheme's own colour.
            assert!(colour.red() < 0.995 || colour.blue() < 0.995, "{id}");
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
