//! Folder contents: a folder clicked in the sidebar (or a favourite folder) is shown in place of
//! the note — its path from the open folder at the top (click a folder there to go up), then its
//! subfolders by name and its notes in the sidebar's order, each with its date modified. A click
//! or Enter on a row goes into the subfolder or opens the note; Back returns here (folders are
//! history entries like notes). The open note is saved first and stays loaded underneath:
//! opening it again (sidebar, Back, its tab) brings it back as it was.
//!
//! The rows come from the sidebar's own listing (`sidebar::children`: direct children only, no
//! extra folder walk), so they follow the same rescans; dates are read when a row is drawn.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::a11y::{self, Kind};
use crate::{Editor, file_name, sidebar, title};

/// The folder whose contents are shown, and its list (None: the "not found" page).
pub(crate) struct Shown {
    pub dir: PathBuf,
    /// The folder the page was built for (a rename changes `dir`; the path at the top follows
    /// on the next refresh).
    built: PathBuf,
    store: Option<gio::ListStore>,
}

/// One row: the date is read the first time the row is drawn.
struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    modified: Cell<Option<Option<SystemTime>>>,
}

impl Entry {
    fn modified(&self) -> Option<SystemTime> {
        if let Some(date) = self.modified.get() {
            return date;
        }
        let date = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        self.modified.set(Some(date));
        date
    }
}

/// The folders from `root` down to `dir` (both included) as (name, path); just `dir` when it
/// is not inside `root`. The open folder is named by its own name.
pub fn trail(root: &Path, dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rel) = dir.strip_prefix(root) else {
        return vec![(file_name(dir), dir.to_path_buf())];
    };
    let mut out = vec![(file_name(root), root.to_path_buf())];
    let mut path = root.to_path_buf();
    for part in rel.components() {
        path.push(part);
        out.push((
            part.as_os_str().to_string_lossy().into_owned(),
            path.clone(),
        ));
    }
    out
}

/// A date as rows show it: the time for today, else the date ("21 Nov 2026"), in local time.
fn date_text(date: SystemTime, now: &glib::DateTime) -> String {
    let secs = date
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let Ok(local) = glib::DateTime::from_unix_local(secs) else {
        return String::new();
    };
    let today = local.year() == now.year() && local.day_of_year() == now.day_of_year();
    let format = if today { "%H:%M" } else { "%-d %b %Y" };
    local
        .format(format)
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn objects(entries: Vec<(String, PathBuf, bool)>) -> Vec<glib::BoxedAnyObject> {
    entries
        .into_iter()
        .map(|(name, path, is_dir)| {
            glib::BoxedAnyObject::new(Entry {
                name,
                path,
                is_dir,
                modified: Cell::new(None),
            })
        })
        .collect()
}

impl Editor {
    /// The folder whose contents are shown in place of the note, if any.
    pub(crate) fn shown_folder(&self) -> Option<PathBuf> {
        self.folder_shown.borrow().as_ref().map(|s| s.dir.clone())
    }

    /// Shows folder `dir`'s contents in place of the note (a sidebar click, a favourite, a row
    /// of this page, Back / Forward), the open note saved first; stays on the note if it
    /// cannot be saved.
    pub(crate) fn show_folder_contents(self: &Rc<Self>, dir: &Path) {
        if !self.save() {
            return;
        }
        self.keep_position();
        self.build_folder_page(dir);
        if !self.tab_busy.get() {
            self.record_shown(dir);
        }
    }

    /// Forgets the folder page (the note or another page is about to replace it).
    pub(crate) fn hide_folder_contents(&self) {
        self.folder_shown.replace(None);
    }

    /// The open folder was rescanned or a folder renamed: the shown list follows (scroll kept);
    /// the page is rebuilt when the folder moved or is no longer listed.
    pub(crate) fn refresh_folder_contents(self: &Rc<Self>) {
        let Some((dir, same)) = self
            .folder_shown
            .borrow()
            .as_ref()
            .map(|s| (s.dir.clone(), s.built == s.dir && s.store.is_some()))
        else {
            return;
        };
        if same && self.refill_folder_contents() {
            return;
        }
        self.build_folder_page(&dir);
    }

    /// A note's date changed on disk: the shown list is read again from the sidebar (its order
    /// and dates). False when the folder is no longer listed.
    pub(crate) fn refill_folder_contents(&self) -> bool {
        let shown = self.folder_shown.borrow();
        let Some(Shown {
            dir,
            store: Some(store),
            ..
        }) = shown.as_ref()
        else {
            return false;
        };
        let Some(entries) = self.folder_entries(dir) else {
            return false;
        };
        store.splice(0, store.n_items(), &objects(entries));
        true
    }

    fn folder_entries(&self, dir: &Path) -> Option<Vec<(String, PathBuf, bool)>> {
        let root = self.root.borrow().clone()?;
        let store = self.sidebar_store.borrow().clone()?;
        sidebar::children(&store, &root, dir)
    }

    fn build_folder_page(self: &Rc<Self>, dir: &Path) {
        let entries = self.folder_entries(dir);
        let root = self
            .root
            .borrow()
            .clone()
            .unwrap_or_else(|| dir.to_path_buf());
        let store = entries.map(|entries| {
            let store = gio::ListStore::new::<glib::BoxedAnyObject>();
            store.extend_from_slice(&objects(entries));
            store
        });
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.append(&self.trail_bar(&root, dir));
        page.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        match &store {
            Some(store) => page.append(&self.contents_list(store, dir)),
            None => page.append(
                &adw::StatusPage::builder()
                    .icon_name("folder-symbolic")
                    .title("Folder Not Found")
                    .description("It was moved, removed or has no notes left.")
                    .vexpand(true)
                    .build(),
            ),
        }
        self.folder_shown.replace(Some(Shown {
            dir: dir.to_path_buf(),
            built: dir.to_path_buf(),
            store,
        }));
        self.split.set_content(Some(&page));
        self.window
            .set_title(Some(&title::window_title(Some(&file_name(dir)))));
    }

    /// The path from the open folder to `dir`: the folders above are buttons going there.
    fn trail_bar(self: &Rc<Self>, root: &Path, dir: &Path) -> gtk::Widget {
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        bar.set_margin_start(12);
        bar.set_margin_end(12);
        bar.set_margin_top(8);
        bar.set_margin_bottom(8);
        let parts = trail(root, dir);
        let last = parts.len() - 1;
        for (i, (name, path)) in parts.into_iter().enumerate() {
            if i > 0 {
                let sep = gtk::Label::new(Some("/"));
                sep.add_css_class("dim-label");
                bar.append(&sep);
            }
            if i == last {
                let label = gtk::Label::builder()
                    .label(&name)
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .margin_start(6)
                    .build();
                label.add_css_class("heading");
                bar.append(&label);
            } else {
                let button = gtk::Button::builder()
                    .label(&name)
                    .has_frame(false)
                    .tooltip_text(format!("Show “{name}”"))
                    .build();
                a11y::label(&button, &format!("Go up to {name}"));
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    if let Some(editor) = weak.upgrade() {
                        editor.show_folder_contents(&path);
                    }
                });
                bar.append(&button);
            }
        }
        a11y::label(&bar, "Folder path");
        bar.upcast()
    }

    /// `store`'s rows (icon, name, date modified) in a list; a click or Enter goes into a
    /// subfolder or opens a note. Virtualized: only rows on screen get widgets.
    fn contents_list(self: &Rc<Self>, store: &gio::ListStore, dir: &Path) -> gtk::Widget {
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, obj| {
            let Some(li) = obj.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.set_margin_start(6);
            row.set_margin_end(6);
            row.set_margin_top(4);
            row.set_margin_bottom(4);
            row.append(&gtk::Image::new());
            row.append(
                &gtk::Label::builder()
                    .xalign(0.0)
                    .hexpand(true)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build(),
            );
            let date = gtk::Label::new(None);
            date.add_css_class("dim-label");
            date.add_css_class("numeric");
            row.append(&date);
            li.set_child(Some(&row));
        });
        factory.connect_bind(|_, obj| {
            let Some(li) = obj.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let (Some(entry), Some(row)) = (
                li.item().and_downcast::<glib::BoxedAnyObject>(),
                li.child().and_downcast::<gtk::Box>(),
            ) else {
                return;
            };
            let entry = entry.borrow::<Entry>();
            let (Some(icon), Some(name), Some(date)) = (
                row.first_child().and_downcast::<gtk::Image>(),
                row.first_child()
                    .and_then(|w| w.next_sibling())
                    .and_downcast::<gtk::Label>(),
                row.last_child().and_downcast::<gtk::Label>(),
            ) else {
                return;
            };
            icon.set_icon_name(Some(if entry.is_dir {
                "folder-symbolic"
            } else {
                "text-x-generic-symbolic"
            }));
            name.set_label(&entry.name);
            let when = match (entry.modified(), glib::DateTime::now_local()) {
                (Some(d), Ok(now)) => date_text(d, &now),
                _ => String::new(),
            };
            date.set_label(&when);
            // Screen readers: "Ideas, note, modified 21 Nov 2026".
            let kind = if entry.is_dir {
                Kind::Folder
            } else {
                Kind::Note
            };
            let label = a11y::row(&entry.name, kind, false);
            li.set_accessible_label(&if when.is_empty() {
                label
            } else {
                format!("{label}, modified {when}")
            });
        });
        let view = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .single_click_activate(true)
            .build();
        view.add_css_class("navigation-sidebar");
        a11y::label(&view, &format!("Contents of {}", file_name(dir)));
        let weak = Rc::downgrade(self);
        view.connect_activate(move |view, pos| {
            let (Some(editor), Some(obj)) = (
                weak.upgrade(),
                view.model()
                    .and_then(|m| m.item(pos))
                    .and_downcast::<glib::BoxedAnyObject>(),
            ) else {
                return;
            };
            let (path, is_dir) = {
                let entry = obj.borrow::<Entry>();
                (entry.path.clone(), entry.is_dir)
            };
            if is_dir {
                editor.show_folder_contents(&path);
            } else {
                editor.open(&path);
            }
        });
        // Focus the list once shown, so the arrow keys and Enter work at once.
        let v = view.clone();
        glib::idle_add_local_once(move || {
            v.grab_focus();
        });
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build()
            .upcast()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_goes_from_the_open_folder_down() {
        let p = PathBuf::from;
        assert_eq!(
            trail(Path::new("/n/Notes"), Path::new("/n/Notes/a/b")),
            [
                ("Notes".into(), p("/n/Notes")),
                ("a".into(), p("/n/Notes/a")),
                ("b".into(), p("/n/Notes/a/b")),
            ]
        );
        assert_eq!(
            trail(Path::new("/n/Notes"), Path::new("/n/Notes")),
            [("Notes".into(), p("/n/Notes"))]
        );
        assert_eq!(
            trail(Path::new("/n/Notes"), Path::new("/x/y")),
            [("y".into(), p("/x/y"))]
        );
    }
}
