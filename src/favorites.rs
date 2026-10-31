//! Favorites: notes and folders pinned at the top of the sidebar, per open folder, in the
//! order they were added. Kept as paths relative to the open folder (`state.rs`, `favorite=`
//! lines), so they follow the folder when it moves as a whole. Pure Rust (no GTK) so it is
//! testable headless.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::editor::moved;

/// Every folder's favourites: open folder → its favourites (relative paths, order added).
pub type All = BTreeMap<PathBuf, Vec<PathBuf>>;

/// A path relative to the open folder that stays inside it: not empty, no `..`, `.` or root.
pub fn is_clean(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)))
}

/// `path` relative to `root`; None for `root` itself or a path outside it.
pub fn relative(root: &Path, path: &Path) -> Option<PathBuf> {
    let rel = path.strip_prefix(root).ok()?;
    is_clean(rel).then(|| rel.to_path_buf())
}

/// Whether `path` (absolute) is one of `list`'s favourites of `root`.
pub fn contains(list: &[PathBuf], root: &Path, path: &Path) -> bool {
    relative(root, path).is_some_and(|rel| list.contains(&rel))
}

/// Adds `path` at the end (`on`, if not already there) or removes it (`!on`). Paths outside
/// `root` (or `root` itself) are ignored.
pub fn toggle(list: &mut Vec<PathBuf>, root: &Path, path: &Path, on: bool) {
    let Some(rel) = relative(root, path) else {
        return;
    };
    let at = list.iter().position(|p| *p == rel);
    match (at, on) {
        (None, true) => list.push(rel),
        (Some(i), false) => {
            list.remove(i);
        }
        _ => {}
    }
}

/// After `from` was renamed or moved to `to` inside `root`: a favourite that is `from` or
/// inside it follows, in its place. One that would land outside `root` is dropped.
pub fn follow(list: &mut Vec<PathBuf>, root: &Path, from: &Path, to: &Path) {
    *list = list
        .drain(..)
        .filter_map(|rel| match moved(&root.join(&rel), from, to) {
            Some(new) => relative(root, &new),
            None => Some(rel),
        })
        .collect();
}

/// The favourites as absolute paths, in order, each with whether it is still there (`exists`):
/// a missing one is shown dimmed and removed when clicked.
pub fn entries(
    list: &[PathBuf],
    root: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Vec<(PathBuf, bool)> {
    list.iter()
        .map(|rel| {
            let path = root.join(rel);
            let there = exists(&path);
            (path, there)
        })
        .collect()
}

/// After `target` was moved to the Trash: drops the favourites that are it or inside it.
pub fn remove_under(list: &mut Vec<PathBuf>, root: &Path, target: &Path) {
    list.retain(|rel| !root.join(rel).starts_with(target));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn add_keeps_order_and_remove_drops_one() {
        let root = Path::new("/n");
        let mut list = Vec::new();
        for p in ["/n/b.md", "/n/a", "/n/a/c.md", "/n/b.md"] {
            toggle(&mut list, root, Path::new(p), true);
        }
        assert_eq!(
            list,
            paths(&["b.md", "a", "a/c.md"]),
            "order added, no duplicate"
        );
        toggle(&mut list, root, Path::new("/n/a"), false);
        toggle(&mut list, root, Path::new("/n/gone.md"), false);
        assert_eq!(list, paths(&["b.md", "a/c.md"]));
        assert!(contains(&list, root, Path::new("/n/a/c.md")));
        assert!(!contains(&list, root, Path::new("/n/a")));
        // The open folder itself and paths outside it are never favourites.
        toggle(&mut list, root, Path::new("/n"), true);
        toggle(&mut list, root, Path::new("/other/x.md"), true);
        toggle(&mut list, root, Path::new("/n/../x.md"), true);
        assert_eq!(list, paths(&["b.md", "a/c.md"]));
    }

    #[test]
    fn follows_renames_and_moves_in_place() {
        let root = Path::new("/n");
        let mut list = paths(&["a.md", "d", "d/e/c.md", "dd.md"]);
        follow(&mut list, root, Path::new("/n/a.md"), Path::new("/n/z.md"));
        assert_eq!(list, paths(&["z.md", "d", "d/e/c.md", "dd.md"]));
        // A folder takes its favourites along; `dd.md` only shares the prefix text.
        follow(&mut list, root, Path::new("/n/d"), Path::new("/n/x/d"));
        assert_eq!(list, paths(&["z.md", "x/d", "x/d/e/c.md", "dd.md"]));
        // A note moved into a folder.
        follow(
            &mut list,
            root,
            Path::new("/n/dd.md"),
            Path::new("/n/x/dd.md"),
        );
        assert_eq!(list, paths(&["z.md", "x/d", "x/d/e/c.md", "x/dd.md"]));
        // Unrelated moves change nothing.
        follow(&mut list, root, Path::new("/n/q.md"), Path::new("/n/r.md"));
        assert_eq!(list, paths(&["z.md", "x/d", "x/d/e/c.md", "x/dd.md"]));
    }

    #[test]
    fn trash_removes_the_entry_and_what_is_inside() {
        let root = Path::new("/n");
        let mut list = paths(&["a.md", "d", "d/e/c.md", "dd.md"]);
        remove_under(&mut list, root, Path::new("/n/d"));
        assert_eq!(list, paths(&["a.md", "dd.md"]));
        remove_under(&mut list, root, Path::new("/n/a.md"));
        assert_eq!(list, paths(&["dd.md"]));
    }

    #[test]
    fn missing_entries_are_marked_not_dropped() {
        let root = Path::new("/n");
        let list = paths(&["a.md", "gone.md", "d"]);
        let got = entries(&list, root, |p| p != Path::new("/n/gone.md"));
        assert_eq!(
            got,
            [
                (PathBuf::from("/n/a.md"), true),
                (PathBuf::from("/n/gone.md"), false),
                (PathBuf::from("/n/d"), true)
            ]
        );
    }

    #[test]
    fn only_clean_relative_paths() {
        assert!(is_clean(Path::new("a/b.md")));
        for bad in ["", "/a.md", "../a.md", "a/../b.md", "./a.md"] {
            assert!(!is_clean(Path::new(bad)), "{bad}");
        }
        assert_eq!(relative(Path::new("/n"), Path::new("/n")), None);
        assert_eq!(
            relative(Path::new("/n"), Path::new("/n/a/b.md")),
            Some(PathBuf::from("a/b.md"))
        );
    }
}
