//! Open Quickly (Ctrl+P): fuzzy, case-insensitive note search by name. Pure Rust (no GTK) so
//! it is testable headless.
//!
//! Ranking, best first: the file name starts with the query > contains it in one piece >
//! contains its characters in order; then the same three against the path relative to the
//! open folder. Ties: shorter name, then relative path. The open folder's Favorites are
//! listed and preferred, and front-matter aliases match like names (`with_favorites`).
//! A query starting with `#` searches the headings of every note instead (`headings`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::aliases::Aliases;
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

/// Order of Open Quickly's rows: kind of match (`key`), alias row, not a favourite, length,
/// relative path.
type Rank = (u8, bool, bool, usize, String);

/// One row of Open Quickly: a note or favourite folder, or a note reached by one of its
/// front-matter aliases (`alias`, as written; shown "Alias → Note").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: PathBuf,
    /// One of the open folder's Favorites (never for an alias row).
    pub favorite: bool,
    pub alias: Option<String>,
}

/// Open Quickly's list. Empty `query`: the `recent` notes, then the open folder's `favorites`
/// (notes and folders that exist) not listed yet (minus `current`); with neither, every note
/// by path. Otherwise favourites (folders too), `notes` and the `aliases` of `notes` ranked
/// together like `search`, an alias matched as a name is; within each kind of match (prefix,
/// in one piece, …) favourites first, then other notes, then aliases, so a note named like an
/// alias comes before it. A note listed by its name or path gets no alias row, and a note
/// whose aliases match gets one row (its best alias). At most `LIMIT`.
pub fn with_favorites(
    query: &str,
    favorites: &[PathBuf],
    current: Option<&Path>,
    root: &Path,
    notes: &[PathBuf],
    recent: &[&PathBuf],
    aliases: &Aliases,
) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    let row = |path: &PathBuf, favorite| Row {
        path: path.clone(),
        favorite,
        alias: None,
    };
    if query.is_empty() && !(recent.is_empty() && favorites.is_empty()) {
        let mut out: Vec<Row> = recent
            .iter()
            .map(|p| row(p, favorites.contains(p)))
            .collect();
        for f in favorites {
            if Some(f.as_path()) != current && !recent.contains(&f) {
                out.push(row(f, true));
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
            Some(((rank, false, !fav, len, rel), row(path, fav)))
        })
        .collect();
    if !query.is_empty() {
        let listed: HashSet<&Path> = hits.iter().map(|(_, r)| r.path.as_path()).collect();
        let known: HashSet<&Path> = notes.iter().map(PathBuf::as_path).collect();
        // Best alias per note; `all` is in note order, so a note's aliases are adjacent.
        let mut best: Vec<(Rank, Row)> = Vec::new();
        for (note, alias) in aliases.all() {
            if listed.contains(note.as_path()) || !known.contains(note.as_path()) {
                continue;
            }
            let Some(rank) = tier(&query, alias) else {
                continue;
            };
            let rel = note.strip_prefix(root).unwrap_or(note);
            let rank = (
                rank,
                true,
                true,
                alias.chars().count(),
                rel.to_string_lossy().to_lowercase(),
            );
            match best.last_mut() {
                Some((at, r)) if r.path == *note => {
                    if rank < *at {
                        *at = rank;
                        r.alias = Some(alias.clone());
                    }
                }
                _ => best.push((
                    rank,
                    Row {
                        path: note.clone(),
                        favorite: false,
                        alias: Some(alias.clone()),
                    },
                )),
            }
        }
        hits.extend(best);
    }
    hits.sort_by(|a, b| a.0.cmp(&b.0));
    hits.into_iter().take(LIMIT).map(|(_, r)| r).collect()
}

/// A heading of a note, as Open Quickly lists it (`render::heading_targets`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// Plain text, as the Outline shows it.
    pub title: String,
    /// What follows `#` in a `[[Note#…]]` link to it, which opening it goes to.
    pub fragment: String,
}

impl Heading {
    /// The headings of `md` in order (any level; not in code blocks or front matter).
    pub fn all(md: &str) -> Vec<Heading> {
        crate::render::heading_targets(md)
            .into_iter()
            .map(|h| Heading {
                title: h.title,
                fragment: h.fragment,
            })
            .collect()
    }
}

/// A note and its headings, in document order.
pub type NoteHeadings = (PathBuf, Vec<Heading>);

/// The heading part of an Open Quickly query, lowercase: what follows the leading `#`s of a
/// query starting with `#`, trimmed (empty for `#` alone). None for a note query.
pub fn heading_query(query: &str) -> Option<String> {
    let rest = query.trim().strip_prefix('#')?;
    Some(rest.trim_start_matches('#').trim().to_lowercase())
}

/// One heading row of Open Quickly: Enter opens `path` at `heading`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadingRow {
    pub path: PathBuf,
    pub heading: Heading,
}

impl HeadingRow {
    /// "Heading — Note" (the note's file name without its extension).
    pub fn label(&self) -> String {
        let note = self.path.file_stem().unwrap_or_default().to_string_lossy();
        format!("{} — {note}", self.heading.title)
    }
}

/// The headings of `notes` (each note's, in document order) matching `query`
/// (`heading_query`'s text) like `search` matches names: starting with it, then containing
/// it in one piece, then its characters in order. Ties by note name, then in `notes` order
/// and document order. Nothing for an empty `query`; at most `LIMIT`.
pub fn headings(query: &str, notes: &[NoteHeadings]) -> Vec<HeadingRow> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<((u8, String), &PathBuf, &Heading)> = Vec::new();
    for (path, list) in notes {
        let mut name = None;
        for heading in list {
            let Some(rank) = tier(query, &heading.title) else {
                continue;
            };
            let name = name.get_or_insert_with(|| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase()
            });
            hits.push(((rank, name.clone()), path, heading));
        }
    }
    // Stable: equal keys keep note and document order.
    hits.sort_by(|a, b| a.0.cmp(&b.0));
    hits.into_iter()
        .take(LIMIT)
        .map(|(_, path, heading)| HeadingRow {
            path: path.clone(),
            heading: heading.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(title: &str) -> Heading {
        Heading {
            title: title.to_string(),
            fragment: title.to_string(),
        }
    }

    #[test]
    fn heading_mode_starts_with_a_hash() {
        assert_eq!(heading_query("#budget"), Some("budget".to_string()));
        assert_eq!(
            heading_query("  # Budget Q3 "),
            Some("budget q3".to_string())
        );
        assert_eq!(heading_query("## Plan"), Some("plan".to_string()));
        assert_eq!(heading_query("#"), Some(String::new()));
        assert_eq!(heading_query("budget"), None);
        assert_eq!(heading_query("a#b"), None);
        assert_eq!(heading_query(""), None);
        // `#` alone lists nothing.
        let notes = [(PathBuf::from("/n/a.md"), vec![heading("Budget")])];
        assert!(headings("", &notes).is_empty());
    }

    #[test]
    fn headings_rank_like_names_then_by_note_name() {
        let notes = [
            (
                PathBuf::from("/n/zeta.md"),
                vec![heading("Budget"), heading("Old budget"), heading("Intro")],
            ),
            (PathBuf::from("/n/b/Alpha.md"), vec![heading("Q3 budget")]),
            (PathBuf::from("/n/alpha.md"), vec![heading("Budget 2027")]),
            (PathBuf::from("/n/beta.md"), vec![heading("B-u-d-g-e-t")]),
        ];
        let got: Vec<String> = headings("budget", &notes)
            .iter()
            .map(HeadingRow::label)
            .collect();
        assert_eq!(
            got,
            [
                // Starts with it: alpha before zeta.
                "Budget 2027 — alpha",
                "Budget — zeta",
                // In one piece.
                "Q3 budget — Alpha",
                "Old budget — zeta",
                // Letters in order.
                "B-u-d-g-e-t — beta",
            ]
        );
        assert!(headings("nothing", &notes).is_empty());
    }

    #[test]
    fn heading_rows_open_their_fragment_at_most_limit() {
        let md = "---\ntitle: x\n---\n# Plan\n```\n# not a heading\n```\n## Plan\nSetext\n---\n";
        let all = Heading::all(md);
        let titles: Vec<&str> = all.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(titles, ["Plan", "Plan", "Setext"]);
        // The repeated title is reached by its id, as Copy Link to Heading writes it.
        assert_eq!(all[1].fragment, "plan-1");
        let many = vec![(PathBuf::from("/n/a.md"), vec![heading("x"); LIMIT + 5])];
        assert_eq!(headings("x", &many).len(), LIMIT);
    }

    #[test]
    fn favorites_after_recent_notes_and_ranked_with_matches() {
        let root = Path::new("/n");
        let p = |v: &[&str]| v.iter().map(|s| root.join(s)).collect::<Vec<_>>();
        let favorites = p(&["Daily", "plan.md", "b.md"]);
        let notes = p(&["a.md", "b.md", "plan.md", "Daily/x.md", "pla.md"]);
        let none = Aliases::default();
        let names = |got: Vec<Row>| {
            got.into_iter()
                .map(|r| {
                    let rel = r.path.strip_prefix(root).unwrap().display().to_string();
                    (rel, r.favorite)
                })
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
                &recent,
                &none
            )),
            [s("a.md", false), s("b.md", true), s("Daily", true)]
        );
        // Nothing opened yet: favourites alone; with no favourites either, every note.
        assert_eq!(
            names(with_favorites(
                "",
                &favorites,
                None,
                root,
                &notes,
                &[],
                &none
            )),
            [s("Daily", true), s("plan.md", true), s("b.md", true)]
        );
        assert_eq!(
            with_favorites("", &[], None, root, &notes, &[], &none).len(),
            notes.len()
        );
        // A query: ranked like `search`, folders too; within a kind of match a favourite comes
        // first, listed once.
        assert_eq!(
            names(with_favorites(
                "pla",
                &favorites,
                None,
                root,
                &notes,
                &[],
                &none
            )),
            [s("plan.md", true), s("pla.md", false)]
        );
        assert_eq!(
            names(with_favorites(
                "a",
                &favorites,
                None,
                root,
                &notes,
                &[],
                &none
            )),
            [
                s("a.md", false),
                s("Daily", true),
                s("plan.md", true),
                s("pla.md", false),
                s("Daily/x.md", false)
            ]
        );
        assert!(with_favorites("zzz", &favorites, None, root, &notes, &[], &none).is_empty());
    }

    /// Open Quickly rows for `query` as "name" or "alias → name" (relative paths).
    fn quick(query: &str, notes: &[&str], aliases: &[(&str, &[&str])]) -> Vec<String> {
        let root = Path::new("/n");
        let paths: Vec<PathBuf> = notes.iter().map(|n| root.join(n)).collect();
        let aliases = Aliases::new(
            aliases
                .iter()
                .map(|(n, a)| (root.join(n), a.iter().map(|a| a.to_string()).collect()))
                .collect(),
        );
        with_favorites(query, &[], None, root, &paths, &[], &aliases)
            .into_iter()
            .map(|r| {
                let rel = r.path.strip_prefix(root).unwrap().display().to_string();
                match r.alias {
                    Some(alias) => format!("{alias} → {rel}"),
                    None => rel,
                }
            })
            .collect()
    }

    #[test]
    fn aliases_match_like_names() {
        let notes = ["Meeting notes.md", "Daily/2026-11-02.md", "Other.md"];
        let aliases: &[(&str, &[&str])] = &[
            ("Meeting notes.md", &["MN", "Standup"]),
            ("Daily/2026-11-02.md", &["Today", "Monday plan"]),
            ("Gone.md", &["Today"]),
        ];
        // Prefix, any case: the alias row opens the note.
        assert_eq!(
            quick("stand", &notes, aliases),
            ["Standup → Meeting notes.md"]
        );
        // Letters in order, as for names; a note listed once, by its best alias. Aliases of
        // notes no longer in the list (trashed since the alias index) are left out.
        assert_eq!(
            quick("tdy", &notes, aliases),
            ["Today → Daily/2026-11-02.md"]
        );
        assert_eq!(
            quick("mon", &notes, aliases),
            ["Monday plan → Daily/2026-11-02.md"]
        );
        // The empty box is unchanged: notes by path, no alias rows.
        assert_eq!(
            quick("", &notes, aliases),
            ["Other.md", "Daily/2026-11-02.md", "Meeting notes.md"]
        );
    }

    #[test]
    fn note_matching_by_name_gets_no_alias_row() {
        let notes = ["Meeting notes.md", "Other.md"];
        let aliases: &[(&str, &[&str])] = &[("Meeting notes.md", &["Meetings", "Weekly sync"])];
        assert_eq!(quick("meet", &notes, aliases), ["Meeting notes.md"]);
        assert_eq!(
            quick("weekly", &notes, aliases),
            ["Weekly sync → Meeting notes.md"]
        );
    }

    #[test]
    fn note_named_like_an_alias_comes_first() {
        let notes = ["Project X.md", "Plans/Roadmap.md", "Projects old.md"];
        let aliases: &[(&str, &[&str])] = &[("Plans/Roadmap.md", &["Project X", "Xylo"])];
        assert_eq!(
            quick("project x", &notes, aliases),
            ["Project X.md", "Project X → Plans/Roadmap.md"]
        );
        // Same kind of match: names first; a better kind of match still wins.
        assert_eq!(
            quick("proj", &notes, aliases),
            [
                "Project X.md",
                "Projects old.md",
                "Project X → Plans/Roadmap.md"
            ]
        );
        assert_eq!(
            quick("x", &notes, aliases),
            ["Xylo → Plans/Roadmap.md", "Project X.md"]
        );
    }

    #[test]
    fn creates_a_note_only_when_no_name_or_alias_matches() {
        // Open Quickly's Enter creates a note only when the list is empty.
        let notes = ["Meeting notes.md"];
        let aliases: &[(&str, &[&str])] = &[("Meeting notes.md", &["Standup"])];
        assert!(!quick("standup", &notes, aliases).is_empty());
        assert!(!quick("meeting", &notes, aliases).is_empty());
        assert!(quick("retro", &notes, aliases).is_empty());
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

    /// Index build with headings, and a heading query against a name query, over a generated
    /// folder (`examples/generate_notes.rs`):
    /// `MM_QUICK_NOTES=<dir> cargo test --release quick::tests::time_headings -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn time_headings() {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "md") {
                    out.push(path);
                }
            }
        }
        let root = PathBuf::from(std::env::var("MM_QUICK_NOTES").expect("MM_QUICK_NOTES"));
        let mut notes = Vec::new();
        walk(&root, &mut notes);
        notes.sort();
        let start = std::time::Instant::now();
        let mut index = crate::backlinks::LinkIndex::default();
        index.sync(&notes, &Aliases::default(), &root, &HashSet::new());
        println!("{} notes indexed in {:?}", notes.len(), start.elapsed());
        let start = std::time::Instant::now();
        let all = index.headings(None);
        let count: usize = all.iter().map(|(_, h)| h.len()).sum();
        println!("{count} headings copied in {:?}", start.elapsed());
        for query in ["#budget", "#zzqx", "#n", "budget", "zzqx", "n"] {
            let start = std::time::Instant::now();
            let found = match heading_query(query) {
                Some(q) => headings(&q, &all).len(),
                None => search(query, &notes, &root).len(),
            };
            println!("{query:>8}: {found} rows in {:?}", start.elapsed());
        }
    }
}
