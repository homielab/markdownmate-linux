//! Hover preview of note links in the rendered preview: resting the pointer on a wiki link
//! (alias links and embed titles too) shows, after `DELAY`, the linked note's name and first
//! lines in a popover at the pointer — the same text as the editor's hover tooltip
//! (`hover::read`). Closed when the pointer leaves the link, on scroll, click, key press and
//! re-render; it never takes the keyboard.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use webkit6::prelude::*;

use crate::editor::Editor;
use crate::hover::{self, Peek};
use crate::render;

/// How long the pointer rests on a link before its preview shows.
const DELAY: Duration = Duration::from_millis(600);

pub(crate) struct LinkPeek {
    editor: Weak<Editor>,
    view: webkit6::WebView,
    popover: gtk::Popover,
    /// The link URI under the pointer, if any.
    target: RefCell<Option<String>>,
    /// Bumped by every change of target and every close: a pending preview for an older one
    /// is dropped.
    generation: Cell<u64>,
    /// Last pointer position in the page's coordinates.
    pointer: Cell<(f64, f64)>,
}

impl LinkPeek {
    /// Watches `view`'s pointer and links (`mouse-target-changed`) for `editor`.
    pub(crate) fn new(editor: &Rc<Editor>, view: &webkit6::WebView) -> Rc<Self> {
        let popover = gtk::Popover::builder()
            .autohide(false)
            .has_arrow(false)
            .can_focus(false)
            .position(gtk::PositionType::Bottom)
            .build();
        popover.set_offset(0, 12);
        popover.set_parent(view);
        let peek = Rc::new(Self {
            editor: Rc::downgrade(editor),
            view: view.clone(),
            popover,
            target: RefCell::new(None),
            generation: Cell::new(0),
            pointer: Cell::new((0.0, 0.0)),
        });
        let p = peek.popover.clone();
        view.connect_destroy(move |_| p.unparent());
        // Hidden with the page (Edit mode) and not shown again with it.
        let weak = Rc::downgrade(&peek);
        view.connect_unmap(move |_| {
            if let Some(peek) = weak.upgrade() {
                peek.close();
            }
        });

        let weak = Rc::downgrade(&peek);
        view.connect_mouse_target_changed(move |_, hit, _| {
            if let Some(peek) = weak.upgrade() {
                let uri = hit.context_is_link().then(|| hit.link_uri()).flatten();
                peek.hover(uri.map(String::from));
            }
        });

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&peek);
        motion.connect_motion(move |_, x, y| {
            if let Some(peek) = weak.upgrade() {
                peek.pointer.set((x, y));
            }
        });
        let weak = Rc::downgrade(&peek);
        motion.connect_leave(move |_| {
            if let Some(peek) = weak.upgrade() {
                peek.hover(None);
            }
        });
        view.add_controller(motion);

        // Scroll, click and keys on the page close it; WebKit sees them all the same.
        let input = gtk::EventControllerLegacy::new();
        input.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&peek);
        input.connect_event(move |_, event| {
            use gdk::EventType as E;
            if matches!(
                event.event_type(),
                E::Scroll | E::ButtonPress | E::KeyPress | E::TouchBegin
            ) && let Some(peek) = weak.upgrade()
            {
                peek.close();
            }
            glib::Propagation::Proceed
        });
        view.add_controller(input);

        // A key typed anywhere in the window (the editor side by side) closes it too.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&peek);
        keys.connect_key_pressed(move |_, _, _, _| {
            if let Some(peek) = weak.upgrade() {
                peek.close();
            }
            glib::Propagation::Proceed
        });
        editor.window.add_controller(keys);
        peek
    }

    /// The link under the pointer changed to `uri` (None: no link). A note link schedules its
    /// preview; anything else closes the shown one.
    fn hover(self: &Rc<Self>, uri: Option<String>) {
        if *self.target.borrow() == uri {
            return;
        }
        self.close();
        self.target.replace(uri.clone());
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        let Some((path, heading)) = uri.and_then(|u| render::note_link(&u, &editor.page_uri()))
        else {
            return;
        };
        let Some(root) = editor.root.borrow().clone() else {
            return;
        };
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            glib::timeout_future(DELAY).await;
            let current = |peek: &Self| {
                peek.generation.get() == generation
                    && !peek
                        .editor
                        .upgrade()
                        .and_then(|e| e.preview_find.get().map(|f| f.has_focus()))
                        .unwrap_or(false)
            };
            if !weak.upgrade().is_some_and(|peek| current(&peek)) {
                return;
            }
            let peek = gio::spawn_blocking(move || read(&path, &root, heading.as_deref()))
                .await
                .ok()
                .flatten();
            if let (Some(this), Some(peek)) = (weak.upgrade(), peek)
                && current(&this)
            {
                this.show(&peek);
            }
        });
    }

    fn show(&self, peek: &Peek) {
        if !self.view.is_mapped() {
            return;
        }
        let (x, y) = self.pointer.get();
        self.popover
            .set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.popover.set_child(Some(&hover::widget(peek)));
        self.popover.popup();
    }

    /// Hides the preview and drops a pending one (scroll, click, key, re-render). The link
    /// stays the target: it shows again only once the pointer leaves it and comes back.
    pub(crate) fn close(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        if self.popover.is_visible() {
            self.popover.popdown();
        }
    }
}

/// The preview of note `path` (as linked) if it is a note inside `root` (`note_in_folder`, as a
/// click checks): its name and `hover::read` excerpt. None for missing or unreadable notes.
fn read(path: &std::path::Path, root: &std::path::Path, heading: Option<&str>) -> Option<Peek> {
    let note: PathBuf = render::note_in_folder(path, root)?;
    let name = note.file_stem()?.to_string_lossy().into_owned();
    let text = hover::read(&note, heading).ok()?;
    Some(Peek { name, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_notes_inside_the_folder_only() {
        let folder = render::TestFolder::new(
            "previewhover",
            &[
                (
                    "Notes/Plan.md",
                    "---\ntags: [a]\n---\nIntro\n\n## My Part\nthere\n",
                ),
                ("Other.txt", "not a note"),
            ],
        );
        let root = folder.0.clone();
        let page = format!("file://{}/Home.md", root.display());
        let link = |uri: &str| {
            let (path, heading) = render::note_link(uri, &page)?;
            read(&path, &root, heading.as_deref())
        };
        let plan = format!("file://{}/Notes/Plan.md", root.display());
        assert_eq!(
            link(&plan),
            Some(Peek {
                name: "Plan".into(),
                text: "Intro\n## My Part\nthere".into()
            })
        );
        // `[[Plan#My Part]]` links the heading's id; a missing heading gives the top.
        assert_eq!(
            link(&format!("{plan}#my-part")).map(|p| p.text),
            Some("## My Part\nthere".into())
        );
        assert_eq!(
            link(&format!("{plan}#nope")).map(|p| p.text),
            Some("Intro\n## My Part\nthere".into())
        );
        // Missing notes, notes outside the folder, links to the page itself: nothing.
        assert_eq!(link(&format!("file://{}/Gone.md", root.display())), None);
        assert_eq!(link(&format!("file://{}/../x.md", root.display())), None);
        assert_eq!(link(&format!("{page}#part")), None);
        assert_eq!(link("mm-missing:Gone"), None);
    }
}
