//! Today's Note (Ctrl+D): `<open folder>/<daily notes folder>/YYYY-MM-DD.md` for the local
//! date, created from `Template.md` in that folder (replacing `{{date}}` and `{{title}}`),
//! or `# YYYY-MM-DD` and a blank line when missing or unreadable. The daily notes folder is a
//! preference (`Prefs::daily_folder`), relative to the open folder, `Daily` by default; empty
//! means the open folder itself. Pure Rust (no GTK) so it is testable headless.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The daily notes folder a new state file starts with.
pub const DEFAULT_FOLDER: &str = "Daily";

/// The template file name in the daily notes folder.
pub const TEMPLATE_FILE: &str = "Template.md";

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

/// Reads `<folder>/Template.md` if it exists with that exact name and casing and can be read
/// as UTF-8.
pub fn load_template(folder: &Path) -> Option<String> {
    let entries = fs::read_dir(folder).ok()?;
    let mut exact_match = false;
    for entry in entries.flatten() {
        if entry.file_name() == TEMPLATE_FILE {
            exact_match = true;
            break;
        }
    }
    if !exact_match {
        return None;
    }
    let template_path = folder.join(TEMPLATE_FILE);
    if template_path.is_dir() {
        return None;
    }
    fs::read_to_string(&template_path).ok()
}

/// What a new daily note starts with: `template` with `{{date}}` and `{{title}}` replaced by
/// `date` (`templates::fill`, as New from Template), or `# <date>\n\n` when there is no
/// template.
pub fn initial_text_from_template(template: Option<&str>, date: &str) -> String {
    match template {
        Some(t) => crate::templates::fill(t, date, date),
        None => format!("# {date}\n\n"),
    }
}

/// Resolves what a new daily note in `daily_folder` starts with: reads `Template.md` if present
/// and readable, or falls back to `# <date>\n\n`.
pub fn initial_text(daily_folder: &Path, date: &str) -> String {
    initial_text_from_template(load_template(daily_folder).as_deref(), date)
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn make_temp_dir(tag: &str) -> (PathBuf, TempDir) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("mm-daily-{}-{}-{}", tag, std::process::id(), n));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        let guard = TempDir(path.clone());
        (path, guard)
    }

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
    fn initial_text_without_template() {
        assert_eq!(
            initial_text_from_template(None, "2026-10-25"),
            "# 2026-10-25\n\n"
        );

        let (folder, _guard) = make_temp_dir("no-tpl");
        assert_eq!(initial_text(&folder, "2026-10-25"), "# 2026-10-25\n\n");
        assert_eq!(
            initial_text(&folder.join("missing"), "2026-10-25"),
            "# 2026-10-25\n\n"
        );
    }

    #[test]
    fn template_with_placeholders() {
        let template = "# {{title}}\n\nCreated: {{date}}\nLog for {{date}}:\n";
        let rendered = initial_text_from_template(Some(template), "2026-10-25");
        assert_eq!(
            rendered,
            "# 2026-10-25\n\nCreated: 2026-10-25\nLog for 2026-10-25:\n"
        );

        let (folder, _guard) = make_temp_dir("with-placeholders");
        fs::write(folder.join(TEMPLATE_FILE), template).unwrap();
        assert_eq!(initial_text(&folder, "2026-10-25"), rendered);
    }

    #[test]
    fn template_without_placeholders() {
        let template = "# Journal\n\n## Plan\n\n## Notes\n";
        let rendered = initial_text_from_template(Some(template), "2026-10-25");
        assert_eq!(rendered, template);

        let (folder, _guard) = make_temp_dir("no-placeholders");
        fs::write(folder.join(TEMPLATE_FILE), template).unwrap();
        assert_eq!(initial_text(&folder, "2026-10-25"), template);
    }

    #[test]
    fn unreadable_template_falls_back_to_default() {
        let (folder, _guard) = make_temp_dir("unreadable");
        let tpl_path = folder.join(TEMPLATE_FILE);

        // Template.md is a directory.
        fs::create_dir_all(&tpl_path).unwrap();
        assert_eq!(initial_text(&folder, "2026-10-25"), "# 2026-10-25\n\n");
        fs::remove_dir(&tpl_path).unwrap();

        // Template.md contains non-UTF-8 bytes.
        fs::write(&tpl_path, [0xFF, 0xFE, 0xFD]).unwrap();
        assert_eq!(initial_text(&folder, "2026-10-25"), "# 2026-10-25\n\n");
    }

    #[test]
    fn template_case_sensitivity() {
        let (folder, _guard) = make_temp_dir("case");
        fs::write(folder.join("template.md"), "# Lowercase\n").unwrap();

        let entries: Vec<String> = fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        if entries.iter().any(|e| e == "template.md") && !entries.iter().any(|e| e == "Template.md")
        {
            assert_eq!(initial_text(&folder, "2026-10-25"), "# 2026-10-25\n\n");
        }
    }

    #[test]
    fn creates_once_and_never_overwrites() {
        let (root, _guard) = make_temp_dir("creates-once");
        let note = path(&root, "Daily/Sub", "2026-10-25");
        let text = initial_text(&root.join("Daily/Sub"), "2026-10-25");
        assert_eq!(text, "# 2026-10-25\n\n");
        assert!(create_if_missing(&note, &text).unwrap());
        assert_eq!(fs::read_to_string(&note).unwrap(), text);
        fs::write(&note, "edited").unwrap();
        assert!(!create_if_missing(&note, &text).unwrap());
        assert_eq!(fs::read_to_string(&note).unwrap(), "edited");
        let taken = path(&root, "", "2026-10-26");
        fs::create_dir_all(&taken).unwrap();
        assert!(create_if_missing(&taken, &text).is_err());
    }
}
