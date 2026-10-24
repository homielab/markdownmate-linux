//! Scans a folder into a tree of notes. Pure Rust (no GTK) so it is testable headless.
//!
//! Rules: only subfolders and `.md`/`.markdown` files (case-insensitive extension); names
//! starting with `.` and `node_modules` folders are skipped; folders come first, then names sorted case-insensitively;
//! folders with no note anywhere inside are hidden. Symlinked folders are not followed (so a
//! symlink loop cannot hang the scan); symlinks to note files are kept.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Eq)]
pub enum Node {
    Dir { name: String, children: Vec<Node> },
    Note { name: String, path: PathBuf },
}

impl Node {
    fn name(&self) -> &str {
        match self {
            Node::Dir { name, .. } | Node::Note { name, .. } => name,
        }
    }
}

/// Tree of `root`'s contents. Only an unreadable `root` is an error; unreadable subfolders
/// are skipped.
pub fn scan(root: &Path) -> io::Result<Vec<Node>> {
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
            let children = scan(&path).unwrap_or_default();
            if !children.is_empty() {
                nodes.push(Node::Dir { name, children });
            }
        // fs::metadata follows the link: keeps symlinked notes, drops symlinked folders.
        } else if is_note(&path) && fs::metadata(&path).is_ok_and(|m| m.is_file()) {
            nodes.push(Node::Note { name, path });
        }
    }
    nodes.sort_by(|a, b| {
        let is_note = |n: &Node| matches!(n, Node::Note { .. });
        (is_note(a), a.name().to_lowercase(), a.name()).cmp(&(
            is_note(b),
            b.name().to_lowercase(),
            b.name(),
        ))
    });
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
        outline(&scan(root).unwrap(), 0, &mut out);
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
        let Node::Dir { children, .. } = &scan(&t.0).unwrap()[0] else {
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
        assert!(scan(Path::new("/definitely/not/here")).is_err());
    }
}
