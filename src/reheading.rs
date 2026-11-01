//! Rename Heading (Outline ▸ right-click a heading): the edit that gives a heading of the open
//! note new text (its `#`s, closing `#`s or Setext underline kept), and the wiki links of the
//! open folder's notes that follow it: `[[Note#Old]]`, `[[Note#Old|text]]`, `![[Note#Old]]`,
//! `[[Folder/Note#Old]]`, `[[Alias#Old]]`, and `[[#Old]]` in the note itself. A link follows
//! when its heading part reaches this heading the way following the link finds it (Ctrl+click,
//! the preview, Find Broken Links: `broken::heading_of`), so links with the text of an earlier
//! heading of the same text stay with that one. Fenced code, code spans, the front matter and
//! Markdown links (`[t](Note.md#slug)`) are left alone. Pure logic, no GTK; the editor asks
//! first (`Editor::rename_heading`) and runs `find` / `write` on a background thread.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::links::{self, Target};
use crate::relink::{Edit, apply};
use crate::render::{self, WikiIndex, is_embed_image, wiki_key};
use crate::{broken, lists, outline, save};

/// A heading of the note, as the outline lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    /// 1-based line of its text (the outline's line; a Setext heading's text line).
    pub line: usize,
    /// Byte range of that line in the note (without its `\n`).
    pub line_range: Range<usize>,
    /// Byte range of its text in the note: no markers, no closing `#`s.
    pub range: Range<usize>,
    /// Its text as written.
    pub written: String,
    /// The line `render::heading_ids` gives it, which links reach it by; None when the preview
    /// doesn't read it as a heading (then no link follows).
    pub id_line: Option<usize>,
    /// Links with its text reach it: no earlier heading has the same text.
    pub first: bool,
}

/// The heading the outline shows on 1-based `line` of `text`; None if there is none.
pub fn heading(text: &str, line: usize) -> Option<Heading> {
    if !outline::headings(text).iter().any(|h| h.line == line) {
        return None;
    }
    let line_range = line_ranges(text).nth(line - 1)?;
    let raw = &text[line_range.clone()];
    let range = match outline::atx_title(raw) {
        Some((_, range)) => range,
        None => {
            let lead = raw.len() - raw.trim_start().len();
            lead..lead + raw.trim().len()
        }
    };
    let range = line_range.start + range.start..line_range.start + range.end;
    let target = render::heading_targets(text)
        .into_iter()
        .find(|t| t.lines.contains(&line));
    let id_line = target.as_ref().map(|t| *t.lines.start());
    let first = target.is_some_and(|t| {
        render::find_heading(&render::heading_ids(text), &t.title).map(|h| h.0) == id_line
    });
    Some(Heading {
        line,
        line_range,
        written: text[range.clone()].to_string(),
        range,
        id_line,
        first,
    })
}

/// The byte range of each line of `text` (as `str::lines` splits it, a `\r` before `\n` kept).
fn line_ranges(text: &str) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut start = 0;
    text.split_inclusive('\n').map(move |l| {
        let range = start..start + l.strip_suffix('\n').unwrap_or(l).len();
        start += l.len();
        range
    })
}

/// The edit giving `heading` the text `new` (trimmed), or None when that text is empty, the
/// same as now, or would not read back as this heading's whole text (a closing `#`, a list
/// marker before a Setext underline, a line break).
pub fn line_edit(text: &str, heading: &Heading, new: &str) -> Option<Edit> {
    let new = new.trim();
    if new.is_empty() || new == heading.written || new.contains(['\n', '\r']) {
        return None;
    }
    let line = format!(
        "{}{new}{}",
        &text[heading.line_range.start..heading.range.start],
        &text[heading.range.end..heading.line_range.end]
    );
    let next = text[heading.line_range.end..]
        .strip_prefix('\n')
        .and_then(|rest| rest.lines().next())
        .unwrap_or("");
    let read = outline::headings(&format!("{line}\n{next}"));
    (read.first().is_some_and(|h| h.line == 1 && h.title == new)).then(|| Edit {
        range: heading.range.clone(),
        text: new.to_string(),
    })
}

/// Whether another heading of `text` than `heading` has the text `new` (as links compare it):
/// links with it then reach the first of them.
pub fn shared_text(text: &str, heading: &Heading, new: &str) -> bool {
    let want = comrak::Anchorizer::new().anchorize(new.trim());
    render::heading_targets(text).iter().any(|t| {
        Some(*t.lines.start()) != heading.id_line
            && comrak::Anchorizer::new().anchorize(&t.title) == want
    })
}

/// What follows `#` in the links after the rename: the heading's text in `renamed` (the note
/// with `line` rewritten), or its id when the text holds `[`, `]`, `|`, `#` or a line break,
/// which a wiki link can't carry as written.
pub fn fragment(renamed: &str, line: usize, new: &str) -> String {
    let Some(target) = render::heading_targets(renamed)
        .into_iter()
        .find(|t| t.lines.contains(&line))
    else {
        return new.trim().to_string();
    };
    if !target.title.contains(['[', ']', '|', '#', '\n', '\r']) {
        return target.title;
    }
    let start = *target.lines.start();
    render::heading_ids(renamed)
        .into_iter()
        .find(|(l, _)| *l == start)
        .map_or(target.fragment, |(_, id)| id)
}

/// The byte ranges of `text` (the note at `note`) holding the heading part of a wiki link that
/// reaches the heading on `id_line` of `target`, whose `render::heading_ids` are `ids`. Names
/// resolve through `wiki` (aliases too); `[[#Heading]]` counts only in `target` itself.
pub fn link_ranges(
    text: &str,
    note: &Path,
    wiki: &WikiIndex,
    target: &Path,
    ids: &[(usize, String)],
    id_line: usize,
) -> Vec<Range<usize>> {
    let skip = lists::front_matter_lines(text);
    let mut open = None;
    let mut out = Vec::new();
    for (i, range) in line_ranges(text).enumerate() {
        let line = &text[range.clone()];
        let line = line.strip_suffix('\r').unwrap_or(line);
        if i < skip || lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        for (span, link) in links::spans(line) {
            let Target::Wiki(name) = link else { continue };
            let embed = links::is_embed(line, &span);
            if embed && is_embed_image(&name) {
                continue;
            }
            let into = match wiki_key(&name) {
                None => Some(note),
                Some(_) => wiki.resolve(&name, Some(note)).map(PathBuf::as_path),
            };
            let reaches = into == Some(target)
                && broken::heading_part(&name, embed)
                    .and_then(|part| broken::heading_of(ids, part))
                    .is_some_and(|h| h.0 == id_line);
            if let (true, Some(part)) = (reaches, part_range(line, &span)) {
                out.push(range.start + part.start..range.start + part.end);
            }
        }
    }
    out
}

/// The byte range of the heading part (after the first `#`, to the end of the name) of the
/// wiki link at `span` of `line` (from `links::spans`).
fn part_range(line: &str, span: &Range<usize>) -> Option<Range<usize>> {
    let inner = span.start + 2;
    let raw = line[inner..span.end - 2].split('|').next()?;
    let lead = raw.len() - raw.trim_start().len();
    let name = raw.trim();
    let hash = name.find('#')?;
    Some(inner + lead + hash + 1..inner + lead + name.len())
}

/// A note with links to the heading, as read when it was searched.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    /// Its text then (the editor's text for the open note).
    pub text: String,
    /// The heading parts of its links to the heading, in order.
    pub links: Vec<Range<usize>>,
}

/// The notes of `notes` with links to `heading` of the open note `open` (path, editor text):
/// read from disk, the open note's text from `open`. With a folder `root`, notes whose real
/// file is outside it (a symlink) are skipped; unreadable and non-UTF-8 notes too.
pub fn find(
    notes: &[PathBuf],
    root: Option<&Path>,
    wiki: &WikiIndex,
    open: &(PathBuf, String),
    heading: &Heading,
) -> Vec<Found> {
    let Some(id_line) = heading.id_line else {
        return Vec::new();
    };
    let root = root.and_then(|r| fs::canonicalize(r).ok());
    let ids = render::heading_ids(&open.1);
    let mut out: Vec<Found> = Vec::new();
    for note in notes {
        if out.iter().any(|f| &f.path == note)
            || root.as_ref().is_some_and(|root| {
                !fs::canonicalize(note).is_ok_and(|real| real.starts_with(root))
            })
        {
            continue;
        }
        let text = if *note == open.0 {
            open.1.clone()
        } else {
            match save::read_note(note) {
                Ok(text) => text,
                Err(_) => continue,
            }
        };
        let links = link_ranges(&text, note, wiki, &open.0, &ids, id_line);
        if !links.is_empty() {
            out.push(Found {
                path: note.clone(),
                text,
                links,
            });
        }
    }
    out
}

/// The edits of a note's `links` to `fragment`, plus `first` (the heading line's edit, in the
/// renamed note); a link inside that edit's range goes with it. In order, not overlapping.
pub fn edits(links: &[Range<usize>], fragment: &str, first: Option<Edit>) -> Vec<Edit> {
    let mut out: Vec<Edit> = links
        .iter()
        .filter(|r| {
            first
                .as_ref()
                .is_none_or(|e| r.end <= e.range.start || r.start >= e.range.end)
        })
        .map(|r| Edit {
            range: r.clone(),
            text: fragment.to_string(),
        })
        .collect();
    out.extend(first);
    out.sort_by_key(|e| e.range.start);
    out
}

/// Points the links of the closed notes `found` to `fragment` (atomic writes); a note that
/// changed on disk since it was read is left alone. Returns the notes written.
pub fn write(found: &[Found], fragment: &str) -> Vec<PathBuf> {
    found
        .iter()
        .filter(|f| {
            save::read_note(&f.path).is_ok_and(|disk| disk == f.text)
                && save::write_atomic(&f.path, &apply(&f.text, &edits(&f.links, fragment, None)))
                    .is_ok()
        })
        .map(|f| f.path.clone())
        .collect()
}

/// What the Rename Heading dialog says the rename does to links: "Also updates N links in M
/// notes" or "No links point here", and whether links with its text reach an earlier heading.
pub fn summary(found: &[Found], first: bool) -> String {
    let links: usize = found.iter().map(|f| f.links.len()).sum();
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut out = Vec::new();
    if links > 0 {
        out.push(format!(
            "Also updates {} in {}",
            plural(links, "link", "links"),
            plural(found.len(), "note", "notes")
        ));
    } else if first {
        out.push("No links point here".to_string());
    }
    if !first {
        out.push("Links point to the first heading with this text and stay".to_string());
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::aliases::Aliases;

    /// Gives the heading on `line` of `text` the text `new`.
    fn rename(text: &str, line: usize, new: &str) -> Option<String> {
        let h = heading(text, line)?;
        line_edit(text, &h, new).map(|e| apply(text, &[e]))
    }

    /// Notes of a folder `/n`: `Note.md` (alias "Nick") and `Sub/Note.md`, `Other.md`.
    fn wiki() -> WikiIndex {
        let notes: Vec<PathBuf> = ["/n/Note.md", "/n/Sub/Note.md", "/n/Other.md"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let aliases = Aliases::new(HashMap::from([(
            PathBuf::from("/n/Note.md"),
            vec!["Nick".to_string()],
        )]));
        WikiIndex::with_aliases(notes, Path::new("/n"), &aliases)
    }

    /// `text` (in note `from`) with its links to the heading on `line` of `target_text` (the
    /// note `/n/Note.md`) pointed to `new`.
    fn follow(text: &str, from: &str, target_text: &str, line: usize, new: &str) -> String {
        follow_into("/n/Note.md", text, from, target_text, line, new)
    }

    /// `follow` with the heading in the note `target`.
    fn follow_into(
        target: &str,
        text: &str,
        from: &str,
        target_text: &str,
        line: usize,
        new: &str,
    ) -> String {
        let h = heading(target_text, line).unwrap();
        let ids = render::heading_ids(target_text);
        let target = Path::new(target);
        let ranges = link_ranges(
            text,
            Path::new(from),
            &wiki(),
            target,
            &ids,
            h.id_line.unwrap(),
        );
        apply(text, &edits(&ranges, new, None))
    }

    #[test]
    fn heading_line_keeps_level_and_closing_hashes() {
        assert_eq!(rename("# Old\n", 1, "New").unwrap(), "# New\n");
        assert_eq!(
            rename("text\n### Old ###\n", 2, "New").unwrap(),
            "text\n### New ###\n"
        );
        assert_eq!(
            rename("  ##  Old  \r\n", 1, " New ").unwrap(),
            "  ##  New  \r\n"
        );
        assert_eq!(rename("# C#\n", 1, "C++").unwrap(), "# C++\n");
        assert_eq!(rename("Old\n===\n", 1, "New").unwrap(), "New\n===\n");
        assert_eq!(
            rename("Old\n---\n", 1, "Tiếng Việt").unwrap(),
            "Tiếng Việt\n---\n"
        );
        assert_eq!(heading("# Old ##", 1).unwrap().written, "Old");
    }

    #[test]
    fn heading_line_refuses_text_that_reads_back_otherwise() {
        for new in ["", "  ", "Old", "Foo #", "a\nb"] {
            assert_eq!(rename("# Old\n", 1, new), None, "{new:?}");
        }
        assert_eq!(rename("Old\n---\n", 1, "- item"), None);
        assert_eq!(rename("Old\n---\n", 1, "# Old"), None);
        // Not a heading line.
        assert_eq!(heading("# Old\ntext\n", 2), None);
        assert_eq!(heading("```\n# Old\n```\n", 2), None);
    }

    #[test]
    fn each_link_form_follows() {
        let note = "# Intro\n## Old\n";
        let text = "[[Note#Old]] [[Note#Old|see]] ![[Note#Old]] [[Note.md#old]]\n\
                    [[Nick#Old]] [[note#OLD]] ![[ Note# Old ]] [[Note#Old]]\r\n";
        assert_eq!(
            follow(text, "/n/Other.md", note, 2, "New"),
            "[[Note#New]] [[Note#New|see]] ![[Note#New]] [[Note.md#New]]\n\
             [[Nick#New]] [[note#New]] ![[ Note#New ]] [[Note#New]]\r\n"
        );
        // A `%`-escaped part, read the way Ctrl+click decodes it.
        assert_eq!(
            follow("[[Note#My%20Old]]", "/n/Other.md", "## My Old\n", 1, "New"),
            "[[Note#New]]"
        );
    }

    #[test]
    fn other_links_stay() {
        let note = "# Intro\n## Old\n";
        let text = "[[Note#Intro]] [[Other#Old]] [[Note#Older]] [[Note]] [[#Old]]\n\
                    [t](Note.md#old) `[[Note#Old]]` [[x.png#Old]]\n\
                    ```\n[[Note#Old]]\n```\n";
        assert_eq!(follow(text, "/n/Other.md", note, 2, "New"), text);
    }

    #[test]
    fn note_in_another_folder() {
        // Two notes named Note: each link follows only into the one it reaches.
        let note = "## Old\n";
        assert_eq!(
            follow(
                "[[Sub/Note#Old]] [[Note#Old]]",
                "/n/Other.md",
                note,
                1,
                "New"
            ),
            "[[Sub/Note#Old]] [[Note#New]]"
        );
        // From Sub/, `[[Note#Old]]` reaches Sub/Note.md; the alias reaches Note.md.
        assert_eq!(
            follow(
                "[[Note#Old]] [[Nick#Old]]",
                "/n/Sub/Other.md",
                note,
                1,
                "New"
            ),
            "[[Note#Old]] [[Nick#New]]"
        );
        assert_eq!(
            follow_into(
                "/n/Sub/Note.md",
                "[[Sub/Note#Old]] [[Note#Old]]",
                "/n/Other.md",
                note,
                1,
                "New"
            ),
            "[[Sub/Note#New]] [[Note#Old]]"
        );
    }

    #[test]
    fn own_links_only_in_the_note_itself() {
        let note = "---\nsee: \"[[#Old]]\"\n---\n# Old\n[[#Old]] and [[#old]]\n";
        let h = heading(note, 4).unwrap();
        let ids = render::heading_ids(note);
        let own = Path::new("/n/Note.md");
        let ranges = link_ranges(note, own, &wiki(), own, &ids, h.id_line.unwrap());
        assert_eq!(ranges.len(), 2);
        let line = line_edit(note, &h, "New");
        assert_eq!(
            apply(note, &edits(&ranges, "New", line)),
            "---\nsee: \"[[#Old]]\"\n---\n# New\n[[#New]] and [[#New]]\n"
        );
        // Another note's `[[#Old]]` is its own heading.
        let other = Path::new("/n/Other.md");
        assert!(link_ranges("[[#Old]]", other, &wiki(), own, &ids, h.id_line.unwrap()).is_empty());
        // A link inside the heading line's edit goes with it.
        let line = Edit {
            range: 4..8,
            text: "x".into(),
        };
        let got: Vec<Range<usize>> = edits(&[0..2, 5..7, 9..10], "N", Some(line))
            .into_iter()
            .map(|e| e.range)
            .collect();
        assert_eq!(got, [0..2, 4..8, 9..10]);
    }

    #[test]
    fn first_heading_rule() {
        let note = "# Old\ntext\n## Old\n";
        let first = heading(note, 1).unwrap();
        let second = heading(note, 3).unwrap();
        assert!(first.first);
        assert!(!second.first);
        let text = "[[Note#Old]] [[Note#old-1]]";
        assert_eq!(
            follow(text, "/n/Other.md", note, 1, "New"),
            "[[Note#New]] [[Note#old-1]]"
        );
        assert_eq!(
            follow(text, "/n/Other.md", note, 3, "New"),
            "[[Note#Old]] [[Note#New]]"
        );
        assert!(shared_text(note, &first, "old"));
        assert!(!shared_text("# Old\n## Other\n", &first, "Old again"));
        assert!(shared_text("# Old\n## Other\n", &first, " other "));
    }

    #[test]
    fn new_fragment() {
        assert_eq!(fragment("# New **bold**\n", 1, "New **bold**"), "New bold");
        assert_eq!(fragment("# a|b\n", 1, "a|b"), "ab");
        assert_eq!(fragment("# C# basics\n", 1, "C# basics"), "c-basics");
        assert_eq!(
            fragment("# [draft] plan\n", 1, "[draft] plan"),
            "draft-plan"
        );
        assert_eq!(fragment("# a]b\n", 1, "a]b"), "ab");
        // A repeated title's id carries its `-1`.
        assert_eq!(fragment("# C#\n## C#\n", 2, "C#"), "c-1");
        assert_eq!(fragment("# Same\n# Same\n", 2, "Same"), "Same");
    }

    #[test]
    fn dialog_summary() {
        let found = |n: usize| Found {
            path: PathBuf::from("/n/a.md"),
            text: String::new(),
            links: vec![0..1; n],
        };
        assert_eq!(summary(&[], true), "No links point here");
        assert_eq!(summary(&[found(1)], true), "Also updates 1 link in 1 note");
        assert_eq!(
            summary(&[found(2), found(1)], true),
            "Also updates 3 links in 2 notes"
        );
        assert_eq!(
            summary(&[], false),
            "Links point to the first heading with this text and stay"
        );
    }

    #[test]
    fn find_and_write() {
        let dir = std::env::temp_dir().join(format!("mm-reheading-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let (note, a, b, c) = (
            dir.join("Note.md"),
            dir.join("a.md"),
            dir.join("b.md"),
            dir.join("c.md"),
        );
        fs::write(&note, "on disk\n").unwrap();
        fs::write(&a, "[[Note#Old]] and `[[Note#Old]]`\n").unwrap();
        fs::write(&b, "[[Note#Old|x]]\n").unwrap();
        fs::write(&c, "[[Note#Other]]\n").unwrap();
        let notes = vec![
            note.clone(),
            a.clone(),
            b.clone(),
            c.clone(),
            dir.join("gone.md"),
        ];
        let wiki = WikiIndex::new(notes.clone(), &dir);
        let open = (note.clone(), "# Old\n## Other\nsee [[#Old]]\n".to_string());
        let h = heading(&open.1, 1).unwrap();
        let found = find(&notes, Some(&dir), &wiki, &open, &h);
        let paths: Vec<&PathBuf> = found.iter().map(|f| &f.path).collect();
        assert_eq!(paths, [&note, &a, &b]);
        assert_eq!(found[0].text, open.1);
        assert_eq!(summary(&found, h.first), "Also updates 3 links in 3 notes");
        // b changed on disk after it was read: left alone.
        fs::write(&b, "[[Note#Old|x]] edited\n").unwrap();
        assert_eq!(write(&found[1..], "New"), [a.as_path()]);
        assert_eq!(
            fs::read_to_string(&a).unwrap(),
            "[[Note#New]] and `[[Note#Old]]`\n"
        );
        assert_eq!(fs::read_to_string(&b).unwrap(), "[[Note#Old|x]] edited\n");
    }
}
