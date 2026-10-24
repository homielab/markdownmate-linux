mod backlinks;
mod complete;
mod diagrams;
mod dialogs;
mod editor;
mod find;
mod format;
mod goto;
mod images;
mod input;
mod links;
mod lists;
mod naming;
mod notes;
mod outline;
mod paste;
mod preview;
mod quick;
mod render;
mod save;
mod search;
mod sidebar;
mod state;
mod stats;
mod table;
mod tags;
mod title;
mod url;
mod window;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self};

use editor::Editor;
use window::build_window;

const APP_ID: &str = "com.homielab.MarkdownMate";
/// Auto-save after this long without typing.
const SAVE_DELAY: Duration = Duration::from_secs(1);
/// Quiet time before the status bar recounts words/characters.
const STATUS_DELAY: Duration = Duration::from_millis(200);
/// Folder changes are batched this long before the sidebar is rescanned.
const RESCAN_DELAY: Duration = Duration::from_millis(300);
/// Side by side: re-render the preview this long after typing stops.
const PREVIEW_DELAY: Duration = Duration::from_millis(300);
/// Outline panel: re-list headings this long after typing stops.
/// Name of the editor's text tag that colours `#tags`.
const TAG_STYLE: &str = "mm-tag";
/// TextTag of GFM table lines: monospace so aligned columns line up (#104).
const TABLE_STYLE: &str = "mm-table";
/// TextTag colouring the editor's formulas (`render::math_spans`).
const MATH_STYLE: &str = "mm-math";
const OUTLINE_DELAY: Duration = Duration::from_millis(300);
/// Script world for the app's scroll scripts, apart from the page.
const SCRIPT_WORLD: &str = "markdownmate";
/// Side by side: the preview follows editor scrolling at most this often.
const SYNC_DELAY: Duration = Duration::from_millis(50);
/// Scrolls the page to 1-based source line `L` (fractional: part of the line scrolled past),
/// interpolating within / between block elements' `data-sourcepos` (render::page).
/// Inline elements are skipped: they would cut a paragraph into uneven pieces.
const SYNC_JS: &str = "(function(l){var b=null,bp,n=null,np;\
for(var e of document.querySelectorAll('[data-sourcepos]:not(a,em,strong,code,del,span,img,sup)')){\
var p=e.dataset.sourcepos.split(/[:-]/).map(Number);\
if(p[0]<=l){if(!b||p[0]>=bp[0]){b=e;bp=p}}else if(!n||p[0]<np[0]){n=e;np=p}}\
var y=0;if(b){var q=b.getBoundingClientRect(),end=bp[2]+1;\
if(l<end)y=q.top+(l-bp[0])/(end-bp[0])*q.height;\
else if(n)y=q.bottom+(l-end)/Math.max(np[0]-end,1)*(n.getBoundingClientRect().top-q.bottom);\
else y=q.bottom;y+=window.scrollY}window.scrollTo(0,y)})";
/// The inverse of `SYNC_JS` (same selector): `"<scrollY> <L>"`, `L` the fractional 1-based
/// source line at the top of the page; 0 at the page's top, -1 at its end.
const PREVIEW_LINE_JS: &str = "(function(){var y=scrollY,b=null,bp,br,n=null,np,nr;\
if(y<=0)return y+' 0';if(y+innerHeight>=document.documentElement.scrollHeight-1)return y+' -1';\
for(var e of document.querySelectorAll('[data-sourcepos]:not(a,em,strong,code,del,span,img,sup)')){\
var r=e.getBoundingClientRect(),p=e.dataset.sourcepos.split(/[:-]/).map(Number);\
if(r.top<=0){if(!b||p[0]>=bp[0]){b=e;bp=p;br=r}}else if(!n||p[0]<np[0]){n=e;np=p;nr=r}}\
var l=1;if(b){var end=bp[2]+1;\
if(br.bottom>0)l=bp[0]-br.top/Math.max(br.height,1)*(end-bp[0]);\
else if(n)l=end-br.bottom/Math.max(nr.top-br.bottom,1)*(np[0]-end);else l=end}\
return y+' '+l})()";
const UNTITLED: &str = "Untitled.md";

fn main() -> gtk::glib::ExitCode {
    // HANDLES_OPEN: `markdownmate note.md` / Files ▸ Open With pass files to `open`.
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        // GNOME báo kết thúc phiên (logout) qua `query-end`.
        .register_session(true)
        .build();
    app.connect_query_end(save_all);
    app.connect_shutdown(save_all);
    // quit → `shutdown` lưu.
    for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
        glib::unix_signal_add_local(
            signal,
            glib::clone!(
                #[weak]
                app,
                #[upgrade_or]
                glib::ControlFlow::Break,
                move || {
                    app.quit();
                    glib::ControlFlow::Continue
                }
            ),
        );
    }
    app.connect_startup(|_| {
        // Icon cài từ gói .deb (hicolor); chạy chưa cài thì GTK dùng icon mặc định.
        gtk::Window::set_default_icon_name(APP_ID);
        add_scheme_paths();
        // Adwaita đặt min-height 36px cho row .navigation-sidebar: kết quả Find in Folder
        // chỉ một dòng sẽ nằm trên cùng và để trống bên dưới, nên bỏ min-height.
        let css = gtk::CssProvider::new();
        // Kéo thả trong sidebar: GTK tô cả list (:drop(active)); thay bằng tô đúng thư mục đích
        // (class drop-target do sidebar::drop_target đặt), cùng kiểu viền accent của Adwaita.
        css.load_from_data(
            ".find-results > row { min-height: 0; }
             .sidebar-tree:drop(active) { box-shadow: none; }
             .sidebar-tree.drop-target, .sidebar-tree > row.drop-target {
                 box-shadow: inset 0 0 0 1px @accent_bg_color; }",
        );
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &css,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    app.connect_activate(|app| build_window(app, None));
    app.connect_open(|app, files, _| {
        // `markdownmate://` links (a second instance forwards them here) go to the open window.
        let links: Vec<_> = files
            .iter()
            .filter(|f| f.has_uri_scheme("markdownmate"))
            .collect();
        if links.is_empty() {
            // shortcut: only the first path is used and each call opens a new window (like activate).
            return build_window(app, files.first().and_then(|f| f.path()));
        }
        if app.active_window().is_none() {
            build_window(app, None);
        }
        if let Some(window) = app.active_window() {
            for f in links {
                let _ = WidgetExt::activate_action(&window, "win.url", Some(&f.uri().to_variant()));
            }
        }
    });
    app.run()
}

/// Lưu note + phiên của mọi cửa sổ (`win.flush`), dùng khi app thoát không qua đóng cửa sổ.
fn save_all(app: &adw::Application) {
    for window in app.windows() {
        // Dialog không có action này: bỏ qua.
        let _ = WidgetExt::activate_action(&window, "win.flush", None);
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `$XDG_CONFIG_HOME/markdownmate/state` (usually `~/.config/markdownmate/state`).
fn state_file() -> PathBuf {
    glib::user_config_dir().join("markdownmate").join("state")
}

/// Cho GtkSourceView tìm scheme riêng của app (Paper, Graphite): bản cài từ .deb nằm cạnh
/// binary (`/usr/bin/../share/markdownmate/styles`); bản debug chạy từ repo (smoke) lấy thêm
/// trong cây mã nguồn. Thư mục không tồn tại thì bị bỏ qua.
fn add_scheme_paths() {
    let manager = sourceview5::StyleSchemeManager::default();
    if let Some(bin) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        manager.append_search_path(&bin.join("../share/markdownmate/styles").to_string_lossy());
    }
    // Chỉ bản debug: đường dẫn lúc build không được nằm trong binary phát hành.
    #[cfg(debug_assertions)]
    manager.append_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/packaging/styles"));
}

fn choose_folder(editor: &Rc<Editor>) {
    let dialog = gtk::FileDialog::builder()
        .title("Open Folder")
        .modal(true)
        .build();
    let editor = Rc::clone(editor);
    dialog.select_folder(
        Some(&editor.window.clone()),
        gio::Cancellable::NONE,
        move |result| {
            // Err = the user cancelled; no path = a non-local location.
            let Some(dir) = result.ok().and_then(|f| f.path()) else {
                return;
            };
            // Remembered once the scan has shown it.
            editor.load_folder(&dir);
        },
    );
}

/// Open File…: a Markdown note opens inside its own folder, like from the command line.
fn choose_file(editor: &Rc<Editor>) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Markdown"));
    filter.add_suffix("md");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder()
        .title("Open File")
        .modal(true)
        .filters(&filters)
        .build();
    let editor = Rc::clone(editor);
    dialog.open(
        Some(&editor.window.clone()),
        gio::Cancellable::NONE,
        move |result| {
            // Err = the user cancelled; no path = a non-local location.
            if let Some(path) = result.ok().and_then(|f| f.path()) {
                open_path(&editor, &path);
            }
        },
    );
}

/// Opens a folder, or a note inside its own folder.
fn open_path(editor: &Rc<Editor>, target: &Path) {
    if target.is_dir() {
        editor.load_folder(target);
    } else if let Some(dir) = target.parent() {
        editor.load_folder(dir);
        editor.open(target);
    }
}
