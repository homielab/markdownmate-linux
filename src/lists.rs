//! Smart lists: what Enter / Tab / Shift+Tab do on a Markdown list line. Pure Rust (no GTK),
//! the editor key handler in `main.rs` applies the result to the buffer.

/// What Enter does on a list line.
#[derive(Debug, PartialEq, Eq)]
pub enum Enter {
    /// Insert a newline followed by this prefix (indent + next marker).
    Continue(String),
    /// The item is empty (only the marker): clear the line to leave the list.
    Exit,
}

/// What kind of line prefix a marker is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Quote,
    Bullet,
    Numbered,
    Task,
}

/// A list marker found at the start of a line.
struct Marker {
    kind: Kind,
    /// The marker the next line gets.
    next: String,
    /// Byte length of indent + marker (where the item text starts).
    len: usize,
}

fn parse(line: &str) -> Option<Marker> {
    let rest = line.trim_start_matches([' ', '\t']);
    let indent = &line[..line.len() - rest.len()];
    let (kind, marker_len, next) = if let Some(after) = rest.strip_prefix(['-', '*', '+']) {
        after.strip_prefix(' ')?;
        let bullet = &rest[..1];
        // To-dos always continue unchecked.
        if ["[ ] ", "[x] ", "[X] "]
            .iter()
            .any(|t| after[1..].starts_with(t))
        {
            (Kind::Task, 6, format!("{bullet} [ ] "))
        } else {
            (Kind::Bullet, 2, format!("{bullet} "))
        }
    } else if let Some(after) = rest.strip_prefix('>') {
        (
            Kind::Quote,
            1 + usize::from(after.starts_with(' ')),
            "> ".to_string(),
        )
    } else {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        // CommonMark caps ordered markers at 9 digits, which also keeps the parse in u64.
        if digits == 0 || digits > 9 {
            return None;
        }
        let delim = rest[digits..]
            .chars()
            .next()
            .filter(|c| *c == '.' || *c == ')')?;
        rest[digits + 1..].strip_prefix(' ')?;
        let n: u64 = rest[..digits].parse().ok()?;
        (Kind::Numbered, digits + 2, format!("{}{delim} ", n + 1))
    };
    Some(Marker {
        kind,
        next: format!("{indent}{next}"),
        len: indent.len() + marker_len,
    })
}

/// The to-do box on `line` (`- [ ] a`, `1. [x] a`, nested or in `>` quotes): char offset of
/// the char between the brackets and the char that toggles it (`' '` → `'x'`, `x`/`X` →
/// `' '`). None if the line is not a to-do item.
pub fn toggle_task(line: &str) -> Option<(usize, char)> {
    let mut rest = line.trim_start_matches([' ', '\t']);
    while let Some(r) = rest.strip_prefix('>') {
        rest = r.trim_start_matches([' ', '\t']);
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let rest = if digits > 0 {
        rest[digits..].strip_prefix(['.', ')'])?
    } else {
        rest.strip_prefix(['-', '*', '+'])?
    };
    let after = rest.trim_start_matches([' ', '\t']);
    let mut chars = after.chars();
    let new = match (
        after.len() < rest.len(),
        chars.next(),
        chars.next(),
        chars.next(),
    ) {
        (true, Some('['), Some(' '), Some(']')) => 'x',
        (true, Some('['), Some('x' | 'X'), Some(']')) => ' ',
        _ => return None,
    };
    if !matches!(chars.next(), None | Some(' ' | '\t')) {
        return None;
    }
    let at = line.len() - after.len() + 1;
    Some((line[..at].chars().count(), new))
}

/// Enter with the cursor after `before` and `after` the rest of the line. `None` = not a
/// list line (or the cursor is inside the marker): let the editor insert a plain newline.
pub fn on_enter(before: &str, after: &str) -> Option<Enter> {
    let m = parse(before)?;
    if before.len() < m.len {
        return None;
    }
    if before[m.len..].trim().is_empty() && after.trim().is_empty() {
        return Some(Enter::Exit);
    }
    Some(Enter::Continue(m.next))
}

/// The prefix `line` starts with (a `>` quote, or a bullet / numbered / to-do item, after
/// any indent) and the byte length of indent + marker; a to-do box at the end of the line
/// (`- [ ]`) counts too. None for any other line.
pub fn marker(line: &str) -> Option<(Kind, usize)> {
    let m = parse(line)?;
    let rest = line.get(m.len..)?;
    if m.kind == Kind::Bullet && ["[ ]", "[x]", "[X]"].contains(&rest) {
        return Some((Kind::Task, line.len()));
    }
    Some((m.kind, m.len))
}

/// Is `line` a list item (bullet, to-do, numbered, quote)?
pub fn is_item(line: &str) -> bool {
    parse(line).is_some_and(|m| line.len() >= m.len)
}

/// Bytes Shift+Tab removes from the start of a list line: one tab, or up to `width` spaces
/// (one indent level, the Preferences setting: 2, or 4 to nest under `10. `).
pub fn outdent_len(line: &str, width: usize) -> usize {
    if line.starts_with('\t') {
        return 1;
    }
    line.bytes().take(width).take_while(|b| *b == b' ').count()
}

/// Tab / Shift+Tab applies to the lines only if every non-blank one is a list item; otherwise
/// the editor keeps its default Tab behaviour.
pub fn all_items<'a>(lines: impl IntoIterator<Item = &'a str>) -> bool {
    let mut any = false;
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        if !is_item(line) {
            return false;
        }
        any = true;
    }
    any
}

/// Number of lines of the YAML front matter that opens `text` (`---` on line 1 up to the
/// next `---` line, both included), 0 if none — as the preview (comrak) reads it.
pub fn front_matter_lines(text: &str) -> usize {
    let mut lines = text.lines().map(str::trim_end);
    if lines.next() != Some("---") {
        return 0;
    }
    lines.position(|l| l == "---").map_or(0, |i| i + 2)
}

/// Is the line after `text_before` inside a fenced code block (``` or ~~~)?
pub fn in_fence(text_before: &str) -> bool {
    let mut open = None;
    for line in text_before.lines() {
        fence_step(&mut open, line);
    }
    open.is_some()
}

/// Feeds one line to the fence state `open` (the open fence's char); true if `line` is a
/// fence line (opening or closing, or a different fence inside a block).
pub fn fence_step(open: &mut Option<char>, line: &str) -> bool {
    let t = line.trim_start();
    let fence = if t.starts_with("```") {
        '`'
    } else if t.starts_with("~~~") {
        '~'
    } else {
        return false;
    };
    match *open {
        None => *open = Some(fence),
        Some(f) if f == fence => *open = None,
        Some(_) => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cont(before: &str, after: &str) -> Option<String> {
        match on_enter(before, after) {
            Some(Enter::Continue(p)) => Some(p),
            _ => None,
        }
    }

    #[test]
    fn bullets_continue_with_same_marker_and_indent() {
        assert_eq!(cont("- a", "").as_deref(), Some("- "));
        assert_eq!(cont("* a", "").as_deref(), Some("* "));
        assert_eq!(cont("+ a", "").as_deref(), Some("+ "));
        assert_eq!(cont("    - a", "").as_deref(), Some("    - "));
        assert_eq!(cont("\t* a", "").as_deref(), Some("\t* "));
    }

    #[test]
    fn todos_continue_unchecked() {
        assert_eq!(cont("- [ ] a", "").as_deref(), Some("- [ ] "));
        assert_eq!(cont("- [x] a", "").as_deref(), Some("- [ ] "));
        assert_eq!(cont("  * [X] a", "").as_deref(), Some("  * [ ] "));
    }

    #[test]
    fn numbers_increase() {
        assert_eq!(cont("1. a", "").as_deref(), Some("2. "));
        assert_eq!(cont("  9) a", "").as_deref(), Some("  10) "));
        assert_eq!(cont("999999999. a", "").as_deref(), Some("1000000000. "));
        assert_eq!(cont("1234567890. a", ""), None);
    }

    #[test]
    fn quotes_continue() {
        assert_eq!(cont("> a", "").as_deref(), Some("> "));
        assert_eq!(cont(">a", "").as_deref(), Some("> "));
    }

    #[test]
    fn enter_mid_line_continues() {
        assert_eq!(cont("- fo", "o").as_deref(), Some("- "));
    }

    #[test]
    fn empty_item_exits() {
        for line in ["- ", "  - ", "- [ ] ", "- [x] ", "3. ", ">", "> "] {
            assert_eq!(on_enter(line, ""), Some(Enter::Exit), "{line:?}");
        }
        assert_eq!(on_enter("- ", "  "), Some(Enter::Exit));
        // Text after the cursor: not empty, continue.
        assert_eq!(cont("- ", "a").as_deref(), Some("- "));
    }

    #[test]
    fn non_lists_are_left_alone() {
        for line in [
            "", "text", "-a", "*bold*", "---", "1.a", "a. b", "  ", "#1. x",
        ] {
            assert_eq!(on_enter(line, ""), None, "{line:?}");
        }
        // Cursor inside the marker.
        assert_eq!(on_enter("-", " a"), None);
        assert_eq!(on_enter("- [", " ] a"), Some(Enter::Continue("- ".into())));
    }

    #[test]
    fn tab_targets() {
        assert!(is_item("- a") && is_item("  1. a") && is_item("> a") && is_item("- [ ] a"));
        assert!(!is_item("a") && !is_item("-a"));
        assert!(all_items(["- a", "", "  - b"]));
        assert!(!all_items(["- a", "text"]));
        assert!(!all_items(["", " "]));
    }

    #[test]
    fn shift_tab_removes_one_level() {
        assert_eq!(outdent_len("    - a", 4), 4);
        assert_eq!(outdent_len("      - a", 4), 4);
        assert_eq!(outdent_len("  - a", 4), 2);
        assert_eq!(outdent_len("\t- a", 4), 1);
        assert_eq!(outdent_len("- a", 4), 0);
        assert_eq!(outdent_len("    - a", 2), 2);
        assert_eq!(outdent_len(" - a", 2), 1);
        assert_eq!(outdent_len("\t- a", 2), 1);
    }

    #[test]
    fn fences() {
        assert!(!in_fence("- a\n"));
        assert!(in_fence("x\n```rust\n"));
        assert!(!in_fence("```\ncode\n```\n"));
        assert!(in_fence("~~~\n```\n"));
        assert!(!in_fence("  ~~~\n```\n~~~\n"));
    }

    #[test]
    fn markers() {
        assert_eq!(marker("- a"), Some((Kind::Bullet, 2)));
        assert_eq!(marker("  * [x] a"), Some((Kind::Task, 8)));
        assert_eq!(marker("- [ ]"), Some((Kind::Task, 5)));
        assert_eq!(marker("10) a"), Some((Kind::Numbered, 4)));
        assert_eq!(marker(">a"), Some((Kind::Quote, 1)));
        assert_eq!(marker("> - a"), Some((Kind::Quote, 2)));
        assert_eq!(marker("-"), None);
        assert_eq!(marker("a"), None);
    }

    #[test]
    fn toggles_tasks() {
        assert_eq!(toggle_task("- [ ] a"), Some((3, 'x')));
        assert_eq!(toggle_task("- [x] a"), Some((3, ' ')));
        assert_eq!(toggle_task("  * [X]"), Some((5, ' ')));
        assert!(toggle_task("ô 1. [ ] a").is_none());
        assert_eq!(toggle_task("> > 12) [ ]\tb"), Some((9, 'x')));
        assert_eq!(toggle_task("\t+  [x] đã"), Some((5, ' ')));
        for line in [
            "- a", "-[ ] a", "- [ ]a", "- [y] a", "[ ] a", "# [ ] a", "", "- [ ", "1 [ ] a",
        ] {
            assert_eq!(toggle_task(line), None, "{line}");
        }
    }
}
