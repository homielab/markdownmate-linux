use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};

use crate::a11y::{self, IconItem, Kind};
use crate::editor::Editor;
use crate::file_name;
use crate::{attachments, broken, find, goto, naming, query, quick, render, sidebar, state, tasks};

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
    a11y::label(&font, "Editor Font");
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

    let spelling = adw::SwitchRow::builder()
        .title("Check Spelling")
        .subtitle("Underline misspelled words; right-click one for suggestions")
        .active(prefs.spell_check)
        .build();
    spelling.connect_active_notify(clone!(
        #[strong]
        update,
        move |r| {
            let on = r.is_active();
            update(&|p| p.spell_check = on);
        }
    ));

    let auto_pair = adw::SwitchRow::builder()
        .title("Auto-Pair Brackets and Quotes")
        .subtitle("Insert matching brackets, backticks and quotes while typing")
        .active(prefs.auto_pair)
        .build();
    auto_pair.connect_active_notify(clone!(
        #[strong]
        update,
        move |r| {
            let on = r.is_active();
            update(&|p| p.auto_pair = on);
        }
    ));

    let spacing_names: Vec<String> = state::LINE_SPACINGS
        .iter()
        .map(|s| format!("{}", f64::from(*s) / 100.0))
        .collect();
    let spacing_names: Vec<&str> = spacing_names.iter().map(String::as_str).collect();
    let line_spacing = adw::ComboRow::builder()
        .title("Line Spacing")
        .model(&gtk::StringList::new(&spacing_names))
        .selected(
            state::LINE_SPACINGS
                .iter()
                .position(|s| *s == prefs.line_spacing)
                .unwrap_or(0) as u32,
        )
        .build();
    line_spacing.connect_selected_notify(clone!(
        #[strong]
        update,
        move |r| {
            let spacing = state::LINE_SPACINGS
                .get(r.selected() as usize)
                .copied()
                .unwrap_or(100);
            update(&|p| p.line_spacing = spacing);
        }
    ));

    let width_names: Vec<&str> = state::TextWidth::ALL.iter().map(|w| w.title()).collect();
    let text_width = adw::ComboRow::builder()
        .title("Text Width")
        .subtitle("Of the editor and preview")
        .model(&gtk::StringList::new(&width_names))
        .selected(
            state::TextWidth::ALL
                .iter()
                .position(|w| *w == prefs.text_width)
                .unwrap_or(0) as u32,
        )
        .build();
    text_width.connect_selected_notify(clone!(
        #[strong]
        editor,
        move |r| {
            let width = state::TextWidth::ALL
                .get(r.selected() as usize)
                .copied()
                .unwrap_or_default();
            editor.prefs.borrow_mut().text_width = width;
            editor.apply_prefs();
            editor.remember();
            editor.refresh_preview();
        }
    ));

    let group = adw::PreferencesGroup::builder().title("Editor").build();
    group.add(&font_row);
    group.add(&numbers);
    group.add(&wrap);
    group.add(&indent);
    group.add(&line_spacing);
    group.add(&text_width);
    group.add(&typewriter);
    group.add(&spelling);
    group.add(&auto_pair);

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
    let line_breaks = adw::ComboRow::builder()
        .title("Line Breaks")
        .subtitle(
            "Every New Line: each newline starts a new line in the preview, print and exports",
        )
        .model(&gtk::StringList::new(&[
            "Markdown Standard",
            "Every New Line",
        ]))
        .selected(u32::from(prefs.hard_breaks))
        .build();
    line_breaks.connect_selected_notify(clone!(
        #[strong]
        editor,
        move |r| {
            let on = r.selected() == 1;
            editor.prefs.borrow_mut().hard_breaks = on;
            render::set_hard_breaks(on);
            editor.remember();
            editor.refresh_preview();
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
    // What the preview, print and exports make of the text; the editor keeps it as typed.
    let smart = adw::SwitchRow::builder()
        .title("Smart Punctuation")
        .subtitle("“Curly quotes”, – and — dashes and … in the preview, print and exports")
        .active(prefs.smart_punctuation)
        .build();
    smart.connect_active_notify(clone!(
        #[strong]
        editor,
        move |r| {
            let on = r.is_active();
            editor.prefs.borrow_mut().smart_punctuation = on;
            crate::smart::set_enabled(on);
            editor.remember();
            editor.refresh_preview();
        }
    ));
    let front_matter = adw::ComboRow::builder()
        .title("Front Matter")
        .subtitle("How the YAML properties at the top of a note show in the preview")
        .model(&gtk::StringList::new(
            &state::FrontMatter::ALL.map(state::FrontMatter::title),
        ))
        .selected(
            state::FrontMatter::ALL
                .iter()
                .position(|m| *m == prefs.front_matter)
                .unwrap_or(0) as u32,
        )
        .build();
    front_matter.connect_selected_notify(clone!(
        #[strong]
        editor,
        move |r| {
            let mode = state::FrontMatter::ALL
                .get(r.selected() as usize)
                .copied()
                .unwrap_or_default();
            editor.prefs.borrow_mut().front_matter = mode;
            render::set_front_matter(mode);
            editor.remember();
            editor.refresh_preview();
        }
    ));
    let preview_group = adw::PreferencesGroup::builder().title("Preview").build();
    preview_group.add(&line_breaks);
    preview_group.add(&smart);
    preview_group.add(&front_matter);
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
    let web_images = adw::SwitchRow::builder()
        .title("Load Images from the Web")
        .subtitle("In the preview, print and exports. Off: nothing is downloaded")
        .active(prefs.web_images)
        .build();
    web_images.connect_active_notify(clone!(
        #[strong]
        editor,
        move |r| {
            editor.prefs.borrow_mut().web_images = r.is_active();
            editor.remember();
            // A page keeps the CSP it was loaded with: load it again (also a hidden one,
            // which coming back to Preview would only update in place).
            if editor.preview_view.get().is_some() {
                editor.load_preview();
            }
        }
    ));
    let privacy = adw::PreferencesGroup::builder().title("Privacy").build();
    privacy.add(&web_images);

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
    // Saved when valid (trimmed); otherwise the problem shows in the row below and the last
    // valid pattern stays in use.
    let daily_name = adw::EntryRow::builder()
        .title("Daily Note Name")
        .text(prefs.daily_name.as_str())
        .build();
    let daily_name_problem = adw::ActionRow::builder()
        .use_markup(false)
        .title_lines(0)
        .activatable(false)
        .css_classes(["error"])
        .visible(false)
        .build();
    daily_name.connect_changed(clone!(
        #[strong]
        editor,
        #[weak]
        daily_name_problem,
        move |row| match crate::dailyname::Pattern::new(&row.text()) {
            Ok(pattern) => {
                row.remove_css_class("error");
                daily_name_problem.set_visible(false);
                editor.prefs.borrow_mut().daily_name = pattern.text().to_string();
                editor.remember();
                editor.update_history_actions();
            }
            Err(problem) => {
                row.add_css_class("error");
                daily_name_problem.set_title(&problem);
                daily_name_problem.set_visible(true);
            }
        }
    ));
    // Saved as typed; the main menu reads it each time it opens.
    let templates = adw::EntryRow::builder()
        .title("Templates Folder")
        .text(prefs.templates_folder.as_str())
        .build();
    templates.connect_changed(clone!(
        #[strong]
        editor,
        move |row| {
            editor.prefs.borrow_mut().templates_folder = row.text().to_string();
            editor.remember();
        }
    ));
    let attachments = adw::EntryRow::builder()
        .title("Attachments Folder")
        .text(prefs.attachments_folder.as_str())
        .build();
    attachments.connect_changed(clone!(
        #[strong]
        editor,
        move |row| {
            editor.prefs.borrow_mut().attachments_folder = row.text().to_string();
            editor.remember();
        }
    ));
    // Image data pasted from the clipboard only (`images::convert`); dropped files, GIF and SVG
    // are stored as they are.
    let pasted_format = adw::ComboRow::builder()
        .title("Pasted Images")
        .subtitle("Format of images pasted from the clipboard; dropped files, GIF and SVG stay as they are")
        .model(&gtk::StringList::new(
            &crate::images::PastedFormat::ALL.map(crate::images::PastedFormat::title),
        ))
        .selected(
            crate::images::PastedFormat::ALL
                .iter()
                .position(|f| *f == prefs.pasted_image_format)
                .unwrap_or(0) as u32,
        )
        .build();
    pasted_format.connect_selected_notify(clone!(
        #[strong]
        editor,
        move |r| {
            editor.prefs.borrow_mut().pasted_image_format = crate::images::PastedFormat::ALL
                .get(r.selected() as usize)
                .copied()
                .unwrap_or_default();
            editor.remember();
        }
    ));
    let width_titles: Vec<String> = crate::images::MAX_WIDTHS
        .iter()
        .map(|w| match w {
            0 => "Off".to_string(),
            w => format!("{w} px"),
        })
        .collect();
    let width_titles: Vec<&str> = width_titles.iter().map(String::as_str).collect();
    let pasted_width = adw::ComboRow::builder()
        .title("Scale Down To")
        .subtitle("Wider pasted images are saved at this width, keeping their proportions")
        .model(&gtk::StringList::new(&width_titles))
        .selected(
            crate::images::MAX_WIDTHS
                .iter()
                .position(|w| *w == prefs.pasted_image_max_width)
                .unwrap_or(0) as u32,
        )
        .build();
    pasted_width.connect_selected_notify(clone!(
        #[strong]
        editor,
        move |r| {
            editor.prefs.borrow_mut().pasted_image_max_width = crate::images::MAX_WIDTHS
                .get(r.selected() as usize)
                .copied()
                .unwrap_or(0);
            editor.remember();
        }
    ));
    let notes_group = adw::PreferencesGroup::builder()
        .title("Notes")
        .description("Today's Note (Ctrl+D) opens YYYY-MM-DD.md in this folder of the open folder; empty: the open folder itself. Daily Note Name sets another name from the date tokens YYYY YY MMMM MMM MM M DD D dddd ddd, with [text] kept as written and / for subfolders: YYYY/MM/YYYY-MM-DD keeps 2026-11-02.md in 2026/11. A Template.md in this folder sets the initial text for new daily notes, replacing {{date}}, {{time}} and {{title}} (the file name) with today's date and time (formats like {{date:dddd, MMMM D, YYYY}}). New from Template (main menu) lists the .md notes of the Templates Folder and creates an Untitled note from one, with {{date}}, {{time}}, {{title}} and {{cursor}}. Attachments Folder: images you paste or drop are saved here, next to the note. Start with / for a folder at the top of the open folder.")
        .build();
    notes_group.add(&daily);
    notes_group.add(&daily_name);
    notes_group.add(&daily_name_problem);
    let open_today = adw::SwitchRow::builder()
        .title("Open Today's Note at Launch")
        .active(prefs.open_today_at_launch)
        .build();
    open_today.connect_active_notify(clone!(
        #[strong]
        editor,
        move |r| {
            editor.prefs.borrow_mut().open_today_at_launch = r.is_active();
            editor.remember();
        }
    ));
    notes_group.add(&open_today);
    notes_group.add(&templates);
    notes_group.add(&attachments);
    notes_group.add(&pasted_format);
    notes_group.add(&pasted_width);
    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();
    page.add(&group);
    page.add(&look);
    page.add(&preview_group);
    page.add(&tag_group);
    page.add(&notes_group);
    page.add(&privacy);
    page.add(&default_app_group(editor));
    let dialog = adw::PreferencesDialog::new();
    dialog.set_search_enabled(true);
    dialog.add(&page);
    dialog.add(&crate::about::about_page(editor, &dialog));
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
        let label = a11y::located(
            &a11y::row(&file_name(dir), Kind::Folder, false),
            &dir.parent().unwrap_or(dir).to_string_lossy(),
        );
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
        let row = gtk::ListBoxRow::builder().child(&row).build();
        a11y::label(&row, &label);
        row
    };
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    a11y::label(&list, "Bookmarks");
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
    a11y::label(&recent_list, "Recent");
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
    a11y::label(&entry, "Go to Line");
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

/// Ctrl+P: search box over the notes of the open folder, their front-matter aliases ("Alias →
/// Note" rows) and its Favorites (with a star). Up/Down select, Enter opens the selected note
/// (a favourite folder is shown in the sidebar), or creates a note named after the query in
/// the folder root when no name or alias matches. A query starting with `#` lists the headings
/// of every note instead ("Heading — Note", from the link index; never a Create row), and Enter
/// opens the note at that heading as a `[[Note#Heading]]` link does. Esc closes (adw::Dialog
/// default).
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
    let favorites = editor.favorite_paths();
    // The last alias index (`load_aliases`); no file is read here.
    let aliases = Arc::clone(&editor.aliases.borrow());

    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Note name or alias, # for headings")
        // Results update on every key, so Enter never acts on a stale list.
        .search_delay(0)
        .build();
    a11y::label(&entry, "Open Quickly");
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    a11y::label(&list, "Notes");
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

    // Each row's note and, for a heading row, the heading's link fragment.
    type Shown = Vec<(PathBuf, Option<String>)>;
    let shown: Rc<RefCell<Shown>> = Rc::default();
    // Every note's headings, copied from the link index at the first `#` query.
    let headings: Rc<RefCell<Option<Vec<quick::NoteHeadings>>>> = Rc::default();
    let fill = {
        let (list, notes, shown, root) = (list.clone(), notes.clone(), shown.clone(), root.clone());
        let headings = headings.clone();
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
            if let Some(query) = quick::heading_query(query) {
                let hits = headings
                    .borrow()
                    .as_ref()
                    .map_or_else(Vec::new, |all| quick::headings(&query, all));
                for hit in &hits {
                    let rel = hit.path.strip_prefix(&root).unwrap_or(&hit.path);
                    let name = a11y::row(&hit.label(), Kind::Heading, false);
                    list.append(&quick_row(&hit.label(), rel, false, &name));
                }
                list.select_row(list.row_at_index(0).as_ref());
                shown.replace(
                    hits.into_iter()
                        .map(|hit| (hit.path, Some(hit.heading.fragment)))
                        .collect(),
                );
                return;
            }
            // Nothing opened yet: favourites, else every note by path.
            let hits = quick::with_favorites(
                query,
                &favorites,
                current.as_deref(),
                &root,
                &notes,
                &recent,
                &aliases,
            );
            for hit in &hits {
                let path = &hit.path;
                let rel = path.strip_prefix(&root).unwrap_or(path);
                // As `[[` completion words it: "Alias → Note".
                let label = match &hit.alias {
                    Some(alias) => {
                        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                        format!("{alias} → {stem}")
                    }
                    None => file_name(path),
                };
                // Favorites can be folders; any other row is a note.
                let kind = if hit.favorite && path.is_dir() {
                    Kind::Folder
                } else {
                    Kind::Note
                };
                let name = a11y::row(&label, kind, hit.favorite);
                list.append(&quick_row(&label, rel, hit.favorite, &name));
            }
            list.select_row(list.row_at_index(0).as_ref());
            shown.replace(hits.into_iter().map(|hit| (hit.path, None)).collect());
        }
    };
    let fill = Rc::new(fill);
    fill("");
    let asked = std::cell::Cell::new(false);
    let weak = Rc::downgrade(editor);
    entry.connect_search_changed(move |e| {
        let text = e.text();
        // The first `#` query copies the headings off the link index in the background (it
        // may still be building), with the open note's unsaved text; the list fills when
        // they arrive.
        if quick::heading_query(&text).is_some()
            && !asked.replace(true)
            && let Some(editor) = weak.upgrade()
        {
            let open = editor.path.borrow().clone().map(|path| {
                let (start, end) = editor.buffer.bounds();
                (path, editor.buffer.text(&start, &end, true).to_string())
            });
            let job = editor.links_job(move |index| {
                index.headings(open.as_ref().map(|(p, t)| (p.as_path(), t.as_str())))
            });
            if let Some(job) = job {
                let (fill, headings, entry) = (fill.clone(), headings.clone(), e.downgrade());
                glib::spawn_future_local(async move {
                    if let Ok(all) = gio::spawn_blocking(job).await {
                        headings.replace(Some(all));
                        if let Some(entry) = entry.upgrade() {
                            fill(&entry.text());
                        }
                    }
                });
            }
        }
        fill(&text);
    });

    let choose = {
        let (editor, dialog, list, shown) =
            (editor.clone(), dialog.downgrade(), list.clone(), shown);
        move |query: &str| {
            let index = list.selected_row().map(|r| r.index());
            let picked = index.and_then(|i| shown.borrow().get(i as usize).cloned());
            if let Some(dialog) = dialog.upgrade() {
                dialog.close();
            }
            match picked {
                Some((path, Some(heading))) => {
                    let reload = editor.path.borrow().as_deref() != Some(path.as_path());
                    editor.open(&path);
                    if editor.path.borrow().as_deref() == Some(path.as_path()) {
                        editor.go_to_heading(&heading, reload);
                    }
                }
                Some((path, None)) if path.is_dir() => editor.show_in_sidebar(&path),
                Some((path, None)) => editor.open(&path),
                // A heading query never creates a note.
                None if quick::heading_query(query).is_some() => {}
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

/// An Open Quickly row: `label` (after a star for a favourite) over the dimmed relative path
/// `rel`. Screen readers read `name` (`a11y::row`) and the folder it is in instead.
fn quick_row(label: &str, rel: &Path, favorite: bool, name: &str) -> gtk::ListBoxRow {
    let folder = rel.parent().unwrap_or(Path::new("")).to_string_lossy();
    let accessible = a11y::located(name, &folder);
    let rel = rel.to_string_lossy();
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let name = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    if favorite {
        let star = gtk::Image::from_icon_name("starred-symbolic");
        star.set_tooltip_text(Some("Favorite"));
        star.update_property(&[gtk::accessible::Property::Label("Favorite")]);
        name.append(&star);
    }
    let label = gtk::Label::builder()
        .label(label)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name.append(&label);
    row.append(&name);
    let dim = gtk::Label::builder()
        .label(rel)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    dim.add_css_class("dim-label");
    dim.add_css_class("caption");
    row.append(&dim);
    let row = gtk::ListBoxRow::builder().child(&row).build();
    a11y::label(&row, &accessible);
    row
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

/// Find in Folder's ✕ on the scope chip ("In Folder/Sub") and on the scan mode chip.
const SCOPE_CLEAR: IconItem = IconItem {
    label: "Search the whole folder",
    tooltip: "Search the whole folder",
    icon: "window-close-symbolic",
};
const MODE_CLEAR: IconItem = IconItem {
    label: "Search text again",
    tooltip: "Search text again",
    icon: "window-close-symbolic",
};

/// Shift+Ctrl+F: full-text search over the notes of the open folder (the last scan's list,
/// read from disk: unsaved edits of the open note are not searched). Runs on a background
/// thread, 250 ms after typing stops; a newer query cancels the older one. Activating a
/// matching line opens the note at that line. With Regex on, an invalid pattern shows its error
/// under the box and no results. `tag:name` / `path:folder/` words (`query.rs`) narrow the notes
/// searched; with no text left they list the matching notes, which Replace All leaves alone.
/// Replace All replaces the listed matches after a confirmation (`replace_all`). Esc closes.
pub(crate) fn find_in_folder(editor: &Rc<Editor>) {
    open_find(editor, None, FindMode::Text);
}

/// What Find in Folder lists: text matches, or the result of one of its scans. The scan modes
/// exclude each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindMode {
    Text,
    /// The links of the folder's notes that lead nowhere (`broken.rs`).
    BrokenLinks,
    /// The unchecked `- [ ]` tasks of the folder's notes (`tasks.rs`).
    OpenTasks,
    /// The files in `assets` folders no note uses (`attachments.rs`).
    UnusedAttachments,
}

/// Main menu ▸ Find Broken Links… / Find Open Tasks… / Find Unused Attachments…: Find in Folder
/// in that scan mode instead of searching text. The search box gives way to a chip ("Broken
/// links", "Open tasks", "Unused attachments") whose ✕ (or the mode's toggle) returns to text
/// search. Scanned when shown, when the toggle is switched on or the menu entry chosen again;
/// not live.
pub(crate) fn find_in_mode(editor: &Rc<Editor>, mode: FindMode) {
    let set = editor.find_mode.borrow().clone();
    match set {
        Some(set) => set(mode),
        None => open_find(editor, None, mode),
    }
}

/// Sidebar ▸ Find in “Folder”…: Find in Folder limited to the notes under `folder`, shown as
/// an "In Folder/Sub" chip whose ✕ widens it to the whole folder again. With the dialog
/// already open, its scope becomes `folder` and the search runs again.
pub(crate) fn find_in_scope(editor: &Rc<Editor>, folder: &Path) {
    let set = editor.find_set_scope.borrow().clone();
    match set {
        Some(set) => set(Some(folder.to_path_buf())),
        None => open_find(editor, Some(folder.to_path_buf()), FindMode::Text),
    }
}

fn open_find(editor: &Rc<Editor>, scope: Option<PathBuf>, mode: FindMode) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let Some(root) = editor.root.borrow().clone() else {
        return;
    };
    let all: Arc<Vec<PathBuf>> = Arc::new(editor.note_list.borrow().to_vec());
    // The notes searched for a scope: all, or those under the folder (`find::in_scope`). The
    // open folder itself, or a folder outside it, is no scope.
    let scoped = {
        let (all, root) = (all.clone(), root.clone());
        move |scope: Option<PathBuf>| {
            let scope = scope.filter(|dir| dir != &root && dir.starts_with(&root));
            let notes = match &scope {
                Some(dir) => Arc::new(find::in_scope(&all, Some(dir))),
                None => all.clone(),
            };
            (scope, notes)
        }
    };
    let (scope, notes) = scoped(scope);
    let notes = Rc::new(RefCell::new(notes));
    // The folder scope, for Broken Links (which filters the link index's notes instead) and
    // Unused Attachments (which lists the files under it, checked against all notes).
    let current_scope = Rc::new(RefCell::new(scope.clone()));
    // The open note is searched as in the buffer (unsaved edits); the dialog is modal, so a
    // snapshot taken now stays current until Replace All edits it.
    let snapshot = {
        let editor = editor.clone();
        Rc::new(move || Arc::new(editor.path.borrow().clone().map(|p| (p, editor.text()))))
    };
    let open_note = Rc::new(RefCell::new(snapshot()));

    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Text in notes, tag:name path:folder/")
        .search_delay(250)
        .build();
    a11y::label(&entry, "Find in Folder");
    let match_case = gtk::CheckButton::builder()
        .label("Match Case")
        .active(editor.find_match_case.get())
        .build();
    let whole_word = gtk::CheckButton::builder()
        .label("Whole Word")
        .active(editor.find_whole_word.get())
        .build();
    let regex = gtk::CheckButton::builder()
        .label("Regex")
        .active(editor.find_regex.get())
        .tooltip_text(
            "Search with a regular expression (Rust regex syntax), line by line. \
             In Replace with, $1…$9 or ${1} insert a group, $$ a literal $.",
        )
        .build();
    let toggles = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    toggles.append(&match_case);
    toggles.append(&whole_word);
    toggles.append(&regex);
    let broken_toggle = gtk::CheckButton::builder()
        .label("Broken Links")
        .tooltip_text(
            "List the wiki links and links to files in the notes that lead nowhere, \
             instead of searching text",
        )
        .build();
    toggles.append(&broken_toggle);
    let tasks_toggle = gtk::CheckButton::builder()
        .label("Open Tasks")
        .tooltip_text("List the unchecked tasks (- [ ]) in the notes, instead of searching text")
        .build();
    toggles.append(&tasks_toggle);
    let unused_toggle = gtk::CheckButton::builder()
        .label("Unused Attachments")
        .tooltip_text(
            "List the files in assets folders that no note links to or shows, \
             instead of searching text",
        )
        .build();
    toggles.append(&unused_toggle);
    // An invalid pattern's reason, under the box.
    let error = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .build();
    error.add_css_class("error");
    error.add_css_class("caption");
    let status = gtk::Label::builder().xalign(0.0).build();
    status.add_css_class("dim-label");
    status.add_css_class("caption");
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    list.add_css_class("find-results");
    a11y::label(&list, "Results");
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
    let replacement = gtk::Entry::builder()
        .placeholder_text("Replace with")
        .hexpand(true)
        .build();
    a11y::label(&replacement, "Replace with");
    let replace = gtk::Button::builder()
        .label("Replace All")
        .sensitive(false)
        .build();
    let replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    replace_row.append(&replacement);
    replace_row.append(&replace);
    // The scope chip: "In Folder/Sub" and ✕, shown while the search is limited to a folder.
    let scope_text = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    scope_text.add_css_class("caption");
    let scope_clear = SCOPE_CLEAR.button();
    scope_clear.set_valign(gtk::Align::Center);
    scope_clear.add_css_class("flat");
    scope_clear.add_css_class("circular");
    let scope_chip = gtk::Box::builder()
        .spacing(4)
        .halign(gtk::Align::Start)
        .css_classes(["card", "find-scope"])
        .build();
    scope_text.set_margin_start(10);
    scope_chip.append(&scope_text);
    scope_chip.append(&scope_clear);
    let show_scope = {
        let (scope_chip, scope_text, root) = (scope_chip.clone(), scope_text.clone(), root.clone());
        move |scope: Option<&Path>| {
            if let Some(dir) = scope {
                scope_text.set_label(&find::scope_label(&root, dir));
            }
            scope_chip.set_visible(scope.is_some());
        }
    };
    show_scope(scope.as_deref());
    // A scan mode (Broken Links, Open Tasks): this chip stands in for the search box; ✕
    // returns to text search.
    let mode_text = gtk::Label::builder().margin_start(10).build();
    mode_text.add_css_class("caption");
    let mode_clear = MODE_CLEAR.button();
    mode_clear.set_valign(gtk::Align::Center);
    mode_clear.add_css_class("flat");
    mode_clear.add_css_class("circular");
    let mode_chip = gtk::Box::builder()
        .spacing(4)
        .halign(gtk::Align::Start)
        .visible(false)
        .css_classes(["card", "find-scope"])
        .build();
    mode_chip.append(&mode_text);
    mode_chip.append(&mode_clear);
    content.append(&mode_chip);
    content.append(&entry);
    content.append(&scope_chip);
    content.append(&error);
    content.append(&toggles);
    content.append(&replace_row);
    content.append(&status);
    content.append(&scroll);
    let dialog = adw::Dialog::builder()
        .title("Find in Folder")
        .content_width(640)
        .content_height(520)
        .child(&content)
        .build();

    // Row index → (note, line); line 0: a file to show in Files (Unused Attachments).
    let targets: Rc<RefCell<Vec<(PathBuf, usize)>>> = Rc::default();
    // What Replace All replaces: the results shown; None while a search runs.
    let listed: Rc<RefCell<Option<Listed>>> = Rc::default();
    // Bumped by every query and on close; a search whose number is stale stops / is dropped.
    let generation = Arc::new(AtomicU64::new(0));

    let show = {
        let (list, status, targets, root) =
            (list.clone(), status.clone(), targets.clone(), root.clone());
        let (listed, replace) = (listed.clone(), replace.clone());
        move |found: find::Found, query: find::Query| {
            let matches: usize = found
                .files
                .iter()
                .flat_map(|f| &f.lines)
                .map(|h| h.count)
                .sum();
            let files = found.files.iter().map(|f| {
                let lines = f.lines.iter().map(|h| h.line).collect();
                (f.path.clone(), f.version, lines)
            });
            replace.set_sensitive(matches > 0);
            let empty = query.is_empty();
            listed.replace((matches > 0).then(|| Listed {
                query,
                matches,
                files: files.collect(),
            }));
            let files = found.files.iter().map(|f| {
                let lines = f
                    .lines
                    .iter()
                    .map(|h| (h.line, highlight(&h.snippet, &h.ranges)));
                (f.path.clone(), lines.collect())
            });
            let rows = result_rows(&list, &root, files.collect());
            let count = rows.len();
            targets.replace(rows);
            if found.notes_only {
                status.set_label(&match (count, found.more) {
                    (_, true) => format!("First {count} matching notes (more not shown)"),
                    (0, _) => "No matching notes".to_string(),
                    (1, _) => "1 matching note".to_string(),
                    _ => format!("{count} matching notes"),
                });
                return;
            }
            status.set_label(&match (count, found.more) {
                (_, true) => format!("First {count} matching lines (more not shown)"),
                (0, _) if empty => String::new(),
                (0, _) => "No matches".to_string(),
                (1, _) => "1 matching line".to_string(),
                _ => format!("{count} matching lines"),
            });
        }
    };
    let show = Rc::new(show);

    let run = {
        let (generation, open_note, notes) = (generation.clone(), open_note.clone(), notes.clone());
        let (listed, replace) = (listed.clone(), replace.clone());
        let (match_case, whole_word, regex) =
            (match_case.clone(), whole_word.clone(), regex.clone());
        let (error, list, status) = (error.clone(), list.clone(), status.clone());
        let (targets, root) = (targets.clone(), root.clone());
        move |query: String| {
            listed.replace(None);
            replace.set_sensitive(false);
            let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let options = find::Options {
                match_case: match_case.is_active(),
                whole_word: whole_word.is_active(),
                regex: regex.is_active(),
            };
            let query::Parsed { text, operators } = query::parse(&query);
            let query = match find::Query::new(&text, options) {
                Ok(query) => query,
                Err(reason) => {
                    error.set_label(&reason);
                    error.set_visible(true);
                    while let Some(row) = list.first_child() {
                        list.remove(&row);
                    }
                    targets.replace(Vec::new());
                    status.set_label("");
                    return;
                }
            };
            error.set_visible(false);
            let (notes, open_note) = (notes.borrow().clone(), open_note.borrow().clone());
            let (generation, show, root) = (generation.clone(), show.clone(), root.clone());
            glib::spawn_future_local(async move {
                let g = generation.clone();
                let q = query.clone();
                let found = gio::spawn_blocking(move || {
                    find::search(
                        &notes,
                        open_note.as_ref().as_ref(),
                        &q,
                        &operators,
                        &root,
                        &|| g.load(Ordering::SeqCst) != mine,
                    )
                })
                .await;
                if let Ok(Some(found)) = found
                    && generation.load(Ordering::SeqCst) == mine
                {
                    show(found, query);
                }
            });
        }
    };
    let run = Rc::new(run);
    // Broken Links: scans the link index on a background thread (cancelled like a search).
    let scan_broken = {
        let (editor, generation, current_scope) =
            (editor.clone(), generation.clone(), current_scope.clone());
        let (listed, replace, error) = (listed.clone(), replace.clone(), error.clone());
        let (list, status, targets, root) =
            (list.clone(), status.clone(), targets.clone(), root.clone());
        let snapshot = snapshot.clone();
        move || {
            listed.replace(None);
            replace.set_sensitive(false);
            error.set_visible(false);
            targets.replace(result_rows(&list, &root, Vec::new()));
            status.set_label("Looking for broken links…");
            let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let (open, scope) = (snapshot(), current_scope.borrow().clone());
            let g = generation.clone();
            let images = render::WikiIndex::new(editor.image_list.borrow().to_vec(), &root);
            let Some(job) = editor.links_job(move |index| {
                broken::find(
                    index,
                    &images,
                    open.as_ref().as_ref(),
                    scope.as_deref(),
                    &|| g.load(Ordering::SeqCst) != mine,
                )
            }) else {
                return;
            };
            let (generation, list, status, targets, root) = (
                generation.clone(),
                list.clone(),
                status.clone(),
                targets.clone(),
                root.clone(),
            );
            glib::spawn_future_local(async move {
                let Ok(Some(found)) = gio::spawn_blocking(job).await else {
                    return;
                };
                if generation.load(Ordering::SeqCst) != mine {
                    return;
                }
                let files = found.iter().map(|n| {
                    let links = n.links.iter().map(|l| {
                        let text = glib::markup_escape_text(&l.text).to_string();
                        (l.line, text)
                    });
                    (n.path.clone(), links.collect())
                });
                targets.replace(result_rows(&list, &root, files.collect()));
                status.set_label(&broken::summary(&found));
                if let Some(row) = list.row_at_index(0) {
                    row.grab_focus();
                }
            });
        }
    };
    let scan_broken = Rc::new(scan_broken);
    // Open Tasks: reads the (scoped) notes on a background thread, cancelled like a search.
    let scan_tasks = {
        let (generation, notes, snapshot) = (generation.clone(), notes.clone(), snapshot.clone());
        let (listed, replace, error) = (listed.clone(), replace.clone(), error.clone());
        let (list, status, targets, root) =
            (list.clone(), status.clone(), targets.clone(), root.clone());
        move || {
            listed.replace(None);
            replace.set_sensitive(false);
            error.set_visible(false);
            targets.replace(result_rows(&list, &root, Vec::new()));
            status.set_label("Looking for open tasks…");
            let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let (notes, open) = (notes.borrow().clone(), snapshot());
            let (generation, list, status, targets, root) = (
                generation.clone(),
                list.clone(),
                status.clone(),
                targets.clone(),
                root.clone(),
            );
            glib::spawn_future_local(async move {
                let g = generation.clone();
                let found = gio::spawn_blocking(move || {
                    tasks::find(&notes, open.as_ref().as_ref(), find::LIMIT, &|| {
                        g.load(Ordering::SeqCst) != mine
                    })
                })
                .await;
                let Ok(Some(found)) = found else {
                    return;
                };
                if generation.load(Ordering::SeqCst) != mine {
                    return;
                }
                let files = found.notes.iter().map(|n| {
                    let rows = n.tasks.iter().map(|t| {
                        let text = match t.text.as_str() {
                            "" => "(empty task)".to_string(),
                            text => glib::markup_escape_text(text).to_string(),
                        };
                        (t.line, text)
                    });
                    (n.path.clone(), rows.collect())
                });
                targets.replace(result_rows(&list, &root, files.collect()));
                status.set_label(&tasks::summary(&found, find::LIMIT));
                if let Some(row) = list.row_at_index(0) {
                    row.grab_focus();
                }
            });
        }
    };
    let scan_tasks = Rc::new(scan_tasks);
    // Unused Attachments: lists the files under the scope (or the open folder) and checks them
    // against all of the folder's notes on a background thread, cancelled like a search.
    let scan_unused = {
        let (generation, current_scope, snapshot) =
            (generation.clone(), current_scope.clone(), snapshot.clone());
        let (listed, replace, error) = (listed.clone(), replace.clone(), error.clone());
        let editor = editor.clone();
        let (list, status, targets, root, all) = (
            list.clone(),
            status.clone(),
            targets.clone(),
            root.clone(),
            all.clone(),
        );
        move || {
            listed.replace(None);
            replace.set_sensitive(false);
            error.set_visible(false);
            targets.replace(result_rows(&list, &root, Vec::new()));
            status.set_label("Looking for unused attachments…");
            let mine = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let base = current_scope
                .borrow()
                .clone()
                .unwrap_or_else(|| root.clone());
            let (open, notes, folder) = (snapshot(), all.clone(), root.clone());
            let setting = editor.prefs.borrow().attachments_folder.clone();
            let (generation, list, status, targets, root) = (
                generation.clone(),
                list.clone(),
                status.clone(),
                targets.clone(),
                root.clone(),
            );
            glib::spawn_future_local(async move {
                let g = generation.clone();
                let found = gio::spawn_blocking(move || {
                    attachments::find(
                        &base,
                        &folder,
                        &setting,
                        &notes,
                        open.as_ref().as_ref(),
                        &|| g.load(Ordering::SeqCst) != mine,
                    )
                })
                .await;
                let Ok(Some(found)) = found else {
                    return;
                };
                if generation.load(Ordering::SeqCst) != mine {
                    return;
                }
                targets.replace(attachment_rows(&list, &root, &found.files, find::LIMIT));
                let size = |bytes: u64| glib::format_size(bytes).to_string();
                status.set_label(&attachments::summary(&found, find::LIMIT, &size));
                if let Some(row) = list.row_at_index(0) {
                    row.grab_focus();
                }
            });
        }
    };
    let scan_unused = Rc::new(scan_unused);
    // The mode the toggles select. Weak: the toggles' own handlers hold this.
    let mode_now = {
        let (broken, tasks, unused) = (
            broken_toggle.downgrade(),
            tasks_toggle.downgrade(),
            unused_toggle.downgrade(),
        );
        Rc::new(move || {
            let on =
                |t: &glib::WeakRef<gtk::CheckButton>| t.upgrade().is_some_and(|t| t.is_active());
            if on(&broken) {
                FindMode::BrokenLinks
            } else if on(&tasks) {
                FindMode::OpenTasks
            } else if on(&unused) {
                FindMode::UnusedAttachments
            } else {
                FindMode::Text
            }
        })
    };
    // Searches or scans again in the current mode (after a scope change, or the menu again).
    let refresh = {
        let (mode_now, entry, run) = (mode_now.clone(), entry.clone(), run.clone());
        let (scan_broken, scan_tasks, scan_unused) =
            (scan_broken.clone(), scan_tasks.clone(), scan_unused.clone());
        Rc::new(move || match mode_now() {
            FindMode::Text => run(entry.text().to_string()),
            FindMode::BrokenLinks => scan_broken(),
            FindMode::OpenTasks => scan_tasks(),
            FindMode::UnusedAttachments => scan_unused(),
        })
    };
    // Sets the scope (None = the whole folder) and searches again.
    let set_scope = {
        let (notes, current_scope, refresh) =
            (notes.clone(), current_scope.clone(), refresh.clone());
        move |scope: Option<PathBuf>| {
            let (scope, scoped_notes) = scoped(scope);
            notes.replace(scoped_notes);
            show_scope(scope.as_deref());
            current_scope.replace(scope);
            refresh();
        }
    };
    let set_scope: Rc<dyn Fn(Option<PathBuf>)> = Rc::new(set_scope);
    scope_clear.connect_clicked(clone!(
        #[strong]
        set_scope,
        #[weak]
        entry,
        move |_| {
            set_scope(None);
            entry.grab_focus();
        }
    ));
    editor.find_set_scope.replace(Some(set_scope));
    entry.connect_search_changed(clone!(
        #[strong]
        run,
        move |e| run(e.text().to_string())
    ));
    let on_toggle = {
        let (editor, entry, run) = (editor.clone(), entry.clone(), run.clone());
        let (match_case, whole_word, regex) =
            (match_case.clone(), whole_word.clone(), regex.clone());
        move || {
            editor.find_match_case.set(match_case.is_active());
            editor.find_whole_word.set(whole_word.is_active());
            editor.find_regex.set(regex.is_active());
            editor.remember();
            run(entry.text().to_string());
        }
    };
    let on_toggle = Rc::new(on_toggle);
    match_case.connect_toggled(clone!(
        #[strong]
        on_toggle,
        move |_| on_toggle()
    ));
    whole_word.connect_toggled(clone!(
        #[strong]
        on_toggle,
        move |_| on_toggle()
    ));
    regex.connect_toggled(clone!(
        #[strong]
        on_toggle,
        move |_| on_toggle()
    ));
    // A scan mode on: the chip replaces the search box, the text options and Replace are off,
    // and the folder is scanned; all off: the text search runs again.
    let apply_mode = {
        let (mode_now, refresh) = (mode_now.clone(), refresh.clone());
        let (entry, mode_chip, mode_text, replace_row) = (
            entry.clone(),
            mode_chip.clone(),
            mode_text.clone(),
            replace_row.clone(),
        );
        let (match_case, whole_word, regex) =
            (match_case.clone(), whole_word.clone(), regex.clone());
        Rc::new(move || {
            let mode = mode_now();
            let text = mode == FindMode::Text;
            entry.set_visible(text);
            mode_chip.set_visible(!text);
            mode_text.set_label(match mode {
                FindMode::Text => "",
                FindMode::BrokenLinks => "Broken links",
                FindMode::OpenTasks => "Open tasks",
                FindMode::UnusedAttachments => "Unused attachments",
            });
            for option in [&match_case, &whole_word, &regex] {
                option.set_sensitive(text);
            }
            replace_row.set_sensitive(text);
            refresh();
            if text {
                entry.grab_focus();
            }
        })
    };
    // The scan modes exclude each other: turning one on turns the one that was on off first,
    // whose handler then shows the new mode.
    let scan_toggles = [
        broken_toggle.clone(),
        tasks_toggle.clone(),
        unused_toggle.clone(),
    ];
    for toggle in &scan_toggles {
        let others: Vec<glib::WeakRef<gtk::CheckButton>> = scan_toggles
            .iter()
            .filter(|t| *t != toggle)
            .map(|t| t.downgrade())
            .collect();
        let apply_mode = apply_mode.clone();
        toggle.connect_toggled(move |toggle| {
            let on = others
                .iter()
                .filter_map(glib::WeakRef::upgrade)
                .find(|t| t.is_active());
            match on {
                Some(other) if toggle.is_active() => other.set_active(false),
                _ => apply_mode(),
            }
        });
    }
    mode_clear.connect_clicked(clone!(
        #[weak]
        broken_toggle,
        #[weak]
        tasks_toggle,
        #[weak]
        unused_toggle,
        move |_| {
            broken_toggle.set_active(false);
            tasks_toggle.set_active(false);
            unused_toggle.set_active(false);
        }
    ));
    // The menu entries while the dialog is open: switch to that mode, or scan again.
    editor.find_mode.replace(Some(Rc::new(clone!(
        #[weak]
        broken_toggle,
        #[weak]
        tasks_toggle,
        #[weak]
        unused_toggle,
        #[strong]
        refresh,
        move |mode| {
            let toggle = match mode {
                FindMode::Text => return,
                FindMode::BrokenLinks => &broken_toggle,
                FindMode::OpenTasks => &tasks_toggle,
                FindMode::UnusedAttachments => &unused_toggle,
            };
            if toggle.is_active() {
                refresh();
            } else {
                toggle.set_active(true);
            }
        }
    ))));
    replace.connect_clicked(clone!(
        #[strong]
        editor,
        #[weak]
        dialog,
        #[weak]
        entry,
        #[weak]
        replacement,
        move |button| {
            let Some(found) = listed.borrow().clone() else {
                return;
            };
            // Afterwards the search runs again, over the new texts.
            let rerun: Rc<dyn Fn()> = Rc::new(clone!(
                #[strong]
                run,
                #[strong]
                open_note,
                #[strong]
                snapshot,
                #[weak]
                entry,
                move || {
                    open_note.replace(snapshot());
                    run(entry.text().to_string());
                }
            ));
            confirm_replace(
                &editor,
                &dialog,
                button,
                found,
                replacement.text().into(),
                rerun,
            );
        }
    ));
    dialog.connect_closed(clone!(
        #[strong]
        generation,
        #[weak]
        editor,
        move |_| {
            generation.fetch_add(1, Ordering::SeqCst);
            editor.find_set_scope.replace(None);
            editor.find_mode.replace(None);
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
            // An attachment: shown in Files, the list stays for the next one.
            if line == 0 {
                sidebar::show_in_files(&editor, &path);
                return;
            }
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
    match mode {
        FindMode::Text => {
            entry.grab_focus();
        }
        FindMode::BrokenLinks => broken_toggle.set_active(true),
        FindMode::OpenTasks => tasks_toggle.set_active(true),
        FindMode::UnusedAttachments => unused_toggle.set_active(true),
    }
}

/// Fills Find in Folder's result list with unused attachments (the first `limit`): each row is
/// the file's path relative to `root` and its size. Their (file, 0) in order, 0 = show in Files.
fn attachment_rows(
    list: &gtk::ListBox,
    root: &Path,
    files: &[attachments::Attachment],
    limit: usize,
) -> Vec<(PathBuf, usize)> {
    while let Some(row) = list.first_child() {
        list.remove(&row);
    }
    let mut rows = Vec::new();
    for file in files.iter().take(limit) {
        let rel = file.path.strip_prefix(root).unwrap_or(&file.path);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(
            &gtk::Label::builder()
                .label(rel.to_string_lossy())
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build(),
        );
        let size = gtk::Label::new(Some(&glib::format_size(file.size)));
        size.add_css_class("dim-label");
        size.add_css_class("numeric");
        row.append(&size);
        row.set_tooltip_text(Some("Show in Files"));
        list.append(&row);
        rows.push((file.path.clone(), 0));
    }
    rows
}

/// Fills Find in Folder's result list: per note, its rows as (1-based line, Pango markup); the
/// note's name and path head its first row. The (note, line) of each row, in order.
fn result_rows(
    list: &gtk::ListBox,
    root: &Path,
    files: Vec<(PathBuf, Vec<(usize, String)>)>,
) -> Vec<(PathBuf, usize)> {
    while let Some(row) = list.first_child() {
        list.remove(&row);
    }
    let mut rows = Vec::new();
    for (path, lines) in files {
        let rel = path.strip_prefix(root).unwrap_or(&path);
        for (i, (number, markup)) in lines.into_iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
            // The file's name and path head its first row (a separate header row would take
            // keyboard focus without opening anything).
            if i == 0 {
                let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let name = gtk::Label::new(Some(&file_name(&path)));
                name.add_css_class("heading");
                head.append(&name);
                let rel = gtk::Label::builder()
                    .label(rel.to_string_lossy())
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build();
                rel.add_css_class("dim-label");
                rel.add_css_class("caption");
                head.append(&rel);
                if !rows.is_empty() {
                    head.set_margin_top(8);
                }
                row.append(&head);
            }
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let label = gtk::Label::builder()
                .label(number.to_string())
                .width_chars(5)
                .xalign(1.0)
                .build();
            label.add_css_class("dim-label");
            label.add_css_class("numeric");
            line.append(&label);
            line.append(
                &gtk::Label::builder()
                    .use_markup(true)
                    .label(markup)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build(),
            );
            row.append(&line);
            list.append(&row);
            rows.push((path.clone(), number));
        }
    }
    rows
}

/// The results Find in Folder shows, as Replace All needs them.
#[derive(Clone)]
struct Listed {
    query: find::Query,
    /// All matches of the listed lines.
    matches: usize,
    /// (note, `find::version` of the text searched, listed line numbers).
    files: Vec<(PathBuf, u64, Vec<usize>)>,
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "Replace N matches in M notes?"; Replace All then runs `replace_all`.
fn confirm_replace(
    editor: &Rc<Editor>,
    parent: &adw::Dialog,
    button: &gtk::Button,
    found: Listed,
    with: String,
    rerun: Rc<dyn Fn()>,
) {
    let heading = format!(
        "Replace {} in {}?",
        count(found.matches, "match", "matches"),
        count(found.files.len(), "note", "notes")
    );
    let what = if found.query.options.regex {
        "the pattern "
    } else {
        ""
    };
    let body = if with.is_empty() {
        format!(
            "Every listed match of {what}“{}” is deleted.",
            found.query.text
        )
    } else {
        format!(
            "Every listed match of {what}“{}” becomes “{with}”.",
            found.query.text
        )
    };
    let confirm = adw::AlertDialog::new(Some(&heading), Some(&body));
    confirm.add_responses(&[("cancel", "Cancel"), ("replace", "Replace All")]);
    confirm.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    confirm.set_default_response(Some("cancel"));
    confirm.set_close_response("cancel");
    let (editor, parent_ref, button) = (editor.clone(), parent.downgrade(), button.downgrade());
    confirm.connect_response(None, move |_, response| {
        if response == "replace" {
            // Off until the search ran again.
            if let Some(button) = button.upgrade() {
                button.set_sensitive(false);
            }
            replace_all(&editor, &parent_ref, found.clone(), &with, rerun.clone());
        }
    });
    confirm.present(Some(parent));
}

/// Replace All: the open note in its buffer (one undo step, then saved), the others written on
/// a background thread; each only if its text is still the one searched. Then a toast, one
/// message naming the notes skipped, and the search again.
fn replace_all(
    editor: &Rc<Editor>,
    parent: &glib::WeakRef<adw::Dialog>,
    found: Listed,
    with: &str,
    rerun: Rc<dyn Fn()>,
) {
    const CHANGED: &str = "changed after the search";
    let open = editor.path.borrow().clone();
    let (mut replaced, mut notes) = (0, 0);
    let mut skipped: Vec<(PathBuf, &str)> = Vec::new();
    let mut closed = Vec::new();
    for (path, version, lines) in found.files {
        if open.as_ref() != Some(&path) {
            closed.push((path, version, lines));
            continue;
        }
        let text = editor.text();
        if find::version(&text) != version {
            skipped.push((path, CHANGED));
            continue;
        }
        let edits = find::replace_edits(&text, &found.query, &lines, with);
        if !edits.is_empty() {
            editor.edit_buffer(&text, &edits);
            editor.save();
            (replaced, notes) = (replaced + edits.len(), notes + 1);
        }
    }
    let (weak, parent, with) = (Rc::downgrade(editor), parent.clone(), with.to_string());
    let query = found.query;
    glib::spawn_future_local(async move {
        let paths: Vec<PathBuf> = closed.iter().map(|(path, ..)| path.clone()).collect();
        let results = gio::spawn_blocking(move || {
            let replace = |(path, version, lines): &(PathBuf, u64, Vec<usize>)| {
                find::replace_in_file(path, *version, &query, lines, &with)
            };
            closed.iter().map(replace).collect::<Vec<_>>()
        })
        .await
        .unwrap_or_else(|_| {
            paths
                .iter()
                .map(|_| Err(find::Skipped::NotWritten))
                .collect()
        });
        let Some(editor) = weak.upgrade() else { return };
        for (path, result) in paths.into_iter().zip(results) {
            match result {
                Ok(0) => {}
                Ok(n) => {
                    (replaced, notes) = (replaced + n, notes + 1);
                    editor.note_changed(path);
                }
                Err(find::Skipped::Changed) => skipped.push((path, CHANGED)),
                Err(find::Skipped::NotWritten) => skipped.push((path, "could not be saved")),
            }
        }
        if notes > 0 {
            editor.schedule_tags_index();
        }
        editor.toast(&format!(
            "Replaced {} in {}",
            count(replaced, "match", "matches"),
            count(notes, "note", "notes")
        ));
        let parent = parent.upgrade();
        if !skipped.is_empty() {
            let mut lines: Vec<String> = skipped
                .iter()
                .take(10)
                .map(|(path, why)| format!("{}: {why}", file_name(path)))
                .collect();
            if skipped.len() > 10 {
                lines.push(format!("and {} more", skipped.len() - 10));
            }
            let alert = adw::AlertDialog::new(
                Some(&format!(
                    "{} left as they were",
                    count(skipped.len(), "note", "notes")
                )),
                Some(&lines.join("\n")),
            );
            alert.add_response("ok", "OK");
            match &parent {
                Some(parent) => alert.present(Some(parent)),
                None => alert.present(Some(&editor.window)),
            }
        }
        if parent.is_some() {
            rerun();
        }
    });
}

/// Daily Note for Date… (main menu): a calendar on today with the days that have a daily
/// note marked, a line saying what Open does, Open (Enter, or a double-click on a day)
/// opens or creates that day's note like Today's Note; Esc cancels.
pub(crate) fn daily_note_for_date(editor: &Rc<Editor>) {
    let Some(root) = editor.root.borrow().clone() else {
        return;
    };
    let folder = crate::daily::folder(&root, &editor.prefs.borrow().daily_folder);
    let pattern = editor.daily_pattern();
    let dates: Vec<String> = crate::daily::notes(&folder, &pattern)
        .into_iter()
        .map(|(date, _)| date)
        .collect();
    let has_template = crate::daily::load_template(&folder).is_some();
    let calendar = gtk::Calendar::new();
    a11y::label(&calendar, "Date");
    let label = gtk::Label::builder()
        .wrap(true)
        .css_classes(["dim-label"])
        .build();
    let date_of = |c: &gtk::Calendar| {
        let d = c.date();
        (d.year(), d.month() as u32, d.day_of_month() as u32)
    };
    let refresh = {
        let folder = folder.clone();
        move |c: &gtk::Calendar, label: &gtk::Label| {
            let (y, m, d) = date_of(c);
            c.clear_marks();
            for day in crate::daily::marked_days(dates.iter().map(String::as_str), y, m) {
                c.mark_day(day);
            }
            let name = pattern.format(&crate::templates::Components::from_ymd(y, m, d));
            let exists = folder.join(format!("{name}.md")).is_file();
            label.set_label(&crate::daily::date_label(&name, exists, has_template));
        }
    };
    refresh(&calendar, &label);
    let refresh = Rc::new(refresh);
    calendar.connect_day_selected(clone!(
        #[strong]
        refresh,
        #[weak]
        label,
        move |c| refresh(c, &label)
    ));
    let open = gtk::Button::builder()
        .label("Open")
        .css_classes(["suggested-action"])
        .build();
    let cancel = gtk::Button::with_label("Cancel");
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&open);
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    body.append(&calendar);
    body.append(&label);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&body));
    let dialog = adw::Dialog::builder()
        .title("Daily Note for Date")
        .child(&view)
        .default_widget(&open)
        .build();
    let weak = Rc::downgrade(editor);
    let go = Rc::new(clone!(
        #[weak]
        dialog,
        #[weak]
        calendar,
        move || {
            let (y, m, d) = date_of(&calendar);
            dialog.close();
            if let Some(editor) = weak.upgrade()
                && let Some(now) = editor.now()
            {
                editor.open_daily(crate::daily::components_for_date(&now, y, m, d));
            }
        }
    ));
    open.connect_clicked(clone!(
        #[strong]
        go,
        move |_| go()
    ));
    cancel.connect_clicked(clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));
    // Enter on the focused calendar would only select a day: Open instead.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(move |_, key, _, _| {
        if matches!(key, gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter) {
            go();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    dialog.add_controller(keys);
    dialog.present(Some(&editor.window));
    calendar.grab_focus();
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
