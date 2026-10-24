use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self};
use webkit6::prelude::*;

use crate::editor::Editor;
use crate::{PREVIEW_DELAY, PREVIEW_LINE_JS, SCRIPT_WORLD, SYNC_DELAY, SYNC_JS, file_name};
use crate::{diagrams, math, render, save, state};

impl Editor {
    /// Side by side: re-render the preview shortly after typing stops.
    pub(crate) fn schedule_preview(self: &Rc<Self>) {
        if self.mode.get() != state::Mode::Split {
            return;
        }
        if let Some(id) = self.preview_timer.take() {
            id.remove();
        }
        let editor = Rc::clone(self);
        let id = glib::timeout_add_local_once(PREVIEW_DELAY, move || {
            editor.preview_timer.take();
            editor.refresh_preview();
        });
        self.preview_timer.replace(Some(id));
    }

    /// Renders the buffer into the preview, if the preview is showing. Re-rendering the same
    /// note swaps the page's head and body in place (no reload: no jump to the top, the scroll
    /// position stays); another note loads a new page. The page's own scripts stay off
    /// (`enable-javascript-markup`); only this app's scripts run, in a separate script world.
    pub(crate) fn refresh_preview(self: &Rc<Self>) {
        self.preview_generation
            .set(self.preview_generation.get().wrapping_add(1));
        if self.mode.get() == state::Mode::Edit {
            return;
        }
        if !sandbox_usable() {
            self.toast(NO_SANDBOX);
            // Back to Edit once the current mode change is done: the preview cannot load.
            let editor = Rc::clone(self);
            glib::idle_add_local_once(move || editor.set_mode(state::Mode::Edit));
            return;
        }
        let uri = self.page_uri();
        // A footnote click leaves `#fragment` on the URI; still the same note.
        let same_note = self
            .preview
            .uri()
            .is_some_and(|u| u.split('#').next() == Some(uri.as_str()));
        if !same_note || self.preview.is_loading() {
            self.load_preview();
            return;
        }
        // Rendered off the main thread (a large note with much code takes ~1s, mostly syntax
        // highlighting); a newer refresh or page load meanwhile makes this result stale.
        // At most one such render runs: refreshes meanwhile only ask for one more afterwards.
        let mut slot = self.preview_render.get();
        let start = slot.start();
        self.preview_render.set(slot);
        if !start {
            return;
        }
        let generation = self.preview_generation.get();
        let job = self.page_job(None);
        let editor = Rc::clone(self);
        glib::spawn_future_local(async move {
            let html = gio::spawn_blocking(job).await;
            let mut slot = editor.preview_render.get();
            let again = slot.finish();
            editor.preview_render.set(slot);
            match html {
                Ok(html) if editor.preview_generation.get() == generation => {
                    editor.swap_page(&html)
                }
                _ if again => editor.refresh_preview(), // with the latest text
                _ => {}
            }
        });
    }

    /// Replaces the shown page's head and body with `html` in place (scroll position kept).
    /// Diagrams and formulas already drawn are reused; only a changed one is drawn again
    /// (`diagrams`, `math`).
    fn swap_page(self: &Rc<Self>, html: &str) {
        // The class carries light/dark and the head the theme, so style changes apply too.
        let script = format!(
            "(function(h){{var y=scrollY,d=new DOMParser().parseFromString(h,'text/html');{}{}\
document.documentElement.className=d.documentElement.className;\
document.head.replaceWith(d.head);document.body.replaceWith(d.body);{}{}scrollTo(0,y);return true}})({})",
            diagrams::TAKE,
            math::TAKE,
            math::APPLY,
            diagrams::APPLY,
            render::js_string(html)
        );
        let has_diagrams = render::has_diagrams(html);
        let has_math = render::has_math(html);
        let editor = Rc::clone(self);
        self.preview.evaluate_javascript(
            &script,
            Some(SCRIPT_WORLD),
            None,
            gio::Cancellable::NONE,
            move |result| match result {
                Ok(_) => {
                    editor.refresh_preview_find();
                    editor.sync_preview();
                    if has_math {
                        editor.draw_math();
                    }
                    if has_diagrams {
                        editor.draw_diagrams();
                    }
                }
                // No usable page (e.g. the web process died): load it instead.
                Err(_) => editor.load_preview(),
            },
        );
    }

    /// The page changed under an open Find in Preview bar: recount its matches.
    pub(crate) fn refresh_preview_find(&self) {
        if let Some(find) = self.preview_find.get() {
            find.refresh();
        }
    }

    /// Typesets the preview's formulas with KaTeX (nothing is loaded for a page without one),
    /// then follows the editor again: KaTeX changed the page's heights.
    pub(crate) fn draw_math(self: &Rc<Self>) {
        let editor = Rc::clone(self);
        math::draw(&self.preview, move |typeset| {
            if typeset {
                editor.sync_preview();
            }
        });
    }

    /// Draws the preview's Mermaid diagrams (nothing is loaded for a page without one), then
    /// follows the editor again: the diagrams changed the page's heights.
    pub(crate) fn draw_diagrams(self: &Rc<Self>) {
        let editor = Rc::clone(self);
        diagrams::draw(&self.preview, move |drawn| {
            if drawn {
                editor.sync_preview();
            }
        });
    }

    /// Side by side with Sync scrolling on: scrolls the preview to the source line at the top
    /// of the editor (the end when the editor is at its end). See `follow_preview` for the
    /// other direction.
    pub(crate) fn sync_preview(self: &Rc<Self>) {
        if self.mode.get() != state::Mode::Split || !self.prefs.borrow().sync_scroll {
            return;
        }
        // The visible area in buffer coordinates; lines in it are laid out, so their heights
        // are exact (the adjustment's `upper` is only an estimate until the end is shown).
        let rect = self.view.visible_rect();
        let (end_y, end_h) = self.view.line_yrange(&self.buffer.end_iter());
        let script = if rect.y() <= 0 {
            "window.scrollTo(0, 0)".to_string()
        } else if end_y + end_h <= rect.y() + rect.height() {
            "window.scrollTo(0, document.documentElement.scrollHeight)".to_string()
        } else {
            let y = rect.y();
            let (iter, top) = self.view.line_at_y(y);
            let (_, height) = self.view.line_yrange(&iter);
            let part = f64::from(y - top) / f64::from(height.max(1));
            format!("{SYNC_JS}({})", f64::from(iter.line() + 1) + part)
        };
        let editor = Rc::clone(self);
        self.preview.evaluate_javascript(
            &format!("{script};scrollY"),
            Some(SCRIPT_WORLD),
            None,
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(y) = result {
                    editor.preview_y.set(y.to_double());
                }
            },
        );
    }

    /// Side by side with Sync scrolling on, after user input on the preview: checks its
    /// position every `SYNC_DELAY` until it stops moving, scrolling the editor to the source
    /// line at the top of the preview. Only the editor's scroll position changes, not its cursor.
    pub(crate) fn follow_preview(self: &Rc<Self>) {
        if self.mode.get() != state::Mode::Split
            || !self.prefs.borrow().sync_scroll
            || self.preview_poll.borrow().is_some()
        {
            return;
        }
        let editor = Rc::clone(self);
        let id = glib::timeout_add_local_once(SYNC_DELAY, move || {
            let e = Rc::clone(&editor);
            editor.preview.evaluate_javascript(
                PREVIEW_LINE_JS,
                Some(SCRIPT_WORLD),
                None,
                gio::Cancellable::NONE,
                move |result| {
                    let pos = result.ok().and_then(|v| parse_position(&v.to_str()));
                    let moved = pos.filter(|&(y, _)| y != e.preview_y.get());
                    // Still polling here, so the editor's scroll does not sync back.
                    if let Some((y, line)) = moved {
                        e.preview_y.set(y);
                        e.scroll_editor_to(line);
                    }
                    e.preview_poll.take();
                    if moved.is_some() {
                        e.follow_preview();
                    }
                },
            );
        });
        self.preview_poll.replace(Some(id));
    }

    /// Scrolls the editor so 1-based source line `line` (fractional) is at its top; 0 is the
    /// top, -1 the end.
    fn scroll_editor_to(&self, line: f64) {
        let Some(adj) = self.view.vadjustment() else {
            return;
        };
        let y = if line < 0.0 {
            adj.upper()
        } else if line < 1.0 {
            0.0
        } else {
            let iter = self
                .buffer
                .iter_at_line(line as i32 - 1)
                .unwrap_or_else(|| self.buffer.end_iter());
            let (top, height) = self.view.line_yrange(&iter);
            f64::from(top) + line.fract() * f64::from(height)
        };
        adj.set_value(y);
    }

    /// Loads the buffer as a new page. Rendered off the main thread like `refresh_preview`
    /// (opening a large note or switching to the preview stays responsive); the old page
    /// stays shown meanwhile, and a newer refresh or load makes this result stale.
    pub(crate) fn load_preview(self: &Rc<Self>) {
        self.preview_generation
            .set(self.preview_generation.get().wrapping_add(1));
        let generation = self.preview_generation.get();
        let job = self.page_job(None);
        let uri = self.page_uri();
        let editor = Rc::clone(self);
        glib::spawn_future_local(async move {
            let Ok(html) = gio::spawn_blocking(job).await else {
                return;
            };
            if editor.preview_generation.get() == generation {
                editor.preview.load_html(&html, Some(&uri));
            }
        });
    }

    /// The preview page of the buffer, as a job that may run on another thread: everything
    /// it needs from the editor is copied now. `style` = (dark, theme), default the preview's.
    fn page_job(
        &self,
        style: Option<(bool, state::Theme)>,
    ) -> impl FnOnce() -> String + Send + 'static {
        let (start, end) = self.buffer.bounds();
        let text = self.buffer.text(&start, &end, true).to_string();
        let notes = self.note_list.borrow().to_vec(); // the Rc cannot cross threads
        let root = self.root.borrow().clone().unwrap_or_default();
        let current = self.path.borrow().clone();
        let (dark, theme) = style.unwrap_or_else(|| {
            (
                adw::StyleManager::default().is_dark(),
                self.prefs.borrow().theme,
            )
        });
        move || {
            let wiki =
                |t: &str| render::resolve_wiki(t, &notes, &root, current.as_deref()).cloned();
            render::page(&text, dark, theme, &wiki)
        }
    }
}

/// Export as HTML…: save dialog suggesting `<note>.html` next to the note, then writes
/// `render::export_page` of the editor's text (unsaved edits included). GTK asks before
/// overwriting an existing file.
pub(crate) fn export_html(editor: &Rc<Editor>) {
    let Some(note) = editor.path.borrow().clone() else {
        return;
    };
    let dialog = gtk::FileDialog::builder()
        .title("Export as HTML")
        .modal(true)
        .initial_name(format!("{}.html", stem(&note)))
        .build();
    if let Some(dir) = note.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    let editor = Rc::clone(editor);
    dialog.save(
        Some(&editor.window.clone()),
        gio::Cancellable::NONE,
        move |result| {
            // Err = the user cancelled; no path = a non-local location.
            let Some(dest) = result.ok().and_then(|f| f.path()) else {
                return;
            };
            let (start, end) = editor.buffer.bounds();
            let text = editor.buffer.text(&start, &end, true).to_string();
            let notes = editor.note_list.borrow().to_vec(); // the Rc cannot cross threads
            let root = editor.root.borrow().clone().unwrap_or_default();
            let uri = gio::File::for_path(&note).uri().to_string();
            // Rendered and written off the main thread: ~1s/MB of code would freeze the window.
            let job = move || {
                let wiki = |t: &str| render::resolve_wiki(t, &notes, &root, Some(&note)).cloned();
                let dir = note.parent().unwrap_or(Path::new("/"));
                render::export_page(&text, &stem(&note), dir, &wiki)
            };
            glib::spawn_future_local(async move {
                let Ok(html) = gio::spawn_blocking(job).await else {
                    return editor.toast("Could not render the note");
                };
                // Diagrams and formulas are drawn in an off-screen WebView and saved as static
                // SVG / KaTeX HTML; without a usable WebKit sandbox they stay code / MathML.
                if !(render::has_diagrams(&html) || render::has_math(&html)) || !sandbox_usable() {
                    return write_export(&editor, dest, html);
                }
                let weak = Rc::downgrade(&editor);
                let pending = std::cell::RefCell::new(Some((dest, html.clone())));
                offscreen(&editor, &html, &uri, move |view| {
                    let Some((dest, html)) = pending.take() else {
                        return;
                    };
                    let weak = weak.clone();
                    finished_page(view, move |drawn| {
                        if let Some(editor) = weak.upgrade() {
                            write_export(&editor, dest, drawn.unwrap_or(html));
                        }
                    });
                });
            });
        },
    );
}

/// Export as HTML: the page shown in `view` (after `offscreen` drew it) as a static file: no
/// script, diagrams as SVG without their code, formulas as KaTeX HTML with its style sheet
/// (fonts embedded) in the head. None if the page could not be read.
fn finished_page(view: &webkit6::WebView, done: impl FnOnce(Option<String>) + 'static) {
    let script = format!(
        "{}{}'<!doctype html>'+document.documentElement.outerHTML",
        diagrams::FINISH,
        math::FINISH
    );
    view.evaluate_javascript(
        &script,
        Some(SCRIPT_WORLD),
        None,
        gio::Cancellable::NONE,
        move |result| {
            let html = result
                .ok()
                .filter(|v| v.is_string())
                .map(|v| v.to_str().to_string());
            done(html)
        },
    );
}

/// Writes an exported HTML page off the main thread, then says how it went.
fn write_export(editor: &Rc<Editor>, dest: std::path::PathBuf, html: String) {
    let editor = Rc::clone(editor);
    glib::spawn_future_local(async move {
        match gio::spawn_blocking(move || save::write_atomic(&dest, &html).map(|()| dest)).await {
            Ok(Ok(dest)) => editor.toast(&format!("Exported {}", file_name(&dest))),
            Ok(Err(e)) => editor.toast(&format!("Could not export: {e}")),
            Err(_) => editor.toast("Could not export the note"),
        }
    });
}

/// Export as Plain Text…: save dialog suggesting `<note>.txt` next to the note, then writes
/// `plain::plain_text` of the editor's text (unsaved edits included).
pub(crate) fn export_text(editor: &Rc<Editor>) {
    let Some(note) = editor.path.borrow().clone() else {
        return;
    };
    let dialog = gtk::FileDialog::builder()
        .title("Export as Plain Text")
        .modal(true)
        .initial_name(format!("{}.txt", stem(&note)))
        .build();
    if let Some(dir) = note.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    let editor = Rc::clone(editor);
    dialog.save(
        Some(&editor.window.clone()),
        gio::Cancellable::NONE,
        move |result| {
            let Some(dest) = result.ok().and_then(|f| f.path()) else {
                return;
            };
            let (start, end) = editor.buffer.bounds();
            let text = editor.buffer.text(&start, &end, true).to_string();
            glib::spawn_future_local(async move {
                match gio::spawn_blocking(move || crate::plain::plain_text(&text)).await {
                    Ok(plain) => write_export(&editor, dest, plain),
                    Err(_) => editor.toast("Could not export the note"),
                }
            });
        },
    );
}

/// Copy as HTML: the selection (or the whole note, unsaved edits included) as
/// `render::html_fragment` (`text/html`) plus its Markdown (`text/plain`) on the clipboard.
pub(crate) fn copy_as_html(editor: &Rc<Editor>) {
    let Some(note) = editor.path.borrow().clone() else {
        return;
    };
    let (start, end) = editor
        .buffer
        .selection_bounds()
        .unwrap_or_else(|| editor.buffer.bounds());
    let text = editor.buffer.text(&start, &end, true).to_string();
    let notes = editor.note_list.borrow().to_vec(); // the Rc cannot cross threads
    let root = editor.root.borrow().clone().unwrap_or_default();
    let job = move || {
        let wiki = |t: &str| render::resolve_wiki(t, &notes, &root, Some(&note)).cloned();
        let dir = note.parent().unwrap_or(Path::new("/"));
        (render::html_fragment(&text, dir, &wiki), text)
    };
    let editor = Rc::clone(editor);
    glib::spawn_future_local(async move {
        let Ok((html, text)) = gio::spawn_blocking(job).await else {
            editor.toast("Could not render the note");
            return;
        };
        let provider = gtk::gdk::ContentProvider::new_union(&[
            gtk::gdk::ContentProvider::for_bytes("text/html", &glib::Bytes::from_owned(html)),
            gtk::gdk::ContentProvider::for_bytes(
                "text/plain;charset=utf-8",
                &glib::Bytes::from_owned(text),
            ),
        ]);
        match editor.window.clipboard().set_content(Some(&provider)) {
            Ok(()) => editor.toast("Copied as HTML"),
            Err(e) => editor.toast(&format!("Could not copy: {e}")),
        }
    });
}

/// Print…: the note as the print page (`print_page`), then GTK's print dialog (which has
/// Print to File → PDF). Nothing without an open note.
pub(crate) fn print_note(editor: &Rc<Editor>) {
    let window = editor.window.clone();
    print_page(editor, move |view| {
        webkit6::PrintOperation::new(view).run_dialog(Some(&window));
    });
}

/// Export as PDF…: asks where to save (`<note>.pdf` next to the note), then prints the print
/// page to that file through GTK's Print to File printer, without the print dialog.
pub(crate) fn export_pdf(editor: &Rc<Editor>) {
    let Some(note) = editor.path.borrow().clone() else {
        return;
    };
    if !sandbox_usable() {
        editor.toast(NO_SANDBOX);
        return;
    }
    let dialog = gtk::FileDialog::builder()
        .title("Export as PDF")
        .modal(true)
        .initial_name(format!("{}.pdf", stem(&note)))
        .build();
    if let Some(dir) = note.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    let editor = Rc::clone(editor);
    dialog.save(
        Some(&editor.window.clone()),
        gio::Cancellable::NONE,
        move |result| {
            // Err = the user cancelled; no path = a non-local location.
            let Some(dest) = result.ok().and_then(|f| f.path()) else {
                return;
            };
            let weak = Rc::downgrade(&editor);
            print_page(&editor, move |view| {
                let settings = gtk::PrintSettings::new();
                // GTK's file printer has a translated name (domain gtk40).
                settings.set_printer(&glib::dgettext(Some("gtk40"), "Print to File"));
                settings.set(gtk::PRINT_SETTINGS_OUTPUT_FILE_FORMAT, Some("pdf"));
                settings.set(
                    gtk::PRINT_SETTINGS_OUTPUT_URI,
                    Some(&gio::File::for_path(&dest).uri()),
                );
                let op = webkit6::PrintOperation::new(view);
                op.set_print_settings(&settings);
                let (w, name) = (weak.clone(), file_name(&dest));
                op.connect_finished(move |_| {
                    if let Some(e) = w.upgrade() {
                        e.toast(&format!("Exported {name}"));
                    }
                });
                let w = weak.clone();
                op.connect_failed(move |_, err| {
                    if let Some(e) = w.upgrade() {
                        e.toast(&format!("Could not export: {err}"));
                    }
                });
                op.print();
            });
        },
    );
}

/// Renders the editor's text (unsaved edits included) as the preview page, always light, in an
/// off-screen WebView, so printing works the same from Edit, Preview and Side by side; `done`
/// runs once it is loaded. The HTML is built off the main thread (a large note with much code
/// takes ~1s); a Print/Export PDF asked for meanwhile is ignored. Nothing without an open note
/// or a usable WebKit sandbox.
fn print_page(editor: &Rc<Editor>, done: impl Fn(&webkit6::WebView) + 'static) {
    if editor.path.borrow().is_none() {
        return;
    }
    if !sandbox_usable() {
        editor.toast(NO_SANDBOX);
        return;
    }
    if editor.print_busy.replace(true) {
        editor.toast("Preparing…");
        return;
    }
    let job = editor.page_job(Some((false, state::Theme::Default)));
    let uri = editor.page_uri();
    let editor = Rc::clone(editor);
    glib::spawn_future_local(async move {
        let html = gio::spawn_blocking(job).await;
        editor.print_busy.set(false);
        let Ok(html) = html else {
            editor.toast("Could not render the note");
            return;
        };
        offscreen(&editor, &html, &uri, done);
    });
}

/// Loads `html` in an off-screen WebView kept in `editor.print_view` and runs `done` once it
/// is loaded and its formulas and diagrams are drawn (KaTeX's fonts loaded). Without a formula
/// or diagram no script runs at all; with one, only the app's script world runs (as in the
/// preview: page scripts off).
fn offscreen(editor: &Editor, html: &str, uri: &str, done: impl Fn(&webkit6::WebView) + 'static) {
    let scripts = render::has_diagrams(html) || render::has_math(html);
    let view = webkit6::WebView::new();
    if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_enable_javascript(scripts);
        settings.set_enable_javascript_markup(false);
    }
    if scripts {
        diagrams::keep_world(&view);
    }
    let done = Rc::new(done);
    view.connect_load_changed(move |view, event| {
        if event != webkit6::LoadEvent::Finished {
            return;
        }
        if !scripts {
            return done(view);
        }
        let (view, done) = (view.clone(), Rc::clone(&done));
        let target = view.clone();
        math::draw(&target, move |_| {
            let target = view.clone();
            diagrams::draw(&target, move |_| done(&view));
        });
    });
    view.load_html(html, Some(uri));
    editor.print_view.replace(Some(view));
}

const NO_SANDBOX: &str =
    "Preview unavailable: the system blocks WebKit's sandbox. See README, Troubleshooting.";

/// Whether WebKit can start its web process. WebKitGTK 6 always runs it in a bubblewrap
/// sandbox and aborts the whole app (`g_error`, "Failed to fully launch dbus-proxy") when
/// bwrap cannot create user namespaces: Ubuntu 24.04 blocks them for apps without an AppArmor
/// profile allowing `userns` (packaging/markdownmate.apparmor), so does a container. Probed
/// once, with the namespaces and /proc mount WebKit uses, before the first page load.
fn sandbox_usable() -> bool {
    static USABLE: OnceLock<bool> = OnceLock::new();
    *USABLE.get_or_init(|| {
        if std::env::var_os("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS").is_some() {
            return true;
        }
        let probe = Command::new("bwrap")
            .args(["--unshare-all", "--die-with-parent", "--ro-bind", "/", "/"])
            .args(["--proc", "/proc", "--dev", "/dev", "true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status();
        // No bwrap to run: not our call, WebKit decides (on Ubuntu it depends on bubblewrap).
        let usable = probe.as_ref().map_or(true, |s| s.success());
        if !usable {
            eprintln!("markdownmate: bubblewrap cannot create a sandbox, preview disabled");
        }
        usable
    })
}

pub(crate) fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The preview's in-place background render: at most one in flight; requests meanwhile are
/// coalesced into a single re-render once it finishes (no queue).
#[derive(Clone, Copy, Default)]
pub(crate) struct RenderSlot {
    busy: bool,
    again: bool,
}

impl RenderSlot {
    /// `true`: start a render now. `false`: one is in flight; it asks for a re-render when done.
    fn start(&mut self) -> bool {
        if self.busy {
            self.again = true;
            false
        } else {
            self.busy = true;
            true
        }
    }

    /// The render finished: `true` if requests came meanwhile, so one more must run.
    fn finish(&mut self) -> bool {
        self.busy = false;
        std::mem::take(&mut self.again)
    }
}

/// `PREVIEW_LINE_JS`'s result: `(scrollY, line)`.
fn parse_position(s: &str) -> Option<(f64, f64)> {
    let (y, line) = s.split_once(' ')?;
    Some((y.parse().ok()?, line.parse().ok()?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn render_slot_coalesces() {
        let mut slot = super::RenderSlot::default();
        assert!(slot.start());
        assert!(!slot.start());
        assert!(!slot.start());
        assert!(slot.finish()); // three requests in flight → exactly one re-render
        assert!(slot.start());
        assert!(!slot.finish()); // nothing came meanwhile
        assert!(slot.start());
    }

    #[test]
    fn parse_position() {
        assert_eq!(super::parse_position("120.5 7.25"), Some((120.5, 7.25)));
        assert_eq!(super::parse_position("0 -1"), Some((0.0, -1.0)));
        assert_eq!(super::parse_position("undefined"), None);
    }
}
