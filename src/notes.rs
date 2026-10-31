//! Scans a folder into a tree of notes. Pure Rust (no GTK) so it is testable headless.
//!
//! Rules: only subfolders and `.md`/`.markdown` files (case-insensitive extension); names
//! starting with `.` and `node_modules` folders are skipped; entries are in the sidebar's sort
//! order (`SortOrder`, `compare`); folders with no note anywhere inside are hidden. Symlinked folders are not followed (so a
//! symlink loop cannot hang the scan); symlinks to note files are kept.

use std::cmp::Ordering;
use std::fs;
use std::io;
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// A note's `date` is the one its folder's order sorts by (`SortOrder::date`); None under
/// Name, or when it could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum Node {
    Dir {
        name: String,
        children: Vec<Node>,
    },
    Note {
        name: String,
        path: PathBuf,
        date: Option<SystemTime>,
    },
}

/// How the sidebar is sorted (main menu ▸ Sort Sidebar By, or Sort By in the sidebar's
/// right-click menu): one setting for the whole app, `sidebar_sort=` in the state file.
///
/// The rule, the same at every level of the tree (as the macOS app):
/// 1. Folders before notes; folders always by name, whatever the order.
/// 2. Notes by the chosen order: Name A→Z, or Date Modified / Date Created newest first.
/// 3. Names compare naturally (`compare_names`: "Note 2" before "Note 10", case and accents
///    loose); notes with the same date go by name, and a note without one (read failed) goes
///    after those with one.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    #[default]
    Name,
    Modified,
    Created,
}

impl SortOrder {
    pub const ALL: [SortOrder; 3] = [SortOrder::Name, SortOrder::Modified, SortOrder::Created];

    /// Its value in the state file and in the `win.sidebar-sort` action.
    pub fn name(self) -> &'static str {
        match self {
            SortOrder::Name => "name",
            SortOrder::Modified => "modified",
            SortOrder::Created => "created",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            SortOrder::Name => "Name",
            SortOrder::Modified => "Date Modified",
            SortOrder::Created => "Date Created",
        }
    }

    pub fn from_name(name: &str) -> Option<SortOrder> {
        SortOrder::ALL.into_iter().find(|o| o.name() == name)
    }

    /// The date of a file with `meta` this order sorts by; None under Name (not read at all).
    /// Date Created is the birth time where the file system keeps one, else the modified date.
    pub fn date(self, meta: &fs::Metadata) -> Option<SystemTime> {
        match self {
            SortOrder::Name => None,
            SortOrder::Modified => meta.modified().ok(),
            SortOrder::Created => meta.created().or_else(|_| meta.modified()).ok(),
        }
    }
}

/// What `compare` needs of a sidebar entry.
#[derive(Debug, Clone, Copy)]
pub struct Key<'a> {
    pub is_note: bool,
    pub date: Option<SystemTime>,
    pub name: &'a str,
}

/// Where `a` goes relative to `b` in `order` (the rule on `SortOrder`).
pub fn compare(order: SortOrder, a: Key, b: Key) -> Ordering {
    a.is_note.cmp(&b.is_note).then_with(|| {
        let by_date = if a.is_note && order != SortOrder::Name {
            match (a.date, b.date) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
        } else {
            Ordering::Equal
        };
        by_date.then_with(|| compare_names(a.name, b.name))
    })
}

/// Natural name order, like Files and the macOS Finder: runs of digits compare as numbers
/// ("Note 2" before "Note 10"), letters without case or accents ("é" = "e" = "E"). Names
/// that are equal that way fall back to case-insensitive, then exact order, so the result
/// never depends on the listing order.
pub fn compare_names(a: &str, b: &str) -> Ordering {
    let mut x = folded(a).peekable();
    let mut y = folded(b).peekable();
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let (n, m) = (digits(&mut x), digits(&mut y));
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let by_number = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if by_number != Ordering::Equal {
                    return by_number;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

/// `name` lower-cased without accents (decomposed, combining marks dropped).
pub(crate) fn folded(name: &str) -> impl Iterator<Item = char> + '_ {
    name.nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
}

/// The run of ASCII digits at the front of `chars`, taken from it.
fn digits(chars: &mut Peekable<impl Iterator<Item = char>>) -> String {
    let mut run = String::new();
    while let Some(c) = chars.next_if(char::is_ascii_digit) {
        run.push(c);
    }
    run
}

impl Node {
    fn name(&self) -> &str {
        match self {
            Node::Dir { name, .. } | Node::Note { name, .. } => name,
        }
    }

    fn key(&self) -> Key<'_> {
        Key {
            is_note: matches!(self, Node::Note { .. }),
            date: match self {
                Node::Note { date, .. } => *date,
                Node::Dir { .. } => None,
            },
            name: self.name(),
        }
    }
}

/// Tree of `root`'s contents in `order`. Only an unreadable `root` is an error; unreadable
/// subfolders are skipped. Notes' dates are read only when `order` sorts by one (under Name a
/// plain file costs no `stat`, only the folder listing).
pub fn scan(root: &Path, order: SortOrder) -> io::Result<Vec<Node>> {
    let mut nodes = Vec::new();
    for entry in fs::read_dir(root)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_skipped(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            let children = scan(&path, order).unwrap_or_default();
            if !children.is_empty() {
                nodes.push(Node::Dir { name, children });
            }
        } else if is_note(&path) {
            // The listing's file type is enough for a plain file under Name; a symlink is
            // followed (fs::metadata: keeps symlinked notes, drops symlinked folders), and a
            // date order needs the file's dates anyway.
            let date = if kind.is_file() && order == SortOrder::Name {
                None
            } else {
                match fs::metadata(&path) {
                    Ok(meta) if meta.is_file() => order.date(&meta),
                    _ => continue,
                }
            };
            nodes.push(Node::Note { name, path, date });
        }
    }
    nodes.sort_by(|a, b| compare(order, a.key(), b.key()));
    Ok(nodes)
}

/// Names every folder walk ignores: hidden ones and `node_modules` (never notes, often huge).
fn is_skipped(name: &str) -> bool {
    name.starts_with('.') || name == "node_modules"
}

/// `root` and every folder under it, skipping hidden names, `node_modules` and symlinked folders like `scan`
/// (empty folders included: a note may appear in them later). The folders to watch.
pub fn folders(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut i = 0;
    while let Some(dir) = out.get(i).cloned() {
        i += 1;
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let skipped = is_skipped(&entry.file_name().to_string_lossy());
            // file_type does not follow symlinks.
            if !skipped && entry.file_type().is_ok_and(|t| t.is_dir()) {
                out.push(entry.path());
            }
        }
    }
    out
}

/// Whether scan number `scan_gen` of `dir` should be shown. Scans run in the background and
/// may finish out of order: drop one older than the last shown (`applied`), or of a folder
/// other than the one asked for last (`wanted`).
pub fn is_fresh(scan_gen: u64, applied: u64, dir: &Path, wanted: Option<&Path>) -> bool {
    scan_gen > applied && wanted == Some(dir)
}

pub fn is_note(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh empty folder under the system temp dir, removed on drop.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mdmate-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn file(&self, rel: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Tree as indented names, e.g. "sub/\n  a.md\nb.md".
    fn outline(nodes: &[Node], depth: usize, out: &mut Vec<String>) {
        for n in nodes {
            let pad = "  ".repeat(depth);
            match n {
                Node::Dir { name, children } => {
                    out.push(format!("{pad}{name}/"));
                    outline(children, depth + 1, out);
                }
                Node::Note { name, .. } => out.push(format!("{pad}{name}")),
            }
        }
    }
    fn names(root: &Path) -> Vec<String> {
        let mut out = Vec::new();
        outline(&scan(root, SortOrder::Name).unwrap(), 0, &mut out);
        out
    }

    #[test]
    fn skips_node_modules_at_any_level() {
        assert!(is_skipped("node_modules") && is_skipped(".git"));
        assert!(!is_skipped("node_modules2") && !is_skipped("Node_Modules.md"));
        let t = TempDir::new("nodemod");
        for f in [
            "a.md",
            "node_modules/x.md",
            "sub/node_modules/y.md",
            "sub/b.md",
        ] {
            t.file(f);
        }
        assert_eq!(names(&t.0), ["sub/", "  b.md", "a.md"]);
        assert_eq!(folders(&t.0), [t.0.clone(), t.0.join("sub")]);
    }

    #[test]
    fn keeps_only_markdown_and_skips_hidden() {
        let t = TempDir::new("filter");
        for f in [
            "a.md",
            "B.MARKDOWN",
            "c.txt",
            "d",
            ".hidden.md",
            ".git/x.md",
            "img.png",
        ] {
            t.file(f);
        }
        assert_eq!(names(&t.0), ["a.md", "B.MARKDOWN"]);
    }

    #[test]
    fn folders_first_then_case_insensitive_names() {
        let t = TempDir::new("sort");
        for f in ["b.md", "A.md", "c.md", "zeta/n.md", "Alpha/n.md"] {
            t.file(f);
        }
        assert_eq!(
            names(&t.0),
            [
                "Alpha/", "  n.md", "zeta/", "  n.md", "A.md", "b.md", "c.md"
            ]
        );
    }

    #[test]
    fn nested_folders_and_empty_folders_hidden() {
        let t = TempDir::new("nested");
        t.file("a/b/c/deep.md");
        t.file("a/top.md");
        t.file("only-txt/x.txt");
        fs::create_dir_all(t.0.join("empty/inner")).unwrap();
        assert_eq!(
            names(&t.0),
            ["a/", "  b/", "    c/", "      deep.md", "  top.md"]
        );
        let Node::Dir { children, .. } = &scan(&t.0, SortOrder::Name).unwrap()[0] else {
            panic!("expected dir")
        };
        let Node::Note { path, .. } = &children[1] else {
            panic!("expected note")
        };
        assert_eq!(path, &t.0.join("a/top.md"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_loop_does_not_hang_and_file_links_are_kept() {
        use std::os::unix::fs::symlink;
        let t = TempDir::new("symlink");
        t.file("sub/n.md");
        t.file("real.md");
        symlink(&t.0, t.0.join("sub/loop")).unwrap();
        symlink(t.0.join("real.md"), t.0.join("link.md")).unwrap();
        symlink(t.0.join("missing.md"), t.0.join("broken.md")).unwrap();
        // A folder link named like a note is not a note either.
        symlink(t.0.join("sub"), t.0.join("folder.md")).unwrap();
        assert_eq!(names(&t.0), ["sub/", "  n.md", "link.md", "real.md"]);
        // The looping folder link is not watched either.
        assert_eq!(folders(&t.0), [t.0.clone(), t.0.join("sub")]);
    }

    #[test]
    fn folders_include_empty_and_skip_hidden() {
        let t = TempDir::new("folders");
        t.file("a/b/n.md");
        t.file(".hidden/n.md");
        fs::create_dir_all(t.0.join("empty")).unwrap();
        let mut got = folders(&t.0);
        got.sort();
        assert_eq!(
            got,
            [
                t.0.clone(),
                t.0.join("a"),
                t.0.join("a/b"),
                t.0.join("empty")
            ]
        );
    }

    #[test]
    fn only_newer_scans_of_the_wanted_folder_are_fresh() {
        let (a, b) = (Path::new("/a"), Path::new("/b"));
        assert!(is_fresh(2, 1, a, Some(a)));
        // Scan 1 finishing after scan 2 was shown.
        assert!(!is_fresh(1, 2, a, Some(a)));
        // Folder A's rescan finishing after the user opened folder B.
        assert!(!is_fresh(3, 1, a, Some(b)));
    }

    #[test]
    fn missing_root_is_an_error() {
        assert!(scan(Path::new("/definitely/not/here"), SortOrder::Name).is_err());
    }

    /// Seconds before an arbitrary fixed moment.
    fn ago(secs: Option<u64>) -> Option<SystemTime> {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        secs.map(|s| now - std::time::Duration::from_secs(s))
    }

    /// The macOS app's test set: (name, is folder, modified, created), seconds ago.
    const ENTRIES: [(&str, bool, Option<u64>, Option<u64>); 6] = [
        ("Note 10.md", false, Some(10), Some(300)),
        ("zeta", true, None, None),
        ("note 2.md", false, Some(100), Some(100)),
        ("b.md", false, Some(1), Some(200)),
        ("Alpha", true, None, None),
        ("a.md", false, Some(10), None),
    ];

    fn sorted(order: SortOrder) -> Vec<&'static str> {
        let mut entries: Vec<_> = ENTRIES
            .iter()
            .map(|&(name, folder, modified, created)| {
                let date = match order {
                    SortOrder::Name => None,
                    SortOrder::Modified => ago(modified),
                    SortOrder::Created => ago(created),
                };
                (
                    name,
                    Key {
                        is_note: !folder,
                        date,
                        name,
                    },
                )
            })
            .collect();
        entries.sort_by(|a, b| compare(order, a.1, b.1));
        entries.into_iter().map(|(name, _)| name).collect()
    }

    #[test]
    fn name_order_is_natural_with_folders_first() {
        assert_eq!(
            sorted(SortOrder::Name),
            ["Alpha", "zeta", "a.md", "b.md", "note 2.md", "Note 10.md"]
        );
    }

    #[test]
    fn date_modified_newest_first_ties_by_name() {
        assert_eq!(
            sorted(SortOrder::Modified),
            ["Alpha", "zeta", "b.md", "a.md", "Note 10.md", "note 2.md"]
        );
    }

    #[test]
    fn date_created_newest_first_missing_date_last() {
        assert_eq!(
            sorted(SortOrder::Created),
            ["Alpha", "zeta", "note 2.md", "b.md", "Note 10.md", "a.md"]
        );
    }

    #[test]
    fn names_compare_naturally_without_case_or_accents() {
        use Ordering::*;
        for (a, b, want) in [
            ("Note 2", "Note 10", Less),
            ("note 10", "Note 9", Greater),
            ("Note 007", "Note 7", Less), // same number: case-insensitive, then exact text
            ("x2y10", "x2y9", Greater),
            ("café", "Cafe b", Less),
            ("École", "eclair", Greater),
            ("Été", "ete", Greater), // equal folded: then lower-case text, é after e
            ("a", "A", Greater),
            ("a", "a", Equal),
            ("abc", "ab", Greater),
            ("2026-10-09.md", "2026-10-10.md", Less),
        ] {
            assert_eq!(compare_names(a, b), want, "{a} vs {b}");
            assert_eq!(compare_names(b, a), want.reverse(), "{b} vs {a}");
        }
    }

    #[test]
    fn sort_order_names_round_trip() {
        for order in SortOrder::ALL {
            assert_eq!(SortOrder::from_name(order.name()), Some(order));
        }
        assert_eq!(SortOrder::from_name("size"), None);
        assert_eq!(SortOrder::default(), SortOrder::Name);
    }

    #[test]
    fn scan_reads_only_the_date_the_order_needs() {
        let t = TempDir::new("dates");
        t.file("folder/n.md");
        for (name, secs) in [("old.md", 300), ("new.md", 1), ("middle.md", 60)] {
            t.file(name);
            let file = fs::File::options()
                .write(true)
                .open(t.0.join(name))
                .unwrap();
            file.set_modified(ago(Some(secs)).unwrap()).unwrap();
        }
        let notes = |order| {
            let mut out = Vec::new();
            for node in scan(&t.0, order).unwrap() {
                match node {
                    Node::Dir { name, .. } => out.push((name, None)),
                    Node::Note { name, date, .. } => out.push((name, date)),
                }
            }
            out
        };
        let by_date = notes(SortOrder::Modified);
        let names: Vec<_> = by_date.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["folder", "new.md", "middle.md", "old.md"]);
        assert_eq!(by_date[1].1, ago(Some(1)));
        let by_name = notes(SortOrder::Name);
        let names: Vec<_> = by_name.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["folder", "middle.md", "new.md", "old.md"]);
        assert!(by_name.iter().all(|(_, date)| date.is_none()));
        // Birth time where the file system has one, else the modified date: never missing.
        assert!(
            notes(SortOrder::Created)[1..]
                .iter()
                .all(|(_, d)| d.is_some())
        );
    }
}
