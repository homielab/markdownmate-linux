//! New Note from Selection: moves the editor's selection into a note of its own and leaves a
//! `[[wiki link]]` to it in its place. The name suggestion and the text of the new note and of
//! the replacement are pure (tested here); `ask` is the dialog, `Editor::extract_selection`
//! does the work.

use std::rc::Rc;

use adw::prelude::*;

use crate::editor::Editor;
use crate::naming;

/// Longest suggested name, in characters.
const NAME_LIMIT: usize = 60;

/// The note name suggested for `selection`: its first non-empty line, as heading text if it is
/// a heading, without Markdown markers (`#`, `>`, list bullets and numbers, `[ ]` / `[x]`,
/// `**`, backticks, `[[` / `]]`), `/` turned into `-` and leading dots dropped (New File's
/// rules), trimmed to 60 characters. "Untitled" when nothing is left.
pub fn suggested_name(selection: &str) -> String {
    let line = selection
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let mut text = line;
    loop {
        let before = text;
        text = strip_marker(text).trim_start();
        if text == before {
            break;
        }
    }
    // A closing `#` sequence of an ATX heading.
    let trimmed = text.trim_end_matches('#');
    if trimmed.len() < text.len() && (trimmed.is_empty() || trimmed.ends_with(char::is_whitespace))
    {
        text = trimmed;
    }
    let mut name = text
        .replace("**", "")
        .replace('`', "")
        .replace("[[", "")
        .replace("]]", "")
        .replace('/', "-");
    name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.trim_start_matches('.');
    let name: String = name.chars().take(NAME_LIMIT).collect();
    let name = name.trim_end();
    if name.is_empty() {
        "Untitled".to_string()
    } else {
        name.to_string()
    }
}

/// `text` without one leading block marker: `#`… (heading), `>`, `-` / `*` / `+` / `1.` /
/// `1)` followed by a space, or a task box `[ ]` / `[x]`. Unchanged when it has none.
fn strip_marker(text: &str) -> &str {
    let hashes = text.len() - text.trim_start_matches('#').len();
    if hashes > 0 {
        return &text[hashes..];
    }
    if let Some(rest) = text.strip_prefix('>') {
        return rest;
    }
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = text.strip_prefix(bullet) {
            return rest;
        }
    }
    if text.trim_end() == "-" || text.trim_end() == "*" || text.trim_end() == "+" {
        return "";
    }
    for task in ["[ ]", "[x]", "[X]"] {
        if let Some(rest) = text.strip_prefix(task) {
            return rest;
        }
    }
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0 {
        let rest = &text[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return rest;
        }
    }
    text
}

/// `name`, or "name 2", "name 3", … — the first that `taken` refuses (New File's numbering).
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    (1..=10_000)
        .map(|n| naming::numbered(name, "", n))
        .find(|n| !taken(n))
        .unwrap_or_else(|| name.to_string())
}

/// `selection` split into leading blank lines, the text, and trailing blank lines (from the
/// line break after the last text): the text's byte range in `selection`. It keeps the first
/// line's indentation and, before a line break, the last line's trailing spaces (a Markdown
/// line break); spaces after the text inside a line stay outside (kept beside the link).
fn parts(selection: &str) -> (usize, usize) {
    let lead = selection.len() - selection.trim_start().len();
    let start = selection[..lead].rfind('\n').map_or(0, |i| i + 1);
    let text_end = selection.trim_end().len();
    let end = selection[text_end..]
        .find('\n')
        .map_or(text_end, |i| text_end + i);
    if text_end <= start {
        // Only blank space.
        return (start, start);
    }
    (start, end)
}

/// The new note's text: the selection without its leading and trailing blank lines, ending in
/// one line break. None when the selection is only blank space.
pub fn note_text(selection: &str) -> Option<String> {
    let (start, end) = parts(selection);
    (start < end).then(|| format!("{}\n", &selection[start..end]))
}

/// What replaces the selection in the source note: the link where the text was, keeping the
/// selection's leading and trailing blank lines, so a link for whole lines stands on its own
/// line (and paragraphs around it stay apart); inside a line it is just the link.
pub fn replacement(selection: &str, link: &str) -> String {
    let (start, end) = parts(selection);
    format!("{}{link}{}", &selection[..start], &selection[end..])
}

/// Where the link starts in `replacement(selection, …)`, in characters (buffer offsets), so
/// the source note can keep it selected.
pub fn link_start(selection: &str) -> usize {
    selection[..parts(selection).0].chars().count()
}

/// The wiki link to the note named `name` (`complete::link_name`).
pub fn link(name: &str) -> String {
    format!("[[{name}]]")
}

/// The selection being moved: the note it is in, its char offsets in the buffer, its text.
pub(crate) struct Selection {
    pub(crate) note: std::path::PathBuf,
    pub(crate) start: i32,
    pub(crate) end: i32,
    pub(crate) text: String,
}

/// Main menu ▸ New Note from Selection…: asks for the name (suggested from the selection,
/// numbered when taken; checked as typed with New File's rules), then
/// `Editor::extract_selection`. Nothing without a selected text in an open note.
pub(crate) fn ask(editor: &Rc<Editor>) {
    let Some(note) = editor.path.borrow().clone() else {
        return;
    };
    let Some((start, end)) = editor.buffer.selection_bounds() else {
        return;
    };
    let text = editor.buffer.text(&start, &end, true).to_string();
    if note_text(&text).is_none() {
        return editor.toast("The selection has no text to move");
    }
    let Some(dir) = editor.extract_dir() else {
        return;
    };
    let selection = Selection {
        note,
        start: start.offset(),
        end: end.offset(),
        text,
    };
    let suggested = free_name(&suggested_name(&selection.text), |n| {
        naming::note_name(n).is_ok_and(|file| dir.join(file).exists())
    });
    let entry = adw::EntryRow::builder()
        .title("Name")
        .text(suggested.as_str())
        .activates_default(true)
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .build();
    list.add_css_class("boxed-list");
    list.append(&entry);
    let dialog = adw::AlertDialog::builder()
        .heading("New Note from Selection")
        .body("The selected text moves into the new note, and a link to it takes its place.")
        .extra_child(&list)
        .default_response("create")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("create", "Create")]);
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    let check = {
        let dialog = dialog.downgrade();
        move |entry: &adw::EntryRow| {
            let ok = naming::note_name(&entry.text()).is_ok();
            if let Some(dialog) = dialog.upgrade() {
                dialog.set_response_enabled("create", ok);
            }
            if ok {
                entry.remove_css_class("error");
            } else {
                entry.add_css_class("error");
            }
        }
    };
    check(&entry);
    entry.connect_changed(check);
    let weak = Rc::downgrade(editor);
    let selection = std::cell::RefCell::new(Some(selection));
    let name = entry.clone();
    dialog.connect_response(Some("create"), move |_, _| {
        let (Some(editor), Some(selection)) = (weak.upgrade(), selection.take()) else {
            return;
        };
        if let Ok(file) = naming::note_name(&name.text()) {
            editor.extract_selection(&dir, &file, selection);
        }
    });
    dialog.present(Some(&editor.window));
    // Select the name so typing replaces it.
    entry.grab_focus();
    entry.select_region(0, -1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::complete;
    use std::path::{Path, PathBuf};

    #[test]
    fn name_from_heading() {
        assert_eq!(suggested_name("## Meeting notes\n\nText"), "Meeting notes");
        assert_eq!(suggested_name("\n\n# Plan #\nbody"), "Plan");
        assert_eq!(suggested_name("# C#"), "C#");
        assert_eq!(suggested_name("#"), "Untitled");
    }

    #[test]
    fn name_from_plain_line() {
        assert_eq!(
            suggested_name("  \nFirst line here\nsecond"),
            "First line here"
        );
        assert_eq!(suggested_name("a  b\tc"), "a b c");
        assert_eq!(suggested_name(" \n\t\n"), "Untitled");
    }

    #[test]
    fn name_without_markers() {
        assert_eq!(suggested_name("- [ ] Buy **milk**"), "Buy milk");
        assert_eq!(suggested_name("* [x] Done"), "Done");
        assert_eq!(suggested_name("> > Quoted `code`"), "Quoted code");
        assert_eq!(suggested_name("1. First step"), "First step");
        assert_eq!(suggested_name("2) Second"), "Second");
        assert_eq!(suggested_name("+ See [[Other]]"), "See Other");
        assert_eq!(suggested_name("2026 plans"), "2026 plans");
        assert_eq!(suggested_name("-dash"), "-dash");
        // New File's rules: no `/`, no leading `.`.
        assert_eq!(suggested_name("Q1/Q2 review"), "Q1-Q2 review");
        assert_eq!(suggested_name(".hidden"), "hidden");
        assert!(naming::check(&suggested_name("../x")).is_ok());
    }

    #[test]
    fn name_trimmed_to_60_characters() {
        let long = "é".repeat(70);
        assert_eq!(suggested_name(&long).chars().count(), 60);
        let words = format!("{} tail", "a".repeat(59));
        assert_eq!(suggested_name(&words), "a".repeat(59));
    }

    #[test]
    fn taken_name_is_numbered() {
        let names = ["Plan.md", "Plan 2.md", "Idea.md"];
        let taken = |n: &str| names.contains(&format!("{n}.md").as_str());
        assert_eq!(free_name("Plan", taken), "Plan 3");
        assert_eq!(free_name("Idea", taken), "Idea 2");
        assert_eq!(free_name("New", taken), "New");
    }

    #[test]
    fn link_text() {
        let root = Path::new("/r");
        let notes: Vec<PathBuf> = ["/r/Plan.md", "/r/a/Idea.md", "/r/b/idea.md"]
            .iter()
            .map(PathBuf::from)
            .collect();
        assert_eq!(
            link(&complete::link_name(&notes[0], &notes, root)),
            "[[Plan]]"
        );
        assert_eq!(
            link(&complete::link_name(&notes[1], &notes, root)),
            "[[a/Idea]]"
        );
        // Beside a note opened without a folder: no other notes.
        assert_eq!(
            link(&complete::link_name(
                Path::new("/x/New.md"),
                &[],
                Path::new("/x")
            )),
            "[[New]]"
        );
    }

    #[test]
    fn note_text_drops_blank_lines() {
        assert_eq!(
            note_text("\n\n  \n# A\n\nb  \n\n \n").unwrap(),
            "# A\n\nb  \n"
        );
        assert_eq!(note_text("word").unwrap(), "word\n");
        assert_eq!(note_text("    code\n").unwrap(), "    code\n");
        assert_eq!(note_text("\n \t\n"), None);
        assert_eq!(note_text(""), None);
    }

    #[test]
    fn whole_lines_become_a_link_line() {
        // "Intro\n[# A\nbody\n]Next": the selected lines become the link's own line.
        assert_eq!(replacement("# A\nbody\n", "[[A]]"), "[[A]]\n");
        assert_eq!(replacement("# A\nbody", "[[A]]"), "[[A]]");
        // Blank lines at the edges stay, so the paragraphs around keep apart.
        assert_eq!(replacement("\n# A\nbody\n\n", "[[A]]"), "\n[[A]]\n\n");
        let source = "Intro\n\n# A\nbody\n\nNext\n";
        let selected = &source[6..17];
        let out = format!(
            "{}{}{}",
            &source[..6],
            replacement(selected, "[[A]]"),
            &source[17..]
        );
        assert_eq!(out, "Intro\n\n[[A]]\n\nNext\n");
    }

    #[test]
    fn link_start_skips_leading_blank_lines() {
        assert_eq!(link_start("# A\nbody\n"), 0);
        assert_eq!(link_start("\n \n# A\nbody"), 3);
        assert_eq!(link_start("\u{a0}\n\n# Ä"), 3);
        let selection = "\n\nbig idea\n";
        let out = replacement(selection, "[[X]]");
        let at = link_start(selection);
        assert_eq!(out.chars().skip(at).take(5).collect::<String>(), "[[X]]");
    }

    #[test]
    fn part_of_a_line_becomes_an_inline_link() {
        assert_eq!(replacement("big idea", "[[big idea]]"), "[[big idea]]");
        let source = "This is a big idea here.";
        let out = format!(
            "{}{}{}",
            &source[..10],
            replacement("big idea", "[[X]]"),
            &source[18..]
        );
        assert_eq!(out, "This is a [[X]] here.");
        // A space selected after the words stays in the line.
        assert_eq!(replacement("big idea ", "[[X]]"), "[[X]] ");
        assert_eq!(note_text("big idea ").unwrap(), "big idea\n");
    }
}
