use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::translate::IntoGlib;
use gtk::glib::{self};
use sourceview5::prelude::*;
use webkit6::prelude::*;

use crate::history::{self, Direction};
use crate::{
    MARK_MARKER_STYLE, MARK_STYLE, MATH_STYLE, OUTLINE_DELAY, RESCAN_DELAY, SAVE_DELAY,
    STATUS_DELAY, TABLE_STYLE, TAG_STYLE, UNTITLED, file_name, state_file,
};
use crate::{
    aliases, backlinks, complete, extract, favorites, find, images, links, lists, mentions, naming,
    notes, outline, quick, reheading, relink, render, retag, save, sidebar, state, stats, tags,
    templates, timing, title, url,
};

/// Find in Folder's scope setter (`Editor::find_set_scope`).
pub(crate) type SetScope = Rc<dyn Fn(Option<PathBuf>)>;
/// Switches the open Find in Folder dialog to a mode (`Editor::find_mode`).
pub(crate) type SetMode = Rc<dyn Fn(crate::dialogs::FindMode)>;

/// The window's widgets and the open note. Shared (Rc) by sidebar buttons, the auto-save
/// timer and window handlers.
pub(crate) struct Editor {
    pub(crate) window: adw::ApplicationWindow,
    pub(crate) toasts: adw::ToastOverlay,
    pub(crate) split: adw::OverlaySplitView,
    /// Editor (start) and preview (end); the mode hides one of them or shows both.
    pub(crate) panes: gtk::Paned,
    /// What an open note shows: the panes, then the outline panel (Alt+Ctrl+O) on the right.
    pub(crate) note_box: gtk::Box,
    pub(crate) outline_panel: gtk::Box,
    /// Outline rows (heading titles) and the 1-based line of each; pending refresh.
    pub(crate) outline: gtk::ListBox,
    pub(crate) outline_lines: RefCell<Vec<usize>>,
    pub(crate) outline_timer: RefCell<Option<glib::SourceId>>,
    /// Backlinks under the outline: rows, the (note, 1-based line) of each, and the request
    /// generation (an older background result is dropped).
    pub(crate) backlinks: gtk::ListBox,
    pub(crate) backlink_targets: RefCell<Vec<(PathBuf, usize)>>,
    pub(crate) backlinks_gen: Cell<u64>,
    /// Every note's links (built in the background when a folder opens), and the notes whose
    /// text changed since it was last synced.
    pub(crate) link_index: Arc<Mutex<backlinks::LinkIndex>>,
    pub(crate) links_changed: RefCell<HashSet<PathBuf>>,
    /// Unlinked Mentions under Backlinks (collapsed by default): the expander, its rows, the
    /// (note, 1-based line) of each, and the request generation (shared with the background
    /// search, which stops when it changes).
    pub(crate) mentions_expander: gtk::Expander,
    pub(crate) mentions: gtk::ListBox,
    pub(crate) mention_targets: RefCell<Vec<(PathBuf, usize)>>,
    pub(crate) mentions_gen: Arc<AtomicU64>,
    /// Pending re-colouring of `#tags` in the editor.
    pub(crate) tag_timer: RefCell<Option<glib::SourceId>>,
    /// The preview's page, made on first use (`preview()`): the first WebView starts WebKit's
    /// network process and loads most of WebKit, memory a session spent in Edit never needs.
    pub(crate) preview_view: std::cell::OnceCell<webkit6::WebView>,
    /// Find in the preview: its bar above the page (`previewfind`); set with the page.
    pub(crate) preview_find: std::cell::OnceCell<Rc<crate::previewfind::Bar>>,
    /// Off-screen page being printed or exported, dropped once done (`preview::offscreen`).
    pub(crate) print_view: RefCell<Option<webkit6::WebView>>,
    /// A print page is rendering in the background; another Print/Export PDF is ignored meanwhile.
    pub(crate) print_busy: Cell<bool>,
    pub(crate) mode: Cell<state::Mode>,
    /// Pending side-by-side preview refresh after typing.
    pub(crate) preview_timer: RefCell<Option<glib::SourceId>>,
    /// Bumped by every preview refresh/load; a background render for an older one is dropped.
    pub(crate) preview_generation: Cell<u64>,
    /// The in-place preview render in flight, if any (see `RenderSlot`).
    pub(crate) preview_render: Cell<crate::preview::RenderSlot>,
    /// The note as the shown page renders it, per top-level block, for the next refresh to
    /// patch only what changed (`incremental`); None when the page is rendered whole.
    pub(crate) preview_state: RefCell<Option<crate::incremental::Preview>>,
    /// Bumped by every page load: a patch rendered for the page before is dropped.
    pub(crate) preview_loads: Cell<u64>,
    /// Pending scroll sync of the preview to the editor (side by side).
    pub(crate) sync_timer: RefCell<Option<glib::SourceId>>,
    /// Pending check of the preview's position after the user scrolled it (preview → editor),
    /// and its last known `scrollY` (set by both directions: unchanged = nothing to follow).
    pub(crate) preview_poll: RefCell<Option<glib::SourceId>>,
    pub(crate) preview_y: Cell<f64>,
    /// Heading id the preview scrolls to once the page being loaded is shown (a link to
    /// another note's heading, `go_to_heading`).
    pub(crate) preview_anchor: RefCell<Option<String>>,
    pub(crate) buffer: sourceview5::Buffer,
    pub(crate) view: sourceview5::View,
    /// Status bar under the editor (hidden with it in Preview / when no note is open), the
    /// word/character counts it shows and their pending recount.
    pub(crate) status: gtk::Label,
    pub(crate) counts: Cell<(usize, usize)>,
    pub(crate) status_timer: RefCell<Option<glib::SourceId>>,
    /// The note shown in the editor.
    pub(crate) path: RefCell<Option<PathBuf>>,
    /// Text last read from or written to `path`; saving is skipped while the buffer equals it.
    pub(crate) saved: RefCell<String>,
    /// Pending auto-save.
    pub(crate) timer: RefCell<Option<glib::SourceId>>,
    /// Set after a close was refused because saving failed: the next close goes through.
    pub(crate) close_anyway: Cell<bool>,
    /// Watches the open note for changes made by other apps.
    pub(crate) note_monitor: RefCell<Option<gio::FileMonitor>>,
    /// The "removed on disk" toast was shown for the open note (show it once).
    pub(crate) missing_told: Cell<bool>,
    /// The open folder, one monitor per folder in it, and the pending rescan.
    pub(crate) root: RefCell<Option<PathBuf>>,
    pub(crate) dir_monitors: RefCell<Vec<gio::FileMonitor>>,
    pub(crate) rescan_timer: RefCell<Option<glib::SourceId>>,
    /// Expanded sidebar folders, kept across rescans.
    pub(crate) expanded: RefCell<HashSet<PathBuf>>,
    /// Number of the last folder scan started, and of the last one applied (see
    /// `notes::is_fresh`).
    pub(crate) scan_gen: Cell<u64>,
    pub(crate) applied_gen: Cell<u64>,
    /// Folder of the newest scan (rescans target it, not a folder being replaced).
    pub(crate) scan_dir: RefCell<Option<PathBuf>>,
    /// `--new` / `--today` that arrived while the folder was still being scanned.
    pub(crate) pending_quick: Cell<Option<crate::cli::Quick>>,
    /// The sidebar's top-level entries (`sidebar::resort_note`) and its tree view.
    pub(crate) sidebar_store: RefCell<Option<gio::ListStore>>,
    pub(crate) sidebar_view: RefCell<Option<gtk::ListView>>,
    /// Sidebar Favorites panel and its list; the open folder's favourites as last read from
    /// the state file (relative paths; row i = favorites[i]).
    pub(crate) favorites_panel: gtk::Box,
    pub(crate) favorites_list: gtk::ListBox,
    pub(crate) favorites: RefCell<Vec<PathBuf>>,
    /// Every note from the last scan, for Open Quickly.
    pub(crate) note_list: RefCell<Rc<Vec<PathBuf>>>,
    /// Every image `![[name.png]]` can embed, from the same scan (`notes::scan_with_images`).
    pub(crate) image_list: RefCell<Rc<Vec<PathBuf>>>,
    /// The front-matter aliases of those notes (`load_aliases`; `[[alias]]` links resolve
    /// through them), and the number of the last listing started.
    pub(crate) aliases: RefCell<Arc<aliases::Aliases>>,
    pub(crate) aliases_gen: Cell<u64>,
    /// Sidebar Tags panel and its list; the tags shown (row i = tag_index[i]); the number of
    /// the last tag listing started and the pending one.
    pub(crate) tags_panel: gtk::Box,
    pub(crate) tags_list: gtk::ListBox,
    pub(crate) tag_index: RefCell<Vec<(String, Vec<PathBuf>)>>,
    pub(crate) tags_gen: Cell<u64>,
    pub(crate) tags_timer: RefCell<Option<glib::SourceId>>,
    /// Bookmarked folders in order; Alt+1…Alt+9 open the first nine.
    pub(crate) bookmarks: RefCell<Vec<PathBuf>>,
    /// Folders opened lately, newest first (`state::push_recent`).
    pub(crate) recent: RefCell<Vec<PathBuf>>,
    /// Vị trí con trỏ theo note, mới nhất trước (`state::push_position`).
    pub(crate) positions: RefCell<Vec<state::Position>>,
    /// Back / Forward: the notes this window showed (each window has its own).
    pub(crate) history: RefCell<history::History>,
    /// The history entry a Back / Forward is opening (`open` moves there instead of adding one).
    pub(crate) history_target: Cell<Option<usize>>,
    /// An entry the next sidebar build scrolls to and focuses (`reveal_in_sidebar`).
    pub(crate) sidebar_reveal: RefCell<Option<PathBuf>>,
    pub(crate) find_match_case: Cell<bool>,
    pub(crate) find_whole_word: Cell<bool>,
    pub(crate) find_regex: Cell<bool>,
    /// While Find in Folder is open: sets its folder scope and searches again (`None` = all).
    pub(crate) find_set_scope: RefCell<Option<SetScope>>,
    /// While Find in Folder is open: switches it to a scan mode (Broken Links, Open Tasks,
    /// Unused Attachments), or scans again if it is in that mode.
    pub(crate) find_mode: RefCell<Option<SetMode>>,
    /// Editor preferences (Ctrl+,) and the CSS that sets the chosen font.
    pub(crate) prefs: RefCell<state::Prefs>,
    pub(crate) font_css: gtk::CssProvider,
    /// Main menu ▸ Open Recent: `recent` as `win.open-recent` items.
    pub(crate) recent_menu: gio::Menu,
    /// Main menu ▸ New from Template: the templates folder's notes (`fill_templates_menu`).
    pub(crate) templates_menu: gio::Menu,
    /// Focus Mode (Shift+Ctrl+J): the editor dims all but the cursor's paragraph.
    pub(crate) focus: Rc<crate::focus::Focus>,
    /// Spell check (Preferences ▸ Editor ▸ Check Spelling), on while `prefs.spell_check` and
    /// not `spell_held`.
    pub(crate) spelling: libspelling::TextBufferAdapter,
    /// The open note is over `spell::LARGE_NOTE`: spell check stays off for it until the
    /// right-click menu's Check Spelling turns it on (which clears this).
    pub(crate) spell_held: Cell<bool>,
}

impl Editor {
    pub(crate) fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    /// The editor's whole text.
    pub(crate) fn text(&self) -> String {
        let (start, end) = self.buffer.bounds();
        self.buffer.text(&start, &end, true).to_string()
    }

    /// Writes the open folder, note and view mode for the next launch. Errors are
    /// only logged: losing this must never block the app.
    pub(crate) fn remember(&self) {
        self.write_state(None);
    }

    /// `remember`, with `favorites` (open folder, its list) replacing that folder's Favorites.
    /// Every other Favorites list is kept as the state file has it, so a window never undoes
    /// another window's change (`change_favorites`).
    fn write_state(&self, favorites: Option<(&Path, &[PathBuf])>) {
        // `recent` only changes right before a `remember`.
        self.fill_recent_menu();
        self.keep_position();
        let mut expanded: Vec<PathBuf> = self.expanded.borrow().iter().cloned().collect();
        expanded.sort();
        // Kept as the state file has them: other windows change them too.
        let saved = state::load(&state_file());
        let state = state::State {
            folder: self.root.borrow().clone(),
            note: self.path.borrow().clone(),
            mode: self.mode.get(),
            bookmarks: self.bookmarks.borrow().clone(),
            recent: self.recent.borrow().clone(),
            prefs: self.prefs.borrow().clone(),
            positions: self.positions.borrow().clone(),
            expanded,
            find_match_case: self.find_match_case.get(),
            find_whole_word: self.find_whole_word.get(),
            find_regex: self.find_regex.get(),
            favorites: {
                let mut all = saved.favorites;
                if let Some((root, list)) = favorites {
                    all.remove(root);
                    if !list.is_empty() {
                        all.insert(root.to_path_buf(), list.to_vec());
                    }
                }
                all
            },
            last_seen_whats_new: saved.last_seen_whats_new,
        };
        if let Err(e) = state::store(&state_file(), &state) {
            eprintln!("markdownmate: cannot save session state: {e}");
        }
    }

    /// Ghi vị trí con trỏ (dòng:cột 1-based) của note đang mở vào `positions`.
    fn keep_position(&self) {
        if let Some(path) = self.path.borrow().as_deref() {
            let at = self.buffer.iter_at_mark(&self.buffer.get_insert());
            let (line, column) = (at.line() as usize + 1, at.line_offset() as usize + 1);
            state::push_position(&mut self.positions.borrow_mut(), path, line, column);
        }
    }

    /// Scheme GtkSourceView của theme, theo sáng/tối hiện tại của app (`Style`).
    pub(crate) fn apply_scheme(&self) {
        let dark = adw::StyleManager::default().is_dark();
        let id = self.prefs.borrow().theme.scheme(dark);
        let manager = sourceview5::StyleSchemeManager::default();
        // Scheme của Default có sẵn: thiếu scheme riêng của app thì vẫn có scheme để dùng.
        let scheme = manager.scheme(id).or_else(|| {
            eprintln!("markdownmate: style scheme {id} not found");
            manager.scheme(state::Theme::Default.scheme(dark))
        });
        self.buffer.set_style_scheme(scheme.as_ref());
        self.focus.set_colours(scheme.as_ref(), dark);
        let (fill, marker) = render::editor_mark_colours(self.prefs.borrow().theme, dark);
        let tags = self.buffer.tag_table();
        if let Some(tag) = tags.lookup(MARK_STYLE) {
            tag.set_background(Some(fill));
        }
        if let Some(tag) = tags.lookup(MARK_MARKER_STYLE) {
            tag.set_foreground(Some(marker));
        }
    }

    /// Applies `prefs` to the app style (light/dark) and the editor view (font, line numbers,
    /// wrapping, list indent, theme).
    pub(crate) fn apply_prefs(&self) {
        // Đổi sáng/tối phát `notify::dark` → window.rs cập nhật scheme + preview.
        adw::StyleManager::default().set_color_scheme(match self.prefs.borrow().style {
            state::Style::System => adw::ColorScheme::Default,
            state::Style::Light => adw::ColorScheme::ForceLight,
            state::Style::Dark => adw::ColorScheme::ForceDark,
        });
        self.apply_scheme();
        let p = self.prefs.borrow();
        tags::set_options(p.tags, p.ignore_colours);
        self.view.set_show_line_numbers(p.line_numbers);
        self.view.set_wrap_mode(if p.wrap {
            gtk::WrapMode::WordChar
        } else {
            gtk::WrapMode::None
        });
        // Read back by the Tab / Shift+Tab list handler.
        self.view.set_indent_width(p.indent as i32);
        drop(p);
        self.apply_spelling();
        let p = self.prefs.borrow();
        let css = match &p.font {
            Some(font) => {
                let d = gtk::pango::FontDescription::from_string(font);
                let size = (d.size() > 0).then(|| {
                    let s = f64::from(d.size()) / f64::from(gtk::pango::SCALE);
                    // An absolute size is in device units (px); CSS pt = px * 0.75.
                    if d.is_size_absolute() { s * 0.75 } else { s }
                });
                state::font_css(
                    d.family().as_deref().unwrap_or(""),
                    size,
                    d.weight().into_glib(),
                    d.style() != gtk::pango::Style::Normal,
                    p.zoom,
                )
            }
            None => state::zoom_css(p.zoom),
        };
        self.font_css
            .load_from_string(&(css + &state::line_height_css(p.line_spacing)));
        // Only the on-screen preview: print and Export HTML use their own page at 100%.
        if let Some(preview) = self.preview_view.get() {
            preview.set_zoom_level(f64::from(p.zoom) / 100.0);
        }
        drop(p);
        self.text_width_margins();
        self.typewriter_margin();
        self.typewriter_scroll();
    }

    /// Typewriter scrolling: room under the text (half the view) so the last line can reach
    /// the middle; off: the normal margin.
    pub(crate) fn typewriter_margin(&self) {
        let margin = if self.prefs.borrow().typewriter {
            (self.view.height() / 2).max(BOTTOM_MARGIN)
        } else {
            BOTTOM_MARGIN
        };
        if self.view.bottom_margin() != margin {
            self.view.set_bottom_margin(margin);
        }
    }

    /// Text Width: side margins that centre the text at most that wide (Full Width: the normal
    /// margin). Follows the view's width (`text_width_resizing`).
    pub(crate) fn text_width_margins(&self) {
        let max = self.prefs.borrow().text_width.pixels();
        let margin = state::centred_margin(self.view.visible_rect().width(), max, SIDE_MARGIN);
        if self.view.left_margin() != margin || self.view.right_margin() != margin {
            self.view.set_left_margin(margin);
            self.view.set_right_margin(margin);
        }
    }

    /// Typewriter scrolling on: scrolls the cursor's line to the middle of the editor (not
    /// with a selection, so selecting does not jump).
    pub(crate) fn typewriter_scroll(&self) {
        if self.prefs.borrow().typewriter && !self.buffer.has_selection() {
            self.view
                .scroll_to_mark(&self.buffer.get_insert(), 0.0, true, 0.0, 0.5);
        }
    }

    pub(crate) fn cancel_timer(&self) {
        if let Some(id) = self.timer.take() {
            id.remove();
        }
    }

    /// Shows the cursor's line/column now; recounts words/characters (of the selection if
    /// any) after STATUS_DELAY of quiet, so typing in a large note does not stutter.
    pub(crate) fn schedule_status(self: &Rc<Self>) {
        self.show_status();
        if let Some(id) = self.status_timer.take() {
            id.remove();
        }
        let editor = Rc::clone(self);
        let id = glib::timeout_add_local_once(STATUS_DELAY, move || {
            editor.status_timer.take();
            let (start, end) = editor
                .buffer
                .selection_bounds()
                .unwrap_or_else(|| editor.buffer.bounds());
            editor
                .counts
                .set(stats::count(&editor.buffer.text(&start, &end, true)));
            editor.show_status();
        });
        self.status_timer.replace(Some(id));
    }

    /// Re-lists the outline after OUTLINE_DELAY of quiet (only while the panel is shown).
    pub(crate) fn schedule_outline(self: &Rc<Self>) {
        if let Some(id) = self.outline_timer.take() {
            id.remove();
        }
        if !self.outline_panel.is_visible() {
            return;
        }
        let editor = Rc::clone(self);
        let id = glib::timeout_add_local_once(OUTLINE_DELAY, move || {
            editor.outline_timer.take();
            editor.refresh_outline();
        });
        self.outline_timer.replace(Some(id));
    }

    /// Re-colours the `#tags`, formulas and `==highlights==` of the editor and sets table lines in monospace (so
    /// columns aligned by `table` line up whatever the editor font) after OUTLINE_DELAY of quiet.
    pub(crate) fn schedule_tags(self: &Rc<Self>) {
        if let Some(id) = self.tag_timer.take() {
            id.remove();
        }
        let editor = Rc::clone(self);
        let id = glib::timeout_add_local_once(OUTLINE_DELAY, move || {
            editor.tag_timer.take();
            let buffer = &editor.buffer;
            let (start, end) = buffer.bounds();
            buffer.remove_tag_by_name(TAG_STYLE, &start, &end);
            let text = buffer.text(&start, &end, true);
            for (line, r) in tags::spans(&text) {
                let (Some(a), Some(b)) = (
                    buffer.iter_at_line_index(line as i32, r.start as i32),
                    buffer.iter_at_line_index(line as i32, r.end as i32),
                ) else {
                    continue;
                };
                buffer.apply_tag_by_name(TAG_STYLE, &a, &b);
            }
            buffer.remove_tag_by_name(TABLE_STYLE, &start, &end);
            for line in crate::table::lines(&text) {
                if let Some(a) = buffer.iter_at_line(line as i32) {
                    let mut b = a;
                    b.forward_to_line_end();
                    buffer.apply_tag_by_name(TABLE_STYLE, &a, &b);
                }
            }
            for name in [MATH_STYLE, MARK_STYLE, MARK_MARKER_STYLE] {
                buffer.remove_tag_by_name(name, &start, &end);
            }
            let spans = render::editor_spans(&text);
            let apply = |name: &str, ((l1, c1), (l2, c2)): render::Span| {
                if let (Some(a), Some(b)) = (
                    buffer.iter_at_line_index(l1 as i32, c1 as i32),
                    buffer.iter_at_line_index(l2 as i32, c2 as i32),
                ) {
                    buffer.apply_tag_by_name(name, &a, &b);
                }
            };
            for span in spans.math {
                apply(MATH_STYLE, span);
            }
            // One line each: the text between the `==` gets the background, the `==` dimmed.
            for ((line, a), (_, b)) in spans.highlights {
                apply(MARK_STYLE, ((line, a + 2), (line, b - 2)));
                apply(MARK_MARKER_STYLE, ((line, a), (line, a + 2)));
                apply(MARK_MARKER_STYLE, ((line, b - 2), (line, b)));
            }
        });
        self.tag_timer.replace(Some(id));
    }

    /// Fills the outline panel with the headings of the editor's text, indented by level.
    pub(crate) fn refresh_outline(&self) {
        while let Some(row) = self.outline.first_child() {
            self.outline.remove(&row);
        }
        let (start, end) = self.buffer.bounds();
        let headings = outline::headings(&self.buffer.text(&start, &end, true));
        for h in &headings {
            let label = gtk::Label::builder()
                .label(&h.title)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .tooltip_text(&h.title)
                .margin_start(6 + 12 * (h.level as i32 - 1))
                .margin_end(6)
                .margin_top(4)
                .margin_bottom(4)
                .build();
            if h.level == 1 {
                label.add_css_class("heading");
            }
            self.outline.append(&label);
        }
        self.outline_lines
            .replace(headings.iter().map(|h| h.line).collect());
        self.select_outline_at_cursor();
    }

    /// Right-click / long-press on an outline heading: a menu with Copy Link to Heading and
    /// Rename Heading….
    pub(crate) fn outline_menu(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let show = Rc::new(move |x: f64, y: f64| -> bool {
            let Some(editor) = weak.upgrade() else {
                return false;
            };
            let line = editor
                .outline
                .row_at_y(y as i32)
                .and_then(|row| usize::try_from(row.index()).ok())
                .and_then(|i| editor.outline_lines.borrow().get(i).copied());
            let Some(line) = line else {
                return false;
            };
            let copy = gtk::Button::builder()
                .label("Copy Link to Heading")
                .has_frame(false)
                .build();
            let rename = gtk::Button::builder()
                .label("Rename Heading…")
                .has_frame(false)
                .build();
            for button in [&copy, &rename] {
                if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                    label.set_xalign(0.0);
                }
            }
            let items = gtk::Box::new(gtk::Orientation::Vertical, 0);
            items.append(&copy);
            items.append(&rename);
            let popover = gtk::Popover::builder()
                .child(&items)
                .has_arrow(false)
                .pointing_to(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1))
                .build();
            popover.set_parent(&editor.outline);
            popover.connect_closed(|p| {
                // Unparent after the button's click handler has run.
                let p = p.clone();
                glib::idle_add_local_once(move || p.unparent());
            });
            let (p, weak) = (popover.clone(), Rc::downgrade(&editor));
            copy.connect_clicked(move |_| {
                p.popdown();
                if let Some(editor) = weak.upgrade() {
                    editor.copy_heading_link(line);
                }
            });
            let (p, weak) = (popover.clone(), Rc::downgrade(&editor));
            rename.connect_clicked(move |_| {
                p.popdown();
                if let Some(editor) = weak.upgrade() {
                    editor.rename_heading(line);
                }
            });
            popover.popup();
            true
        });
        let click = gtk::GestureClick::builder()
            .button(gtk::gdk::BUTTON_SECONDARY)
            .build();
        let s = Rc::clone(&show);
        click.connect_pressed(move |g, _, x, y| {
            if s(x, y) {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.outline.add_controller(click);
        let press = gtk::GestureLongPress::builder().touch_only(true).build();
        press.connect_pressed(move |g, x, y| {
            if show(x, y) {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        self.outline.add_controller(press);
    }

    /// Copies `[[Note#Heading]]` for the heading on 1-based `line` of the open note.
    fn copy_heading_link(&self, line: usize) {
        let Some(path) = self.path.borrow().clone() else {
            return;
        };
        let (start, end) = self.buffer.bounds();
        let Some(fragment) = render::heading_fragment(&self.buffer.text(&start, &end, true), line)
        else {
            return;
        };
        let root = self.root.borrow().clone().unwrap_or_default();
        let name = complete::link_name(&path, &self.note_list.borrow(), &root);
        self.window
            .clipboard()
            .set_text(&format!("[[{name}#{fragment}]]"));
        self.toast("Link copied");
    }

    /// Lists the notes linking to the open note under the outline (only while the panel is
    /// shown): a lookup in the link index, after bringing it up to date (`links_job`).
    pub(crate) fn refresh_backlinks(self: &Rc<Self>) {
        let generation = self.backlinks_gen.get() + 1;
        self.backlinks_gen.set(generation);
        let (Some(target), Some(_)) = (self.path.borrow().clone(), self.root.borrow().as_ref())
        else {
            return self.fill_backlinks(Vec::new());
        };
        if !self.outline_panel.is_visible() {
            return;
        }
        let weak = Rc::downgrade(self);
        let start = Instant::now();
        let Some(job) = self.links_job(move |index| index.backlinks(&target)) else {
            return;
        };
        glib::spawn_future_local(async move {
            let Ok(found) = gio::spawn_blocking(job).await else {
                return;
            };
            if let Some(editor) = weak.upgrade()
                && editor.backlinks_gen.get() == generation
            {
                timing::took(start, "backlinks listed");
                editor.fill_backlinks(found);
            }
        });
    }

    /// A job for a background thread: brings the link index in line with the note list and
    /// the notes changed since the last sync, then runs `then` on it. The first sync of a folder
    /// reads every note; later ones only what changed. None without a folder.
    pub(crate) fn links_job<T: Send + 'static>(
        &self,
        then: impl FnOnce(&backlinks::LinkIndex) -> T + Send + 'static,
    ) -> Option<impl FnOnce() -> T + Send + 'static> {
        let root = self.root.borrow().clone()?;
        let notes = self.note_list.borrow().to_vec(); // the Rc cannot cross threads
        let aliases = Arc::clone(&self.aliases.borrow());
        let changed = self.links_changed.take();
        let index = Arc::clone(&self.link_index);
        Some(move || {
            // A sync that panicked left at worst a partly updated index; the next one fixes it.
            let mut index = index.lock().unwrap_or_else(|e| e.into_inner());
            index.sync(&notes, &aliases, &root, &changed);
            then(&index)
        })
    }

    /// Builds (or updates) the link index in the background, so Backlinks are ready when shown.
    pub(crate) fn index_links(&self) {
        let start = Instant::now();
        let Some(job) = self.links_job(|_| ()) else {
            return;
        };
        glib::spawn_future_local(async move {
            if gio::spawn_blocking(job).await.is_ok() {
                timing::took(start, "links indexed");
            }
        });
    }

    /// `path`'s text changed on disk (another app, our save, link updates): read it again at
    /// the next sync of the link index.
    pub(crate) fn note_changed(&self, path: PathBuf) {
        self.links_changed.borrow_mut().insert(path);
    }

    fn fill_backlinks(&self, found: Vec<backlinks::Backlink>) {
        let rows = found.into_iter().map(|b| (b.path, b.line, b.snippet));
        let targets = fill_note_rows(&self.backlinks, rows.collect(), "No backlinks", None);
        self.backlink_targets.replace(targets);
    }

    /// Lists the notes naming the open note without a link (Unlinked Mentions, under
    /// Backlinks). Only while that section is expanded and the panel shown, so it costs nothing
    /// otherwise; reads every note from disk on a background thread, and an older search still
    /// running is cancelled.
    pub(crate) fn refresh_mentions(self: &Rc<Self>) {
        let generation = self.mentions_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let (Some(target), Some(_)) = (self.path.borrow().clone(), self.root.borrow().as_ref())
        else {
            return self.fill_mentions(PathBuf::new(), Vec::new());
        };
        if !self.outline_panel.is_visible() || !self.mentions_expander.is_expanded() {
            return;
        }
        let notes = self.note_list.borrow().to_vec(); // the Rc cannot cross threads
        let names = self.aliases.borrow().of(&target).to_vec();
        let current = self.mentions_gen.clone();
        let weak = Rc::downgrade(self);
        let searched = target.clone();
        glib::spawn_future_local(async move {
            let Ok(Some(found)) = gio::spawn_blocking(move || {
                mentions::find(&notes, &searched, &names, &|| {
                    current.load(Ordering::SeqCst) != generation
                })
            })
            .await
            else {
                return;
            };
            if let Some(editor) = weak.upgrade()
                && editor.mentions_gen.load(Ordering::SeqCst) == generation
            {
                editor.fill_mentions(target, found);
            }
        });
    }

    /// Unlinked Mentions rows of `target`, each with a Link button (`link_mention`).
    fn fill_mentions(self: &Rc<Self>, target: PathBuf, found: Vec<mentions::Mention>) {
        let rows = found
            .iter()
            .map(|m| (m.path.clone(), m.spot.line, m.spot.snippet.clone()))
            .collect();
        let weak = Rc::downgrade(self);
        let button = move |i: usize| {
            let button = gtk::Button::builder()
                .label("Link")
                .tooltip_text("Make this mention a wiki link to this note")
                .valign(gtk::Align::Center)
                .build();
            button.add_css_class("flat");
            let (weak, target, mention) = (weak.clone(), target.clone(), found[i].clone());
            button.connect_clicked(move |button| {
                if let Some(editor) = weak.upgrade() {
                    // One click per row: the list is searched again afterwards.
                    button.set_sensitive(false);
                    editor.link_mention(&target, mention.clone());
                }
            });
            button
        };
        let targets = fill_note_rows(&self.mentions, rows, "No unlinked mentions", Some(&button));
        self.mention_targets.replace(targets);
    }

    /// Link on an Unlinked Mentions row: the mention becomes a wiki link to `target`
    /// (`mentions::link_text`), without asking. The open note is edited in the editor (one undo
    /// step, then saved); a closed one is written on a background thread, only if its text is
    /// still the one searched. When the mention isn't where it was found, nothing is written
    /// and a toast says so. Then Backlinks and Unlinked Mentions are listed again.
    pub(crate) fn link_mention(self: &Rc<Self>, target: &Path, mention: mentions::Mention) {
        let root = self.root.borrow().clone().unwrap_or_default();
        let link = mentions::link_text(&mention.spot.text, target, &self.note_list.borrow(), &root);
        let spot = mention.spot;
        if self.path.borrow().as_ref() == Some(&mention.path) {
            // The editor's text now, which may differ from the file searched.
            let text = self.text();
            let result = match mentions::link_edit(&text, spot.line, spot.column, &spot.text, &link)
            {
                Some(edit) => {
                    self.edit_buffer(&text, &[edit]);
                    // A failed save toasts itself and keeps the edit in the editor.
                    self.save();
                    Ok(())
                }
                None => Err(find::Skipped::Changed),
            };
            return self.mention_linked(&mention.path, result);
        }
        let weak = Rc::downgrade(self);
        let path = mention.path;
        glib::spawn_future_local(async move {
            let job = {
                let path = path.clone();
                move || mentions::link_in_file(&path, mention.version, &spot, &link)
            };
            let result = gio::spawn_blocking(job)
                .await
                .unwrap_or(Err(find::Skipped::NotWritten));
            if let Some(editor) = weak.upgrade() {
                if result.is_ok() {
                    // Written through a temp file the folder watcher ignores.
                    editor.note_changed(path.clone());
                }
                editor.mention_linked(&path, result);
            }
        });
    }

    fn mention_linked(self: &Rc<Self>, path: &Path, result: Result<(), find::Skipped>) {
        match result {
            Ok(()) => self.refresh_backlinks(),
            Err(find::Skipped::Changed) => self.toast(&format!(
                "“{}” changed since this mention was found, so it wasn’t linked. Try again once the list updates.",
                path.file_stem().unwrap_or_default().to_string_lossy()
            )),
            Err(find::Skipped::NotWritten) => {
                self.toast(&format!("Could not save {}", file_name(path)))
            }
        }
        self.refresh_mentions();
    }

    /// Highlights the outline row of the section holding the cursor, without activating it
    /// or taking focus from the editor, and scrolls the list to show it.
    pub(crate) fn select_outline_at_cursor(&self) {
        if !self.outline_panel.is_visible() {
            return;
        }
        let line = self.buffer.iter_at_mark(&self.buffer.get_insert()).line() as usize + 1;
        let row = outline::current(&self.outline_lines.borrow(), line)
            .and_then(|i| self.outline.row_at_index(i as i32));
        self.outline.select_row(row.as_ref());
        // shortcut: rows just rebuilt have no size yet, so no scroll until the next cursor move.
        let scroll = self.outline.ancestor(gtk::ScrolledWindow::static_type());
        if let (Some(row), Some(scroll)) = (row, scroll.and_downcast::<gtk::ScrolledWindow>())
            && let Some(b) = row.compute_bounds(&self.outline)
        {
            scroll
                .vadjustment()
                .clamp_page(b.y() as f64, (b.y() + b.height()) as f64);
        }
    }

    pub(crate) fn show_status(&self) {
        let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert());
        let (line, col) = (cursor.line() + 1, cursor.line_offset() + 1);
        let mut text = stats::status_text(line as usize, col as usize, self.counts.get());
        if self.spell_held.get() && self.prefs.borrow().spell_check {
            text.push_str(" · ");
            text.push_str(crate::spell::HELD_HINT);
        }
        self.status.set_text(&text);
    }

    /// Turns the spell checker on or off to match the preference, off while the open note is
    /// held (`spell_held`), and refreshes the status bar's hint.
    pub(crate) fn apply_spelling(&self) {
        let on = self.prefs.borrow().spell_check && !self.spell_held.get();
        if self.spelling.is_enabled() != on {
            self.spelling.set_enabled(on);
        }
        self.show_status();
    }

    /// Holds spell check off for a note of `bytes` over `spell::LARGE_NOTE`, or releases it.
    /// Called before the note's text goes into the buffer, so a held note is never queued.
    fn fit_spelling(&self, bytes: usize) {
        self.spell_held.set(crate::spell::too_large(bytes));
        self.apply_spelling();
    }

    /// Restarts the auto-save countdown (debounce).
    pub(crate) fn schedule_save(self: &Rc<Self>) {
        self.cancel_timer();
        let editor = Rc::clone(self);
        let id = glib::timeout_add_local_once(SAVE_DELAY, move || {
            // The source is finished once this runs: forget it so nobody removes it again.
            editor.timer.take();
            editor.save();
        });
        self.timer.replace(Some(id));
    }

    /// Saves the open note now if its text changed. On failure shows a toast, keeps the text
    /// in the editor and returns false.
    pub(crate) fn save(self: &Rc<Self>) -> bool {
        self.cancel_timer();
        let Some(path) = self.path.borrow().clone() else {
            return true;
        };
        let (start, end) = self.buffer.bounds();
        let text = self.buffer.text(&start, &end, true).to_string();
        let result = save::save_checked(&path, &self.saved.borrow(), &text);
        match result {
            Ok(outcome) => {
                if matches!(outcome, save::Saved::Written) {
                    self.note_changed(path.clone());
                }
                if let save::Saved::Conflict(copy) = outcome {
                    self.toast(&format!(
                        "{} changed or was removed on disk. Your version was saved as {}.",
                        file_name(&path),
                        file_name(&copy)
                    ));
                    // Keep editing the copy so later saves don't make more copies.
                    self.update_note_list(None, Some(&copy));
                    self.show_path(&copy);
                    // Back / Forward lead to the copy, the version this window edits.
                    self.history
                        .borrow_mut()
                        .retarget(|p| (p == path).then(|| copy.clone()));
                    // The copy arrives by renaming our own temp file, which the folder
                    // watcher ignores.
                    self.schedule_rescan();
                }
                self.saved.replace(text);
                self.close_anyway.set(false);
                true
            }
            Err(e) => {
                // Lúc thoát (SIGTERM, logout) toast không ai thấy: ghi cả stderr.
                eprintln!("markdownmate: could not save {}: {e}", path.display());
                self.toast(&format!("Could not save {}: {e}", file_name(&path)));
                false
            }
        }
    }

    /// Inserts the image link `store` makes for the open note (`images.rs`) in place of the
    /// selection, as one undo step. No note open: does nothing. Error: toast, nothing inserted.
    pub(crate) fn insert_image(
        &self,
        store: impl FnOnce(&crate::images::Dest) -> std::io::Result<String>,
    ) {
        let Some(note) = self.path.borrow().clone() else {
            return;
        };
        match store(&self.image_dest(&note)) {
            Ok(link) => {
                self.buffer.begin_user_action();
                self.buffer.delete_selection(true, true);
                self.buffer.insert_at_cursor(&link);
                self.buffer.end_user_action();
            }
            Err(e) => self.toast(&format!("Cannot add image: {e}")),
        }
    }

    /// Saves the current note, then shows `path`. Stays on the current note if it cannot be
    /// saved or `path` cannot be read (e.g. not UTF-8).
    pub(crate) fn open(self: &Rc<Self>, path: &Path) {
        if self.path.borrow().as_deref() == Some(path) || !self.save() {
            return;
        }
        // Rời note cũ: nhớ con trỏ của nó (save có thể đã chuyển sang bản conflict).
        self.keep_position();
        let text = match save::read_note(path) {
            Ok(text) => text,
            Err(e) => {
                self.toast(&format!("Cannot open {}: {e}", file_name(path)));
                return;
            }
        };
        self.show_path(path);
        self.saved.replace(text.clone());
        self.fit_spelling(text.len());
        // Irreversible: Ctrl+Z must not bring the previous note's text into this one.
        self.buffer.begin_irreversible_action();
        self.buffer.set_text(&text);
        self.buffer.end_irreversible_action();
        // set_text fired "changed"; nothing to save yet.
        self.cancel_timer();
        self.restore_position(path);
        // Only after "No note open": setting it again while shown is a GTK CRITICAL.
        if self.note_box.parent().is_none() {
            self.split.set_content(Some(&self.note_box));
        }
        self.record_shown(path);
        self.refresh_preview();
        self.remember();
        self.refresh_backlinks();
        self.refresh_mentions();
        timing::mark(|| format!("note opened {}", file_name(path)));
    }

    /// Adds the note `open` just showed to the history; a Back / Forward moves to its entry
    /// instead.
    fn record_shown(&self, path: &Path) {
        let mut history = self.history.borrow_mut();
        match self.history_target.take() {
            Some(i) if history.entries().get(i).map(PathBuf::as_path) == Some(path) => {
                history.move_to(i)
            }
            _ => history.visit(path),
        }
        drop(history);
        self.update_history_actions();
    }

    /// Back (Alt+Left) / Forward (Alt+Right): opens the nearest note in the window's history
    /// that still exists, with its folder if it is outside the open one. Nothing to go to: bell.
    pub(crate) fn go(self: &Rc<Self>, direction: Direction) {
        let target = self.history.borrow().target(direction, Path::is_file);
        let Some(i) = target else {
            self.window.error_bell();
            return self.update_history_actions();
        };
        let path = self.history.borrow().entries()[i].clone();
        self.history_target.set(Some(i));
        self.open(&path);
        self.history_target.set(None);
        let outside = !self
            .root
            .borrow()
            .as_deref()
            .is_some_and(|root| path.starts_with(root));
        if outside
            && self.path.borrow().as_deref() == Some(path.as_path())
            && let Some(dir) = path.parent()
        {
            self.load_folder(dir);
        }
        self.update_history_actions();
    }

    /// Previous / Next Daily Note (Ctrl+Alt+[ / ]): from the open daily note, opens the nearest
    /// earlier / later existing one; Previous from any other note opens the most recent. Never
    /// creates a note; nothing to go to: bell.
    pub(crate) fn step_daily(self: &Rc<Self>, forward: bool) {
        let Some(target) = self.daily_target(forward) else {
            self.window.error_bell();
            return self.update_history_actions();
        };
        if !self.save() {
            return;
        }
        if let Some(dir) = target.parent() {
            self.expand_to(dir);
        }
        self.open(&target);
    }

    fn daily_target(&self, forward: bool) -> Option<PathBuf> {
        let root = self.root.borrow().clone()?;
        let setting = self.prefs.borrow().daily_folder.clone();
        let current = self.path.borrow().clone();
        let from = current
            .as_deref()
            .and_then(|p| crate::daily::date_of_path(&root, &setting, p));
        crate::daily::neighbour(
            &crate::daily::folder(&root, &setting),
            from.as_deref(),
            forward,
        )
    }

    /// Greys out Back / Forward (and the daily note steps) when there is no note to go to.
    pub(crate) fn update_history_actions(&self) {
        for (name, forward) in [("previous-daily-note", false), ("next-daily-note", true)] {
            if let Some(a) = self.window.lookup_action(name)
                && let Ok(a) = a.downcast::<gio::SimpleAction>()
            {
                a.set_enabled(self.daily_target(forward).is_some());
            }
        }
        let history = self.history.borrow();
        for (name, direction) in [
            ("go-back", Direction::Back),
            ("go-forward", Direction::Forward),
        ] {
            if let Some(a) = self.window.lookup_action(name)
                && let Ok(a) = a.downcast::<gio::SimpleAction>()
            {
                a.set_enabled(history.target(direction, Path::is_file).is_some());
            }
        }
    }

    /// Đặt con trỏ ở vị trí đã nhớ của `path` (kẹp nếu file ngắn đi; chưa nhớ: đầu note) và
    /// cuộn tới. Không đổi chế độ xem, không lấy focus; người gọi có dòng đích (`go_to`) đè lên.
    fn restore_position(&self, path: &Path) {
        let at = self
            .positions
            .borrow()
            .iter()
            .find(|p| p.note == path)
            .map(|p| (p.line, p.column));
        let Some((line, column)) = at else {
            self.buffer.place_cursor(&self.buffer.start_iter());
            return;
        };
        self.buffer
            .place_cursor(&self.line_iter(line, Some(column)));
        // scroll_to_mark đợi tới khi layout xong (buffer vừa nạp, view có thể chưa hiện).
        self.view
            .scroll_to_mark(&self.buffer.get_insert(), 0.0, true, 0.0, 0.3);
    }

    /// Iter at 1-based `line` / `column`, clamped to the last line / the line end.
    fn line_iter(&self, line: usize, column: Option<usize>) -> gtk::TextIter {
        let line = i32::try_from(line.saturating_sub(1)).unwrap_or(i32::MAX);
        let mut iter = self
            .buffer
            .iter_at_line(line)
            .unwrap_or_else(|| self.buffer.end_iter());
        if let Some(column) = column {
            let mut end = iter;
            if !end.ends_line() {
                end.forward_to_line_end();
            }
            let column = i32::try_from(column.saturating_sub(1)).unwrap_or(i32::MAX);
            iter.set_line_offset(column.min(end.line_offset()));
        }
        iter
    }

    /// Makes `path` the open note: title and disk watch. The buffer is left as is.
    pub(crate) fn show_path(self: &Rc<Self>, path: &Path) {
        self.path.replace(Some(path.to_path_buf()));
        self.missing_told.set(false);
        self.window
            .set_title(Some(&title::window_title(Some(&file_name(path)))));
        // shortcut: a symlinked note's target is not watched (only the link's folder entry);
        // edits through the link are still caught by save_checked before any write.
        let monitor = gio::File::for_path(path)
            .monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
            .ok();
        if let Some(m) = &monitor {
            let editor = Rc::downgrade(self);
            m.connect_changed(move |_, _, _, event| {
                // `Changed` fires mid-write; wait for ChangesDoneHint / create / move / delete.
                if event != gio::FileMonitorEvent::Changed
                    && let Some(editor) = editor.upgrade()
                {
                    editor.check_disk();
                }
            });
        }
        self.note_monitor.replace(monitor);
    }

    /// The open note changed on disk: reload it if the editor has no unsaved changes
    /// (otherwise the next save finds the conflict and keeps both versions).
    pub(crate) fn check_disk(self: &Rc<Self>) {
        let Some(path) = self.path.borrow().clone() else {
            return;
        };
        let (start, end) = self.buffer.bounds();
        if self.buffer.text(&start, &end, true) != self.saved.borrow().as_str() {
            return;
        }
        match save::read_note(&path) {
            // Our own write (or no real change).
            Ok(disk) if disk == *self.saved.borrow() => self.missing_told.set(false),
            Ok(disk) => {
                self.missing_told.set(false);
                self.load_text(disk);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !self.missing_told.replace(true) {
                    self.toast(&format!(
                        "{} was deleted or renamed on disk. Its text stays in the editor.",
                        file_name(&path)
                    ));
                }
            }
            // Unreadable / not UTF-8 now: keep the editor as is; saving would make a copy.
            Err(_) => {}
        }
    }

    /// Ctrl+R: replaces the editor's text with the note on disk (unsaved edits are dropped;
    /// the caller asks first). Missing / unreadable / not UTF-8: toast, editor unchanged.
    pub(crate) fn reload(self: &Rc<Self>) {
        let Some(path) = self.path.borrow().clone() else {
            return;
        };
        match save::read_note(&path) {
            Ok(disk) => {
                self.missing_told.set(false);
                self.load_text(disk);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.toast(&format!(
                "{} was deleted or renamed on disk. Its text stays in the editor.",
                file_name(&path)
            )),
            Err(e) => self.toast(&format!("Cannot reload {}: {e}", file_name(&path))),
        }
    }

    /// Shows `disk` (just read from the open note) as its saved text, keeping the cursor's
    /// line. Irreversible: reloading clears the undo history.
    fn load_text(self: &Rc<Self>, disk: String) {
        let line = self.buffer.iter_at_mark(&self.buffer.get_insert()).line();
        // Same note: keep its spell check (held, or turned on from the menu) unless it crossed
        // the size limit.
        if crate::spell::too_large(disk.len()) != crate::spell::too_large(self.saved.borrow().len())
        {
            self.fit_spelling(disk.len());
        }
        // Baseline for save_checked: auto-save neither rewrites it nor makes a conflict copy.
        self.saved.replace(disk.clone());
        self.buffer.begin_irreversible_action();
        self.buffer.set_text(&disk);
        self.buffer.end_irreversible_action();
        // set_text fired "changed" (auto-save scheduled): nothing to save.
        self.cancel_timer();
        let iter = self
            .buffer
            .iter_at_line(line)
            .unwrap_or_else(|| self.buffer.end_iter());
        self.buffer.place_cursor(&iter);
        self.refresh_preview();
    }

    /// The link under widget point (`x`, `y`) of the editor, if any.
    pub(crate) fn link_at(&self, x: f64, y: f64) -> Option<links::Target> {
        let (bx, by) =
            self.view
                .window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let iter = self.view.iter_at_location(bx, by)?;
        let start = self.buffer.iter_at_line(iter.line())?;
        let mut end = start;
        end.forward_to_line_end();
        let above = self.buffer.text(&self.buffer.start_iter(), &start, true);
        let line = self.buffer.text(&start, &end, true);
        links::at(&above, &line, iter.line_offset() as usize)
    }

    /// The `#tag` (without `#`) at a widget position, while a folder is open (`tags::at`).
    pub(crate) fn tag_at(&self, x: f64, y: f64) -> Option<String> {
        self.root.borrow().as_ref()?;
        let (bx, by) =
            self.view
                .window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let iter = self.view.iter_at_location(bx, by)?;
        let text = self
            .buffer
            .text(&self.buffer.start_iter(), &self.buffer.end_iter(), true);
        tags::at(&text, iter.line() as usize, iter.line_index() as usize)
    }

    /// Shows `tag` in the sidebar's Tags section: the sidebar opens, the tag's row (or its
    /// nearest listed parent) is selected and scrolled to, and its notes are listed
    /// (`sidebar::tag_notes`). Nothing without an open folder.
    pub(crate) fn show_tag(self: &Rc<Self>, tag: &str) {
        if self.root.borrow().is_none() {
            return;
        }
        let found = tags::row_of(&self.tag_index.borrow(), tag)
            .and_then(|i| Some((self.tags_list.row_at_index(i as i32)?, i)));
        let Some((row, i)) = found else {
            return self.toast(&format!("No notes listed for #{tag} yet"));
        };
        self.split.set_show_sidebar(true);
        self.tags_list.select_row(Some(&row));
        row.grab_focus();
        let entry = self.tag_index.borrow().get(i).cloned();
        if let Some((tag, notes)) = entry {
            sidebar::tag_notes(self, &tag, &notes);
        }
    }

    /// The name of the `[[wiki link]]` at a widget position, without `link_at`'s fenced-code
    /// check (which copies the text above): cheap enough for every pointer move.
    pub(crate) fn wiki_near(&self, x: f64, y: f64) -> Option<String> {
        let (bx, by) =
            self.view
                .window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
        let iter = self.view.iter_at_location(bx, by)?;
        let start = self.buffer.iter_at_line(iter.line())?;
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        let line = self.buffer.text(&start, &end, true);
        match links::at("", &line, iter.line_offset() as usize)? {
            links::Target::Wiki(name) => Some(name),
            links::Target::Href(_) => None,
        }
    }

    /// The image of the open folder a wiki name like `photo.png` (`![[photo.png]]`) shows,
    /// nearest to the note first.
    pub(crate) fn embed_image(&self, name: &str) -> Option<PathBuf> {
        if !render::is_embed_image(name) {
            return None;
        }
        let root = self.root.borrow().clone()?;
        let images = self.image_list.borrow().clone();
        let current = self.path.borrow().clone();
        render::resolve_wiki(name, &images, &root, current.as_deref()).cloned()
    }

    /// The image file in the open folder that `href` (as written in the note) points to, if
    /// any; web URLs and anything outside the folder give None.
    pub(crate) fn image_path(&self, href: &str) -> Option<PathBuf> {
        let page = self.page_uri();
        let uri = glib::Uri::resolve_relative(Some(&page), href, glib::UriFlags::NONE).ok()?;
        matches!(render::link_action(&uri, &page), render::Link::Ignore)
            .then(|| self.image_in_folder(&uri))
            .flatten()
    }

    /// `uri` (resolved, `file://`) as an image file inside the open folder.
    fn image_in_folder(&self, uri: &str) -> Option<PathBuf> {
        let path = gio::File::for_uri(uri).path()?;
        images::in_folder(&path, self.root.borrow().as_deref()?)
    }

    /// Ctrl+click on a link in the editor: same rules as the preview (`follow`); images in
    /// the open folder open in the default app.
    pub(crate) fn open_editor_link(self: &Rc<Self>, target: links::Target) {
        let page = self.page_uri();
        // `![[photo.png]]`: the image it embeds.
        let target = match target {
            links::Target::Wiki(name) => match self.embed_image(&name) {
                Some(path) => links::Target::Href(gio::File::for_path(path).uri().to_string()),
                None => links::Target::Wiki(name),
            },
            href => href,
        };
        let href = match target {
            links::Target::Wiki(name) => {
                if let Some(heading) = name.strip_prefix('#') {
                    return self.go_to_heading(heading, false);
                }
                // `[[Note#Heading]]`: `follow` reads the heading from the "URI".
                let heading = name
                    .find('#')
                    .map_or(String::new(), |i| name[i..].to_string());
                let notes = self.note_list.borrow().clone();
                let root = self.root.borrow().clone().unwrap_or_default();
                let current = self.path.borrow().clone();
                let aliases = self.aliases.borrow().clone();
                let link = match render::resolve_wiki_with(
                    &name,
                    &notes,
                    &aliases,
                    &root,
                    current.as_deref(),
                ) {
                    Some(path) => render::Link::Note(path.clone()),
                    None => render::Link::Missing(name),
                };
                return self.follow(link, &heading);
            }
            links::Target::Href(href) => href,
        };
        let Ok(uri) = glib::Uri::resolve_relative(Some(&page), &href, glib::UriFlags::NONE) else {
            return self.toast(&format!("Cannot open link: {href}"));
        };
        match render::link_action(&uri, &page) {
            // `[x](#heading)` in this note.
            render::Link::Follow => {
                if let Some(heading) = fragment(&uri) {
                    self.go_to_heading(&heading, false);
                }
            }
            render::Link::Ignore => match self.image_in_folder(&uri) {
                Some(path) => {
                    let weak = Rc::downgrade(self);
                    gtk::FileLauncher::new(Some(&gio::File::for_path(path))).launch(
                        Some(&self.window),
                        gio::Cancellable::NONE,
                        move |result| {
                            if let (Err(e), Some(editor)) = (result, weak.upgrade()) {
                                editor.toast(&format!("Cannot open file: {e}"));
                            }
                        },
                    );
                }
                None => self.toast(&format!("Cannot open link: {href}")),
            },
            link => self.follow(link, &uri),
        }
    }

    /// Opens a link clicked in the preview or Ctrl+clicked in the editor: notes inside the
    /// open folder in the editor, web/mail links (`uri`) in the default app.
    pub(crate) fn follow(self: &Rc<Self>, link: render::Link, uri: &str) {
        match link {
            render::Link::Note(path) => {
                let root = self.root.borrow().clone();
                match root.and_then(|r| render::note_in_folder(&path, &r)) {
                    // After the caller returns: open() reloads the preview web view.
                    Some(note) => {
                        let editor = self.clone();
                        let heading = fragment(uri);
                        glib::idle_add_local_once(move || {
                            let reload = editor.path.borrow().as_deref() != Some(&note);
                            editor.open(&note);
                            let opened = editor.path.borrow().as_deref() == Some(&note);
                            if let Some(heading) = heading.filter(|_| opened) {
                                editor.go_to_heading(&heading, reload);
                            }
                        });
                    }
                    None => self.toast(&format!("Note not found in folder: {}", file_name(&path))),
                }
            }
            // After the caller returns: refresh_preview() reloads the preview web view.
            render::Link::Task(line) => {
                let editor = self.clone();
                glib::idle_add_local_once(move || editor.toggle_task(line));
            }
            render::Link::Copy(line) => {
                let md = self
                    .buffer
                    .text(&self.buffer.start_iter(), &self.buffer.end_iter(), true);
                match render::code_block_at(&md, line) {
                    Some(code) => {
                        self.window.clipboard().set_text(&code);
                        self.toast("Copied");
                    }
                    None => self.toast("The code block has changed; try again"),
                }
            }
            render::Link::Missing(name) => self.open_or_create_wiki_note(&name),
            render::Link::Tag(tag) => {
                let editor = self.clone();
                glib::idle_add_local_once(move || editor.show_tag(&tag));
            }
            render::Link::External => {
                let weak = Rc::downgrade(self);
                gtk::UriLauncher::new(uri).launch(
                    Some(&self.window),
                    gio::Cancellable::NONE,
                    move |result| {
                        if let (Err(e), Some(editor)) = (result, weak.upgrade()) {
                            editor.toast(&format!("Cannot open link: {e}"));
                        }
                    },
                );
            }
            render::Link::Follow | render::Link::Ignore => {}
        }
    }

    /// Puts the cursor on the heading of the open note that a link fragment names (its id or
    /// its text, `render::heading_at`), scrolled into view, without changing the mode. When
    /// `reload` (a new page is loading) and the preview shows, it scrolls there once loaded.
    pub(crate) fn go_to_heading(&self, fragment: &str, reload: bool) {
        let (start, end) = self.buffer.bounds();
        let text = self.buffer.text(&start, &end, true);
        let Some((line, id)) = render::heading_at(&text, fragment) else {
            return self.toast(&format!("No heading “{fragment}” in this note"));
        };
        self.buffer.place_cursor(&self.line_iter(line, None));
        self.view
            .scroll_to_mark(&self.buffer.get_insert(), 0.0, true, 0.0, 0.3);
        if reload && self.mode.get() != state::Mode::Edit {
            self.preview_anchor.replace(Some(id));
        }
    }

    /// The open note's file:// URI: base for relative images, and the only page the preview
    /// may navigate to.
    pub(crate) fn page_uri(&self) -> String {
        self.path.borrow().as_ref().map_or_else(
            || "file:///".to_string(),
            |p| gio::File::for_path(p).uri().into(),
        )
    }

    /// Switches Edit / Preview / Side by side (same as the header buttons and Ctrl+1/2/3).
    pub(crate) fn set_mode(&self, mode: state::Mode) {
        WidgetExt::activate_action(&self.window, "win.mode", Some(&mode.name().to_variant())).ok();
    }

    /// Opens `path` (saving the current note first), shows the editor if in Preview, and puts
    /// the cursor on 1-based `line`, scrolled into view.
    pub(crate) fn open_at_line(self: &Rc<Self>, path: &Path, line: usize) {
        self.open(path);
        if self.path.borrow().as_deref() != Some(path) {
            return; // not opened (save or read failed; a toast says why)
        }
        self.go_to(line, None);
    }

    /// Shows the editor if in Preview and puts the cursor on 1-based `line` / `column`
    /// (clamped to the last line / the line end), scrolled to the middle.
    pub(crate) fn go_to(&self, line: usize, column: Option<usize>) {
        if self.mode.get() == state::Mode::Preview {
            self.set_mode(state::Mode::Edit);
        }
        self.buffer.place_cursor(&self.line_iter(line, column));
        self.view.grab_focus();
        self.view
            .scroll_to_mark(&self.buffer.get_insert(), 0.0, true, 0.0, 0.3);
    }

    /// Shows the panes for `mode`; called by the `win.mode` action only.
    pub(crate) fn apply_mode(self: &Rc<Self>, mode: state::Mode) {
        crate::timing::mark(|| format!("mode {mode:?}"));
        self.mode.set(mode);
        let editor = self.panes.start_child().expect("editor pane");
        editor.set_visible(mode != state::Mode::Preview);
        self.panes
            .end_child()
            .expect("preview pane")
            .set_visible(mode != state::Mode::Edit);
        if mode == state::Mode::Edit
            && let Some(find) = self.preview_find.get()
        {
            find.close(false);
        }
        if mode == state::Mode::Split {
            // Not laid out yet (startup): the max-position handler halves it once it is.
            match self.panes.width() {
                0 => self.panes.set_property("position-set", false),
                w => self.panes.set_position(w / 2),
            }
        }
        if mode == state::Mode::Preview {
            self.preview().grab_focus();
        } else {
            self.view.grab_focus();
        }
        self.refresh_preview();
        self.remember();
        self.update_selection_actions();
    }

    /// New Note from Selection is enabled only with text selected in an open note's editor
    /// (not in Preview, where the selection is hidden).
    pub(crate) fn update_selection_actions(&self) {
        if let Some(a) = self.window.lookup_action("new-note-from-selection")
            && let Ok(a) = a.downcast::<gio::SimpleAction>()
        {
            a.set_enabled(
                self.buffer.has_selection()
                    && self.mode.get() != state::Mode::Preview
                    && self.path.borrow().is_some(),
            );
        }
    }

    /// Scans `dir` on a background thread, then shows it in the sidebar and watches it for
    /// added/removed/renamed entries. Results older than what is shown, or for a folder no
    /// longer wanted, are dropped.
    pub(crate) fn load_folder(self: &Rc<Self>, dir: &Path) {
        let order = self.prefs.borrow().sidebar_sort;
        self.load_scan(FolderScan::start(dir, order));
    }

    /// Shows `scan` like `load_folder`. A scan that has already finished (started while the
    /// window was being built) is shown at once, so the sidebar is in the window's first frame;
    /// otherwise when it finishes.
    pub(crate) fn load_scan(self: &Rc<Self>, scan: FolderScan) {
        let scan_gen = self.scan_gen.get() + 1;
        self.scan_gen.set(scan_gen);
        self.scan_dir.replace(Some(scan.dir.clone()));
        let FolderScan { dir, start, job } = scan;
        if job.is_finished() {
            self.apply_scan(scan_gen, &dir, start, job.join());
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || job.join())
                .await
                .unwrap_or_else(Err);
            if let Some(editor) = weak.upgrade() {
                editor.apply_scan(scan_gen, &dir, start, result);
            }
        });
    }

    /// Shows scan number `scan_gen` of `dir` (`load_scan`) unless a newer one or another
    /// folder was asked for since.
    fn apply_scan(
        self: &Rc<Self>,
        scan_gen: u64,
        dir: &Path,
        start: Instant,
        result: std::thread::Result<ScanResult>,
    ) {
        let wanted = self.scan_dir.borrow().clone();
        if !notes::is_fresh(scan_gen, self.applied_gen.get(), dir, wanted.as_deref()) {
            return;
        }
        self.applied_gen.set(scan_gen);
        timing::took(start, "folder scanned");
        match result {
            Ok(Ok((tree, folders, images))) => {
                self.image_list.replace(Rc::new(images));
                return self.show_folder(dir, &tree, &folders);
            }
            Ok(Err(e)) => self.toast(&format!("Cannot open {}: {e}", dir.display())),
            Err(_) => self.toast(&format!("Cannot open {}", dir.display())),
        }
        // A folder that could not be opened: rescans go back to the open one.
        let root = self.root.borrow().clone();
        self.scan_dir.replace(root);
    }

    /// Lists the tags of the last scan's notes in the Tags panel. Reads every note, so it runs
    /// on a background thread; a result older than the last request is dropped.
    pub(crate) fn load_tags(self: &Rc<Self>) {
        let tags_gen = self.tags_gen.get() + 1;
        self.tags_gen.set(tags_gen);
        if !tags::enabled() {
            sidebar::fill_tags(&self.tags_list, &self.tags_panel, &[]);
            self.tag_index.replace(Vec::new());
            return;
        }
        let notes = self.note_list.borrow().clone();
        let weak = Rc::downgrade(self);
        let start = Instant::now();
        glib::spawn_future_local(async move {
            let list = notes.to_vec();
            let Ok(index) = gio::spawn_blocking(move || tags::index(&list)).await else {
                return;
            };
            if let Some(editor) = weak.upgrade()
                && editor.tags_gen.get() == tags_gen
            {
                timing::took(start, "tags indexed");
                sidebar::fill_tags(&editor.tags_list, &editor.tags_panel, &index);
                editor.tag_index.replace(index);
            }
        });
    }

    /// Reads the front-matter aliases of the last scan's notes on a background thread; a
    /// result older than the last request is dropped. When they changed, wiki links resolve
    /// again: the preview, Backlinks and Unlinked Mentions are refreshed.
    pub(crate) fn load_aliases(self: &Rc<Self>) {
        let generation = self.aliases_gen.get() + 1;
        self.aliases_gen.set(generation);
        let notes = self.note_list.borrow().clone();
        let weak = Rc::downgrade(self);
        let start = Instant::now();
        glib::spawn_future_local(async move {
            let list = notes.to_vec();
            let Ok(found) = gio::spawn_blocking(move || aliases::index(&list)).await else {
                return;
            };
            let Some(editor) = weak.upgrade() else { return };
            if editor.aliases_gen.get() != generation {
                return;
            }
            timing::took(start, "aliases indexed");
            if **editor.aliases.borrow() == found {
                return;
            }
            editor.aliases.replace(Arc::new(found));
            editor.refresh_preview();
            editor.refresh_backlinks();
            editor.refresh_mentions();
        });
    }

    /// Re-lists tags and aliases (and Backlinks, if shown) RESCAN_DELAY after a note's content
    /// changed on disk.
    pub(crate) fn schedule_tags_index(self: &Rc<Self>) {
        if let Some(id) = self.tags_timer.take() {
            id.remove();
        }
        let editor = Rc::clone(self);
        let id = glib::timeout_add_local_once(RESCAN_DELAY, move || {
            editor.tags_timer.take();
            editor.load_tags();
            editor.load_aliases();
            editor.refresh_backlinks();
        });
        self.tags_timer.replace(Some(id));
    }

    /// Applies a finished scan of `dir`.
    pub(crate) fn show_folder(
        self: &Rc<Self>,
        dir: &Path,
        tree: &[notes::Node],
        folders: &[PathBuf],
    ) {
        let changed = self.root.borrow().as_deref() != Some(dir);
        if changed {
            // Giữ những thư mục bên trong `dir` (khôi phục phiên đặt sẵn trước khi quét xong).
            self.expanded.borrow_mut().retain(|d| d.starts_with(dir));
        }
        self.root.replace(Some(dir.to_path_buf()));
        // Rescans follow changes on disk: a deleted note may leave nothing to go back to.
        self.update_history_actions();
        for name in [
            "new-note",
            "new-folder",
            "todays-note",
            "new-from-template",
            "templates",
            "create-starter-templates",
            "open-quickly",
            "find-in-folder",
            "find-broken-links",
            "find-open-tasks",
            "find-unused-attachments",
            "export-folder-html",
        ] {
            if let Some(a) = self.window.lookup_action(name)
                && let Ok(a) = a.downcast::<gio::SimpleAction>()
            {
                a.set_enabled(true);
            }
        }
        if let Some(q) = self.pending_quick.take() {
            self.quick(q);
        }
        let mut list = Vec::new();
        quick::note_paths(tree, &mut list);
        self.note_list.replace(Rc::new(list));
        // Wiki links resolve against the scan: render again with the new note list.
        self.refresh_preview();
        // Shown Backlinks bring the link index up to date themselves.
        if !self.outline_panel.is_visible() {
            self.index_links();
        }
        self.refresh_backlinks();
        self.refresh_mentions();
        // The Tags panel is kept across rescans (its own refresh: `load_tags`).
        for panel in [&self.favorites_panel, &self.tags_panel] {
            if let Some(parent) = panel.parent().and_downcast::<gtk::Box>() {
                parent.remove(panel);
            }
        }
        let side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side.append(&self.favorites_panel);
        self.fill_favorites();
        side.append(&sidebar::sidebar(self, tree, dir));
        side.append(&self.tags_panel);
        if timing::enabled() {
            side.add_tick_callback(|_, _| {
                timing::mark(|| "sidebar drawn".into());
                glib::ControlFlow::Break
            });
        }
        self.split.set_sidebar(Some(&side));
        self.load_tags();
        self.load_aliases();
        // Show it at once, not animated: libadwaita 1.5 starts the slide-in with velocity
        // 0 / (hidden sidebar width 0) = NaN, which on x86 gives the content a width near i32::MIN
        // (Gtk-WARNING size_allocate). The animation is skipped while animations are off.
        let settings = self.split.settings();
        let animate = settings.is_gtk_enable_animations();
        settings.set_gtk_enable_animations(false);
        self.split.set_show_sidebar(true);
        settings.set_gtk_enable_animations(animate);
        self.dir_monitors.replace(watch_dirs(self, folders));
        timing::mark(|| format!("sidebar filled ({} notes)", self.note_list.borrow().len()));
        if changed {
            state::push_recent(&mut self.recent.borrow_mut(), dir);
            self.remember();
        }
    }

    /// Opens bookmark `i` (0-based) like Open Folder, saving the open note first. A bookmark
    /// whose folder is gone stays listed (a drive may come back); only a toast is shown.
    pub(crate) fn open_bookmark(self: &Rc<Self>, i: usize) {
        let Some(dir) = self.bookmarks.borrow().get(i).cloned() else {
            return;
        };
        if !dir.is_dir() {
            self.toast(&format!("Bookmarked folder not found: {}", dir.display()));
            return;
        }
        self.save();
        self.load_folder(&dir);
    }

    /// Main menu ▸ Open Recent: one item per recent folder (`~` for the home folder).
    pub(crate) fn fill_recent_menu(&self) {
        self.recent_menu.remove_all();
        let home = glib::home_dir();
        for dir in self.recent.borrow().iter() {
            let shown = dir.strip_prefix(&home).map_or_else(
                |_| dir.display().to_string(),
                |p| format!("~/{}", p.display()),
            );
            // `_` marks a mnemonic in menu labels.
            let item = gio::MenuItem::new(Some(&shown.replace('_', "__")), None);
            item.set_action_and_target_value(
                Some("win.open-recent"),
                Some(&dir.to_string_lossy().to_variant()),
            );
            self.recent_menu.append_item(&item);
        }
    }

    /// Main menu ▸ New from Template: one `win.new-from-template` item per template of the
    /// templates folder (`templates::list`), or a greyed-out "No Templates" (also without an
    /// open folder). Refilled each time the main menu opens, so it follows the folder, its
    /// files and the preference.
    pub(crate) fn fill_templates_menu(&self) {
        self.templates_menu.remove_all();
        let list = self
            .templates_dir()
            .map_or_else(Vec::new, |d| templates::list(&d));
        if list.is_empty() {
            // `win.no-templates` is never enabled.
            self.templates_menu
                .append(Some("No Templates"), Some("win.no-templates"));
            self.templates_menu.append(
                Some("Create Starter Templates…"),
                Some("win.create-starter-templates"),
            );
        }
        for t in list {
            // `_` marks a mnemonic in menu labels.
            let item = gio::MenuItem::new(Some(&t.title.replace('_', "__")), None);
            item.set_action_and_target_value(
                Some("win.new-from-template"),
                Some(&t.path.to_string_lossy().to_variant()),
            );
            self.templates_menu.append_item(&item);
        }
    }

    /// Where `note`'s pasted and dropped images go (`Prefs::attachments_folder`).
    pub(crate) fn image_dest(&self, note: &Path) -> crate::images::Dest {
        crate::images::dest(
            note,
            self.root.borrow().as_deref(),
            &self.prefs.borrow().attachments_folder,
        )
    }

    /// The templates folder of the open folder (`Prefs::templates_folder`); None without one.
    fn templates_dir(&self) -> Option<PathBuf> {
        let root = self.root.borrow().clone()?;
        Some(crate::daily::folder(
            &root,
            &self.prefs.borrow().templates_folder,
        ))
    }

    /// Opens a recent folder like Open Folder; one that is gone is dropped from the list.
    pub(crate) fn open_recent(self: &Rc<Self>, dir: &Path) {
        if !dir.is_dir() {
            self.recent.borrow_mut().retain(|r| r != dir);
            self.remember();
            self.toast(&format!("Folder not found: {}", dir.display()));
            return;
        }
        self.save();
        self.load_folder(dir);
    }

    /// Runs a `markdownmate://` link (`url.rs`). The window comes forward first, so nothing
    /// changes unseen; a bad or unsafe link only shows a toast.
    pub(crate) fn open_link(self: &Rc<Self>, uri: &str) {
        self.window.present();
        let action = match url::parse(uri) {
            Ok(a) => a,
            Err(msg) => return self.toast(msg),
        };
        // The folder being loaded (startup, bookmark) counts as open.
        let root = (self.scan_dir.borrow().clone()).or_else(|| self.root.borrow().clone());
        let bookmarks = self.bookmarks.borrow().clone();
        let (path, text) = match action {
            url::Action::Create { name, text } => {
                let Some(root) = root else {
                    return self.toast("Open a folder first");
                };
                let before = self.path.borrow().clone();
                self.new_note(&root, &name);
                if *self.path.borrow() != before && !text.is_empty() {
                    self.append(&text);
                }
                return;
            }
            url::Action::Open { path } => (path, None),
            url::Action::Append { path, text } => (path, Some(text)),
        };
        let (dir, note) = match url::resolve(&path, root.as_deref(), &bookmarks) {
            Ok(found) => found,
            Err(msg) => return self.toast(msg),
        };
        if root.as_deref() != Some(&dir) {
            // A bookmarked folder: open it like Alt+N.
            if !self.save() {
                return;
            }
            self.load_folder(&dir);
        }
        self.open(&note);
        if let Some(text) = text
            && self.path.borrow().as_deref() == Some(&note)
        {
            self.append(&text);
        }
    }

    /// Ticks/unticks the to-do on 1-based `line` (a preview checkbox click) as one undoable
    /// edit, then saves and re-renders. The buffer may have changed since that render: if the
    /// line is no longer a to-do item, nothing happens.
    pub(crate) fn toggle_task(self: &Rc<Self>, line: usize) {
        let Some(mut start) = line
            .checked_sub(1)
            .and_then(|l| i32::try_from(l).ok())
            .and_then(|l| self.buffer.iter_at_line(l))
        else {
            return;
        };
        let mut end = start;
        if !end.ends_line() {
            end.forward_to_line_end();
        }
        let Some((at, new)) = lists::toggle_task(&self.buffer.text(&start, &end, true)) else {
            return;
        };
        start.forward_chars(at as i32);
        let mut next = start;
        next.forward_char();
        self.buffer.begin_user_action();
        self.buffer.delete(&mut start, &mut next);
        self.buffer.insert(&mut start, &new.to_string());
        self.buffer.end_user_action();
        self.save();
        self.refresh_preview();
    }

    /// Adds `text` at the end of the open note on a new line, as one undoable edit.
    pub(crate) fn append(self: &Rc<Self>, text: &str) {
        let mut end = self.buffer.end_iter();
        let text = if end.starts_line() {
            text.to_string()
        } else {
            format!("\n{text}")
        };
        self.buffer.begin_user_action();
        self.buffer.insert(&mut end, &text);
        self.buffer.end_user_action();
        self.buffer.place_cursor(&self.buffer.end_iter());
        self.view
            .scroll_to_mark(&self.buffer.get_insert(), 0.0, false, 0.0, 0.0);
        self.refresh_preview();
    }

    /// Folder for New Note: the open note's folder if it is inside the open folder, else the
    /// open folder. None when no folder is open.
    pub(crate) fn current_dir(&self) -> Option<PathBuf> {
        let root = self.root.borrow().clone()?;
        let parent = self
            .path
            .borrow()
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        Some(parent.filter(|p| p.starts_with(&root)).unwrap_or(root))
    }

    /// Creates note `name` (numbered if taken, e.g. `Untitled 2.md`) in `dir` and opens it.
    pub(crate) fn new_note(self: &Rc<Self>, dir: &Path, name: &str) {
        if !self.save() {
            return;
        }
        match naming::create_note(dir, name) {
            Ok(path) => {
                self.expand_to(dir);
                self.update_note_list(None, Some(&path));
                self.open(&path);
                self.schedule_rescan();
            }
            Err(e) => self.toast(&format!("Cannot create a note: {e}")),
        }
    }

    /// A click on a `[[missing]]` link: opens the note it names if the file is already there
    /// (the index not refreshed yet), else creates it empty (`naming::wiki_note_path`) and opens
    /// it, so the link stops being red. An unusable name or a failure: a toast.
    fn open_or_create_wiki_note(self: &Rc<Self>, name: &str) {
        let root = self.root.borrow().clone();
        let doc_dir = self
            .path
            .borrow()
            .as_ref()
            .and_then(|p| p.parent().map(Path::to_path_buf));
        let Some(path) =
            naming::wiki_note_path(name, root.as_deref(), doc_dir.as_deref(), |p| p.is_dir())
        else {
            return self.toast(&format!("No note named “{name}” in folder"));
        };
        if path.is_file() {
            self.open(&path);
            return;
        }
        let (Some(dir), Some(file)) = (path.parent(), path.file_name()) else {
            return;
        };
        self.new_note(dir, &file.to_string_lossy());
    }

    /// New from Template: a new note like New File (`Untitled.md`, `Untitled 2.md`, … in
    /// `current_dir`, never overwriting) whose text is the template's filled in by
    /// `templates::fill` (`{{date}}`, `{{time}}`, `{{title}}` = the new note's name without
    /// `.md`, `{{cursor}}`), opened with the cursor at the first `{{cursor}}`, else at its end.
    /// An unreadable template only shows a toast.
    pub(crate) fn new_from_template(self: &Rc<Self>, template: &Path) {
        let Some(dir) = self.current_dir() else {
            return;
        };
        let text = match std::fs::read_to_string(template) {
            Ok(text) => text,
            Err(e) => {
                self.toast(&format!(
                    "Cannot read template {}: {e}",
                    file_name(template)
                ));
                return;
            }
        };
        let Some(components) = self.now() else {
            return;
        };
        if !self.save() {
            return;
        }
        let path = match naming::create_note(&dir, UNTITLED) {
            Ok(path) => path,
            Err(e) => return self.toast(&format!("Cannot create a note: {e}")),
        };
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let filled = templates::fill(&text, &components, &title);
        if let Err(e) = std::fs::write(&path, &filled.text) {
            self.toast(&format!("Cannot write {}: {e}", file_name(&path)));
        }
        self.expand_to(&dir);
        self.update_note_list(None, Some(&path));
        self.open(&path);
        if self.path.borrow().as_deref() == Some(path.as_path()) {
            self.place_new_note_cursor(filled.cursor);
        }
        self.schedule_rescan();
    }

    /// Main menu ▸ New from Template ▸ Create Starter Templates…: creates the templates
    /// folder inside the open folder with the four starter templates, reveals it in the
    /// sidebar, and shows a toast.
    pub(crate) fn create_starter_templates(self: &Rc<Self>) {
        let Some(dir) = self.templates_dir() else {
            return;
        };
        if let Err(e) = templates::create_starters(&dir) {
            self.toast(&format!("Cannot create starter templates: {e}"));
            return;
        }
        self.reveal_in_sidebar(&dir);
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(templates::DEFAULT_FOLDER);
        self.toast(&format!("4 templates added to {name}"));
        self.schedule_rescan();
        self.fill_templates_menu();
    }

    /// Folder for New Note from Selection: New Note's (`current_dir`), or without an open
    /// folder the open note's own folder.
    pub(crate) fn extract_dir(&self) -> Option<PathBuf> {
        self.current_dir().or_else(|| {
            self.path
                .borrow()
                .as_ref()
                .and_then(|p| p.parent().map(Path::to_path_buf))
        })
    }

    /// New Note from Selection, once named: creates `file` in `dir` (numbered if taken
    /// meanwhile, never overwriting) with the selected text (`extract::note_text`), replaces
    /// the selection with the wiki link to it (`complete::link_name`) as one undo step and
    /// leaves the link selected. It stays in this note, so Ctrl+Z brings the text back
    /// (opening the new note would drop the undo history); Ctrl+click on the link opens it.
    /// Nothing changes if the note changed under the dialog or the new note cannot be written.
    pub(crate) fn extract_selection(
        self: &Rc<Self>,
        dir: &Path,
        file: &str,
        selection: extract::Selection,
    ) {
        if self.path.borrow().as_deref() != Some(selection.note.as_path()) {
            return;
        }
        let start = self.buffer.iter_at_offset(selection.start);
        let end = self.buffer.iter_at_offset(selection.end);
        if self.buffer.text(&start, &end, true) != selection.text.as_str() {
            return self.toast("The note changed, so nothing was moved. Select the text again.");
        }
        let Some(text) = extract::note_text(&selection.text) else {
            return;
        };
        let path = match naming::create_note(dir, file) {
            Ok(path) => path,
            Err(e) => return self.toast(&format!("Cannot create a note: {e}")),
        };
        if let Err(e) = std::fs::write(&path, text) {
            let _ = std::fs::remove_file(&path);
            return self.toast(&format!("Cannot write {}: {e}", file_name(&path)));
        }
        self.update_note_list(None, Some(&path));
        let root = self
            .root
            .borrow()
            .clone()
            .unwrap_or_else(|| dir.to_path_buf());
        let link = extract::link(&complete::link_name(&path, &self.note_list.borrow(), &root));
        let (mut start, mut end) = (start, end);
        self.buffer.begin_user_action();
        self.buffer.delete(&mut start, &mut end);
        self.buffer
            .insert(&mut start, &extract::replacement(&selection.text, &link));
        self.buffer.end_user_action();
        let link_start = selection.start + extract::link_start(&selection.text) as i32;
        let link_end = link_start + link.chars().count() as i32;
        self.buffer.select_range(
            &self.buffer.iter_at_offset(link_end),
            &self.buffer.iter_at_offset(link_start),
        );
        self.view.grab_focus();
        self.expand_to(dir);
        self.schedule_rescan();
    }

    /// The local date and time components for templates; None (with a toast) if the clock
    /// cannot be read.
    fn now(&self) -> Option<templates::Components> {
        let Ok(now) = glib::DateTime::now_local() else {
            self.toast("Cannot read today's date");
            return None;
        };
        Some(templates::Components {
            year: now.year(),
            month: now.month() as u32,
            day: now.day_of_month() as u32,
            weekday: now.day_of_week() as u32,
            hour: now.hour() as u32,
            minute: now.minute() as u32,
            second: now.second() as u32,
        })
    }

    /// Puts the cursor of a note just created from a template at `offset` (characters; past
    /// the end = the end) and scrolls to it.
    fn place_new_note_cursor(&self, offset: usize) {
        let offset = i32::try_from(offset).unwrap_or(i32::MAX);
        self.buffer
            .place_cursor(&self.buffer.iter_at_offset(offset));
        self.view
            .scroll_to_mark(&self.buffer.get_insert(), 0.0, true, 0.0, 0.3);
    }

    /// `markdownmate --new` / `--today` (the dock menu): in the open folder; while the
    /// window's folder is still being scanned, once it is shown.
    pub(crate) fn quick(self: &Rc<Self>, q: crate::cli::Quick) {
        use crate::cli::Quick;
        if self.root.borrow().is_none() {
            if self.scan_dir.borrow().is_some() {
                self.pending_quick.set(Some(q));
            } else if q == Quick::Today {
                self.toast("Open a folder for Today's Note");
            }
            return;
        }
        match q {
            Quick::New => {
                let _ = WidgetExt::activate_action(&self.window, "win.new-note", None);
            }
            Quick::Today => self.todays_note(),
        }
    }

    /// Today's Note (Ctrl+D): opens `<open folder>/<daily notes folder>/YYYY-MM-DD.md` for the
    /// local date (`daily`); a missing one is created (with its folders) from `Template.md`
    /// (filled in like New from Template) or `# YYYY-MM-DD` and a blank line, and opens with
    /// the cursor at the template's first `{{cursor}}`, else at its end. Nothing without an
    /// open folder.
    pub(crate) fn todays_note(self: &Rc<Self>) {
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let Some(components) = self.now() else {
            return;
        };
        let date = components.date_string();
        let path = crate::daily::path(&root, &self.prefs.borrow().daily_folder, &date);
        if self.path.borrow().as_deref() == Some(path.as_path()) {
            return;
        }
        if !self.save() {
            return;
        }
        let daily_folder = crate::daily::folder(&root, &self.prefs.borrow().daily_folder);
        let filled = crate::daily::initial_text(&daily_folder, &components);
        let created = match crate::daily::create_if_missing(&path, &filled.text) {
            Ok(created) => created,
            Err(e) => {
                self.toast(&format!("Cannot create today's note: {e}"));
                return;
            }
        };
        if let Some(dir) = path.parent() {
            self.expand_to(dir);
        }
        if created {
            self.update_note_list(None, Some(&path));
            self.schedule_rescan();
        }
        self.open(&path);
        if created && self.path.borrow().as_deref() == Some(path.as_path()) {
            self.place_new_note_cursor(filled.cursor);
        }
    }

    /// Expands `dir` and its parents so a note added there shows in the sidebar.
    fn expand_to(&self, dir: &Path) {
        let root = self.root.borrow().clone();
        self.expanded.borrow_mut().extend(
            dir.ancestors()
                .take_while(|d| root.as_deref().is_some_and(|r| d.starts_with(r) && *d != r))
                .map(Path::to_path_buf),
        );
    }

    /// Copies files dropped from another app into `dir` (numbered if the name is taken, never
    /// overwriting). Only `.md`/`.markdown` files are copied; folders, other types and hidden
    /// names are skipped with a toast. A single dropped note is opened.
    pub(crate) fn copy_in(self: &Rc<Self>, files: &[PathBuf], dir: &Path) {
        let (notes, skipped): (Vec<&PathBuf>, Vec<&PathBuf>) = files.iter().partition(|p| {
            crate::notes::is_note(p)
                && p.is_file()
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        });
        let mut copied = Vec::new();
        for src in notes {
            match naming::copy_in(src, dir) {
                Ok(path) => {
                    self.update_note_list(None, Some(&path));
                    copied.push(path);
                }
                Err(e) => self.toast(&format!("Cannot copy {}: {e}", file_name(src))),
            }
        }
        if !skipped.is_empty() {
            self.toast(&match skipped.as_slice() {
                [one] => format!("Skipped {}: only .md notes can be added", file_name(one)),
                many => format!("Skipped {} items: only .md notes can be added", many.len()),
            });
        }
        if copied.is_empty() {
            return;
        }
        self.expand_to(dir);
        if let [one] = copied.as_slice() {
            self.open(one);
        }
        self.schedule_rescan();
    }

    /// Creates folder `name` in `parent` with a new note in it (the sidebar hides empty
    /// folders). Never replaces an existing folder.
    pub(crate) fn new_folder(self: &Rc<Self>, parent: &Path, name: &str) {
        let name = match naming::check(name) {
            Ok(n) => n,
            Err(msg) => return self.toast(msg),
        };
        let dir = parent.join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => self.new_note(&dir, UNTITLED),
            Err(e) => self.toast(&format!("Cannot create folder {name}: {e}")),
        }
    }

    /// Saves the open note if it is `target` or inside it, then stops watching it so the
    /// coming rename/trash is not reported as "deleted on disk". Returns the open note's path
    /// (None if unaffected), or Err if saving failed.
    pub(crate) fn release_note(self: &Rc<Self>, target: &Path) -> Result<Option<PathBuf>, ()> {
        let inside = |e: &Self| {
            e.path
                .borrow()
                .as_ref()
                .is_some_and(|p| p.starts_with(target))
        };
        if !inside(self) {
            return Ok(None);
        }
        if !self.save() {
            return Err(());
        }
        // Saving may have moved the note to a conflict copy elsewhere.
        if !inside(self) {
            return Ok(None);
        }
        if let Some(m) = self.note_monitor.take() {
            m.cancel();
        }
        Ok(self.path.borrow().clone())
    }

    /// Renames a note (keeping/adding `.md`) or folder in place; never replaces an existing
    /// entry. The open note follows the rename.
    pub(crate) fn rename(self: &Rc<Self>, from: &Path, input: &str, is_note: bool) {
        let name = if is_note {
            naming::note_name(input)
        } else {
            naming::check(input).map(str::to_string)
        };
        let name = match name {
            Ok(n) => n,
            Err(msg) => return self.toast(msg),
        };
        self.relocate(from, "rename", |from| naming::rename(from, &name));
    }

    /// Moves a note or folder dropped on folder `dir` into it (sidebar drag and drop, Move to
    /// Folder…), only inside the open folder; never replaces an existing entry. The open note
    /// follows. Returns the new path if it moved.
    pub(crate) fn move_into(self: &Rc<Self>, from: &Path, dir: &Path) -> Option<PathBuf> {
        let root = self.scan_dir.borrow().clone()?;
        match naming::move_target(from, dir, &root) {
            // `to` is under the folder's real path; keep the paths the sidebar lists.
            Ok(Some(to)) => {
                let to = relink::rebase(&to, &root);
                self.relocate(from, "move", |from| naming::move_to(from, &to))
            }
            Ok(None) => None,
            Err(msg) => {
                self.toast(&format!("Cannot move {}: {msg}", file_name(from)));
                None
            }
        }
    }

    /// Shows `path` in the sidebar once the rescan lists it: its folders open and its row
    /// scrolled to and focused (`sidebar::sidebar`).
    pub(crate) fn reveal_in_sidebar(&self, path: &Path) {
        if let Some(root) = self.root.borrow().as_deref() {
            let mut expanded = self.expanded.borrow_mut();
            for dir in path.ancestors().skip(1) {
                if !dir.starts_with(root) || dir == root {
                    break;
                }
                expanded.insert(dir.to_path_buf());
            }
        }
        self.sidebar_reveal.replace(Some(path.to_path_buf()));
    }

    /// Runs `op` (a rename/move of `from` returning the new path) with the open note saved
    /// first; the open note and the expanded folders follow the new path. Then offers to
    /// update the links to it (`offer_link_updates`). Returns the new path if `op` succeeded.
    fn relocate(
        self: &Rc<Self>,
        from: &Path,
        verb: &str,
        op: impl FnOnce(&Path) -> std::io::Result<PathBuf>,
    ) -> Option<PathBuf> {
        let Ok(open) = self.release_note(from) else {
            return None;
        };
        match op(from) {
            Ok(to) => {
                let before = self.note_list.borrow().to_vec();
                self.update_note_list(Some(from), Some(&to));
                for p in self.positions.borrow_mut().iter_mut() {
                    if let Some(new) = moved(&p.note, from, &to) {
                        p.note = new;
                    }
                }
                if let Some(new) = open.as_deref().and_then(|p| moved(p, from, &to)) {
                    self.show_path(&new);
                }
                self.history.borrow_mut().retarget(|p| moved(p, from, &to));
                self.update_history_actions();
                let mut expanded = self.expanded.borrow_mut();
                *expanded = expanded
                    .drain()
                    .map(|d| moved(&d, from, &to).unwrap_or(d))
                    .collect();
                drop(expanded);
                let (f, t) = (from.to_path_buf(), to.clone());
                self.change_favorites(move |list, root| favorites::follow(list, root, &f, &t));
                self.schedule_rescan();
                if to != from {
                    self.offer_link_updates(vec![(from.to_path_buf(), to.clone())], before);
                }
                Some(to)
            }
            Err(e) => {
                if let Some(p) = &open {
                    self.show_path(p);
                }
                self.toast(&format!("Cannot {verb} {}: {e}", file_name(from)));
                None
            }
        }
    }

    /// After the sidebar renamed or moved entries (`moves`: old and new path; `before`: the
    /// folder's notes before), finds the notes in the open folder whose links need changing
    /// (on a background thread) and asks once: "Update N links in M notes?", Don't Update
    /// being the default. Update edits the open note in the editor (one undo step, saved like
    /// any edit) and writes the others, unless they changed meanwhile.
    pub(crate) fn offer_link_updates(
        self: &Rc<Self>,
        moves: Vec<(PathBuf, PathBuf)>,
        before: Vec<PathBuf>,
    ) {
        let Some(root) = self.scan_dir.borrow().clone() else {
            return;
        };
        let name = match moves.as_slice() {
            [(_, to)] => Some(
                to.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            ),
            _ => None,
        };
        let plan = relink::Plan::new(moves, before, root);
        let open = self.path.borrow().clone().map(|p| (p, self.text()));
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok((plan, found)) = gio::spawn_blocking(move || {
                let found = relink::find(&plan, open.as_ref());
                (plan, found)
            })
            .await
            else {
                return;
            };
            let Some(editor) = weak.upgrade() else { return };
            if found.is_empty() {
                return;
            }
            let links: usize = found.iter().map(|f| f.links).sum();
            let plural =
                |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
            let body = match &name {
                Some(name) => format!("These notes link to “{name}” by its old name or location."),
                None => "These notes link to the moved items by their old names or locations."
                    .to_string(),
            };
            let dialog = adw::AlertDialog::builder()
                .heading(format!(
                    "Update {} in {}?",
                    plural(links, "link"),
                    plural(found.len(), "note")
                ))
                .body(body)
                .default_response("cancel")
                .close_response("cancel")
                .build();
            dialog.add_responses(&[("cancel", "Don’t Update"), ("update", "Update")]);
            let weak = Rc::downgrade(&editor);
            dialog.connect_response(Some("update"), move |_, _| {
                if let Some(editor) = weak.upgrade() {
                    editor.update_links(&plan, &found);
                }
            });
            dialog.present(Some(&editor.window));
        });
    }

    /// Makes `plan`'s edits: in the editor for the open note (one undo step), on disk for the
    /// other notes of `found` (in the background); a toast tells how many were updated.
    fn update_links(self: &Rc<Self>, plan: &relink::Plan, found: &[relink::Found]) {
        let open = self.path.borrow().clone();
        let mut done = 0;
        let mut closed = Vec::new();
        for f in found {
            if open.as_ref() == Some(&f.path) {
                let text = self.text();
                self.edit_buffer(&text, &plan.edits(&text, &f.path));
                done += 1;
            } else {
                closed.push(f.clone());
            }
        }
        let (plan, total) = (plan.clone(), found.len());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let paths: Vec<PathBuf> = closed.iter().map(|f| f.path.clone()).collect();
            let written = gio::spawn_blocking(move || relink::write(&plan, &closed))
                .await
                .unwrap_or(0);
            let Some(editor) = weak.upgrade() else { return };
            for path in paths {
                editor.note_changed(path);
            }
            let done = done + written;
            editor.toast(&if done == total {
                format!(
                    "Updated links in {done} note{}",
                    if done == 1 { "" } else { "s" }
                )
            } else {
                format!("Updated links in {done} of {total} notes; the others changed meanwhile")
            });
            editor.refresh_backlinks();
        });
    }

    /// Makes `edits` (in order, not overlapping; byte ranges of `text`, the buffer's text) in
    /// the editor as one undo step.
    pub(crate) fn edit_buffer(&self, text: &str, edits: &[relink::Edit]) {
        let (Some(first), Some(last)) = (edits.first(), edits.last()) else {
            return;
        };
        self.buffer.begin_user_action();
        if edits.len() <= 100 {
            // Char offsets counted once, front to back; edited back to front.
            let (mut byte, mut chars) = (0, 0);
            let mut offsets = Vec::with_capacity(edits.len());
            for edit in edits {
                chars += text[byte..edit.range.start].chars().count();
                let start = chars;
                chars += text[edit.range.clone()].chars().count();
                byte = edit.range.end;
                offsets.push((start as i32, chars as i32));
            }
            for (edit, &(start, end)) in edits.iter().zip(&offsets).rev() {
                let mut start = self.buffer.iter_at_offset(start);
                let mut end = self.buffer.iter_at_offset(end);
                self.buffer.delete(&mut start, &mut end);
                self.buffer.insert(&mut start, &edit.text);
            }
        } else {
            // Many edits (a large note): the span from the first to the last replaced at once,
            // so the buffer changes twice instead of thousands of times; the cursor keeps its
            // place.
            let span = first.range.start..last.range.end;
            let local: Vec<relink::Edit> = edits
                .iter()
                .map(|e| relink::Edit {
                    range: e.range.start - span.start..e.range.end - span.start,
                    text: e.text.clone(),
                })
                .collect();
            let new = relink::apply(&text[span.clone()], &local);
            let from = text[..span.start].chars().count() as i32;
            let old_len = text[span].chars().count() as i32;
            let new_len = new.chars().count() as i32;
            let cursor = self.buffer.iter_at_mark(&self.buffer.get_insert()).offset();
            let mut start = self.buffer.iter_at_offset(from);
            let mut end = self.buffer.iter_at_offset(from + old_len);
            self.buffer.delete(&mut start, &mut end);
            self.buffer.insert(&mut start, &new);
            let cursor = if cursor >= from + old_len {
                cursor - old_len + new_len
            } else {
                cursor.min(from + new_len)
            };
            self.buffer
                .place_cursor(&self.buffer.iter_at_offset(cursor));
        }
        self.buffer.end_user_action();
    }

    /// Sidebar Tags ▸ Rename Tag…: asks for the new name of `#old` (checked as it is typed),
    /// then `offer_tag_rename`.
    pub(crate) fn rename_tag(self: &Rc<Self>, old: &str) {
        let entry = gtk::Entry::builder()
            .text(old)
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Rename #{old}"))
            .body(format!(
                "The new name for #{old} in every note of the folder. Nested tags such as \
#{old}/… move with it."
            ))
            .extra_child(&entry)
            .default_response("rename")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("rename", "Rename")]);
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        let name = move |entry: &gtk::Entry, old: &str| {
            retag::valid_name(&entry.text()).filter(|new| new != old)
        };
        let check = {
            let (dialog, old) = (dialog.downgrade(), old.to_string());
            move |entry: &gtk::Entry| {
                let ok = name(entry, &old).is_some();
                if let Some(dialog) = dialog.upgrade() {
                    dialog.set_response_enabled("rename", ok);
                }
                let typed = entry.text().trim().trim_start_matches('#').to_string();
                if ok || typed == old {
                    entry.remove_css_class("error");
                } else {
                    entry.add_css_class("error");
                }
            }
        };
        check(&entry);
        entry.connect_changed(check);
        let (weak, old) = (Rc::downgrade(self), old.to_string());
        dialog.connect_response(Some("rename"), move |_, _| {
            if let (Some(editor), Some(new)) = (weak.upgrade(), name(&entry, &old)) {
                editor.offer_tag_rename(old.clone(), new);
            }
        });
        dialog.present(Some(&self.window));
    }

    /// Counts the notes of the open folder that use `#old` (on a background thread) and asks
    /// "Rename #old to #new in N notes?" (saying when `#new` already exists: the notes join
    /// it). Rename edits the open note in the editor (one undo step, saved at once) and writes
    /// the others unless they changed meanwhile, then lists the tags again.
    fn offer_tag_rename(self: &Rc<Self>, old: String, new: String) {
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let notes = self.note_list.borrow().to_vec();
        let open = self.path.borrow().clone().map(|p| (p, self.text()));
        let joins = self.tag_index.borrow().iter().any(|(tag, _)| {
            tag.to_lowercase() == new.to_lowercase() && retag::under(tag, &old).is_none()
        });
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (o, n) = (old.clone(), new.clone());
            let Ok(found) =
                gio::spawn_blocking(move || retag::find(&notes, &root, &o, &n, open.as_ref()))
                    .await
            else {
                return;
            };
            let Some(editor) = weak.upgrade() else { return };
            if found.is_empty() {
                editor.toast(&format!("No notes use #{old}"));
                return;
            }
            let n = found.len();
            let notes = format!("{n} note{}", if n == 1 { "" } else { "s" });
            let body = if joins {
                format!("#{new} already exists: these notes join it.")
            } else {
                format!("Nested tags such as #{old}/… become #{new}/….")
            };
            let dialog = adw::AlertDialog::builder()
                .heading(format!("Rename #{old} to #{new} in {notes}?"))
                .body(body)
                .default_response("cancel")
                .close_response("cancel")
                .build();
            dialog.add_responses(&[("cancel", "Cancel"), ("rename", "Rename")]);
            dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
            let weak = Rc::downgrade(&editor);
            dialog.connect_response(Some("rename"), move |_, _| {
                if let Some(editor) = weak.upgrade() {
                    editor.retag(&old, &new, &found);
                }
            });
            dialog.present(Some(&editor.window));
        });
    }

    /// Renames `#old` to `#new` in `found` (see `offer_tag_rename`); a toast tells how many
    /// notes were updated.
    fn retag(self: &Rc<Self>, old: &str, new: &str, found: &[retag::Found]) {
        let open = self.path.borrow().clone();
        let mut done = 0;
        let mut closed = Vec::new();
        for f in found {
            if open.as_ref() == Some(&f.path) {
                let text = self.text();
                self.edit_buffer(&text, &retag::edits(&text, old, new));
                // Saved now, so the Tags tree below reads the renamed tag.
                if self.save() {
                    done += 1;
                }
            } else {
                closed.push(f.clone());
            }
        }
        let (old, new, total) = (old.to_string(), new.to_string(), found.len());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let (o, n) = (old.clone(), new.clone());
            let written = gio::spawn_blocking(move || retag::write(&closed, &o, &n))
                .await
                .unwrap_or(0);
            let Some(editor) = weak.upgrade() else { return };
            let done = done + written;
            editor.toast(&if done == total {
                format!(
                    "Renamed #{old} to #{new} in {done} note{}",
                    if done == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "Renamed #{old} to #{new} in {done} of {total} notes; the others changed meanwhile"
                )
            });
            editor.load_tags();
        });
    }

    /// Outline ▸ Rename Heading…: asks for the new text of the heading on 1-based `line` of
    /// the open note (checked as it is typed) and says how many wiki links in the folder's
    /// notes follow (counted in the background), then `reheading`.
    pub(crate) fn rename_heading(self: &Rc<Self>, line: usize) {
        let Some(path) = self.path.borrow().clone() else {
            return;
        };
        let text = self.text();
        let Some(heading) = reheading::heading(&text, line) else {
            return;
        };
        let entry = gtk::Entry::builder()
            .text(&heading.written)
            .activates_default(true)
            .build();
        let status = gtk::Label::builder()
            .label("Counting links…")
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build();
        let notice = gtk::Label::builder()
            .label("Another heading has this text; links will reach the first")
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .css_classes(["caption", "warning"])
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.append(&entry);
        content.append(&status);
        content.append(&notice);
        let dialog = adw::AlertDialog::builder()
            .heading("Rename Heading")
            .body("Wiki links to it in the folder’s notes follow.")
            .extra_child(&content)
            .default_response("rename")
            .close_response("cancel")
            .build();
        dialog.add_responses(&[("cancel", "Cancel"), ("rename", "Rename")]);
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        // The links found, once counted; Rename waits for them.
        let found: Rc<RefCell<Option<Vec<reheading::Found>>>> = Rc::default();
        let heading = Rc::new(heading);
        let open = Rc::new((path.clone(), text));
        let check = {
            let (dialog, found, heading, open) = (
                dialog.downgrade(),
                Rc::clone(&found),
                Rc::clone(&heading),
                Rc::clone(&open),
            );
            move |entry: &gtk::Entry| {
                let typed = entry.text();
                let ok = reheading::line_edit(&open.1, &heading, &typed).is_some();
                if let Some(dialog) = dialog.upgrade() {
                    dialog.set_response_enabled("rename", ok && found.borrow().is_some());
                }
                if ok || typed.trim() == heading.written {
                    entry.remove_css_class("error");
                } else {
                    entry.add_css_class("error");
                }
                notice.set_visible(ok && reheading::shared_text(&open.1, &heading, &typed));
            }
        };
        let check = Rc::new(check);
        check(&entry);
        let c = Rc::clone(&check);
        entry.connect_changed(move |e| c(e));
        let weak = Rc::downgrade(self);
        let (h, o, f, e) = (
            Rc::clone(&heading),
            Rc::clone(&open),
            Rc::clone(&found),
            entry.clone(),
        );
        dialog.connect_response(Some("rename"), move |_, _| {
            if let (Some(editor), Some(found)) = (weak.upgrade(), f.borrow_mut().take()) {
                editor.reheading(&o, &h, &e.text(), &found);
            }
        });
        dialog.present(Some(&self.window));

        // Count: the notes the link index says link to this note, and the note itself.
        let (heading, open, first) = ((*heading).clone(), (*open).clone(), heading.first);
        let root = self.root.borrow().clone();
        let job: Box<dyn FnOnce() -> Vec<reheading::Found> + Send> = match self.links_job({
            let (heading, open, root) = (heading.clone(), open.clone(), root.clone());
            move |index: &backlinks::LinkIndex| {
                let mut notes = vec![open.0.clone()];
                notes.extend(index.backlinks(&open.0).into_iter().map(|b| b.path));
                reheading::find(&notes, root.as_deref(), index.wiki(), &open, &heading)
            }
        }) {
            Some(job) => Box::new(job),
            None => Box::new(move || {
                let wiki = render::WikiIndex::new(
                    vec![open.0.clone()],
                    open.0.parent().unwrap_or(Path::new("/")),
                );
                reheading::find(std::slice::from_ref(&open.0), None, &wiki, &open, &heading)
            }),
        };
        let dialog = dialog.downgrade();
        glib::spawn_future_local(async move {
            let Ok(links) = gio::spawn_blocking(job).await else {
                return;
            };
            if dialog.upgrade().is_none() {
                return;
            }
            status.set_label(&reheading::summary(&links, first));
            found.replace(Some(links));
            check(&entry);
        });
    }

    /// Renames the heading of `open` (the note and its text when the dialog opened) to `new`
    /// and points the links `found` to it: in the editor for the open note (one undo step,
    /// saved at once), on disk for the others unless they changed meanwhile. A toast tells how
    /// many links followed.
    fn reheading(
        self: &Rc<Self>,
        open: &(PathBuf, String),
        heading: &reheading::Heading,
        new: &str,
        found: &[reheading::Found],
    ) {
        if self.path.borrow().as_ref() != Some(&open.0) || self.text() != open.1 {
            self.toast("The note changed; rename the heading again");
            return;
        }
        let Some(line) = reheading::line_edit(&open.1, heading, new) else {
            return;
        };
        let renamed = relink::apply(&open.1, std::slice::from_ref(&line));
        let fragment = reheading::fragment(&renamed, heading.line, new);
        let own = found.iter().find(|f| f.path == open.0);
        let links = own.map_or(&[][..], |f| &f.links[..]);
        self.edit_buffer(&open.1, &reheading::edits(links, &fragment, Some(line)));
        // Saved now, so Backlinks and Find Broken Links read the new heading.
        self.save();
        self.note_changed(open.0.clone());
        let closed: Vec<reheading::Found> =
            found.iter().filter(|f| f.path != open.0).cloned().collect();
        let total: usize = found.iter().map(|f| f.links.len()).sum();
        let own_links = links.len();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let counts: Vec<(PathBuf, usize)> = closed
                .iter()
                .map(|f| (f.path.clone(), f.links.len()))
                .collect();
            let written = gio::spawn_blocking(move || reheading::write(&closed, &fragment))
                .await
                .unwrap_or_default();
            let Some(editor) = weak.upgrade() else { return };
            let mut done = own_links;
            for (path, n) in counts {
                if written.contains(&path) {
                    done += n;
                    editor.note_changed(path);
                }
            }
            let plural = |n: usize| format!("{n} link{}", if n == 1 { "" } else { "s" });
            editor.toast(&if total == 0 {
                "Heading renamed".to_string()
            } else if done == total {
                format!("Heading renamed; {} updated", plural(done))
            } else {
                format!(
                    "Heading renamed; {} of {} updated, the other notes changed meanwhile",
                    done, total
                )
            });
            editor.refresh_backlinks();
        });
    }

    /// Moves a note or folder to the Trash (never deletes). The open note, if inside, is saved
    /// first and then closed.
    pub(crate) fn trash(self: &Rc<Self>, target: &Path) {
        let Ok(open) = self.release_note(target) else {
            return;
        };
        match gio::File::for_path(target).trash(gio::Cancellable::NONE) {
            Ok(()) => {
                if open.is_some() {
                    self.close_note();
                }
                let t = target.to_path_buf();
                self.change_favorites(move |list, root| favorites::remove_under(list, root, &t));
                self.update_note_list(Some(target), None);
                self.update_history_actions();
                self.schedule_rescan();
            }
            Err(e) => {
                if let Some(p) = &open {
                    self.show_path(p);
                }
                self.toast(&format!("Cannot move {} to Trash: {e}", file_name(target)));
            }
        }
    }

    /// Empties the editor after the open note was trashed (already saved).
    pub(crate) fn close_note(&self) {
        self.path.replace(None);
        self.saved.replace(String::new());
        self.fit_spelling(0);
        self.buffer.begin_irreversible_action();
        self.buffer.set_text("");
        self.buffer.end_irreversible_action();
        self.cancel_timer();
        self.window.set_title(Some(&title::window_title(None)));
        self.split.set_content(Some(
            &adw::StatusPage::builder()
                .icon_name("text-x-generic-symbolic")
                .title("No note open")
                .build(),
        ));
        self.remember();
    }

    /// Applies a change the app itself made (see `quick::updated`) to the note list at once, so
    /// Open Quickly, `[[` completion and wiki links see it before the rescan lands.
    fn update_note_list(&self, from: Option<&Path>, to: Option<&Path>) {
        let list = quick::updated(&self.note_list.borrow(), from, to);
        self.note_list.replace(Rc::new(list));
    }

    /// Applies `change` to the open folder's Favorites as the state file has them (another
    /// window may have changed them since), saves them with the rest of the session
    /// (`remember`) and shows them.
    pub(crate) fn change_favorites(&self, change: impl FnOnce(&mut Vec<PathBuf>, &Path)) {
        let Some(root) = self.root.borrow().clone() else {
            return self.remember();
        };
        let mut list = state::load(&state_file())
            .favorites
            .remove(&root)
            .unwrap_or_default();
        change(&mut list, &root);
        self.write_state(Some((&root, &list)));
        self.show_favorites(&root, list);
    }

    /// Reads the open folder's Favorites from the state file and shows them (on every scan).
    pub(crate) fn fill_favorites(&self) {
        let Some(root) = self.root.borrow().clone() else {
            return;
        };
        let list = state::load(&state_file())
            .favorites
            .remove(&root)
            .unwrap_or_default();
        self.show_favorites(&root, list);
    }

    fn show_favorites(&self, root: &Path, list: Vec<PathBuf>) {
        let entries = favorites::entries(&list, root, Path::exists);
        sidebar::fill_favorites(&self.favorites_list, &self.favorites_panel, &entries);
        self.favorites.replace(list);
    }

    /// The open folder's favourites that are still there, as full paths (Open Quickly).
    pub(crate) fn favorite_paths(&self) -> Vec<PathBuf> {
        let Some(root) = self.root.borrow().clone() else {
            return Vec::new();
        };
        favorites::entries(&self.favorites.borrow(), &root, Path::exists)
            .into_iter()
            .filter_map(|(path, there)| there.then_some(path))
            .collect()
    }

    /// A click on Favorites row `index`: opens the note, shows the folder in the tree, or
    /// removes a favourite that is gone (with a toast).
    pub(crate) fn open_favorite(self: &Rc<Self>, index: usize) {
        let (Some(root), Some(rel)) = (
            self.root.borrow().clone(),
            self.favorites.borrow().get(index).cloned(),
        ) else {
            return;
        };
        let path = root.join(rel);
        if !path.exists() {
            let name = file_name(&path);
            self.change_favorites(move |list, root| favorites::toggle(list, root, &path, false));
            self.toast(&format!("“{name}” is gone and was removed from Favorites"));
        } else if path.is_dir() {
            self.show_in_sidebar(&path);
        } else {
            self.open(&path);
        }
    }

    /// Opens the folders above `path` in the sidebar tree and focuses its row.
    pub(crate) fn show_in_sidebar(&self, path: &Path) {
        if let Some(view) = self.sidebar_view.borrow().as_ref() {
            sidebar::reveal(view, path);
        }
    }

    /// Duplicate (sidebar right-click): copies the note or folder `path` next to itself as
    /// "Name copy.md" / "Name copy" (`naming::duplicate`), the open note saved first if it is
    /// inside. The sidebar then shows the copy, and a note copy opens.
    pub(crate) fn duplicate(self: &Rc<Self>, path: &Path) {
        let inside = self
            .path
            .borrow()
            .as_ref()
            .is_some_and(|p| p.starts_with(path));
        if inside && !self.save() {
            return;
        }
        let (src, weak) = (path.to_path_buf(), Rc::downgrade(self));
        glib::spawn_future_local(async move {
            let job = src.clone();
            let result = gio::spawn_blocking(move || naming::duplicate(&job)).await;
            let Some(editor) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(copy)) => {
                    if notes::is_note(&copy) && copy.is_file() {
                        editor.update_note_list(None, Some(&copy));
                        editor.open(&copy);
                    }
                    editor.reveal_in_sidebar(&copy);
                    editor.schedule_rescan();
                }
                Ok(Err(e)) => editor.toast(&format!("Cannot duplicate {}: {e}", file_name(&src))),
                Err(_) => editor.toast(&format!("Cannot duplicate {}", file_name(&src))),
            }
        });
    }

    /// Sorts the sidebar by `order` from now on (remembered): the folder is listed again,
    /// expanded folders kept.
    pub(crate) fn set_sidebar_sort(self: &Rc<Self>, order: notes::SortOrder) {
        if self.prefs.borrow().sidebar_sort == order {
            return;
        }
        self.prefs.borrow_mut().sidebar_sort = order;
        self.remember();
        let dir = self.scan_dir.borrow().clone();
        if let Some(dir) = dir {
            self.load_folder(&dir);
        }
    }

    /// Note `path`'s content changed on disk: under a date order its sidebar entry moves.
    pub(crate) fn resort_note(&self, path: &Path) {
        let order = self.prefs.borrow().sidebar_sort;
        let (Some(store), Some(root)) = (
            self.sidebar_store.borrow().clone(),
            self.root.borrow().clone(),
        ) else {
            return;
        };
        sidebar::resort_note(&store, &root, path, order);
    }

    /// Rescans the open folder after RESCAN_DELAY of quiet (debounce).
    pub(crate) fn schedule_rescan(self: &Rc<Self>) {
        if let Some(id) = self.rescan_timer.take() {
            id.remove();
        }
        let editor = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(RESCAN_DELAY, move || {
            let Some(editor) = editor.upgrade() else {
                return;
            };
            editor.rescan_timer.take();
            let root = editor.scan_dir.borrow().clone();
            if let Some(root) = root {
                editor.load_folder(&root);
            }
        });
        self.rescan_timer.replace(Some(id));
    }
}

/// One monitor per folder (GIO does not watch recursively); `folders` from `notes::folders`.
// shortcut: one inotify watch per folder; a tree beyond the inotify limit (default 8192+)
// silently stops watching some folders — upgrade to a single recursive watcher if that matters.
pub(crate) fn watch_dirs(editor: &Rc<Editor>, folders: &[PathBuf]) -> Vec<gio::FileMonitor> {
    folders
        .iter()
        .filter_map(|dir| watch_dir(editor, dir))
        .collect()
}

pub(crate) fn watch_dir(editor: &Rc<Editor>, dir: &Path) -> Option<gio::FileMonitor> {
    let monitor = gio::File::for_path(dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        .ok()?;
    let weak = Rc::downgrade(editor);
    monitor.connect_changed(move |_, file, other, event| {
        use gio::FileMonitorEvent as E;
        // Content edits don't change the tree (a note may only move under a date order); the
        // app's own temp file is never shown.
        let is_temp = file
            .basename()
            .is_some_and(|n| n.to_string_lossy().ends_with(".mmtmp"));
        let tree_event = matches!(
            event,
            E::Created | E::Deleted | E::Renamed | E::MovedIn | E::MovedOut
        );
        let Some(editor) = weak.upgrade() else {
            return;
        };
        if tree_event && !is_temp {
            editor.schedule_rescan();
        // A note's text changed (an app's edit, or our save renaming its temp file): its tags
        // may have too.
        } else if event == E::ChangesDoneHint || (is_temp && event == E::Renamed) {
            // Our save renames its temp file onto the note.
            let note = if is_temp { other } else { Some(file) };
            if let Some(path) = note.and_then(|f| f.path()) {
                editor.resort_note(&path);
                editor.note_changed(path);
            }
            editor.schedule_tags_index();
        }
    });
    Some(monitor)
}

/// Where `p` ends up when `from` is renamed to `to`; None if `p` is not `from` or inside it.
pub(crate) fn moved(p: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    let rel = p.strip_prefix(from).ok()?;
    // join("") would add a trailing slash.
    Some(if rel.as_os_str().is_empty() {
        to.to_path_buf()
    } else {
        to.join(rel)
    })
}

/// The editor's normal bottom margin (px).
pub(crate) const BOTTOM_MARGIN: i32 = 12;
/// The editor's left and right margin at Full Width.
pub(crate) const SIDE_MARGIN: i32 = 12;

/// Text Width: the side margins follow the editor's width, once the main loop is idle (not
/// during the size allocation that changed it: setting a margin queues a resize).
pub(crate) fn text_width_resizing(editor: &Rc<Editor>, scroll: &gtk::ScrolledWindow) {
    let editor = Rc::clone(editor);
    scroll.hadjustment().connect_page_size_notify(move |_| {
        let editor = Rc::clone(&editor);
        glib::idle_add_local_once(move || editor.text_width_margins());
    });
}

/// Typewriter scrolling: after the cursor moves or the text changes, once the main loop is
/// idle (after GTK's own scroll-to-cursor), centre the cursor's line; not while a mouse button
/// is down (a drag selects), but once it is released. The bottom margin follows the view height.
pub(crate) fn typewriter_scrolling(editor: &Rc<Editor>, scroll: &gtk::ScrolledWindow) {
    let pressed = Rc::new(Cell::new(false));
    let pending = Rc::new(Cell::new(false));
    let center: Rc<dyn Fn()> = Rc::new({
        let editor = Rc::clone(editor);
        let pressed = Rc::clone(&pressed);
        move || {
            if !editor.prefs.borrow().typewriter || pending.replace(true) {
                return;
            }
            let editor = Rc::clone(&editor);
            let pending = Rc::clone(&pending);
            let pressed = Rc::clone(&pressed);
            glib::idle_add_local_once(move || {
                pending.set(false);
                if !pressed.get() {
                    editor.typewriter_scroll();
                }
            });
        }
    });
    let c = Rc::clone(&center);
    editor.buffer.connect_mark_set(move |buffer, _, mark| {
        if *mark == buffer.get_insert() {
            c();
        }
    });
    let c = Rc::clone(&center);
    editor.buffer.connect_changed(move |_| c());
    let legacy = gtk::EventControllerLegacy::new();
    legacy.set_propagation_phase(gtk::PropagationPhase::Capture);
    legacy.connect_event(move |_, event| {
        match event.event_type() {
            gtk::gdk::EventType::ButtonPress => pressed.set(true),
            gtk::gdk::EventType::ButtonRelease => {
                pressed.set(false);
                center();
            }
            _ => {}
        }
        glib::Propagation::Proceed
    });
    editor.view.add_controller(legacy);
    let editor = Rc::clone(editor);
    // Not during the size allocation that changed it: set_bottom_margin queues a resize.
    scroll.vadjustment().connect_page_size_notify(move |_| {
        let editor = Rc::clone(&editor);
        glib::idle_add_local_once(move || editor.typewriter_margin());
    });
}

/// The percent-decoded fragment of a link (`…#frag`), if any.
fn fragment(uri: &str) -> Option<String> {
    let (_, fragment) = uri.split_once('#')?;
    let fragment = glib::Uri::unescape_string(fragment, None::<&str>)?;
    (!fragment.is_empty()).then(|| fragment.into())
}

/// Fills a Backlinks-style list: per (note, 1-based line, snippet) a row with the note name
/// and the snippet, and `button(i)` (when given) at the right of row `i`; `empty` as a dimmed,
/// inert row when there are none. Returns the (note, line) of each row, in order.
fn fill_note_rows(
    list: &gtk::ListBox,
    rows: Vec<(PathBuf, usize, String)>,
    empty: &str,
    button: Option<&dyn Fn(usize) -> gtk::Button>,
) -> Vec<(PathBuf, usize)> {
    while let Some(row) = list.first_child() {
        list.remove(&row);
    }
    if rows.is_empty() {
        let label = gtk::Label::builder()
            .label(empty)
            .xalign(0.0)
            .margin_start(6)
            .margin_top(4)
            .build();
        label.add_css_class("dim-label");
        list.append(&label);
        if let Some(row) = list.row_at_index(0) {
            row.set_activatable(false);
            row.set_selectable(false);
        }
    }
    for (i, (path, line, snippet)) in rows.iter().enumerate() {
        let name = file_name(path);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_margin_start(6);
        row.set_margin_end(6);
        row.set_margin_top(4);
        row.set_margin_bottom(4);
        let labels = gtk::Box::new(gtk::Orientation::Vertical, 2);
        labels.set_hexpand(true);
        for (text, class) in [(name.as_str(), "heading"), (snippet.as_str(), "caption")] {
            let label = gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            label.add_css_class(class);
            labels.append(&label);
        }
        labels.set_tooltip_text(Some(&format!("{name}:{line}\n{snippet}")));
        row.append(&labels);
        if let Some(button) = button {
            row.append(&button(i));
        }
        list.append(&row);
    }
    rows.into_iter().map(|(p, l, _)| (p, l)).collect()
}

/// What a folder scan gives: the sidebar's tree (`notes::scan`), the folders to watch
/// (`notes::folders`) and the images embeds can show.
type ScanResult = std::io::Result<(Vec<notes::Node>, Vec<PathBuf>, Vec<PathBuf>)>;

/// A folder scan running on its own thread; `Editor::load_scan` shows it.
pub(crate) struct FolderScan {
    dir: PathBuf,
    start: Instant,
    job: std::thread::JoinHandle<ScanResult>,
}

impl FolderScan {
    /// Starts scanning `dir` in `order`. Launch starts the session's folder this way before
    /// building the window, so the scan runs while GTK sets the window up.
    pub(crate) fn start(dir: &Path, order: notes::SortOrder) -> FolderScan {
        let d = dir.to_path_buf();
        let job = std::thread::spawn(move || {
            let mut images = Vec::new();
            notes::scan_with_images(&d, order, &mut images)
                .map(|tree| (tree, notes::folders(&d), images))
        });
        FolderScan {
            dir: dir.to_path_buf(),
            start: Instant::now(),
            job,
        }
    }
}
