use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib::{self, clone};
use sourceview5::prelude::*;

use crate::editor::Editor;
use crate::{complete, format, images, lines, lists, paste, relink, render, table};

/// What the completion list offers.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Note names after `[[` (closed with `]]`).
    Note,
    /// Headings after `[[Note#` / `[[#` (closed with `]]`).
    Heading,
    /// `#tag`s (inserted as is).
    Tag,
    /// Code fence languages (inserted as is).
    Fence,
}

/// `[[` note-name, `[[Note#` heading, `#tag` and code fence language completion
/// (`complete.rs`): a list floating over the editor (an overlay, not a
/// popup window, so it never takes focus — typing and input methods stay in the editor); its
/// keys are handled in `smart_lists`. Hand-made rather than GtkSourceView's completion: that
/// needs two GObject subclasses and replaces only the word at the cursor, while names here
/// hold `/` and spaces.
pub(crate) struct Completer {
    buffer: gtk::TextBuffer,
    view: sourceview5::View,
    overlay: gtk::Overlay,
    pub(crate) panel: gtk::Frame,
    list: gtk::ListBox,
    /// The rows shown (label, text to insert), and the char offset where the query starts.
    names: RefCell<Vec<(String, String)>>,
    start: Cell<i32>,
    kind: Cell<Kind>,
    /// Query start closed with Esc: stays closed until the cursor leaves that link.
    dismissed: Cell<Option<i32>>,
}

impl Completer {
    pub(crate) fn new(view: &sourceview5::View, overlay: &gtk::Overlay) -> Rc<Self> {
        let list = gtk::ListBox::builder().focusable(false).build();
        let panel = gtk::Frame::builder()
            .child(&list)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .css_classes(["card"])
            .visible(false)
            .build();
        overlay.add_overlay(&panel);
        let c = Rc::new(Self {
            buffer: view.buffer(),
            view: view.clone(),
            overlay: overlay.clone(),
            panel,
            list,
            names: RefCell::default(),
            start: Cell::new(0),
            kind: Cell::new(Kind::Note),
            dismissed: Cell::new(None),
        });
        c.list.connect_row_activated(clone!(
            #[weak]
            c,
            move |_, row| c.accept(row.index())
        ));
        // Typing scrolls the view after `update` ran: follow it.
        if let Some(adj) = view.vadjustment() {
            adj.connect_value_changed(clone!(
                #[weak]
                c,
                move |_| c.place_later()
            ));
        }
        c
    }

    /// Shows the notes matching the `[[query` (the headings matching `[[Note#query`, the tags
    /// matching `#query`, the languages matching ```` ```query ````) at the cursor, or hides
    /// the list.
    pub(crate) fn update(self: &Rc<Self>, editor: &Editor) {
        let buffer = &self.buffer;
        let cursor = buffer.iter_at_mark(&buffer.get_insert());
        let mut line_start = cursor;
        line_start.set_line_offset(0);
        let line = buffer.text(&line_start, &cursor, false);
        let root = editor.root.borrow().clone();
        let found = (|| {
            if !editor.view.has_focus()
                || buffer.has_selection()
                || !line.contains(['[', '#', '`', '~'])
            {
                return None;
            }
            let above = buffer.text(&buffer.start_iter(), &line_start, false);
            if let Some(query) = complete::fence_query(&above, &line) {
                return Some((query.to_string(), Kind::Fence, String::new()));
            }
            if let Some(query) = complete::query(&above, &line) {
                return Some(match complete::heading_query(query) {
                    Some((note, q)) => (q.to_string(), Kind::Heading, note.to_string()),
                    None => (query.to_string(), Kind::Note, String::new()),
                });
            }
            complete::tag_query(&above, &line).map(|q| (q.to_string(), Kind::Tag, String::new()))
        })();
        // Other notes need the open folder; this note's headings and tags do not.
        let Some((query, kind, note)) = found
            .filter(|(_, kind, note)| root.is_some() || (*kind != Kind::Note && note.is_empty()))
        else {
            self.dismissed.set(None);
            self.panel.set_visible(false);
            return;
        };
        let start = cursor.offset() - query.chars().count() as i32;
        let same = |s: &str| (s.to_string(), s.to_string());
        let names: Vec<(String, String)> = match kind {
            Kind::Tag => complete::tag_suggestions(&query, &editor.tag_index.borrow())
                .iter()
                .map(|t| same(t))
                .collect(),
            Kind::Fence => complete::fence_suggestions(&query)
                .into_iter()
                .map(same)
                .collect(),
            Kind::Note => {
                let (notes, root) = (editor.note_list.borrow().clone(), root.unwrap_or_default());
                let aliases = editor.aliases.borrow().clone();
                let current = editor.path.borrow().clone();
                complete::note_suggestions(&query, &notes, &aliases, &root, current.as_deref())
            }
            Kind::Heading => {
                let text = self.heading_source(editor, &note, root.as_deref());
                let headings = render::heading_targets(&text.unwrap_or_default());
                complete::heading_suggestions(&query, &headings)
                    .into_iter()
                    .map(|h| {
                        let indent = "  ".repeat(usize::from(h.level.saturating_sub(1)));
                        (format!("{indent}{}", h.title), h.fragment.clone())
                    })
                    .collect()
            }
        };
        if self.dismissed.get() == Some(start) || names.is_empty() {
            self.panel.set_visible(false);
            return;
        }
        while let Some(row) = self.list.row_at_index(0) {
            self.list.remove(&row);
        }
        for (label, _) in &names {
            let label = gtk::Label::builder().label(label).xalign(0.0).build();
            let row = gtk::ListBoxRow::builder()
                .child(&label)
                .focusable(false)
                .build();
            self.list.append(&row);
        }
        self.list.select_row(self.list.row_at_index(0).as_ref());
        self.names.replace(names);
        self.start.set(start);
        self.kind.set(kind);
        self.panel.set_visible(true);
        self.place_later();
    }

    /// The Markdown whose headings `[[note#` offers: the editor's text for `[[#` or a link to
    /// the open note, else the linked note's file (None if it does not resolve or read).
    fn heading_source(
        &self,
        editor: &Editor,
        note: &str,
        root: Option<&std::path::Path>,
    ) -> Option<String> {
        let current = editor.path.borrow().clone();
        let path = if note.trim().is_empty() {
            None
        } else {
            let notes = editor.note_list.borrow().clone();
            let aliases = editor.aliases.borrow().clone();
            let path =
                render::resolve_wiki_with(note, &notes, &aliases, root?, current.as_deref())?
                    .clone();
            Some(path).filter(|p| current.as_ref() != Some(p))
        };
        match path {
            Some(path) => std::fs::read_to_string(path).ok(),
            None => Some(
                self.buffer
                    .text(&self.buffer.start_iter(), &self.buffer.end_iter(), true)
                    .into(),
            ),
        }
    }

    /// `place` once the view has scrolled to the cursor (it does that after the text change).
    pub(crate) fn place_later(self: &Rc<Self>) {
        glib::idle_add_local_once(clone!(
            #[weak(rename_to = c)]
            self,
            move || {
                if c.panel.is_visible() {
                    c.place();
                }
            }
        ));
    }

    /// Puts the list under the cursor line, or above it when it would run off the bottom.
    pub(crate) fn place(&self) {
        let view = &self.view;
        let rect = view.iter_location(&self.buffer.iter_at_mark(&self.buffer.get_insert()));
        let (x, y) = view.buffer_to_window_coords(gtk::TextWindowType::Widget, rect.x(), rect.y());
        let point = gtk::graphene::Point::new(x as f32, y as f32);
        let Some(p) = view.compute_point(&self.overlay, &point) else {
            return;
        };
        let height = self.panel.measure(gtk::Orientation::Vertical, -1).1;
        let below = p.y() as i32 + rect.height();
        let top = if below + height > self.overlay.height() {
            p.y() as i32 - height
        } else {
            below
        };
        self.panel.set_margin_start(p.x().max(0.0) as i32);
        self.panel.set_margin_top(top.max(0));
    }

    /// Up/Down/Enter/Tab/Esc while the list is shown; false for any other key.
    pub(crate) fn key(&self, key: gtk::gdk::Key) -> bool {
        use gtk::gdk::Key;
        if !self.panel.is_visible() {
            return false;
        }
        let i = self.list.selected_row().map_or(0, |r| r.index());
        let last = self.names.borrow().len() as i32 - 1;
        match key {
            Key::Up | Key::Down => {
                let i = if key == Key::Up { i - 1 } else { i + 1 };
                let row = self.list.row_at_index(i.clamp(0, last));
                self.list.select_row(row.as_ref());
            }
            Key::Return | Key::KP_Enter | Key::Tab => self.accept(i),
            Key::Escape => {
                self.dismissed.set(Some(self.start.get()));
                self.panel.set_visible(false);
            }
            _ => return false,
        }
        true
    }

    /// Replaces the query with suggestion `i`; a link is closed (`complete::insertion`).
    pub(crate) fn accept(&self, i: i32) {
        let Some((_, name)) = self.names.borrow().get(i as usize).cloned() else {
            return;
        };
        let buffer = &self.buffer;
        let mut cursor = buffer.iter_at_mark(&buffer.get_insert());
        let mut end = cursor;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        let after = buffer.text(&cursor, &end, false);
        let link = matches!(self.kind.get(), Kind::Note | Kind::Heading);
        let text = if link {
            complete::insertion(&name, &after)
        } else {
            name.clone()
        };
        self.panel.set_visible(false);
        if self.kind.get() == Kind::Fence {
            // `cs` would still offer `css`: keep the list closed so Enter starts the block.
            self.dismissed.set(Some(self.start.get()));
        }
        buffer.begin_user_action();
        buffer.delete(&mut buffer.iter_at_offset(self.start.get()), &mut cursor);
        buffer.insert_at_cursor(&text);
        if text == name && link {
            // `]]` was already there: step over it.
            let mut past = buffer.iter_at_mark(&buffer.get_insert());
            past.forward_chars(2);
            buffer.place_cursor(&past);
        }
        buffer.end_user_action();
    }
}

/// Smart lists (`lists.rs`): Enter continues / ends a list item, Tab / Shift+Tab indent list
/// lines; in a table (`table.rs`) they move between cells / add a row instead; Ctrl+B / Ctrl+I / Ctrl+K format and `*` `_` `` ` `` `~` `[` `(` `"` wrap a selection
/// (`format.rs`); Alt+Ctrl+Enter / Backspace (with or without Shift) run the table commands;
/// Ctrl+Enter, Alt+Ctrl+Up / Down (or Alt+Up / Down, taking over GtkSourceView's
/// own move-lines keys so a move is one undo step) and Shift+Ctrl+D run the line commands
/// (`lines.rs`); Alt+Left / Right are Back / Forward, not GtkSourceView's move-words; the
/// `[[` suggestion keys go first. Capture phase so it runs before the view inserts the newline
/// / tab itself. On the view, so only while the editor has focus.
pub(crate) fn smart_lists(view: &sourceview5::View, completer: &Rc<Completer>) {
    // Enter while an input method is composing must commit the text, not touch the list.
    let preedit = Rc::new(Cell::new(false));
    view.connect_preedit_changed(clone!(
        #[strong]
        preedit,
        move |_, s| preedit.set(!s.is_empty())
    ));
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let buffer = view.buffer();
    let completer = completer.clone();
    keys.connect_key_pressed(move |keys, key, _, mods| {
        use gtk::gdk::{Key, ModifierType as M};
        let mods = mods & (M::SHIFT_MASK | M::CONTROL_MASK | M::ALT_MASK | M::SUPER_MASK);
        let handled = match key {
            _ if mods.is_empty() && !preedit.get() && completer.key(key) => true,
            Key::Return | Key::KP_Enter if mods.is_empty() && !preedit.get() => {
                table_key(&buffer, table::Key::Enter) || list_enter(&buffer)
            }
            Key::Tab if mods.is_empty() => {
                (!preedit.get() && table_key(&buffer, table::Key::Tab))
                    || list_shift(&buffer, false, indent(keys))
            }
            Key::ISO_Left_Tab | Key::Tab if mods == M::SHIFT_MASK => {
                (!preedit.get() && table_key(&buffer, table::Key::BackTab))
                    || list_shift(&buffer, true, indent(keys))
            }
            // Plain-text paste: GTK's own, without HTML → Markdown, URL links or images.
            _ if mods == M::SHIFT_MASK | M::CONTROL_MASK && key.to_lower() == Key::v => {
                let view = keys.widget().and_downcast::<gtk::TextView>();
                if let Some(view) = view {
                    buffer.paste_clipboard(&view.clipboard(), None, view.is_editable());
                }
                true
            }
            // Line commands (`lines.rs`), here rather than window accelerators so they act
            // only in the editor: Ctrl+Enter stays Replace All in the Replace box.
            Key::Return | Key::KP_Enter if mods == M::CONTROL_MASK && !preedit.get() => {
                line_key(keys, LineCommand::ToggleTask)
            }
            Key::Up | Key::Down
                if (mods == M::ALT_MASK || mods == M::ALT_MASK | M::CONTROL_MASK)
                    && !preedit.get() =>
            {
                line_key(keys, LineCommand::Move { up: key == Key::Up })
            }
            // Back / Forward (window actions): GtkSourceView would move the word instead.
            Key::Left | Key::Right if mods == M::ALT_MASK && !preedit.get() => {
                let name = if key == Key::Left {
                    "win.go-back"
                } else {
                    "win.go-forward"
                };
                if let Some(view) = keys.widget() {
                    let _ = view.activate_action(name, None);
                }
                true
            }
            _ if mods == M::SHIFT_MASK | M::CONTROL_MASK && key.to_lower() == Key::d => {
                line_key(keys, LineCommand::Duplicate)
            }
            // Table commands (`table.rs`): Alt+Ctrl+Enter adds a row below, Shift for above;
            // Alt+Ctrl+Backspace deletes the row, Shift the column. Editor only, like the
            // line commands; outside a table they ring the bell.
            Key::Return | Key::KP_Enter
                if mods & !M::SHIFT_MASK == M::ALT_MASK | M::CONTROL_MASK && !preedit.get() =>
            {
                table_shortcut(
                    keys,
                    if mods.contains(M::SHIFT_MASK) {
                        table::Command::AddRowAbove
                    } else {
                        table::Command::AddRowBelow
                    },
                )
            }
            Key::BackSpace if mods & !M::SHIFT_MASK == M::ALT_MASK | M::CONTROL_MASK => {
                table_shortcut(
                    keys,
                    if mods.contains(M::SHIFT_MASK) {
                        table::Command::DeleteColumn
                    } else {
                        table::Command::DeleteRow
                    },
                )
            }
            _ if mods == M::CONTROL_MASK => match key.to_lower() {
                Key::b => {
                    format_selection(&buffer, |t, s| format::emphasis(t, s, format::Style::Bold))
                }
                Key::i => format_selection(&buffer, |t, s| {
                    format::emphasis(t, s, format::Style::Italic)
                }),
                Key::k => format_selection(&buffer, format::link),
                _ => false,
            },
            // Highlight: Ctrl+H is Find and Replace, so Shift+Ctrl+H (Typora's key).
            _ if mods == M::SHIFT_MASK | M::CONTROL_MASK && key.to_lower() == Key::h => {
                format_selection(&buffer, |t, s| format::toggle(t, s, "=="))
            }
            _ if (mods.is_empty() || mods == M::SHIFT_MASK)
                && !preedit.get()
                && key.to_unicode().is_some_and(|c| wrap_selection(&buffer, c)) =>
            {
                true
            }
            _ => false,
        };
        if handled {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    view.add_controller(keys);
}

/// A table command's shortcut in the editor: rings the bell when there is nothing to do.
fn table_shortcut(keys: &gtk::EventControllerKey, command: table::Command) -> bool {
    if let Some(view) = keys.widget().and_downcast::<gtk::TextView>() {
        if table_command(&view.buffer(), command) {
            view.scroll_mark_onscreen(&view.buffer().get_insert());
        } else {
            view.error_bell();
        }
    }
    true
}

/// The editor's line commands (`lines.rs`).
#[derive(Clone, Copy)]
pub(crate) enum LineCommand {
    ToggleTask,
    Footnote,
    Move {
        up: bool,
    },
    Duplicate,
    /// Format bar: Heading, the list / quote prefixes, Code Block.
    Heading,
    Prefix(lists::Kind),
    CodeBlock,
}

/// A line command's shortcut in the editor: rings the bell when there is nothing to do.
fn line_key(keys: &gtk::EventControllerKey, command: LineCommand) -> bool {
    if let Some(view) = keys.widget().and_downcast::<gtk::TextView>()
        && !line_command(&view, command)
    {
        view.error_bell();
    }
    true
}

/// Runs a line command on the view's buffer as one undo step and keeps the cursor in sight.
/// False (nothing done) when there is nothing to do: no task on the lines, a footnote inside
/// a code block, the first / last line, the empty line after a final newline, a format
/// toggle on code only.
pub(crate) fn line_command(view: &gtk::TextView, command: LineCommand) -> bool {
    let buffer = view.buffer();
    let insert = buffer.iter_at_mark(&buffer.get_insert()).offset() as usize;
    let bound = buffer.iter_at_mark(&buffer.selection_bound()).offset() as usize;
    let sel = (insert.min(bound), insert.max(bound));
    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
    let at = |n: usize| buffer.iter_at_offset(n as i32);
    // The selection keeps its direction (which end the cursor is at).
    let select = |(s, e): (usize, usize)| {
        if insert < bound {
            buffer.select_range(&at(s), &at(e));
        } else {
            buffer.select_range(&at(e), &at(s));
        }
    };
    let replace = |r: Option<lines::Replace>| {
        let Some(r) = r else { return false };
        buffer.begin_user_action();
        buffer.delete(&mut at(r.start), &mut at(r.end));
        buffer.insert(&mut at(r.start), &r.text);
        select(r.select);
        buffer.end_user_action();
        true
    };
    let done = match command {
        LineCommand::ToggleTask => {
            let Some((marks, mark)) = lines::toggle_tasks(&text, sel) else {
                return false;
            };
            buffer.begin_user_action();
            for m in marks {
                buffer.delete(&mut at(m), &mut at(m + 1));
                buffer.insert(&mut at(m), &mark.to_string());
            }
            select(sel);
            buffer.end_user_action();
            true
        }
        LineCommand::Footnote => {
            let Some(f) = lines::footnote(&text, sel) else {
                return false;
            };
            buffer.begin_user_action();
            buffer.insert(&mut buffer.end_iter(), &f.definition);
            buffer.insert(&mut at(f.at), &f.reference);
            buffer.place_cursor(&at(f.cursor));
            buffer.end_user_action();
            true
        }
        LineCommand::Move { up } => replace(lines::move_lines(&text, sel, up)),
        LineCommand::Duplicate => replace(lines::duplicate_lines(&text, sel)),
        LineCommand::Heading => replace(lines::cycle_heading(&text, sel)),
        LineCommand::Prefix(kind) => replace(lines::toggle_prefix(&text, sel, kind)),
        LineCommand::CodeBlock => replace(lines::toggle_code_block(&text, sel)),
    };
    if done {
        view.scroll_mark_onscreen(&buffer.get_insert());
    }
    done
}

/// Paste (clipboard with an image and no plain text) and drop (image files) put images into
/// the open note's `assets/` and insert a link (`images.rs`); other files and folders dropped
/// from Files or the sidebar are linked where they are (`insert_dropped`). Text pasted over a one-line
/// selection outside code: a URL makes `[text](url)` (`format::paste_link`). Rich text
/// (`text/html` without an image) outside code becomes Markdown (`paste.rs`). Anything else:
/// GTK's own handling.
pub(crate) fn image_input(view: &sourceview5::View, editor: &Rc<Editor>) {
    view.connect_paste_clipboard(clone!(
        #[weak]
        editor,
        move |view| {
            let clipboard = view.clipboard();
            let formats = clipboard.formats();
            let mime = |p: &str| formats.mime_types().iter().any(|m| m.starts_with(p));
            let image = formats.contains_type(gtk::gdk::Texture::static_type()) || mime("image/");
            let text = formats.contains_type(glib::Type::STRING) || mime("text/plain");
            // Local: our own Copy as HTML, whose text/plain already is the note's Markdown.
            let html = !image && !clipboard.is_local() && mime("text/html");
            if text || html {
                paste_text(view, clipboard, html);
                return;
            }
            if !image || editor.path.borrow().is_none() {
                return;
            }
            view.stop_signal_emission_by_name("paste-clipboard");
            glib::spawn_future_local(async move {
                match clipboard.read_texture_future().await {
                    Ok(Some(texture)) => editor.insert_image(|note| {
                        images::store(note, "image.png", |p| {
                            std::fs::write(p, texture.save_to_png_bytes())
                        })
                    }),
                    _ => editor.toast("Cannot read the image from the clipboard"),
                }
            });
        }
    ));

    let drop = gtk::DropTarget::new(
        gtk::gdk::FileList::static_type(),
        gtk::gdk::DragAction::COPY,
    );
    // Capture: before the text view's own drop handling.
    drop.set_propagation_phase(gtk::PropagationPhase::Capture);
    drop.connect_drop(clone!(
        #[weak]
        editor,
        #[upgrade_or]
        false,
        move |target, value, x, y| {
            let Ok(files) = value.get::<gtk::gdk::FileList>() else {
                return false;
            };
            let paths: Vec<PathBuf> = files.files().iter().filter_map(|f| f.path()).collect();
            if paths.is_empty() || editor.path.borrow().is_none() {
                return false;
            }
            place_drop(target.widget().as_ref(), &editor, x, y);
            insert_dropped(&editor, &paths);
            true
        }
    ));
    view.add_controller(drop);

    // An entry dragged from the sidebar (its private type, a move there): linked here.
    let entry = gtk::DropTargetAsync::new(
        Some(gtk::gdk::ContentFormats::new(&[crate::sidebar::DRAG_MIME])),
        gtk::gdk::DragAction::MOVE,
    );
    entry.set_propagation_phase(gtk::PropagationPhase::Capture);
    entry.connect_drop(clone!(
        #[weak]
        editor,
        #[upgrade_or]
        false,
        move |target, drop, x, y| {
            if editor.path.borrow().is_none() {
                return false;
            }
            place_drop(target.widget().as_ref(), &editor, x, y);
            let drop = drop.clone();
            glib::spawn_future_local(async move {
                let path = crate::sidebar::read_path(&drop).await;
                // Nothing moves: the sidebar ignores how its drag ended.
                drop.finish(if path.is_some() {
                    gtk::gdk::DragAction::MOVE
                } else {
                    gtk::gdk::DragAction::empty()
                });
                if let Some(path) = path {
                    insert_dropped(&editor, &[path]);
                }
            });
            true
        }
    ));
    view.add_controller(entry);
}

/// Puts the cursor where a drop at (x, y) of the editor `widget` lands: past the end of the
/// line when right of its text, at the end of the note when below it.
fn place_drop(widget: Option<&gtk::Widget>, editor: &Editor, x: f64, y: f64) {
    if let Some(view) = widget.and_then(|w| w.downcast_ref::<gtk::TextView>()) {
        let (bx, by) =
            view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let iter = view.iter_at_location(bx, by).unwrap_or_else(|| {
            let (mut iter, _) = view.line_at_y(by);
            if !iter.ends_line() {
                iter.forward_to_line_end();
            }
            iter
        });
        editor.buffer.place_cursor(&iter);
    }
}

/// Dropped files at the cursor, one per line, as one undo step: images copied into the
/// note's `assets/` (`images.rs`), other files and folders as relative links
/// (`relink::dropped_links`).
fn insert_dropped(editor: &Editor, paths: &[PathBuf]) {
    let Some(note) = editor.path.borrow().clone() else {
        return;
    };
    let root = editor.root.borrow().clone();
    let mut lines = vec![];
    for path in paths {
        if images::is_image(path) && path.is_file() {
            match images::store_file(&note, path) {
                Ok(link) => lines.push(link),
                Err(e) => editor.toast(&format!("Cannot add image: {e}")),
            }
        } else {
            // Written as the sidebar lists it when it is under the open folder's real path.
            let path = root
                .as_deref()
                .map_or_else(|| path.clone(), |r| relink::rebase(path, r));
            let is_dir = path.is_dir();
            lines.push(relink::dropped_links(&[(path, is_dir)], note.parent()));
        }
    }
    if lines.is_empty() {
        return;
    }
    let buffer = &editor.buffer;
    buffer.begin_user_action();
    buffer.delete_selection(true, true);
    buffer.insert_at_cursor(&lines.join("\n"));
    buffer.end_user_action();
    editor.view.grab_focus();
}

/// Text paste outside code (`html`: HTML on the clipboard): the
/// clipboard is read (async) first, then a URL over the selection becomes `[text](url)`, HTML
/// becomes Markdown, tab-separated plain text (spreadsheet cells) a table (`paste::tsv_to_table`;
/// one undo step each), and anything else is pasted as GTK would.
fn paste_text(view: &sourceview5::View, clipboard: gtk::gdk::Clipboard, html: bool) {
    let buffer = view.buffer();
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let (a, b) = buffer.selection_bounds().unwrap_or((cursor, cursor));
    if in_code_block(&buffer, &line_start(a)) {
        return;
    }
    let link = buffer.has_selection() && a.line() == b.line();
    view.stop_signal_emission_by_name("paste-clipboard");
    let editable = view.is_editable();
    let view = view.clone();
    glib::spawn_future_local(async move {
        if link {
            let pasted = clipboard.read_text_future().await.ok().flatten();
            let edit = pasted.as_deref().and_then(format::url).and_then(|url| {
                let (a, b) = buffer.selection_bounds()?;
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
                format::paste_link(&text, (a.offset() as usize, b.offset() as usize), url)
            });
            if let Some(edit) = edit {
                return apply_edit(&buffer, edit);
            }
        }
        let markdown = match html {
            true => read_html(&clipboard).await,
            false => None,
        };
        let mut md = markdown.as_deref().and_then(paste::html_to_markdown);
        if md.is_none() {
            let text = clipboard.read_text_future().await.ok().flatten();
            md = text.as_deref().and_then(paste::tsv_to_table).map(|table| {
                // A table is a block: on its own lines.
                let (a, b) = buffer.selection_bounds().unwrap_or_else(|| {
                    let c = buffer.iter_at_mark(&buffer.get_insert());
                    (c, c)
                });
                let before = buffer.text(&line_start(a), &a, false);
                let mut after_end = b;
                if !after_end.ends_line() {
                    after_end.forward_to_line_end();
                }
                let after = buffer.text(&b, &after_end, false);
                let head = if before.trim().is_empty() { "" } else { "\n\n" };
                let tail = if after.trim().is_empty() { "" } else { "\n\n" };
                format!("{head}{table}{tail}")
            });
        }
        match md {
            Some(md) => {
                buffer.begin_user_action();
                buffer.delete_selection(true, editable);
                buffer.insert_interactive_at_cursor(&md, editable);
                buffer.end_user_action();
                view.scroll_mark_onscreen(&buffer.get_insert());
            }
            None => buffer.paste_clipboard(&clipboard, None, editable),
        }
    });
}

/// The clipboard's `text/html`, decoded (`paste::decode`); None if it cannot be read.
async fn read_html(clipboard: &gtk::gdk::Clipboard) -> Option<String> {
    let (stream, _) = clipboard
        .read_future(&["text/html"], glib::Priority::DEFAULT)
        .await
        .ok()?;
    let mut bytes = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await
            .ok()?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(paste::decode(&bytes))
}

/// Typed `c` over a selection outside code wraps it (`format::wrap`); false: type as usual.
fn wrap_selection(buffer: &gtk::TextBuffer, c: char) -> bool {
    let Some((a, b)) = buffer.selection_bounds() else {
        return false;
    };
    let Some(edit) = format::wrap((a.offset() as usize, b.offset() as usize), c) else {
        return false;
    };
    if in_code_block(buffer, &line_start(a)) {
        return false;
    }
    apply_edit(buffer, edit);
    true
}

/// Applies a `format.rs` edit as one undo step and selects what it says.
pub(crate) fn format_selection(
    buffer: &gtk::TextBuffer,
    f: impl Fn(&str, (usize, usize)) -> format::Edit,
) -> bool {
    let (a, b) = buffer.selection_bounds().unwrap_or_else(|| {
        let c = buffer.iter_at_mark(&buffer.get_insert());
        (c, c)
    });
    let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
    apply_edit(buffer, f(&text, (a.offset() as usize, b.offset() as usize)));
    true
}

fn apply_edit(buffer: &gtk::TextBuffer, edit: format::Edit) {
    let at = |n: usize| buffer.iter_at_offset(n as i32);
    buffer.begin_user_action();
    // Closing mark first, so the opening mark's offsets still hold.
    for (start, end, insert) in [edit.after, edit.before] {
        buffer.delete(&mut at(start), &mut at(end));
        buffer.insert(&mut at(start), &insert);
    }
    buffer.select_range(&at(edit.select.1), &at(edit.select.0));
    buffer.end_user_action();
}

fn line_start(mut iter: gtk::TextIter) -> gtk::TextIter {
    iter.set_line_offset(0);
    iter
}

pub(crate) fn in_code_block(buffer: &gtk::TextBuffer, line_start: &gtk::TextIter) -> bool {
    lists::in_fence(&buffer.text(&buffer.start_iter(), line_start, false))
}

/// Enter on a list line. With a selection, the default (replace it with a newline) is kept.
pub(crate) fn list_enter(buffer: &gtk::TextBuffer) -> bool {
    if buffer.has_selection() {
        return false;
    }
    let cursor = buffer.iter_at_mark(&buffer.get_insert());
    let mut start = cursor;
    start.set_line_offset(0);
    let mut end = cursor;
    if !end.ends_line() {
        end.forward_to_line_end();
    }
    let before = buffer.text(&start, &cursor, false);
    let after = buffer.text(&cursor, &end, false);
    let Some(action) = lists::on_enter(&before, &after) else {
        return false;
    };
    if in_code_block(buffer, &start) {
        return false;
    }
    buffer.begin_user_action();
    match action {
        lists::Enter::Continue(prefix) => buffer.insert_at_cursor(&format!("\n{prefix}")),
        lists::Enter::Exit => buffer.delete(&mut start, &mut end),
    }
    buffer.end_user_action();
    true
}

fn line_text(buffer: &gtk::TextBuffer, n: i32) -> Option<String> {
    let start = buffer.iter_at_line(n)?;
    let mut end = start;
    if !end.ends_line() {
        end.forward_to_line_end();
    }
    Some(buffer.text(&start, &end, false).to_string())
}

/// The consecutive lines containing `|` around the cursor (or the selection's start, when it
/// stays on one line), the first one's line number, and the cursor's row and column in them.
/// None when the selection spans lines or the cursor's line has no `|`.
fn table_lines(buffer: &gtk::TextBuffer) -> Option<(i32, Vec<String>, usize, usize)> {
    let (a, b) = buffer.selection_bounds().unwrap_or_else(|| {
        let c = buffer.iter_at_mark(&buffer.get_insert());
        (c, c)
    });
    if a.line() != b.line() {
        return None;
    }
    let piped = |n: i32| line_text(buffer, n).filter(|l| l.contains('|'));
    let row = a.line();
    piped(row)?;
    let mut top = row;
    while top > 0 && piped(top - 1).is_some() {
        top -= 1;
    }
    let lines: Vec<String> = (top..).map_while(piped).collect();
    Some((top, lines, (row - top) as usize, a.line_offset() as usize))
}

/// Replaces the table at the cursor with `f`'s result (`table.rs`, outside code) as one undo
/// step and selects its `select` range (backwards: the cursor at its start). False when `f`
/// has nothing to do.
fn table_change(
    buffer: &gtk::TextBuffer,
    f: impl FnOnce(&[&str], usize, usize) -> Option<table::Edit>,
) -> bool {
    let Some((top, lines, row, col)) = table_lines(buffer) else {
        return false;
    };
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let Some(edit) = f(&refs, row, col) else {
        return false;
    };
    let (Some(mut start), Some(mut end)) = (
        buffer.iter_at_line(top + edit.first as i32),
        buffer.iter_at_line(top + lines.len() as i32 - 1),
    ) else {
        return false;
    };
    if in_code_block(buffer, &start) {
        return false;
    }
    if !end.ends_line() {
        end.forward_to_line_end();
    }
    let base = start.offset();
    let at = |n: usize| buffer.iter_at_offset(base + n as i32);
    buffer.begin_user_action();
    buffer.delete(&mut start, &mut end);
    buffer.insert(&mut start, &edit.text);
    buffer.select_range(&at(edit.select.1), &at(edit.select.0));
    buffer.end_user_action();
    true
}

/// Tab / Shift+Tab / Enter in a GFM table outside code (`table.rs`): move between cells or
/// add / leave a row, realigning the table, as one undo step. A selection is fine if it stays
/// on one line (Tab selects the next cell's text). False outside a table.
pub(crate) fn table_key(buffer: &gtk::TextBuffer, key: table::Key) -> bool {
    table_change(buffer, |lines, row, col| table::edit(lines, row, col, key))
}

/// A table command (Table menu, its shortcut, the table bar) on the table at the cursor, one
/// undo step, the cursor left in its cell. False outside a table or with nothing to do (Delete
/// Row on the header, Delete Column on the only column).
pub(crate) fn table_command(buffer: &gtk::TextBuffer, command: table::Command) -> bool {
    table_change(buffer, |lines, row, col| {
        table::command(lines, row, col, command)
    })
}

/// Whether the cursor is in a GFM table outside code: the table bar shows only then.
pub(crate) fn in_table(buffer: &gtk::TextBuffer) -> bool {
    let Some((top, lines, row, col)) = table_lines(buffer) else {
        return false;
    };
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    table::contains(&refs, row, col)
        && buffer
            .iter_at_line(top)
            .is_some_and(|start| !in_code_block(buffer, &start))
}

/// Insert Table: an empty 3-column table at the cursor's line (`table::skeleton`), first
/// header cell selected, one undo step. False (nothing done) inside a code block.
pub(crate) fn insert_table(buffer: &gtk::TextBuffer) -> bool {
    let row = buffer.iter_at_mark(&buffer.get_insert()).line();
    let Some(mut start) = buffer.iter_at_line(row) else {
        return false;
    };
    if in_code_block(buffer, &start) {
        return false;
    }
    let line = line_text(buffer, row).unwrap_or_default();
    let above = (row > 0).then(|| line_text(buffer, row - 1)).flatten();
    let edit = table::skeleton(
        above.as_deref(),
        &line,
        line_text(buffer, row + 1).as_deref(),
    );
    let mut end = start;
    if !end.ends_line() {
        end.forward_to_line_end();
    }
    let base = start.offset();
    buffer.begin_user_action();
    buffer.delete(&mut start, &mut end);
    buffer.insert(&mut start, &edit.text);
    let at = |n: usize| buffer.iter_at_offset(base + n as i32);
    buffer.select_range(&at(edit.select.1), &at(edit.select.0));
    buffer.end_user_action();
    true
}

/// List indent width (Preferences), kept as the view's `indent-width`.
pub(crate) fn indent(keys: &gtk::EventControllerKey) -> usize {
    keys.widget()
        .and_downcast::<sourceview5::View>()
        .and_then(|v| usize::try_from(v.indent_width()).ok())
        .filter(|w| (1..=8).contains(w))
        .unwrap_or(4)
}

/// Tab / Shift+Tab on the cursor's line or the selected lines, when they are all list items;
/// one level is `width` spaces.
pub(crate) fn list_shift(buffer: &gtk::TextBuffer, outdent: bool, width: usize) -> bool {
    let (a, b) = buffer.selection_bounds().unwrap_or_else(|| {
        let c = buffer.iter_at_mark(&buffer.get_insert());
        (c, c)
    });
    let first = a.line();
    // A selection ending at the start of a line does not include that line.
    let last = if b.line() > first && b.starts_line() {
        b.line() - 1
    } else {
        b.line()
    };
    let lines: Vec<String> = (first..=last)
        .filter_map(|n| line_text(buffer, n))
        .collect();
    if !lists::all_items(lines.iter().map(String::as_str)) {
        return false;
    }
    if buffer
        .iter_at_line(first)
        .is_some_and(|s| in_code_block(buffer, &s))
    {
        return false;
    }
    buffer.begin_user_action();
    for (n, line) in (first..=last).zip(&lines) {
        if !lists::is_item(line) {
            continue;
        }
        let Some(mut start) = buffer.iter_at_line(n) else {
            continue;
        };
        if outdent {
            let mut end = start;
            end.forward_chars(lists::outdent_len(line, width) as i32);
            buffer.delete(&mut start, &mut end);
        } else {
            buffer.insert(&mut start, &" ".repeat(width));
        }
    }
    buffer.end_user_action();
    true
}
