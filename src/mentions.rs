//! Unlinked Mentions (under Backlinks): the notes that name the open note without linking to
//! it — its file name as a whole word, any case, outside code and links. Pure logic, no GTK;
//! the window runs `find` on a background thread, only while the section is expanded.
//!
//! Link (each row's button) turns one mention into a wiki link to the note: `link_text` is the
//! link written, `link_edit` where it goes; nothing is written when the mention is no longer
//! at the line and column where it was found.

use std::path::{Path, PathBuf};

use crate::relink::{Edit, apply};
use crate::{complete, find, links, lists, notes, save};

/// Shorter names are not looked for: they would match almost everywhere.
pub const MIN_NAME_CHARS: usize = 3;

/// Where a note mentions the target.
#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    /// 1-based line number (lines as `str::lines` counts them).
    pub line: usize,
    /// Byte offset of the mention in its line.
    pub column: usize,
    /// The mention as written.
    pub text: String,
    /// The line (or a window of it around the mention, with "…" where cut), trimmed.
    pub snippet: String,
}

/// The first mention of the target note in another note.
#[derive(Debug, Clone, PartialEq)]
pub struct Mention {
    pub path: PathBuf,
    /// `find::version` of the note's text when it was searched.
    pub version: u64,
    pub spot: Spot,
}

/// The notes among `notes` (other than `target`) mentioning `target`'s name, one entry per
/// note (its first mention), in `notes` order. Unreadable or non-UTF-8 notes are skipped.
/// Returns None as soon as `cancelled()` is true.
pub fn find(
    notes: &[PathBuf],
    target: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<Mention>> {
    let Some(name) = target.file_stem().and_then(|s| s.to_str()) else {
        return Some(Vec::new());
    };
    if name.chars().count() < MIN_NAME_CHARS {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for note in notes.iter().filter(|n| n.as_path() != target) {
        if cancelled() {
            return None;
        }
        let Some(text) = find::read_utf8(note) else {
            continue;
        };
        if let Some(spot) = first_mention(name, &text) {
            out.push(Mention {
                path: note.clone(),
                version: find::version(&text),
                spot,
            });
        }
    }
    Some(out)
}

/// The first whole-word, case-insensitive occurrence of `name` in `text` outside fenced code,
/// code spans and links (`[[Name]]`, `[Name](…)`, URLs).
pub fn first_mention(name: &str, text: &str) -> Option<Spot> {
    let query = find::fold(name.trim());
    // Cheap check first: most notes do not contain the name at all.
    if query.is_empty() || !find::fold(text).contains(&query) {
        return None;
    }
    let mut open = None;
    for (i, line) in text.lines().enumerate() {
        if lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        let mut excluded = None; // Only worked out for lines with the name.
        for m in find::matches(line, &query, find::Options::default()) {
            let before = line[..m.start].chars().next_back();
            let after = line[m.end..].chars().next();
            if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
                continue;
            }
            let excluded = excluded.get_or_insert_with(|| links::code_and_links(line));
            if excluded.iter().any(|r| r.start < m.end && m.start < r.end) {
                continue;
            }
            let (column, text) = (m.start, line[m.clone()].to_string());
            let (snippet, _) = find::snippet(line, vec![m]);
            return Some(Spot {
                line: i + 1,
                column,
                text,
                snippet: snippet.trim().to_string(),
            });
        }
    }
    None
}

/// A mention must not be part of a longer word.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The wiki link replacing `mention` (as written) of `target`: Copy Wiki Link's `[[Name]]` /
/// `[[Folder/Name]]` (`complete::link_name`) when the mention is the note's name ignoring case
/// and accents, else the same with the mention as its text, `[[Name|mention]]`.
pub fn link_text(mention: &str, target: &Path, notes: &[PathBuf], root: &Path) -> String {
    let name = complete::link_name(target, notes, root);
    let stem = target.file_stem().unwrap_or_default().to_string_lossy();
    if notes::folded(mention).eq(notes::folded(&stem)) {
        format!("[[{name}]]")
    } else {
        format!("[[{name}|{mention}]]")
    }
}

/// The edit putting `link` in place of `mention` at 1-based `line` and byte `column` of that
/// line (as `first_mention` found it); None when `text` doesn't have `mention` there any more
/// (the note changed since the search).
pub fn link_edit(
    text: &str,
    line: usize,
    column: usize,
    mention: &str,
    link: &str,
) -> Option<Edit> {
    if line == 0 || mention.is_empty() {
        return None;
    }
    // `split('\n')` counts lines as `str::lines` does; a `\r` before the `\n` stays at the
    // line's end, after any mention.
    let mut start = 0;
    for (n, content) in text.split('\n').enumerate() {
        if n + 1 == line {
            let end = column.checked_add(mention.len())?;
            return (content.get(column..end) == Some(mention)).then(|| Edit {
                range: start + column..start + end,
                text: link.to_string(),
            });
        }
        start += content.len() + 1;
    }
    None
}

/// Links `spot` in a closed note: written atomically, only if the note on disk is still the
/// text searched (`version`) and has the mention at its place.
pub fn link_in_file(
    path: &Path,
    version: u64,
    spot: &Spot,
    link: &str,
) -> Result<(), find::Skipped> {
    let text = find::read_utf8(path)
        .filter(|text| find::version(text) == version)
        .ok_or(find::Skipped::Changed)?;
    let edit =
        link_edit(&text, spot.line, spot.column, &spot.text, link).ok_or(find::Skipped::Changed)?;
    save::write_atomic(path, &apply(&text, &[edit])).map_err(|_| find::Skipped::NotWritten)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(name: &str, text: &str) -> Option<usize> {
        first_mention(name, text).map(|s| s.line)
    }

    /// Fresh empty folder under the system temp dir, removed on drop (also when a test fails).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mm-mentions-{tag}-{}", std::process::id()));
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
    fn whole_word_any_case() {
        assert_eq!(line("Plan", "x\nthe plan is set"), Some(2));
        assert_eq!(line("Plan", "PLAN."), Some(1));
        assert_eq!(line("plan", "(Plan)"), Some(1));
        assert_eq!(line("Plan", "planning, airplane, plan_b, plan2"), None);
        assert_eq!(line("Plan", "Planning\nmy plan"), Some(2));
        assert_eq!(line("Việt", "người việt nam"), Some(1));
        assert_eq!(line("Việt", "việtnam"), None);
        assert_eq!(line("My Note", "see my  note, then my note"), Some(1));
        assert_eq!(line("Plan", "nothing here"), None);
    }

    #[test]
    fn code_is_skipped() {
        assert_eq!(line("Plan", "`plan` and ``a plan``"), None);
        assert_eq!(
            line("Plan", "```\nplan\n```\n~~~md\nPlan\n~~~\nplan"),
            Some(7)
        );
        assert_eq!(line("Plan", "`code` then plan"), Some(1));
    }

    #[test]
    fn links_are_skipped() {
        assert_eq!(line("Plan", "[[Plan]] [[x|the plan]] [[Plan#Goals]]"), None);
        assert_eq!(
            line("Plan", "[Plan](Plan.md) [x](plan.md) ![plan](p.png)"),
            None
        );
        assert_eq!(line("Plan", "https://x.y/plan <https://plan.z>"), None);
        assert_eq!(line("Plan", "[[Plan]] is the plan"), Some(1));
        assert_eq!(line("Plan", "[[Plan]]\nplan"), Some(2));
    }

    #[test]
    fn snippet_is_trimmed_and_cut_around_the_mention() {
        let s = first_mention("Plan", "   the plan  ").unwrap().snippet;
        assert_eq!(s, "the plan");
        let long = format!("{} plan {}", "a".repeat(300), "b".repeat(300));
        let s = first_mention("Plan", &long).unwrap().snippet;
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains(" plan "));
    }

    #[test]
    fn find_skips_the_note_itself_and_short_names() {
        let tmp = TempDir::new("find");
        let dir = &tmp.0;
        let (plan, a, b, ab) = (
            dir.join("Plan.md"),
            dir.join("A.md"),
            dir.join("B.md"),
            dir.join("Ab.md"),
        );
        std::fs::write(&plan, "the plan\n").unwrap();
        std::fs::write(&a, "intro\n[[Plan]]\nour plan, and ab\n").unwrap();
        std::fs::write(&b, "[[Plan]] only\n").unwrap();
        std::fs::write(&ab, [0xff, 0xfe]).unwrap();
        let notes = vec![a.clone(), b, plan.clone(), ab.clone()];
        let found = find(&notes, &plan, &|| false).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].path.as_path(), found[0].spot.line),
            (a.as_path(), 3)
        );
        assert_eq!(found[0].spot.snippet, "our plan, and ab");
        assert_eq!(
            found[0].version,
            find::version("intro\n[[Plan]]\nour plan, and ab\n")
        );
        assert_eq!(find(&notes, &ab, &|| false), Some(Vec::new()));
        assert_eq!(find(&notes, &plan, &|| true), None);
    }

    #[test]
    fn spot_is_the_mention_as_written() {
        let s = first_mention("Plan", "[[Plan]] and the PLAN.").unwrap();
        assert_eq!((s.line, s.column, s.text.as_str()), (1, 17, "PLAN"));
        let s = first_mention("Việt", "a\r\nnói tiếng VIỆT").unwrap();
        assert_eq!((s.line, s.column, s.text.as_str()), (2, 13, "VIỆT"));
    }

    #[test]
    fn link_text_plain_folder_and_alias() {
        let root = Path::new("/n");
        let plan = root.join("Plan.md");
        let cafe = root.join("Café.md");
        let notes = vec![plan.clone(), cafe.clone(), root.join("a.md")];
        assert_eq!(link_text("plan", &plan, &notes, root), "[[Plan]]");
        assert_eq!(link_text("PLAN", &plan, &notes, root), "[[Plan]]");
        assert_eq!(link_text("cafe", &cafe, &notes, root), "[[Café]]");
        // Decomposed é (e + U+0301) is the same name.
        assert_eq!(link_text("cafe\u{301}", &cafe, &notes, root), "[[Café]]");
        assert_eq!(
            link_text("my plan", &plan, &notes, root),
            "[[Plan|my plan]]"
        );
        let other = root.join("work/plan.md");
        let notes = vec![plan.clone(), other.clone()];
        assert_eq!(link_text("Plan", &other, &notes, root), "[[work/plan]]");
        assert_eq!(
            link_text("plans", &other, &notes, root),
            "[[work/plan|plans]]"
        );
    }

    fn linked(text: &str, line: usize, column: usize, mention: &str) -> Option<String> {
        let edit = link_edit(text, line, column, mention, "[[Plan]]")?;
        Some(apply(text, &[edit]))
    }

    #[test]
    fn link_edit_replaces_the_mention_only() {
        assert_eq!(
            linked("x\nthe plan, plan", 2, 4, "plan").as_deref(),
            Some("x\nthe [[Plan]], plan")
        );
        assert_eq!(
            linked("a\r\nthe plan\r\nb", 2, 4, "plan").as_deref(),
            Some("a\r\nthe [[Plan]]\r\nb")
        );
        assert_eq!(linked("plan", 1, 0, "plan").as_deref(), Some("[[Plan]]"));
        assert_eq!(
            linked("người việt", 1, 9, "việt").as_deref(),
            Some("người [[Plan]]")
        );
    }

    #[test]
    fn link_edit_refuses_a_stale_spot() {
        // Moved column, other line, other text, past the end, not a char boundary.
        assert_eq!(linked("the plan", 1, 3, "plan"), None);
        assert_eq!(linked("x\nthe plan", 1, 4, "plan"), None);
        assert_eq!(linked("the PLAN", 1, 4, "plan"), None);
        assert_eq!(linked("the plan", 1, 6, "plan"), None);
        assert_eq!(linked("the plan", 3, 4, "plan"), None);
        assert_eq!(linked("the\nplan", 1, 3, "\npla"), None);
        assert_eq!(linked("é plan", 1, 1, "plan"), None);
        assert_eq!(linked("the plan", 0, 4, "plan"), None);
        assert_eq!(linked("the plan", 1, usize::MAX, "plan"), None);
    }

    #[test]
    fn link_in_file_writes_only_the_text_searched() {
        let tmp = TempDir::new("link");
        let note = tmp.0.join("A.md");
        let text = "intro\nour plan\n";
        std::fs::write(&note, text).unwrap();
        let spot = first_mention("Plan", text).unwrap();
        let version = find::version(text);
        std::fs::write(&note, "intro\nour plan!\n").unwrap();
        assert_eq!(
            link_in_file(&note, version, &spot, "[[Plan]]"),
            Err(find::Skipped::Changed)
        );
        assert_eq!(
            std::fs::read_to_string(&note).unwrap(),
            "intro\nour plan!\n"
        );
        std::fs::write(&note, text).unwrap();
        assert_eq!(link_in_file(&note, version, &spot, "[[Plan]]"), Ok(()));
        assert_eq!(
            std::fs::read_to_string(&note).unwrap(),
            "intro\nour [[Plan]]\n"
        );
        let gone = tmp.0.join("gone.md");
        assert_eq!(
            link_in_file(&gone, version, &spot, "[[Plan]]"),
            Err(find::Skipped::Changed)
        );
    }
}
