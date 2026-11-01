//! Export Folder as HTML… (main menu): every note of the open folder as a standalone page
//! (`render::folder_export_page`, as Export as HTML) at the same relative path in an empty
//! destination folder, with `.html` for its extension. Wiki links and relative links to the
//! folder's notes point at their pages (`#heading` kept), images shown by a relative path inside
//! the folder and every `assets/` folder are copied alongside once (never through a symlink
//! leading out of the folder), and `index.html` lists the notes as a nested tree in the
//! sidebar's order. The notes are the sidebar's (`notes::scan`: hidden files, `.git` and
//! `node_modules` skipped). The pages are rendered on background threads; the window shows the
//! progress and can cancel, keeping what was written (and an index of it). Pages with diagrams or formulas are then
//! drawn one at a time in an off-screen WebView (`preview::draw_static`), as Export as HTML
//! does.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

use crate::editor::Editor;
use crate::notes::{self, Node, SortOrder};
use crate::{file_name, preview, quick, render, state};

/// The tree page at the destination's root.
pub const INDEX: &str = "index.html";

/// Shared by the export threads and the window: notes done of `total`, and Cancel.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    pub cancel: AtomicBool,
}

/// What `export` did.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Notes in the folder.
    pub total: usize,
    /// Notes written as pages.
    pub written: usize,
    /// Images and `assets/` files copied.
    pub files: usize,
    /// Notes and images that could not be read or written, with the error.
    pub failed: Vec<(PathBuf, String)>,
    /// Written pages with diagrams or formulas, to draw on the main thread.
    pub draw: Vec<PathBuf>,
    /// Stopped by Cancel before every note was written or every `assets/` file copied
    /// (`index.html` then lists the pages written).
    pub cancelled: bool,
    /// The notes written, relative to the folder.
    listed: Vec<PathBuf>,
}

/// Whether `dest` can take an export: an existing empty folder.
pub fn check_destination(dest: &Path) -> Result<(), String> {
    let name = file_name(dest);
    match fs::read_dir(dest).map(|mut entries| entries.next().is_none()) {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!(
            "“{name}” is not empty: choose an empty or new folder"
        )),
        Err(e) => Err(format!("Cannot use “{name}”: {e}")),
    }
}

/// Writes every note of `root` (in `order`, the sidebar's) as a page in `dest` with its images,
/// on as many threads as there are cores (at most 8), then copies the `assets/` folders and
/// writes `index.html`. Only an unreadable `root` is an error; a note or file that fails is
/// listed in `Outcome::failed`.
pub fn export(
    root: &Path,
    dest: &Path,
    order: SortOrder,
    progress: &Progress,
) -> io::Result<Outcome> {
    let tree = notes::scan(root, order)?;
    let mut paths = Vec::new();
    quick::note_paths(&tree, &mut paths);
    let rels: Vec<PathBuf> = paths
        .iter()
        .map(|p| p.strip_prefix(root).unwrap_or(p).to_path_buf())
        .collect();
    let pages = page_paths(&rels);
    progress.total.store(paths.len(), Ordering::Relaxed);
    let wiki = render::WikiIndex::new(paths.clone(), root);
    let job = Job {
        root,
        real_root: root.canonicalize()?,
        dest,
        pages: &pages,
        wiki: &wiki,
        copied: Mutex::new(HashSet::new()),
    };
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8)
        .min(paths.len().max(1));
    let parts: Vec<Outcome> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut part = Outcome::default();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= paths.len() || progress.cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        job.note(&paths[i], &rels[i], &mut part);
                        progress.done.fetch_add(1, Ordering::Relaxed);
                    }
                    part
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().unwrap_or_default())
            .collect()
    });
    let mut outcome = Outcome {
        total: paths.len(),
        ..Outcome::default()
    };
    for part in parts {
        outcome.written += part.written;
        outcome.files += part.files;
        outcome.failed.extend(part.failed);
        outcome.draw.extend(part.draw);
        outcome.listed.extend(part.listed);
    }
    outcome.cancelled = progress.done.load(Ordering::Relaxed) < outcome.total;
    for file in assets(root) {
        if progress.cancel.load(Ordering::Relaxed) {
            outcome.cancelled = true;
            break;
        }
        job.copy(&file, &mut outcome);
    }
    let listed: HashMap<PathBuf, PathBuf> = outcome
        .listed
        .iter()
        .map(|rel| (rel.clone(), pages[rel].clone()))
        .collect();
    let title = file_name(root);
    let page = render::standalone_page(&title, &index_body(&title, &tree, root, &listed));
    if let Err(e) = fs::write(dest.join(INDEX), page) {
        outcome.failed.push((dest.join(INDEX), e.to_string()));
    }
    Ok(outcome)
}

/// The files of every `assets` folder (any case) of `root` and of the folders inside it,
/// relative to `root`: hidden names, `node_modules` and symlinked folders skipped as in
/// `notes::folders`.
fn assets(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in notes::folders(root) {
        let Ok(rel) = dir.strip_prefix(root) else {
            continue;
        };
        let in_assets = rel
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case("assets"));
        if !in_assets {
            continue;
        }
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let hidden = entry.file_name().as_bytes().starts_with(b".");
            // file_type does not follow symlinks: a symlinked folder is left out here, a
            // symlinked file is checked by `Job::copy`.
            if !hidden && entry.file_type().is_ok_and(|t| !t.is_dir()) {
                files.push(rel.join(entry.file_name()));
            }
        }
    }
    files
}

/// What every export thread shares.
struct Job<'a> {
    root: &'a Path,
    /// `root` with symlinks resolved: copied files must lie inside it.
    real_root: PathBuf,
    dest: &'a Path,
    pages: &'a HashMap<PathBuf, PathBuf>,
    wiki: &'a render::WikiIndex,
    /// Files already copied (or being copied), relative to `root`.
    copied: Mutex<HashSet<PathBuf>>,
}

impl Job<'_> {
    /// Writes `note` (`rel` inside the folder) as its page and copies its new images.
    fn note(&self, note: &Path, rel: &Path, part: &mut Outcome) {
        let bytes = match fs::read(note) {
            Ok(bytes) => bytes,
            Err(e) => return part.failed.push((note.to_path_buf(), e.to_string())),
        };
        let text = String::from_utf8_lossy(&bytes);
        let title = rel
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Absolute page paths in the folder's tree: `render` makes them relative to the note.
        let link = |target: &str| {
            let found = self.wiki.resolve(target, Some(note))?;
            let page = self.pages.get(found.strip_prefix(self.root).ok()?)?;
            Some(self.root.join(page))
        };
        let note_dir = note.parent().unwrap_or(self.root);
        let html = render::folder_export_page(&text, &title, note_dir, &link);
        let dir = rel.parent().unwrap_or(Path::new(""));
        let (html, images) = rewrite(&html, dir, &|t| self.pages.get(t).cloned());
        for image in images {
            self.copy(&image, part);
        }
        let out = self.dest.join(&self.pages[rel]);
        let written = out
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&out, &html));
        match written {
            Ok(()) => {
                part.written += 1;
                part.listed.push(rel.to_path_buf());
                if render::has_diagrams(&html) || render::has_math(&html) {
                    part.draw.push(out);
                }
            }
            Err(e) => part.failed.push((note.to_path_buf(), e.to_string())),
        }
    }
}

impl Job<'_> {
    /// Copies file `rel` of the folder to the same place in the destination, unless it was
    /// copied already.
    fn copy(&self, rel: &Path, part: &mut Outcome) {
        let new = self
            .copied
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(rel.to_path_buf());
        if new {
            match copy_file(self.root, &self.real_root, self.dest, rel) {
                Ok(true) => part.files += 1,
                Ok(false) => {}
                Err(e) => part.failed.push((self.dest.join(rel), e.to_string())),
            }
        }
    }
}

/// Copies file `rel` of `root` (`real_root` once symlinks are resolved) to the same place in
/// `dest`; false when it is missing (the page shows an image's alt text, as the preview does),
/// not a file, or a symlink leading out of the folder.
fn copy_file(root: &Path, real_root: &Path, dest: &Path, rel: &Path) -> io::Result<bool> {
    let Ok(from) = root.join(rel).canonicalize() else {
        return Ok(false);
    };
    if !from.starts_with(real_root) || !from.is_file() {
        return Ok(false);
    }
    let to = dest.join(rel);
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::copy(&from, &to).map(|_| true)
}

/// Each note's page (`notes` relative to the folder, in the sidebar's order), relative to the
/// destination: the note's path with `.html` for its extension. A name already taken by
/// `index.html` at the root or an earlier note (`a.md` and `a.markdown`), in any case (the
/// export may be copied to a case-insensitive disk), gets ` 2`, ` 3`, …
pub fn page_paths(notes: &[PathBuf]) -> HashMap<PathBuf, PathBuf> {
    let mut taken = HashSet::from([INDEX.to_string()]);
    notes
        .iter()
        .map(|note| {
            let stem = note.file_stem().unwrap_or_default();
            let dir = note.parent().unwrap_or(Path::new(""));
            let page = (1..)
                .map(|n| {
                    let mut name = OsString::from(stem);
                    if n > 1 {
                        name.push(format!(" {n}"));
                    }
                    name.push(".html");
                    dir.join(name)
                })
                .find(|page| taken.insert(page.to_string_lossy().to_lowercase()))
                .expect("some number is free");
            (note.clone(), page)
        })
        .collect()
}

/// A rendered page `html` of a note in folder `dir` (relative to the open folder) with each
/// relative link to a note that has a page (`page`, both relative to the folder) pointing at
/// that page instead (its `#heading` / `?query` kept, the folders as written), and the images it
/// shows by a relative path inside the folder (relative to the folder), each once, in order.
/// Absolute paths, URLs and paths leaving the folder are left alone.
pub fn rewrite(
    html: &str,
    dir: &Path,
    page: &dyn Fn(&Path) -> Option<PathBuf>,
) -> (String, Vec<PathBuf>) {
    const LINK: &str = "<a href=\"";
    const IMAGE: &str = "<img src=\"";
    let mut out = String::with_capacity(html.len());
    let mut images = Vec::new();
    let mut rest = html;
    // The note's own text is escaped, so these tags only come from Markdown links and images.
    loop {
        let link = rest.find(LINK);
        let image = rest.find(IMAGE);
        let (start, is_link) = match (link, image) {
            (Some(l), Some(i)) => (l.min(i), l < i),
            (Some(l), None) => (l, true),
            (None, Some(i)) => (i, false),
            (None, None) => break,
        };
        let value = start + if is_link { LINK.len() } else { IMAGE.len() };
        let Some(len) = rest[value..].find('"') else {
            break;
        };
        out.push_str(&rest[..value]);
        let raw = &rest[value..value + len];
        let url = unescape(raw);
        if is_link {
            match note_link(&url, dir, page) {
                Some(new) => comrak::html::escape(&mut out, &new).expect("writing to a String"),
                None => out.push_str(raw),
            }
        } else {
            let target = split_local(&url).and_then(|(path, _)| inside(dir, path));
            if let Some(target) = target.filter(|t| !images.contains(t)) {
                images.push(target);
            }
            out.push_str(raw);
        }
        rest = &rest[value + len..];
    }
    out.push_str(rest);
    (out, images)
}

/// `url` (a relative link of a note in `dir`) pointing at the page of the note it names, if
/// that note has one.
fn note_link(url: &str, dir: &Path, page: &dyn Fn(&Path) -> Option<PathBuf>) -> Option<String> {
    let (path, suffix) = split_local(url)?;
    let target = inside(dir, path)?;
    if !notes::is_note(&target) {
        return None;
    }
    let page = page(&target)?;
    let name = page.file_name()?;
    let folders = path.rfind('/').map_or("", |i| &path[..=i]);
    Some(format!(
        "{folders}{}{suffix}",
        render::percent_encode(name.as_bytes())
    ))
}

/// A relative URL split into its (percent-encoded) path and its `?query` / `#fragment`; None
/// for URLs with a scheme, absolute paths and fragment-only links.
fn split_local(url: &str) -> Option<(&str, &str)> {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    let (path, suffix) = url.split_at(end);
    let scheme = path.find(':').is_some_and(|c| !path[..c].contains('/'));
    (!path.is_empty() && !path.starts_with('/') && !scheme).then_some((path, suffix))
}

/// Percent-encoded relative `path` from folder `dir`, both inside the open folder, as a path
/// relative to the open folder; None if it leaves it or has a bad `%` escape.
fn inside(dir: &Path, path: &str) -> Option<PathBuf> {
    let path = state::unescape(path)?;
    let mut out = PathBuf::new();
    for part in dir.join(path).components() {
        match part {
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            Component::ParentDir if out.pop() => {}
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// An attribute value as comrak wrote it, entities decoded.
fn unescape(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// `index.html`'s body: `title`, then the notes of `tree` (the sidebar's tree of `root`) that
/// have a page in `pages` as nested lists in its order, a folder (with such a note inside) as an
/// open `<details>` with its name, a note as a link to its page named without its extension.
pub fn index_body(
    title: &str,
    tree: &[Node],
    root: &Path,
    pages: &HashMap<PathBuf, PathBuf>,
) -> String {
    let mut out = String::from("<h1>");
    comrak::html::escape(&mut out, title).expect("writing to a String");
    out.push_str("</h1>\n");
    index_list(tree, root, pages, &mut out);
    out
}

/// Appends the list of `tree`'s notes that have a page; false (nothing appended) when none has.
fn index_list(
    tree: &[Node],
    root: &Path,
    pages: &HashMap<PathBuf, PathBuf>,
    out: &mut String,
) -> bool {
    let mut items = String::new();
    for node in tree {
        match node {
            Node::Dir { name, children } => {
                let mut inner = String::new();
                if index_list(children, root, pages, &mut inner) {
                    items.push_str("<li><details open><summary>");
                    comrak::html::escape(&mut items, name).expect("writing to a String");
                    items.push_str("</summary>\n");
                    items.push_str(&inner);
                    items.push_str("</details></li>\n");
                }
            }
            Node::Note { name, path, .. } => {
                let Some(page) = path.strip_prefix(root).ok().and_then(|r| pages.get(r)) else {
                    continue;
                };
                let href = render::percent_encode(page.as_os_str().as_bytes());
                let stem = Path::new(name).file_stem().unwrap_or_default();
                items.push_str(&format!("<li><a href=\"{href}\">"));
                comrak::html::escape(&mut items, &stem.to_string_lossy())
                    .expect("writing to a String");
                items.push_str("</a></li>\n");
            }
        }
    }
    if items.is_empty() {
        return false;
    }
    out.push_str("<ul class=\"index\">\n");
    out.push_str(&items);
    out.push_str("</ul>\n");
    true
}

/// Main menu ▸ Export Folder as HTML…: picks an empty (or new) destination folder, then runs
/// `export` in the background behind a progress dialog with Cancel, draws the pages with
/// diagrams or formulas, and ends with a toast giving the count, with Show in Files.
pub(crate) fn export_folder(editor: &Rc<Editor>) {
    let Some(root) = editor.root.borrow().clone() else {
        return;
    };
    let dialog = gtk::FileDialog::builder()
        .title("Export Folder as HTML")
        .accept_label("Export")
        .modal(true)
        .build();
    if let Some(parent) = root.parent() {
        dialog.set_initial_folder(Some(&gio::File::for_path(parent)));
    }
    let editor = Rc::clone(editor);
    dialog.select_folder(
        Some(&editor.window.clone()),
        gio::Cancellable::NONE,
        move |result| {
            // Err = the user cancelled; no path = a non-local location.
            let Some(dest) = result.ok().and_then(|f| f.path()) else {
                return;
            };
            match check_destination(&dest) {
                Ok(()) => run(&editor, root, dest),
                Err(message) => editor.toast(&message),
            }
        },
    );
}

/// The progress dialog: a status line, a bar and Cancel.
struct Shown {
    dialog: adw::Dialog,
    status: gtk::Label,
    bar: gtk::ProgressBar,
}

impl Shown {
    fn new(editor: &Editor, progress: &Arc<Progress>) -> Shown {
        let status = gtk::Label::builder()
            .label("Reading the folder…")
            .xalign(0.0)
            .build();
        let bar = gtk::ProgressBar::builder().show_text(true).build();
        let cancel = gtk::Button::builder()
            .label("Cancel")
            .halign(gtk::Align::End)
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        content.append(&status);
        content.append(&bar);
        content.append(&cancel);
        let header = adw::HeaderBar::builder()
            .show_start_title_buttons(false)
            .show_end_title_buttons(false)
            .build();
        let view = adw::ToolbarView::new();
        view.add_top_bar(&header);
        view.set_content(Some(&content));
        let dialog = adw::Dialog::builder()
            .title("Export Folder as HTML")
            .content_width(420)
            .can_close(false)
            .child(&view)
            .build();
        let (p, s) = (Arc::clone(progress), status.clone());
        cancel.connect_clicked(move |button| {
            p.cancel.store(true, Ordering::Relaxed);
            button.set_sensitive(false);
            s.set_label("Cancelling…");
        });
        dialog.present(Some(&editor.window));
        Shown {
            dialog,
            status,
            bar,
        }
    }

    fn show(&self, done: usize, total: usize, what: &str) {
        self.bar.set_text(Some(&format!("{done} of {total}")));
        self.bar.set_fraction(if total == 0 {
            0.0
        } else {
            done as f64 / total as f64
        });
        self.status.set_label(what);
    }
}

fn run(editor: &Rc<Editor>, root: PathBuf, dest: PathBuf) {
    // Pages are rendered from the files: the open note's last edits first.
    editor.save();
    let order = editor.prefs.borrow().sidebar_sort;
    let progress = Arc::new(Progress::default());
    let shown = Rc::new(Shown::new(editor, &progress));
    let running = Rc::new(std::cell::Cell::new(true));
    let (p, s, r) = (
        Arc::clone(&progress),
        Rc::clone(&shown),
        Rc::clone(&running),
    );
    glib::timeout_add_local(Duration::from_millis(100), move || {
        if !r.get() {
            return glib::ControlFlow::Break;
        }
        let total = p.total.load(Ordering::Relaxed);
        if total > 0 && !p.cancel.load(Ordering::Relaxed) {
            s.show(p.done.load(Ordering::Relaxed), total, "Writing notes…");
        }
        glib::ControlFlow::Continue
    });
    let editor = Rc::clone(editor);
    glib::spawn_future_local(async move {
        let (r, d, p) = (root.clone(), dest.clone(), Arc::clone(&progress));
        let result = gio::spawn_blocking(move || export(&r, &d, order, &p)).await;
        running.set(false);
        let outcome = match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(e)) => {
                shown.dialog.force_close();
                return editor.toast(&format!("Could not read {}: {e}", file_name(&root)));
            }
            Err(_) => {
                shown.dialog.force_close();
                return editor.toast("Could not export the folder");
            }
        };
        for (path, error) in &outcome.failed {
            eprintln!("markdownmate: export {}: {error}", path.display());
        }
        let draw = Rc::new(Draw {
            editor: Rc::clone(&editor),
            dest,
            progress,
            shown,
            pages: std::cell::RefCell::new(outcome.draw.clone().into_iter()),
            drawn: std::cell::Cell::new(0),
            outcome,
        });
        if draw.outcome.cancelled {
            draw.finish();
        } else {
            draw.next();
        }
    });
}

/// The pages with diagrams or formulas, drawn one after the other (one off-screen WebView).
struct Draw {
    editor: Rc<Editor>,
    dest: PathBuf,
    progress: Arc<Progress>,
    shown: Rc<Shown>,
    outcome: Outcome,
    pages: std::cell::RefCell<std::vec::IntoIter<PathBuf>>,
    drawn: std::cell::Cell<usize>,
}

impl Draw {
    fn next(self: Rc<Self>) {
        let total = self.outcome.draw.len();
        if self.progress.cancel.load(Ordering::Relaxed) {
            return self.finish();
        }
        let Some(page) = self.pages.borrow_mut().next() else {
            return self.finish();
        };
        self.shown
            .show(self.drawn.get(), total, "Drawing diagrams and formulas…");
        let html = match fs::read_to_string(&page) {
            Ok(html) => html,
            Err(_) => return self.after(),
        };
        let uri = gio::File::for_path(&page).uri();
        let editor = Rc::clone(&self.editor);
        preview::draw_static(&editor, html, &uri, move |html| {
            if let Err(e) = fs::write(&page, html) {
                eprintln!("markdownmate: export {}: {e}", page.display());
            }
            self.after();
        });
    }

    /// One more page done: the next one from the main loop (no recursion when a page is not
    /// drawn and `draw_static` answers at once).
    fn after(self: Rc<Self>) {
        self.drawn.set(self.drawn.get() + 1);
        glib::idle_add_local_once(move || self.next());
    }

    fn finish(&self) {
        self.shown.dialog.force_close();
        let o = &self.outcome;
        let notes = |n: usize| if n == 1 { "note" } else { "notes" };
        let folder = file_name(&self.dest);
        let mut message = if o.cancelled {
            format!(
                "Export cancelled: {} of {} {} are in “{folder}”",
                o.written,
                o.total,
                notes(o.total)
            )
        } else {
            format!("Exported {} {} to “{folder}”", o.written, notes(o.written))
        };
        if !o.failed.is_empty() {
            message.push_str(&format!("; {} could not be written", o.failed.len()));
        }
        let toast = adw::Toast::builder()
            .title(message)
            .button_label("Show in Files")
            .timeout(10)
            .build();
        let (window, dest) = (self.editor.window.clone(), self.dest.clone());
        let weak = Rc::downgrade(&self.editor);
        toast.connect_button_clicked(move |_| {
            let weak = weak.clone();
            gtk::FileLauncher::new(Some(&gio::File::for_path(&dest))).launch(
                Some(&window),
                gio::Cancellable::NONE,
                move |result| {
                    if let (Err(err), Some(e)) = (result, weak.upgrade()) {
                        e.toast(&format!("Cannot open Files: {err}"));
                    }
                },
            );
        });
        self.editor.toasts.add_toast(toast);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh empty folder under the system temp dir, removed on drop (also when a test fails).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mm-site-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn write(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.0.join(rel)).unwrap()
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pages_of(notes: &[&str]) -> HashMap<PathBuf, PathBuf> {
        page_paths(&notes.iter().map(PathBuf::from).collect::<Vec<_>>())
    }

    #[test]
    fn page_paths_swap_the_extension_and_never_collide() {
        let pages = pages_of(&["a.md", "A.markdown", "index.md", "sub/index.md", "sub/b.MD"]);
        let page = |n: &str| pages[Path::new(n)].to_string_lossy().into_owned();
        assert_eq!(page("a.md"), "a.html");
        assert_eq!(page("A.markdown"), "A 2.html");
        assert_eq!(page("index.md"), "index 2.html");
        assert_eq!(page("sub/index.md"), "sub/index.html");
        assert_eq!(page("sub/b.MD"), "sub/b.html");
    }

    fn rewritten(html: &str, dir: &str) -> (String, Vec<PathBuf>) {
        let pages = pages_of(&["a.md", "sub/b.md", "sub/My Note.md", "a.markdown"]);
        rewrite(html, Path::new(dir), &|t| pages.get(t).cloned())
    }

    #[test]
    fn rewrites_relative_note_links_keeping_heading_and_folders() {
        let (html, _) = rewritten(
            "<p><a href=\"sub/b.md#part-two\">B</a> <a href=\"./a.md?x=1&amp;y=2#top\">A</a></p>",
            "",
        );
        assert_eq!(
            html,
            "<p><a href=\"sub/b.html#part-two\">B</a> <a href=\"./a.html?x=1&amp;y=2#top\">A</a></p>"
        );
        let (html, _) = rewritten(
            "<a href=\"../a.md\">A</a><a href=\"My%20Note.md\">N</a>",
            "sub",
        );
        assert_eq!(
            html,
            "<a href=\"../a.html\">A</a><a href=\"My%20Note.html\">N</a>"
        );
        // Taken names follow the page map: `a.markdown` is `a 2.html`.
        let (html, _) = rewritten("<a href=\"a.markdown\" title=\"t\">A</a>", "");
        assert_eq!(html, "<a href=\"a%202.html\" title=\"t\">A</a>");
    }

    #[test]
    fn leaves_other_links_alone() {
        for html in [
            "<a href=\"https://example.com/a.md\">w</a>",
            "<a href=\"mailto:x@y.z\">m</a>",
            "<a href=\"#heading\">h</a>",
            "<a href=\"/abs/a.md\">abs</a>",
            "<a href=\"../a.md\">outside</a>",
            "<a href=\"missing.md\">missing</a>",
            "<a href=\"report.pdf\">file</a>",
            "<a href=\"sub/b.html\">wiki page</a>",
            "<pre><code>&lt;a href=&quot;a.md&quot;&gt;</code></pre>",
        ] {
            assert_eq!(rewritten(html, "").0, html);
        }
    }

    #[test]
    fn lists_relative_images_inside_the_folder_once() {
        let (html, images) = rewritten(
            "<p><img src=\"../img/a%20b.png\" alt=\"x\" /><img src=\"https://e.com/w.png\" alt=\"\" />\
<img src=\"../../out.png\" alt=\"\" /><img src=\"/abs.png\" alt=\"\" /><img src=\"data:image/png;base64,AA\" alt=\"\" />\
<img src=\"./c.png\" alt=\"\" /><img src=\"../img/a%20b.png\" alt=\"again\" /></p>",
            "sub",
        );
        assert!(
            html.contains("<img src=\"../img/a%20b.png\""),
            "src unchanged"
        );
        assert_eq!(
            images,
            vec![PathBuf::from("img/a b.png"), PathBuf::from("sub/c.png")]
        );
    }

    fn tree_text(html: &str) -> String {
        html.lines()
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn index_is_a_nested_tree_in_sidebar_order() {
        let root = Path::new("/r");
        let note = |name: &str, rel: &str| Node::Note {
            name: name.to_string(),
            path: root.join(rel),
            date: None,
        };
        let tree = vec![
            Node::Dir {
                name: "Guides".into(),
                children: vec![note("Start <here>.md", "Guides/Start <here>.md")],
            },
            note("Note 2.md", "Note 2.md"),
            note("Note 10.markdown", "Note 10.markdown"),
        ];
        let pages = pages_of(&["Guides/Start <here>.md", "Note 2.md", "Note 10.markdown"]);
        assert_eq!(
            tree_text(&index_body("Handbook & Co", &tree, root, &pages)),
            "<h1>Handbook &amp; Co</h1>\n<ul class=\"index\">\n\
<li><details open><summary>Guides</summary>\n<ul class=\"index\">\n\
<li><a href=\"Guides/Start%20%3Chere%3E.html\">Start &lt;here&gt;</a></li>\n</ul>\n</details></li>\n\
<li><a href=\"Note%202.html\">Note 2</a></li>\n\
<li><a href=\"Note%2010.html\">Note 10</a></li>\n</ul>"
        );
    }

    #[test]
    fn destination_must_be_an_empty_folder() {
        let t = TempDir::new("dest");
        assert!(check_destination(&t.0).is_ok());
        t.write("x.txt", "x");
        assert!(check_destination(&t.0).is_err());
        assert!(check_destination(&t.0.join("missing")).is_err());
    }

    #[test]
    fn exports_a_folder_with_links_images_and_index() {
        let src = TempDir::new("src");
        let dest = TempDir::new("dest-full");
        src.write(
            "Home.md",
            "# Home\n\nSee [[Guide#Step One]], [the guide](Guides/Guide.md#step-one), \
[[Nowhere]] and ![logo](assets/logo.png).\n",
        );
        src.write(
            "Guides/Guide.md",
            "# Guide\n\n## Step One\n\nBack [[Home]]. ![logo](../assets/logo.png) ![gone](../gone.png)\n",
        );
        src.write("assets/logo.png", "png");
        src.write("assets/unused.png", "png");
        src.write("assets/docs/manual.pdf", "pdf");
        src.write("Guides/Assets/style.css", "css");
        src.write("assets/.DS_Store", "x");
        let outside = TempDir::new("outside");
        outside.write("secret.png", "secret");
        outside.write("dir/secret.txt", "secret");
        std::os::unix::fs::symlink(
            outside.0.join("secret.png"),
            src.0.join("assets/secret.png"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.0.join("dir"), src.0.join("assets/dir")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("secret.png"), src.0.join("shown.png")).unwrap();
        src.write("Linked.md", "![s](shown.png) ![s](assets/secret.png)");
        src.write(".hidden/Secret.md", "# Secret");
        src.write("node_modules/pkg/README.md", "# Readme");
        src.write(".git/HEAD.md", "x");
        let progress = Progress::default();
        let outcome = export(&src.0, &dest.0, SortOrder::Name, &progress).unwrap();
        // logo.png (shown and in assets/), unused.png, manual.pdf, style.css.
        assert_eq!((outcome.total, outcome.written, outcome.files), (3, 3, 4));
        assert!(outcome.failed.is_empty() && !outcome.cancelled);
        assert_eq!(progress.done.load(Ordering::Relaxed), 3);

        let home = dest.read("Home.html");
        assert!(
            home.contains("href=\"Guides/Guide.html#step-one\""),
            "{home}"
        );
        assert_eq!(home.matches("Guides/Guide.html#step-one").count(), 2);
        assert!(home.contains("Nowhere") && !home.contains("href=\"mm-missing"));
        assert!(home.contains("src=\"assets/logo.png\""));
        assert!(home.contains("<title>Home</title>"));
        let guide = dest.read("Guides/Guide.html");
        assert!(guide.contains("href=\"../Home.html\""), "{guide}");
        assert!(guide.contains("id=\"step-one\""));
        assert_eq!(dest.read("assets/logo.png"), "png");
        assert_eq!(dest.read("assets/unused.png"), "png");
        assert_eq!(dest.read("assets/docs/manual.pdf"), "pdf");
        assert_eq!(dest.read("Guides/Assets/style.css"), "css");
        assert!(!dest.0.join("gone.png").exists());
        for out_of_folder in [
            "assets/secret.png",
            "assets/dir",
            "shown.png",
            "assets/.DS_Store",
        ] {
            assert!(!dest.0.join(out_of_folder).exists(), "{out_of_folder}");
        }
        for skipped in [".hidden", "node_modules", ".git"] {
            assert!(!dest.0.join(skipped).exists(), "{skipped}");
        }
        let index = dest.read(INDEX);
        assert!(index.contains("<summary>Guides</summary>"));
        assert!(index.contains("<a href=\"Guides/Guide.html\">Guide</a>"));
        assert!(index.contains("<a href=\"Home.html\">Home</a>"));
        assert!(index.find("Guides/Guide.html") < index.find("Home.html"));
    }

    #[test]
    fn cancel_still_writes_an_index_of_the_pages_written() {
        let src = TempDir::new("src-cancel");
        let dest = TempDir::new("dest-cancel");
        for i in 0..20 {
            src.write(&format!("n{i}.md"), "# Note");
        }
        src.write("assets/a.png", "png");
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        let outcome = export(&src.0, &dest.0, SortOrder::Name, &progress).unwrap();
        assert!(outcome.cancelled);
        assert_eq!((outcome.total, outcome.written, outcome.files), (20, 0, 0));
        let index = dest.read(INDEX);
        assert!(index.contains("<h1>") && !index.contains("<ul"), "{index}");
    }

    #[test]
    fn index_lists_only_the_pages_written() {
        let src = TempDir::new("src-partial");
        let dest = TempDir::new("dest-partial");
        src.write("Kept.md", "# Kept");
        src.write("Lost/Lost.md", "# Lost");
        // A folder where Lost's page goes: that page cannot be written.
        fs::create_dir_all(dest.0.join("Lost/Lost.html")).unwrap();
        let outcome = export(&src.0, &dest.0, SortOrder::Name, &Progress::default()).unwrap();
        assert_eq!((outcome.written, outcome.failed.len()), (1, 1));
        let index = dest.read(INDEX);
        assert!(index.contains("<a href=\"Kept.html\">Kept</a>"), "{index}");
        assert!(!index.contains("Lost"), "{index}");
    }

    /// Times exporting a generated folder (`MM_SITE_NOTES`, e.g. 10,000 notes from
    /// `examples/generate_notes`): `cargo test --release site::tests::time_export -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn time_export() {
        let Some(src) = std::env::var_os("MM_SITE_NOTES").map(PathBuf::from) else {
            return;
        };
        let dest = TempDir::new("dest-time");
        let start = std::time::Instant::now();
        let outcome = export(&src, &dest.0, SortOrder::Name, &Progress::default()).unwrap();
        println!(
            "{} notes, {} files, {} to draw in {:?}",
            outcome.written,
            outcome.files,
            outcome.draw.len(),
            start.elapsed()
        );
        assert!(outcome.failed.is_empty());
    }
}
