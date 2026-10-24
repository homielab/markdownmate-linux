use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};

use crate::editor::Editor;
use crate::file_name;
use crate::{find, goto, naming, quick, state};

/// Desktop id of `packaging/com.homielab.MarkdownMate.desktop`.
const DESKTOP_ID: &str = "com.homielab.MarkdownMate.desktop";

/// Bookmarks dialog: Bookmark This Folder / Remove Bookmark for the open folder, then the
/// bookmarks in order (Enter or click opens one), then a "Recent" list of folders opened
/// lately (not the open one). Esc closes (adw::Dialog default).
/// Preferences (Ctrl+,): every change applies at once and is saved in the state file; the
/// header search (Ctrl+F) finds a row by title (smoke.sh reaches rows this way).
pub(crate) fn preferences_dialog(editor: &Rc<Editor>) {
    let update = {
        let editor = editor.clone();
        move |change: &dyn Fn(&mut state::Prefs)| {
            change(&mut editor.prefs.borrow_mut());
            editor.apply_prefs();
            editor.remember();
        }
    };
    let prefs = editor.prefs.borrow().clone();

    let font = gtk::FontDialogButton::new(Some(gtk::FontDialog::new()));
    font.set_valign(gtk::Align::Center);
    font.set_font_desc(&gtk::pango::FontDescription::from_string(
        prefs.font.as_deref().unwrap_or("Monospace 11"),
    ));
    font.connect_font_desc_notify(clone!(
        #[strong]
        update,
        move |b| {
            let desc = b.font_desc().map(|d| d.to_str().to_string());
            update(&|p| p.font = desc.clone());
        }
    ));
    let font_row = adw::ActionRow::builder()
        .title("Editor Font")
        .activatable_widget(&font)
        .build();
    font_row.add_suffix(&font);

    let numbers = adw::SwitchRow::builder()
        .title("Show Line Numbers")
        .active(prefs.line_numbers)
        .build();
    numbers.connect_active_notify(clone!(
        #[strong]
        update,
        move |r| {
            let on = r.is_active();
            update(&|p| p.line_numbers = on);
        }
    ));
    let wrap = adw::SwitchRow::builder()
        .title("Wrap Lines")
        .active(prefs.wrap)
        .build();
    wrap.connect_active_notify(clone!(
        #[strong]
        update,
        move |r| {
            let on = r.is_active();
            update(&|p| p.wrap = on);
        }
    ));
    let indent = adw::ComboRow::builder()
        .title("List Indent (Tab)")
        .model(&gtk::StringList::new(&["2 spaces", "4 spaces"]))
        .selected(u32::from(prefs.indent == 4))
        .build();
    indent.connect_selected_notify(clone!(
        #[strong]
        update,
        move |r| {
            let width = if r.selected() == 0 { 2 } else { 4 };
            update(&|p| p.indent = width);
        }
    ));

    let typewriter = adw::SwitchRow::builder()
        .title("Typewriter Scrolling")
        .subtitle("Keep the current line in the middle")
        .active(prefs.typewriter)
        .build();
    typewriter.connect_active_notify(clone!(
        #[strong]
        update,
        move |r| {
            let on = r.is_active();
            update(&|p| p.typewriter = on);
        }
    ));

    let group = adw::PreferencesGroup::builder().title("Editor").build();
    group.add(&font_row);
    group.add(&numbers);
    group.add(&wrap);
    group.add(&indent);
    group.add(&typewriter);

    let theme = adw::ComboRow::builder()
        .title("Theme")
        .subtitle("Colours of the editor and preview")
        // Cùng thứ tự với `Theme::ALL`.
        .model(&gtk::StringList::new(&[
            "Default",
            "Paper",
            "Graphite",
            "Solarized",
        ]))
        .selected(
            state::Theme::ALL
                .iter()
                .position(|t| *t == prefs.theme)
                .unwrap_or(0) as u32,
        )
        .build();
    theme.connect_selected_notify(clone!(
        #[strong]
        editor,
        move |r| {
            let t = state::Theme::ALL
                .get(r.selected() as usize)
                .copied()
                .unwrap_or_default();
            editor.prefs.borrow_mut().theme = t;
            editor.apply_prefs();
            editor.remember();
            editor.refresh_preview();
        }
    ));
    let style = adw::ComboRow::builder()
        .title("Style")
        // Cùng thứ tự với `Style::ALL`.
        .model(&gtk::StringList::new(&["System", "Light", "Dark"]))
        .selected(
            state::Style::ALL
                .iter()
                .position(|s| *s == prefs.style)
                .unwrap_or(0) as u32,
        )
        .build();
    style.connect_selected_notify(clone!(
        #[strong]
        update,
        move |r| {
            let s = state::Style::ALL
                .get(r.selected() as usize)
                .copied()
                .unwrap_or_default();
            update(&|p| p.style = s);
        }
    ));
    let sync = adw::SwitchRow::builder()
        .title("Sync Scrolling")
        .subtitle("Side by side: the preview follows the editor")
        .active(prefs.sync_scroll)
        .build();
    sync.connect_active_notify(clone!(
        #[strong]
        editor,
        move |r| {
            editor.prefs.borrow_mut().sync_scroll = r.is_active();
            editor.remember();
            editor.sync_preview();
        }
    ));
    let look = adw::PreferencesGroup::builder().title("Appearance").build();
    look.add(&style);
    look.add(&theme);
    look.add(&sync);
    // Tags change what is a tag: re-colour the editor, re-render, re-list (or hide) the panel.
    let tags_update = {
        let editor = editor.clone();
        move |change: &dyn Fn(&mut state::Prefs)| {
            update(change);
            editor.schedule_tags();
            editor.refresh_preview();
            editor.load_tags();
        }
    };
    let tags_on = adw::SwitchRow::builder()
        .title("Tags")
        .subtitle("#tags in the editor, preview and sidebar")
        .active(prefs.tags)
        .build();
    tags_on.connect_active_notify(clone!(
        #[strong]
        tags_update,
        move |r| {
            let on = r.is_active();
            tags_update(&|p| p.tags = on);
        }
    ));
    let colours = adw::SwitchRow::builder()
        .title("Ignore Colour Codes")
        .subtitle("#fff, #a1b2c3 (3, 4, 6 or 8 hex digits) are not tags")
        .active(prefs.ignore_colours)
        .build();
    tags_on
        .bind_property("active", &colours, "sensitive")
        .sync_create()
        .build();
    colours.connect_active_notify(move |r| {
        let on = r.is_active();
        tags_update(&|p| p.ignore_colours = on);
    });
    let tag_group = adw::PreferencesGroup::builder().title("Tags").build();
    tag_group.add(&tags_on);
    tag_group.add(&colours);
    // Saved as typed; nothing to re-apply (Today's Note reads it when used).
    let daily = adw::EntryRow::builder()
        .title("Daily Notes Folder")
        .text(prefs.daily_folder.as_str())
        .build();
    daily.connect_changed(clone!(
        #[strong]
        editor,
        move |row| {
            editor.prefs.borrow_mut().daily_folder = row.text().to_string();
            editor.remember();
        }
    ));
    let notes_group = adw::PreferencesGroup::builder()
        .title("Notes")
        .description("Today's Note (Ctrl+D) opens YYYY-MM-DD.md in this folder of the open folder; empty: the open folder itself")
        .build();
    notes_group.add(&daily);
    let page = adw::PreferencesPage::new();
    page.add(&group);
    page.add(&look);
    page.add(&tag_group);
    page.add(&notes_group);
    page.add(&default_app_group(editor));
    let dialog = adw::PreferencesDialog::new();
    dialog.set_search_enabled(true);
    dialog.add(&page);
    dialog.present(Some(&editor.window));
}

/// The MIME types the .desktop file lists for Markdown notes.
const MARKDOWN_TYPES: [&str; 2] = ["text/markdown", "text/x-markdown"];

/// Whether `default_id` (the desktop id of the app now opening `text/markdown`) is ours.
fn is_default_app(default_id: Option<&str>) -> bool {
    default_id == Some(DESKTOP_ID)
}

fn default_app_subtitle(is_default: bool) -> &'static str {
    if is_default {
        "MarkdownMate opens Markdown files"
    } else {
        "Another app opens Markdown files"
    }
}

/// Make MarkdownMate the default app for Markdown files; the error is a toast message.
fn set_default_app() -> Result<(), String> {
    let app = gio::DesktopAppInfo::new(DESKTOP_ID)
        .ok_or("MarkdownMate is not installed, so it can't be the default app")?;
    for mime in MARKDOWN_TYPES {
        app.set_as_default_for_type(mime)
            .map_err(|e| format!("Could not set the default app: {e}"))?;
    }
    Ok(())
}

/// Preferences ▸ Default App: whether MarkdownMate is the default for Markdown files
/// (read when Preferences opens), with a button that makes it so. Nothing at launch.
fn default_app_group(editor: &Rc<Editor>) -> adw::PreferencesGroup {
    let current = gio::AppInfo::default_for_type(MARKDOWN_TYPES[0], false);
    let is_default = is_default_app(current.as_ref().and_then(|a| a.id()).as_deref());
    let row = adw::ActionRow::builder()
        .title("Default App for Markdown Files")
        .subtitle(default_app_subtitle(is_default))
        .build();
    let button = gtk::Button::builder()
        .label("Make Default")
        .valign(gtk::Align::Center)
        .sensitive(!is_default)
        .build();
    button.connect_clicked(clone!(
        #[strong]
        editor,
        #[weak]
        row,
        move |button| match set_default_app() {
            Ok(()) => {
                row.set_subtitle(default_app_subtitle(true));
                button.set_sensitive(false);
            }
            Err(msg) => editor.toast(&msg),
        }
    ));
    row.add_suffix(&button);
    let group = adw::PreferencesGroup::builder()
        .title("Default App")
        .build();
    group.add(&row);
    group
}

pub(crate) fn bookmarks_dialog(editor: &Rc<Editor>) {
    let root = editor.root.borrow().clone();
    let marked = root
        .as_ref()
        .is_some_and(|r| editor.bookmarks.borrow().contains(r));
    let toggle = gtk::Button::builder()
        .label(if marked {
            "Remove Bookmark"
        } else {
            "Bookmark This Folder"
        })
        .sensitive(root.is_some())
        .build();
    let folder_row = |dir: &Path, key: Option<String>| {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&gtk::Label::new(Some(&file_name(dir))));
        let full = gtk::Label::builder()
            .label(dir.to_string_lossy())
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .css_classes(["dim-label", "caption"])
            .build();
        row.append(&full);
        if let Some(key) = key {
            let key = gtk::Label::new(Some(&key));
            key.add_css_class("dim-label");
            row.append(&key);
        }
        row
    };
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    for (i, dir) in editor.bookmarks.borrow().iter().enumerate() {
        list.append(&folder_row(dir, (i < 9).then(|| format!("Alt+{}", i + 1))));
    }
    // Recent folders other than the open one (row i = recent[i]).
    let recent: Vec<PathBuf> = (editor.recent.borrow().iter())
        .filter(|r| Some(*r) != root.as_ref())
        .cloned()
        .collect();
    let recent_list = gtk::ListBox::new();
    recent_list.add_css_class("navigation-sidebar");
    for dir in &recent {
        recent_list.append(&folder_row(dir, None));
    }
    let lists = gtk::Box::new(gtk::Orientation::Vertical, 6);
    lists.append(&list);
    if !recent.is_empty() {
        let title = gtk::Label::builder()
            .label("Recent")
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        lists.append(&title);
        lists.append(&recent_list);
    }
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    body.append(&toggle);
    body.append(
        &gtk::ScrolledWindow::builder()
            .child(&lists)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build(),
    );
    let dialog = adw::Dialog::builder()
        .title("Bookmarks")
        .content_width(480)
        .content_height(360)
        .child(&body)
        .build();
    let (weak, d) = (Rc::downgrade(editor), dialog.downgrade());
    toggle.connect_clicked(clone!(
        #[strong]
        weak,
        #[strong]
        d,
        move |_| {
            if let (Some(editor), Some(root)) = (weak.upgrade(), root.as_ref()) {
                state::toggle_bookmark(&mut editor.bookmarks.borrow_mut(), root, !marked);
                editor.remember();
                editor.toast(if marked {
                    "Bookmark removed"
                } else {
                    "Folder bookmarked"
                });
            }
            if let Some(d) = d.upgrade() {
                d.close();
            }
        }
    ));
    list.connect_row_activated(clone!(
        #[strong]
        weak,
        #[strong]
        d,
        move |_, row| {
            if let Some(d) = d.upgrade() {
                d.close();
            }
            if let (Some(editor), Ok(i)) = (weak.upgrade(), usize::try_from(row.index())) {
                editor.open_bookmark(i);
            }
        }
    ));
    recent_list.connect_row_activated(move |_, row| {
        if let Some(d) = d.upgrade() {
            d.close();
        }
        let dir = usize::try_from(row.index())
            .ok()
            .and_then(|i| recent.get(i));
        if let (Some(editor), Some(dir)) = (weak.upgrade(), dir) {
            editor.open_recent(dir);
        }
    });
    dialog.present(Some(&editor.window));
    match list.row_at_index(0) {
        Some(row) if !toggle.is_sensitive() || marked => row.grab_focus(),
        _ => toggle.grab_focus(),
    };
}

/// Ctrl+R: reloads the open note from disk; with unsaved edits, asks first. No note open:
/// does nothing.
pub(crate) fn reload_note(editor: &Rc<Editor>) {
    if editor.path.borrow().is_none() {
        return;
    }
    let (start, end) = editor.buffer.bounds();
    if editor.buffer.text(&start, &end, true) == editor.saved.borrow().as_str() {
        return editor.reload();
    }
    // Pending auto-save would write the edits (or a conflict copy) while the dialog is open.
    editor.cancel_timer();
    let dialog = adw::AlertDialog::builder()
        .heading("Discard unsaved changes and reload?")
        .body("The note is read again from disk; your edits since the last save are lost.")
        .default_response("cancel")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("reload", "Reload")]);
    dialog.set_response_appearance("reload", adw::ResponseAppearance::Destructive);
    dialog.connect_response(
        None,
        clone!(
            #[strong]
            editor,
            move |_, response| {
                if response == "reload" {
                    editor.reload();
                } else {
                    // Kept the edits: resume auto-save.
                    editor.schedule_save();
                }
            }
        ),
    );
    dialog.present(Some(&editor.window));
}

/// Ctrl+L: small box for `line` or `line:column`. Enter jumps (bad input marks the box red
/// and stays), Esc closes (adw::Dialog default).
pub(crate) fn go_to_line(editor: &Rc<Editor>) {
    if editor.path.borrow().is_none() {
        return;
    }
    let entry = gtk::Entry::builder()
        .placeholder_text("Line or line:column")
        .activates_default(false)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    let dialog = adw::Dialog::builder()
        .title("Go to Line")
        .content_width(300)
        .child(&entry)
        .build();
    entry.connect_changed(|e| e.remove_css_class("error"));
    entry.connect_activate(clone!(
        #[strong]
        editor,
        #[weak]
        dialog,
        move |e| match goto::parse(&e.text()) {
            Some((line, column)) => {
                dialog.close();
                editor.go_to(line, column);
            }
            None => e.add_css_class("error"),
        }
    ));
    dialog.present(Some(&editor.window));
    entry.grab_focus();
}

/// Ctrl+P: search box over the notes of the open folder. Up/Down select, Enter opens the
/// selected note, or creates a note named after the query in the folder root when nothing
/// matches. Esc closes (adw::Dialog default).
pub(crate) fn open_quickly(editor: &Rc<Editor>) {
    let Some(root) = editor.root.borrow().clone() else {
        return;
    };
    // The last scan, plus notes the app created/moved/trashed since (`update_note_list`).
    let notes = editor.note_list.borrow().clone();
    // Empty query: notes opened lately (remembered cursor positions, newest first).
    let history: Vec<PathBuf> = editor
        .positions
        .borrow()
        .iter()
        .map(|p| p.note.clone())
        .collect();
    let current = editor.path.borrow().clone();

    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Note name")
        // Results update on every key, so Enter never acts on a stale list.
        .search_delay(0)
        .build();
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&entry);
    content.append(&scroll);
    let dialog = adw::Dialog::builder()
        .title("Open Quickly")
        .content_width(520)
        .content_height(420)
        .child(&content)
        .build();

    let shown: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let fill = {
        let (list, notes, shown, root) = (list.clone(), notes.clone(), shown.clone(), root.clone());
        move |query: &str| {
            let recent = quick::recent(
                history.iter().map(PathBuf::as_path),
                &notes,
                current.as_deref(),
            );
            // ListBox::remove_all needs GTK 4.12; the crate targets 4.10.
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
            // Nothing opened yet: fall back to every note by path.
            let hits = if query.trim().is_empty() && !recent.is_empty() {
                recent
            } else {
                quick::search(query, &notes, &root)
            };
            let hits: Vec<PathBuf> = hits.into_iter().cloned().collect();
            for path in &hits {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
                row.append(
                    &gtk::Label::builder()
                        .label(file_name(path))
                        .xalign(0.0)
                        .build(),
                );
                let dim = gtk::Label::builder()
                    .label(rel.to_string_lossy())
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build();
                dim.add_css_class("dim-label");
                dim.add_css_class("caption");
                row.append(&dim);
                list.append(&row);
            }
            list.select_row(list.row_at_index(0).as_ref());
            shown.replace(hits);
        }
    };
    fill("");
    entry.connect_search_changed(move |e| fill(&e.text()));

    let choose = {
        let (editor, dialog, list, shown) =
            (editor.clone(), dialog.downgrade(), list.clone(), shown);
        move |query: &str| {
            let index = list.selected_row().map(|r| r.index());
            let path = index.and_then(|i| shown.borrow().get(i as usize).cloned());
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            match path {
                Some(path) => editor.open(&path),
                None if shown.borrow().is_empty() => match naming::note_name(query) {
                    Ok(name) => editor.new_note(&root, &name),
                    Err(msg) => editor.toast(msg),
                },
                None => {}
            }
        }
    };
    let choose = Rc::new(choose);
    entry.connect_activate(clone!(
        #[strong]
        choose,
        move |e| choose(&e.text())
    ));
    list.connect_row_activated(clone!(
        #[weak]
        entry,
        move |list, row| {
            list.select_row(Some(row));
            choose(&entry.text());
        }
    ));

    // Up/Down move the selection while typing stays in the search box.
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(clone!(
        #[weak]
        list,
        #[weak]
        scroll,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, _| {
            let step = match key {
                gtk::gdk::Key::Down => 1,
                gtk::gdk::Key::Up => -1,
                _ => return glib::Propagation::Proceed,
            };
            let at = list.selected_row().map_or(-1, |r| r.index());
            if let Some(row) = list.row_at_index((at + step).max(0)) {
                list.select_row(Some(&row));
                // Scroll the row into view; focus stays in the entry so typing goes on.
                if let Some(top) = row.compute_point(&list, &gtk::graphene::Point::zero()) {
                    let top = f64::from(top.y());
                    scroll
                        .vadjustment()
                        .clamp_page(top, top + f64::from(row.height()));
                }
            }
            glib::Propagation::Stop
        }
    ));
    entry.add_controller(keys);

    dialog.present(Some(&editor.window));
    entry.grab_focus();
}

/// Pango markup for a snippet with its match ranges in bold on a yellow background.
pub(crate) fn highlight(text: &str, ranges: &[std::ops::Range<usize>]) -> String {
    let mut out = String::new();
    let mut at = 0;
    for r in ranges {
        out.push_str(&glib::markup_escape_text(&text[at..r.start]));
        out.push_str("<span weight=\"bold\" background=\"#f8e45c\" foreground=\"#000000\">");
        out.push_str(&glib::markup_escape_text(&text[r.clone()]));
        out.push_str("</span>");
        at = r.end;
    }
    out.push_str(&glib::markup_escape_text(&text[at..]));
    out
}

/// Shift+Ctrl+F: full-text search over the notes of the open folder (the last scan's list,
/// read from disk: unsaved edits of the open note are not searched). Runs on a background
/// thread, 250 ms after typing stops; a newer query cancels the older one. Activating a
/// matching line opens the note at that line. Esc closes.
pub(crate) fn find_in_folder(editor: &Rc<Editor>) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let Some(root) = editor.root.borrow().clone() else {
        return;
    };
    let notes: Arc<Vec<PathBuf>> = Arc::new(editor.note_list.borrow().to_vec());
    // The open note is searched as in the buffer (unsaved edits); the dialog is modal, so a
    // snapshot taken now stays current.
    let open_note: Arc<Option<(PathBuf, String)>> =
        Arc::new(editor.path.borrow().clone().map(|p| {
            let (start, end) = editor.buffer.bounds();
            (p, editor.buffer.text(&start, &end, true).to_string())
        }));

    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Text in notes")
        .search_delay(250)
        .build();
    let status = gtk::Label::builder().xalign(0.0).build();
    status.add_css_class("dim-label");
    status.add_css_class("caption");
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    list.add_css_class("find-results");
    let scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&entry);
    content.append(&status);
    content.append(&scroll);
    let dialog = adw::Dialog::builder()
        .title("Find in Folder")
        .content_width(640)
        .content_height(520)
        .child(&content)
        .build();

    // Row index → (note, line).
    let targets: Rc<RefCell<Vec<(PathBuf, usize)>>> = Rc::default();
    // Bumped by every query and on close; a search whose number is stale stops / is dropped.
    let generation = Arc::new(AtomicU64::new(0));

    let show = {
        let (list, status, targets, root) = (list.clone(), status.clone(), targets.clone(), root);
        move |found: find::Found, query: &str| {
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
            let mut rows = Vec::new();
            let mut count = 0;
            for file in &found.files {
                let rel = file.path.strip_prefix(&root).unwrap_or(&file.path);
                for (i, hit) in file.lines.iter().enumerate() {
                    let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
                    // The file's name and path head its first match (a separate header row
                    // would take keyboard focus without opening anything).
                    if i == 0 {
                        let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        let name = gtk::Label::new(Some(&file_name(&file.path)));
                        name.add_css_class("heading");
                        head.append(&name);
                        let path = gtk::Label::builder()
                            .label(rel.to_string_lossy())
                            .xalign(0.0)
                            .ellipsize(gtk::pango::EllipsizeMode::Middle)
                            .build();
                        path.add_css_class("dim-label");
                        path.add_css_class("caption");
                        head.append(&path);
                        if count > 0 {
                            head.set_margin_top(8);
                        }
                        row.append(&head);
                    }
                    let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let number = gtk::Label::builder()
                        .label(hit.line.to_string())
                        .width_chars(5)
                        .xalign(1.0)
                        .build();
                    number.add_css_class("dim-label");
                    number.add_css_class("numeric");
                    line.append(&number);
                    line.append(
                        &gtk::Label::builder()
                            .use_markup(true)
                            .label(highlight(&hit.snippet, &hit.ranges))
                            .xalign(0.0)
                            .ellipsize(gtk::pango::EllipsizeMode::End)
                            .build(),
                    );
                    row.append(&line);
                    list.append(&row);
                    rows.push((file.path.clone(), hit.line));
                    count += 1;
                }
            }
            targets.replace(rows);
            status.set_label(&match (count, found.more) {
                (_, true) => format!("First {count} matching lines (more not shown)"),
                (0, _) if query.trim().is_empty() => String::new(),
                (0, _) => "No matches".to_string(),
                (1, _) => "1 matching line".to_string(),
                _ => format!("{count} matching lines"),
            });
        }
    };
    let show = Rc::new(show);

    entry.connect_search_changed(clone!(
        #[strong]
        generation,
        #[strong]
        show,
        move |e| {
            let query = e.text().to_string();
            let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let (notes, open_note) = (notes.clone(), open_note.clone());
            let (generation, show) = (generation.clone(), show.clone());
            glib::spawn_future_local(async move {
                let g = generation.clone();
                let q = query.clone();
                let found = gio::spawn_blocking(move || {
                    find::search(&notes, open_note.as_ref().as_ref(), &q, &|| {
                        g.load(Ordering::SeqCst) != mine
                    })
                })
                .await;
                if let Ok(Some(found)) = found
                    && generation.load(Ordering::SeqCst) == mine
                {
                    show(found, &query);
                }
            });
        }
    ));
    dialog.connect_closed(clone!(
        #[strong]
        generation,
        move |_| {
            generation.fetch_add(1, Ordering::SeqCst);
        }
    ));

    let open = {
        let (editor, dialog) = (editor.clone(), dialog.downgrade());
        move |index: i32| {
            let target = usize::try_from(index)
                .ok()
                .and_then(|i| targets.borrow().get(i).cloned());
            let Some((path, line)) = target else {
                return;
            };
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            editor.open_at_line(&path, line);
        }
    };
    let open = Rc::new(open);
    list.connect_row_activated(clone!(
        #[strong]
        open,
        move |_, row| open(row.index())
    ));
    // Enter in the box opens the first match.
    entry.connect_activate(move |_| open(0));
    // Down moves from the box into the results.
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(clone!(
        #[weak]
        list,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |_, key, _, _| {
            if key != gtk::gdk::Key::Down {
                return glib::Propagation::Proceed;
            }
            if let Some(row) = list.row_at_index(0) {
                row.grab_focus();
            }
            glib::Propagation::Stop
        }
    ));
    entry.add_controller(keys);

    dialog.present(Some(&editor.window));
    entry.grab_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_app_is_recognised_by_desktop_id() {
        assert!(is_default_app(Some("com.homielab.MarkdownMate.desktop")));
        assert!(!is_default_app(Some("org.gnome.TextEditor.desktop")));
        assert!(!is_default_app(None));
    }

    #[test]
    fn desktop_file_lists_the_markdown_types() {
        let desktop = include_str!("../packaging/com.homielab.MarkdownMate.desktop");
        for mime in MARKDOWN_TYPES {
            assert!(
                desktop.contains(mime),
                "{mime} missing from the .desktop file"
            );
        }
    }
}
