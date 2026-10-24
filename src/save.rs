//! Reading and safely saving a note. Pure Rust (no GTK) so it is testable headless.
//!
//! - `read_note` refuses non-UTF-8 files, so they are never opened for editing (and never
//!   overwritten).
//! - `save_checked` writes only when the text differs from what was last saved, and never
//!   overwrites a note that changed (or vanished) on disk since it was last read/written: the
//!   editor's text goes to a new `<stem> (conflict).<ext>` file next to it instead.
//! - `write_atomic` writes a temp file in the same folder, syncs it, then renames it over the
//!   note, so a failure midway leaves the old file intact. The original file's permissions are
//!   kept. A symlinked note is resolved first: its target is updated and the link stays a link.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The note's text; `InvalidData` if it is not valid UTF-8.
pub fn read_note(path: &Path) -> io::Result<String> {
    String::from_utf8(fs::read(path)?)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not a UTF-8 text file"))
}

/// Result of `save_checked`.
#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
    /// The text equals what was last saved: nothing written.
    Unchanged,
    /// Written to the note.
    Written,
    /// The note on disk is no longer what was last read/written (edited elsewhere, deleted,
    /// renamed, unreadable): it is left alone and the text was written to this new file.
    Conflict(PathBuf),
}

/// Saves `text` to `path` unless it equals `saved` (the text last read from or written to
/// `path`). Compares content, so the app's own last write never counts as a conflict.
pub fn save_checked(path: &Path, saved: &str, text: &str) -> io::Result<Saved> {
    if text == saved {
        return Ok(Saved::Unchanged);
    }
    // shortcut: another app can still write between this read and the rename (ms window);
    // closing it needs file locking, which other editors ignore anyway.
    match read_note(path) {
        Ok(disk) if disk == saved => {
            write_atomic(path, text)?;
            Ok(Saved::Written)
        }
        _ => {
            let copy = conflict_path(path);
            write_atomic(&copy, text)?;
            Ok(Saved::Conflict(copy))
        }
    }
}

/// First free `<stem> (conflict).<ext>`, `<stem> (conflict 2).<ext>`, … next to `path`.
fn conflict_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (1..)
        .map(|n| {
            let tag = if n == 1 {
                String::new()
            } else {
                format!(" {n}")
            };
            path.with_file_name(format!("{stem} (conflict{tag}){ext}"))
        })
        .find(|p| fs::symlink_metadata(p).is_err())
        .expect("unbounded range")
}

/// Replaces `path`'s contents with `text` via temp file + rename in the same folder.
pub fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    // Follow a symlinked note so the rename replaces the target, not the link.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let perms = fs::metadata(&target).ok().map(|m| m.permissions());
    let tmp = temp_path(&target)?;
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(text.as_bytes())?;
        if let Some(perms) = perms {
            file.set_permissions(perms)?;
        }
        file.sync_all()?;
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// `.<name>.<pid>.mmtmp` next to `target`: hidden, so the sidebar scan skips it.
fn temp_path(target: &Path) -> io::Result<PathBuf> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "note path has no file name"))?;
    let tmp_name = format!(".{}.{}.mmtmp", name.to_string_lossy(), std::process::id());
    Ok(target.with_file_name(tmp_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh empty folder under the system temp dir, removed on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mdmate-save-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn entries(&self) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(&self.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_temp_file() {
        let dir = TempDir::new("atomic");
        let note = dir.0.join("a.md");
        fs::write(&note, "old text that is longer").unwrap();
        write_atomic(&note, "new ✓").unwrap();
        assert_eq!(read_note(&note).unwrap(), "new ✓");
        assert_eq!(dir.entries(), ["a.md"]);
    }

    #[test]
    fn write_atomic_creates_missing_file() {
        let dir = TempDir::new("create");
        let note = dir.0.join("new.md");
        write_atomic(&note, "hi").unwrap();
        assert_eq!(fs::read_to_string(&note).unwrap(), "hi");
        assert_eq!(dir.entries(), ["new.md"]);
    }

    #[test]
    fn write_atomic_fails_cleanly_when_folder_missing() {
        let dir = TempDir::new("missing");
        assert!(write_atomic(&dir.0.join("gone/a.md"), "x").is_err());
        assert!(dir.entries().is_empty());
    }

    #[test]
    fn save_skips_when_unchanged() {
        let dir = TempDir::new("skip");
        let note = dir.0.join("a.md");
        fs::write(&note, "on disk").unwrap();
        // Same text as last saved: nothing written, disk not even compared.
        assert_eq!(
            save_checked(&note, "same", "same").unwrap(),
            Saved::Unchanged
        );
        assert_eq!(fs::read_to_string(&note).unwrap(), "on disk");
    }

    #[test]
    fn save_after_own_write_is_not_a_conflict() {
        let dir = TempDir::new("own");
        let note = dir.0.join("a.md");
        fs::write(&note, "v1").unwrap();
        assert_eq!(save_checked(&note, "v1", "v2").unwrap(), Saved::Written);
        // `saved` is now what the app just wrote.
        assert_eq!(save_checked(&note, "v2", "v3").unwrap(), Saved::Written);
        assert_eq!(fs::read_to_string(&note).unwrap(), "v3");
        assert_eq!(dir.entries(), ["a.md"]);
    }

    #[test]
    fn save_keeps_external_change_and_writes_conflict_copy() {
        let dir = TempDir::new("conflict");
        let note = dir.0.join("a.md");
        fs::write(&note, "theirs").unwrap();
        let copy = dir.0.join("a (conflict).md");
        assert_eq!(
            save_checked(&note, "read earlier", "mine").unwrap(),
            Saved::Conflict(copy.clone())
        );
        assert_eq!(fs::read_to_string(&note).unwrap(), "theirs");
        assert_eq!(fs::read_to_string(&copy).unwrap(), "mine");
        // A second conflict does not overwrite the first copy.
        let copy2 = dir.0.join("a (conflict 2).md");
        assert_eq!(
            save_checked(&note, "read earlier", "mine again").unwrap(),
            Saved::Conflict(copy2.clone())
        );
        assert_eq!(fs::read_to_string(&copy).unwrap(), "mine");
        assert_eq!(fs::read_to_string(&copy2).unwrap(), "mine again");
    }

    #[test]
    fn save_does_not_recreate_deleted_note() {
        let dir = TempDir::new("deleted");
        let note = dir.0.join("a.md");
        assert_eq!(
            save_checked(&note, "old", "new").unwrap(),
            Saved::Conflict(dir.0.join("a (conflict).md"))
        );
        assert_eq!(dir.entries(), ["a (conflict).md"]);
    }

    #[test]
    fn read_note_refuses_non_utf8() {
        let dir = TempDir::new("utf8");
        let note = dir.0.join("latin1.md");
        fs::write(&note, b"caf\xe9").unwrap();
        let err = read_note(&note).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&note).unwrap(), b"caf\xe9");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("perms");
        let note = dir.0.join("a.md");
        fs::write(&note, "x").unwrap();
        fs::set_permissions(&note, fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&note, "y").unwrap();
        assert_eq!(
            fs::metadata(&note).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_symlink() {
        let dir = TempDir::new("link");
        let real = dir.0.join("real.md");
        let link = dir.0.join("link.md");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, "new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(dir.entries(), ["link.md", "real.md"]);
    }
}
