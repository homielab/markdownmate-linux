//! Rename Tag: the edits that turn `#old` into `#new` (and `#old/…` into `#new/…`) in a note,
//! and the notes of the open folder that need them. Tags are found as everywhere else
//! (`tags::spans`: not in front matter, fenced code, code spans, link destinations, URLs or a
//! heading's `#`); only the tags change, the rest of a note stays byte for byte. Pure logic, no
//! GTK; the editor asks first (`Editor::rename_tag`) and runs `find` / `write` on a background
//! thread.

use std::fs;
use std::path::{Path, PathBuf};

use crate::relink::{Edit, apply};
use crate::{save, tags};

/// A note that uses the tag, as read when it was searched.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    /// Its text then (the editor's text for the open note).
    pub text: String,
    /// How many tags change.
    pub tags: usize,
}

/// The name typed for a tag, cleaned (trimmed, one leading `#` dropped), or None if it is not
/// a tag the notes would read back whole: a letter first, then letters, digits, `_` and `-`,
/// with `/` between non-empty parts.
pub fn valid_name(input: &str) -> Option<String> {
    let name = input.trim();
    let name = name.strip_prefix('#').unwrap_or(name);
    let tag = format!("#{name}");
    let whole = tags::in_line(&tag).first() == Some(&(0..tag.len()));
    (whole && name.split('/').all(|part| !part.is_empty())).then(|| name.to_string())
}

/// If the tag `name` (no `#`) is `old` or nested under it (its first parts equal `old`'s,
/// ignoring case), the rest of it after those parts: `""` or `/child…`. `older` and
/// `workout/x` are not under `work`.
pub fn under<'a>(name: &'a str, old: &str) -> Option<&'a str> {
    let mut rest = name;
    for (i, part) in old.split('/').enumerate() {
        if i > 0 {
            rest = rest.strip_prefix('/')?;
        }
        let end = rest.find('/').unwrap_or(rest.len());
        if rest[..end].to_lowercase() != part.to_lowercase() {
            return None;
        }
        rest = &rest[end..];
    }
    Some(rest)
}

/// The edits renaming `#old` to `#new` in `text`: each tag under `old` gets `new` in place of
/// `old`'s parts, keeping its own child parts (`#Work/a` → `#job/a`). A tag already written as
/// the new name is left alone.
pub fn edits(text: &str, old: &str, new: &str) -> Vec<Edit> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    tags::spans(text)
        .into_iter()
        .filter_map(|(line, r)| {
            let range = starts[line] + r.start..starts[line] + r.end;
            let rest = under(&text[range.start + 1..range.end], old)?;
            let renamed = format!("#{new}{rest}");
            (text[range.clone()] != renamed).then_some(Edit {
                range,
                text: renamed,
            })
        })
        .collect()
}

/// The notes of the open folder `root` (`notes`) that use `#old` (or a tag under it): read from
/// disk, or `open` (path, editor text) for the open note. Unreadable and non-UTF-8 notes, and
/// notes whose real file is outside the folder (a symlink), are skipped.
pub fn find(
    notes: &[PathBuf],
    root: &Path,
    old: &str,
    new: &str,
    open: Option<&(PathBuf, String)>,
) -> Vec<Found> {
    let Ok(root) = fs::canonicalize(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for note in notes {
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
        let tags = edits(&text, old, new).len();
        if tags > 0 {
            out.push(Found {
                path: note.clone(),
                text,
                tags,
            });
        }
    }
    out
}

/// Renames the tag in the closed notes `found` (atomic writes); a note that changed on disk
/// since it was read is left alone. Returns how many were written.
pub fn write(found: &[Found], old: &str, new: &str) -> usize {
    found
        .iter()
        .filter(|f| {
            save::read_note(&f.path).is_ok_and(|disk| disk == f.text)
                && save::write_atomic(&f.path, &apply(&f.text, &edits(&f.text, old, new))).is_ok()
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rename(text: &str, old: &str, new: &str) -> String {
        apply(text, &edits(text, old, new))
    }

    #[test]
    fn names() {
        for (input, want) in [
            ("job", "job"),
            ("#job", "job"),
            ("  #work/project ", "work/project"),
            ("việc-làm", "việc-làm"),
            ("a_b/c-d/e1", "a_b/c-d/e1"),
        ] {
            assert_eq!(valid_name(input).as_deref(), Some(want), "{input:?}");
        }
        for input in [
            "",
            "#",
            "##job",
            "1st",
            "two words",
            "a#b",
            "a/",
            "/a",
            "a//b",
            "a-",
            "-a",
            "a.b",
            "fff",
        ] {
            assert_eq!(valid_name(input), None, "{input:?}");
        }
    }

    #[test]
    fn under_old() {
        assert_eq!(under("work", "work"), Some(""));
        assert_eq!(under("Work/project", "work"), Some("/project"));
        assert_eq!(under("work/a/b", "Work/A"), Some("/b"));
        for name in ["older", "workout/x", "wor", "home/work", "work-x"] {
            assert_eq!(under(name, "work"), None, "{name}");
        }
        assert_eq!(under("work", "work/a"), None);
    }

    #[test]
    fn renames_whole_tags_and_children() {
        assert_eq!(
            rename(
                "#work, #Work/Project and (#work/a/b). #older #workout/x #work-x #work/\n",
                "work",
                "job"
            ),
            "#job, #job/Project and (#job/a/b). #older #workout/x #work-x #job/\n"
        );
        assert_eq!(
            rename("#a/b #a/b/c #a/bc #a", "a/b", "x"),
            "#x #x/c #a/bc #a"
        );
        assert_eq!(
            rename("Hôm nay #việc và #việc/nhà", "việc", "làm"),
            "Hôm nay #làm và #làm/nhà"
        );
    }

    #[test]
    fn leaves_code_links_urls_and_headings() {
        let md = "---\ntags: '#work'\n---\n# work\n## Plan #work\n\
`#work` [x](#work) [y](http://a/#work) http://b.com/#work a#work\n\
```\n#work\n```\n~~~\n#work\n~~~\nend #work\r\nlast #work";
        let want = "---\ntags: '#work'\n---\n# work\n## Plan #job\n\
`#work` [x](#work) [y](http://a/#work) http://b.com/#work a#work\n\
```\n#work\n```\n~~~\n#work\n~~~\nend #job\r\nlast #job";
        assert_eq!(rename(md, "work", "job"), want);
    }

    #[test]
    fn same_name_is_no_edit() {
        assert!(edits("#Work #work", "work", "Work").len() == 1);
        assert!(edits("#job", "job", "job").is_empty());
    }

    #[test]
    fn find_and_write() {
        let dir = std::env::temp_dir().join(format!("mm-retag-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let (a, b, c, open) = (
            dir.join("a.md"),
            dir.join("sub/b.md"),
            dir.join("c.md"),
            dir.join("open.md"),
        );
        fs::write(&a, "#work today\n").unwrap();
        fs::write(&b, "#work/project\n").unwrap();
        fs::write(&c, "#older only\n").unwrap();
        fs::write(&open, "on disk\n").unwrap();
        let notes = [
            a.clone(),
            b.clone(),
            c.clone(),
            open.clone(),
            dir.join("gone.md"),
        ];
        let editor = (open.clone(), "#work in the editor\n".to_string());
        let found = find(&notes, &dir, "work", "job", Some(&editor));
        let paths: Vec<&PathBuf> = found.iter().map(|f| &f.path).collect();
        assert_eq!(paths, [&a, &b, &open]);
        assert_eq!(found[2].text, editor.1);
        // b changed on disk after it was read: left alone.
        fs::write(&b, "#work/project edited\n").unwrap();
        assert_eq!(write(&found[..2], "work", "job"), 1);
        assert_eq!(fs::read_to_string(&a).unwrap(), "#job today\n");
        assert_eq!(fs::read_to_string(&b).unwrap(), "#work/project edited\n");
        assert_eq!(fs::read_to_string(&c).unwrap(), "#older only\n");
    }
}
