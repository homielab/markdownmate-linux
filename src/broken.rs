//! Find Broken Links (main menu, or the Broken Links toggle of Find in Folder): the links in
//! the open folder's notes that lead nowhere. A `[[wiki link]]` is broken when its name
//! resolves to no note and no alias (the resolver of Ctrl+click and the preview), or when its
//! heading part (`[[Note#Heading]]`, `![[Note#Heading]]`, `[[#Heading]]` in the note itself)
//! names no heading of that note (`render::find_heading`, the rule following the link uses) or
//! its `^id` no block (`blocks`);
//! an `![[name.png]]` embed when no image of the folder has that name (`[[name.png]]` without
//! `!` is a note link); a relative Markdown link or image (`[t](a.md)`, `![](assets/x.png)`,
//! `<raw path>`, percent-decoded) when its file does not exist relative to the note (its
//! `#fragment` is not checked). URLs with a scheme (`mailto:` too), absolute paths,
//! `#anchor`-only links, fenced code, code spans and the front matter are skipped. Pure logic,
//! no GTK; the dialog runs `find` on a background thread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::backlinks::{self, LinkIndex};
use crate::links::{self, Target};
use crate::render::{self, WikiIndex, is_embed_image, wiki_key};
use crate::{lists, state};

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

/// The headings and block ids of the notes that links name a heading or block of, each note
/// read once per scan.
pub struct Headings<'a> {
    /// A note's text (the editor's for the open note); None if it can't be read.
    read: &'a dyn Fn(&Path) -> Option<String>,
    /// `render::heading_ids` and the block ids (`blocks::scan`) of each note read; None when it
    /// couldn't be.
    notes: HashMap<PathBuf, Option<Targets>>,
}

/// A note's headings (`render::heading_ids`) and block ids.
type Targets = (Vec<(usize, String)>, Vec<String>);

impl<'a> Headings<'a> {
    pub fn new(read: &'a dyn Fn(&Path) -> Option<String>) -> Self {
        Headings {
            read,
            notes: HashMap::new(),
        }
    }

    /// Whether `note` (whose text is `own`, when the caller has it) has a heading `fragment`
    /// names, as following the link finds it: by id or title, and, for a `%`-escaped part, the
    /// way Ctrl+click decodes it; a block `^id` by its id. True for a note that can't be read
    /// (nothing to check).
    fn has(&mut self, note: &Path, own: Option<&str>, fragment: &str) -> bool {
        let read = self.read;
        let targets = self.notes.entry(note.to_path_buf()).or_insert_with(|| {
            own.map(str::to_string).or_else(|| read(note)).map(|text| {
                let blocks = crate::blocks::scan(&text).into_iter().map(|b| b.id);
                (render::heading_ids(&text), blocks.collect())
            })
        });
        let Some((ids, blocks)) = targets else {
            return true;
        };
        match crate::blocks::block_ref(fragment) {
            Some(id) => blocks.iter().any(|b| b == id),
            None => heading_of(ids, fragment).is_some(),
        }
    }
}

/// The heading of `ids` (`render::heading_ids`) a link's heading part `fragment` reaches, as
/// following the link finds it: `render::find_heading`, then, for a `%`-escaped part, the way
/// Ctrl+click decodes it. None for a block (`^id`).
pub(crate) fn heading_of<'a>(
    ids: &'a [(usize, String)],
    fragment: &str,
) -> Option<&'a (usize, String)> {
    if crate::blocks::block_ref(fragment).is_some() {
        return None;
    }
    render::find_heading(ids, fragment).or_else(|| {
        let decoded = fragment
            .contains('%')
            .then(|| {
                state::unescape(fragment)?
                    .into_os_string()
                    .into_string()
                    .ok()
            })
            .flatten()?;
        render::find_heading(ids, &decoded)
    })
}

/// The heading part of the wiki link `name` (as `links::spans` gives it) as following the link
/// reads it: after the first `#`; trimmed for an embed (the preview embeds that section). None
/// when there is none or it is empty.
pub(crate) fn heading_part(name: &str, embed: bool) -> Option<&str> {
    let (_, part) = name.split_once('#')?;
    let part = if embed { part.trim() } else { part };
    (!part.is_empty()).then_some(part)
}

/// The broken links of `text` (the note at `note`). Wiki names resolve through `wiki`, embedded
/// images through `images`, heading parts against `headings`; `exists` tells whether a linked
/// file is there.
pub fn in_text(
    text: &str,
    note: &Path,
    wiki: &WikiIndex,
    images: &WikiIndex,
    headings: &mut Headings,
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
            let embed = links::is_embed(line, &range);
            // An embed as written: with its `!`.
            let start = if embed { range.start - 1 } else { range.start };
            let as_written = || line[start..range.end].to_string();
            let found = match &target {
                Target::Wiki(name) if embed && is_embed_image(name) => {
                    images.resolve(name, Some(note)).is_none().then(as_written)
                }
                Target::Wiki(name) => {
                    // A missing note is listed as written, its heading unchecked; `[[#Heading]]`
                    // names a heading of this note.
                    let into = match wiki_key(name) {
                        None => Some(note),
                        Some(_) => match wiki.resolve(name, Some(note)) {
                            Some(found) => Some(found.as_path()),
                            None => {
                                out.push(Link {
                                    line: i + 1,
                                    text: as_written(),
                                });
                                continue;
                            }
                        },
                    };
                    into.zip(heading_part(name, embed))
                        .filter(|(into, part)| {
                            let own = (*into == note).then_some(text);
                            !headings.has(into, own, part)
                        })
                        .map(|(_, part)| match crate::blocks::block_ref(part) {
                            Some(_) => format!("{name} — no such block"),
                            None => format!("{name} — no such heading"),
                        })
                }
                Target::Href(href) => file_target(href, note)
                    .is_some_and(|f| !exists(&f))
                    .then(as_written),
            };
            if let Some(shown) = found {
                out.push(Link {
                    line: i + 1,
                    text: shown,
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
/// note list order; `images` are the folder's embeddable images. The open note (path, editor text) is checked as in the editor; other notes
/// are read only when the index lists a link of theirs that may lead nowhere, and those a heading
/// part names once each. None if `cancelled` turned true.
pub fn find(
    index: &LinkIndex,
    images: &WikiIndex,
    open: Option<&(PathBuf, String)>,
    scope: Option<&Path>,
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<NoteLinks>> {
    let exists = |p: &Path| p.exists();
    let wiki = index.wiki();
    let suspects: HashSet<PathBuf> = index.suspects(&exists).into_iter().collect();
    let read = |p: &Path| match open {
        Some((path, text)) if path == p => Some(text.clone()),
        _ => crate::find::read_utf8(p),
    };
    let mut headings = Headings::new(&read);
    let mut out = Vec::new();
    for note in wiki.notes() {
        if cancelled() {
            return None;
        }
        if scope.is_some_and(|dir| !note.starts_with(dir)) {
            continue;
        }
        let links = match open {
            Some((path, text)) if path == note => {
                in_text(text, note, wiki, images, &mut headings, &exists)
            }
            _ if suspects.contains(note) => match crate::find::read_utf8(note) {
                Some(text) => in_text(&text, note, wiki, images, &mut headings, &exists),
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

    /// The images of the folder `/n`.
    fn images() -> WikiIndex {
        WikiIndex::new(vec![PathBuf::from("/n/assets/x.png")], Path::new("/n"))
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

    /// The texts of the folder's notes other than `/n/A.md` (the one checked).
    fn read(p: &Path) -> Option<String> {
        let text = match p.to_str()? {
            "/n/Name.md" => {
                "# Intro\n\nSetext Part\n---\n\n## Plan\n\n## Plan\n\n```\n# Fenced\n```\n\n- item ^abc\n"
            }
            "/n/Sub/Other.md" => "---\ntitle: x\n# yaml\n---\n## Ideas & Plans\n",
            "/n/Sub/Name.md" | "/n/A.md" => "",
            _ => return None,
        };
        Some(text.to_string())
    }

    /// `in_text` of `text` as the note `/n/A.md`.
    fn check(text: &str) -> Vec<Link> {
        let mut headings = Headings::new(&read);
        let note = Path::new("/n/A.md");
        in_text(text, note, &wiki(), &images(), &mut headings, &exists)
    }

    fn broken(text: &str) -> Vec<String> {
        check(text).into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn missing_embeds() {
        // An image by name among the folder's images; `[[x.png]]` is a link to a note.
        assert_eq!(
            broken(
                "![[x.png|300]] ![[ASSETS/X.PNG]] ![[gone.png]] [[x.png]]\n![[Name#Setext Part]] ![[Gone]]"
            ),
            ["![[gone.png]]", "[[x.png]]", "![[Gone]]"]
        );
        assert!(broken("`![[gone.png]]`\n```\n![[Gone]]\n```\n").is_empty());
    }

    #[test]
    fn missing_wiki_target() {
        assert_eq!(broken("see [[Missing]] and [[Name]]"), ["[[Missing]]"]);
        assert_eq!(
            broken("[[Gone|shown]] [[Gone#Part]]"),
            ["[[Gone|shown]]", "[[Gone#Part]]"]
        );
        assert!(broken("[[name.md]] [[NAME]]").is_empty());
    }

    #[test]
    fn missing_heading() {
        // By title in any case, by id (`plan-1`: the second "Plan"), ATX or setext.
        assert!(
            broken("[[Name#Intro]] [[Name#INTRO|x]] [[Name#setext part]] [[Name#plan-1]]")
                .is_empty()
        );
        assert_eq!(
            broken("[[Name#Gone]] [[Name#Gone|text]] ![[Name#Gone]] [[Name#plan-2]]"),
            [
                "Name#Gone — no such heading",
                "Name#Gone — no such heading",
                "Name#Gone — no such heading",
                "Name#plan-2 — no such heading",
            ]
        );
        // A heading inside a fence or the front matter does not count.
        assert_eq!(
            broken("[[Name#Fenced]] [[Other#yaml]]"),
            [
                "Name#Fenced — no such heading",
                "Other#yaml — no such heading"
            ]
        );
        // An empty part opens the note; the line is reported where the link is.
        assert!(broken("[[Name#]] ![[Name# ]]").is_empty());
        assert_eq!(
            check("x\n[[Name#Gone]]"),
            [Link {
                line: 2,
                text: "Name#Gone — no such heading".into()
            }]
        );
    }

    #[test]
    fn heading_part_follows_the_link_rule() {
        // Ctrl+click jumps with the part as written (spaces count) and decodes `%` escapes; an
        // embed shows the section of its trimmed part.
        assert_eq!(broken("[[Name# Intro]]"), ["Name# Intro — no such heading"]);
        assert!(broken("![[Name# Intro ]] [[Name#Setext%20Part]] [[Name#intro ]]").is_empty());
        // Markdown links: only the file is checked.
        assert!(broken("[a](Name.md#gone) [b](#gone)").is_empty());
    }

    #[test]
    fn missing_block() {
        assert!(broken("[[Name#^abc]] [[Name#^abc|x]] ![[Name#^abc]] [[#^own]] ^own").is_empty());
        assert_eq!(
            broken("[[Name#^nope]] ![[Name#^nope]] [[#^gone]] [[Name#^ABC]]"),
            [
                "Name#^nope — no such block",
                "Name#^nope — no such block",
                "#^gone — no such block",
                "Name#^ABC — no such block",
            ]
        );
        // A block id is no heading (Rename Heading leaves such links alone).
        assert!(heading_of(&[(1, "abc".to_string())], "^abc").is_none());
    }

    #[test]
    fn heading_in_this_note() {
        assert_eq!(
            broken("# Top\n\n[[#Top]] [[#top|up]] [[#Gone]] [[A#Gone]] [[A#top]]"),
            ["#Gone — no such heading", "A#Gone — no such heading"]
        );
    }

    #[test]
    fn note_missing_is_reported_once() {
        assert_eq!(
            broken("[[Gone#Part]] ![[Gone#Part]]"),
            ["[[Gone#Part]]", "![[Gone#Part]]"]
        );
    }

    #[test]
    fn alias_then_heading() {
        // The note through its alias first, then its headings.
        assert!(broken("[[Nickname#Ideas & Plans]] [[nickname#ideas--plans|x]]").is_empty());
        assert_eq!(
            broken("[[Nickname#Intro]]"),
            ["Nickname#Intro — no such heading"]
        );
    }

    #[test]
    fn headings_read_once_per_note() {
        let reads = std::cell::RefCell::new(Vec::new());
        let counting = |p: &Path| {
            reads.borrow_mut().push(p.to_path_buf());
            read(p)
        };
        let mut headings = Headings::new(&counting);
        let note = Path::new("/n/A.md");
        for text in ["[[Name#A]] [[Name#B]]", "![[Name#C]]\n[[A#X]] [[#Y]]"] {
            in_text(text, note, &wiki(), &images(), &mut headings, &exists);
        }
        // The checked note's own headings come from its text, not a read.
        assert_eq!(*reads.borrow(), [PathBuf::from("/n/Name.md")]);
        // A note that can't be read has nothing to check.
        let none = |_: &Path| None;
        let mut headings = Headings::new(&none);
        let found = in_text(
            "[[Name#Gone]]",
            note,
            &wiki(),
            &images(),
            &mut headings,
            &exists,
        );
        assert!(found.is_empty());
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
            &images(),
            &mut Headings::new(&read),
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
        let found = check("x\n\n![a](<img/a b.png>)");
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
        let found = check(text);
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
        let found = check(text);
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
            ("E.md", "# Here\n[[E#Here]] [[B#Nowhere]] [[Sub/C#Gone]]\n"),
        ];
        let notes: Vec<PathBuf> = files.iter().map(|(n, _)| root.join(n)).collect();
        for (note, (_, text)) in notes.iter().zip(files) {
            std::fs::create_dir_all(note.parent().unwrap()).unwrap();
            std::fs::write(note, text).unwrap();
        }
        let mut index = LinkIndex::default();
        index.sync(&notes, &Aliases::default(), &root, &HashSet::new());
        let never = || false;

        let found = find(&index, &images(), None, None, &never).unwrap();
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
                (
                    notes[4].clone(),
                    vec![
                        (2, "B#Nowhere — no such heading"),
                        (2, "Sub/C#Gone — no such heading")
                    ]
                ),
            ]
        );
        assert_eq!(summary(&found), "6 broken links in 3 notes");

        // The image appears; the open note is checked from the editor's text, unsaved edits too.
        std::fs::create_dir_all(root.join("img")).unwrap();
        std::fs::write(root.join("img/x.png"), b"png").unwrap();
        let open = (notes[1].clone(), "[[Typo]]\n".to_string());
        let found = find(&index, &images(), Some(&open), None, &never).unwrap();
        let paths: Vec<&PathBuf> = found.iter().map(|n| &n.path).collect();
        assert_eq!(paths, [&notes[0], &notes[1], &notes[2], &notes[4]]);
        assert_eq!(summary(&found), "6 broken links in 4 notes");

        // A heading added in the editor counts before it is saved.
        let open = (notes[1].clone(), "# Nowhere\n".to_string());
        let found = find(&index, &images(), Some(&open), None, &never).unwrap();
        assert_eq!(summary(&found), "4 broken links in 3 notes");

        // A scope keeps the notes under that folder.
        let found = find(&index, &images(), None, Some(&root.join("Sub")), &never).unwrap();
        assert_eq!(summary(&found), "2 broken links in 1 note");
        let one = NoteLinks {
            path: notes[0].clone(),
            links: vec![found[0].links[0].clone()],
        };
        assert_eq!(summary(&[one]), "1 broken link in 1 note");
        assert!(find(&index, &images(), None, None, &|| true).is_none());
        assert_eq!(summary(&[]), "No broken links");
    }
}
