//! Today's Note (Ctrl+D): `<open folder>/<daily notes folder>/YYYY-MM-DD.md` for the local
//! date, created with `# YYYY-MM-DD` and a blank line when missing. The daily notes folder is a
//! preference (`Prefs::daily_folder`), relative to the open folder, `Daily` by default; empty
//! means the open folder itself. Pure Rust (no GTK) so it is testable headless.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The daily notes folder a new state file starts with.
pub const DEFAULT_FOLDER: &str = "Daily";

/// `YYYY-MM-DD` (Gregorian, Western digits).
pub fn date_string(year: i32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// The daily notes folder inside `root`: `setting` read as a relative path, with blank, `.`
/// and `..` parts and surrounding spaces dropped (so it cannot point outside `root`); nothing
/// left = `root` itself.
pub fn folder(root: &Path, setting: &str) -> PathBuf {
    setting
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .fold(root.to_path_buf(), |dir, part| dir.join(part))
}

/// Today's note for `date` (`date_string`): `<folder>/<date>.md`.
pub fn path(root: &Path, setting: &str, date: &str) -> PathBuf {
    folder(root, setting).join(format!("{date}.md"))
}

/// What a new daily note starts with: its date as a heading, then a blank line.
pub fn initial_text(date: &str) -> String {
    format!("# {date}\n\n")
}

/// Writes `text` to `path` (creating its folders) unless something is already there; true
/// when it created the file. Never overwrites (create_new); a folder of that name is an error.
pub fn create_if_missing(path: &Path, text: &str) -> io::Result<bool> {
    if path.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a folder has that name",
        ));
    }
    if path.exists() {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(text.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_is_zero_padded() {
        assert_eq!(date_string(2026, 3, 7), "2026-03-07");
        assert_eq!(date_string(2026, 12, 31), "2026-12-31");
        assert_eq!(date_string(987, 1, 1), "0987-01-01");
    }

    #[test]
    fn folder_stays_inside_the_root() {
        let root = Path::new("/notes");
        assert_eq!(folder(root, "Daily"), Path::new("/notes/Daily"));
        assert_eq!(folder(root, ""), root);
        assert_eq!(folder(root, " / . /"), root);
        assert_eq!(
            folder(root, " Journal / 2026 /"),
            Path::new("/notes/Journal/2026")
        );
        assert_eq!(folder(root, "../../etc"), Path::new("/notes/etc"));
        assert_eq!(folder(root, "/abs/olute"), Path::new("/notes/abs/olute"));
        assert_eq!(
            path(root, "Daily", "2026-10-25"),
            Path::new("/notes/Daily/2026-10-25.md")
        );
        assert_eq!(
            path(root, "", "2026-10-25"),
            Path::new("/notes/2026-10-25.md")
        );
    }

    #[test]
    fn creates_once_and_never_overwrites() {
        let root = std::env::temp_dir().join(format!("mm-daily-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let note = path(&root, "Daily/Sub", "2026-10-25");
        let text = initial_text("2026-10-25");
        assert_eq!(text, "# 2026-10-25\n\n");
        assert!(create_if_missing(&note, &text).unwrap());
        assert_eq!(fs::read_to_string(&note).unwrap(), text);
        fs::write(&note, "edited").unwrap();
        assert!(!create_if_missing(&note, &text).unwrap());
        assert_eq!(fs::read_to_string(&note).unwrap(), "edited");
        let taken = path(&root, "", "2026-10-26");
        fs::create_dir_all(&taken).unwrap();
        assert!(create_if_missing(&taken, &text).is_err());
        fs::remove_dir_all(&root).unwrap();
    }
}
