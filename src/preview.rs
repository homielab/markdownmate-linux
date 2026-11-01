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
use crate::{diagrams, incremental, math, render, save, state};

impl Editor {
    /// The preview's page, made on first use: settings, zoom, find bar and signals
    /// (`window::wire_preview`). Nothing of WebKit runs before, so a session spent in Edit
    /// costs no WebKit process or memory.
    pub(crate) fn preview(self: &Rc<Self>) -> &webkit6::WebView {
        if let Some(view) = self.preview_view.get() {
            return view;
        }
        let view = self.preview_view.get_or_init(web_view);
        if let Some(settings) = WebViewExt::settings(view) {
            // No scripts from the page (the CSP blocks them too). JavaScript itself stays on
            // only for the app's own scripts in its script world: reading/restoring the scroll
            // position (evaluate_javascript needs it), drawing Mermaid diagrams and KaTeX
            // formulas (`diagrams`, `math`).
            settings.set_enable_javascript_markup(false);
        }
        diagrams::keep_world(view);
        view.set_vexpand(true);
        view.set_zoom_level(f64::from(self.prefs.borrow().zoom) / 100.0);
        let pane = self
            .panes
            .end_child()
            .and_downcast::<gtk::Box>()
            .expect("preview pane");
        pane.append(view);
        self.preview_find
            .set(crate::previewfind::find_bar(view, &pane))
            .ok();
        crate::window::wire_preview(self, view);
        view
    }

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
    /// note updates the page in place (no reload: no jump to the top, the scroll position
    /// stays): only the top-level blocks that changed are rendered and replaced
    /// (`incremental`), or the head and body are swapped when the note must render whole;
    /// another note loads a new page. The page's own scripts stay off
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
            .preview()
            .uri()
            .is_some_and(|u| u.split('#').next() == Some(uri.as_str()));
        if !same_note || self.preview().is_loading() {
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
        // Shown even if the text changed meanwhile (a step behind is still closer to it); the
        // refresh asked for meanwhile follows. Only a page loaded meanwhile drops it.
        let loads = self.preview_loads.get();
        let job = self.preview_job(self.preview_state.take());
        let editor = Rc::clone(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(job).await;
            let mut slot = editor.preview_render.get();
            let again = slot.finish();
            editor.preview_render.set(slot);
            if let Ok((state, update)) = result
                && editor.preview_loads.get() == loads
                && !editor.preview().is_loading()
            {
                editor.preview_state.replace(state);
                match update {
                    incremental::Update::Page(html) => editor.swap_page(&html),
                    incremental::Update::Patch(patch) => editor.patch_page(&patch),
                }
            }
            if again {
                editor.refresh_preview(); // with the latest text
            }
        });
    }

    /// Applies `patch` (`incremental::Patch`) to the shown page: its changed top-level nodes
    /// replaced, the source lines of the nodes after them moved, the scroll position kept.
    /// Formulas and diagrams of the replaced nodes are reused as in `swap_page`. A page that
    /// is not the one the patch was made for gets the whole page instead.
    fn patch_page(self: &Rc<Self>, patch: &incremental::Patch) {
        if patch.edits.is_empty() && patch.delta == 0 {
            return;
        }
        let script = patch_script(patch);
        let has_math = patch.has_math();
        let has_diagrams = patch.has_diagrams();
        let editor = Rc::clone(self);
        self.preview().evaluate_javascript(
            &script,
            Some(SCRIPT_WORLD),
            None,
            gio::Cancellable::NONE,
            move |result| match result {
                Ok(done) if done.to_boolean() => {
                    editor.refresh_preview_find();
                    editor.sync_preview();
                    if has_math {
                        editor.draw_math();
                    }
                    if has_diagrams {
                        editor.draw_diagrams();
                    }
                }
                // Not the page the patch was for: render it whole.
                Ok(_) => {
                    editor.preview_state.take();
                    editor.refresh_preview();
                }
                Err(_) => editor.load_preview(),
            },
        );
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
        self.preview().evaluate_javascript(
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
        math::draw(self.preview(), move |typeset| {
            if typeset {
                crate::timing::mark(|| "formulas typeset".into());
                editor.sync_preview();
            }
        });
    }

    /// Draws the preview's Mermaid diagrams (nothing is loaded for a page without one), then
    /// follows the editor again: the diagrams changed the page's heights.
    pub(crate) fn draw_diagrams(self: &Rc<Self>) {
        let editor = Rc::clone(self);
        diagrams::draw(self.preview(), move |drawn| {
            if drawn {
                crate::timing::mark(|| "diagrams drawn".into());
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
        self.preview().evaluate_javascript(
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
            editor.preview().evaluate_javascript(
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
        self.preview_loads
            .set(self.preview_loads.get().wrapping_add(1));
        let generation = self.preview_generation.get();
        let job = self.preview_job(None);
        let uri = self.page_uri();
        let editor = Rc::clone(self);
        glib::spawn_future_local(async move {
            let Ok((state, update)) = gio::spawn_blocking(job).await else {
                return;
            };
            let incremental::Update::Page(html) = update else {
                unreachable!("a first render is a page");
            };
            if editor.preview_generation.get() == generation {
                editor.preview_state.replace(state);
                editor.preview().load_html(&html, Some(&uri));
            }
        });
    }

    /// The open folder's links as the preview and exports resolve them, for the open note;
    /// copied, as the Rc lists cannot cross threads.
    pub(crate) fn links(&self) -> render::Folder {
        render::Folder {
            notes: self.note_list.borrow().to_vec(),
            images: self.image_list.borrow().to_vec(),
            aliases: self.aliases.borrow().clone(),
            root: self.root.borrow().clone(),
            current: self.path.borrow().clone(),
        }
    }

    /// The preview's render of the buffer building on `previous` (`incremental::render`), as
    /// a job that may run on another thread, like `page_job`.
    fn preview_job(
        &self,
        previous: Option<incremental::Preview>,
    ) -> impl FnOnce() -> (Option<incremental::Preview>, incremental::Update) + Send + 'static {
        let (start, end) = self.buffer.bounds();
        let text = self.buffer.text(&start, &end, true).to_string();
        let wiki = self.links();
        let links = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            let fingerprint = wiki.aliases.fingerprint();
            (
                &wiki.notes,
                &wiki.images,
                &wiki.root,
                &wiki.current,
                fingerprint,
            )
                .hash(&mut hasher);
            hasher.finish()
        };
        let style = incremental::Style {
            dark: adw::StyleManager::default().is_dark(),
            theme: self.prefs.borrow().theme,
            width: self.prefs.borrow().text_width,
            web_images: self.prefs.borrow().web_images,
            hard_breaks: render::hard_breaks(),
            smart: crate::smart::enabled(),
            links,
        };
        move || incremental::render(previous, &text, style, &wiki)
    }

    /// The preview page of the buffer, as a job that may run on another thread: everything
    /// it needs from the editor is copied now. `style` = (dark, theme), default the preview's.
    fn page_job(
        &self,
        style: Option<(bool, state::Theme)>,
    ) -> impl FnOnce() -> String + Send + 'static {
        let (start, end) = self.buffer.bounds();
        let text = self.buffer.text(&start, &end, true).to_string();
        let wiki = self.links();
        let (dark, theme) = style.unwrap_or_else(|| {
            (
                adw::StyleManager::default().is_dark(),
                self.prefs.borrow().theme,
            )
        });
        let web_images = self.prefs.borrow().web_images;
        move || render::page(&text, dark, theme, web_images, &wiki)
    }
}

/// The script `Editor::patch_page` runs: true when the page was patched, false (untouched)
/// when its body does not have the nodes the patch was made for.
fn patch_script(patch: &incremental::Patch) -> String {
    let edits: Vec<String> = patch
        .edits
        .iter()
        .map(|e| format!("[{},{},{}]", e.at, e.remove, render::js_string(&e.html)))
        .collect();
    format!(
        "(function(n,s,d,edits){{var b=document.body;if(!b||b.childNodes.length!==n)return false;\
var y=scrollY,kids=Array.from(b.childNodes),removed=[],added=[];\
if(d){{var line=function(v){{return String(Number(v)+d)}};\
for(var i=s;i<kids.length;i++){{var k=kids[i];if(k.nodeType!==1)continue;\
var els=Array.from(k.querySelectorAll('[data-sourcepos]'));if(k.hasAttribute('data-sourcepos'))els.push(k);\
for(var e of els)e.dataset.sourcepos=e.dataset.sourcepos.replace(/^(\\d+)(:\\d+-)(\\d+)/,\
function(m,a,c,z){{return line(a)+c+line(z)}});\
for(var a of k.querySelectorAll('a.task,a.copy'))a.setAttribute('href',\
a.getAttribute('href').replace(/\\d+$/,line))}}}}\
for(var e of edits)for(var i=e[0];i<e[0]+e[1];i++)removed.push(kids[i]);\
var mmOld=window.mmDiagrams?window.mmDiagrams.take(removed):null;\
var mmMathOld=window.mmMath?window.mmMath.take(removed):null;\
for(var e of edits){{var t=document.createElement('template');t.innerHTML=e[2];\
added.push(...t.content.childNodes);b.insertBefore(t.content,kids[e[0]]||null);\
for(var i=e[0];i<e[0]+e[1];i++)kids[i].remove()}}\
if(window.mmMath)window.mmMath.render(mmMathOld);\
if(window.mmDiagrams)window.mmDiagrams.apply(mmOld,added);\
scrollTo(0,y);return b.childNodes.length==={}}})({},{},{},[{}])",
        patch.after,
        patch.nodes,
        patch.shift,
        patch.delta,
        edits.join(",")
    )
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
            let wiki = render::Folder {
                current: Some(note.clone()),
                ..editor.links()
            };
            let uri = gio::File::for_path(&note).uri().to_string();
            // Rendered and written off the main thread: ~1s/MB of code would freeze the window.
            let job = move || {
                let dir = note.parent().unwrap_or(Path::new("/"));
                render::export_page(&text, &stem(&note), dir, &wiki)
            };
            glib::spawn_future_local(async move {
                let Ok(html) = gio::spawn_blocking(job).await else {
                    return editor.toast("Could not render the note");
                };
                let weak = Rc::downgrade(&editor);
                draw_static(&editor, html, &uri, move |html| {
                    if let Some(editor) = weak.upgrade() {
                        write_export(&editor, dest, html);
                    }
                });
            });
        },
    );
}

/// An exported page `html` with its diagrams and formulas drawn in an off-screen WebView
/// (`uri` its base, for relative images) and saved as static SVG / KaTeX HTML, given to
/// `done`. Without any, without a usable WebKit sandbox, or when the drawn page cannot be read,
/// `done` gets `html` as it is (diagrams as code, formulas as MathML).
pub(crate) fn draw_static(
    editor: &Rc<Editor>,
    html: String,
    uri: &str,
    done: impl FnOnce(String) + 'static,
) {
    if !(render::has_diagrams(&html) || render::has_math(&html)) || !sandbox_usable() {
        return done(html);
    }
    // The file lets web images load wherever it is opened; drawing it here loads them only
    // with Load Images from the Web on (the preview's CSP meanwhile).
    let web_images = editor.prefs.borrow().web_images;
    let shown = if web_images {
        html.clone()
    } else {
        html.replacen(render::EXPORT_CSP, render::PREVIEW_CSP, 1)
    };
    let weak = Rc::downgrade(editor);
    let pending = std::cell::RefCell::new(Some((html, done)));
    offscreen(editor, &shown, uri, move |view| {
        let Some((html, done)) = pending.take() else {
            return;
        };
        let (weak, page) = (weak.clone(), view.clone());
        finished_page(view, move |drawn| {
            if let Some(editor) = weak.upgrade() {
                drop_print_view(&editor, &page);
            }
            let drawn = drawn.map(|d| match web_images {
                true => d,
                false => d.replacen(render::PREVIEW_CSP, render::EXPORT_CSP, 1),
            });
            done(drawn.unwrap_or(html));
        });
    });
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
    export_converted(
        editor,
        "Export as Plain Text",
        "txt",
        crate::plain::plain_text,
    );
}

/// Export as RTF…: the same with `rtf::rtf`, suggesting `<note>.rtf`.
pub(crate) fn export_rtf(editor: &Rc<Editor>) {
    export_converted(editor, "Export as RTF", "rtf", crate::rtf::rtf);
}

/// Save dialog titled `title` suggesting `<note>.<ext>` next to the note, then writes
/// `convert` of the editor's text (unsaved edits included).
fn export_converted(editor: &Rc<Editor>, title: &str, ext: &str, convert: fn(&str) -> String) {
    let Some(note) = editor.path.borrow().clone() else {
        return;
    };
    let dialog = gtk::FileDialog::builder()
        .title(title)
        .modal(true)
        .initial_name(format!("{}.{ext}", stem(&note)))
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
                match gio::spawn_blocking(move || convert(&text)).await {
                    Ok(out) => write_export(&editor, dest, out),
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
    let text = copy_source(editor);
    let wiki = render::Folder {
        current: Some(note.clone()),
        ..editor.links()
    };
    copy_html_and_text(editor, "Copied as HTML", move || {
        let dir = note.parent().unwrap_or(Path::new("/"));
        (render::html_fragment(&text, dir, &wiki), text)
    });
}

/// Copy as Rich Text: the selection (or the whole note, unsaved edits included) as
/// `render::rich_text_page` (`text/html`, local images inside the open folder inlined) plus
/// `plain::plain_text` (`text/plain`, as Export as Plain Text) on the clipboard, so it pastes
/// formatted into LibreOffice, Thunderbird or web mail and without Markdown syntax elsewhere.
pub(crate) fn copy_as_rich_text(editor: &Rc<Editor>) {
    let Some(note) = editor.path.borrow().clone() else {
        return;
    };
    let text = copy_source(editor);
    let wiki = render::Folder {
        current: Some(note.clone()),
        ..editor.links()
    };
    let dir = note.parent().unwrap_or(Path::new("/")).to_path_buf();
    let root = wiki.root.clone().unwrap_or_else(|| dir.clone());
    let page_uri = editor.page_uri();
    copy_html_and_text(editor, "Copied as Rich Text", move || {
        // Same rule as Ctrl+click on an image: a file inside the open folder.
        let image = |src: &str| {
            let uri =
                glib::Uri::resolve_relative(Some(&page_uri), src, glib::UriFlags::NONE).ok()?;
            let path = gio::File::for_uri(&uri).path()?;
            crate::images::data_url(&crate::images::in_folder(&path, &root)?)
        };
        let html = render::rich_text_page(&text, &dir, &wiki, &image);
        (html, crate::plain::plain_text(&text))
    });
}

/// The editor's selection, or its whole text (unsaved edits included) when nothing is selected.
fn copy_source(editor: &Editor) -> String {
    let (start, end) = editor
        .buffer
        .selection_bounds()
        .unwrap_or_else(|| editor.buffer.bounds());
    editor.buffer.text(&start, &end, true).to_string()
}

/// Runs `job` off the main thread, then puts its `(html, text)` on the clipboard as
/// `text/html` and `text/plain` and toasts `done`.
fn copy_html_and_text(
    editor: &Rc<Editor>,
    done: &'static str,
    job: impl FnOnce() -> (String, String) + Send + 'static,
) {
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
            Ok(()) => editor.toast(done),
            Err(e) => editor.toast(&format!("Could not copy: {e}")),
        }
    });
}

/// Print…: the note as the print page (`print_page`), then GTK's print dialog (which has
/// Print to File → PDF). Nothing without an open note.
pub(crate) fn print_note(editor: &Rc<Editor>) {
    let window = editor.window.clone();
    let weak = Rc::downgrade(editor);
    print_page(editor, move |view| {
        let op = webkit6::PrintOperation::new(view);
        let (w, page) = (weak.clone(), view.clone());
        let done = move || {
            if let Some(editor) = w.upgrade() {
                drop_print_view(&editor, &page);
            }
        };
        let finished = done.clone();
        op.connect_finished(move |_| finished());
        let failed = done.clone();
        op.connect_failed(move |_, _| failed());
        // Returns once the dialog is closed; printing goes on until `finished`.
        if op.run_dialog(Some(&window)) == webkit6::PrintOperationResponse::Cancel {
            done();
        }
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
                let (w, name, page) = (weak.clone(), file_name(&dest), view.clone());
                op.connect_finished(move |_| {
                    if let Some(e) = w.upgrade() {
                        drop_print_view(&e, &page);
                        e.toast(&format!("Exported {name}"));
                    }
                });
                let (w, page) = (weak.clone(), view.clone());
                op.connect_failed(move |_, err| {
                    if let Some(e) = w.upgrade() {
                        drop_print_view(&e, &page);
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
    let view = web_view();
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

/// A new WebView. Its web context keeps the smallest caches (`DocumentViewer`): the app shows
/// one local page at a time and never goes back or forward, so WebKit's page and resource
/// caches only hold memory.
fn web_view() -> webkit6::WebView {
    if let Some(context) = webkit6::WebContext::default() {
        context.set_cache_model(webkit6::CacheModel::DocumentViewer);
    }
    webkit6::WebView::new()
}

/// Lets go of the off-screen page `view` once its print or export is done, unless a newer
/// one took its place: its web process (a whole WebKit renderer) ends with it. Dropped after
/// the signal that reported it, not inside it.
fn drop_print_view(editor: &Rc<Editor>, view: &webkit6::WebView) {
    let (editor, view) = (Rc::clone(editor), view.clone());
    glib::idle_add_local_once(move || {
        let done = editor.print_view.borrow().as_ref() == Some(&view);
        if done {
            // Taken first: dropping the page may run its handlers.
            let page = editor.print_view.take();
            drop(page);
        }
    });
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
