use std::cell::{Cell, RefCell};
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use gtk::gio;
use gtk::gio::prelude::*;
use gtk::glib;

pub const MAX_BYTES: usize = 256 * 1024;
pub const STARTER: &str = include_str!("../resources/preview.css");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Off,
    Missing,
    Empty,
    TooLarge,
    Active(String),
}

/// Evaluates the status of custom CSS from the enabled flag and raw data.
pub fn status(enabled: bool, data: Option<&[u8]>) -> Status {
    if !enabled {
        return Status::Off;
    }
    let Some(data) = data else {
        return Status::Missing;
    };
    if data.is_empty() {
        return Status::Empty;
    }
    if data.len() > MAX_BYTES {
        return Status::TooLarge;
    }
    let text = String::from_utf8_lossy(data);
    if text.trim().is_empty() {
        Status::Empty
    } else {
        Status::Active(text.into_owned())
    }
}

/// The `<style id="custom-css">` element to inject into the page, or empty for non-active.
pub fn style_block(s: &Status) -> String {
    match s {
        Status::Active(css) => {
            format!(
                "<style id=\"custom-css\">{}</style>",
                css.replace("</", "<\\/")
            )
        }
        Status::Off | Status::Missing | Status::Empty | Status::TooLarge => String::new(),
    }
}

/// Path to `~/.config/markdownmate/preview.css`.
pub fn path() -> PathBuf {
    glib::user_config_dir()
        .join("markdownmate")
        .join("preview.css")
}

static BLOCK: Mutex<String> = Mutex::new(String::new());

/// The current `<style id="custom-css">` element injected into preview and exports.
pub fn block() -> String {
    BLOCK.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Reads the file if enabled (checking file size first to avoid large reads), stores the
/// current style block, and returns the status.
pub fn reload(enabled: bool) -> Status {
    let s = if !enabled {
        Status::Off
    } else {
        let p = path();
        match std::fs::metadata(&p) {
            Ok(m) => {
                if m.len() > MAX_BYTES as u64 {
                    Status::TooLarge
                } else {
                    let bytes = std::fs::read(&p).ok();
                    status(enabled, bytes.as_deref())
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Status::Missing,
            Err(_) => Status::Missing,
        }
    };
    let block = style_block(&s);
    *BLOCK.lock().unwrap_or_else(PoisonError::into_inner) = block;
    s
}

/// Creates the directory and writes the starter `preview.css` if missing; returns the path.
pub fn create_if_missing() -> io::Result<PathBuf> {
    let p = path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
    {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(STARTER.as_bytes())?;
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    Ok(p)
}

thread_local! {
    static MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
    static WATCH_STARTED: Cell<bool> = const { Cell::new(false) };
}

/// Starts watching `preview.css` for modifications and renames (only once).
pub fn watch(on_change: impl Fn() + 'static) {
    WATCH_STARTED.with(|started| {
        if started.get() {
            return;
        }
        started.set(true);

        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = gio::File::for_path(&p);
        if let Ok(m) = file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        {
            m.connect_changed(move |_, _, _, event| {
                use gio::FileMonitorEvent as E;
                if matches!(
                    event,
                    E::Changed
                        | E::ChangesDoneHint
                        | E::Created
                        | E::Deleted
                        | E::Renamed
                        | E::MovedIn
                ) {
                    on_change();
                }
            });
            MONITOR.with(|cell| {
                cell.replace(Some(m));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_matches_resource() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/preview.css");
        let bytes = std::fs::read(path).expect("reading preview.css resource");
        assert_eq!(bytes.len(), 878);
        assert_eq!(STARTER.as_bytes(), bytes.as_slice());
    }

    #[test]
    fn status_variants() {
        assert_eq!(status(false, None), Status::Off);
        assert_eq!(status(false, Some(b"body {}")), Status::Off);
        assert_eq!(status(true, None), Status::Missing);
        assert_eq!(status(true, Some(b"")), Status::Empty);
        assert_eq!(status(true, Some(b"   \n\t  \r\n")), Status::Empty);
        let too_large = vec![b'x'; MAX_BYTES + 1];
        assert_eq!(status(true, Some(&too_large)), Status::TooLarge);
        let exact = vec![b'x'; MAX_BYTES];
        assert_eq!(
            status(true, Some(&exact)),
            Status::Active(String::from_utf8(exact.clone()).unwrap())
        );
        assert_eq!(
            status(true, Some(b"body { font: serif; }")),
            Status::Active("body { font: serif; }".to_string())
        );
    }

    #[test]
    fn style_block_escaping() {
        assert_eq!(style_block(&Status::Off), "");
        assert_eq!(style_block(&Status::Missing), "");
        assert_eq!(style_block(&Status::Empty), "");
        assert_eq!(style_block(&Status::TooLarge), "");

        let s = Status::Active("body { color: red; }".to_string());
        assert_eq!(
            style_block(&s),
            "<style id=\"custom-css\">body { color: red; }</style>"
        );

        let script = Status::Active("</style><script>alert(1)</script>".to_string());
        assert_eq!(
            style_block(&script),
            "<style id=\"custom-css\"><\\/style><script>alert(1)<\\/script></style>"
        );
    }
}
