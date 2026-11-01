//! Find Open Tasks (main menu, or the Open Tasks toggle of Find in Folder): every unchecked task
//! in the open folder's notes. A task line is optional indentation (spaces, tabs) and `>` quote
//! markers, a list marker (`-`, `*`, `+`, `1.` or `1)`), one space, `[ ]`, then a space or the
//! end of the line. `[x]` / `[X]` are done and not listed. Lines in fenced code and in the front
//! matter are skipped. Pure logic, no GTK; the dialog runs `find` on a background thread.

use std::path::{Path, PathBuf};

use crate::lists;

/// The longest task text kept for a row (the rest is cut with "…").
const SHOWN_CHARS: usize = 200;

/// An open task.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    /// 1-based line number (lines as `str::lines` counts them).
    pub line: usize,
    /// The text after `[ ] `, trimmed; empty for an empty task.
    pub text: String,
}

/// A note and its open tasks, in line order.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteTasks {
    pub path: PathBuf,
    pub tasks: Vec<Task>,
}

/// What `find` found: the notes with open tasks, and whether it stopped at `limit`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub notes: Vec<NoteTasks>,
    pub more: bool,
}

/// The task text of `line` if it is an open task (trimmed, possibly empty), else None.
pub fn task_text(line: &str) -> Option<&str> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut s = line.trim_start_matches([' ', '\t']);
    while let Some(rest) = s.strip_prefix('>') {
        s = rest.trim_start_matches([' ', '\t']);
    }
    let after_marker = match s.strip_prefix(['-', '*', '+']) {
        Some(rest) => rest,
        None => {
            let digits = s.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || digits > 9 {
                return None;
            }
            s[digits..].strip_prefix(['.', ')'])?
        }
    };
    let rest = after_marker.strip_prefix(" [ ]")?;
    if rest.is_empty() {
        return Some("");
    }
    rest.strip_prefix(' ').map(|t| t.trim_matches([' ', '\t']))
}

/// The open tasks of `text`, in line order.
pub fn in_text(text: &str) -> Vec<Task> {
    if !text.contains("[ ]") {
        return Vec::new();
    }
    let skip = lists::front_matter_lines(text);
    let mut open = None;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if i < skip || lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        if let Some(task) = task_text(line) {
            let text = match task.char_indices().nth(SHOWN_CHARS) {
                Some((cut, _)) => format!("{}…", &task[..cut]),
                None => task.to_string(),
            };
            out.push(Task { line: i + 1, text });
        }
    }
    out
}

/// The open tasks of `notes` (in their order), at most `limit` in all. The open note (path,
/// editor text) is read as in the editor; notes that cannot be read or are not UTF-8 are
/// skipped. None as soon as `cancelled` turns true.
pub fn find(
    notes: &[PathBuf],
    open: Option<&(PathBuf, String)>,
    limit: usize,
    cancelled: &dyn Fn() -> bool,
) -> Option<Found> {
    find_with(notes, limit, cancelled, &|path| match open {
        Some((p, text)) if p == path => Some(text.clone()),
        _ => crate::find::read_utf8(path),
    })
}

fn find_with(
    notes: &[PathBuf],
    limit: usize,
    cancelled: &dyn Fn() -> bool,
    read: &dyn Fn(&Path) -> Option<String>,
) -> Option<Found> {
    let mut found = Found::default();
    let mut left = limit;
    for note in notes {
        if cancelled() {
            return None;
        }
        let Some(text) = read(note) else {
            continue;
        };
        let mut tasks = in_text(&text);
        if tasks.is_empty() {
            continue;
        }
        if tasks.len() > left {
            tasks.truncate(left);
            found.more = true;
        }
        left -= tasks.len();
        if !tasks.is_empty() {
            found.notes.push(NoteTasks {
                path: note.clone(),
                tasks,
            });
        }
        if found.more {
            break;
        }
    }
    Some(found)
}

/// "N open tasks in M notes" (with "(stopped at L)" when cut at `limit`), or "No open tasks".
pub fn summary(found: &Found, limit: usize) -> String {
    let tasks: usize = found.notes.iter().map(|n| n.tasks.len()).sum();
    if tasks == 0 {
        return "No open tasks".to_string();
    }
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let summary = format!(
        "{} in {}",
        plural(tasks, "open task", "open tasks"),
        plural(found.notes.len(), "note", "notes")
    );
    if found.more {
        format!("{summary} (stopped at {limit})")
    } else {
        summary
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn texts(text: &str) -> Vec<String> {
        in_text(text).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn six_marker_forms() {
        let text = "- [ ] dash\n* [ ] star\n+ [ ] plus\n1. [ ] dot\n12) [ ] paren\n3) [ ] three";
        assert_eq!(
            texts(text),
            ["dash", "star", "plus", "dot", "paren", "three"]
        );
    }

    #[test]
    fn not_tasks() {
        let text = "-  [ ] two spaces\n- [ ]no space after\n-[ ] no space before\n\
                    [ ] no marker\n- [] no space inside\na. [ ] letter\n\
                    Some - [ ] in a sentence\n1234567890. [ ] ten digits\n- [ ]\ttab after";
        assert!(texts(text).is_empty());
    }

    #[test]
    fn nested_and_quoted_tasks() {
        let text = "- [ ] top\n  - [ ] nested\n\t\t* [ ] tabbed\n> - [ ] quoted\n\
                    > > 1. [ ] twice quoted\n>   - [ ] nested in quote\n  > + [ ] indented quote";
        assert_eq!(
            texts(text),
            [
                "top",
                "nested",
                "tabbed",
                "quoted",
                "twice quoted",
                "nested in quote",
                "indented quote"
            ]
        );
    }

    #[test]
    fn checked_tasks_are_done() {
        assert_eq!(texts("- [x] done\n- [X] done too\n- [ ] open"), ["open"]);
    }

    #[test]
    fn fenced_code_is_skipped() {
        let text = "```\n- [ ] in code\n```\n~~~md\n- [ ] tilde code\n~~~\n- [ ] after";
        assert_eq!(texts(text), ["after"]);
    }

    #[test]
    fn front_matter_is_skipped() {
        assert_eq!(
            texts("---\ntitle: x\n- [ ] in front matter\n---\n- [ ] body"),
            ["body"]
        );
        // Not at the top: no front matter, so a task after a rule counts.
        assert_eq!(
            texts("Intro\n---\n- [ ] after a rule\n---"),
            ["after a rule"]
        );
    }

    #[test]
    fn empty_task_and_trimmed_text() {
        let tasks = in_text("- [ ]\n- [ ] \n- [ ]   spaced out  \t\n");
        let texts: Vec<_> = tasks.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts, ["", "", "spaced out"]);
        assert_eq!(tasks.iter().map(|t| t.line).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn crlf_lines() {
        let tasks = in_text("# Title\r\n\r\nText\r\n  - [ ] Call Ana\r\n- [ ]\r\n");
        assert_eq!(
            tasks,
            [
                Task {
                    line: 4,
                    text: "Call Ana".into()
                },
                Task {
                    line: 5,
                    text: String::new()
                }
            ]
        );
    }

    #[test]
    fn long_text_is_cut() {
        let long = "é".repeat(250);
        let text = &texts(&format!("- [ ] {long}"))[0];
        assert_eq!(text.chars().count(), SHOWN_CHARS + 1);
        assert!(text.ends_with('…'));
    }

    fn scan(texts: &HashMap<PathBuf, &str>, notes: &[PathBuf], limit: usize) -> Found {
        find_with(notes, limit, &|| false, &|p| {
            texts.get(p).map(|t| t.to_string())
        })
        .unwrap()
    }

    #[test]
    fn count_order_and_summary() {
        let notes: Vec<PathBuf> = ["/n/B.md", "/n/A.md", "/n/C.md", "/n/Gone.md"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let texts = HashMap::from([
            (notes[0].clone(), "- [ ] one\n- [x] done\n- [ ] two"),
            (notes[1].clone(), "Nothing open"),
            (notes[2].clone(), "1. [ ]"),
        ]);
        let found = scan(&texts, &notes, 500);
        assert!(!found.more);
        assert_eq!(found.notes.len(), 2);
        // Notes in the given (sidebar) order, lines in order.
        assert_eq!(found.notes[0].path, notes[0]);
        assert_eq!(
            found.notes[0]
                .tasks
                .iter()
                .map(|t| t.line)
                .collect::<Vec<_>>(),
            [1, 3]
        );
        assert_eq!(found.notes[1].tasks[0].text, "");
        assert_eq!(summary(&found, 500), "3 open tasks in 2 notes");
        assert_eq!(summary(&Found::default(), 500), "No open tasks");

        let one = scan(&texts, &notes[2..], 500);
        assert_eq!(summary(&one, 500), "1 open task in 1 note");

        let cut = scan(&texts, &notes, 2);
        assert!(cut.more);
        assert_eq!(cut.notes.len(), 1);
        assert_eq!(summary(&cut, 2), "2 open tasks in 1 note (stopped at 2)");
    }

    #[test]
    fn open_note_is_read_from_the_editor() {
        let note = PathBuf::from("/does/not/exist.md");
        let open = (note.clone(), "- [ ] unsaved".to_string());
        let found = find(&[note], Some(&open), 500, &|| false).unwrap();
        assert_eq!(found.notes[0].tasks[0].text, "unsaved");
        assert!(find(&[], None, 500, &|| true).is_some());
        let gone = PathBuf::from("/does/not/exist.md");
        assert!(find(&[gone], None, 500, &|| true).is_none());
    }
}
