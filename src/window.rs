use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};
use sourceview5::prelude::*;
use webkit6::prelude::*;

use crate::dialogs::{
    bookmarks_dialog, find_in_folder, go_to_line, open_quickly, preferences_dialog, reload_note,
};
use crate::editor::{Editor, FolderScan};
use crate::input::{
    Completer, LineCommand, image_input, in_table, insert_table, line_command, smart_lists,
    table_command,
};
use crate::preview::{
    copy_as_html, copy_as_rich_text, export_html, export_pdf, export_rtf, export_text, print_note,
};
use crate::{
    APP_ID, MARK_MARKER_STYLE, MARK_STYLE, MATH_STYLE, SCRIPT_WORLD, SYNC_DELAY, TABLE_STYLE,
    TAG_STYLE, UNTITLED, state_file,
};
use crate::{choose_file, choose_folder, folder_of};
use crate::{links, render, sidebar, state, title};

/// `target`: a folder or note given on the command line; `None` reopens the last session.
pub(crate) fn build_window(app: &adw::Application, target: Option<PathBuf>) {
    // Reopen last session; anything gone since is skipped silently.
    let mut session = state::load(&state_file()).usable();
    render::set_hard_breaks(session.prefs.hard_breaks);
    // The folder to show is scanned on its own thread while the window is built.
    let scan = match &target {
        Some(target) => folder_of(target),
        None => session.folder.as_deref(),
    }
    .map(|dir| FolderScan::start(dir, session.prefs.sidebar_sort));
    let header = adw::HeaderBar::new();
    // Open Recent is refilled by `Editor::fill_recent_menu`.
    let recent_menu = gio::Menu::new();
    // New from Template is refilled by `Editor::fill_templates_menu` when the menu opens.
    let templates_menu = gio::Menu::new();
    let menu = gio::Menu::new();
    for items in [
        &[
            ("New File", "win.new-note"),
            ("New Folder…", "win.new-folder"),
            ("Today's Note", "win.todays-note"),
        ][..],
        &[
            ("Open File…", "win.open-file"),
            ("Open Folder…", "win.open-folder"),
        ],
        &[("Back", "win.go-back"), ("Forward", "win.go-forward")],
        &[
            ("Export as PDF…", "win.export-pdf"),
            ("Export as HTML…", "win.export-html"),
            ("Export as Plain Text…", "win.export-text"),
            ("Export as RTF…", "win.export-rtf"),
            ("Copy as HTML", "win.copy-html"),
            ("Copy as Rich Text", "win.copy-rich-text"),
            ("Print…", "win.print"),
        ],
        &[
            ("Keyboard Shortcuts", "win.show-help-overlay"),
            ("About MarkdownMate", "win.about"),
        ],
    ] {
        let section = gio::Menu::new();
        for (label, action) in items {
            section.append(Some(label), Some(action));
            if *action == "win.new-folder" {
                // Its action greys the submenu out until a folder is open.
                let item = gio::MenuItem::new_submenu(Some("New from Template"), &templates_menu);
                item.set_attribute_value("action", Some(&"win.templates".to_variant()));
                section.append_item(&item);
            }
        }
        if items[0].1 == "win.open-file" {
            section.append_submenu(Some("Open Recent"), &recent_menu);
        }
        if items[0].1 == "win.export-pdf" {
            // Edit and Format before export. Their keys work in the editor only (`input.rs`),
            // not as window accelerators, so the menu shows them through the `accel` attribute.
            let commands = gio::Menu::new();
            for (title, items) in [
                (
                    "Edit",
                    &[
                        ("Move Line Up", "win.move-line-up", "<Control><Alt>Up"),
                        ("Move Line Down", "win.move-line-down", "<Control><Alt>Down"),
                        ("Duplicate Line", "win.duplicate-line", "<Shift><Control>d"),
                    ][..],
                ),
                (
                    "Format",
                    &[
                        ("Highlight", "win.format-highlight", "<Shift><Control>h"),
                        ("Toggle Task", "win.toggle-task", "<Control>Return"),
                        ("Footnote", "win.footnote", ""),
                    ],
                ),
            ] {
                let submenu = gio::Menu::new();
                for (label, action, accel) in items {
                    let item = gio::MenuItem::new(Some(label), Some(action));
                    if !accel.is_empty() {
                        item.set_attribute_value("accel", Some(&accel.to_variant()));
                    }
                    submenu.append_item(&item);
                }
                if title == "Format" {
                    // A check item (stateful action); Shift+Ctrl+M is a window accelerator.
                    let bar = gio::Menu::new();
                    bar.append(Some("Format Bar"), Some("win.format-bar"));
                    submenu.append_section(None, &bar);
                }
                commands.append_submenu(Some(title), &submenu);
            }
            // Table: Insert Table, then the commands of the table bar (`tablebar.rs`), greyed
            // out while the cursor is not in a table.
            let table = gio::Menu::new();
            table.append(Some("Insert Table"), Some("win.insert-table"));
            let edits = gio::Menu::new();
            for e in crate::tablebar::COMMANDS {
                let item = gio::MenuItem::new(Some(e.label), Some(&format!("win.{}", e.action)));
                if !e.accel.is_empty() {
                    item.set_attribute_value("accel", Some(&e.accel.to_variant()));
                }
                edits.append_item(&item);
            }
            table.append_section(None, &edits);
            commands.append_submenu(Some("Table"), &table);
            // Radio items of one stateful action (also in the sidebar's right-click menu).
            let sort = gio::Menu::new();
            for order in crate::notes::SortOrder::ALL {
                let item = gio::MenuItem::new(Some(order.title()), None);
                item.set_action_and_target_value(
                    Some("win.sidebar-sort"),
                    Some(&order.name().to_variant()),
                );
                sort.append_item(&item);
            }
            commands.append_submenu(Some("Sort Sidebar By"), &sort);
            // A check item (stateful action); Shift+Ctrl+J is a window accelerator.
            commands.append(Some("Focus Mode"), Some("win.focus-mode"));
            menu.append_section(None, &commands);
        }
        menu.append_section(None, &section);
    }
    // Smoke tests activate the actions, not this menu (an open GtkPopover logs Gdk-CRITICAL
    // under Xvfb). F10 opens it (`primary`).
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .tooltip_text("Main Menu")
        .menu_model(&menu)
        .primary(true)
        .build();
    header.pack_start(&menu_button);
    header.pack_start(
        &gtk::Button::builder()
            .icon_name("user-bookmarks-symbolic")
            .tooltip_text("Bookmarks (Alt+1…Alt+9 open one)")
            .action_name("win.bookmarks")
            .build(),
    );

    let split = adw::OverlaySplitView::builder()
        .vexpand(true)
        .show_sidebar(false)
        .content(
            &adw::StatusPage::builder()
                .icon_name("folder-open-symbolic")
                .title("No folder open")
                .description("Open a folder of Markdown notes to get started.")
                .build(),
        )
        .build();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&split));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&toasts);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(title::window_title(None))
        .default_width(900)
        .default_height(600)
        .content(&content)
        .build();

    let buffer = sourceview5::Buffer::new(None);
    buffer.set_language(
        sourceview5::LanguageManager::default()
            .language("markdown")
            .as_ref(),
    );
    // Readable on light and dark schemes; only the colour changes, never the text.
    buffer.create_tag(Some(TAG_STYLE), &[("foreground", &"#3584e4")]);
    buffer.create_tag(Some(TABLE_STYLE), &[("family", &"monospace")]);
    // Mid-tone purple, readable on light and dark schemes like the tag blue.
    buffer.create_tag(Some(MATH_STYLE), &[("foreground", &"#9141ac")]);
    // Colours set per theme by `Editor::apply_scheme`.
    buffer.create_tag(Some(MARK_STYLE), &[]);
    buffer.create_tag(Some(MARK_MARKER_STYLE), &[]);
    let style = adw::StyleManager::default();
    // A GtkSourceView that draws Focus Mode's veil.
    let focus_view = crate::focus::View::new(&buffer);
    let view = focus_view.clone().upcast::<sourceview5::View>();
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_monospace(true);
    view.add_css_class("mm-editor");
    view.set_left_margin(crate::editor::SIDE_MARGIN);
    view.set_right_margin(crate::editor::SIDE_MARGIN);
    view.set_top_margin(12);
    view.set_bottom_margin(crate::editor::BOTTOM_MARGIN);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&view)
        .vexpand(true)
        .build();
    // Typing and opening a note change the buffer inside a frame (input is handled in the
    // frame clock's event phase). GtkTextView then validates the new lines only while drawing
    // and grows its vertical adjustment mid-snapshot, after the scrollbar was allocated: GTK
    // warns "Trying to snapshot GtkGizmo … without a current allocation" (the scrollbar's
    // trough). Re-allocating the view lets it validate and resize the scrollbar in the layout
    // phase of the same frame instead.
    buffer.connect_changed(clone!(
        #[weak]
        view,
        move |_| view.queue_allocate()
    ));
    let status = gtk::Label::builder()
        .xalign(0.0)
        .margin_start(12)
        .margin_end(12)
        .margin_top(4)
        .margin_bottom(4)
        .css_classes(["caption", "dim-label"])
        .build();
    let editor_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    // The format bar (Shift+Ctrl+M) above the editor; in the editor pane, so Preview hides it.
    let (format_bar, format_table) = crate::formatbar::bar();
    let format_rule = gtk::Separator::new(gtk::Orientation::Horizontal);
    format_rule.set_visible(false);
    editor_pane.append(&format_bar);
    editor_pane.append(&format_rule);
    // The `[[` suggestions float over the editor in this overlay.
    let overlay = gtk::Overlay::builder().child(&scroll).vexpand(true).build();
    editor_pane.append(&overlay);
    // The table bar shows under the editor while the cursor is in a table, unless the format
    // bar is on: its table section shows the same buttons then.
    let table_bar = crate::tablebar::bar();
    let table_rule = gtk::Separator::new(gtk::Orientation::Horizontal);
    table_rule.set_visible(false);
    editor_pane.append(&table_rule);
    editor_pane.append(&table_bar);
    editor_pane.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    editor_pane.append(&status);
    let completer = Completer::new(&view, &overlay);
    smart_lists(&view, &completer);

    // The preview under its find bar (`previewfind`); the mode shows or hides the whole pane.
    // Both are added on first use (`Editor::preview`).
    let preview_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
    preview_pane.set_visible(false);
    let panes = gtk::Paned::builder()
        .start_child(&editor_pane)
        .end_child(&preview_pane)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .hexpand(true)
        .build();

    // Outline (Alt+Ctrl+O): a side panel, not a popover (GtkPopover logs Gdk-CRITICAL under
    // Xvfb, which fails the smoke test).
    let outline = gtk::ListBox::new();
    outline.add_css_class("navigation-sidebar");
    // Backlinks: notes linking to the open note, listed under the outline.
    let backlinks = gtk::ListBox::new();
    backlinks.add_css_class("navigation-sidebar");
    let backlinks_title = gtk::Label::builder()
        .label("Backlinks")
        .xalign(0.0)
        .margin_start(12)
        .margin_top(12)
        .build();
    backlinks_title.add_css_class("heading");
    let outline_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    outline_column.append(&outline);
    outline_column.append(&backlinks_title);
    outline_column.append(&backlinks);
    // Unlinked Mentions: notes naming the open note without a link; collapsed by default and
    // searched only while expanded.
    let mentions = gtk::ListBox::new();
    mentions.add_css_class("navigation-sidebar");
    let mentions_title = gtk::Label::new(Some("Unlinked Mentions"));
    mentions_title.add_css_class("heading");
    let mentions_expander = gtk::Expander::builder()
        .label_widget(&mentions_title)
        .child(&mentions)
        .margin_start(6)
        .margin_top(12)
        .build();
    outline_column.append(&mentions_expander);
    let outline_panel = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    outline_panel.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    outline_panel.append(
        &gtk::ScrolledWindow::builder()
            .child(&outline_column)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .width_request(220)
            .build(),
    );
    outline_panel.set_visible(false);
    let (tags_panel, tags_list) = sidebar::tags_panel();
    let (favorites_panel, favorites_list) = sidebar::favorites_panel();
    let note_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    note_box.append(&panes);
    note_box.append(&outline_panel);

    let editor = Rc::new(Editor {
        window: window.clone(),
        toasts,
        split,
        panes,
        note_box,
        outline_panel,
        outline,
        outline_lines: RefCell::default(),
        outline_timer: RefCell::new(None),
        backlinks,
        backlink_targets: RefCell::default(),
        backlinks_gen: Cell::new(0),
        link_index: Arc::default(),
        links_changed: RefCell::default(),
        mentions_expander,
        mentions,
        mention_targets: RefCell::default(),
        mentions_gen: Arc::default(),
        tag_timer: RefCell::new(None),
        preview_view: std::cell::OnceCell::new(),
        preview_find: std::cell::OnceCell::new(),
        print_view: RefCell::new(None),
        print_busy: Cell::new(false),
        mode: Cell::new(state::Mode::Edit),
        preview_timer: RefCell::new(None),
        preview_generation: Cell::new(0),
        preview_render: Cell::default(),
        preview_state: RefCell::new(None),
        preview_loads: Cell::new(0),
        sync_timer: RefCell::new(None),
        preview_poll: RefCell::new(None),
        preview_y: Cell::new(0.0),
        preview_anchor: RefCell::new(None),
        buffer: buffer.clone(),
        view: view.clone(),
        status,
        counts: Cell::new((0, 0)),
        status_timer: RefCell::new(None),
        path: RefCell::new(None),
        saved: RefCell::new(String::new()),
        timer: RefCell::new(None),
        close_anyway: Cell::new(false),
        note_monitor: RefCell::new(None),
        missing_told: Cell::new(false),
        root: RefCell::new(None),
        dir_monitors: RefCell::new(Vec::new()),
        rescan_timer: RefCell::new(None),
        expanded: RefCell::new(HashSet::new()),
        sidebar_store: RefCell::new(None),
        sidebar_view: RefCell::new(None),
        favorites_panel,
        favorites_list: favorites_list.clone(),
        favorites: RefCell::default(),
        scan_gen: Cell::new(0),
        applied_gen: Cell::new(0),
        scan_dir: RefCell::new(None),
        note_list: RefCell::default(),
        tags_panel,
        tags_list: tags_list.clone(),
        tag_index: RefCell::default(),
        tags_gen: Cell::new(0),
        tags_timer: RefCell::new(None),
        bookmarks: RefCell::default(),
        recent: RefCell::default(),
        positions: RefCell::default(),
        history: RefCell::default(),
        history_target: Cell::new(None),
        sidebar_reveal: RefCell::new(None),
        find_match_case: Cell::new(false),
        find_whole_word: Cell::new(false),
        find_regex: Cell::new(false),
        prefs: RefCell::default(),
        recent_menu,
        templates_menu,
        font_css: gtk::CssProvider::new(),
        focus: crate::focus::Focus::attach(&focus_view),
        spelling: crate::spell::attach(&view),
        spell_held: Cell::new(false),
    });
    // Window-wide so the rule (class `mm-editor`) beats the theme's `.monospace` font.
    gtk::style_context_add_provider_for_display(
        &gtk::prelude::WidgetExt::display(&window),
        &editor.font_css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    sidebar::tags_menu(&tags_list, &Rc::downgrade(&editor));
    sidebar::connect_favorites(&favorites_list, &Rc::downgrade(&editor));
    tags_list.connect_row_activated(clone!(
        #[weak]
        editor,
        move |_, row| {
            let tag = usize::try_from(row.index())
                .ok()
                .and_then(|i| editor.tag_index.borrow().get(i).cloned());
            if let Some((tag, notes)) = tag {
                sidebar::tag_notes(&editor, &tag, &notes);
            }
        }
    ));

    image_input(&view, &editor);
    // The right-click menu's own Check Spelling item: remembered like the Preferences switch.
    // On a held large note it turns spell check on for that note; holding it off is not saved.
    editor.spelling.connect_enabled_notify(clone!(
        #[weak]
        editor,
        move |a| {
            let on = a.is_enabled();
            if on && editor.spell_held.replace(false) {
                editor.show_status();
            } else if !on && editor.spell_held.get() {
                return;
            }
            if editor.prefs.borrow().spell_check != on {
                editor.prefs.borrow_mut().spell_check = on;
                editor.remember();
            }
        }
    ));
    crate::search::find_bar(&editor, &editor_pane, app);
    crate::editor::typewriter_scrolling(&editor, &scroll);
    crate::editor::text_width_resizing(&editor, &scroll);

    buffer.connect_changed(clone!(
        #[strong]
        editor,
        #[strong]
        completer,
        move |_| {
            editor.schedule_save();
            editor.schedule_preview();
            editor.schedule_status();
            editor.schedule_outline();
            editor.schedule_tags();
            completer.update(&editor);
        }
    ));
    buffer.connect_mark_set(clone!(
        #[strong]
        editor,
        #[strong]
        completer,
        move |buffer, _, mark| {
            if *mark == buffer.get_insert() || *mark == buffer.selection_bound() {
                editor.schedule_status();
                editor.select_outline_at_cursor();
                completer.update(&editor);
            }
        }
    ));
    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave(clone!(
        #[strong]
        completer,
        move |_| completer.panel.set_visible(false)
    ));
    view.add_controller(focus);

    let save_action = gio::SimpleAction::new("save", None);
    save_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| {
            editor.save();
        }
    ));
    window.add_action(&save_action);
    app.set_accels_for_action("win.save", &["<Control>s"]);
    // Thoát không qua đóng cửa sổ (main.rs: SIGTERM, logout, shutdown): lưu một lần, không
    // chặn — lỗi đã được `save` ghi stderr.
    let flush_action = gio::SimpleAction::new("flush", None);
    flush_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| {
            editor.save();
            editor.remember();
        }
    ));
    window.add_action(&flush_action);

    // Disabled until a folder is open (load_folder enables it).
    let new_note_action = gio::SimpleAction::new("new-note", None);
    new_note_action.set_enabled(false);
    new_note_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| {
            if let Some(dir) = editor.current_dir() {
                editor.new_note(&dir, UNTITLED);
            }
        }
    ));
    window.add_action(&new_note_action);
    app.set_accels_for_action("win.new-note", &["<Control>n"]);

    // Disabled until a folder is open (load_folder enables it). Ctrl+D is not bound by GTK or
    // GtkSourceView (Shift+Ctrl+D is Duplicate Line, `input.rs`).
    let today_action = gio::SimpleAction::new("todays-note", None);
    today_action.set_enabled(false);
    today_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| editor.todays_note()
    ));
    window.add_action(&today_action);
    app.set_accels_for_action("win.todays-note", &["<Control>d"]);

    // New from Template ▸ <template>: the target is the template's path. Disabled until a
    // folder is open (load_folder enables it, and `templates`, the submenu's own no-op action).
    let template_action =
        gio::SimpleAction::new("new-from-template", Some(glib::VariantTy::STRING));
    template_action.set_enabled(false);
    template_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, param| {
            if let Some(path) = param.and_then(|p| p.str()) {
                editor.new_from_template(std::path::Path::new(path));
            }
        }
    ));
    window.add_action(&template_action);
    let templates_action = gio::SimpleAction::new("templates", None);
    templates_action.set_enabled(false);
    window.add_action(&templates_action);
    // The "No Templates" item: always greyed out.
    let no_templates = gio::SimpleAction::new("no-templates", None);
    no_templates.set_enabled(false);
    window.add_action(&no_templates);
    if let Some(popover) = menu_button.popover() {
        popover.connect_show(clone!(
            #[weak]
            editor,
            move |_| editor.fill_templates_menu()
        ));
    }

    // Disabled until a folder is open (load_folder enables it).
    let quick_action = gio::SimpleAction::new("open-quickly", None);
    quick_action.set_enabled(false);
    quick_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| open_quickly(&editor)
    ));
    window.add_action(&quick_action);
    app.set_accels_for_action("win.open-quickly", &["<Control>p"]);

    // Disabled until a folder is open (load_folder enables it).
    let find_action = gio::SimpleAction::new("find-in-folder", None);
    find_action.set_enabled(false);
    find_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| find_in_folder(&editor)
    ));
    window.add_action(&find_action);
    app.set_accels_for_action("win.find-in-folder", &["<Shift><Control>f"]);

    // Ctrl+L is not bound by GTK or GtkSourceView; does nothing without an open note.
    let goto_action = gio::SimpleAction::new("go-to-line", None);
    goto_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| go_to_line(&editor)
    ));
    window.add_action(&goto_action);
    app.set_accels_for_action("win.go-to-line", &["<Control>l"]);

    // Insert Table. Alt+Ctrl+T opens a terminal on Ubuntu; Shift+Ctrl+T is not bound by GTK or
    // GtkSourceView. Does nothing without an open note; Preview switches to Edit like Go to Line.
    let table_action = gio::SimpleAction::new("insert-table", None);
    table_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| {
            if editor.path.borrow().is_none() {
                return;
            }
            if editor.mode.get() == state::Mode::Preview {
                editor.set_mode(state::Mode::Edit);
            }
            if insert_table(editor.buffer.upcast_ref()) {
                editor.view.grab_focus();
            } else {
                editor.toast("Cannot insert a table inside a code block");
            }
        }
    ));
    window.add_action(&table_action);
    app.set_accels_for_action("win.insert-table", &["<Shift><Control>t"]);

    // Line commands from the menu (`lines.rs`); their keys are handled in the editor
    // (`input.rs`). Do nothing without an open note; Preview switches to Edit like Insert Table.
    for (name, command) in [
        ("toggle-task", LineCommand::ToggleTask),
        ("footnote", LineCommand::Footnote),
        ("move-line-up", LineCommand::Move { up: true }),
        ("move-line-down", LineCommand::Move { up: false }),
        ("duplicate-line", LineCommand::Duplicate),
    ] {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(clone!(
            #[strong]
            editor,
            move |_, _| {
                if editor.path.borrow().is_none() {
                    return;
                }
                if editor.mode.get() == state::Mode::Preview {
                    editor.set_mode(state::Mode::Edit);
                }
                editor.view.grab_focus();
                if !line_command(editor.view.upcast_ref(), command) {
                    match command {
                        LineCommand::Footnote => {
                            editor.toast("Cannot insert a footnote inside a code block")
                        }
                        LineCommand::ToggleTask => editor.toast("No task on the selected lines"),
                        _ => editor.view.error_bell(),
                    }
                }
            }
        ));
        window.add_action(&action);
    }

    // Table commands (`table.rs`) for the Table menu and the table bar; their keys are handled
    // in the editor (`input.rs`). Enabled only while the cursor is in a table (`table_context`);
    // Preview switches to Edit like Insert Table.
    let mut table_actions = vec![];
    for e in crate::tablebar::COMMANDS {
        let command = e.command;
        let action = gio::SimpleAction::new(e.action, None);
        action.set_enabled(false);
        action.connect_activate(clone!(
            #[strong]
            editor,
            move |_, _| {
                if editor.path.borrow().is_none() {
                    return;
                }
                if editor.mode.get() == state::Mode::Preview {
                    editor.set_mode(state::Mode::Edit);
                }
                editor.view.grab_focus();
                if table_command(editor.buffer.upcast_ref(), command) {
                    editor
                        .view
                        .scroll_mark_onscreen(&editor.buffer.get_insert());
                } else {
                    editor.view.error_bell();
                }
            }
        ));
        window.add_action(&action);
        table_actions.push(action);
    }
    // Shows the table controls (the format bar's table section, or else the table bar) and
    // enables the table commands while the cursor is in a table.
    let table_context = Rc::new(clone!(
        #[strong]
        editor,
        #[strong]
        format_bar,
        move || {
            let on = in_table(editor.buffer.upcast_ref());
            let in_format_bar = format_bar.is_visible();
            if format_table.is_visible() != on {
                format_table.set_visible(on);
            }
            if table_bar.is_visible() != (on && !in_format_bar) {
                table_bar.set_visible(on && !in_format_bar);
                table_rule.set_visible(on && !in_format_bar);
            }
            for action in &table_actions {
                action.set_enabled(on);
            }
        }
    ));
    editor.buffer.connect_changed(clone!(
        #[strong]
        table_context,
        move |_| table_context()
    ));
    editor.buffer.connect_mark_set(clone!(
        #[strong]
        table_context,
        move |buffer, _, mark| {
            if *mark == buffer.get_insert() || *mark == buffer.selection_bound() {
                table_context();
            }
        }
    ));

    // Format bar on/off (main menu ▸ Format, Shift+Ctrl+M, not bound by GTK or GtkSourceView),
    // kept in the state file. Its buttons' actions: `formatbar.rs`.
    crate::formatbar::add_actions(&window, &editor);
    let format_action = gio::SimpleAction::new_stateful("format-bar", None, &false.to_variant());
    format_action.connect_change_state(clone!(
        #[strong]
        editor,
        move |action, value| {
            let Some(show) = value.and_then(|v| v.get::<bool>()) else {
                return;
            };
            action.set_state(&show.to_variant());
            format_bar.set_visible(show);
            format_rule.set_visible(show);
            table_context();
            if editor.prefs.borrow().format_bar != show {
                editor.prefs.borrow_mut().format_bar = show;
                editor.remember();
            }
        }
    ));
    window.add_action(&format_action);
    app.set_accels_for_action("win.format-bar", &["<Shift><Control>m"]);

    // Focus Mode on/off (main menu, Shift+Ctrl+J, not bound by GTK or GtkSourceView), kept in
    // the state file: `focus.rs`.
    let focus_action = gio::SimpleAction::new_stateful("focus-mode", None, &false.to_variant());
    focus_action.connect_change_state(clone!(
        #[strong]
        editor,
        move |action, value| {
            let Some(on) = value.and_then(|v| v.get::<bool>()) else {
                return;
            };
            action.set_state(&on.to_variant());
            editor.focus.set_on(on);
            if editor.prefs.borrow().focus_mode != on {
                editor.prefs.borrow_mut().focus_mode = on;
                editor.remember();
            }
        }
    ));
    window.add_action(&focus_action);
    app.set_accels_for_action("win.focus-mode", &["<Shift><Control>j"]);

    // Sidebar order (main menu ▸ Sort Sidebar By, the sidebar's Sort By), kept in the state
    // file: `notes::SortOrder`.
    let sort_action = gio::SimpleAction::new_stateful(
        "sidebar-sort",
        Some(glib::VariantTy::STRING),
        &crate::notes::SortOrder::default().name().to_variant(),
    );
    sort_action.connect_change_state(clone!(
        #[strong]
        editor,
        move |action, value| {
            let Some(order) = value
                .and_then(|v| v.str())
                .and_then(crate::notes::SortOrder::from_name)
            else {
                return;
            };
            action.set_state(&order.name().to_variant());
            editor.set_sidebar_sort(order);
        }
    ));
    window.add_action(&sort_action);

    // Ctrl+R is not bound by GTK or GtkSourceView; does nothing without an open note.
    let reload_action = gio::SimpleAction::new("reload", None);
    reload_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| reload_note(&editor)
    ));
    window.add_action(&reload_action);
    app.set_accels_for_action("win.reload", &["<Control>r"]);

    // Outline panel on/off. Clicking (or Enter on) a heading jumps to its line in the editor.
    let outline_action = gio::SimpleAction::new_stateful("outline", None, &false.to_variant());
    outline_action.connect_activate(clone!(
        #[strong]
        editor,
        move |action, _| {
            let show = !editor.outline_panel.is_visible();
            action.set_state(&show.to_variant());
            editor.outline_panel.set_visible(show);
            if show {
                editor.refresh_outline();
                editor.refresh_backlinks();
                editor.refresh_mentions();
                // Keyboard: Up/Down + Enter right away, from the cursor's section if any.
                if let Some(row) = editor
                    .outline
                    .selected_row()
                    .or_else(|| editor.outline.row_at_index(0))
                {
                    row.grab_focus();
                }
            } else if editor.mode.get() != state::Mode::Preview {
                editor.view.grab_focus();
            }
        }
    ));
    window.add_action(&outline_action);
    app.set_accels_for_action("win.outline", &["<Alt><Control>o"]);
    editor.outline.connect_row_activated(clone!(
        #[strong]
        editor,
        move |_, row| {
            let line = usize::try_from(row.index())
                .ok()
                .and_then(|i| editor.outline_lines.borrow().get(i).copied());
            if let Some(line) = line {
                editor.go_to(line, None);
            }
        }
    ));
    editor.outline_menu();
    editor.backlinks.connect_row_activated(clone!(
        #[strong]
        editor,
        move |_, row| {
            let target = usize::try_from(row.index())
                .ok()
                .and_then(|i| editor.backlink_targets.borrow().get(i).cloned());
            if let Some((path, line)) = target {
                editor.open_at_line(&path, line);
            }
        }
    ));
    editor.mentions_expander.connect_expanded_notify(clone!(
        #[strong]
        editor,
        move |_| editor.refresh_mentions()
    ));
    editor.mentions.connect_row_activated(clone!(
        #[strong]
        editor,
        move |_, row| {
            let target = usize::try_from(row.index())
                .ok()
                .and_then(|i| editor.mention_targets.borrow().get(i).cloned());
            if let Some((path, line)) = target {
                editor.open_at_line(&path, line);
            }
        }
    ));
    header.pack_end(
        &gtk::ToggleButton::builder()
            .icon_name("view-list-symbolic")
            .tooltip_text("Outline (Alt+Ctrl+O)")
            .action_name("win.outline")
            .build(),
    );

    // Shift+Ctrl+E is not bound by GTK or GtkSourceView; does nothing without an open note.
    let export_action = gio::SimpleAction::new("export-html", None);
    export_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| export_html(&editor)
    ));
    window.add_action(&export_action);
    app.set_accels_for_action("win.export-html", &["<Shift><Control>e"]);

    // Main menu. Ctrl+O is not bound by GTK or GtkSourceView.
    let simple = |name: &str, run: fn(&Rc<Editor>)| {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(clone!(
            #[strong]
            editor,
            move |_, _| run(&editor)
        ));
        window.add_action(&action);
        action
    };
    simple("open-folder", choose_folder);
    app.set_accels_for_action("win.open-folder", &["<Control>o"]);
    simple("open-file", choose_file);
    // Does nothing without an open note.
    simple("export-pdf", export_pdf);
    simple("export-text", export_text);
    simple("export-rtf", export_rtf);
    // No shortcut, as on macOS: none free fits next to Copy as HTML's.
    simple("copy-rich-text", copy_as_rich_text);
    simple("about", about_dialog);
    // Disabled until a folder is open (load_folder enables it).
    simple("new-folder", |editor| {
        let Some(dir) = editor.current_dir() else {
            return;
        };
        let e = Rc::clone(editor);
        sidebar::ask_name(editor, "New Folder", "New Folder", "Create", move |name| {
            e.new_folder(&dir, name)
        });
    })
    .set_enabled(false);
    let open_recent = gio::SimpleAction::new("open-recent", Some(glib::VariantTy::STRING));
    open_recent.connect_activate(clone!(
        #[strong]
        editor,
        move |_, param| {
            if let Some(dir) = param.and_then(|p| p.str()) {
                editor.open_recent(std::path::Path::new(dir));
            }
        }
    ));
    window.add_action(&open_recent);
    // Adds win.show-help-overlay; Ctrl+? is the GNOME convention.
    window.set_help_overlay(Some(&shortcuts_window()));
    app.set_accels_for_action("win.show-help-overlay", &["<Control>question"]);

    // Alt+Shift+Ctrl+C is not bound by GTK or GtkSourceView; does nothing without an open note.
    let copy_html_action = gio::SimpleAction::new("copy-html", None);
    copy_html_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| copy_as_html(&editor)
    ));
    window.add_action(&copy_html_action);
    app.set_accels_for_action("win.copy-html", &["<Alt><Shift><Control>c"]);

    // Ctrl+Shift+P differs from Open Quickly (Ctrl+P) by Shift; GTK/GtkSourceView don't bind it.
    let print_action = gio::SimpleAction::new("print", None);
    print_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| print_note(&editor)
    ));
    window.add_action(&print_action);
    app.set_accels_for_action("win.print", &["<Shift><Control>p"]);

    // Bookmarks: a dialog list, not a header MenuButton (its GtkPopover logs Gdk-CRITICAL under
    // Xvfb, which fails the smoke test). Alt+1…9 are not bound by GTK or GtkSourceView here.
    let bookmarks_action = gio::SimpleAction::new("bookmarks", None);
    bookmarks_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| bookmarks_dialog(&editor)
    ));
    window.add_action(&bookmarks_action);
    let open_bookmark = gio::SimpleAction::new("open-bookmark", Some(glib::VariantTy::STRING));
    open_bookmark.connect_activate(clone!(
        #[strong]
        editor,
        move |_, param| {
            if let Some(n) = param
                .and_then(|p| p.str())
                .and_then(|n| n.parse::<usize>().ok())
            {
                editor.open_bookmark(n.saturating_sub(1));
            }
        }
    ));
    window.add_action(&open_bookmark);
    let url_action = gio::SimpleAction::new("url", Some(glib::VariantTy::STRING));
    url_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, param| {
            if let Some(uri) = param.and_then(|p| p.str()) {
                editor.open_link(uri);
            }
        }
    ));
    window.add_action(&url_action);
    for n in 1..=9 {
        app.set_accels_for_action(&format!("win.open-bookmark::{n}"), &[&format!("<Alt>{n}")]);
    }

    // Back / Forward through the notes this window showed (`history.rs`). Alt+Left / Right are
    // the GNOME keys for it; GtkSourceView's own move-words binding on them is taken over in
    // the editor (`input.rs`). Greyed out until there is a note to go to.
    for (name, direction, accel) in [
        ("go-back", crate::history::Direction::Back, "<Alt>Left"),
        (
            "go-forward",
            crate::history::Direction::Forward,
            "<Alt>Right",
        ),
    ] {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(false);
        action.connect_activate(clone!(
            #[strong]
            editor,
            move |_, _| editor.go(direction)
        ));
        window.add_action(&action);
        app.set_accels_for_action(&format!("win.{name}"), &[accel]);
    }

    // Ctrl+, is the GNOME convention for Preferences; GTK/GtkSourceView don't bind it.
    let prefs_action = gio::SimpleAction::new("preferences", None);
    prefs_action.connect_activate(clone!(
        #[strong]
        editor,
        move |_, _| preferences_dialog(&editor)
    ));
    window.add_action(&prefs_action);
    app.set_accels_for_action("win.preferences", &["<Control>comma"]);
    header.pack_end(
        &gtk::Button::builder()
            .icon_name("preferences-system-symbolic")
            .tooltip_text("Preferences (Ctrl+,)")
            .action_name("win.preferences")
            .build(),
    );

    // Zoom editor + preview (not sidebar/header): Ctrl+= / Ctrl++ in, Ctrl+- out, Ctrl+0 reset.
    // None of these is bound by GTK, GtkSourceView or the app's other shortcuts.
    for (name, steps, accels) in [
        (
            "zoom-in",
            1,
            &["<Control>equal", "<Control>plus", "<Control>KP_Add"][..],
        ),
        (
            "zoom-out",
            -1,
            &["<Control>minus", "<Control>KP_Subtract"][..],
        ),
        ("zoom-reset", 0, &["<Control>0", "<Control>KP_0"][..]),
    ] {
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(clone!(
            #[strong]
            editor,
            move |_, _| {
                let zoom = state::zoom_step(editor.prefs.borrow().zoom, steps);
                editor.prefs.borrow_mut().zoom = zoom;
                editor.apply_prefs();
                editor.remember();
            }
        ));
        window.add_action(&action);
        app.set_accels_for_action(&format!("win.{name}"), accels);
    }

    // Edit / Preview / Side by side. Ctrl+1/2/3 are not bound by GTK or GtkSourceView.
    let mode_action = gio::SimpleAction::new_stateful(
        "mode",
        Some(glib::VariantTy::STRING),
        &state::Mode::Edit.name().to_variant(),
    );
    mode_action.connect_activate(clone!(
        #[strong]
        editor,
        move |action, param| {
            let Some(mode) = param.and_then(|p| p.str()).and_then(state::Mode::from_name) else {
                return;
            };
            action.set_state(&mode.name().to_variant());
            editor.apply_mode(mode);
        }
    ));
    window.add_action(&mode_action);
    let mode_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mode_buttons.add_css_class("linked");
    for (n, (mode, icon, label)) in [
        (state::Mode::Edit, "document-edit-symbolic", "Edit"),
        (state::Mode::Preview, "view-reveal-symbolic", "Preview"),
        (state::Mode::Split, "view-dual-symbolic", "Side by Side"),
    ]
    .into_iter()
    .enumerate()
    {
        let detailed = format!("win.mode::{}", mode.name());
        app.set_accels_for_action(&detailed, &[&format!("<Control>{}", n + 1)]);
        let button = gtk::ToggleButton::builder()
            .icon_name(icon)
            .tooltip_text(format!("{label} (Ctrl+{})", n + 1))
            .build();
        button.set_detailed_action_name(&detailed);
        mode_buttons.append(&button);
    }
    header.pack_end(&mode_buttons);
    style.connect_dark_notify(clone!(
        #[strong]
        editor,
        move |_| {
            editor.apply_scheme();
            editor.refresh_preview()
        }
    ));

    // First layout in side by side: split in the middle (a dragged position is kept).
    editor.panes.connect_max_position_notify(|panes| {
        if !panes.is_position_set() && panes.max_position() > 0 {
            panes.set_position(panes.max_position() / 2);
        }
    });

    // Side by side: the preview follows editor scrolling (throttled).
    let weak = Rc::downgrade(&editor);
    scroll.vadjustment().connect_value_changed(move |_| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        // While the editor follows the preview, syncing back would fight the user's scrolling.
        if editor.sync_timer.borrow().is_some() || editor.preview_poll.borrow().is_some() {
            return;
        }
        let e = Rc::clone(&editor);
        let id = glib::timeout_add_local_once(SYNC_DELAY, move || {
            e.sync_timer.take();
            e.sync_preview();
        });
        editor.sync_timer.replace(Some(id));
    });

    // Ctrl+click on a link in the editor opens it like a click in the preview; the pointer
    // turns into a hand over links while Ctrl is held.
    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&editor);
    click.connect_pressed(move |click, _, x, y| {
        let ctrl = click
            .current_event_state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK);
        if let Some(editor) = weak.upgrade().filter(|_| ctrl)
            && let Some(target) = editor.link_at(x, y)
        {
            click.set_state(gtk::EventSequenceState::Claimed);
            editor.open_editor_link(target);
        }
    });
    editor.view.add_controller(click);
    let motion = gtk::EventControllerMotion::new();
    let weak = Rc::downgrade(&editor);
    // shortcut: the cursor updates on pointer motion only, not when Ctrl alone is pressed.
    motion.connect_motion(move |motion, x, y| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        let ctrl = motion
            .current_event_state()
            .contains(gtk::gdk::ModifierType::CONTROL_MASK);
        let hand = ctrl && editor.link_at(x, y).is_some();
        editor
            .view
            .set_cursor_from_name(Some(if hand { "pointer" } else { "text" }));
    });
    editor.view.add_controller(motion);

    // Hovering an image link shows a thumbnail (max 320x240) of an image in the open folder.
    // The last one is kept: query-tooltip runs on every pointer move.
    // shortcut: cached by path only, an image edited on disk shows stale until another is hovered.
    editor.view.set_has_tooltip(true);
    let weak = Rc::downgrade(&editor);
    let last: RefCell<Option<(PathBuf, gtk::gdk::Texture)>> = RefCell::default();
    editor
        .view
        .connect_query_tooltip(move |_, x, y, keyboard, tooltip| {
            let Some(editor) = weak.upgrade().filter(|_| !keyboard) else {
                return false;
            };
            let Some(links::Target::Href(href)) = editor.link_at(f64::from(x), f64::from(y)) else {
                return false;
            };
            let Some(path) = editor.image_path(&href) else {
                return false;
            };
            let mut last = last.borrow_mut();
            if last.as_ref().is_none_or(|(p, _)| *p != path) {
                // Only shrink (at_scale also enlarges); broken / unsupported file: no tooltip.
                use gtk::gdk_pixbuf::Pixbuf;
                let Some((_, w, h)) = Pixbuf::file_info(&path) else {
                    return false;
                };
                let pixbuf = if w <= 320 && h <= 240 {
                    Pixbuf::from_file(&path)
                } else {
                    Pixbuf::from_file_at_scale(&path, 320, 240, true)
                };
                let Ok(pixbuf) = pixbuf else {
                    return false;
                };
                *last = Some((path, gtk::gdk::Texture::for_pixbuf(&pixbuf)));
            }
            let texture = &last.as_ref().expect("set above").1;
            tooltip.set_custom(Some(&gtk::Picture::for_paintable(texture)));
            true
        });

    // Closing the window (= quitting) saves first. If that fails, the first close is refused
    // so the text is not lost silently; closing again quits without saving.
    window.connect_close_request(clone!(
        #[strong]
        editor,
        move |_| {
            if editor.save() || editor.close_anyway.replace(true) {
                editor.remember();
                glib::Propagation::Proceed
            } else {
                editor.toast("Unsaved changes. Close again to quit without saving.");
                glib::Propagation::Stop
            }
        }
    ));

    // Before anything calls `remember`, or the saved bookmarks would be overwritten.
    editor
        .bookmarks
        .replace(std::mem::take(&mut session.bookmarks));
    editor.recent.replace(std::mem::take(&mut session.recent));
    editor.fill_recent_menu();
    editor.fill_templates_menu();
    editor.prefs.replace(std::mem::take(&mut session.prefs));
    editor
        .positions
        .replace(std::mem::take(&mut session.positions));
    editor.find_match_case.set(session.find_match_case);
    editor.find_whole_word.set(session.find_whole_word);
    editor.find_regex.set(session.find_regex);
    // Chỉ dùng khi thư mục mở ra chính là thư mục đã lưu (`show_folder` lọc theo thư mục).
    editor
        .expanded
        .replace(std::mem::take(&mut session.expanded).into_iter().collect());
    editor.apply_prefs();
    let show_format_bar = editor.prefs.borrow().format_bar;
    format_action.change_state(&show_format_bar.to_variant());
    let focus_mode = editor.prefs.borrow().focus_mode;
    focus_action.change_state(&focus_mode.to_variant());
    let sort = editor.prefs.borrow().sidebar_sort;
    sort_action.change_state(&sort.name().to_variant());
    if crate::timing::enabled() {
        window.add_tick_callback(|_, _| {
            crate::timing::mark(|| "window shown".into());
            glib::ControlFlow::Break
        });
    }
    // The note, then the folder: a scan that is already done fills the sidebar right here, so
    // it is in the window's first frame; a slower one fills it when it ends.
    if let Some(target) = &target {
        if !target.is_dir() {
            editor.open(target);
        }
    } else if let Some(note) = &session.note {
        editor.open(note);
        if editor.path.borrow().is_some() {
            editor.set_mode(session.mode);
        }
    }
    if let Some(scan) = scan {
        editor.load_scan(scan);
    }
    // After the saved Style is applied: no light/dark flash on start.
    window.present();
}

/// About MarkdownMate: name, version, links (same as the metainfo).
/// Wires the preview's page, made on first use (`Editor::preview`): scroll sync from the
/// preview, scripts after each load, the web process dying, and links clicked in the page.
pub(crate) fn wire_preview(editor: &Rc<Editor>, preview: &webkit6::WebView) {
    // Side by side: the editor follows the preview when the user scrolls it (wheel/touchpad,
    // keys, dragging the scrollbar). Watched from GTK, so the page's scripts stay off; the app's
    // own scrolling of the preview sends no input events, so it never syncs back.
    let input = gtk::EventControllerLegacy::new();
    input.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(editor);
    input.connect_event(move |_, event| {
        let user_scroll = match event.event_type() {
            gtk::gdk::EventType::Scroll | gtk::gdk::EventType::KeyPress => true,
            gtk::gdk::EventType::MotionNotify => event
                .modifier_state()
                .contains(gtk::gdk::ModifierType::BUTTON1_MASK),
            _ => false,
        };
        if let Some(editor) = weak.upgrade().filter(|_| user_scroll) {
            editor.follow_preview();
        }
        glib::Propagation::Proceed
    });
    preview.add_controller(input);
    // A click on a link to a heading or footnote of the note scrolls the preview with no
    // scroll input: the URI changes (same page, not loading) once it has scrolled.
    let weak = Rc::downgrade(editor);
    preview.connect_uri_notify(move |preview| {
        if let Some(editor) = weak.upgrade().filter(|_| !preview.is_loading()) {
            editor.follow_preview();
        }
    });

    // After a load (side by side, scroll sync on): follow the editor. A new note starts at
    // the top; re-renders of the same note update in place (refresh_preview).
    let weak = Rc::downgrade(editor);
    preview.connect_load_changed(move |_, event| {
        if event != webkit6::LoadEvent::Finished {
            return;
        }
        let Some(editor) = weak.upgrade() else {
            return;
        };
        crate::timing::mark(|| "preview loaded".into());
        match editor.preview_anchor.take() {
            Some(id) => editor.preview().evaluate_javascript(
                &format!(
                    "var e=document.getElementById({});if(e)e.scrollIntoView();0",
                    render::js_string(&id)
                ),
                Some(SCRIPT_WORLD),
                None,
                gio::Cancellable::NONE,
                |_| {},
            ),
            None => editor.sync_preview(),
        }
        // KaTeX / Mermaid are installed only if the page has a formula / diagram.
        editor.draw_math();
        editor.draw_diagrams();
        editor.refresh_preview_find();
    });

    // The web process died (crash, out of memory): the app keeps running; the next edit or
    // Reload starts a new one.
    let weak = Rc::downgrade(editor);
    preview.connect_web_process_terminated(move |_, reason| {
        let Some(editor) = weak.upgrade() else {
            return;
        };
        let title = match reason {
            webkit6::WebProcessTerminationReason::ExceededMemoryLimit => {
                "Preview ran out of memory"
            }
            _ => "Preview stopped unexpectedly",
        };
        let toast = adw::Toast::builder()
            .title(title)
            .button_label("Reload")
            .build();
        let weak = Rc::downgrade(&editor);
        toast.connect_button_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.load_preview();
            }
        });
        editor.toasts.add_toast(toast);
    });

    // The preview never navigates away from the note: web links open in the default app,
    // anchors (footnotes) scroll, links to notes in the open folder open them in the editor
    // (relative hrefs arrive resolved against the note's file:// URI), anything else is ignored.
    let weak = Rc::downgrade(editor);
    preview.connect_decide_policy(move |_, decision, kind| {
        use webkit6::PolicyDecisionType as T;
        if !matches!(kind, T::NavigationAction | T::NewWindowAction) {
            return false;
        }
        let Some(editor) = weak.upgrade() else {
            return false;
        };
        let uri = decision
            .downcast_ref::<webkit6::NavigationPolicyDecision>()
            .and_then(|d| d.navigation_action())
            .and_then(|mut a| a.request())
            .and_then(|r| r.uri());
        match uri.map(|u| (render::link_action(&u, &editor.page_uri()), u)) {
            Some((render::Link::Follow, _)) => return false,
            Some((link, uri)) => editor.follow(link, &uri),
            None => {}
        }
        decision.ignore();
        true
    });
}

fn about_dialog(editor: &Rc<Editor>) {
    let dialog = adw::AboutDialog::builder()
        .application_name("MarkdownMate")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .comments("Write notes as plain Markdown files in your own folders.")
        .developer_name("HomieLab")
        .website("https://markdownmate.homielab.com/")
        .copyright("© 2026 HomieLab")
        .license_type(gtk::License::Custom)
        .license("All rights reserved.")
        .build();
    dialog.add_link("Donate", "https://ko-fi.com/homielab");
    // Bundled in the binary for the preview's diagrams (`diagrams.rs`).
    dialog.add_legal_section(
        "Mermaid",
        Some("© 2014–2022 Knut Sveidqvist and the Mermaid contributors"),
        gtk::License::MitX11,
        None,
    );
    // Bundled for the preview's formulas (`math.rs`); its fonts' glyphs come from the MathJax
    // fonts (SIL OFL 1.1).
    dialog.add_legal_section(
        "KaTeX",
        Some("© 2013–2020 Khan Academy and other contributors"),
        gtk::License::MitX11,
        None,
    );
    dialog.add_legal_section(
        "KaTeX fonts",
        Some("© 2009–2010 Design Science, Inc.\n© 2014–2017 Khan Academy"),
        gtk::License::Custom,
        Some("SIL Open Font License 1.1 (https://openfontlicense.org)"),
    );
    // Compiled into the preview's code highlighting (`render.rs`, `resources/syntaxes`).
    dialog.add_legal_section(
        "TypeScript and TSX syntaxes",
        Some("© Microsoft Corporation"),
        gtk::License::Apache20,
        None,
    );
    dialog.present(Some(&editor.window));
}

/// Keyboard shortcuts (`gtk::ShortcutsWindow`, Ctrl+?): what `build_window`, `search.rs` and
/// `input.rs` bind. Built from UI XML: GTK 4.14 has no API to add groups to a section.
/// Three named sections (title-bar switcher) of two columns each: all of it on one page is
/// taller than a 600 px screen, and GTK puts at most two columns on a page.
fn shortcuts_window() -> gtk::ShortcutsWindow {
    type Group = (&'static str, &'static [(&'static str, &'static str)]);
    const SECTIONS: &[(&str, &str, &[Group])] = &[
        (
            "files",
            "Files and Editing",
            &[
                (
                    "Files",
                    &[
                        ("<Control>n", "New file"),
                        ("<Control>d", "Today's note"),
                        ("<Control>o", "Open folder"),
                        ("<Control>p", "Open quickly"),
                        ("<Shift><Control>f", "Find in folder"),
                        ("<Control>s", "Save"),
                        ("<Control>r", "Reload from disk"),
                        ("<Alt>1...<Alt>9", "Open bookmark 1–9"),
                        ("<Alt>Left", "Back"),
                        ("<Alt>Right", "Forward"),
                        ("<Control>l", "Go to line"),
                    ],
                ),
                (
                    "Editing",
                    &[
                        ("<Control>f", "Find"),
                        ("<Control>h", "Find and replace"),
                        ("<Control>g", "Next match"),
                        ("<Shift><Control>g", "Previous match"),
                        ("<Shift><Control>v", "Paste as plain text"),
                    ],
                ),
            ],
        ),
        (
            "writing",
            "Writing",
            &[
                (
                    "Format",
                    &[
                        ("<Control>b", "Bold"),
                        ("<Control>i", "Italic"),
                        ("<Control>k", "Link"),
                        ("<Shift><Control>h", "Highlight"),
                        ("<Control>Return", "Toggle task"),
                        ("<Shift><Control>m", "Format bar"),
                    ],
                ),
                (
                    "Lines",
                    &[
                        ("<Control><Alt>Up <Alt>Up", "Move line up"),
                        ("<Control><Alt>Down <Alt>Down", "Move line down"),
                        ("<Shift><Control>d", "Duplicate line"),
                    ],
                ),
                (
                    "Tables",
                    &[
                        ("<Shift><Control>t", "Insert table"),
                        ("Tab <Shift>Tab", "Next / previous cell"),
                        ("<Control><Alt>Return", "Add row below"),
                        ("<Shift><Control><Alt>Return", "Add row above"),
                        ("<Control><Alt>BackSpace", "Delete row"),
                        ("<Shift><Control><Alt>BackSpace", "Delete column"),
                    ],
                ),
            ],
        ),
        (
            "view",
            "View and Export",
            &[
                (
                    "View",
                    &[
                        ("<Control>1", "Edit"),
                        ("<Control>2", "Preview"),
                        ("<Control>3", "Side by side"),
                        ("<Control>f", "Find in preview (preview focused)"),
                        ("<Alt><Control>o", "Outline"),
                        ("<Shift><Control>j", "Focus mode"),
                        ("<Control>equal", "Zoom in"),
                        ("<Control>minus", "Zoom out"),
                        ("<Control>0", "Reset zoom"),
                    ],
                ),
                (
                    "Export",
                    &[
                        ("<Shift><Control>e", "Export as HTML"),
                        ("<Alt><Shift><Control>c", "Copy as HTML"),
                        ("<Shift><Control>p", "Print"),
                    ],
                ),
                (
                    "General",
                    &[
                        ("F10", "Main menu"),
                        ("<Control>comma", "Preferences"),
                        ("<Control>question", "Keyboard shortcuts"),
                    ],
                ),
            ],
        ),
    ];
    let esc = |t: &str| glib::markup_escape_text(t);
    let mut xml = String::from(
        "<interface><object class=\"GtkShortcutsWindow\" id=\"w\"><property name=\"modal\">1</property>",
    );
    for (name, title, groups) in SECTIONS {
        // max-height 10: the tallest group (9 shortcuts + title) fills one column.
        xml += &format!(
            "<child><object class=\"GtkShortcutsSection\"><property name=\"section-name\">{name}</property>\
             <property name=\"title\">{}</property><property name=\"max-height\">10</property>",
            esc(title)
        );
        for (title, keys) in *groups {
            xml += &format!(
                "<child><object class=\"GtkShortcutsGroup\"><property name=\"title\">{}</property>",
                esc(title)
            );
            for (accel, label) in *keys {
                xml += &format!(
                    "<child><object class=\"GtkShortcutsShortcut\"><property name=\"accelerator\">{}</property>\
                     <property name=\"title\">{}</property></object></child>",
                    esc(accel),
                    esc(label)
                );
            }
            xml += "</object></child>";
        }
        xml += "</object></child>";
    }
    xml += "</object></interface>";
    gtk::Builder::from_string(&xml)
        .object("w")
        .expect("shortcuts window XML")
}
