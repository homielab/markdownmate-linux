//! Backlinks (under the Outline, Alt+Ctrl+O): the notes that link to the open note, by
//! `[[wiki]]` link (same resolution as the preview) or a relative Markdown link to its file.
//! Pure logic, no GTK; the window keeps a `LinkIndex` and syncs and queries it on a
//! background thread.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::aliases::Aliases;
use crate::links::{self, Target};
use crate::render::WikiIndex;
use crate::{lists, quick, state};

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
pub fn find(notes: Vec<PathBuf>, aliases: &Aliases, root: &Path, target: &Path) -> Vec<Backlink> {
    let index = WikiIndex::with_aliases(notes, root, aliases);
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
/// current note list and its front-matter aliases; when either changes (a note created,
/// renamed, moved or deleted, an alias added or removed) every link is resolved again, from
/// memory.
#[derive(Debug, Default)]
pub struct LinkIndex {
    root: PathBuf,
    wiki: WikiIndex,
    /// `Aliases::fingerprint` of the aliases `wiki` resolves with.
    aliases: u64,
    /// Position of each note in the note list (Backlinks keep that order).
    order: HashMap<PathBuf, usize>,
    /// The link lines of each note of the list (none if unreadable).
    lines: HashMap<PathBuf, Vec<LinkLine>>,
    /// The headings of each note of the list that has any (Open Quickly's `#` search).
    headings: HashMap<PathBuf, Vec<quick::Heading>>,
    /// Linked note → (position of the linking note, 1-based line), unsorted.
    back: HashMap<PathBuf, Vec<(usize, usize)>>,
}

impl LinkIndex {
    /// Brings the index in line with the note list `notes` of the folder `root` and their
    /// `aliases`: notes new to it are read, notes no longer in it dropped, and `changed` notes
    /// read again (their text changed on disk). Reads nothing else.
    pub fn sync(
        &mut self,
        notes: &[PathBuf],
        aliases: &Aliases,
        root: &Path,
        changed: &HashSet<PathBuf>,
    ) {
        if self.root != root {
            *self = LinkIndex {
                root: root.to_path_buf(),
                ..LinkIndex::default()
            };
        }
        if notes != self.wiki.notes() || aliases.fingerprint() != self.aliases {
            self.wiki = WikiIndex::with_aliases(notes.to_vec(), root, aliases);
            self.aliases = aliases.fingerprint();
            self.order = notes.iter().cloned().zip(0..).collect();
            let order = &self.order;
            self.lines.retain(|note, _| order.contains_key(note));
            self.headings.retain(|note, _| order.contains_key(note));
            for note in notes {
                if changed.contains(note) || !self.lines.contains_key(note) {
                    self.scan(note);
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
            self.scan(note);
            self.link(note, true);
        }
    }

    /// Reads `note`'s link lines and headings (none if unreadable).
    fn scan(&mut self, note: &Path) {
        let text = read(note);
        let text = text.as_deref().unwrap_or("");
        self.lines
            .insert(note.to_path_buf(), link_lines(text, note));
        let headings = if might_have_headings(text) {
            quick::Heading::all(text)
        } else {
            Vec::new()
        };
        if headings.is_empty() {
            self.headings.remove(note);
        } else {
            self.headings.insert(note.to_path_buf(), headings);
        }
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

impl LinkIndex {
    /// The wiki resolver of the synced notes and aliases.
    pub fn wiki(&self) -> &WikiIndex {
        &self.wiki
    }

    /// Every note's headings, in note list order (notes without any left out), as they were
    /// on disk at the last sync; the `open` note's (path, editor text) read from that text.
    pub fn headings(&self, open: Option<(&Path, &str)>) -> Vec<quick::NoteHeadings> {
        self.wiki
            .notes()
            .iter()
            .filter_map(|note| {
                let list = match open {
                    Some((path, text)) if path == note => quick::Heading::all(text),
                    _ => self.headings.get(note)?.clone(),
                };
                (!list.is_empty()).then(|| (note.clone(), list))
            })
            .collect()
    }

    /// The notes with a link that may lead nowhere: a wiki name that resolves to no note or
    /// has a heading part, or a file that `exists` says is missing. Find Broken Links reads
    /// only these (and checks what this does not know: front matter, absolute paths, headings).
    pub fn suspects(&self, exists: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for note in self.wiki.notes() {
            let Some(lines) = self.lines.get(note) else {
                continue;
            };
            let suspect = lines.iter().any(|l| {
                l.wiki
                    .iter()
                    .any(|n| n.contains('#') || self.wiki.resolve(n, Some(note)).is_none())
                    || l.files.iter().any(|f| !exists(f))
            });
            if suspect {
                out.push(note.clone());
            }
        }
        out
    }
}

/// Whether `text` may hold a heading: a `#`, or a line of only `=` or `-` (a Setext
/// underline), past quote markers and indentation. Most notes without headings skip parsing.
fn might_have_headings(text: &str) -> bool {
    text.contains('#')
        || text.lines().any(|line| {
            let t = line
                .trim_start_matches(|c: char| c == '>' || c.is_whitespace())
                .trim_end();
            !t.is_empty() && (t.bytes().all(|b| b == b'=') || t.bytes().all(|b| b == b'-'))
        })
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
pub(crate) fn href_path(href: &str, note: &Path) -> Option<PathBuf> {
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
        let found = find(notes, &Aliases::default(), &dir, &name);
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
        same_with_aliases(index, notes, &Aliases::default(), dir);
    }

    fn same_with_aliases(index: &LinkIndex, notes: &[PathBuf], aliases: &Aliases, dir: &Path) {
        for target in notes {
            let scan = find(notes.to_vec(), aliases, dir, target);
            assert_eq!(index.backlinks(target), scan, "{}", target.display());
        }
    }

    #[test]
    fn index_equals_the_full_scan() {
        let dir = std::env::temp_dir().join(format!("mm-linkindex-{}", std::process::id()));
        let mut notes = sample(&dir);
        let mut index = LinkIndex::default();
        let none = HashSet::new();
        index.sync(&notes, &Aliases::default(), &dir, &none);
        same_as_scan(&index, &notes, &dir);
        assert!(!index.backlinks(&dir.join("Name.md")).is_empty());

        // An edit on disk, reported as changed.
        let a = dir.join("A.md");
        std::fs::write(&a, "[[Sub/Name]]\n\n[[My Note]]\n").unwrap();
        index.sync(
            &notes,
            &Aliases::default(),
            &dir,
            &HashSet::from([a.clone()]),
        );
        same_as_scan(&index, &notes, &dir);

        // A rename: `[[Name]]` from the root now resolves to Sub/Name.md.
        let (old, new) = (dir.join("Name.md"), dir.join("Renamed.md"));
        std::fs::rename(&old, &new).unwrap();
        notes[0] = new.clone();
        index.sync(&notes, &Aliases::default(), &dir, &none);
        same_as_scan(&index, &notes, &dir);
        // Markdown links to the old path still point there, as in the scan.
        assert_eq!(
            index.backlinks(&old),
            find(notes.clone(), &Aliases::default(), &dir, &old)
        );

        // A move into a folder, a new note and a deleted one.
        let moved = dir.join("Sub/Deep/A.md");
        std::fs::rename(&a, &moved).unwrap();
        notes[1] = moved;
        let added = dir.join("Name.md");
        std::fs::write(&added, "[[B]]\n").unwrap();
        notes.push(added);
        std::fs::remove_file(&notes[2]).unwrap();
        notes.remove(2);
        index.sync(&notes, &Aliases::default(), &dir, &none);
        same_as_scan(&index, &notes, &dir);

        // Another folder starts over.
        let other = dir.join("Sub");
        let inside: Vec<PathBuf> = notes
            .iter()
            .filter(|n| n.starts_with(&other))
            .cloned()
            .collect();
        index.sync(&inside, &Aliases::default(), &other, &none);
        same_as_scan(&index, &inside, &other);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn index_keeps_headings_up_to_date() {
        let dir = std::env::temp_dir().join(format!("mm-headingindex-{}", std::process::id()));
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a.md"), dir.join("b.md"), dir.join("c.md"));
        std::fs::write(&a, "# Plan\ntext\n## Budget\n").unwrap();
        std::fs::write(&b, "no headings, [[a]]\n").unwrap();
        std::fs::write(&c, "Setext\n===\n").unwrap();
        let titles = |index: &LinkIndex| -> Vec<(String, Vec<String>)> {
            index
                .headings(None)
                .into_iter()
                .map(|(note, list)| {
                    let name = note.file_name().unwrap().to_string_lossy().to_string();
                    (name, list.into_iter().map(|h| h.title).collect())
                })
                .collect()
        };
        let s = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        let mut notes = vec![a.clone(), b.clone(), c.clone()];
        let mut index = LinkIndex::default();
        let none = HashSet::new();
        index.sync(&notes, &Aliases::default(), &dir, &none);
        assert_eq!(
            titles(&index),
            [
                ("a.md".to_string(), s(&["Plan", "Budget"])),
                ("c.md".to_string(), s(&["Setext"])),
            ]
        );
        // A change on disk, then a deleted note.
        std::fs::write(&a, "# Plan\n").unwrap();
        index.sync(&notes, &Aliases::default(), &dir, &HashSet::from([a]));
        notes.pop();
        index.sync(&notes, &Aliases::default(), &dir, &none);
        assert_eq!(titles(&index), [("a.md".to_string(), s(&["Plan"]))]);
        // The open note's unsaved text wins over the disk.
        let open = index.headings(Some((&notes[1], "# Draft\n")));
        assert_eq!(open[1].0, notes[1]);
        assert_eq!(open[1].1[0].title, "Draft");
        assert!(!might_have_headings("text\n- item\n> quote\n"));
        assert!(might_have_headings("Title\n> ---\n"));
    }

    #[test]
    fn alias_links_are_backlinks() {
        let dir = std::env::temp_dir().join(format!("mm-aliaslinks-{}", std::process::id()));
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(dir.clone());
        std::fs::create_dir_all(&dir).unwrap();
        let (note, a, mn) = (dir.join("Note.md"), dir.join("A.md"), dir.join("MN.md"));
        std::fs::write(&note, "---\naliases: [MN, Meeting notes]\n---\n# Note\n").unwrap();
        std::fs::write(
            &a,
            "[[Meeting notes#Agenda]]\n[[MN|x]]\n[[meeting NOTES|y]]\n",
        )
        .unwrap();
        let notes = vec![note.clone(), a.clone()];
        let aliases = crate::aliases::index(&notes);
        let mut index = LinkIndex::default();
        let none = HashSet::new();
        // Synced before the aliases were read: no alias resolves yet.
        index.sync(&notes, &Aliases::default(), &dir, &none);
        assert!(index.backlinks(&note).is_empty());
        index.sync(&notes, &aliases, &dir, &none);
        let lines: Vec<usize> = index.backlinks(&note).iter().map(|b| b.line).collect();
        assert_eq!(lines, [1, 2, 3]);
        same_with_aliases(&index, &notes, &aliases, &dir);

        // A note named MN takes `[[MN]]` over the alias.
        std::fs::write(&mn, "# MN\n").unwrap();
        let notes = vec![note.clone(), a.clone(), mn.clone()];
        let aliases = crate::aliases::index(&notes);
        index.sync(&notes, &aliases, &dir, &none);
        let lines: Vec<usize> = index.backlinks(&note).iter().map(|b| b.line).collect();
        assert_eq!(lines, [1, 3]);
        assert_eq!(index.backlinks(&mn).len(), 1);
        same_with_aliases(&index, &notes, &aliases, &dir);
    }

    #[test]
    fn embeds_are_backlinks() {
        let folder = crate::render::TestFolder::new(
            "backlinks",
            &[
                ("Note.md", "# Note\n\n## Part\n"),
                ("Home.md", "x\n![[Note]]\n![[Note#Part|y]]\n![[pic.png]]\n"),
            ],
        );
        let (note, home) = (folder.0.join("Note.md"), folder.0.join("Home.md"));
        let notes = vec![note.clone(), home.clone()];
        let mut index = LinkIndex::default();
        index.sync(&notes, &Aliases::default(), &folder.0, &HashSet::new());
        let found: Vec<_> = index
            .backlinks(&note)
            .iter()
            .map(|b| (b.path.clone(), b.line))
            .collect();
        assert_eq!(found, [(home.clone(), 2), (home, 3)]);
    }
}
