//! Open Quickly (Ctrl+P): fuzzy, case-insensitive note search by name. Pure Rust (no GTK) so
//! it is testable headless.
//!
//! Ranking, best first: the file name starts with the query > contains it in one piece >
//! contains its characters in order; then the same three against the path relative to the
//! open folder. Ties: shorter name, then relative path. The open folder's Favorites are
//! listed and preferred (`with_favorites`).

use std::path::{Path, PathBuf};

use crate::notes::Node;

/// Most results shown.
pub const LIMIT: usize = 50;

/// Most recently opened notes shown for an empty query.
pub const RECENT: usize = 10;

/// What an empty query lists: up to `RECENT` of `history` (newest first, e.g. the remembered
/// cursor positions) that are still in `notes`, minus `current`, so Enter goes back to the
/// previous note.
pub fn recent<'a>(
    history: impl IntoIterator<Item = &'a Path>,
    notes: &'a [PathBuf],
    current: Option<&Path>,
) -> Vec<&'a PathBuf> {
    history
        .into_iter()
        .filter(|h| Some(*h) != current)
        .filter_map(|h| notes.iter().find(|n| n.as_path() == h))
        .take(RECENT)
        .collect()
}

/// Every note path in `tree`, in sidebar order.
pub fn note_paths(tree: &[Node], out: &mut Vec<PathBuf>) {
    for node in tree {
        match node {
            Node::Dir { children, .. } => note_paths(children, out),
            Node::Note { path, .. } => out.push(path.clone()),
        }
    }
}

/// `notes` after the app itself created note `to` (`from` None), moved note or folder `from`
/// to `to`, or trashed `from` (`to` None). Bridges the gap until the next scan, which stays
/// the source of truth (e.g. a note renamed to a hidden name drops out only then).
pub fn updated(notes: &[PathBuf], from: Option<&Path>, to: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = notes
        .iter()
        .filter_map(|p| {
            let Some(rel) = from.and_then(|f| p.strip_prefix(f).ok()) else {
                return Some(p.clone());
            };
            // join("") would add a trailing slash.
            to.map(|t| {
                if rel.as_os_str().is_empty() {
                    t.to_path_buf()
                } else {
                    t.join(rel)
                }
            })
        })
        .collect();
    if from.is_none()
        && let Some(t) = to
        && !out.iter().any(|p| p == t)
    {
        out.push(t.to_path_buf());
    }
    out
}

/// 0 = prefix, 1 = contiguous, 2 = scattered, None = no match. `query` is lowercase.
fn tier(query: &str, text: &str) -> Option<u8> {
    let text = text.to_lowercase();
    if text.starts_with(query) {
        return Some(0);
    }
    if text.contains(query) {
        return Some(1);
    }
    let mut chars = text.chars();
    query.chars().all(|q| chars.any(|c| c == q)).then_some(2)
}

/// Rank of `path` for lowercase, trimmed `query` (smaller is better); None = no match.
fn key(query: &str, path: &Path, root: &Path) -> Option<(u8, usize, String)> {
    let name = path.file_name()?.to_string_lossy();
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    let rank = tier(query, &name).or_else(|| tier(query, &rel).map(|t| t + 3))?;
    Some((rank, name.chars().count(), rel.to_lowercase()))
}

/// Notes matching `query`, best first, at most `LIMIT`. An empty query lists notes by path.
pub fn search<'a>(query: &str, notes: &'a [PathBuf], root: &Path) -> Vec<&'a PathBuf> {
    let query = query.trim().to_lowercase();
    let mut hits: Vec<_> = notes
        .iter()
        .filter_map(|path| Some((key(&query, path, root)?, path)))
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0));
    hits.into_iter().take(LIMIT).map(|(_, p)| p).collect()
}

/// Open Quickly's list, each path with whether it is one of the open folder's `favorites`
/// (notes and folders that exist). Empty `query`: the `recent` notes, then the favourites not
/// listed yet (minus `current`); with neither, every note by path. Otherwise favourites
/// (folders too) and `notes` ranked together like `search`, favourites first within each kind
/// of match (prefix, in one piece, …). At most `LIMIT`.
pub fn with_favorites(
    query: &str,
    favorites: &[PathBuf],
    current: Option<&Path>,
    root: &Path,
    notes: &[PathBuf],
    recent: &[&PathBuf],
) -> Vec<(PathBuf, bool)> {
    let query = query.trim().to_lowercase();
    if query.is_empty() && !(recent.is_empty() && favorites.is_empty()) {
        let fav = |p: &PathBuf| favorites.contains(p);
        let mut out: Vec<(PathBuf, bool)> = recent.iter().map(|p| ((*p).clone(), fav(p))).collect();
        for f in favorites {
            if Some(f.as_path()) != current && !recent.contains(&f) {
                out.push((f.clone(), true));
            }
        }
        out.truncate(LIMIT);
        return out;
    }
    let others = notes.iter().filter(|n| !favorites.contains(n));
    let mut hits: Vec<_> = favorites
        .iter()
        .map(|p| (p, true))
        .chain(others.map(|p| (p, false)))
        .filter_map(|(path, fav)| {
            let (rank, len, rel) = key(&query, path, root)?;
            Some(((rank, !fav, len, rel), path, fav))
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0));
    hits.into_iter()
        .take(LIMIT)
        .map(|(_, path, fav)| (path.clone(), fav))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn favorites_after_recent_notes_and_ranked_with_matches() {
        let root = Path::new("/n");
        let p = |v: &[&str]| v.iter().map(|s| root.join(s)).collect::<Vec<_>>();
        let favorites = p(&["Daily", "plan.md", "b.md"]);
        let notes = p(&["a.md", "b.md", "plan.md", "Daily/x.md", "pla.md"]);
        let names = |got: Vec<(PathBuf, bool)>| {
            got.into_iter()
                .map(|(path, fav)| (path.strip_prefix(root).unwrap().display().to_string(), fav))
                .collect::<Vec<_>>()
        };
        let s = |v: &str, fav| (v.to_string(), fav);
        let current = root.join("plan.md");
        // Empty box: recent notes (Enter = the previous note), then the other favourites but
        // the open note.
        let recent = [&notes[0], &notes[1]];
        assert_eq!(
            names(with_favorites(
                " ",
                &favorites,
                Some(&current),
                root,
                &notes,
                &recent
            )),
            [s("a.md", false), s("b.md", true), s("Daily", true)]
        );
        // Nothing opened yet: favourites alone; with no favourites either, every note.
        assert_eq!(
            names(with_favorites("", &favorites, None, root, &notes, &[])),
            [s("Daily", true), s("plan.md", true), s("b.md", true)]
        );
        assert_eq!(
            with_favorites("", &[], None, root, &notes, &[]).len(),
            notes.len()
        );
        // A query: ranked like `search`, folders too; within a kind of match a favourite comes
        // first, listed once.
        assert_eq!(
            names(with_favorites("pla", &favorites, None, root, &notes, &[])),
            [s("plan.md", true), s("pla.md", false)]
        );
        assert_eq!(
            names(with_favorites("a", &favorites, None, root, &notes, &[])),
            [
                s("a.md", false),
                s("Daily", true),
                s("plan.md", true),
                s("pla.md", false),
                s("Daily/x.md", false)
            ]
        );
        assert!(with_favorites("zzz", &favorites, None, root, &notes, &[]).is_empty());
    }

    #[test]
    fn updated_follows_create_move_and_trash() {
        let n = |v: &[&str]| v.iter().map(PathBuf::from).collect::<Vec<_>>();
        let notes = n(&["/n/a.md", "/n/d/b.md", "/n/d/e/c.md", "/n/dd.md"]);
        let p = |s| Some(Path::new(s));
        assert_eq!(
            updated(&notes, None, p("/n/new.md")),
            n(&[
                "/n/a.md",
                "/n/d/b.md",
                "/n/d/e/c.md",
                "/n/dd.md",
                "/n/new.md"
            ])
        );
        // Creating a note already listed (the scan was faster) adds no duplicate.
        assert_eq!(updated(&notes, None, p("/n/a.md")), notes);
        assert_eq!(
            updated(&notes, p("/n/a.md"), p("/n/z.md")),
            n(&["/n/z.md", "/n/d/b.md", "/n/d/e/c.md", "/n/dd.md"])
        );
        // A folder takes its notes along; `dd.md` only shares the prefix text.
        assert_eq!(
            updated(&notes, p("/n/d"), p("/n/x/d")),
            n(&["/n/a.md", "/n/x/d/b.md", "/n/x/d/e/c.md", "/n/dd.md"])
        );
        assert_eq!(
            updated(&notes, p("/n/d"), None),
            n(&["/n/a.md", "/n/dd.md"])
        );
    }

    fn find(query: &str, notes: &[&str]) -> Vec<String> {
        let root = Path::new("/n");
        let paths: Vec<PathBuf> = notes.iter().map(|n| root.join(n)).collect();
        search(query, &paths, root)
            .into_iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(find("IDEA", &["ideas.md", "Other.md"]), ["ideas.md"]);
        assert_eq!(find("ideas", &["IDEAS.md"]), ["IDEAS.md"]);
    }

    #[test]
    fn ranks_prefix_then_contiguous_then_scattered_then_path() {
        let notes = [
            "work/misc.md",          // no "p" at all
            "plan/todo.md",          // only the path matches
            "a plan b.md",           // contiguous in name
            "p-l-a-n.md",            // scattered in name
            "planning long name.md", // prefix, longer
            "Plan.md",               // prefix, shortest
        ];
        assert_eq!(
            find("plan", &notes),
            [
                "Plan.md",
                "planning long name.md",
                "a plan b.md",
                "p-l-a-n.md",
                "plan/todo.md"
            ]
        );
        // Path-only match: "wm" is scattered in "work/misc.md" but not in "misc.md".
        assert_eq!(find("wm", &notes), ["work/misc.md"]);
    }

    #[test]
    fn no_match_and_empty_query() {
        assert!(find("zzz", &["a.md", "b/c.md"]).is_empty());
        // Order of characters matters.
        assert!(find("ba", &["ab.md"]).is_empty());
        assert_eq!(find("  ", &["b.md", "a.md"]), ["a.md", "b.md"]);
    }

    #[test]
    fn recent_newest_first_skips_current_and_gone_capped() {
        let p = |v: &[&str]| v.iter().map(PathBuf::from).collect::<Vec<_>>();
        let notes = p(&["/n/a.md", "/n/b.md", "/n/c.md"]);
        let history = p(&["/n/c.md", "/n/gone.md", "/n/a.md", "/n/b.md"]);
        let got = recent(
            history.iter().map(PathBuf::as_path),
            &notes,
            Some(Path::new("/n/c.md")),
        );
        assert_eq!(got, [&notes[0], &notes[1]]);
        assert!(recent([], &notes, None).is_empty());
        let many = p(&(0..20)
            .map(|i| format!("/n/{i}.md"))
            .collect::<Vec<_>>()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>());
        assert_eq!(
            recent(many.iter().map(PathBuf::as_path), &many, None).len(),
            RECENT
        );
    }

    #[test]
    fn at_most_limit_results() {
        let many: Vec<String> = (0..80).map(|i| format!("n{i}.md")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        assert_eq!(find("n", &many).len(), LIMIT);
    }

    #[test]
    fn collects_notes_from_tree() {
        let tree = vec![
            Node::Dir {
                name: "d".into(),
                children: vec![Node::Note {
                    name: "b.md".into(),
                    path: "/n/d/b.md".into(),
                    date: None,
                }],
            },
            Node::Note {
                name: "a.md".into(),
                path: "/n/a.md".into(),
                date: None,
            },
        ];
        let mut out = Vec::new();
        note_paths(&tree, &mut out);
        assert_eq!(out, [PathBuf::from("/n/d/b.md"), PathBuf::from("/n/a.md")]);
    }
}
