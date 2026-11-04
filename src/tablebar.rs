//! The table bar: buttons for the table commands (`table.rs`), shown under the editor only
//! while the cursor is in a table and the format bar (`formatbar.rs`) is off. `controls` is
//! the bar's content on its own: the format bar shows it as its table section. The buttons activate the `win.table-*` actions,
//! which `window.rs` also puts in the main menu's Table submenu.

use gtk::prelude::*;

use crate::table::Command;

/// What a table bar button shows.
pub(crate) enum Face {
    Text(&'static str),
    Icon(&'static str),
}

/// One table command: its action (without `win.`), menu label, button face and shortcut
/// (handled in the editor by `input.rs`; "" = none).
pub(crate) struct Entry {
    pub action: &'static str,
    pub label: &'static str,
    pub face: Face,
    pub accel: &'static str,
    pub command: Command,
}

/// The table commands in menu and bar order; the bar groups them as rows, columns,
/// alignment and Format Table.
pub(crate) const COMMANDS: &[Entry] = &[
    Entry {
        action: "table-row-above",
        label: "Add Row Above",
        face: Face::Text("Above"),
        accel: "<Shift><Control><Alt>Return",
        command: Command::AddRowAbove,
    },
    Entry {
        action: "table-row-below",
        label: "Add Row Below",
        face: Face::Text("Below"),
        accel: "<Control><Alt>Return",
        command: Command::AddRowBelow,
    },
    Entry {
        action: "table-delete-row",
        label: "Delete Row",
        face: Face::Text("Delete"),
        accel: "<Control><Alt>BackSpace",
        command: Command::DeleteRow,
    },
    Entry {
        action: "table-add-column",
        label: "Add Column",
        face: Face::Text("Add"),
        accel: "",
        command: Command::AddColumn,
    },
    Entry {
        action: "table-delete-column",
        label: "Delete Column",
        face: Face::Text("Delete"),
        accel: "<Shift><Control><Alt>BackSpace",
        command: Command::DeleteColumn,
    },
    Entry {
        action: "table-align-left",
        label: "Align Column Left",
        face: Face::Icon("format-justify-left-symbolic"),
        accel: "",
        command: Command::AlignLeft,
    },
    Entry {
        action: "table-align-center",
        label: "Align Column Center",
        face: Face::Icon("format-justify-center-symbolic"),
        accel: "",
        command: Command::AlignCenter,
    },
    Entry {
        action: "table-align-right",
        label: "Align Column Right",
        face: Face::Icon("format-justify-right-symbolic"),
        accel: "",
        command: Command::AlignRight,
    },
    Entry {
        action: "table-format",
        label: "Format Table",
        face: Face::Text("Format"),
        accel: "",
        command: Command::Format,
    },
];

/// The button groups: where each starts in `COMMANDS`, and its caption.
const GROUPS: [(usize, &str); 4] = [(0, "Row"), (3, "Column"), (5, ""), (8, "")];

/// The table buttons, flat, in captioned groups (full names in their tooltips): the content
/// of the table bar.
pub(crate) fn controls() -> gtk::Box {
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    // Compact buttons (the app's CSS, `main.rs`).
    controls.add_css_class("table-bar");
    for (i, e) in COMMANDS.iter().enumerate() {
        if let Some((_, caption)) = GROUPS.iter().find(|(start, _)| *start == i) {
            if i > 0 {
                controls.append(&gtk::Separator::new(gtk::Orientation::Vertical));
            }
            if !caption.is_empty() {
                controls.append(
                    &gtk::Label::builder()
                        .label(*caption)
                        .margin_start(4)
                        .css_classes(["caption", "dim-label"])
                        .build(),
                );
            }
        }
        controls.append(&button(e.action, e.label, &e.face, e.accel));
    }
    controls
}

/// A flat bar button for `win.<action>`: its name and shortcut in the tooltip, its name for
/// screen readers (the menu title; the face may be an icon or a short word such as "Delete").
/// Clicking it leaves the focus in the editor.
pub(crate) fn button(action: &str, label: &str, face: &Face, accel: &str) -> gtk::Button {
    let tooltip = match gtk::accelerator_parse(accel) {
        Some((key, mods)) => format!("{label} ({})", gtk::accelerator_get_label(key, mods)),
        None => label.to_string(),
    };
    let button = gtk::Button::builder()
        .action_name(format!("win.{action}"))
        .focus_on_click(false)
        .css_classes(["flat"])
        .build();
    match face {
        Face::Text(text) => button.set_label(text),
        Face::Icon(icon) => button.set_icon_name(icon),
    }
    crate::a11y::name(&button, label, &tooltip);
    button
}

/// The table bar: a "Table" heading and `controls`, scrolling sideways when the editor is
/// narrower than the buttons. Hidden; the window shows it while the cursor is in a table.
pub(crate) fn bar() -> gtk::ScrolledWindow {
    let content = gtk::Box::builder()
        .spacing(6)
        .margin_start(6)
        .margin_end(6)
        .margin_top(2)
        .margin_bottom(2)
        .build();
    content.append(
        &gtk::Label::builder()
            .label("Table")
            .margin_start(6)
            .margin_end(6)
            .css_classes(["caption-heading"])
            .build(),
    );
    content.append(&controls());
    gtk::ScrolledWindow::builder()
        .child(&content)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .visible(false)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_label() {
        let mut seen = std::collections::HashSet::new();
        for e in COMMANDS {
            assert!(!e.label.trim().is_empty(), "{}", e.action);
            assert!(e.action.starts_with("table-"), "{}", e.action);
            assert!(seen.insert(e.label), "two buttons named {}", e.label);
            match e.face {
                Face::Icon(icon) => assert!(!icon.is_empty(), "{}", e.action),
                Face::Text(text) => assert!(!text.is_empty(), "{}", e.action),
            }
        }
    }
}
