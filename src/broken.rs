//! Find Broken Links (main menu, or the Broken Links toggle of Find in Folder): the links in
//! the open folder's notes that lead nowhere. A `[[wiki link]]` is broken when its name
//! resolves to no note and no alias (the resolver of Ctrl+click and the preview; `[[#Heading]]`
//! and heading parts are not checked); a relative Markdown link or image (`[t](a.md)`,
//! `![](assets/x.png)`, `<raw path>`, percent-decoded) when its file does not exist relative to
//! the note. URLs with a scheme (`mailto:` too), absolute paths, `#anchor`-only links, fenced
//! code, code spans and the front matter are skipped. Pure logic, no GTK; the dialog runs
//! `find` on a background thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::backlinks::{self, LinkIndex};
use crate::links::{self, Target};
use crate::lists;
use crate::render::{WikiIndex, wiki_key};

/// A broken link.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// 1-based line number (lines as `str::lines` counts them).
    pub line: usize,
    /// The link as written (`[[Name|text]]`, `![alt](x.png)`).
    pub text: String,
}

/// A note and its broken links, in line order.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteLinks {
    pub path: PathBuf,
    pub links: Vec<Link>,
}

/// The broken links of `text` (the note at `note`). Wiki names resolve through `wiki`; `exists`
/// tells whether a linked file is there.
pub fn in_text(
    text: &str,
    note: &Path,
    wiki: &WikiIndex,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<Link> {
    let skip = lists::front_matter_lines(text);
    let mut open = None;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i < skip || lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        for (range, target) in links::spans(line) {
            let broken = match &target {
                Target::Wiki(name) => {
                    wiki_key(name).is_some() && wiki.resolve(name, Some(note)).is_none()
                }
                Target::Href(href) => file_target(href, note).is_some_and(|f| !exists(&f)),
            };
            if broken {
                out.push(Link {
                    line: i + 1,
                    text: line[range].to_string(),
                });
            }
        }
    }
    out
}

/// The file a relative link in `note` points to; None for URLs, `mailto:`, absolute paths and
/// `#anchor`-only links, which are not checked.
fn file_target(href: &str, note: &Path) -> Option<PathBuf> {
    let path = href.split(['#', '?']).next()?;
    if path.is_empty() || path.starts_with('/') || path.starts_with('~') {
        return None;
    }
    backlinks::href_path(href, note)
}

/// The broken links of the notes `index` was synced with (only those under `scope`, if any), in
/// note list order. The open note (path, editor text) is checked as in the editor; other notes
/// are read only when the index lists a link of theirs that leads nowhere. None if
/// `cancelled` turned true.
pub fn find(
    index: &LinkIndex,
    open: Option<&(PathBuf, String)>,
    scope: Option<&Path>,
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<NoteLinks>> {
    let exists = |p: &Path| p.exists();
    let wiki = index.wiki();
    let suspects: HashSet<PathBuf> = index.suspects(&exists).into_iter().collect();
    let mut out = Vec::new();
    for note in wiki.notes() {
        if cancelled() {
            return None;
        }
        if scope.is_some_and(|dir| !note.starts_with(dir)) {
            continue;
        }
        let links = match open {
            Some((path, text)) if path == note => in_text(text, note, wiki, &exists),
            _ if suspects.contains(note) => match crate::find::read_utf8(note) {
                Some(text) => in_text(&text, note, wiki, &exists),
                None => continue,
            },
            _ => continue,
        };
        if !links.is_empty() {
            out.push(NoteLinks {
                path: note.clone(),
                links,
            });
        }
    }
    Some(out)
}

/// "N broken links in M notes", or "No broken links".
pub fn summary(found: &[NoteLinks]) -> String {
    let links: usize = found.iter().map(|n| n.links.len()).sum();
    if links == 0 {
        return "No broken links".to_string();
    }
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    format!(
        "{} in {}",
        plural(links, "broken link", "broken links"),
        plural(found.len(), "note", "notes")
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::aliases::Aliases;

    /// Notes of a folder `/n`, `Sub/Name.md` with the alias "Nickname".
    fn wiki() -> WikiIndex {
        let notes: Vec<PathBuf> = ["/n/A.md", "/n/Name.md", "/n/Sub/Name.md", "/n/Sub/Other.md"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let aliases = Aliases::new(HashMap::from([(
            PathBuf::from("/n/Sub/Other.md"),
            vec!["Nickname".to_string()],
        )]));
        WikiIndex::with_aliases(notes, Path::new("/n"), &aliases)
    }

    /// The files of the folder besides its notes.
    fn exists(p: &Path) -> bool {
        let files = [
            "/n/A.md",
            "/n/Name.md",
            "/n/Sub/Name.md",
            "/n/Sub/Other.md",
            "/n/assets/x.png",
            "/n/My Note.md",
            "/n",
            "/n/Sub",
        ];
        files.iter().any(|f| Path::new(f) == p)
    }

    fn broken(text: &str) -> Vec<String> {
        in_text(text, Path::new("/n/A.md"), &wiki(), &exists)
            .into_iter()
            .map(|l| l.text)
            .collect()
    }

    #[test]
    fn missing_wiki_target() {
        assert_eq!(broken("see [[Missing]] and [[Name]]"), ["[[Missing]]"]);
        assert_eq!(
            broken("[[Gone|shown]] [[Gone#Part]]"),
            ["[[Gone|shown]]", "[[Gone#Part]]"]
        );
        // Heading parts are not checked; a heading-only link is not a note link.
        assert!(broken("[[Name#No such heading]] [[#Somewhere]]").is_empty());
        assert!(broken("[[name.md]] [[NAME]]").is_empty());
    }

    #[test]
    fn alias_resolves() {
        assert!(broken("[[Nickname]] [[nickname|x]]").is_empty());
        assert_eq!(broken("[[Nickname2]]"), ["[[Nickname2]]"]);
    }

    #[test]
    fn folder_qualified_name() {
        assert!(broken("[[Sub/Name]] [[Sub/Other]] [[/Sub/Other]]").is_empty());
        assert_eq!(broken("[[Other/Name]]"), ["[[Other/Name]]"]);
    }

    #[test]
    fn missing_relative_file() {
        assert_eq!(
            broken("[a](Name.md) [b](Gone.md) [c](Sub/Gone.md#top)"),
            ["[b](Gone.md)", "[c](Sub/Gone.md#top)"]
        );
        assert!(broken("[a](./Sub/../Name.md) [b](<My Note.md>) [c](My%20Note.md)").is_empty());
        assert_eq!(broken("[a](<My Notes.md>)"), ["[a](<My Notes.md>)"]);
        let from_sub = in_text(
            "[up](../Name.md) [x](Name2.md)",
            Path::new("/n/Sub/Other.md"),
            &wiki(),
            &exists,
        );
        assert_eq!(
            from_sub,
            [Link {
                line: 1,
                text: "[x](Name2.md)".into()
            }]
        );
    }

    #[test]
    fn missing_image() {
        assert_eq!(
            broken("![ok](assets/x.png)\n![gone](assets/y.png)"),
            ["![gone](assets/y.png)"]
        );
        let found = in_text(
            "x\n\n![a](<img/a b.png>)",
            Path::new("/n/A.md"),
            &wiki(),
            &exists,
        );
        assert_eq!(
            found,
            [Link {
                line: 3,
                text: "![a](<img/a b.png>)".into()
            }]
        );
    }

    #[test]
    fn urls_mailto_anchors_and_absolute_paths_are_skipped() {
        let text = "[w](https://example.com/Gone.md) <https://x.org> https://x.org/a \
                    [m](mailto:me@example.com) <mailto:me@example.com> [t](tel:123) \
                    [h](#heading) [e]() [abs](/tmp/gone.md) [home](~/gone.md) [p](//host/x.png)";
        assert!(broken(text).is_empty());
    }

    #[test]
    fn code_is_skipped() {
        let text = "`[[Missing]]` ``[a](gone.md)``\n```\n[[Missing]]\n```\n~~~\n![x](gone.png)\n~~~\n[[Missing]]";
        let found = in_text(text, Path::new("/n/A.md"), &wiki(), &exists);
        assert_eq!(
            found,
            [Link {
                line: 8,
                text: "[[Missing]]".into()
            }]
        );
    }

    #[test]
    fn front_matter_is_skipped() {
        let text = "---\nup: \"[[Missing]]\"\nimage: [x](gone.png)\n---\n[[Missing]]";
        let found = in_text(text, Path::new("/n/A.md"), &wiki(), &exists);
        assert_eq!(
            found,
            [Link {
                line: 5,
                text: "[[Missing]]".into()
            }]
        );
    }

    /// Fresh empty folder under the system temp dir, removed on drop (also when a test fails).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mm-broken-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn find_over_a_folder_and_the_count() {
        let dir = TempDir::new("find");
        let root = dir.0.clone();
        let files = [
            ("A.md", "[[B]] [[Gone]]\n![i](img/x.png)\n"),
            ("B.md", "fine: [[A]] [a](A.md)\n"),
            (
                "Sub/C.md",
                "[[Nowhere]]\n[up](../B.md) [x](../Missing.md)\n",
            ),
            ("D.md", "---\nup: \"[[Gone]]\"\n---\nno links\n"),
        ];
        let notes: Vec<PathBuf> = files.iter().map(|(n, _)| root.join(n)).collect();
        for (note, (_, text)) in notes.iter().zip(files) {
            std::fs::create_dir_all(note.parent().unwrap()).unwrap();
            std::fs::write(note, text).unwrap();
        }
        let mut index = LinkIndex::default();
        index.sync(&notes, &Aliases::default(), &root, &HashSet::new());
        let never = || false;

        let found = find(&index, None, None, &never).unwrap();
        let shape: Vec<(PathBuf, Vec<(usize, &str)>)> = found
            .iter()
            .map(|n| {
                let links = n.links.iter().map(|l| (l.line, l.text.as_str())).collect();
                (n.path.clone(), links)
            })
            .collect();
        assert_eq!(
            shape,
            [
                (
                    notes[0].clone(),
                    vec![(1, "[[Gone]]"), (2, "![i](img/x.png)")]
                ),
                (
                    notes[2].clone(),
                    vec![(1, "[[Nowhere]]"), (2, "[x](../Missing.md)")]
                ),
            ]
        );
        assert_eq!(summary(&found), "4 broken links in 2 notes");

        // The image appears; the open note is checked from the editor's text, unsaved edits too.
        std::fs::create_dir_all(root.join("img")).unwrap();
        std::fs::write(root.join("img/x.png"), b"png").unwrap();
        let open = (notes[1].clone(), "[[Typo]]\n".to_string());
        let found = find(&index, Some(&open), None, &never).unwrap();
        let paths: Vec<&PathBuf> = found.iter().map(|n| &n.path).collect();
        assert_eq!(paths, [&notes[0], &notes[1], &notes[2]]);
        assert_eq!(summary(&found), "4 broken links in 3 notes");

        // A scope keeps the notes under that folder.
        let found = find(&index, None, Some(&root.join("Sub")), &never).unwrap();
        assert_eq!(summary(&found), "2 broken links in 1 note");
        let one = NoteLinks {
            path: notes[0].clone(),
            links: vec![found[0].links[0].clone()],
        };
        assert_eq!(summary(&[one]), "1 broken link in 1 note");
        assert!(find(&index, None, None, &|| true).is_none());
        assert_eq!(summary(&[]), "No broken links");
    }
}
