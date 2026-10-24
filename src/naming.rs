//! Names for new and renamed notes/folders, and the file operations that must never overwrite
//! an existing entry. Pure Rust (no GTK) so it is testable headless.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A name the user typed, trimmed, or why it cannot be used. A leading `.` is refused because
/// the sidebar hides such entries (the note would vanish).
pub fn check(name: &str) -> Result<&str, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        Err("The name is empty.")
    } else if name.contains(['/', '\0']) {
        Err("A name cannot contain “/”.")
    } else if name.starts_with('.') {
        Err("A name cannot start with “.”.")
    } else {
        Ok(name)
    }
}

/// `check` + `.md` appended unless the name already ends in `.md`/`.markdown`.
pub fn note_name(name: &str) -> Result<String, &'static str> {
    let name = check(name)?;
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".markdown") {
        Ok(name.to_string())
    } else {
        Ok(format!("{name}.md"))
    }
}

/// "Untitled.md", then "Untitled 2.md", "Untitled 3.md", … — `n` starts at 1.
pub fn numbered(stem: &str, ext: &str, n: u32) -> String {
    if n == 1 {
        format!("{stem}{ext}")
    } else {
        format!("{stem} {n}{ext}")
    }
}

/// Creates an empty note named `name` (e.g. "Ideas.md") in `dir`, numbered "Ideas 2.md", …
/// if taken. Uses create_new (O_EXCL), so an entry that appears meanwhile is never overwritten.
pub fn create_note(dir: &Path, name: &str) -> io::Result<PathBuf> {
    create_numbered(dir, name).map(|(_, path)| path)
}

/// Copies the file `src` into `dir` under its own name, numbered "Note 2.md", … if taken
/// (never overwrites). A failed copy removes the partial file.
pub fn copy_in(src: &Path, dir: &Path) -> io::Result<PathBuf> {
    let name = src.file_name().unwrap_or_default().to_string_lossy();
    let mut from = fs::File::open(src)?;
    let (mut to, path) = create_numbered(dir, &name)?;
    if let Err(e) = io::copy(&mut from, &mut to).and_then(|_| to.sync_all()) {
        let _ = fs::remove_file(&path);
        return Err(e);
    }
    Ok(path)
}

/// `name` in `dir`, or the first free numbered variant, created with create_new.
fn create_numbered(dir: &Path, name: &str) -> io::Result<(fs::File, PathBuf)> {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    for n in 1..=10_000 {
        let path = dir.join(numbered(stem, ext, n));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("too many notes named “{name}”"),
    ))
}

/// Renames `from` to `new_name` in the same folder, refusing to replace anything there.
/// Renaming to the same name is a no-op.
pub fn rename(from: &Path, new_name: &str) -> io::Result<PathBuf> {
    move_to(from, &from.with_file_name(new_name))
}

/// Moves (renames) `from` to the path `to` on the same filesystem, refusing to replace anything
/// there. `to == from` is a no-op.
pub fn move_to(from: &Path, to: &Path) -> io::Result<PathBuf> {
    let to = to.to_path_buf();
    if to == from {
        return Ok(to);
    }
    let taken = || {
        let name = to.file_name().unwrap_or_default().to_string_lossy();
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("“{name}” already exists"),
        )
    };
    match rename_noreplace(from, &to) {
        Ok(()) => return Ok(to),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(taken()),
        // Filesystem without RENAME_NOREPLACE (some FUSE/NFS) → fallback below.
        Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS)) => {}
        Err(e) => return Err(e),
    }
    // shortcut: check-then-rename leaves a millisecond window where an entry created by another
    // app could be replaced; only used where the filesystem lacks RENAME_NOREPLACE.
    // symlink_metadata: a dangling symlink also counts as taken.
    if to.symlink_metadata().is_ok() {
        return Err(taken());
    }
    fs::rename(from, &to)?;
    Ok(to)
}

/// Where dropping `from` (a note or folder) on folder `dir` moves it, both inside the open
/// folder `root`: Ok(None) if it is already there, Err if the move is not allowed (out of
/// `root`, a folder into itself or its subfolder). Paths are compared canonically so a
/// symlink or `..` cannot escape `root`; `from` itself is not followed (it may be a symlink).
pub fn move_target(from: &Path, dir: &Path, root: &Path) -> Result<Option<PathBuf>, String> {
    let canon = |p: &Path| fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
    let (Some(name), Some(parent)) = (from.file_name(), from.parent()) else {
        return Err("Cannot move this entry.".into());
    };
    let root = canon(root)?;
    let src = canon(parent)?.join(name);
    let dst = canon(dir)?;
    if !src.starts_with(&root) || src == root || !dst.starts_with(&root) {
        return Err("Can only move inside the open folder.".into());
    }
    if dst.starts_with(&src) {
        return Err("Cannot move a folder into itself.".into());
    }
    if src.parent() == Some(dst.as_path()) {
        return Ok(None);
    }
    Ok(Some(dir.join(name)))
}

/// renameat2(RENAME_NOREPLACE) (Linux ≥ 3.15): atomically fails with EEXIST if `to` exists.
fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| {
        CString::new(p.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    };
    let (from, to) = (c(from)?, c(to)?);
    // SAFETY: both pointers are valid NUL-terminated strings that outlive the call.
    let r = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mdmate-nm-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn invalid_names_are_refused() {
        for bad in ["", "   ", "a/b", "/", ".", "..", ".hidden", "a\0b"] {
            assert!(check(bad).is_err(), "{bad:?}");
            assert!(note_name(bad).is_err(), "{bad:?}");
        }
        assert_eq!(check("  Ideas  "), Ok("Ideas"));
    }

    #[test]
    fn note_names_keep_or_get_md() {
        assert_eq!(note_name("Ideas").unwrap(), "Ideas.md");
        assert_eq!(note_name("Ideas.md").unwrap(), "Ideas.md");
        assert_eq!(note_name("Ideas.MD").unwrap(), "Ideas.MD");
        assert_eq!(note_name("Ideas.markdown").unwrap(), "Ideas.markdown");
        assert_eq!(note_name("v1.2").unwrap(), "v1.2.md");
        assert_eq!(note_name("todo.txt").unwrap(), "todo.txt.md");
    }

    #[test]
    fn new_notes_are_numbered_and_never_overwrite() {
        let t = TempDir::new("create");
        fs::write(t.0.join("Untitled.md"), "keep").unwrap();
        let second = create_note(&t.0, "Untitled.md").unwrap();
        assert_eq!(second, t.0.join("Untitled 2.md"));
        assert_eq!(
            create_note(&t.0, "Untitled.md").unwrap(),
            t.0.join("Untitled 3.md")
        );
        assert_eq!(create_note(&t.0, "v1.2.md").unwrap(), t.0.join("v1.2.md"));
        assert_eq!(create_note(&t.0, "v1.2.md").unwrap(), t.0.join("v1.2 2.md"));
        assert_eq!(fs::read_to_string(t.0.join("Untitled.md")).unwrap(), "keep");
        assert_eq!(fs::read_to_string(second).unwrap(), "");
    }

    #[test]
    fn copy_in_is_numbered_and_never_overwrites() {
        let t = TempDir::new("copyin");
        let (src, dst) = (t.0.join("src"), t.0.join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("Note.md"), "new").unwrap();
        fs::write(dst.join("Note.md"), "keep").unwrap();
        let copy = copy_in(&src.join("Note.md"), &dst).unwrap();
        assert_eq!(copy, dst.join("Note 2.md"));
        assert_eq!(fs::read_to_string(&copy).unwrap(), "new");
        assert_eq!(fs::read_to_string(dst.join("Note.md")).unwrap(), "keep");
        assert_eq!(fs::read_to_string(src.join("Note.md")).unwrap(), "new");
        // A file dropped into its own folder gets a numbered copy too.
        assert_eq!(copy_in(&copy, &dst).unwrap(), dst.join("Note 2 2.md"));
        assert!(copy_in(&src.join("missing.md"), &dst).is_err());
        assert!(!dst.join("missing.md").exists());
    }

    #[test]
    fn rename_refuses_existing_target() {
        let t = TempDir::new("rename");
        let a = t.0.join("a.md");
        fs::write(&a, "a").unwrap();
        fs::write(t.0.join("b.md"), "b").unwrap();
        fs::create_dir(t.0.join("empty")).unwrap();

        let err = rename(&a, "b.md").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(t.0.join("b.md")).unwrap(), "b");
        // An empty folder is not replaced either (plain rename(2) would replace it).
        fs::create_dir(t.0.join("dir")).unwrap();
        assert_eq!(
            rename(&t.0.join("dir"), "empty").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(t.0.join("dir").is_dir());

        assert_eq!(fs::read_to_string(&a).unwrap(), "a");
        // A folder is not renamed over a file either; both stay intact.
        assert_eq!(
            rename(&t.0.join("dir"), "b.md").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_to_string(t.0.join("b.md")).unwrap(), "b");
        assert!(t.0.join("empty").is_dir());
        let d = rename(&t.0.join("dir"), "dir2").unwrap();
        assert!(d.is_dir() && !t.0.join("dir").exists());

        let c = rename(&a, "c.md").unwrap();
        assert_eq!(fs::read_to_string(&c).unwrap(), "a");
        assert!(!a.exists());
        assert_eq!(rename(&c, "c.md").unwrap(), c);
    }

    #[test]
    fn move_targets_stay_inside_root() {
        let t = TempDir::new("target");
        let root = &t.0;
        fs::create_dir_all(root.join("a/sub")).unwrap();
        fs::create_dir(root.join("b")).unwrap();
        fs::write(root.join("n.md"), "").unwrap();
        fs::write(root.join("a/m.md"), "").unwrap();
        let outside = TempDir::new("outside");

        let tgt = |from: &str, dir: &Path| move_target(&root.join(from), dir, root);
        assert_eq!(tgt("n.md", &root.join("a")), Ok(Some(root.join("a/n.md"))));
        assert_eq!(tgt("a", &root.join("b")), Ok(Some(root.join("b/a"))));
        assert_eq!(tgt("a/m.md", root), Ok(Some(root.join("m.md"))));
        // Same place: nothing to do (also via a `..` path).
        assert_eq!(tgt("n.md", root), Ok(None));
        assert_eq!(tgt("a/m.md", &root.join("b/../a")), Ok(None));
        // A folder into itself or its subfolder.
        assert!(tgt("a", &root.join("a")).is_err());
        assert!(tgt("a", &root.join("a/sub")).is_err());
        // Out of the open folder, either way; the root itself cannot move.
        assert!(tgt("n.md", &outside.0).is_err());
        assert!(tgt("n.md", &root.join("..")).is_err());
        assert!(move_target(&outside.0, root, root).is_err());
        assert!(move_target(root, &root.join("b"), root).is_err());
        // A symlink pointing out of root is not a way out.
        std::os::unix::fs::symlink(&outside.0, root.join("link")).unwrap();
        assert!(tgt("n.md", &root.join("link")).is_err());
        assert!(tgt("n.md", &root.join("missing")).is_err());
    }

    #[test]
    fn move_to_refuses_existing_target() {
        let t = TempDir::new("move");
        fs::create_dir(t.0.join("d")).unwrap();
        fs::write(t.0.join("n.md"), "outer").unwrap();
        fs::write(t.0.join("d/n.md"), "inner").unwrap();
        let err = move_to(&t.0.join("n.md"), &t.0.join("d/n.md")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(t.0.join("d/n.md")).unwrap(), "inner");
        assert_eq!(fs::read_to_string(t.0.join("n.md")).unwrap(), "outer");
        fs::write(t.0.join("o.md"), "o").unwrap();
        let to = move_to(&t.0.join("o.md"), &t.0.join("d/o.md")).unwrap();
        assert_eq!(fs::read_to_string(to).unwrap(), "o");
        assert!(!t.0.join("o.md").exists());
    }
}
