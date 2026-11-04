//! Spell check in the editor (Preferences ▸ Editor ▸ Check Spelling): GNOME's libspelling
//! underlines misspelled words and puts its suggestions in the editor's right-click menu.
//!
//! The language is the system's (libspelling's default checker: the first of the locale's
//! languages an installed enchant dictionary covers). Words in a GtkSourceView `no-spell-check`
//! context are skipped; the Markdown language marks code spans, code blocks and link targets so.
//! libspelling checks the whole note in the background in slices of a few milliseconds between
//! frames, then only the words that change, so typing in a large note costs no more.
//! Ubuntu 24.04 ships libspelling 0.2: only calls that exist there are used.
//!
//! That first check goes from the top down at about 3 minutes per MB, and libspelling 0.2 has no
//! way to check the visible part first: a note over [`LARGE_NOTE`] is not checked (the status
//! bar says so, [`HELD_HINT`]) unless the right-click menu's Check Spelling turns it on for it.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

/// Notes larger than this (bytes, as on disk) are not spell-checked when opened: the background
/// check of a 1 MB note takes about 3 minutes of slices between frames (`docs/PERFORMANCE.md`).
pub(crate) const LARGE_NOTE: usize = 1_000_000;

/// The status bar's note while spell check is held off for a large note.
pub(crate) const HELD_HINT: &str = "Spell check is off for notes over 1 MB";

/// Whether a note of `bytes` is too large to spell-check when opened.
pub(crate) fn too_large(bytes: usize) -> bool {
    bytes > LARGE_NOTE
}

/// Attaches the checker to `view`'s buffer, off until `set_enabled`: the underline tag, the
/// suggestions, Add to Dictionary, Ignore, Check Spelling and Languages at the top of the
/// right-click menu (the view's extra menu, made if it has none), and a right-click that moves
/// the cursor to the clicked word so the menu is about it.
///
/// Making the checker loads the dictionary (about 16 ms), so the app attaches it after the
/// window's first frame (`Editor::attach_spelling`); enabling it then checks the text already
/// in the buffer.
pub(crate) fn attach(view: &sourceview5::View) -> libspelling::TextBufferAdapter {
    let buffer = view
        .buffer()
        .downcast::<sourceview5::Buffer>()
        .expect("the editor's buffer is a GtkSourceBuffer");
    let checker = libspelling::Checker::default();
    let adapter = libspelling::TextBufferAdapter::new(&buffer, &checker);
    adapter.set_enabled(false);
    let extra = view
        .property::<Option<gio::MenuModel>>("extra-menu")
        .and_then(|m| m.downcast::<gio::Menu>().ok())
        .unwrap_or_else(|| {
            let menu = gio::Menu::new();
            view.set_extra_menu(Some(&menu));
            menu
        });
    let menu = Menu::new(&adapter.menu_model(), extra);
    view.insert_action_group("spelling", Some(&adapter));
    // Runs before the view's own click handler, which opens the menu; never claims the click.
    let click = gtk::GestureClick::builder()
        .button(gtk::gdk::BUTTON_SECONDARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    click.connect_pressed(glib::clone!(
        #[weak]
        view,
        #[weak]
        adapter,
        move |_, _, x, y| {
            if !adapter.is_enabled() {
                menu.show_word(None);
                return;
            }
            let (bx, by) =
                view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            let Some(at) = view.iter_at_location(bx, by) else {
                return;
            };
            menu.show_word(misspelled_word(&adapter, &at).as_deref());
            cursor_to(&view.buffer(), &at);
        }
    ));
    view.add_controller(click);
    adapter
}

/// libspelling's menu, with working Add to Dictionary / Ignore items. libspelling 0.2
/// (Ubuntu 24.04) adds them without the word their action needs, so they never show; here they
/// are replaced by items carrying the right-clicked word, shown only on an underlined word.
/// With a libspelling whose items carry a target, its menu is used as is.
struct Menu {
    /// The menu the items are copied to the top of (the view's extra menu).
    menu: gio::Menu,
    /// Where the word items go, and their (label, action).
    at: i32,
    items: Vec<(String, String)>,
    shown: Cell<bool>,
}

impl Menu {
    fn new(model: &gio::MenuModel, menu: gio::Menu) -> Rc<Menu> {
        let (mut at, mut items, mut next) = (None, Vec::new(), 0);
        for i in 0..model.n_items() {
            let text = |name: &str| {
                model
                    .item_attribute_value(i, name, Some(glib::VariantTy::STRING))
                    .and_then(|v| v.get::<String>())
            };
            let action = text("action");
            let untargeted = model.item_attribute_value(i, "target", None).is_none();
            if let Some(action @ ("spelling.add" | "spelling.ignore")) = action.as_deref()
                && untargeted
            {
                at.get_or_insert(next);
                items.push((text("label").unwrap_or_default(), action.to_string()));
                continue;
            }
            menu.insert_item(next, &gio::MenuItem::from_model(model, i));
            next += 1;
        }
        Rc::new(Menu {
            menu,
            at: at.unwrap_or(0),
            items,
            shown: Cell::new(false),
        })
    }

    /// Shows the word items for `word`, or hides them (`None`).
    fn show_word(&self, word: Option<&str>) {
        if self.shown.replace(false) {
            for _ in &self.items {
                self.menu.remove(self.at);
            }
        }
        let Some(word) = word else {
            return;
        };
        for (n, (label, action)) in self.items.iter().enumerate() {
            let item = gio::MenuItem::new(Some(label), None);
            item.set_action_and_target_value(Some(action), Some(&word.to_variant()));
            self.menu.insert_item(self.at + n as i32, &item);
        }
        self.shown.set(!self.items.is_empty());
    }
}

/// The misspelled word at `at`, found as libspelling finds the word under the cursor; none in
/// text libspelling skips (code, URLs).
fn misspelled_word(adapter: &libspelling::TextBufferAdapter, at: &gtk::TextIter) -> Option<String> {
    let skipped = at
        .buffer()
        .tag_table()
        .lookup("gtksourceview:context-classes:no-spell-check");
    if skipped.is_some_and(|tag| at.has_tag(&tag)) {
        return None;
    }
    let mut start = *at;
    if !start.starts_word() {
        start.backward_word_start();
    }
    let mut end = start;
    if !end.ends_word() {
        end.forward_word_end();
    }
    if start == end || at < &start || at > &end || end.offset() - start.offset() >= 100 {
        return None;
    }
    let word = start.slice(&end).to_string();
    let checker = adapter.checker()?;
    (!checker.check_word(&word)).then_some(word)
}

/// Places the cursor at `at` unless it is inside the selection (the menu then acts on the
/// selection, as before).
pub(crate) fn cursor_to(buffer: &gtk::TextBuffer, at: &gtk::TextIter) {
    if let Some((start, end)) = buffer.selection_bounds()
        && at.in_range(&start, &end)
    {
        return;
    }
    buffer.place_cursor(at);
}

/// The ranges libspelling marked misspelled, as (start, end) character offsets.
#[cfg(test)]
pub(crate) fn misspelled(adapter: &libspelling::TextBufferAdapter) -> Vec<(i32, i32)> {
    let (Some(tag), Some(buffer)) = (adapter.tag(), adapter.buffer()) else {
        return Vec::new();
    };
    let mut ranges = Vec::new();
    let mut at = buffer.start_iter();
    loop {
        if at.starts_tag(Some(&tag)) || (at.offset() == 0 && at.has_tag(&tag)) {
            let mut end = at;
            end.forward_to_tag_toggle(Some(&tag));
            ranges.push((at.offset(), end.offset()));
            at = end;
        }
        if !at.forward_to_tag_toggle(Some(&tag)) {
            return ranges;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use sourceview5::prelude::*;

    use super::*;

    #[test]
    fn large_notes_are_held_off() {
        assert!(!too_large(0));
        assert!(!too_large(LARGE_NOTE));
        assert!(too_large(LARGE_NOTE + 1));
        assert!(too_large(5_000_000));
    }

    /// Needs a display and an en_US dictionary (the container has both); run with
    /// `cargo test spell -- --ignored` like `perf::large_notes`.
    #[test]
    #[ignore]
    fn underlines_prose_not_code_and_turns_off() {
        if gtk::init().is_err() {
            eprintln!("no display");
            return;
        }
        // GtkSourceView's Markdown language marks ``` fences and code spans, not `~~~` fences
        // or 4-space indented blocks (README).
        let text = "A wrod here.\n\n`inlne` code, [a lnk](https://exmple.com/pth)\n\n```rust\nfencd blck\n```\n\nLast lnie.\n";
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_language(
            sourceview5::LanguageManager::default()
                .language("markdown")
                .as_ref(),
        );
        buffer.set_text(text);
        let view = sourceview5::View::with_buffer(&buffer);
        let adapter = attach(&view);
        adapter.set_language("en_US");
        adapter.set_enabled(true);
        let window = gtk::Window::builder().child(&view).build();
        window.present();
        let context = glib::MainContext::default();
        let words = |adapter: &libspelling::TextBufferAdapter| {
            misspelled(adapter)
                .into_iter()
                .map(|(s, e)| {
                    buffer
                        .text(&buffer.iter_at_offset(s), &buffer.iter_at_offset(e), false)
                        .to_string()
                })
                .collect::<Vec<_>>()
        };
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            context.iteration(false);
        }
        // Link text is prose; its URL is skipped.
        assert_eq!(words(&adapter), ["wrod", "lnk", "lnie"]);
        adapter.set_enabled(false);
        assert!(words(&adapter).is_empty());
        window.destroy();
    }
}
