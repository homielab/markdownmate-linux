//! Line commands of the editor: Toggle Task, Footnote, Move Line Up / Down, Duplicate Line,
//! and the format bar's Heading, Quote, lists and Code Block toggles. Pure logic (no GTK): text + selection → the edit to make. Every position is a char
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

/// A line rewritten by a format toggle: its new text, and the chars of prefix it lost
/// (`old`, from the line start) and has now (`new`), to keep the cursor on the same text.
struct Rewrite {
    text: String,
    old: usize,
    new: usize,
}

impl Rewrite {
    fn keep(line: &str) -> Rewrite {
        Rewrite {
            text: line.into(),
            old: 0,
            new: 0,
        }
    }
}

/// Rewrites the lines `first..=last` with `f` (line number, its text). Without a selection
/// the cursor stays on its text (or after the new prefix when it was inside the old one);
/// with a selection, every rewritten line ends up selected.
fn rewrite(
    chars: &[char],
    lines: &[Line],
    (first, last): (usize, usize),
    sel: (usize, usize),
    mut f: impl FnMut(usize, &str) -> Rewrite,
) -> Replace {
    let (s, e) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let start = lines[first].start;
    let mut text = String::new();
    let mut cursor = None;
    for (n, l) in lines.iter().enumerate().take(last + 1).skip(first) {
        let r = f(n, &slice(chars, l.start, l.body));
        if s == e && cursor.is_none() && s <= l.body {
            let c = s.saturating_sub(l.start);
            let col = if c >= r.old {
                c - r.old + r.new
            } else {
                c.min(r.new)
            };
            cursor = Some(start + text.chars().count() + col);
        }
        text += &r.text;
        if n < last {
            text += &slice(chars, l.body, l.end);
        }
    }
    let len = text.chars().count();
    Replace {
        start,
        end: lines[last].body,
        text,
        select: cursor.map_or((start, start + len), |c| (c, c)),
    }
}

/// The lines a format toggle changes: the non-blank ones outside code blocks among those the
/// selection touches. None when there are none, unless the cursor is on one blank line
/// outside code (`Some(empty)`: the toggle starts a new line of that kind there).
fn targets(chars: &[char], lines: &[Line], (first, last): (usize, usize)) -> Option<Vec<usize>> {
    let code = code_lines(chars, lines);
    let content: Vec<usize> = (first..=last)
        .filter(|&n| {
            !code[n]
                && !slice(chars, lines[n].start, lines[n].body)
                    .trim()
                    .is_empty()
        })
        .collect();
    (!content.is_empty() || (first == last && !code[first])).then_some(content)
}

/// Bulleted / Numbered / Task List and Quote: adds the prefix to every non-blank line the
/// selection touches (numbers counting up from 1), or removes it if all of them have it
/// already. List kinds replace each other (bullets become numbers); a quote goes around a
/// list. On a blank line, starts one. Code blocks are left alone; None when only code (or
/// only blank lines) is selected.
pub fn toggle_prefix(text: &str, sel: (usize, usize), kind: lists::Kind) -> Option<Replace> {
    let chars: Vec<char> = text.chars().collect();
    let lines = split(&chars);
    let blk = block(&lines, sel);
    let content = targets(&chars, &lines, blk)?;
    let quote = kind == lists::Kind::Quote;
    let applied = !content.is_empty()
        && content.iter().all(|&n| {
            let l = lines[n];
            lists::marker(&slice(&chars, l.start, l.body)).map(|m| m.0) == Some(kind)
        });
    let mut number = 0;
    Some(rewrite(&chars, &lines, blk, sel, |n, line| {
        if !content.is_empty() && !content.contains(&n) {
            return Rewrite::keep(line);
        }
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        // Quotes and lists come off independently of each other.
        let old = match lists::marker(line) {
            Some((k, len)) if (k == lists::Kind::Quote) == quote => len,
            _ => indent,
        };
        let body = &line[old..];
        let marker = if applied {
            String::new()
        } else {
            number += 1;
            match kind {
                lists::Kind::Quote => "> ".into(),
                lists::Kind::Bullet => "- ".into(),
                lists::Kind::Numbered => format!("{number}. "),
                lists::Kind::Task => "- [ ] ".into(),
            }
        };
        Rewrite {
            text: format!("{}{marker}{body}", &line[..indent]),
            old: line[..old].chars().count(),
            new: line[..indent].chars().count() + marker.len(),
        }
    }))
}

/// The level of an ATX heading line (`#` to `######`, then a space or the line end).
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    ((1..=6).contains(&hashes) && matches!(line.as_bytes().get(hashes), None | Some(b' ')))
        .then_some(hashes)
}

/// Heading: cycles the non-blank lines the selection touches through `#` … `######` and back
/// to plain text, one level further than the first of them. On a blank line, starts a
/// `# ` heading. Code blocks are left alone; None when only code is selected.
pub fn cycle_heading(text: &str, sel: (usize, usize)) -> Option<Replace> {
    let chars: Vec<char> = text.chars().collect();
    let lines = split(&chars);
    let blk = block(&lines, sel);
    let content = targets(&chars, &lines, blk)?;
    let level = content.first().and_then(|&n| {
        let l = lines[n];
        heading_level(&slice(&chars, l.start, l.body))
    });
    let next = match level {
        None => 1,
        Some(6) => 0,
        Some(l) => l + 1,
    };
    let marker = if next == 0 {
        String::new()
    } else {
        format!("{} ", "#".repeat(next))
    };
    Some(rewrite(&chars, &lines, blk, sel, |n, line| {
        if !content.is_empty() && !content.contains(&n) {
            return Rewrite::keep(line);
        }
        let old = heading_level(line).map_or(0, |l| (l + 1).min(line.len()));
        Rewrite {
            text: format!("{marker}{}", &line[old..]),
            old,
            new: marker.len(),
        }
    }))
}

/// Code Block: fences the lines the selection touches with ```` ``` ```` lines, or, when they
/// are inside one fenced block (its fences included), removes that block's fences. The
/// selection stays on the same text. None when the selection is partly in a code block.
pub fn toggle_code_block(text: &str, sel: (usize, usize)) -> Option<Replace> {
    let chars: Vec<char> = text.chars().collect();
    let lines = split(&chars);
    let (first, last) = block(&lines, sel);
    let (s, e) = (sel.0.min(sel.1), sel.0.max(sel.1));
    // Every fenced block: its opening line and its closing one (None: open to the end).
    let mut blocks: Vec<(usize, Option<usize>)> = vec![];
    let mut open = None;
    for (n, l) in lines.iter().enumerate() {
        let was_open = open.is_some();
        if lists::fence_step(&mut open, &slice(&chars, l.start, l.body)) {
            match (was_open, open.is_some()) {
                (false, true) => blocks.push((n, None)),
                (true, false) => {
                    if let Some(b) = blocks.last_mut() {
                        b.1 = Some(n);
                    }
                }
                _ => {}
            }
        }
    }
    let end_of = |b: &(usize, Option<usize>)| b.1.unwrap_or(lines.len() - 1);
    if let Some(&(o, c)) = blocks.iter().find(|b| b.0 <= first && last <= end_of(b)) {
        let start = lines[o].start;
        let (end, inner) = match c {
            Some(c) if c > o + 1 => (
                lines[c].body,
                slice(&chars, lines[o + 1].start, lines[c - 1].body),
            ),
            Some(c) => (lines[c].body, String::new()),
            None => (lines[o].end, String::new()),
        };
        // Positions after the opening fence move up by its line; none goes past the text
        // left between the fences.
        let max = c.map_or(usize::MAX, |_| start + inner.chars().count());
        let shift = lines[o].end - lines[o].start;
        let at = |p: usize| p.saturating_sub(shift).max(start).min(max);
        return Some(Replace {
            start,
            end,
            text: inner,
            select: (at(s), at(e)),
        });
    }
    if blocks.iter().any(|b| b.0 <= last && first <= end_of(b)) {
        return None;
    }
    let ending = match slice(&chars, lines[0].body, lines[0].end) {
        e if e.is_empty() => "\n".to_string(),
        e => e,
    };
    let (start, end) = (lines[first].start, lines[last].body);
    let shift = 3 + ending.chars().count();
    Some(Replace {
        start,
        end,
        text: format!("```{ending}{}{ending}```", slice(&chars, start, end)),
        select: (s + shift, e.min(end) + shift),
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

    /// The new text and selection.
    type Formatted = Option<(String, (usize, usize))>;

    fn format_line(
        text: &str,
        sel: (usize, usize),
        f: fn(&str, (usize, usize)) -> Option<Replace>,
    ) -> Formatted {
        let r = f(text, sel)?;
        Some((apply(text, &r), r.select))
    }

    fn prefix(kind: lists::Kind) -> impl Fn(&str, (usize, usize)) -> Formatted {
        move |text, sel| {
            let r = toggle_prefix(text, sel, kind)?;
            Some((apply(text, &r), r.select))
        }
    }

    #[test]
    fn prefix_toggles_on_cursor_line() {
        let bullet = prefix(lists::Kind::Bullet);
        // The cursor stays on its text.
        assert_eq!(bullet("ab\ncd", (4, 4)), Some(("ab\n- cd".into(), (6, 6))));
        assert_eq!(bullet("ab\n- cd", (6, 6)), Some(("ab\ncd".into(), (4, 4))));
        // Cursor inside the removed marker: at the line start.
        assert_eq!(bullet("- cd", (1, 1)), Some(("cd".into(), (0, 0))));
        // Indent kept, the marker goes after it.
        assert_eq!(bullet("  ý", (3, 3)), Some(("  - ý".into(), (5, 5))));
        // A blank line starts an item, the cursor after the marker.
        assert_eq!(bullet("", (0, 0)), Some(("- ".into(), (2, 2))));
        let task = prefix(lists::Kind::Task);
        assert_eq!(task("a", (0, 0)), Some(("- [ ] a".into(), (6, 6))));
        assert_eq!(task("- [x] a", (7, 7)).unwrap().0, "a");
        assert_eq!(task("- [ ]", (5, 5)).unwrap().0, "");
    }

    #[test]
    fn prefix_toggles_on_selected_lines() {
        let numbered = prefix(lists::Kind::Numbered);
        let text = "a\n\nb\nc\n";
        // Blank lines skipped (not numbered); the rewritten lines are selected.
        assert_eq!(
            numbered(text, (0, 6)),
            Some(("1. a\n\n2. b\n3. c\n".into(), (0, 15)))
        );
        assert_eq!(numbered("1. a\n2. b", (0, 9)).unwrap().0, "a\nb");
        // Some lines without it: all get it; list kinds replace each other.
        assert_eq!(
            numbered("- a\nb\n* [ ] c", (0, 13)).unwrap().0,
            "1. a\n2. b\n3. c"
        );
        let quote = prefix(lists::Kind::Quote);
        assert_eq!(quote("- a\nb", (0, 5)).unwrap().0, "> - a\n> b");
        assert_eq!(quote("> - a\n>b", (0, 8)).unwrap().0, "- a\nb");
        assert_eq!(quote("> a\nb", (0, 5)).unwrap().0, "> a\n> b");
        assert_eq!(
            prefix(lists::Kind::Bullet)("> a", (0, 0)).unwrap().0,
            "- > a"
        );
        assert_eq!(quote("a\r\nb", (0, 4)).unwrap().0, "> a\r\n> b");
    }

    #[test]
    fn prefix_leaves_code_alone() {
        let bullet = prefix(lists::Kind::Bullet);
        let text = "a\n```\nx\n```\nb";
        assert_eq!(bullet(text, (0, 13)).unwrap().0, "- a\n```\nx\n```\n- b");
        assert_eq!(bullet(text, (6, 6)), None);
        // Several blank lines: nothing to do.
        assert_eq!(bullet("\n\n", (0, 2)), None);
    }

    #[test]
    fn heading_cycles_levels() {
        let h = |t: &str, s| format_line(t, s, cycle_heading);
        assert_eq!(h("Title", (2, 2)), Some(("# Title".into(), (4, 4))));
        assert_eq!(h("# Title", (4, 4)), Some(("## Title".into(), (5, 5))));
        assert_eq!(h("###### Title", (0, 12)), Some(("Title".into(), (0, 5))));
        assert_eq!(h("#", (1, 1)), Some(("## ".into(), (3, 3))));
        assert_eq!(h("", (0, 0)), Some(("# ".into(), (2, 2))));
        // `#tag` is not a heading; levels follow the first line.
        assert_eq!(h("#tag", (0, 0)).unwrap().0, "# #tag");
        assert_eq!(
            h("## a\nb\n\n# c", (0, 11)).unwrap().0,
            "### a\n### b\n\n### c"
        );
        assert_eq!(h("```\n# x\n```", (5, 5)), None);
    }

    #[test]
    fn code_block_wraps_and_unwraps() {
        let c = |t: &str, s| format_line(t, s, toggle_code_block);
        assert_eq!(
            c("a\nb\nc", (2, 3)),
            Some(("a\n```\nb\n```\nc".into(), (6, 7)))
        );
        assert_eq!(c("a\nb", (0, 3)), Some(("```\na\nb\n```".into(), (4, 7))));
        // A blank line: an empty block with the cursor inside.
        assert_eq!(c("", (0, 0)), Some(("```\n\n```".into(), (4, 4))));
        assert_eq!(c("a\r\nb", (0, 0)).unwrap().0, "```\r\na\r\n```\r\nb");
        // Inside a block, or on its fences: the fences go.
        let text = "a\n```rust\nx\ny\n```\nb";
        assert_eq!(c(text, (11, 11)), Some(("a\nx\ny\nb".into(), (3, 3))));
        assert_eq!(c(text, (2, 16)), Some(("a\nx\ny\nb".into(), (2, 5))));
        assert_eq!(c("```\n```", (0, 0)), Some((String::new(), (0, 0))));
        assert_eq!(c("```\nopen", (5, 5)), Some(("open".into(), (1, 1))));
        // Partly in a block: nothing to do.
        assert_eq!(c(text, (0, 11)), None);
    }
}
