//! Sidebar: the folder tree as a `gtk::ListView` over a `gtk::TreeListModel`. Virtualized:
//! only rows on screen get widgets, and a folder's rows are added when it is expanded, so a
//! folder of thousands of notes stays fast. Right-click: New Note, New Folder…, Rename…, Move
//! to Trash. Drag an entry onto a folder (or empty space = the open folder) to move it.
//! Below it, the Tags panel (`tags_panel`).

use std::cell::{Ref, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use gtk::{gio, glib};

use crate::{Editor, UNTITLED, file_name, notes};

/// One sidebar entry; `children` (the folder's entries) is None for a note.
struct Item {
    name: String,
    path: PathBuf,
    children: Option<gio::ListStore>,
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
                    }
                }
                notes::Node::Note { name, path } => Item {
                    name: name.clone(),
                    path: path.clone(),
                    children: None,
                },
            })
        })
        .collect();
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    store.extend_from_slice(&items);
    store
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
    let model = gtk::TreeListModel::new(store(tree, root), false, false, |obj| {
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
    drop_target(&view, &weak, root);
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

/// Right-click menu of a sidebar entry: New Note, New Folder… (folders), Rename…, Move to Trash.
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
        item(
            "Show in Files",
            Box::new(move || {
                let weak = Rc::downgrade(&e);
                gtk::FileLauncher::new(Some(&gio::File::for_path(&p))).open_containing_folder(
                    Some(&e.window),
                    gio::Cancellable::NONE,
                    move |result| {
                        if let (Err(err), Some(e)) = (result, weak.upgrade()) {
                            e.toast(&format!("Cannot open Files: {err}"));
                        }
                    },
                );
            }),
        );
        let (e, p) = (Rc::clone(&editor), path.clone());
        item(
            "Copy Path",
            Box::new(move || {
                e.window.clipboard().set_text(&p.to_string_lossy());
                e.toast("Path copied");
            }),
        );
        let (e, p) = (Rc::clone(&editor), path.clone());
        item("Move to Trash", Box::new(move || e.trash(&p)));
        popover.popup();
    });
    widget.add_controller(gesture);
}

/// Private mime type of a sidebar drag: the entry's path. Not text or a file list, so the
/// editor and other apps (Files) ignore it; only the sidebar accepts it.
const DRAG_MIME: &str = "application/x-markdownmate-entry";

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
async fn read_path(drop: &gtk::gdk::Drop) -> Option<PathBuf> {
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
