//! The format bar (Shift+Ctrl+M): flat buttons above the editor for the Markdown formatting
//! commands, Insert Table and, while the cursor is in a table, the table bar's controls
//! (`tablebar.rs`). Each button activates a `win.format-*` action (registered here) or an
//! existing one (`win.insert-table`, `win.table-*`); the edits themselves are the editor's
//! commands (`format.rs` through `input::format_selection`, `lines.rs` through
//! `input::line_command`), so a button does exactly what its key does, as one undo step.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::clone;

use crate::editor::Editor;
use crate::input::{LineCommand, format_selection, line_command};
use crate::tablebar::{self, Face};
use crate::{format, lists, state};

/// What a format action does to the text.
#[derive(Clone, Copy)]
enum Run {
    /// A `format.rs` edit of the selection (or the word at the cursor).
    Inline(fn(&str, (usize, usize)) -> format::Edit),
    /// A `lines.rs` command on the lines the selection touches.
    Line(LineCommand),
}

/// One format bar button: its action (without `win.`), name, face and shortcut ("" = none;
/// the keys themselves are handled in the editor by `input.rs`).
struct Item {
    action: &'static str,
    label: &'static str,
    face: Face,
    accel: &'static str,
    /// None: an action registered elsewhere (`window.rs`).
    run: Option<Run>,
}

/// The buttons in bar order; `GROUPS` puts separators between them.
const ITEMS: &[Item] = &[
    Item {
        action: "format-heading",
        label: "Heading (next level)",
        face: Face::Text("H"),
        accel: "",
        run: Some(Run::Line(LineCommand::Heading)),
    },
    Item {
        action: "format-bold",
        label: "Bold",
        face: Face::Icon("format-text-bold-symbolic"),
        accel: "<Control>b",
        run: Some(Run::Inline(|t, s| {
            format::emphasis(t, s, format::Style::Bold)
        })),
    },
    Item {
        action: "format-italic",
        label: "Italic",
        face: Face::Icon("format-text-italic-symbolic"),
        accel: "<Control>i",
        run: Some(Run::Inline(|t, s| {
            format::emphasis(t, s, format::Style::Italic)
        })),
    },
    Item {
        action: "format-strikethrough",
        label: "Strikethrough",
        face: Face::Icon("format-text-strikethrough-symbolic"),
        accel: "",
        run: Some(Run::Inline(|t, s| format::toggle(t, s, "~~"))),
    },
    Item {
        action: "format-highlight",
        label: "Highlight",
        // No highlighter icon in Adwaita: the Markdown, like Inline Code's `</>`.
        face: Face::Text("=="),
        accel: "<Shift><Control>h",
        run: Some(Run::Inline(|t, s| format::toggle(t, s, "=="))),
    },
    Item {
        action: "format-inline-code",
        label: "Inline Code",
        face: Face::Text("</>"),
        accel: "",
        run: Some(Run::Inline(|t, s| format::toggle(t, s, "`"))),
    },
    Item {
        action: "format-link",
        label: "Link",
        face: Face::Icon("insert-link-symbolic"),
        accel: "<Control>k",
        run: Some(Run::Inline(format::link)),
    },
    Item {
        action: "format-bulleted-list",
        label: "Bulleted List",
        face: Face::Icon("view-list-bullet-symbolic"),
        accel: "",
        run: Some(Run::Line(LineCommand::Prefix(lists::Kind::Bullet))),
    },
    Item {
        action: "format-numbered-list",
        label: "Numbered List",
        face: Face::Icon("view-list-ordered-symbolic"),
        accel: "",
        run: Some(Run::Line(LineCommand::Prefix(lists::Kind::Numbered))),
    },
    Item {
        action: "format-task-list",
        label: "Task List",
        face: Face::Icon("checkbox-checked-symbolic"),
        accel: "",
        run: Some(Run::Line(LineCommand::Prefix(lists::Kind::Task))),
    },
    Item {
        action: "format-quote",
        label: "Quote",
        face: Face::Text("\u{201c}"),
        accel: "",
        run: Some(Run::Line(LineCommand::Prefix(lists::Kind::Quote))),
    },
    Item {
        action: "format-code-block",
        label: "Code Block",
        face: Face::Text("{ }"),
        accel: "",
        run: Some(Run::Line(LineCommand::CodeBlock)),
    },
    Item {
        action: "insert-table",
        label: "Insert Table",
        face: Face::Icon("x-office-spreadsheet-symbolic"),
        accel: "<Shift><Control>t",
        run: None,
    },
];

/// Where each button group starts in `ITEMS`: heading, inline styles, link, blocks, table.
const GROUPS: [usize; 5] = [0, 1, 6, 7, 12];

/// Registers the `win.format-*` actions. Like the menu's line commands they do nothing
/// without an open note, switch Preview to Edit, put the focus back in the editor and ring
/// the bell when there is nothing to do (a block toggle on code only).
pub(crate) fn add_actions(window: &adw::ApplicationWindow, editor: &Rc<Editor>) {
    for item in ITEMS {
        let Some(run) = item.run else { continue };
        let action = gio::SimpleAction::new(item.action, None);
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
                let done = match run {
                    Run::Inline(f) => format_selection(editor.buffer.upcast_ref(), f),
                    Run::Line(command) => line_command(editor.view.upcast_ref(), command),
                };
                if done {
                    editor
                        .view
                        .scroll_mark_onscreen(&editor.buffer.get_insert());
                } else {
                    editor.view.error_bell();
                }
            }
        ));
        window.add_action(&action);
    }
}

/// The format bar (hidden; `win.format-bar` shows it) and its table section, which the
/// window shows only while the cursor is in a table. Scrolls sideways when the editor is
/// narrower than the buttons.
pub(crate) fn bar() -> (gtk::ScrolledWindow, gtk::Box) {
    let content = gtk::Box::builder()
        .spacing(2)
        .margin_start(6)
        .margin_end(6)
        .margin_top(2)
        .margin_bottom(2)
        .css_classes(["table-bar"])
        .build();
    for (i, item) in ITEMS.iter().enumerate() {
        if i > 0 && GROUPS.contains(&i) {
            content.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        }
        content.append(&tablebar::button(
            item.action,
            item.label,
            &item.face,
            item.accel,
        ));
    }
    let table = gtk::Box::builder().spacing(2).visible(false).build();
    table.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    table.append(&tablebar::controls());
    content.append(&table);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&content)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .visible(false)
        .build();
    (scroll, table)
}
