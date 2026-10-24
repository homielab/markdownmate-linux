//! Line commands of the editor: Toggle Task, Footnote, Move Line Up / Down, Duplicate Line.
//! Pure logic (no GTK): text + selection → the edit to make. Every position is a char
//! offset, as in a GtkTextBuffer. Lines end with `\n` (or `\r\n`, kept with its line).

use crate::lists;

/// Replace `start..end` of the old text with `text`; `select` is the selection afterwards
/// (in the new text).
#[derive(Debug, PartialEq, Eq)]
pub struct Replace {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub select: (usize, usize),
}

/// A line: `start..body` is its text, `body..end` its ending (empty on the last line).
#[derive(Clone, Copy)]
struct Line {
    start: usize,
    body: usize,
    end: usize,
}

/// Every line of `chars`; a text ending with a newline has an empty last line after it.
fn split(chars: &[char]) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            let body = if i > start && chars[i - 1] == '\r' {
                i - 1
            } else {
                i
            };
            lines.push(Line {
                start,
                body,
                end: i + 1,
            });
            start = i + 1;
        }
    }
    lines.push(Line {
        start,
        body: chars.len(),
        end: chars.len(),
    });
    lines
}

/// The first and last line the selection touches. A selection ending at a line's start
/// doesn't take that line.
fn block(lines: &[Line], sel: (usize, usize)) -> (usize, usize) {
    let (s, e) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let at = |p: usize| lines.iter().rposition(|l| l.start <= p).unwrap_or(0);
    let first = at(s);
    let mut last = at(e);
    if e > s && last > first && lines[last].start == e {
        last -= 1;
    }
    (first, last)
}

fn slice(chars: &[char], from: usize, to: usize) -> String {
    chars[from..to].iter().collect()
}

/// Whether each line is in a fenced code block (fence lines included).
fn code_lines(chars: &[char], lines: &[Line]) -> Vec<bool> {
    let mut open = None;
    lines
        .iter()
        .map(|l| {
            let line = slice(chars, l.start, l.body);
            lists::fence_step(&mut open, &line) || open.is_some()
        })
        .collect()
}

/// Offset (in chars, within `line`) of the mark in a task item's `[ ]` / `[x]`: a list
/// item, also inside `>` quotes, starting with a task marker followed by a space or the end.
fn task_mark(line: &[char]) -> Option<usize> {
    let blank = |c: char| c == ' ' || c == '\t';
    let skip = |mut i: usize| {
        while i < line.len() && blank(line[i]) {
            i += 1;
        }
        i
    };
    let mut i = skip(0);
    while i < line.len() && line[i] == '>' {
        i = skip(i + 1);
    }
    match line.get(i) {
        Some('-' | '*' | '+') => i += 1,
        Some(c) if c.is_ascii_digit() => {
            let digits = line[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            if digits > 9 || !matches!(line.get(i + digits), Some('.' | ')')) {
                return None;
            }
            i += digits + 1;
        }
        _ => return None,
    }
    if !line.get(i).is_some_and(|c| blank(*c)) {
        return None;
    }
    i = skip(i);
    let ok = line.get(i) == Some(&'[')
        && matches!(line.get(i + 1), Some(' ' | 'x' | 'X'))
        && line.get(i + 2) == Some(&']')
        && line.get(i + 3).is_none_or(|c| blank(*c) || *c == '\r');
    ok.then_some(i + 1)
}

/// Toggle Task: the task items (outside code blocks) on the lines the selection touches all
/// become `[x]` if any is open, else all `[ ]`. The changed marks (offsets) and the new mark;
/// the text keeps its length, so the selection stays. None when no line is a task.
pub fn toggle_tasks(text: &str, sel: (usize, usize)) -> Option<(Vec<usize>, char)> {
    let chars: Vec<char> = text.chars().collect();
    let lines = split(&chars);
    let code = code_lines(&chars, &lines);
    let (first, last) = block(&lines, sel);
    let marks: Vec<usize> = (first..=last)
        .filter(|&n| !code[n])
        .filter_map(|n| {
            let l = lines[n];
            task_mark(&chars[l.start..l.body]).map(|i| l.start + i)
        })
        .collect();
    if marks.is_empty() {
        return None;
    }
    let mark = if marks.iter().any(|&m| chars[m] == ' ') {
        'x'
    } else {
        ' '
    };
    let changed = marks
        .into_iter()
        .filter(|&m| (chars[m] == ' ') != (mark == ' '))
        .collect();
    Some((changed, mark))
}

/// Footnote: insert `reference` at `at` and `definition` at the end of the text (in that
/// order of the text; insert the definition first). `cursor` is the new text's end.
#[derive(Debug, PartialEq, Eq)]
pub struct Footnote {
    pub at: usize,
    pub reference: String,
    pub definition: String,
    pub cursor: usize,
}

/// `[^n]` after the selection and `[^n]: ` on a line at the end of the note (under a blank
/// line unless the last line already is a definition or blank), `n` the smallest positive
/// number not yet used outside code. None when the selection ends inside a code block.
pub fn footnote(text: &str, sel: (usize, usize)) -> Option<Footnote> {
    let chars: Vec<char> = text.chars().collect();
    let lines = split(&chars);
    let at = sel.0.max(sel.1).min(chars.len());
    let (_, line) = block(&lines, (at, at));
    if code_lines(&chars, &lines)[line] {
        return None;
    }
    let label = format!("[^{}]", first_free_number(text));
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut with_reference: String = chars[..at].iter().collect();
    with_reference += &label;
    with_reference.extend(&chars[at..]);
    let ends_with_newline = with_reference.ends_with('\n');
    let last_line = with_reference.trim_end_matches(['\n', '\r']);
    let last_line = &last_line[last_line.rfind('\n').map_or(0, |i| i + 1)..];
    let continues = last_line.trim().is_empty() || is_definition(last_line);
    let mut definition = String::new();
    if !ends_with_newline {
        definition += newline;
    }
    if !continues {
        definition += newline;
    }
    definition += &label;
    definition += ": ";
    let cursor = chars.len() + label.chars().count() + definition.chars().count();
    Some(Footnote {
        at,
        reference: label,
        definition,
        cursor,
    })
}

/// A footnote definition line: `[^label]:` indented at most 3 spaces.
fn is_definition(line: &str) -> bool {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return false;
    }
    let Some(rest) = rest.strip_prefix("[^") else {
        return false;
    };
    rest.find(']').is_some_and(|close| {
        close > 0
            && !rest[..close].contains(char::is_whitespace)
            && rest[close + 1..].starts_with(':')
    })
}

/// The smallest positive number not used as `[^n]` outside fenced code and code spans.
fn first_free_number(text: &str) -> usize {
    let mut used = std::collections::HashSet::new();
    let mut open = None;
    for line in text.lines() {
        if lists::fence_step(&mut open, line) || open.is_some() || !line.contains("[^") {
            continue;
        }
        let line = strip_code_spans(line);
        for part in line.split("[^").skip(1) {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            if part[digits.len()..].starts_with(']')
                && let Ok(n) = digits.parse::<usize>()
            {
                used.insert(n);
            }
        }
    }
    (1..).find(|n| !used.contains(n)).unwrap_or(1)
}

/// `line` without its code spans (a run of backticks up to the next run of the same length).
fn strip_code_spans(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some(i) = rest.find('`') {
        out += &rest[..i];
        let run = rest[i..].chars().take_while(|c| *c == '`').count();
        let after = &rest[i + run..];
        let fence = "`".repeat(run);
        match after.find(&fence) {
            Some(j) => rest = &after[j + run..],
            None => {
                out += &rest[i..i + run];
                rest = after;
            }
        }
    }
    out + rest
}

/// Move Line Up / Down: the lines the selection touches swap with the line above / below,
/// the selection moving with them. Line endings stay with their text; the last line, without
/// one, borrows its neighbour's. None at the first / last line.
pub fn move_lines(text: &str, sel: (usize, usize), up: bool) -> Option<Replace> {
    let chars: Vec<char> = text.chars().collect();
    let lines = split(&chars);
    let (first, last) = block(&lines, sel);
    let blk = Line {
        start: lines[first].start,
        body: lines[last].body,
        end: lines[last].end,
    };
    if blk.start == blk.end || (up && first == 0) || (!up && blk.end == chars.len()) {
        return None;
    }
    let (a, b) = if up {
        (lines[first - 1], blk)
    } else {
        (blk, lines[last + 1])
    };
    let (a_body, a_end) = (slice(&chars, a.start, a.body), slice(&chars, a.body, a.end));
    let (b_body, b_end) = (slice(&chars, b.start, b.body), slice(&chars, b.body, b.end));
    let moved = if b_end.is_empty() {
        format!("{b_body}{a_end}{a_body}")
    } else {
        format!("{b_body}{b_end}{a_body}{a_end}")
    };
    let block_start = if up {
        a.start
    } else {
        let ending = if b_end.is_empty() { &a_end } else { &b_end };
        a.start + b_body.chars().count() + ending.chars().count()
    };
    let (s, e) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let start = block_start + s - blk.start;
    let end = (start + e - s).min(b.end);
    Some(Replace {
        start: a.start,
        end: b.end,
        text: moved,
        select: (start, end),
    })
}

/// Duplicate Line: a copy of the lines the selection touches, inserted below them, with the
/// selection moved onto the copy. The last line, without an ending, gets the note's (its
/// first line's, else `\n`). None on the empty line after a final newline.
pub fn duplicate_lines(text: &str, sel: (usize, usize)) -> Option<Replace> {
    let chars: Vec<char> = text.chars().collect();
    let lines = split(&chars);
    let (first, last) = block(&lines, sel);
    let (start, end) = (lines[first].start, lines[last].end);
    if start == end {
        return None;
    }
    let block = slice(&chars, start, end);
    let copy = if lines[last].body == end {
        let ending = slice(&chars, lines[0].body, lines[0].end);
        format!("{}{block}", if ending.is_empty() { "\n" } else { &ending })
    } else {
        block
    };
    let shift = copy.chars().count();
    let (s, e) = (sel.0.min(sel.1), sel.0.max(sel.1));
    Some(Replace {
        start: end,
        end,
        text: copy,
        select: (s + shift, e + shift),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, r: &Replace) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out: String = chars[..r.start].iter().collect();
        out += &r.text;
        out.extend(&chars[r.end..]);
        out
    }

    fn toggled(text: &str, sel: (usize, usize)) -> Option<String> {
        let (marks, mark) = toggle_tasks(text, sel)?;
        let mut chars: Vec<char> = text.chars().collect();
        for m in marks {
            chars[m] = mark;
        }
        Some(chars.into_iter().collect())
    }

    #[test]
    fn toggle_one_task() {
        assert_eq!(
            toggled("- [ ] a\n- [ ] b", (2, 2)).unwrap(),
            "- [x] a\n- [ ] b"
        );
        assert_eq!(
            toggled("- [ ] a\n- [X] b", (9, 9)).unwrap(),
            "- [ ] a\n- [ ] b"
        );
        assert_eq!(
            toggled("> 1. [ ] quoted", (0, 0)).unwrap(),
            "> 1. [x] quoted"
        );
        assert_eq!(toggled("  * [x]", (0, 0)).unwrap(), "  * [ ]");
        assert_eq!(toggled("- [ ] a\r\n", (0, 0)).unwrap(), "- [x] a\r\n");
    }

    #[test]
    fn toggle_selection_any_open_ticks_all() {
        let text = "- [x] a\ntext\n- [ ] b\n- [x] c\n- [ ] d";
        // Ends at the start of the last line: that line is not taken.
        let sel = (0, text.rfind("- [ ] d").unwrap());
        assert_eq!(
            toggled(text, sel).unwrap(),
            "- [x] a\ntext\n- [x] b\n- [x] c\n- [ ] d"
        );
        let all = "- [x] a\n- [x] b";
        assert_eq!(toggled(all, (3, 12)).unwrap(), "- [ ] a\n- [ ] b");
    }

    #[test]
    fn toggle_skips_non_tasks_and_code() {
        assert_eq!(toggled("text\n- item\n-[ ] no\n- [ ]x", (0, 25)), None);
        let text = "```\n- [ ] in code\n```\n- [ ] out";
        assert_eq!(
            toggled(text, (0, text.len())).unwrap(),
            "```\n- [ ] in code\n```\n- [x] out"
        );
        assert_eq!(toggled(text, (6, 6)), None);
    }

    #[test]
    fn footnote_numbers_and_definition() {
        let f = footnote("Text here", (4, 4)).unwrap();
        assert_eq!(
            f,
            Footnote {
                at: 4,
                reference: "[^1]".into(),
                definition: "\n\n[^1]: ".into(),
                cursor: 21
            }
        );
        // Used numbers (code spans and code blocks don't count); after the selection.
        let text = "a[^1] b[^3] `[^2]`\n```\n[^4]\n```\n\n[^1]: one\n[^3]: three\n";
        let f = footnote(text, (0, 1)).unwrap();
        assert_eq!(
            (f.at, f.reference.as_str(), f.definition.as_str()),
            (1, "[^2]", "[^2]: ")
        );
        // Cursor at the very end: the reference, then the definition under a blank line.
        let f = footnote("end", (3, 3)).unwrap();
        assert_eq!(f.definition, "\n\n[^1]: ");
        assert_eq!(f.cursor, "end[^1]\n\n[^1]: ".chars().count());
        assert_eq!(footnote("é\r\n", (1, 1)).unwrap().definition, "\r\n[^1]: ");
        assert_eq!(footnote("", (0, 0)).unwrap().definition, "\n\n[^1]: ");
    }

    #[test]
    fn footnote_not_in_code_block() {
        assert_eq!(footnote("```\ncode\n```", (5, 5)), None);
        assert_eq!(footnote("```\ncode\n```", (0, 0)), None);
        assert!(footnote("```\ncode\n```\nafter", (13, 13)).is_some());
    }

    #[test]
    fn move_line_up_and_down() {
        let text = "one\ntwo\nthree";
        let r = move_lines(text, (5, 5), true).unwrap();
        assert_eq!(
            (apply(text, &r), r.select),
            ("two\none\nthree".into(), (1, 1))
        );
        let r = move_lines(text, (5, 5), false).unwrap();
        // The last line has no ending: it borrows the moved line's.
        assert_eq!(
            (apply(text, &r), r.select),
            ("one\nthree\ntwo".into(), (11, 11))
        );
        let r = move_lines(text, (9, 13), true).unwrap();
        assert_eq!(
            (apply(text, &r), r.select),
            ("one\nthree\ntwo".into(), (5, 9))
        );
    }

    #[test]
    fn move_selection_spanning_lines() {
        let text = "a\nb\nc\nd\n";
        // "b" and "c" (the selection ends at the start of "d": "d" stays).
        let r = move_lines(text, (2, 6), false).unwrap();
        assert_eq!((apply(text, &r), r.select), ("a\nd\nb\nc\n".into(), (4, 8)));
        let r = move_lines(text, (3, 5), true).unwrap();
        assert_eq!((apply(text, &r), r.select), ("b\nc\na\nd\n".into(), (1, 3)));
    }

    #[test]
    fn move_stops_at_first_and_last_line() {
        assert_eq!(move_lines("a\nb", (0, 0), true), None);
        assert_eq!(move_lines("a\nb", (3, 3), false), None);
        assert_eq!(move_lines("a\nb\n", (2, 2), false), None);
        assert_eq!(move_lines("a\nb\n", (4, 4), true), None);
        let r = move_lines("a\r\nb\r\n", (3, 3), true).unwrap();
        assert_eq!(apply("a\r\nb\r\n", &r), "b\r\na\r\n");
    }

    #[test]
    fn move_inside_code_block() {
        let text = "```\nx\ny\n```";
        let r = move_lines(text, (4, 4), false).unwrap();
        assert_eq!(apply(text, &r), "```\ny\nx\n```");
    }

    #[test]
    fn duplicate() {
        let text = "one\ntwo\n";
        let r = duplicate_lines(text, (1, 2)).unwrap();
        assert_eq!(
            (apply(text, &r), r.select),
            ("one\none\ntwo\n".into(), (5, 6))
        );
        // Two lines (the selection ends at the start of the third).
        let text = "a\nb\nc";
        let r = duplicate_lines(text, (0, 4)).unwrap();
        assert_eq!(
            (apply(text, &r), r.select),
            ("a\nb\na\nb\nc".into(), (4, 8))
        );
        // The last line, without an ending.
        let r = duplicate_lines(text, (5, 5)).unwrap();
        assert_eq!((apply(text, &r), r.select), ("a\nb\nc\nc".into(), (7, 7)));
        assert_eq!(apply("x", &duplicate_lines("x", (0, 0)).unwrap()), "x\nx");
        assert_eq!(
            apply("a\r\nb", &duplicate_lines("a\r\nb", (3, 3)).unwrap()),
            "a\r\nb\r\nb"
        );
        assert_eq!(duplicate_lines("a\n", (2, 2)), None);
        // Code blocks are lines like any other.
        let text = "```\nx\n```";
        assert_eq!(
            apply(text, &duplicate_lines(text, (4, 4)).unwrap()),
            "```\nx\nx\n```"
        );
    }
}
