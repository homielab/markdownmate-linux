//! Unlinked Mentions (under Backlinks): the notes that name the open note without linking to
//! it — its file name as a whole word, any case, outside code and links. Pure logic, no GTK;
//! the window runs `find` on a background thread, only while the section is expanded.

use std::path::{Path, PathBuf};

use crate::{find, links, lists};

/// Shorter names are not looked for: they would match almost everywhere.
pub const MIN_NAME_CHARS: usize = 3;

/// The first mention of the target note in another note.
#[derive(Debug, PartialEq)]
pub struct Mention {
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// The line (or a window of it around the mention, with "…" where cut), trimmed.
    pub snippet: String,
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
        if let Some((line, snippet)) = first_mention(name, &text) {
            out.push(Mention {
                path: note.clone(),
                line,
                snippet,
            });
        }
    }
    Some(out)
}

/// The first whole-word, case-insensitive occurrence of `name` in `text` outside fenced code,
/// code spans and links (`[[Name]]`, `[Name](…)`, URLs): its 1-based line and snippet.
pub fn first_mention(name: &str, text: &str) -> Option<(usize, String)> {
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
        for m in find::matches(line, &query) {
            let before = line[..m.start].chars().next_back();
            let after = line[m.end..].chars().next();
            if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
                continue;
            }
            let excluded = excluded.get_or_insert_with(|| links::code_and_links(line));
            if excluded.iter().any(|r| r.start < m.end && m.start < r.end) {
                continue;
            }
            let (snippet, _) = find::snippet(line, vec![m]);
            return Some((i + 1, snippet.trim().to_string()));
        }
    }
    None
}

/// A mention must not be part of a longer word.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(name: &str, text: &str) -> Option<usize> {
        first_mention(name, text).map(|(l, _)| l)
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
        let (_, s) = first_mention("Plan", "   the plan  ").unwrap();
        assert_eq!(s, "the plan");
        let long = format!("{} plan {}", "a".repeat(300), "b".repeat(300));
        let (_, s) = first_mention("Plan", &long).unwrap();
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains(" plan "));
    }

    #[test]
    fn find_skips_the_note_itself_and_short_names() {
        let dir = std::env::temp_dir().join(format!("mm-mentions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
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
        assert_eq!((found[0].path.as_path(), found[0].line), (a.as_path(), 3));
        assert_eq!(found[0].snippet, "our plan, and ab");
        assert_eq!(find(&notes, &ab, &|| false), Some(Vec::new()));
        assert_eq!(find(&notes, &plan, &|| true), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
