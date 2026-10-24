//! GFM table editing: Tab / Shift+Tab move between cells, Enter adds a row (or leaves the
//! table on an empty last row), and each of them realigns the whole table. Pure Rust; the
//! editor key handler in `input.rs` applies the result to the buffer.
//!
//! A table may sit in a `>` quote or a list item: every row then starts with the same
//! prefix (spaces / tabs / `>`), which is kept as is. Rows with differing prefixes are not
//! one table.

/// The key pressed in a table cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    BackTab,
    Enter,
}

/// Replace `lines[first..]` with `text`, then select `select` (char offsets into `text`).
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub first: usize,
    pub text: String,
    pub select: (usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Align {
    None,
    Left,
    Center,
    Right,
}

/// The quote / indent prefix of a row: its leading spaces, tabs and `>`.
fn prefix(line: &str) -> &str {
    let body = line.trim_start_matches([' ', '\t', '>']);
    &line[..line.len() - body.len()]
}

/// Cells of a table row: split on unescaped `|` (GFM wants `\|` even inside code spans),
/// without the optional outer pipes, trimmed.
fn cells(line: &str) -> Vec<String> {
    let s = line.trim();
    let s = s.strip_prefix('|').unwrap_or(s);
    let s = match s.strip_suffix('|') {
        Some(t) if !t.ends_with('\\') => t,
        _ => s,
    };
    let mut out = vec![];
    let mut cur = String::new();
    let mut esc = false;
    for c in s.chars() {
        if c == '|' && !esc {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
        esc = c == '\\' && !esc;
    }
    out.push(cur.trim().to_string());
    out
}

/// The alignments of a delimiter row (`| --- | :-: |`), None if `line` is not one.
fn delimiter(line: &str) -> Option<Vec<Align>> {
    cells(line)
        .iter()
        .map(|c| {
            let dashes = c.trim_start_matches(':').trim_end_matches(':');
            if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
                return None;
            }
            Some(match (c.starts_with(':'), c.ends_with(':')) {
                (false, false) => Align::None,
                (true, false) => Align::Left,
                (true, true) => Align::Center,
                (false, true) => Align::Right,
            })
        })
        .collect()
}

/// Columns `s` takes in a monospace font: combining marks (decomposed Vietnamese) and
/// zero-width joiners / variation selectors 0, CJK and emoji 2.
/// shortcut: hand-picked wide ranges, not the full East Asian Width table; upgrade with the
/// unicode-width crate if alignment is off for other scripts.
fn width(s: &str) -> usize {
    s.chars()
        .map(|c| match c {
            _ if crate::stats::is_combining(c) => 0,
            '\u{200B}'..='\u{200D}' | '\u{FE00}'..='\u{FE0F}' => 0,
            '\u{1100}'..='\u{115F}'
            | '\u{2E80}'..='\u{A4CF}'
            | '\u{AC00}'..='\u{D7A3}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FE30}'..='\u{FE4F}'
            | '\u{FF00}'..='\u{FF60}'
            | '\u{FFE0}'..='\u{FFE6}'
            | '\u{1F300}'..='\u{1F64F}'
            | '\u{1F900}'..='\u{1F9FF}'
            | '\u{20000}'..='\u{3FFFD}' => 2,
            _ => 1,
        })
        .sum()
}

/// `key` with the cursor at char `col` of `lines[row]`. `lines` are the consecutive lines
/// containing `|` around the cursor; the table starts at the first header + delimiter pair.
/// None = not in a table (or Enter on the header): the editor keeps its usual behaviour.
pub fn edit(lines: &[&str], row: usize, col: usize, key: Key) -> Option<Edit> {
    let header = |i: usize| {
        let pre = prefix(lines[i]);
        prefix(lines[i + 1]) == pre
            && delimiter(&lines[i + 1][pre.len()..])
                .is_some_and(|d| d.len() == cells(&lines[i][pre.len()..]).len())
    };
    let first = (0..lines.len().saturating_sub(1)).find(|&i| header(i))?;
    let pre = prefix(lines[first]);
    if row < first || lines[first..].iter().any(|l| prefix(l) != pre) {
        return None;
    }
    let body: Vec<&str> = lines[first..].iter().map(|l| &l[pre.len()..]).collect();
    let line = body.get(row - first)?;
    let aligns = delimiter(body[1])?;
    let mut rows: Vec<Vec<String>> = body.iter().map(|l| cells(l)).collect();
    let n = rows.iter().map(Vec::len).max()?;
    // The cursor's cell: pipes before it, not counting the leading one (`cells` drops it).
    let col = col.saturating_sub(pre.chars().count());
    let before: String = line.chars().take(col).collect();
    let c = (cells(&format!("{before}x")).len() - 1).min(n - 1);
    // The delimiter row moves like the header.
    let r = match row - first {
        1 => 0,
        r => r,
    };
    let last = rows.len() - 1;
    let (tr, tc) = match key {
        Key::Tab if c + 1 < n => (r, c + 1),
        Key::Tab => (if r == 0 { 2 } else { r + 1 }, 0),
        Key::BackTab if c > 0 => (r, c - 1),
        Key::BackTab if r == 0 => (0, 0),
        Key::BackTab => (if r == 2 { 0 } else { r - 1 }, n - 1),
        Key::Enter if r < 2 => return None,
        Key::Enter if r == last && rows[r].iter().all(String::is_empty) => {
            rows.pop();
            let (text, _) = render(&rows, &aligns, n, (0, 0));
            // The line below keeps the prefix: the quote / list item goes on.
            let text = format!("{}\n{pre}", prefixed(&text, pre));
            let end = text.chars().count();
            return Some(Edit {
                first,
                text,
                select: (end, end),
            });
        }
        Key::Enter => {
            rows.insert(r + 1, vec![]);
            (r + 1, 0)
        }
    };
    if tr == rows.len() {
        rows.push(vec![]);
    }
    let (text, (a, b)) = render(&rows, &aligns, n, (tr, tc));
    // `pre` is added before the selected cell's row and every row above it.
    let shift = pre.chars().count() * (tr + 1);
    Some(Edit {
        first,
        text: prefixed(&text, pre),
        select: (a + shift, b + shift),
    })
}

fn prefixed(text: &str, pre: &str) -> String {
    let rows: Vec<String> = text.lines().map(|l| format!("{pre}{l}")).collect();
    rows.join("\n")
}

/// Line numbers (0-based) of the GFM tables in `text`, for the editor's monospace tag: a
/// header + delimiter pair with the same cell count and the following lines containing `|`.
/// Skips fenced code; rows of one table share their `prefix`, like `edit`. One pass.
pub fn lines(text: &str) -> Vec<usize> {
    let mut open = None;
    let mut out = vec![];
    let mut prev: Option<&str> = None;
    let mut cur = "";
    for (n, line) in text.lines().enumerate() {
        if crate::lists::fence_step(&mut open, line) || open.is_some() || !line.contains('|') {
            prev = None;
            continue;
        }
        let pre = prefix(line);
        if out.last().is_some_and(|&l| l + 1 == n) && pre == cur {
            out.push(n);
        } else if let Some(p) = prev
            && prefix(p) == pre
            && delimiter(&line[pre.len()..])
                .is_some_and(|d| d.len() == cells(&p[pre.len()..]).len())
        {
            out.extend([n - 1, n]);
            cur = pre;
        }
        prev = Some(line);
    }
    out
}

/// Insert Table: the text replacing the cursor's `line` (kept if not blank) with an empty
/// aligned 3-column table (header + 2 rows), blank lines around it where `above` / `below`
/// (None at the start / end of the note) are text. In a quote / list content (`prefix`) every
/// line it adds keeps the line's prefix; on a list item line, plus the marker's width so the
/// table is the item's content. `select` = the first header cell.
pub fn skeleton(above: Option<&str>, line: &str, below: Option<&str>) -> Edit {
    let text_line = |l: Option<&str>| l.is_some_and(|l| !l[prefix(l).len()..].trim().is_empty());
    // A whitespace-only line is blank, not list content.
    let quote = if line.trim().is_empty() {
        ""
    } else {
        prefix(line)
    };
    // On a list item line the table is the item's content: indented past the marker.
    let pre = format!("{quote}{}", " ".repeat(marker_width(&line[quote.len()..])));
    let pre = pre.as_str();
    let blank = pre.trim_end();
    let mut text = if text_line(Some(line)) {
        format!("{line}\n{blank}\n")
    } else if text_line(above) {
        format!("{blank}\n")
    } else {
        String::new()
    };
    let rows = vec![vec![]; 4];
    let (table, (a, b)) = render(&rows, &[Align::None; 3], 3, (0, 0));
    let shift = text.chars().count() + pre.chars().count();
    text += &prefixed(&table, pre);
    if text_line(below) {
        text += &format!("\n{blank}");
    }
    Edit {
        first: 0,
        text,
        select: (a + shift, b + shift),
    }
}

/// Width of the list marker starting `body` (`- `, `* `, `+ `, `1. `, `10) `; a to-do box
/// is item text), 0 if none.
fn marker_width(body: &str) -> usize {
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    let marker = if digits == 0 {
        usize::from(body.starts_with(['-', '*', '+']))
    } else if digits <= 9 && body[digits..].starts_with(['.', ')']) {
        digits + 1
    } else {
        0
    };
    if marker > 0 && body[marker..].starts_with(' ') {
        marker + 1
    } else {
        0
    }
}

fn cell(row: &[String], i: usize) -> &str {
    row.get(i).map_or("", String::as_str)
}

/// The aligned table text (rows[1] is the delimiter) and the char range of cell `at`.
fn render(
    rows: &[Vec<String>],
    aligns: &[Align],
    n: usize,
    at: (usize, usize),
) -> (String, (usize, usize)) {
    let widths: Vec<usize> = (0..n)
        .map(|i| {
            let w = rows.iter().enumerate().filter(|(k, _)| *k != 1);
            w.map(|(_, r)| width(cell(r, i))).max().unwrap_or(0).max(3)
        })
        .collect();
    let mut out = String::new();
    let mut select = (0, 0);
    for (k, row) in rows.iter().enumerate() {
        if k > 0 {
            out.push('\n');
        }
        out.push('|');
        for (i, w) in widths.iter().copied().enumerate() {
            let align = aligns.get(i).copied().unwrap_or(Align::None);
            out.push(' ');
            if k == 1 {
                let d = |n: usize| "-".repeat(n);
                out += &match align {
                    Align::None => d(w),
                    Align::Left => format!(":{}", d(w - 1)),
                    Align::Center => format!(":{}:", d(w - 2)),
                    Align::Right => format!("{}:", d(w - 1)),
                };
            } else {
                let text = cell(row, i);
                let gap = w - width(text);
                let left = match align {
                    Align::Right => gap,
                    Align::Center => gap / 2,
                    _ => 0,
                };
                out += &" ".repeat(left);
                if (k, i) == at {
                    let start = out.chars().count();
                    select = (start, start + text.chars().count());
                }
                out += text;
                out += &" ".repeat(gap - left);
            }
            out += " |";
        }
    }
    (out, select)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, row: usize, col: usize, key: Key) -> Option<(String, String)> {
        let lines: Vec<&str> = text.lines().collect();
        let e = edit(&lines, row, col, key)?;
        let sel: String = e
            .text
            .chars()
            .skip(e.select.0)
            .take(e.select.1 - e.select.0)
            .collect();
        let before: String = e.text.chars().take(e.select.0).collect();
        let lines: Vec<&str> = lines[..e.first].to_vec();
        let mut out = lines.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        // Mark the cursor with ‸ so the tests show where it lands.
        let mark = format!("{out}{before}‸{sel}{}", &e.text[before.len() + sel.len()..]);
        Some((mark, sel))
    }

    #[test]
    fn insert_table() {
        let t =
            "|     |     |     |\n| --- | --- | --- |\n|     |     |     |\n|     |     |     |";
        let e = skeleton(None, "", None);
        assert_eq!((e.text.as_str(), e.select), (t, (2, 2)));
        // Blank lines around it only next to text; a text line is kept above the table.
        let e = skeleton(Some("a"), "  ", Some("b"));
        assert_eq!((e.text, e.select), (format!("\n{t}\n"), (3, 3)));
        let e = skeleton(Some(""), "Tên", Some(""));
        assert_eq!((e.text, e.select), (format!("Tên\n\n{t}"), (7, 7)));
        // The result is a table `edit` understands: Tab moves to the next header cell.
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(edit(&lines, 0, 2, Key::Tab).unwrap().select, (8, 8));
        // In a quote / list content every added line keeps the prefix, Tab still works.
        let q = |p: &str| -> String { t.lines().map(|l| format!("{p}{l}\n")).collect() };
        let e = skeleton(Some("> a"), "> ", Some("> b"));
        assert_eq!(e.text, format!(">\n{}>", q("> ")));
        assert_eq!(e.select, (6, 6));
        let e = skeleton(None, ">> Tên", None);
        assert_eq!(
            e.text.trim_end(),
            format!(">> Tên\n>>\n{}", q(">> ")).trim_end()
        );
        assert_eq!(e.select, (15, 15));
        let e = skeleton(Some("- x"), "    item", Some(""));
        assert_eq!(
            e.text.trim_end(),
            format!("    item\n\n{}", q("    ")).trim_end()
        );
        assert_eq!(e.select, (16, 16));
        let lines: Vec<&str> = e.text.lines().skip(2).collect();
        assert_eq!(edit(&lines, 0, 6, Key::Tab).unwrap().select, (12, 12));
        // On a list item line: indented by the marker width, blank line keeps only `>`.
        for (line, p, blank) in [
            ("- x", "  ", ""),
            ("* [ ] x", "  ", ""),
            ("1. x", "   ", ""),
            ("10) x", "    ", ""),
            ("> - x", ">   ", ">"),
            ("  + x", "    ", ""),
        ] {
            let e = skeleton(None, line, None);
            assert_eq!(e.text, format!("{line}\n{blank}\n{}", q(p)).trim_end());
            let n = line.len() + blank.len() + 2 + p.len() + 2;
            assert_eq!(e.select, (n, n), "{line}");
            let lines: Vec<&str> = e.text.lines().skip(2).collect();
            let c = p.len() + 2;
            assert_eq!(edit(&lines, 0, c, Key::Tab).unwrap().select, (c + 6, c + 6));
        }
        // Not a marker: `-x`, a thematic break.
        assert_eq!(skeleton(None, "-x", None).select, (6, 6));
        assert_eq!(skeleton(None, "---", None).select, (7, 7));
    }

    #[test]
    fn table_lines() {
        let t = "x | y\n| a | b |\n|---|--:|\n| 1 | 2 |\n\ntext\n```\n| c |\n|-|\n```\n| d |\n|-|";
        assert_eq!(lines(t), [1, 2, 3, 10, 11]);
        // Quotes and list items: one prefix on every row.
        assert_eq!(lines("> | a |\n> |-|\n> | 1 |\n| a | b |\n|-|"), [0, 1, 2]);
        assert_eq!(lines("- x\n  | a |\n  |-|\n  | 1 |\n    | 2 |"), [1, 2, 3]);
        assert_eq!(lines("1. x\n    a | b\n    --|--"), [1, 2]);
        assert!(lines("> | a |\n|-|").is_empty());
    }

    #[test]
    fn detects_tables() {
        assert!(edit(&["| a | b |"], 0, 2, Key::Tab).is_none());
        assert!(edit(&["a | b", "c | d"], 0, 1, Key::Tab).is_none());
        // Column counts must match.
        assert!(edit(&["| a | b |", "|---|"], 0, 2, Key::Tab).is_none());
        // No outer pipes is still a table.
        assert!(edit(&["a | b", "--|--", "1 | 2"], 2, 0, Key::Tab).is_some());
        // Differing prefixes are not one table.
        assert!(edit(&["> | a |", "|---|"], 0, 3, Key::Tab).is_none());
        assert!(edit(&["> | a |", "> |---|", ">> | 1 |"], 0, 3, Key::Tab).is_none());
        // A `|` line above the header is not part of the table.
        let (text, _) = run("x | y z\n| a |\n|-|\n| b |", 3, 2, Key::Tab).unwrap();
        assert!(text.starts_with("x | y z\n| a   |"));
        assert!(edit(&["x | y", "| a |", "|-|"], 0, 1, Key::Tab).is_none());
    }

    #[test]
    fn aligns_with_vietnamese_and_alignment() {
        let t = "| Tên | b |\n|:-:|--:|\n| Nguyễn Văn | 1 |\n| e\u{0302} | 22 |";
        let (text, sel) = run(t, 0, 2, Key::Tab).unwrap();
        assert_eq!(
            text,
            "|    Tên     |   ‸b |\n\
             | :--------: | --: |\n\
             | Nguyễn Văn |   1 |\n\
             |     e\u{0302}      |  22 |"
        );
        assert_eq!(sel, "b");
        let (text, _) = run("| 中文 | :--- |\n|:--|---|\n| x | 😀 |", 0, 0, Key::Tab).unwrap();
        assert_eq!(text, "| 中文 | ‸:--- |\n| :--- | ---- |\n| x    | 😀   |");
    }

    #[test]
    fn tab_moves_and_adds_rows() {
        let t = "| a | b |\n|---|---|\n| 1 | 2 |";
        // End of the header → first body cell (skips the delimiter).
        assert_eq!(run(t, 0, 6, Key::Tab).unwrap().1, "1");
        // End of the last row → a new empty row, cursor in its first cell.
        let (text, sel) = run(t, 2, 7, Key::Tab).unwrap();
        assert_eq!(
            text,
            "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n| ‸    |     |"
        );
        assert_eq!(sel, "");
        // Shift+Tab: previous cell, wrapping to the end of the row above.
        assert_eq!(run(t, 2, 6, Key::BackTab).unwrap().1, "1");
        assert_eq!(run(t, 2, 2, Key::BackTab).unwrap().1, "b");
        assert_eq!(run(t, 0, 2, Key::BackTab).unwrap().1, "a");
        // Short rows get their missing cells.
        let (text, _) = run("| a | b |\n|---|---|\n| 1 |", 2, 3, Key::Tab).unwrap();
        assert!(text.ends_with("| 1   | ‸    |"));
    }

    #[test]
    fn enter_adds_row_or_leaves_table() {
        let t = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
        assert!(run(t, 0, 3, Key::Enter).is_none());
        let (text, _) = run(t, 2, 3, Key::Enter).unwrap();
        assert_eq!(
            text,
            "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n| ‸    |     |\n| 3   | 4   |"
        );
        // Enter on an empty last row: the row goes, the cursor is on an empty line below.
        let (text, _) = run("| a | b |\n|---|---|\n| 1 | 2 |\n| | |", 3, 2, Key::Enter).unwrap();
        assert_eq!(text, "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n‸");
        // An empty row that is not the last one just gets a row below.
        let (text, _) = run("| a |\n|---|\n|  |\n| 1 |", 2, 1, Key::Enter).unwrap();
        assert_eq!(text.lines().count(), 5);
    }

    #[test]
    fn keeps_quote_and_list_prefixes() {
        let t = "> | a | b |\n> |---|---|\n> | 1 | 2 |";
        let (text, sel) = run(t, 0, 4, Key::Tab).unwrap();
        assert_eq!(text, "> | a   | ‸b   |\n> | --- | --- |\n> | 1   | 2   |");
        assert_eq!(sel, "b");
        let (text, _) = run(t, 2, 9, Key::Tab).unwrap();
        assert!(text.ends_with("> | 1   | 2   |\n> | ‸    |     |"));
        let (text, sel) = run(
            ">> | a | b |\n>> |---|---|\n>> | 1 | 2 |",
            2,
            3,
            Key::BackTab,
        )
        .unwrap();
        assert_eq!(sel, "b");
        assert!(text.starts_with(">> | a   | ‸b   |"));
        // Cursor inside the prefix counts as the first cell.
        assert_eq!(run(t, 2, 0, Key::Tab).unwrap().1, "2");
        // List item indented 2 and 4 spaces.
        let (text, _) = run("- x\n  | a |\n  |---|\n  | 1 |", 3, 4, Key::Enter).unwrap();
        assert_eq!(text, "- x\n  | a   |\n  | --- |\n  | 1   |\n  | ‸    |");
        let (text, _) = run("    | a |\n    |-|\n    | 1 |", 2, 6, Key::Tab).unwrap();
        assert!(text.ends_with("    | 1   |\n    | ‸    |"));
        // Leaving the table keeps the quote going.
        let (text, _) = run("> | a |\n> |---|\n> | 1 |\n> |  |", 3, 3, Key::Enter).unwrap();
        assert_eq!(text, "> | a   |\n> | --- |\n> | 1   |\n> ‸");
    }

    #[test]
    fn escaped_pipes_stay_in_the_cell() {
        let (text, _) = run("| a \\| b | c |\n|---|---|", 0, 1, Key::Tab).unwrap();
        assert_eq!(text, "| a \\| b | ‸c   |\n| ------ | --- |");
    }
}
