//! Sidebar: the folder tree as a `gtk::ListView` over a `gtk::TreeListModel`. Virtualized:
//! only rows on screen get widgets, and a folder's rows are added when it is expanded, so a
//! folder of thousands of notes stays fast. Right-click: New Note, New Folder…, Find in
//! “Folder”…, Rename…, Duplicate, Add to / Remove from Favorites, Move to Trash, Move to
//! Folder…, Sort By. Drag an entry onto a folder (or empty space = the open folder) to move it.
//! Entries are in the scan's order (`notes::SortOrder`); under a date order a note whose date
//! changed moves in place (`resort_note`). Above it, the Favorites panel (`favorites_panel`);
//! below it, the Tags panel (`tags_panel`).

use std::cell::{Ref, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::SystemTime;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{Editor, UNTITLED, complete, favorites, file_name, notes};

/// One sidebar entry; `children` (the folder's entries) is None for a note. `date`: the one
/// the order sorts by (`notes::Node::Note`).
struct Item {
    name: String,
    path: PathBuf,
    children: Option<gio::ListStore>,
    date: Option<SystemTime>,
}

impl Item {
    fn key(&self) -> notes::Key<'_> {
        notes::Key {
            is_note: self.children.is_none(),
            date: self.date,
            name: &self.name,
        }
    }
}

/// The Item inside a model object.
fn item(obj: &glib::Object) -> Ref<'_, Item> {
    obj.downcast_ref::<glib::BoxedAnyObject>()
        .expect("sidebar items are BoxedAnyObject")
        .borrow()
}

/// `nodes` (in folder `dir`) as list items. All folders' stores are built up front: plain
/// objects, cheap; widgets are only made for visible rows.
fn store(nodes: &[notes::Node], dir: &Path) -> gio::ListStore {
    let items: Vec<glib::BoxedAnyObject> = nodes
        .iter()
        .map(|node| {
            glib::BoxedAnyObject::new(match node {
                notes::Node::Dir { name, children } => {
                    let path = dir.join(name);
                    Item {
                        name: name.clone(),
                        children: Some(store(children, &path)),
                        path,
                        date: None,
                    }
                }
                notes::Node::Note { name, path, date } => Item {
                    name: name.clone(),
                    path: path.clone(),
                    children: None,
                    date: *date,
                },
            })
        })
        .collect();
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    store.extend_from_slice(&items);
    store
}

/// Moves note `path` (under `root`, whose entries are `store`) to its place in `order` after
/// its date changed on disk (saved here or by another app), without rebuilding the tree:
/// expanded folders and the scroll position stay. Nothing to do under Name, or for a path
/// not listed.
pub fn resort_note(store: &gio::ListStore, root: &Path, path: &Path, order: notes::SortOrder) {
    if order == notes::SortOrder::Name {
        return;
    }
    let start = std::time::Instant::now();
    let Ok(rel) = path.strip_prefix(root) else {
        return;
    };
    let names: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let Some((note, dirs)) = names.split_last() else {
        return;
    };
    let mut store = store.clone();
    for dir in dirs {
        let children = (0..store.n_items())
            .filter_map(|i| store.item(i))
            .find_map(|obj| {
                let it = item(&obj);
                it.children.clone().filter(|_| it.name == *dir)
            });
        let Some(children) = children else {
            return;
        };
        store = children;
    }
    let Some((from, obj)) = (0..store.n_items())
        .filter_map(|i| store.item(i).map(|obj| (i, obj)))
        .find(|(_, obj)| {
            let it = item(obj);
            it.children.is_none() && it.name == *note
        })
    else {
        return;
    };
    let date = std::fs::metadata(path).ok().and_then(|m| order.date(&m));
    {
        let boxed = obj
            .downcast_ref::<glib::BoxedAnyObject>()
            .expect("sidebar items are BoxedAnyObject");
        boxed.borrow_mut::<Item>().date = date;
    }
    // The others are still in order: the note goes after every one that sorts before it.
    let it = item(&obj);
    let to = (0..store.n_items())
        .filter(|&i| i != from)
        .filter_map(|i| store.item(i))
        .filter(|other| notes::compare(order, item(other).key(), it.key()).is_lt())
        .count() as u32;
    drop(it);
    if to != from {
        store.remove(from);
        store.insert(to, &obj);
    }
    crate::timing::took(start, "note re-sorted");
}

/// Path of the folder shown by `row`; None for notes and destroyed rows.
fn folder_path(row: &gtk::TreeListRow) -> Option<PathBuf> {
    let obj = row.item()?;
    let it = item(&obj);
    it.children.as_ref().map(|_| it.path.clone())
}

/// Keeps `editor.expanded` in step with the folder `row`. The row is kept alive in `rows` so it
/// is connected once (the model would otherwise make a fresh row object later).
fn track(editor: &Weak<Editor>, rows: &RefCell<Vec<gtk::TreeListRow>>, row: gtk::TreeListRow) {
    let weak = editor.clone();
    row.connect_expanded_notify(move |row| {
        // A row under a folder being collapsed is destroyed (no item): its own expanded
        // state is kept for when the parent opens again, like the old Expander tree.
        let (Some(editor), Some(path)) = (weak.upgrade(), folder_path(row)) else {
            return;
        };
        let mut set = editor.expanded.borrow_mut();
        if row.is_expanded() {
            set.insert(path);
        } else {
            set.remove(&path);
        }
    });
    rows.borrow_mut().push(row);
}

/// The sidebar for `tree` (the scan of `root`). Folders in `editor.expanded` open expanded.
pub fn sidebar(editor: &Rc<Editor>, tree: &[notes::Node], root: &Path) -> gtk::ScrolledWindow {
    let entries = store(tree, root);
    editor.sidebar_store.replace(Some(entries.clone()));
    let model = gtk::TreeListModel::new(entries, false, false, |obj| {
        item(obj).children.clone().map(|s| s.upcast())
    });
    let weak = Rc::downgrade(editor);

    // Reopen remembered folders. Rows are appended as folders open, so this walks them too.
    let rows = RefCell::new(Vec::new());
    let mut i = 0;
    while let Some(row) = model.row(i) {
        if let Some(path) = folder_path(&row) {
            if editor.expanded.borrow().contains(&path) {
                row.set_expanded(true);
            }
            track(&weak, &rows, row);
        }
        i += 1;
    }
    // Rows that appear later (a folder opened by the user).
    let w = weak.clone();
    model.connect_items_changed(move |model, pos, _, added| {
        for i in pos..pos + added {
            let Some(row) = model.row(i) else { continue };
            let Some(path) = folder_path(&row) else {
                continue;
            };
            track(&w, &rows, row.clone());
            if w.upgrade()
                .is_some_and(|e| e.expanded.borrow().contains(&path))
            {
                // Not inside this signal: the list view has not seen these rows yet.
                glib::idle_add_local_once(move || row.set_expanded(true));
            }
        }
    });

    // After Move to Folder…: the moved entry's row (its folders were opened above).
    let reveal = editor.sidebar_reveal.take().and_then(|path| {
        (0..model.n_items()).find(|&i| {
            model
                .row(i)
                .and_then(|r| r.item())
                .is_some_and(|obj| item(&obj).path == path)
        })
    });

    let factory = gtk::SignalListItemFactory::new();
    let w = weak.clone();
    factory.connect_setup(move |_, obj| {
        let Some(li) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let expander = gtk::TreeExpander::builder().child(&label).build();
        context_menu(&expander, &w);
        drag_source(&expander);
        li.set_child(Some(&expander));
    });
    factory.connect_bind(|_, obj| {
        let Some(li) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let (Some(row), Some(expander)) = (
            li.item().and_downcast::<gtk::TreeListRow>(),
            li.child().and_downcast::<gtk::TreeExpander>(),
        ) else {
            return;
        };
        if let (Some(obj), Some(label)) =
            (row.item(), expander.child().and_downcast::<gtk::Label>())
        {
            label.set_label(&item(&obj).name);
        }
        expander.set_list_row(Some(&row));
    });
    factory.connect_unbind(|_, obj| {
        if let Some(expander) = obj
            .downcast_ref::<gtk::ListItem>()
            .and_then(|li| li.child())
            .and_downcast::<gtk::TreeExpander>()
        {
            expander.set_list_row(None);
        }
    });

    let view = gtk::ListView::builder()
        .model(&gtk::NoSelection::new(Some(model)))
        .factory(&factory)
        .single_click_activate(true)
        .build();
    view.add_css_class("navigation-sidebar");
    view.add_css_class("sidebar-tree");
    editor.sidebar_view.replace(Some(view.clone()));
    drop_target(&view, &weak, root);
    if let Some(i) = reveal {
        let view = view.clone();
        // Once the view is in the window.
        glib::idle_add_local_once(move || view.scroll_to(i, gtk::ListScrollFlags::FOCUS, None));
    }
    // Click: a folder opens/closes, a note opens in the editor.
    view.connect_activate(move |view, pos| {
        let Some(row) = view
            .model()
            .and_then(|m| m.item(pos))
            .and_downcast::<gtk::TreeListRow>()
        else {
            return;
        };
        if folder_path(&row).is_some() {
            row.set_expanded(!row.is_expanded());
        } else if let (Some(editor), Some(obj)) = (weak.upgrade(), row.item()) {
            let path = item(&obj).path.clone();
            editor.open(&path);
        }
    });
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&view)
        .build()
}

/// Opens Files at `path`'s folder with `path` selected (sidebar menu ▸ Show in Files, Find
/// Unused Attachments); a toast if Files can't be opened.
pub(crate) fn show_in_files(editor: &Rc<Editor>, path: &Path) {
    let weak = Rc::downgrade(editor);
    gtk::FileLauncher::new(Some(&gio::File::for_path(path))).open_containing_folder(
        Some(&editor.window),
        gio::Cancellable::NONE,
        move |result| {
            if let (Err(err), Some(e)) = (result, weak.upgrade()) {
                e.toast(&format!("Cannot open Files: {err}"));
            }
        },
    );
}

/// Scrolls the tree to `path` and focuses its row, opening the folders above it first (a
/// Favorites folder, Open Quickly). Nothing happens for a path the tree does not list.
pub fn reveal(view: &gtk::ListView, path: &Path) {
    let Some(model) = view
        .model()
        .and_downcast::<gtk::NoSelection>()
        .and_then(|s| s.model())
        .and_downcast::<gtk::TreeListModel>()
    else {
        return;
    };
    let find = |p: &Path| {
        (0..model.n_items()).find_map(|i| {
            let row = model.row(i)?;
            let obj = row.item()?;
            (item(&obj).path == p).then_some((i, row))
        })
    };
    let mut dirs: Vec<&Path> = path.ancestors().skip(1).collect();
    dirs.reverse();
    for dir in dirs {
        // Expanding adds the folder's rows to the model at once.
        if let Some((_, row)) = find(dir) {
            row.set_expanded(true);
        }
    }
    if let Some((i, _)) = find(path) {
        view.scroll_to(i, gtk::ListScrollFlags::FOCUS, None);
    }
}

/// Right-click menu of a sidebar entry: New Note, New Folder… and Find in “Folder”… (folders),
/// Rename…, Duplicate, Add to / Remove from Favorites, Show in Files, Copy Path, Copy Wiki Link
/// (notes), Move to Trash, Move to Folder…, then Sort By (the current order checked).
/// The entry is read at click time: list rows are recycled for other entries.
fn context_menu(widget: &gtk::TreeExpander, editor: &Weak<Editor>) {
    let gesture = gtk::GestureClick::builder()
        .button(gtk::gdk::BUTTON_SECONDARY)
        .build();
    let weak = editor.clone();
    gesture.connect_pressed(move |gesture, _, x, y| {
        let (Some(editor), Some(widget)) = (weak.upgrade(), gesture.widget()) else {
            return;
        };
        let Some(obj) = widget
            .downcast_ref::<gtk::TreeExpander>()
            .and_then(|e| e.item())
        else {
            return;
        };
        let (path, is_note) = {
            let it = item(&obj);
            (it.path.clone(), it.children.is_none())
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let popover = gtk::Popover::builder()
            .child(&menu)
            .has_arrow(false)
            .pointing_to(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1))
            .build();
        popover.set_parent(&widget);
        popover.connect_closed(|p| {
            // Unparent after the item's click handler has run.
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        let dir = if is_note {
            path.parent().map(Path::to_path_buf).unwrap_or_default()
        } else {
            path.clone()
        };
        let item = |label: &str, run: Box<dyn Fn()>| {
            let button = gtk::Button::builder().label(label).has_frame(false).build();
            let popover = popover.clone();
            button.connect_clicked(move |_| {
                popover.popdown();
                run();
            });
            menu.append(&button);
        };
        let (e, d) = (Rc::clone(&editor), dir.clone());
        item("New Note", Box::new(move || e.new_note(&d, UNTITLED)));
        if !is_note {
            let (e, d) = (Rc::clone(&editor), dir.clone());
            item(
                "New Folder…",
                Box::new(move || {
                    let (e2, d) = (Rc::clone(&e), d.clone());
                    ask_name(&e, "New Folder", "New Folder", "Create", move |name| {
                        e2.new_folder(&d, name)
                    });
                }),
            );
            // The open folder itself is Find in Folder (Shift+Ctrl+F); it has no entry here.
            let (e, d) = (Rc::clone(&editor), dir.clone());
            item(
                &format!("Find in “{}”…", file_name(&dir)),
                Box::new(move || crate::dialogs::find_in_scope(&e, &d)),
            );
        }
        let (e, p) = (Rc::clone(&editor), path.clone());
        item(
            "Rename…",
            Box::new(move || {
                let (e2, p2) = (Rc::clone(&e), p.clone());
                ask_name(&e, "Rename", &file_name(&p), "Rename", move |name| {
                    e2.rename(&p2, name, is_note)
                });
            }),
        );
        let (e, p) = (Rc::clone(&editor), path.clone());
        item("Duplicate", Box::new(move || e.duplicate(&p)));
        let root = editor.root.borrow().clone().unwrap_or_default();
        let favourite = favorites::contains(&editor.favorites.borrow(), &root, &path);
        let (e, p) = (Rc::clone(&editor), path.clone());
        item(
            if favourite {
                "Remove from Favorites"
            } else {
                "Add to Favorites"
            },
            Box::new(move || {
                let p = p.clone();
                e.change_favorites(move |list, root| favorites::toggle(list, root, &p, !favourite));
            }),
        );
        let (e, p) = (Rc::clone(&editor), path.clone());
        item("Show in Files", Box::new(move || show_in_files(&e, &p)));
        let (e, p) = (Rc::clone(&editor), path.clone());
        item(
            "Copy Path",
            Box::new(move || {
                e.window.clipboard().set_text(&p.to_string_lossy());
                e.toast("Path copied");
            }),
        );
        if is_note {
            let (e, p) = (Rc::clone(&editor), path.clone());
            item(
                "Copy Wiki Link",
                Box::new(move || {
                    let root = e.root.borrow().clone().unwrap_or_default();
                    let name = complete::link_name(&p, &e.note_list.borrow(), &root);
                    e.window.clipboard().set_text(&format!("[[{name}]]"));
                    e.toast("Link copied");
                }),
            );
        }
        let (e, p) = (Rc::clone(&editor), path.clone());
        item("Move to Trash", Box::new(move || e.trash(&p)));
        let (e, p) = (Rc::clone(&editor), path.clone());
        item("Move to Folder…", Box::new(move || move_to_folder(&e, &p)));
        menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let heading = gtk::Label::builder()
            .label("Sort By")
            .xalign(0.0)
            .margin_start(12)
            .margin_top(6)
            .margin_bottom(3)
            .css_classes(["dim-label", "caption-heading"])
            .build();
        menu.append(&heading);
        let current = editor.prefs.borrow().sidebar_sort;
        for order in notes::SortOrder::ALL {
            let check = gtk::Image::from_icon_name("object-select-symbolic");
            check.set_opacity(if order == current { 1.0 } else { 0.0 });
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            row.append(
                &gtk::Label::builder()
                    .label(order.title())
                    .xalign(0.0)
                    .hexpand(true)
                    .build(),
            );
            row.append(&check);
            let button = gtk::Button::builder().child(&row).has_frame(false).build();
            let (popover, window) = (popover.clone(), editor.window.clone());
            button.connect_clicked(move |_| {
                popover.popdown();
                // The window's action group: names without the `win.` prefix.
                gio::prelude::ActionGroupExt::activate_action(
                    &window,
                    "sidebar-sort",
                    Some(&order.name().to_variant()),
                );
            });
            menu.append(&button);
        }
        popover.popup();
    });
    widget.add_controller(gesture);
}

/// Private mime type of a sidebar drag: the entry's path. Not text or a file list, so other
/// apps (Files) ignore it; the sidebar moves the entry, the editor links to it.
pub(crate) const DRAG_MIME: &str = "application/x-markdownmate-entry";

thread_local! {
    /// The entry being dragged from this sidebar, so drop targets can be highlighted while
    /// hovering (the drag data itself is only readable on drop).
    static DRAGGED: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Lets a row be dragged; the entry is read at drag time since rows are recycled.
fn drag_source(widget: &gtk::TreeExpander) {
    let source = gtk::DragSource::builder()
        .actions(gtk::gdk::DragAction::MOVE)
        .build();
    source.connect_prepare(|source, _, _| {
        use std::os::unix::ffi::OsStrExt;
        let obj = source
            .widget()?
            .downcast_ref::<gtk::TreeExpander>()?
            .item()?;
        let path = item(&obj).path.clone();
        let bytes = glib::Bytes::from(path.as_os_str().as_bytes());
        DRAGGED.set(Some(path));
        Some(gtk::gdk::ContentProvider::for_bytes(DRAG_MIME, &bytes))
    });
    source.connect_drag_end(|_, _, _| DRAGGED.set(None));
    widget.add_controller(source);
}

/// The folder a drop at (x, y) of `view` moves into: a folder row, a note row's folder, or the
/// open folder `root` on empty space.
fn drop_dir(view: &gtk::Widget, x: f64, y: f64, root: &Path) -> PathBuf {
    // The row under the pointer: pick a widget, then walk up to its TreeExpander.
    let row = view
        .pick(x, y, gtk::PickFlags::DEFAULT)
        .and_then(|w| {
            std::iter::successors(Some(w), |w| w.parent())
                .find_map(|w| w.downcast::<gtk::TreeExpander>().ok())
        })
        .and_then(|e| e.item());
    match row {
        Some(o) => {
            let it = item(&o);
            match it.children {
                Some(_) => it.path.clone(),
                None => it
                    .path
                    .parent()
                    .map_or(root.to_path_buf(), Path::to_path_buf),
            }
        }
        None => root.to_path_buf(),
    }
}

/// The on-screen row of folder `dir` (rows are recycled, so only visible folders have one).
fn folder_row(view: &gtk::Widget, dir: &Path) -> Option<gtk::Widget> {
    std::iter::successors(view.first_child(), |w| w.next_sibling()).find(|row| {
        row.first_child()
            .and_downcast::<gtk::TreeExpander>()
            .and_then(|e| e.item())
            .is_some_and(|o| item(&o).path == dir)
    })
}

/// Dropping a dragged entry on a folder row moves it into that folder; on a note row, into the
/// note's folder; on empty space, into the open folder `root`. While dragging, the folder that
/// would receive it is highlighted (CSS class `drop-target`): its row, or the whole list for `root`;
/// nothing where the move is refused or a no-op (`naming::move_target`).
/// Files dropped from another app (a file list) are copied into that folder instead
/// (`Editor::copy_in`); the drop folder is highlighted the same way.
fn drop_target(view: &gtk::ListView, editor: &Weak<Editor>, root: &Path) {
    let formats = gtk::gdk::ContentFormatsBuilder::new()
        .add_mime_type(DRAG_MIME)
        .add_mime_type("text/uri-list")
        .add_type(gtk::gdk::FileList::static_type())
        .build();
    let target = gtk::DropTargetAsync::new(
        Some(formats),
        gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY,
    );
    // The folder under the pointer, whether the dragged entry may move there, and the widget
    // highlighted for it. The row is looked up on every motion: rows are recycled when the
    // tree changes (hovering a folder expands it).
    #[derive(Default)]
    struct Hover {
        dir: Option<PathBuf>,
        ok: bool,
        lit: Option<gtk::Widget>,
    }
    let hover: Rc<RefCell<Hover>> = Rc::default();
    let light = |hover: &RefCell<Hover>, w: Option<gtk::Widget>| {
        let mut h = hover.borrow_mut();
        if h.lit != w {
            if let Some(old) = h.lit.take() {
                old.remove_css_class("drop-target");
            }
            if let Some(new) = &w {
                new.add_css_class("drop-target");
            }
            h.lit = w;
        }
    };
    let (h, r) = (Rc::clone(&hover), root.to_path_buf());
    let mv = move |t: &gtk::DropTargetAsync, drop: &gtk::gdk::Drop, x, y| {
        let internal = is_internal(drop);
        if let Some(view) = t.widget() {
            let dir = drop_dir(&view, x, y, &r);
            if h.borrow().dir.as_ref() != Some(&dir) {
                // An outside drop is a copy: any folder takes it.
                let ok = !internal
                    || DRAGGED.with_borrow(|from| {
                        from.as_ref().is_some_and(|f| {
                            matches!(crate::naming::move_target(f, &dir, &r), Ok(Some(_)))
                        })
                    });
                let mut h = h.borrow_mut();
                (h.dir, h.ok) = (Some(dir.clone()), ok);
            }
            let w = match h.borrow().ok {
                false => None,
                true if dir == r => Some(view.clone()),
                true => folder_row(&view, &dir),
            };
            light(&h, w);
        }
        if internal {
            gtk::gdk::DragAction::MOVE
        } else {
            gtk::gdk::DragAction::COPY
        }
    };
    // A folder opening on hover inserts rows; GTK keeps a row below it in place, so the list
    // jumps and the pointer ends up over another folder (the drop then goes there). Keep the
    // scroll position instead (#129).
    if let Some(model) = view.model() {
        // Weak: the model must not keep its own view alive.
        let (h, v) = (Rc::clone(&hover), view.downgrade());
        model.connect_items_changed(move |_, _, _, _| {
            let Some(v) = v.upgrade() else { return };
            if let (true, Some(adj)) = (h.borrow().dir.is_some(), v.vadjustment()) {
                let value = std::cell::Cell::new(Some(adj.value()));
                // The jump happens at the next layout: restore once that frame is done.
                v.add_tick_callback(move |_, _| {
                    if let Some(value) = value.take() {
                        let adj = adj.clone();
                        glib::idle_add_local_once(move || adj.set_value(value));
                    }
                    glib::ControlFlow::Break
                });
            }
        });
    }
    target.connect_drag_enter(mv.clone());
    target.connect_drag_motion(mv);
    let h = Rc::clone(&hover);
    target.connect_drag_leave(move |_, _| {
        light(&h, None);
        h.borrow_mut().dir = None;
    });
    let (weak, root) = (editor.clone(), root.to_path_buf());
    target.connect_drop(move |target, drop, x, y| {
        light(&hover, None);
        hover.borrow_mut().dir = None;
        let Some(view) = target.widget() else {
            return false;
        };
        let dir = drop_dir(&view, x, y, &root);
        let (weak, drop) = (weak.clone(), drop.clone());
        if !is_internal(&drop) {
            glib::spawn_future_local(async move {
                let files = drop
                    .read_value_future(gtk::gdk::FileList::static_type(), glib::Priority::DEFAULT)
                    .await
                    .ok()
                    .and_then(|v| v.get::<gtk::gdk::FileList>().ok());
                drop.finish(if files.is_some() {
                    gtk::gdk::DragAction::COPY
                } else {
                    gtk::gdk::DragAction::empty()
                });
                let paths: Vec<PathBuf> = files
                    .map(|f| f.files().iter().filter_map(|f| f.path()).collect())
                    .unwrap_or_default();
                if let (Some(editor), false) = (weak.upgrade(), paths.is_empty()) {
                    editor.copy_in(&paths, &dir);
                }
            });
            return true;
        }
        glib::spawn_future_local(async move {
            let from = read_path(&drop).await;
            drop.finish(if from.is_some() {
                gtk::gdk::DragAction::MOVE
            } else {
                gtk::gdk::DragAction::empty()
            });
            // `from` comes from any app that offers DRAG_MIME: move_into checks it is inside
            // the open folder.
            if let (Some(editor), Some(from)) = (weak.upgrade(), from) {
                editor.move_into(&from, &dir);
            }
        });
        true
    });
    view.add_controller(target);
}

/// Whether `drop` is a sidebar drag (DRAG_MIME) rather than files from another app.
fn is_internal(drop: &gtk::gdk::Drop) -> bool {
    drop.formats().contain_mime_type(DRAG_MIME)
}

/// The path carried by a sidebar drag (DRAG_MIME).
pub(crate) async fn read_path(drop: &gtk::gdk::Drop) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let (stream, _) = drop
        .read_future(&[DRAG_MIME], glib::Priority::DEFAULT)
        .await
        .ok()?;
    let mut bytes = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(4096, glib::Priority::DEFAULT)
            .await
            .ok()?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    (!bytes.is_empty()).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&bytes)))
}

/// The folders of sidebar `store` and below, in its order, each with its `depth` (1 for the
/// open folder's own folders).
fn folders(store: &gio::ListStore, depth: usize, out: &mut Vec<(PathBuf, usize)>) {
    for obj in (0..store.n_items()).filter_map(|i| store.item(i)) {
        let it = item(&obj);
        if let Some(children) = &it.children {
            out.push((it.path.clone(), depth));
            folders(children, depth + 1, out);
        }
    }
}

/// Whether Move to Folder… can move `entry` into folder `dir`: not where it already is, not
/// into itself or one of its own folders.
fn can_move_into(entry: &Path, dir: &Path) -> bool {
    entry.parent() != Some(dir) && !dir.starts_with(entry)
}

/// Move to Folder…: the open folder and its folders as an indented tree (sidebar order), the
/// entry's own folder, the entry and its subfolders greyed out. Move (or Enter / a double-click
/// on a row) moves it exactly like dropping it on that folder: same refusal of a taken name,
/// same offer to update links; then the sidebar shows it. Cancel or Esc changes nothing.
fn move_to_folder(editor: &Rc<Editor>, entry: &Path) {
    let (Some(root), Some(store)) = (
        editor.root.borrow().clone(),
        editor.sidebar_store.borrow().clone(),
    ) else {
        return;
    };
    let mut choices = vec![(root.clone(), 0)];
    folders(&store, 1, &mut choices);
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        // A click picks the folder; Move, Enter or a double-click moves.
        .activate_on_single_click(false)
        .css_classes(["navigation-sidebar"])
        .build();
    for (dir, depth) in &choices {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_start(i32::try_from(depth * 18).unwrap_or(i32::MAX));
        row.append(&gtk::Image::from_icon_name("folder-symbolic"));
        row.append(
            &gtk::Label::builder()
                .label(file_name(dir))
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build(),
        );
        let row = gtk::ListBoxRow::builder().child(&row).build();
        let allowed = can_move_into(entry, dir);
        row.set_sensitive(allowed);
        row.set_selectable(allowed);
        row.set_activatable(allowed);
        list.append(&row);
    }
    let cancel = gtk::Button::with_label("Cancel");
    let move_button = gtk::Button::builder()
        .label("Move")
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&move_button);
    let content = adw::ToolbarView::new();
    content.add_top_bar(&header);
    content.set_content(Some(
        &gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build(),
    ));
    let dialog = adw::Dialog::builder()
        .title(format!("Move “{}” to", file_name(entry)))
        .content_width(380)
        .content_height(420)
        .child(&content)
        .build();
    let (weak, d) = (Rc::downgrade(editor), dialog.downgrade());
    let (entry, choices) = (entry.to_path_buf(), Rc::new(choices));
    let run = Rc::new(move |row: &gtk::ListBoxRow| {
        let dir = usize::try_from(row.index())
            .ok()
            .and_then(|i| choices.get(i));
        let (Some(editor), Some((dir, _))) = (weak.upgrade(), dir) else {
            return;
        };
        if let Some(d) = d.upgrade() {
            d.close();
        }
        if let Some(to) = editor.move_into(&entry, dir) {
            editor.reveal_in_sidebar(&to);
        }
    });
    let b = move_button.clone();
    list.connect_row_selected(move |_, row| b.set_sensitive(row.is_some()));
    let r = Rc::clone(&run);
    list.connect_row_activated(move |_, row| r(row));
    let l = list.clone();
    move_button.connect_clicked(move |_| {
        if let Some(row) = l.selected_row() {
            run(&row);
        }
    });
    let d = dialog.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(d) = d.upgrade() {
            d.close();
        }
    });
    dialog.present(Some(&editor.window));
    let mut i = 0;
    while let Some(row) = list.row_at_index(i) {
        if row.is_sensitive() {
            row.grab_focus();
            break;
        }
        i += 1;
    }
}

/// Asks for a name; `done` runs with the text when the user confirms.
pub(crate) fn ask_name(
    editor: &Editor,
    heading: &str,
    initial: &str,
    ok: &str,
    done: impl Fn(&str) + 'static,
) {
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .extra_child(&entry)
        .default_response("ok")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("ok", ok)]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.connect_response(Some("ok"), move |_, _| done(&entry.text()));
    dialog.present(Some(&editor.window));
}

/// The sidebar's Favorites section (hidden while empty), above the folder tree: a heading over
/// a list of notes and folders (`fill_favorites`).
pub fn favorites_panel() -> (gtk::Box, gtk::ListBox) {
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    list.update_property(&[gtk::accessible::Property::Label("Favorites")]);
    let heading = gtk::Label::builder()
        .label("Favorites")
        .xalign(0.0)
        .margin_start(12)
        .margin_top(6)
        .build();
    heading.add_css_class("heading");
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.append(&heading);
    panel.append(
        &gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(220)
            .build(),
    );
    panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    panel.set_visible(false);
    (panel, list)
}

/// Shows `entries` (`favorites::entries`: path, still there) in `list`, in order: icon and
/// name; one that is gone is dimmed (a click removes it).
pub fn fill_favorites(list: &gtk::ListBox, panel: &gtk::Box, entries: &[(PathBuf, bool)]) {
    while let Some(row) = list.first_child() {
        list.remove(&row);
    }
    for (path, there) in entries {
        let is_dir = path.is_dir();
        let name = file_name(path);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_margin_start(6);
        row.append(&gtk::Image::from_icon_name(if is_dir {
            "folder-symbolic"
        } else {
            "text-x-generic-symbolic"
        }));
        row.append(
            &gtk::Label::builder()
                .label(&name)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build(),
        );
        let row = gtk::ListBoxRow::builder().child(&row).build();
        let label = if *there {
            row.set_tooltip_text(Some(&path.to_string_lossy()));
            name
        } else {
            row.add_css_class("dim-label");
            row.set_tooltip_text(Some("No longer there. Click to remove it from Favorites."));
            format!("{name}, missing")
        };
        row.update_property(&[gtk::accessible::Property::Label(&label)]);
        list.append(&row);
    }
    panel.set_visible(!entries.is_empty());
}

/// Favorites rows: a click (or Enter) opens the note, shows the folder in the tree, or removes
/// a favourite that is gone (`Editor::open_favorite`); right-click ▸ Remove from Favorites.
/// Row i is `Editor::favorites[i]`.
pub fn connect_favorites(list: &gtk::ListBox, editor: &Weak<Editor>) {
    let weak = editor.clone();
    list.connect_row_activated(move |_, row| {
        if let (Some(editor), Ok(i)) = (weak.upgrade(), usize::try_from(row.index())) {
            editor.open_favorite(i);
        }
    });
    let gesture = gtk::GestureClick::builder()
        .button(gtk::gdk::BUTTON_SECONDARY)
        .build();
    let (weak, list_weak) = (editor.clone(), list.downgrade());
    gesture.connect_pressed(move |gesture, _, x, y| {
        let (Some(editor), Some(list)) = (weak.upgrade(), list_weak.upgrade()) else {
            return;
        };
        let Some(row) = list.row_at_y(y as i32) else {
            return;
        };
        let (Some(root), Some(rel)) = (
            editor.root.borrow().clone(),
            usize::try_from(row.index())
                .ok()
                .and_then(|i| editor.favorites.borrow().get(i).cloned()),
        ) else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let button = gtk::Button::builder()
            .label("Remove from Favorites")
            .has_frame(false)
            .build();
        let popover = gtk::Popover::builder()
            .child(&button)
            .has_arrow(false)
            .pointing_to(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1))
            .build();
        popover.set_parent(&list);
        popover.connect_closed(|p| {
            // Unparent after the button's click handler has run.
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        let p = popover.clone();
        button.connect_clicked(move |_| {
            p.popdown();
            let path = root.join(&rel);
            editor.change_favorites(move |list, root| favorites::toggle(list, root, &path, false));
        });
        popover.popup();
    });
    list.add_controller(gesture);
}

/// The sidebar's Tags section (hidden while there is no tag): a heading over a list of tags.
pub fn tags_panel() -> (gtk::Box, gtk::ListBox) {
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    let heading = gtk::Label::builder()
        .label("Tags")
        .xalign(0.0)
        .margin_start(12)
        .margin_top(6)
        .build();
    heading.add_css_class("heading");
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    panel.append(&heading);
    panel.append(
        &gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(220)
            .build(),
    );
    panel.set_visible(false);
    (panel, list)
}

/// Shows `index` (tags in tree order, see `tags::index`) in `list`: the tree as an indented
/// list, each row the tag's last part and its note count.
// shortcut: no expand/collapse per tag; switch to a TreeListModel if tag lists get long.
pub fn fill_tags(list: &gtk::ListBox, panel: &gtk::Box, index: &[(String, Vec<PathBuf>)]) {
    while let Some(row) = list.first_child() {
        list.remove(&row);
    }
    for (tag, notes) in index {
        let depth = tag.matches('/').count() as i32;
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.set_margin_start(6 + 12 * depth);
        let name = gtk::Label::builder()
            .label(format!("#{}", tag.rsplit('/').next().unwrap_or(tag)))
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .tooltip_text(format!("#{tag}"))
            .build();
        row.append(&name);
        let count = gtk::Label::new(Some(&notes.len().to_string()));
        count.add_css_class("dim-label");
        count.add_css_class("numeric");
        row.append(&count);
        list.append(&row);
    }
    panel.set_visible(!index.is_empty());
}

/// Right-click menu of a row of the Tags list: Rename Tag… (`Editor::rename_tag`). The tag
/// is read at click time from `Editor::tag_index` (row i = tag i).
pub fn tags_menu(list: &gtk::ListBox, editor: &Weak<Editor>) {
    let gesture = gtk::GestureClick::builder()
        .button(gtk::gdk::BUTTON_SECONDARY)
        .build();
    let (weak, list_weak) = (editor.clone(), list.downgrade());
    gesture.connect_pressed(move |gesture, _, x, y| {
        let (Some(editor), Some(list)) = (weak.upgrade(), list_weak.upgrade()) else {
            return;
        };
        let Some(row) = list.row_at_y(y as i32) else {
            return;
        };
        let tag = usize::try_from(row.index())
            .ok()
            .and_then(|i| editor.tag_index.borrow().get(i).map(|(tag, _)| tag.clone()));
        let Some(tag) = tag else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);
        list.select_row(Some(&row));
        let button = gtk::Button::builder()
            .label("Rename Tag…")
            .has_frame(false)
            .build();
        let popover = gtk::Popover::builder()
            .child(&button)
            .has_arrow(false)
            .pointing_to(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1))
            .build();
        popover.set_parent(&list);
        popover.connect_closed(|p| {
            // Unparent after the button's click handler has run.
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        let p = popover.clone();
        button.connect_clicked(move |_| {
            p.popdown();
            editor.rename_tag(&tag);
        });
        popover.popup();
    });
    list.add_controller(gesture);
}

/// The notes tagged `#tag`: a dialog listing them (name and path in the folder); activating
/// one opens it.
pub fn tag_notes(editor: &Rc<Editor>, tag: &str, notes: &[PathBuf]) {
    let root = editor.root.borrow().clone().unwrap_or_default();
    let list = gtk::ListBox::new();
    list.add_css_class("navigation-sidebar");
    for path in notes {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::new(Some(&file_name(path)));
        row.append(&name);
        let rel = gtk::Label::builder()
            .label(path.strip_prefix(&root).unwrap_or(path).to_string_lossy())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        rel.add_css_class("dim-label");
        rel.add_css_class("caption");
        row.append(&rel);
        list.append(&row);
    }
    let dialog = adw::Dialog::builder()
        .title(format!("#{tag}"))
        .content_width(480)
        .content_height(360)
        .child(
            &gtk::ScrolledWindow::builder()
                .child(&list)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .margin_top(12)
                .margin_bottom(12)
                .margin_start(12)
                .margin_end(12)
                .build(),
        )
        .build();
    let (weak, d, notes) = (Rc::downgrade(editor), dialog.downgrade(), notes.to_vec());
    list.connect_row_activated(move |_, row| {
        let path = usize::try_from(row.index()).ok().and_then(|i| notes.get(i));
        if let (Some(editor), Some(path)) = (weak.upgrade(), path) {
            if let Some(d) = d.upgrade() {
                d.close();
            }
            editor.open(path);
        }
    });
    dialog.present(Some(&editor.window));
    if let Some(row) = list.row_at_index(0) {
        row.grab_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn names(store: &gio::ListStore) -> Vec<String> {
        (0..store.n_items())
            .filter_map(|i| store.item(i))
            .map(|obj| item(&obj).name.clone())
            .collect()
    }

    #[test]
    fn move_to_folder_lists_folders_as_a_tree() {
        let note = |name: &str, dir: &str| notes::Node::Note {
            name: name.into(),
            path: PathBuf::from(format!("{dir}/{name}")),
            date: None,
        };
        let dir = |name: &str, children| notes::Node::Dir {
            name: name.into(),
            children,
        };
        let tree = [
            dir(
                "b",
                vec![dir("c", vec![note("n.md", "/r/b/c")]), note("m.md", "/r/b")],
            ),
            dir("a", vec![]),
            note("top.md", "/r"),
        ];
        let mut out = Vec::new();
        folders(&store(&tree, Path::new("/r")), 1, &mut out);
        let p = PathBuf::from;
        assert_eq!(out, [(p("/r/b"), 1), (p("/r/b/c"), 2), (p("/r/a"), 1)]);
    }

    #[test]
    fn move_to_folder_greys_out_its_own_folder_and_itself() {
        let p = Path::new;
        // A note: anywhere but its own folder.
        assert!(!can_move_into(p("/r/b/m.md"), p("/r/b")));
        assert!(can_move_into(p("/r/b/m.md"), p("/r")));
        assert!(can_move_into(p("/r/b/m.md"), p("/r/b/c")));
        // A folder: not its parent, itself or below it; a sibling with a longer name is fine.
        assert!(!can_move_into(p("/r/b"), p("/r")));
        assert!(!can_move_into(p("/r/b"), p("/r/b")));
        assert!(!can_move_into(p("/r/b"), p("/r/b/c")));
        assert!(can_move_into(p("/r/b"), p("/r/bb")));
        assert!(can_move_into(p("/r/b/c"), p("/r")));
    }

    #[test]
    fn a_changed_note_moves_to_its_place_in_place() {
        let root = std::env::temp_dir().join(format!("mdmate-resort-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let now = SystemTime::now();
        let touch = |rel: &str, secs_ago: u64| {
            let path = root.join(rel);
            std::fs::write(&path, "x").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(now - Duration::from_secs(secs_ago))
                .unwrap();
        };
        for (rel, secs) in [("a.md", 30), ("b.md", 20), ("c.md", 10), ("sub/n.md", 5)] {
            touch(rel, secs);
        }
        let order = notes::SortOrder::Modified;
        let top = store(&notes::scan(&root, order).unwrap(), &root);
        assert_eq!(names(&top), ["sub", "c.md", "b.md", "a.md"]);
        let sub = item(&top.item(0).unwrap()).children.clone().unwrap();

        touch("a.md", 0);
        resort_note(&top, &root, &root.join("a.md"), order);
        assert_eq!(names(&top), ["sub", "a.md", "c.md", "b.md"]);
        // The folder object is the same: its rows (expanded or not) are untouched.
        assert_eq!(item(&top.item(0).unwrap()).children.as_ref(), Some(&sub));

        touch("c.md", 0);
        resort_note(&top, &root, &root.join("c.md"), notes::SortOrder::Name);
        assert_eq!(
            names(&top),
            ["sub", "a.md", "c.md", "b.md"],
            "Name: nothing moves"
        );
        resort_note(&top, &root, &root.join("sub/n.md"), order);
        resort_note(&top, &root, &root.join("gone.md"), order);
        assert_eq!(names(&sub), ["n.md"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
