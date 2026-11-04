//! What screen readers (Orca) read for controls that show only an icon, and for rows that
//! show a name next to an icon. One wording for the sidebar, its Favorites and Tags, the
//! outline, Open Quickly and the preview's tag chips. The texts are pure; `name` puts them
//! on a widget.

use gtk::prelude::*;

/// A control that shows only an icon: the name a screen reader reads (the menu title), the
/// tooltip (the name, often with its shortcut) and the icon.
pub(crate) struct IconItem {
    pub label: &'static str,
    pub tooltip: &'static str,
    pub icon: &'static str,
}

impl IconItem {
    /// A button showing the icon, named by `name`.
    pub(crate) fn button(&self) -> gtk::Button {
        let button = gtk::Button::builder().icon_name(self.icon).build();
        name(&button, self.label, self.tooltip);
        button
    }

    /// A toggle button showing the icon, named by `name`.
    pub(crate) fn toggle_button(&self) -> gtk::ToggleButton {
        let button = gtk::ToggleButton::builder().icon_name(self.icon).build();
        name(&button, self.label, self.tooltip);
        button
    }
}

/// Gives `widget` its accessible name `label` and `tooltip` as both its tooltip (for sighted
/// keyboard and mouse users) and its accessible description, when it says more than the name.
pub(crate) fn name(
    widget: &(impl IsA<gtk::Widget> + IsA<gtk::Accessible>),
    label: &str,
    tooltip: &str,
) {
    widget.set_tooltip_text(Some(tooltip));
    if tooltip == label {
        widget.update_property(&[gtk::accessible::Property::Label(label)]);
    } else {
        widget.update_property(&[
            gtk::accessible::Property::Label(label),
            gtk::accessible::Property::Description(tooltip),
        ]);
    }
}

/// Gives `widget` (a text field, a list) its accessible name, with no tooltip.
pub(crate) fn label(widget: &impl IsA<gtk::Accessible>, label: &str) {
    widget.update_property(&[gtk::accessible::Property::Label(label)]);
}

/// What a row shows next to its name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Kind {
    Note,
    Folder,
    Tag,
    Heading,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Note => "note",
            Kind::Folder => "folder",
            Kind::Tag => "tag",
            Kind::Heading => "heading",
        }
    }
}

/// "Name, kind", plus ", favorite" for a Favorites row: "Ideas, note", "Projects, folder,
/// favorite".
pub(crate) fn row(name: &str, kind: Kind, favorite: bool) -> String {
    let label = format!("{name}, {}", kind.name());
    if favorite {
        format!("{label}, favorite")
    } else {
        label
    }
}

/// A Tags row: "work/project, tag, 3 notes".
pub(crate) fn tag_row(tag: &str, notes: usize) -> String {
    let count = if notes == 1 {
        "1 note".to_string()
    } else {
        format!("{notes} notes")
    };
    format!("{}, {count}", row(tag, Kind::Tag, false))
}

/// An outline row: "Introduction, heading level 2".
pub(crate) fn heading(title: &str, level: usize) -> String {
    format!("{title}, heading level {level}")
}

/// `label` and the folder it is in (relative to the open folder; "" = its top): "Ideas, note,
/// in Projects/2026".
pub(crate) fn located(label: &str, folder: &str) -> String {
    if folder.is_empty() {
        label.to_string()
    } else {
        format!("{label}, in {folder}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_say_name_and_kind() {
        assert_eq!(row("Ideas", Kind::Note, false), "Ideas, note");
        assert_eq!(
            row("Projects", Kind::Folder, true),
            "Projects, folder, favorite"
        );
        assert_eq!(
            row("Setup — Ideas", Kind::Heading, false),
            "Setup — Ideas, heading"
        );
        assert_eq!(tag_row("work/project", 3), "work/project, tag, 3 notes");
        assert_eq!(tag_row("work", 1), "work, tag, 1 note");
        assert_eq!(heading("Introduction", 2), "Introduction, heading level 2");
    }

    #[test]
    fn located_names_the_folder() {
        assert_eq!(located("Ideas, note", ""), "Ideas, note");
        assert_eq!(
            located("Ideas, note", "Projects/2026"),
            "Ideas, note, in Projects/2026"
        );
    }
}
