//! Backlinks (under the Outline, Alt+Ctrl+O): the notes that link to the open note, by
//! `[[wiki]]` link (same resolution as the preview) or a relative Markdown link to its file.
//! Pure logic, no GTK; the window keeps a `LinkIndex` and syncs and queries it on a
//! background thread.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::links::{self, Target};
use crate::render::WikiIndex;
use crate::{lists, state};

/// Snippets longer than this (in chars) are cut.
const SNIPPET_CHARS: usize = 100;

/// One line of another note that links to the target note.
#[derive(Debug, PartialEq)]
pub struct Backlink {
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// The trimmed line, cut to SNIPPET_CHARS with "…".
    pub snippet: String,
}

/// Lines of `notes` (other than `target`) linking to `target`, in `notes` order, reading
/// every note: the reference the index is checked against.
#[cfg(test)]
pub fn find(notes: Vec<PathBuf>, root: &Path, target: &Path) -> Vec<Backlink> {
    let index = WikiIndex::new(notes, root);
    let mut out = Vec::new();
    for note in index.notes().iter().filter(|n| n.as_path() != target) {
        let Some(text) = read(note) else {
            continue;
        };
        for line in lines_linking(&text, note, target, &index) {
            let full = text.lines().nth(line - 1).unwrap_or("").trim();
            out.push(Backlink {
                path: note.clone(),
                line,
                snippet: cut(full),
            });
        }
    }
    out
}

/// The note's text; None if unreadable or not UTF-8.
fn read(note: &Path) -> Option<String> {
    std::fs::read(note)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
}

/// A line of a note with at least one link (outside code): its 1-based number, snippet, the
/// wiki names it links to and the files its Markdown links point to.
#[derive(Debug)]
struct LinkLine {
    line: usize,
    snippet: String,
    wiki: Vec<String>,
    files: Vec<PathBuf>,
}

/// The lines of `text` (the note at `note`) holding links, outside fenced code and code spans.
fn link_lines(text: &str, note: &Path) -> Vec<LinkLine> {
    let mut out = Vec::new();
    let mut open = None;
    for (i, line) in text.lines().enumerate() {
        if lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        let (mut wiki, mut files) = (Vec::new(), Vec::new());
        for (_, t) in links::spans(line) {
            match t {
                Target::Wiki(name) => wiki.push(name),
                Target::Href(href) => files.extend(href_path(&href, note)),
            }
        }
        if !wiki.is_empty() || !files.is_empty() {
            out.push(LinkLine {
                line: i + 1,
                snippet: cut(line.trim()),
                wiki,
                files,
            });
        }
    }
    out
}

/// Every note's links, read once and kept up to date, so the Backlinks of a note are a lookup
/// instead of a read of the whole folder. Wiki links are resolved through a `WikiIndex` of the
/// current note list; when that list changes (a note created, renamed, moved or deleted) every
/// link is resolved again, from memory.
#[derive(Debug, Default)]
pub struct LinkIndex {
    root: PathBuf,
    wiki: WikiIndex,
    /// Position of each note in the note list (Backlinks keep that order).
    order: HashMap<PathBuf, usize>,
    /// The link lines of each note of the list (none if unreadable).
    lines: HashMap<PathBuf, Vec<LinkLine>>,
    /// Linked note → (position of the linking note, 1-based line), unsorted.
    back: HashMap<PathBuf, Vec<(usize, usize)>>,
}

impl LinkIndex {
    /// Brings the index in line with the note list `notes` of the folder `root`: notes new to
    /// it are read, notes no longer in it dropped, and `changed` notes read again (their text
    /// changed on disk). Reads nothing else.
    pub fn sync(&mut self, notes: &[PathBuf], root: &Path, changed: &HashSet<PathBuf>) {
        if self.root != root {
            *self = LinkIndex {
                root: root.to_path_buf(),
                ..LinkIndex::default()
            };
        }
        if notes != self.wiki.notes() {
            self.wiki = WikiIndex::new(notes.to_vec(), root);
            self.order = notes.iter().cloned().zip(0..).collect();
            let order = &self.order;
            self.lines.retain(|note, _| order.contains_key(note));
            for note in notes {
                if changed.contains(note) || !self.lines.contains_key(note) {
                    self.lines.insert(note.clone(), Self::scan(note));
                }
            }
            self.back.clear();
            for note in notes {
                self.link(note, true);
            }
            return;
        }
        for note in changed {
            if !self.order.contains_key(note) {
                continue;
            }
            self.link(note, false);
            self.lines.insert(note.clone(), Self::scan(note));
            self.link(note, true);
        }
    }

    fn scan(note: &Path) -> Vec<LinkLine> {
        read(note).map_or_else(Vec::new, |text| link_lines(&text, note))
    }

    /// Adds (or removes) `note`'s links to `back`.
    fn link(&mut self, note: &Path, add: bool) {
        let (Some(&at), Some(lines)) = (self.order.get(note), self.lines.get(note)) else {
            return;
        };
        for l in lines {
            let mut targets: Vec<&PathBuf> = l.files.iter().collect();
            targets.extend(
                l.wiki
                    .iter()
                    .filter_map(|n| self.wiki.resolve(n, Some(note))),
            );
            targets.sort();
            targets.dedup();
            for target in targets {
                let entry = (at, l.line);
                if add {
                    self.back.entry(target.clone()).or_default().push(entry);
                } else if let Some(from) = self.back.get_mut(target) {
                    from.retain(|e| *e != entry);
                }
            }
        }
    }

    /// The lines of other notes linking to `target`, in note list order: the same as `find`
    /// over the synced note list, without reading a file.
    pub fn backlinks(&self, target: &Path) -> Vec<Backlink> {
        let mut from = self.back.get(target).cloned().unwrap_or_default();
        from.sort_unstable();
        from.dedup();
        let notes = self.wiki.notes();
        from.into_iter()
            .map(|(at, line)| (&notes[at], line))
            .filter(|(note, _)| note.as_path() != target)
            .filter_map(|(note, line)| {
                let lines = self.lines.get(note)?;
                let i = lines.binary_search_by_key(&line, |l| l.line).ok()?;
                Some(Backlink {
                    path: note.clone(),
                    line,
                    snippet: lines[i].snippet.clone(),
                })
            })
            .collect()
    }
}

/// 1-based lines of `text` (the note at `note`) with a link to `target`, outside fenced code
/// and code spans.
#[cfg(test)]
pub fn lines_linking(text: &str, note: &Path, target: &Path, index: &WikiIndex) -> Vec<usize> {
    let mut out = Vec::new();
    let mut open = None;
    for (i, line) in text.lines().enumerate() {
        if lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        let hit = links::spans(line).into_iter().any(|(_, t)| match t {
            Target::Wiki(name) => {
                index.resolve(&name, Some(note)).map(PathBuf::as_path) == Some(target)
            }
            Target::Href(href) => href_path(&href, note).as_deref() == Some(target),
        });
        if hit {
            out.push(i + 1);
        }
    }
    out
}

/// The file a relative (or absolute-path) link in `note` points to; None for URLs.
fn href_path(href: &str, note: &Path) -> Option<PathBuf> {
    let href = href.split(['#', '?']).next()?;
    // `scheme:` before any `/` is a URL (http:, mailto:, file:…).
    if href.split('/').next()?.contains(':') {
        return None;
    }
    let path = note.parent()?.join(state::unescape(href)?);
    let mut clean = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            c => clean.push(c),
        }
    }
    Some(clean)
}

fn cut(line: &str) -> String {
    match line.char_indices().nth(SNIPPET_CHARS) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, note: &str) -> Vec<usize> {
        let root = Path::new("/n");
        let notes: Vec<PathBuf> = ["/n/Name.md", "/n/Sub/Other.md", "/n/Sub/Name.md", "/n/A.md"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let index = WikiIndex::new(notes, root);
        lines_linking(text, Path::new(note), Path::new("/n/Name.md"), &index)
    }

    #[test]
    fn wiki_links() {
        assert_eq!(lines("x\n[[Name]] y", "/n/A.md"), [2]);
        assert_eq!(lines("[[name|title]]", "/n/A.md"), [1]);
        assert_eq!(lines("[[Name#Part]]", "/n/A.md"), [1]);
        assert_eq!(lines("[[Name.md]] and [[Name]]", "/n/A.md"), [1]);
        // From Sub/, `[[Name]]` resolves to the note in the same folder first.
        assert_eq!(lines("[[Name]]", "/n/Sub/Other.md"), Vec::<usize>::new());
        assert_eq!(lines("[[Missing]] [[A]]", "/n/A.md"), Vec::<usize>::new());
    }

    #[test]
    fn markdown_links() {
        assert_eq!(lines("[a](../Name.md)", "/n/Sub/Other.md"), [1]);
        assert_eq!(lines("[a](Name.md#top)", "/n/A.md"), [1]);
        assert_eq!(lines("[a](./Sub/../Name.md)", "/n/A.md"), [1]);
        assert_eq!(lines("[a](<Name.md>)", "/n/A.md"), [1]);
        assert_eq!(lines("[a](Sub/Name.md)", "/n/A.md"), Vec::<usize>::new());
        assert_eq!(
            lines("[a](https://x/Name.md)", "/n/A.md"),
            Vec::<usize>::new()
        );
        let target = PathBuf::from("/n/My Note.md");
        let index = WikiIndex::new(vec![target.clone()], Path::new("/n"));
        let found = |t: &str| lines_linking(t, Path::new("/n/A.md"), &target, &index);
        assert_eq!(found("[a](My%20Note.md)"), [1]);
    }

    #[test]
    fn code_is_skipped() {
        assert_eq!(
            lines("`[[Name]]`\n```\n[[Name]]\n```\n", "/n/A.md"),
            Vec::<usize>::new()
        );
        assert_eq!(lines("~~~\n[a](Name.md)\n~~~\n[[Name]]", "/n/A.md"), [4]);
    }

    #[test]
    fn find_skips_the_note_itself() {
        let dir = std::env::temp_dir().join(format!("mm-backlinks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (name, a) = (dir.join("Name.md"), dir.join("A.md"));
        std::fs::write(&name, "[[Name]] self\n").unwrap();
        let long = format!("  see [[Name]] {}", "x".repeat(200));
        std::fs::write(&a, format!("intro\n{long}\n")).unwrap();
        let notes = vec![a.clone(), name.clone()];
        let found = find(notes, &dir, &name);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].path.as_path(), found[0].line), (a.as_path(), 2));
        assert!(found[0].snippet.starts_with("see [[Name]] x"));
        assert_eq!(found[0].snippet.chars().count(), SNIPPET_CHARS + 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A folder whose notes link to each other in every supported way.
    fn sample(dir: &Path) -> Vec<PathBuf> {
        let notes: Vec<PathBuf> = [
            "Name.md",
            "A.md",
            "Sub/Other.md",
            "Sub/Name.md",
            "Sub/Deep/B.md",
            "My Note.md",
        ]
        .iter()
        .map(|n| dir.join(n))
        .collect();
        let texts = [
            "[[Name]] self\n[[A]] and [[Sub/Name]]\n",
            "x\n[[Name]] y [[name|t]]\n```\n[[Name]]\n```\n[a](Sub/Other.md) `[[A]]`\n",
            "[[Name]] from here\n[up](../Name.md) [[My Note]]\n[[Missing]]\n",
            "[[Other#Part]]\n[a](<../My Note.md>)\n",
            "[[Name]] [[B]]\n[a](../../A.md?x) [b](https://x/A.md)\n",
            "[a](My%20Note.md) [[Name.md]]\n",
        ];
        for (note, text) in notes.iter().zip(texts) {
            std::fs::create_dir_all(note.parent().unwrap()).unwrap();
            std::fs::write(note, text).unwrap();
        }
        notes
    }

    fn same_as_scan(index: &LinkIndex, notes: &[PathBuf], dir: &Path) {
        for target in notes {
            let scan = find(notes.to_vec(), dir, target);
            assert_eq!(index.backlinks(target), scan, "{}", target.display());
        }
    }

    #[test]
    fn index_equals_the_full_scan() {
        let dir = std::env::temp_dir().join(format!("mm-linkindex-{}", std::process::id()));
        let mut notes = sample(&dir);
        let mut index = LinkIndex::default();
        let none = HashSet::new();
        index.sync(&notes, &dir, &none);
        same_as_scan(&index, &notes, &dir);
        assert!(!index.backlinks(&dir.join("Name.md")).is_empty());

        // An edit on disk, reported as changed.
        let a = dir.join("A.md");
        std::fs::write(&a, "[[Sub/Name]]\n\n[[My Note]]\n").unwrap();
        index.sync(&notes, &dir, &HashSet::from([a.clone()]));
        same_as_scan(&index, &notes, &dir);

        // A rename: `[[Name]]` from the root now resolves to Sub/Name.md.
        let (old, new) = (dir.join("Name.md"), dir.join("Renamed.md"));
        std::fs::rename(&old, &new).unwrap();
        notes[0] = new.clone();
        index.sync(&notes, &dir, &none);
        same_as_scan(&index, &notes, &dir);
        // Markdown links to the old path still point there, as in the scan.
        assert_eq!(index.backlinks(&old), find(notes.clone(), &dir, &old));

        // A move into a folder, a new note and a deleted one.
        let moved = dir.join("Sub/Deep/A.md");
        std::fs::rename(&a, &moved).unwrap();
        notes[1] = moved;
        let added = dir.join("Name.md");
        std::fs::write(&added, "[[B]]\n").unwrap();
        notes.push(added);
        std::fs::remove_file(&notes[2]).unwrap();
        notes.remove(2);
        index.sync(&notes, &dir, &none);
        same_as_scan(&index, &notes, &dir);

        // Another folder starts over.
        let other = dir.join("Sub");
        let inside: Vec<PathBuf> = notes
            .iter()
            .filter(|n| n.starts_with(&other))
            .cloned()
            .collect();
        index.sync(&inside, &other, &none);
        same_as_scan(&index, &inside, &other);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
