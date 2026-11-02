//! Several sidebar entries selected at once (Ctrl / Shift-click, Shift+↑↓): which right-click
//! actions apply to all of them, which entries a move or Move to Trash acts on, the texts shown
//! for them and the data a drag of them carries.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// A right-click action offered for a selection of several entries, each acting on all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Duplicate,
    AddToFavorites,
    RemoveFromFavorites,
    ShowInFiles,
    CopyPath,
    MoveToTrash,
    MoveToFolder,
}

impl Action {
    pub fn title(self) -> &'static str {
        match self {
            Action::Duplicate => "Duplicate",
            Action::AddToFavorites => "Add to Favorites",
            Action::RemoveFromFavorites => "Remove from Favorites",
            Action::ShowInFiles => "Show in Files",
            Action::CopyPath => "Copy Path",
            Action::MoveToTrash => "Move to Trash",
            Action::MoveToFolder => "Move to Folder…",
        }
    }
}

/// The actions offered for `entries` (path, is a note) under the open folder `root`, in menu
/// order: none when the selection is empty or holds `root` (or anything outside it);
/// Favorites only when all are notes — Remove when all are favourites (`favourite`), else Add.
/// Single-item actions (Rename, New Note / Folder here, Copy Wiki Link, Find in “Folder”…,
/// Sort By) are never offered.
pub fn actions(
    entries: &[(PathBuf, bool)],
    root: &Path,
    favourite: impl Fn(&Path) -> bool,
) -> Vec<Action> {
    if entries.is_empty()
        || entries
            .iter()
            .any(|(path, _)| path == root || !path.starts_with(root))
    {
        return Vec::new();
    }
    let mut out = vec![Action::Duplicate];
    if entries.iter().all(|(_, is_note)| *is_note) {
        out.push(if entries.iter().all(|(path, _)| favourite(path)) {
            Action::RemoveFromFavorites
        } else {
            Action::AddToFavorites
        });
    }
    out.extend([
        Action::ShowInFiles,
        Action::CopyPath,
        Action::MoveToTrash,
        Action::MoveToFolder,
    ]);
    out
}

/// The entries a move or Move to Trash acts on: `paths` in their order without repeats and
/// without those inside another selected folder (that folder carries them).
pub fn move_set(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for path in paths {
        let nested = paths.iter().any(|p| p != path && path.starts_with(p));
        if !nested && !out.contains(path) {
            out.push(path.clone());
        }
    }
    out
}

/// Whether Move to Folder… offers folder `dir` for `entries`: none of them is `dir` or a folder
/// above it (no folder moves into itself), and at least one is not already in `dir`.
pub fn can_move_into(entries: &[PathBuf], dir: &Path) -> bool {
    entries.iter().all(|e| !dir.starts_with(e)) && entries.iter().any(|e| e.parent() != Some(dir))
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Heading and body of the one confirmation before trashing `notes` notes and `folders`
/// folders at once.
pub fn trash_question(notes: usize, folders: usize) -> (String, String) {
    let what = match (notes, folders) {
        (n, 0) => count(n, "note", "notes"),
        (0, f) => count(f, "folder", "folders"),
        (n, f) => format!(
            "{} and {}",
            count(n, "note", "notes"),
            count(f, "folder", "folders")
        ),
    };
    let body = if folders > 0 {
        "Folders go with everything in them. You can restore them from the Trash in Files."
    } else {
        "You can restore them from the Trash in Files."
    };
    (format!("Move {what} to the Trash?"), body.to_string())
}

/// The toast after some entries could not be moved (`failed`: name, reason).
pub fn failed_moves(failed: &[(String, String)]) -> Option<String> {
    match failed {
        [] => None,
        [(name, why)] => Some(format!("Cannot move {name}: {why}")),
        [(name, why), ..] => Some(format!(
            "Cannot move {} items ({name}: {why})",
            failed.len()
        )),
    }
}

/// Copy Path for several entries: one full path per line.
pub fn copy_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The data of a sidebar drag: the paths separated by NUL bytes (the one byte no path holds).
pub fn encode(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            out.push(0);
        }
        out.extend_from_slice(path.as_os_str().as_bytes());
    }
    out
}

/// The paths of a sidebar drag (`encode`); a single path without NUL is one entry.
pub fn decode(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == 0)
        .filter(|part| !part.is_empty())
        .map(|part| PathBuf::from(std::ffi::OsStr::from_bytes(part)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn notes(paths: &[&str]) -> Vec<(PathBuf, bool)> {
        paths.iter().map(|s| (p(s), true)).collect()
    }

    #[test]
    fn notes_only_offer_favorites() {
        let root = Path::new("/r");
        let sel = notes(&["/r/a.md", "/r/b/c.md"]);
        assert_eq!(
            actions(&sel, root, |_| false),
            [
                Action::Duplicate,
                Action::AddToFavorites,
                Action::ShowInFiles,
                Action::CopyPath,
                Action::MoveToTrash,
                Action::MoveToFolder,
            ]
        );
        // All favourites: Remove; some: Add.
        assert!(actions(&sel, root, |_| true).contains(&Action::RemoveFromFavorites));
        let mixed = actions(&sel, root, |path| path == Path::new("/r/a.md"));
        assert!(mixed.contains(&Action::AddToFavorites));
        assert!(!mixed.contains(&Action::RemoveFromFavorites));
    }

    #[test]
    fn a_folder_in_the_selection_drops_favorites() {
        let sel = vec![(p("/r/a.md"), true), (p("/r/b"), false)];
        assert_eq!(
            actions(&sel, Path::new("/r"), |_| true),
            [
                Action::Duplicate,
                Action::ShowInFiles,
                Action::CopyPath,
                Action::MoveToTrash,
                Action::MoveToFolder,
            ]
        );
    }

    #[test]
    fn the_root_or_nothing_offers_nothing() {
        let root = Path::new("/r");
        let sel = vec![(p("/r"), false), (p("/r/a.md"), true)];
        assert!(actions(&sel, root, |_| false).is_empty());
        assert!(actions(&notes(&["/x/a.md"]), root, |_| false).is_empty());
        assert!(actions(&[], root, |_| false).is_empty());
    }

    #[test]
    fn move_set_drops_entries_inside_selected_folders() {
        let sel = [
            p("/r/b/c.md"),
            p("/r/b"),
            p("/r/bb.md"),
            p("/r/b/d/e.md"),
            p("/r/a.md"),
            p("/r/a.md"),
        ];
        assert_eq!(move_set(&sel), [p("/r/b"), p("/r/bb.md"), p("/r/a.md")]);
    }

    #[test]
    fn move_to_folder_greys_out_its_own_folder_and_itself() {
        let one = |e: &str, d: &str| can_move_into(&[p(e)], Path::new(d));
        // A note: anywhere but its own folder.
        assert!(!one("/r/b/m.md", "/r/b"));
        assert!(one("/r/b/m.md", "/r"));
        assert!(one("/r/b/m.md", "/r/b/c"));
        // A folder: not its parent, itself or below it; a sibling with a longer name is fine.
        assert!(!one("/r/b", "/r"));
        assert!(!one("/r/b", "/r/b"));
        assert!(!one("/r/b", "/r/b/c"));
        assert!(one("/r/b", "/r/bb"));
        assert!(one("/r/b/c", "/r"));
    }

    #[test]
    fn move_to_folder_for_several() {
        let sel = [p("/r/a.md"), p("/r/x/b.md"), p("/r/y")];
        // One already there is fine while another moves.
        assert!(can_move_into(&sel, Path::new("/r/x")));
        // All already there.
        assert!(!can_move_into(&sel[..1], Path::new("/r")));
        // Into a selected folder or below it: never.
        assert!(!can_move_into(&sel, Path::new("/r/y")));
        assert!(!can_move_into(&sel, Path::new("/r/y/z")));
    }

    #[test]
    fn trash_question_names_the_count() {
        assert_eq!(
            trash_question(3, 0),
            (
                "Move 3 notes to the Trash?".to_string(),
                "You can restore them from the Trash in Files.".to_string()
            )
        );
        assert_eq!(trash_question(0, 2).0, "Move 2 folders to the Trash?");
        let (heading, body) = trash_question(1, 1);
        assert_eq!(heading, "Move 1 note and 1 folder to the Trash?");
        assert!(body.starts_with("Folders go with everything in them."));
    }

    #[test]
    fn failed_moves_toast() {
        assert_eq!(failed_moves(&[]), None);
        let one = [("a.md".to_string(), "taken".to_string())];
        assert_eq!(failed_moves(&one).unwrap(), "Cannot move a.md: taken");
        let two = [one[0].clone(), ("b".to_string(), "gone".to_string())];
        assert_eq!(
            failed_moves(&two).unwrap(),
            "Cannot move 2 items (a.md: taken)"
        );
    }

    #[test]
    fn copy_path_one_per_line() {
        assert_eq!(copy_paths(&[p("/r/a.md"), p("/r/b c")]), "/r/a.md\n/r/b c");
    }

    #[test]
    fn drag_data_round_trip() {
        let paths = vec![p("/r/a.md"), p("/r/b c/d.md"), p("/r/ä\nb")];
        assert_eq!(decode(&encode(&paths)), paths);
        // A drag of one entry is its path as before.
        assert_eq!(encode(&[p("/r/a.md")]), b"/r/a.md");
        assert_eq!(decode(b"/r/a.md"), [p("/r/a.md")]);
        assert!(decode(b"").is_empty());
    }
}
