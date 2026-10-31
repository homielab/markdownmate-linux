//! Link updates after the sidebar renamed or moved a note or folder: the notes in the open
//! folder whose links point at the old place, and the edits that make them point at the new
//! one. Links to a moved file are changed, and a moved note's own relative links and images are
//! rewritten relative to its new folder. Only link targets change; the rest of a note stays
//! byte for byte. Links in code (fences and code spans) are left alone. Pure logic, no GTK;
//! the editor asks first (`Editor::offer_link_updates`) and runs `find` / `write` on a
//! background thread.

use std::fs;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use crate::editor::moved;
use crate::links::{self, Target};
use crate::render::WikiIndex;
use crate::{lists, save, state};

/// Replace the bytes `range` of a note's text with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

/// A note that needs edits, as read when it was searched.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    /// Its text then (the editor's text for the open note).
    pub text: String,
    /// How many links change.
    pub links: usize,
}

/// Some moves done in the open folder `root`, and its notes before and after them.
#[derive(Debug, Clone)]
pub struct Plan {
    root: PathBuf,
    /// (old, new) path of each moved note, image or folder; a folder's files move with it.
    moves: Vec<(PathBuf, PathBuf)>,
    /// Every note of the folder before the moves, and the same notes after (indexed: every
    /// wiki link of every note is resolved against them).
    before: WikiIndex,
    after: WikiIndex,
}

impl Plan {
    /// `before`: the folder's notes as listed before the moves (all under `root`).
    pub fn new(moves: Vec<(PathBuf, PathBuf)>, before: Vec<PathBuf>, root: PathBuf) -> Self {
        let mut plan = Plan {
            before: WikiIndex::new(Vec::new(), &root),
            after: WikiIndex::new(Vec::new(), &root),
            root,
            moves,
        };
        let after = before.iter().map(|p| plan.new_path(p)).collect();
        plan.before = WikiIndex::new(before, &plan.root);
        plan.after = WikiIndex::new(after, &plan.root);
        plan
    }

    /// Where the file once at `old` is now.
    fn new_path(&self, old: &Path) -> PathBuf {
        self.moves
            .iter()
            .find_map(|(from, to)| moved(old, from, to))
            .unwrap_or_else(|| old.to_path_buf())
    }

    /// Where the file now at `new` was.
    fn old_path(&self, new: &Path) -> PathBuf {
        self.moves
            .iter()
            .rev()
            .find_map(|(from, to)| moved(new, to, from))
            .unwrap_or_else(|| new.to_path_buf())
    }

    /// The edits to `text`, the note now at `note`, in order and not overlapping.
    pub fn edits(&self, text: &str, note: &Path) -> Vec<Edit> {
        let note_old = self.old_path(note);
        let (Some(old_dir), Some(dir)) = (note_old.parent(), note.parent()) else {
            return Vec::new();
        };
        let note_moved = old_dir != dir;
        let mut out: Vec<Edit> = Vec::new();
        let mut open = None;
        let mut offset = 0;
        for raw in text.split_inclusive('\n') {
            let start = offset;
            offset += raw.len();
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            let line = line.strip_suffix('\r').unwrap_or(line);
            if lists::fence_step(&mut open, line) || open.is_some() {
                continue;
            }
            for (span, target) in links::spans(line) {
                let edit = match target {
                    Target::Wiki(_) => self.wiki_edit(line, span, &note_old, note, note_moved),
                    Target::Href(href) => {
                        self.href_edit(line, span, &href, old_dir, dir, note_moved)
                    }
                };
                if let Some(mut edit) = edit {
                    edit.range = start + edit.range.start..start + edit.range.end;
                    if !out.contains(&edit) {
                        out.push(edit);
                    }
                }
            }
        }
        out
    }

    /// The edit to the wiki link at `span` of `line` (byte range in the line): its name, when
    /// it no longer leads where it led. Its `|text` and `#heading` are kept.
    fn wiki_edit(
        &self,
        line: &str,
        span: Range<usize>,
        note_old: &Path,
        note: &Path,
        note_moved: bool,
    ) -> Option<Edit> {
        let body = line.get(span.start + 2..span.end.checked_sub(2)?)?;
        let raw = body.split('|').next()?.split('#').next()?;
        let name = raw.trim();
        let at = span.start + 2 + (raw.len() - raw.trim_start().len());
        let target = self.before.resolve(name, Some(note_old))?;
        let wanted = self.new_path(target);
        if &wanted == target && !note_moved {
            return None;
        }
        // Still leads there (same folder, or a name matching the new place too).
        if self.after.resolve(name, Some(note)) == Some(&wanted) {
            return None;
        }
        let replacement = self.wiki_name(&wanted, target, name, note);
        (replacement != name).then(|| Edit {
            range: at..at + name.len(),
            text: replacement,
        })
    }

    /// The name a wiki link in `note` gives `new` (formerly `old`), written before as `written`:
    /// as many trailing path parts as it had (more while it would lead to another note), with
    /// the extension only if it had one.
    fn wiki_name(&self, new: &Path, old: &Path, written: &str, note: &Path) -> String {
        let rel = new.strip_prefix(&self.root).unwrap_or(new);
        let mut parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if parts.is_empty() {
            return written.to_string();
        }
        let ext = old
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()));
        if !ext.is_some_and(|e| written.to_lowercase().ends_with(&e))
            && let Some(last) = parts.last_mut()
            && let Some(stem) = Path::new(last.as_str()).file_stem()
        {
            *last = stem.to_string_lossy().into_owned();
        }
        let count = written.split('/').count().clamp(1, parts.len());
        (count..=parts.len())
            .map(|n| parts[parts.len() - n..].join("/"))
            .find(|name| self.after.resolve(name, Some(note)).map(PathBuf::as_path) == Some(new))
            .unwrap_or_else(|| parts.join("/"))
    }

    /// The edit to the relative Markdown or image link `href` at `span` of `line`, in a note
    /// that was in `old_dir` and is now in `dir`: the new relative path of its file. The
    /// `#fragment` / `?query` is kept; `<…>` links stay raw, others are percent-encoded when
    /// they were or when needed.
    fn href_edit(
        &self,
        line: &str,
        span: Range<usize>,
        href: &str,
        old_dir: &Path,
        dir: &Path,
        note_moved: bool,
    ) -> Option<Edit> {
        let path = href.split(['#', '?']).next()?;
        // URLs (`scheme:` before any `/`), absolute paths and `#heading` links stay.
        if path.is_empty() || path.starts_with('/') || path.split('/').next()?.contains(':') {
            return None;
        }
        let source = &line[span.clone()];
        let dest = source.find("](")? + 2;
        let at = span.start + dest + source[dest..].find(href)?;
        let target = clean(&old_dir.join(state::unescape(path)?));
        let wanted = self.new_path(&target);
        if wanted == target && !note_moved {
            return None;
        }
        let mut relative = relative_path(&wanted, dir);
        if path.starts_with("./") && !relative.starts_with("../") {
            relative = format!("./{relative}");
        }
        let angled = line[..at].ends_with('<');
        let needs = |c: char| c.is_whitespace() || "()<>#?%".contains(c);
        if !angled && (path.contains('%') || relative.contains(needs)) {
            relative = encode(&relative);
        }
        (relative != path).then(|| Edit {
            range: at..at + path.len(),
            text: relative,
        })
    }
}

/// `text` with `edits` (in order, not overlapping) made.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    // One pass: a large note can have thousands of edits.
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for edit in edits {
        out.push_str(&text[at..edit.range.start]);
        out.push_str(&edit.text);
        at = edit.range.end;
    }
    out.push_str(&text[at..]);
    out
}

/// The notes of `plan` (after the moves) that need edits: read from disk, or `open` (path,
/// editor text) for the open note. Unreadable and non-UTF-8 notes, and notes whose real file
/// is outside the open folder (a symlink), are skipped.
pub fn find(plan: &Plan, open: Option<&(PathBuf, String)>) -> Vec<Found> {
    let Ok(root) = fs::canonicalize(&plan.root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for note in plan.after.notes() {
        if !fs::canonicalize(note).is_ok_and(|real| real.starts_with(&root)) {
            continue;
        }
        let text = match open {
            Some((path, text)) if path == note => text.clone(),
            _ => match save::read_note(note) {
                Ok(text) => text,
                Err(_) => continue,
            },
        };
        let links = plan.edits(&text, note).len();
        if links > 0 {
            out.push(Found {
                path: note.clone(),
                text,
                links,
            });
        }
    }
    out
}

/// Writes the edits to the closed notes `found` (atomically); a note that changed on disk
/// since it was read is left alone. Returns how many were updated.
pub fn write(plan: &Plan, found: &[Found]) -> usize {
    found
        .iter()
        .filter(|f| {
            save::read_note(&f.path).is_ok_and(|disk| disk == f.text)
                && save::write_atomic(&f.path, &apply(&f.text, &plan.edits(&f.text, &f.path)))
                    .is_ok()
        })
        .count()
}

/// `path` under the open folder `root` as the sidebar lists it, when it is written under
/// `root`'s real (canonical) path instead.
pub fn rebase(path: &Path, root: &Path) -> PathBuf {
    if path.starts_with(root) {
        return path.to_path_buf();
    }
    fs::canonicalize(root)
        .ok()
        .and_then(|real| path.strip_prefix(real).ok().map(|rel| root.join(rel)))
        .unwrap_or_else(|| path.to_path_buf())
}

/// `path` with `.` and `..` resolved (lexically).
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// `file`'s path relative to `folder`, `/`-separated, `../` as needed.
pub fn relative_path(file: &Path, folder: &Path) -> String {
    let (from, to) = (clean(folder), clean(file));
    let (from, to): (Vec<_>, Vec<_>) = (from.components().collect(), to.components().collect());
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let up = std::iter::repeat_n("..".to_string(), from.len() - shared);
    let down = to[shared..]
        .iter()
        .map(|c| c.as_os_str().to_string_lossy().into_owned());
    up.chain(down).collect::<Vec<_>>().join("/")
}

/// `path` percent-encoded for a Markdown link: bytes other than letters, digits, `/` and
/// `-._~!$&'*+,;=:@` (so spaces, brackets, `%`, `#`, `?` and non-ASCII letters too).
fn encode(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~!$&'*+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Links for files and folders dropped into a note, one per line: `[name](path)`, the name
/// without the file's extension (a folder keeps its whole name, `[` `]` dropped), the path
/// relative to the note's `folder` and percent-encoded (`encode`), so the preview, Ctrl+click
/// and link updates follow it. No `folder` (no saved note): a `file://` URL. `files` are
/// (path, is a folder).
pub fn dropped_links(files: &[(PathBuf, bool)], folder: Option<&Path>) -> String {
    files
        .iter()
        .map(|(file, is_dir)| {
            let name = if *is_dir {
                file.file_name()
            } else {
                file.file_stem()
            };
            let name: String = name
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
                .chars()
                .filter(|c| !matches!(c, '[' | ']'))
                .collect();
            let target = match folder {
                Some(folder) => encode(&relative_path(file, folder)),
                None => format!("file://{}", encode(&clean(file).to_string_lossy())),
            };
            format!("[{name}]({target})")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    /// The notes of a sample folder /n.
    fn notes() -> Vec<PathBuf> {
        paths(&[
            "/n/Name.md",
            "/n/A.md",
            "/n/Sub/B.md",
            "/n/Sub/Deep/C.md",
            "/n/Other/Name.md",
        ])
    }

    fn plan(from: &str, to: &str) -> Plan {
        Plan::new(vec![(from.into(), to.into())], notes(), PathBuf::from("/n"))
    }

    /// `text` (the note now at `note`) after `plan`'s edits.
    fn rewrite(plan: &Plan, text: &str, note: &str) -> String {
        apply(text, &plan.edits(text, Path::new(note)))
    }

    #[test]
    fn wiki_links_keep_alias_and_heading() {
        let p = plan("/n/Name.md", "/n/New Name.md");
        assert_eq!(
            rewrite(
                &p,
                "a [[Name]] b [[name|the note]] [[ Name#Part ]]\n",
                "/n/A.md"
            ),
            "a [[New Name]] b [[New Name|the note]] [[ New Name#Part ]]\n"
        );
        assert_eq!(rewrite(&p, "[[Name.md]]", "/n/A.md"), "[[New Name.md]]");
        // From Other/, `[[Name]]` is the note next to it: not changed.
        assert_eq!(rewrite(&p, "[[Name]]", "/n/Other/X.md"), "[[Name]]");
        assert_eq!(
            rewrite(&p, "[[A]] [[Missing]] [[#Top]]", "/n/A.md"),
            "[[A]] [[Missing]] [[#Top]]"
        );
    }

    #[test]
    fn wiki_links_get_folders_when_needed() {
        // Renamed to the name of another note: the plain name would lead there.
        let p = plan("/n/Sub/B.md", "/n/Sub/Name.md");
        assert_eq!(rewrite(&p, "[[B]]", "/n/A.md"), "[[Sub/Name]]");
        // A folder rename: `[[Sub/Deep/C]]` gets the new folder, `[[C]]` still works.
        let p = plan("/n/Sub", "/n/Work");
        assert_eq!(
            rewrite(&p, "[[Sub/Deep/C]] [[C]] [[sub/b|x]]", "/n/A.md"),
            "[[Work/Deep/C]] [[C]] [[Work/B|x]]"
        );
    }

    #[test]
    fn relative_links_up_and_down_the_tree() {
        let p = plan("/n/Sub/Deep/C.md", "/n/Other/C.md");
        assert_eq!(
            rewrite(&p, "[c](Sub/Deep/C.md#h) ![i](x.png)", "/n/A.md"),
            "[c](Other/C.md#h) ![i](x.png)"
        );
        assert_eq!(
            rewrite(&p, "[c](Deep/C.md)", "/n/Sub/B.md"),
            "[c](../Other/C.md)"
        );
        assert_eq!(
            rewrite(&p, "[c](./Deep/C.md)", "/n/Sub/B.md"),
            "[c](../Other/C.md)"
        );
        assert_eq!(
            rewrite(&p, "[c](../Other/../Sub/Deep/C.md \"t\")", "/n/Other/X.md"),
            "[c](C.md \"t\")"
        );
        let p = plan("/n/A.md", "/n/Sub/Deep/A.md");
        assert_eq!(
            rewrite(&p, "[a](./A.md)", "/n/Name.md"),
            "[a](./Sub/Deep/A.md)"
        );
        assert_eq!(
            rewrite(&p, "[a](../../A.md)", "/n/Sub/Deep/C.md"),
            "[a](A.md)"
        );
        assert_eq!(
            rewrite(
                &p,
                "[w](https://x/A.md) [h](#top) [p](/n/A.md)",
                "/n/Name.md"
            ),
            "[w](https://x/A.md) [h](#top) [p](/n/A.md)"
        );
    }

    #[test]
    fn percent_encoded_names() {
        let p = plan("/n/A.md", "/n/My Note (1).md");
        assert_eq!(
            rewrite(&p, "[a](A.md)", "/n/Name.md"),
            "[a](My%20Note%20%281%29.md)"
        );
        assert_eq!(
            rewrite(&p, "[a](<A.md>)", "/n/Name.md"),
            "[a](<My Note (1).md>)"
        );
        let p = plan("/n/My Note (1).md", "/n/Ý tưởng.md");
        assert_eq!(
            rewrite(&p, "[a](My%20Note%20%281%29.md)", "/n/Name.md"),
            "[a](%C3%9D%20t%C6%B0%E1%BB%9Fng.md)"
        );
        let p = plan("/n/My Note.md", "/n/Ý.md");
        assert_eq!(
            rewrite(&p, "[a](<My Note.md>)", "/n/Name.md"),
            "[a](<Ý.md>)"
        );
        // Not encoded before and not needed now: stays readable.
        let p = plan("/n/A.md", "/n/Ý.md");
        assert_eq!(rewrite(&p, "[a](A.md)", "/n/Name.md"), "[a](Ý.md)");
    }

    #[test]
    fn links_in_code_untouched() {
        let p = plan("/n/Name.md", "/n/New.md");
        let text =
            "`[[Name]]` ``[a](Name.md)``\n```\n[[Name]]\n```\n~~~\n[a](Name.md)\n~~~\n[[Name]]\n";
        assert_eq!(
            rewrite(&p, text, "/n/A.md"),
            text.replace("~~~\n[[Name]]", "~~~\n[[New]]")
        );
    }

    #[test]
    fn note_moved_into_a_subfolder_keeps_its_links() {
        let p = plan("/n/A.md", "/n/Sub/A.md");
        let text = "# A\r\n[[Name]] [[B]] [n](Name.md) ![p](assets/p%201.png) [s](Sub/B.md)\r\n[[Other/Name|o]] [web](https://x)\r\n";
        assert_eq!(
            rewrite(&p, text, "/n/Sub/A.md"),
            "# A\r\n[[Name]] [[B]] [n](../Name.md) ![p](../assets/p%201.png) [s](B.md)\r\n[[Other/Name|o]] [web](https://x)\r\n"
        );
        // A note linking to it: its relative link follows, its wiki link still resolves.
        assert_eq!(
            rewrite(&p, "[[A]] [a](A.md)", "/n/Name.md"),
            "[[A]] [a](Sub/A.md)"
        );
        // Moved next to a note named like the root note it linked: a wiki link has no way to
        // name a root note over one in its own folder, so it is left as it was.
        let p = plan("/n/A.md", "/n/Other/A.md");
        assert_eq!(rewrite(&p, "[[Name]]", "/n/Other/A.md"), "[[Name]]");
    }

    #[test]
    fn moved_folder_carries_images_and_its_notes_links() {
        let p = plan("/n/Sub", "/n/Other/Sub");
        assert_eq!(
            rewrite(&p, "![i](Sub/assets/i.png) [c](Sub/Deep/C.md)", "/n/A.md"),
            "![i](Other/Sub/assets/i.png) [c](Other/Sub/Deep/C.md)"
        );
        // Inside the moved folder: links between its files stay, links out get a `../`.
        assert_eq!(
            rewrite(
                &p,
                "[c](Deep/C.md) [a](../A.md) ![i](assets/i.png)",
                "/n/Other/Sub/B.md"
            ),
            "[c](Deep/C.md) [a](../../A.md) ![i](assets/i.png)"
        );
    }

    #[test]
    fn nothing_else_changes() {
        let p = plan("/n/Name.md", "/n/Name.md");
        let text = "[[Name]] [a](Name.md)";
        assert!(p.edits(text, Path::new("/n/A.md")).is_empty());
        let p = plan("/n/Name.md", "/n/New.md");
        let text = "\u{feff}x  [[Name]]\ttail ✓\n\nno newline";
        assert_eq!(
            rewrite(&p, text, "/n/A.md"),
            "\u{feff}x  [[New]]\ttail ✓\n\nno newline"
        );
    }

    #[test]
    fn relative_paths() {
        let r = |f: &str, d: &str| relative_path(Path::new(f), Path::new(d));
        assert_eq!(r("/n/a/b.md", "/n"), "a/b.md");
        assert_eq!(r("/n/b.md", "/n/a/c"), "../../b.md");
        assert_eq!(r("/n/x/b.md", "/n/a"), "../x/b.md");
    }

    #[test]
    fn find_and_write_on_disk() {
        let dir = std::env::temp_dir().join(format!("mm-relink-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Sub")).unwrap();
        let w = |name: &str, text: &str| fs::write(dir.join(name), text).unwrap();
        w("New.md", "# moved\n");
        w("A.md", "[[Old]] and [o](Old.md)\n");
        w("Sub/B.md", "[o](../Old.md)\n");
        w("C.md", "nothing\n");
        w("Open.md", "on disk\n");
        let before = ["Old.md", "A.md", "Sub/B.md", "C.md", "Open.md"].map(|n| dir.join(n));
        let plan = Plan::new(
            vec![(dir.join("Old.md"), dir.join("New.md"))],
            before.to_vec(),
            dir.clone(),
        );
        let open = (dir.join("Open.md"), "editor [[Old]]".to_string());
        let found = find(&plan, Some(&open));
        let summary: Vec<_> = found.iter().map(|f| (f.path.clone(), f.links)).collect();
        assert_eq!(
            summary,
            [
                (dir.join("A.md"), 2),
                (dir.join("Sub/B.md"), 1),
                (dir.join("Open.md"), 1)
            ]
        );
        // Sub/B.md changes meanwhile: left alone.
        w("Sub/B.md", "edited\n");
        let closed: Vec<_> = found.into_iter().filter(|f| f.path != open.0).collect();
        assert_eq!(write(&plan, &closed), 1);
        let r = |name: &str| fs::read_to_string(dir.join(name)).unwrap();
        assert_eq!(r("A.md"), "[[New]] and [o](New.md)\n");
        assert_eq!(r("Sub/B.md"), "edited\n");
        assert_eq!(r("Open.md"), "on disk\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rebase_to_the_listed_root() {
        let p = Path::new("/n/a.md");
        assert_eq!(rebase(p, Path::new("/n")), p);
        assert_eq!(rebase(p, Path::new("/does/not/exist")), p);
    }

    #[test]
    fn links_for_dropped_files() {
        let one = |f: &str, dir: bool, folder: Option<&str>| {
            dropped_links(&[(PathBuf::from(f), dir)], folder.map(Path::new))
        };
        let n = Some("/n/Notes");
        assert_eq!(one("/n/Notes/Plan.md", false, n), "[Plan](Plan.md)");
        assert_eq!(
            one("/n/Notes/Sub/Report 2.pdf", false, n),
            "[Report 2](Sub/Report%202.pdf)"
        );
        assert_eq!(one("/n/Other/a.txt", false, n), "[a](../Other/a.txt)");
        assert_eq!(one("/n/Notes/Sub/More", true, n), "[More](Sub/More)");
        assert_eq!(one("/n/Notes/notes.v2", true, n), "[notes.v2](notes.v2)");
        assert_eq!(
            one("/n/Notes/Ý tưởng (cũ).md", false, n),
            "[Ý tưởng (cũ)](%C3%9D%20t%C6%B0%E1%BB%9Fng%20%28c%C5%A9%29.md)"
        );
        assert_eq!(
            one("/n/Notes/[draft] x#1.md", false, n),
            "[draft x#1](%5Bdraft%5D%20x%231.md)"
        );
        // No saved note: an absolute file:// URL.
        assert_eq!(
            one("/tmp/a b.pdf", false, None),
            "[a b](file:///tmp/a%20b.pdf)"
        );
        let several = [
            (PathBuf::from("/n/Notes/a.md"), false),
            (PathBuf::from("/n/b"), true),
        ];
        assert_eq!(
            dropped_links(&several, n.map(Path::new)),
            "[a](a.md)\n[b](../b)"
        );
    }
}
