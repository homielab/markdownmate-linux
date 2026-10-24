//! Backlinks (under the Outline, Alt+Ctrl+O): the notes that link to the open note, by
//! `[[wiki]]` link (same resolution as the preview) or a relative Markdown link to its file.
//! Pure logic, no GTK; the window runs `find` on a background thread.

use std::path::{Component, Path, PathBuf};

use crate::links::{self, Target};
use crate::{lists, render, state};

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

/// Lines of `notes` (other than `target`) linking to `target`, in `notes` order.
/// Unreadable or non-UTF-8 notes are skipped.
pub fn find(notes: &[PathBuf], root: &Path, target: &Path) -> Vec<Backlink> {
    let mut out = Vec::new();
    for note in notes.iter().filter(|n| n.as_path() != target) {
        let Some(text) = std::fs::read(note)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
        else {
            continue;
        };
        for line in lines_linking(&text, note, target, notes, root) {
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

/// 1-based lines of `text` (the note at `note`) with a link to `target`, outside fenced code
/// and code spans.
pub fn lines_linking(
    text: &str,
    note: &Path,
    target: &Path,
    notes: &[PathBuf],
    root: &Path,
) -> Vec<usize> {
    let mut out = Vec::new();
    let mut open = None;
    for (i, line) in text.lines().enumerate() {
        if lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        let hit = links::spans(line).into_iter().any(|(_, t)| match t {
            Target::Wiki(name) => {
                render::resolve_wiki(&name, notes, root, Some(note)).map(PathBuf::as_path)
                    == Some(target)
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
        lines_linking(text, Path::new(note), Path::new("/n/Name.md"), &notes, root)
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
        let notes = [PathBuf::from("/n/My Note.md")];
        let found =
            |t: &str| lines_linking(t, Path::new("/n/A.md"), &notes[0], &notes, Path::new("/n"));
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
        let found = find(&notes, &dir, &name);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].path.as_path(), found[0].line), (a.as_path(), 2));
        assert!(found[0].snippet.starts_with("see [[Name]] x"));
        assert_eq!(found[0].snippet.chars().count(), SNIPPET_CHARS + 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
