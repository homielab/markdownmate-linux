//! Tags: `#tag` and nested `#work/project` anywhere in a note. Pure logic, no GTK.
//!
//! A tag is `#` then a letter, then letters/digits/`_`/`-`/`/` (a trailing `/` or `-` is not
//! part of it). The `#` must start the line or follow whitespace or punctuation other than
//! `/`, `&`, `#`, `\` (so `## Heading`, `#1`, `x#y`, `http://x/#a` are not tags). Front matter,
//! fenced code blocks, code spans and link destinations `](…)` are skipped.
//!
//! The YAML front matter's `tags:` (or `tag:`) key adds tags too: a flow list `[a, b/c]`, a
//! block list (`- a` lines) or a string (`a, b` or `a b`); each value, with or without a leading
//! `#`, counts if `#value` would be read back whole as a tag (`front_matter_values`). They join
//! the inline tags in `names`, `all_spans` and the index; the editor colours only inline ones.
//!
//! Preferences: tags can be turned off (no tag anywhere), and colour codes (`#fff`,
//! `#a1b2c3ff`: 3/4/6/8 hex digits) are not tags unless that is turned off. Both are
//! process-wide (`set_options`) so the editor, preview, completion and index agree.

use std::collections::BTreeMap;
use std::fs;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::lists;

static ENABLED: AtomicBool = AtomicBool::new(true);
static IGNORE_COLOURS: AtomicBool = AtomicBool::new(true);

/// Applies the Preferences: tags on/off, colour codes ignored or not.
pub fn set_options(enabled: bool, ignore_colours: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    IGNORE_COLOURS.store(ignore_colours, Ordering::Relaxed);
}

/// Whether tags are on (Preferences).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Whether a tag name (without `#`) is a colour code: 3, 4, 6 or 8 hex digits.
fn is_colour(name: &str) -> bool {
    matches!(name.len(), 3 | 4 | 6 | 8) && name.chars().all(|c| c.is_ascii_hexdigit())
}

/// Byte ranges (with the `#`) of the tags in one line of prose, per the Preferences.
pub fn in_line(line: &str) -> Vec<Range<usize>> {
    if !enabled() {
        return Vec::new();
    }
    let mut out = find(line);
    if IGNORE_COLOURS.load(Ordering::Relaxed) {
        out.retain(|r| !is_colour(&line[r.start + 1..r.end]));
    }
    out
}

/// Every tag in `line`, colour codes included.
fn find(line: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut prev: Option<char> = None;
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            // Code span: skip to the closing run of the same length (unclosed: rest is text).
            '`' => {
                let run = line[i..].len() - line[i..].trim_start_matches('`').len();
                let after = i + run;
                let fence = &line[i..after];
                if let Some(close) = line[after..].find(fence) {
                    let end = after + close + run;
                    while chars.peek().is_some_and(|&(j, _)| j < end) {
                        chars.next();
                    }
                } else {
                    while chars.peek().is_some_and(|&(j, _)| j < after) {
                        chars.next();
                    }
                }
                prev = Some('`');
                continue;
            }
            // Link destination `](…)`: skip to the `)`.
            '(' if prev == Some(']') => {
                if let Some(close) = line[i..].find(')') {
                    let end = i + close;
                    while chars.peek().is_some_and(|&(j, _)| j <= end) {
                        chars.next();
                    }
                    prev = Some(')');
                    continue;
                }
            }
            '#' if prev.is_none_or(starts_tag) && !in_url(&line[..i]) => {
                let body = &line[i + 1..];
                if body.chars().next().is_some_and(char::is_alphabetic) {
                    let len = body
                        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '/')))
                        .unwrap_or(body.len());
                    let name = body[..len].trim_end_matches(['/', '-']);
                    let end = i + 1 + name.len();
                    out.push(i..end);
                    while chars.peek().is_some_and(|&(j, _)| j < end) {
                        chars.next();
                    }
                    prev = name.chars().last();
                    continue;
                }
            }
            _ => {}
        }
        prev = Some(c);
    }
    out
}

/// Whether `#` may start a tag right after `c`.
fn starts_tag(c: char) -> bool {
    c.is_whitespace() || (c.is_ascii_punctuation() && !matches!(c, '/' | '&' | '#' | '\\'))
}

/// Whether the word ending at the `#` is a URL (`http://x/a?b=(#c)`).
fn in_url(before: &str) -> bool {
    let word = before.rsplit(char::is_whitespace).next().unwrap_or("");
    word.contains("://") || word.starts_with("www.")
}

/// (0-based line, byte range in that line) of every inline tag in `text`, outside front
/// matter and fenced code (what the editor colours).
pub fn spans(text: &str) -> Vec<(usize, Range<usize>)> {
    inline_spans(text, lists::front_matter_lines(text))
}

/// `spans` after the first `skip` lines.
fn inline_spans(text: &str, skip: usize) -> Vec<(usize, Range<usize>)> {
    let mut open = None;
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate().skip(skip) {
        // shortcut: indented (4-space) code blocks are not skipped; fine for notes.
        if lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        out.extend(in_line(line).into_iter().map(|r| (n, r)));
    }
    out
}

/// Every tag of `text` in text order: the front matter's values (`front_matter_values`), then
/// the inline tags (`spans`). Each range is the tag with its `#` if written (always inline).
pub fn all_spans(text: &str) -> Vec<(usize, Range<usize>)> {
    let skip = lists::front_matter_lines(text);
    let mut out = front_matter_values(text, skip);
    out.extend(inline_spans(text, skip));
    out
}

/// (0-based line, byte range in that line) of each tag value of the front matter's `tags:` /
/// `tag:` keys (top level), read by `front_matter_list` with values split at spaces too. A
/// range covers the value inside its quotes, with its `#` if written. Values that are not a
/// whole tag (`#value` read by `in_line`) are skipped. `lines`: the front matter's line count
/// (`lists::front_matter_lines`).
fn front_matter_values(text: &str, lines: usize) -> Vec<(usize, Range<usize>)> {
    if !enabled() {
        return Vec::new();
    }
    let rows: Vec<&str> = text.lines().take(lines).collect();
    front_matter_list(text, lines, &["tags:", "tag:"], true)
        .into_iter()
        .filter(|(row, r)| {
            let value = &rows[*row][r.clone()];
            let tag = format!("#{}", value.strip_prefix('#').unwrap_or(value));
            in_line(&tag).first() == Some(&(0..tag.len()))
        })
        .collect()
}

/// (0-based line, byte range in that line) of each value of the front matter's top-level
/// `keys` (each with its `:`, as `"tags:"`): flow list `[a, "b c"]` (may span lines up to its
/// `]`; unclosed within the front matter → none), block list (`- a` lines below the key) or
/// string (split at commas, and at spaces if `spaces`). A range covers the value trimmed and
/// inside its quotes; empty values are skipped, and a YAML comment (`#` then a space) ends a
/// value. `lines`: the front matter's line count (`lists::front_matter_lines`).
pub(crate) fn front_matter_list(
    text: &str,
    lines: usize,
    keys: &[&str],
    spaces: bool,
) -> Vec<(usize, Range<usize>)> {
    let mut out = Vec::new();
    if lines == 0 {
        return out;
    }
    // The lines between the two `---`.
    let body: Vec<&str> = text.lines().take(lines - 1).collect();
    let mut n = 1;
    while n < body.len() {
        let (row, line) = (n, body[n]);
        n += 1;
        let Some(start) = list_key(line, keys) else {
            continue;
        };
        let end = uncommented(line, start);
        let value = line[start..end].trim();
        if value.is_empty() {
            // Block list: `- value` lines (any indent; blank and whole-line comment lines
            // between them skipped).
            while let Some(&item) = body.get(n) {
                let t = item.trim_start();
                if !t.is_empty() && !t.starts_with('#') {
                    let Some(rest) = t
                        .strip_prefix('-')
                        .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
                    else {
                        break;
                    };
                    let from = item.len() - rest.len();
                    push_value(&mut out, n, item, from, uncommented(item, from));
                }
                n += 1;
            }
        } else if value.starts_with('[') {
            // Flow list: comma separated up to its `]`, which may be on a later line (each
            // line's comment cut); unclosed within the front matter → no values.
            let (mut row, mut line, mut end) = (row, line, end);
            let mut from = line[..end].find('[').map_or(end, |i| i + 1);
            let mut found = Vec::new();
            loop {
                let close = line[from..end].find(']');
                let to = close.map_or(end, |i| from + i);
                let mut at = from;
                for part in line[from..to].split(',') {
                    push_value(&mut found, row, line, at, at + part.len());
                    at += part.len() + 1;
                }
                if close.is_some() {
                    out.append(&mut found);
                    break;
                }
                let Some(&next) = body.get(n) else {
                    break;
                };
                (row, line, end, from) = (n, next, uncommented(next, 0), 0);
                n += 1;
            }
        } else {
            // String: quotes around the whole value dropped, then split.
            let (from, to) = unquoted(line, start, end);
            let mut at = from;
            for part in
                line[from..to].split(|c: char| c == ',' || spaces && c.is_ascii_whitespace())
            {
                push_value(&mut out, row, line, at, at + part.len());
                at += part.len() + 1;
            }
        }
    }
    out
}

/// Where the value of a top-level key of `keys` starts in `line` (after the `:`).
fn list_key(line: &str, keys: &[&str]) -> Option<usize> {
    let rest = keys.iter().find_map(|k| line.strip_prefix(k))?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| line.len() - rest.len())
}

/// Where the value starting at `from` in `line` ends: at a YAML comment (` #` then a space or
/// the end), else the line's end.
pub(crate) fn uncommented(line: &str, from: usize) -> usize {
    let bytes = line.as_bytes();
    (from..line.len())
        .find(|&i| {
            bytes[i] == b'#'
                && (i == from || bytes[i - 1].is_ascii_whitespace())
                && bytes.get(i + 1).is_none_or(u8::is_ascii_whitespace)
        })
        .unwrap_or(line.len())
}

/// `line[from..to]` trimmed, without a pair of `"` or `'` around it: its new bounds.
pub(crate) fn unquoted(line: &str, from: usize, to: usize) -> (usize, usize) {
    let raw = &line[from..to];
    let from = from + raw.len() - raw.trim_start().len();
    let to = from + raw.trim().len();
    let t = &line[from..to];
    if t.len() >= 2
        && (t.starts_with('"') && t.ends_with('"') || t.starts_with('\'') && t.ends_with('\''))
    {
        (from + 1, to - 1)
    } else {
        (from, to)
    }
}

/// Adds `line[from..to]` (trimmed, unquoted) as a value of line `row` unless it is empty.
fn push_value(
    out: &mut Vec<(usize, Range<usize>)>,
    row: usize,
    line: &str,
    from: usize,
    to: usize,
) {
    let (from, to) = unquoted(line, from, to);
    if from < to {
        out.push((row, from..to));
    }
}

/// The tags of `text` (front matter and inline) without `#`, each once, sorted.
pub fn names(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = all_spans(text)
        .into_iter()
        .map(|(n, r)| {
            let tag = &lines[n][r];
            tag.strip_prefix('#').unwrap_or(tag).to_string()
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every tag in `notes` (unreadable notes skipped) with the notes that have it, in tree
/// order (`a`, `a/b`, `a/c`, `b`). A note tagged `a/b` also counts for `a`.
pub fn index(notes: &[PathBuf]) -> Vec<(String, Vec<PathBuf>)> {
    let mut map: BTreeMap<Vec<String>, Vec<PathBuf>> = BTreeMap::new();
    for path in notes {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let mut keys: Vec<Vec<String>> = Vec::new();
        for name in names(&text) {
            let parts: Vec<String> = name
                .split('/')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect();
            for n in 1..=parts.len() {
                keys.push(parts[..n].to_vec());
            }
        }
        keys.sort();
        keys.dedup();
        for key in keys {
            map.entry(key).or_default().push(path.clone());
        }
    }
    map.into_iter().map(|(k, v)| (k.join("/"), v)).collect()
}

/// The inline tag (without `#`) at byte `col` of 0-based `line` in `text`, as `spans` finds
/// it (front-matter tags, code and URLs are not tags). The `#` and the end count as on it.
#[cfg(test)]
pub fn at(text: &str, line: usize, col: usize) -> Option<String> {
    at_row(text.lines().nth(line)?, line, col, || text.to_string())
}

/// `at` for pointer motion: only `row` (line `line`) is scanned; the whole `text` is read
/// (for code blocks and front matter) only when the row has a tag under `col`. Same result.
pub fn at_row(row: &str, line: usize, col: usize, text: impl FnOnce() -> String) -> Option<String> {
    let r = in_line(row)
        .into_iter()
        .find(|r| r.start <= col && col <= r.end)?;
    let text = text();
    spans(&text)
        .iter()
        .any(|(n, _)| *n == line)
        .then(|| row[r.start + 1..r.end].to_string())
}

/// The row of `tag` in `index` (`index`'s tree order), compared case-insensitively: the tag's
/// own row, else its nearest listed parent (`a/b/c` → `a/b` → `a`).
pub fn row_of(index: &[(String, Vec<PathBuf>)], tag: &str) -> Option<usize> {
    let parts: Vec<&str> = tag.split('/').filter(|p| !p.is_empty()).collect();
    (1..=parts.len()).rev().find_map(|n| {
        let key = parts[..n].join("/").to_lowercase();
        index.iter().position(|(t, _)| t.to_lowercase() == key)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_at_offset() {
        let text = "---\ntags: [fm]\n---\nSee #work/project, mail a#b\n```\n#code\n```\n";
        assert_eq!(at(text, 3, 4).as_deref(), Some("work/project"));
        assert_eq!(at(text, 3, 10).as_deref(), Some("work/project"));
        assert_eq!(at(text, 3, 16).as_deref(), Some("work/project"));
        assert_eq!(at(text, 3, 2), None);
        assert_eq!(at(text, 3, 26), None);
        assert_eq!(at(text, 1, 8), None);
        assert_eq!(at(text, 5, 2), None);
        assert_eq!(at(text, 99, 0), None);
    }

    #[test]
    fn row_of_nested_tag() {
        let index: Vec<(String, Vec<PathBuf>)> = ["a", "work", "work/project", "work/zz"]
            .iter()
            .map(|t| (t.to_string(), Vec::new()))
            .collect();
        assert_eq!(row_of(&index, "work/project"), Some(2));
        assert_eq!(row_of(&index, "Work/Project"), Some(2));
        assert_eq!(row_of(&index, "work"), Some(1));
        assert_eq!(row_of(&index, "work/gone/x"), Some(1));
        assert_eq!(row_of(&index, "none"), None);
    }

    fn tags(text: &str) -> Vec<String> {
        names(text)
    }

    #[test]
    fn front_matter_skipped() {
        assert_eq!(tags("---\ntitle: '#x'\n---\n#y"), ["y"]);
        assert_eq!(spans("---\n#x\n---\n#y"), [(3, 0..2)]);
    }

    fn front_matter_spans(text: &str) -> Vec<(usize, Range<usize>)> {
        front_matter_values(text, lists::front_matter_lines(text))
    }

    /// The front-matter values of `text` as written (with their `#` if any).
    fn values(text: &str) -> Vec<String> {
        let lines: Vec<&str> = text.lines().collect();
        front_matter_spans(text)
            .into_iter()
            .map(|(n, r)| lines[n][r].to_string())
            .collect()
    }

    #[test]
    fn front_matter_flow_list() {
        assert_eq!(values("---\ntags: [a, b/c]\n---\n"), ["a", "b/c"]);
        assert_eq!(
            values("---\ntags: [ \"a\", 'Work/Project' , #c,]\n---\n"),
            ["a", "Work/Project", "#c"]
        );
        assert_eq!(
            front_matter_spans("---\ntags: [a, \"b\"]\n---\n"),
            [(1, 7..8), (1, 11..12)]
        );
        assert!(values("---\ntags: []\n---\n").is_empty());
        // Over several lines up to `]`, comments cut; unclosed in the front matter: none.
        let md = "---\ntags: [a, # first\n  'b/c',\n\n  #d\n  ] # end\nother: [x]\n---\n";
        assert_eq!(values(md), ["a", "b/c", "#d"]);
        assert_eq!(front_matter_spans(md), [(1, 7..8), (2, 3..6), (4, 2..4)]);
        assert_eq!(values("---\r\ntags: [a,\r\n b]\r\n---\r\n"), ["a", "b"]);
        assert!(values("---\ntags: [a,\n  b\n---\n[c]\n").is_empty());
        assert_eq!(tags("---\ntags: [a, b/c]\n---\n#d"), ["a", "b/c", "d"]);
    }

    #[test]
    fn front_matter_block_list() {
        let md = "---\ntitle: x\ntags:\n  - a\n  - \"#b/c\"\n\n- 'd'\nother: [e]\n- f\n---\n";
        assert_eq!(values(md), ["a", "#b/c", "d"]);
        assert_eq!(tags(md), ["a", "b/c", "d"]);
        let md = "---\ntags:\n  # work\n  - a\n\n  # more\n#x\n  - b # why\n---\n";
        assert_eq!(values(md), ["a", "b"]);
    }

    #[test]
    fn front_matter_string() {
        assert_eq!(values("---\ntags: a, b\n---\n"), ["a", "b"]);
        assert_eq!(values("---\ntags: a b,c\n---\n"), ["a", "b", "c"]);
        assert_eq!(values("---\ntag: \"#a #b/c\"\n---\n"), ["#a", "#b/c"]);
        assert_eq!(values("---\ntags: #a #b # a comment\n---\n"), ["#a", "#b"]);
        assert_eq!(values("---\ntags: work\n---\n"), ["work"]);
    }

    #[test]
    fn front_matter_invalid_skipped() {
        // Not a tag read back whole: skipped, the others kept.
        assert_eq!(
            values("---\ntags: [1st, two words, a.b, ok, b/, fff, '']\n---\n"),
            ["ok"]
        );
        assert_eq!(values("---\ntags: 2024, x-, ok, ##y\n---\n"), ["ok"]);
        // Other keys, nested keys, `tagsx:`, no closing `---`, not on line 1: none.
        assert!(values("---\ntitle: [a]\nmeta:\n  tags: [b]\ntagsx: c\n---\n").is_empty());
        assert!(values("---\ntags: [a]\n").is_empty());
        assert!(values("\n---\ntags: [a]\n---\n").is_empty());
        assert!(values("tags: [a]\n").is_empty());
    }

    #[test]
    fn front_matter_crlf_and_unicode() {
        let md = "---\r\ntags: [việc-làm, Đọc_sách]\r\ntag:\r\n  - nhà\r\n---\r\n#x\r\n";
        assert_eq!(values(md), ["việc-làm", "Đọc_sách", "nhà"]);
        assert_eq!(tags(md), ["nhà", "việc-làm", "x", "Đọc_sách"]);
    }

    #[test]
    fn front_matter_not_coloured() {
        // `spans` (the editor's colouring) stays inline only.
        assert_eq!(spans("---\ntags: [a, #b]\n---\n#c"), [(3, 0..2)]);
        assert_eq!(
            all_spans("---\ntags: [a, #b]\n---\n#c"),
            [(1, 7..8), (1, 10..12), (3, 0..2)]
        );
    }

    #[test]
    fn plain_and_nested() {
        assert_eq!(tags("Hôm nay #work và #ý_tưởng."), ["work", "ý_tưởng"]);
        assert_eq!(tags("#work/project, (#home)"), ["home", "work/project"]);
        assert_eq!(tags("#a/b/ #c-"), ["a/b", "c"]);
        assert_eq!(tags("- [ ] #todo"), ["todo"]);
    }

    #[test]
    fn vietnamese() {
        assert_eq!(tags("#việc-làm #Đọc_sách"), ["việc-làm", "Đọc_sách"]);
        assert_eq!(spans("x #việc-làm"), [(0, 2..14)]);
    }

    #[test]
    fn not_tags() {
        assert!(tags("#1 #2024 #").is_empty());
        assert!(tags("# Heading\n## Two #\nabc#def a/#b &#39;").is_empty());
        assert!(tags("http://x/#a https://y.com/p?q=(#b) www.z.vn/(#c)").is_empty());
        assert!(tags("[link](#anchor) [x](http://a/#b)").is_empty());
    }

    #[test]
    fn heading_text_may_have_tags() {
        assert_eq!(tags("## Plan #work"), ["work"]);
    }

    #[test]
    fn code() {
        assert!(tags("`#a` ``x #b ` y``").is_empty());
        assert_eq!(tags("`#a` #c"), ["c"]);
        assert!(tags("```\n#a\n```\n~~~\n#b\n~~~").is_empty());
        assert_eq!(tags("```\n#a\n```\n#after"), ["after"]);
    }

    #[test]
    fn colour_codes() {
        for c in ["fff", "FFF", "face", "a1b2c3", "A1b2C3ff"] {
            assert!(is_colour(c), "{c}");
        }
        for c in ["ff", "fffff", "a1b2c3f", "a1b2c3fff", "fffg", "work", "ạbc"] {
            assert!(!is_colour(c), "{c}");
        }
        // Default: colour codes are not tags; `find` (ignoring off) still sees them.
        assert_eq!(
            tags("#fff #a1b2c3 #work #ff #abcdeg"),
            ["abcdeg", "ff", "work"]
        );
        assert_eq!(find("#fff #work").len(), 2);
    }

    #[test]
    fn row_scan_reads_the_text_only_on_a_tag() {
        let text = "```\n#code\n```\nSee #work and more";
        let read = std::cell::Cell::new(0);
        let whole = || {
            read.set(read.get() + 1);
            text.to_string()
        };
        assert_eq!(at_row("See #work and more", 3, 12, whole), None);
        assert_eq!(read.get(), 0);
        assert_eq!(
            at_row("See #work and more", 3, 5, whole).as_deref(),
            Some("work")
        );
        assert_eq!(at_row("#code", 1, 2, whole), None); // inside the fence
        assert_eq!(read.get(), 2);
        for (line, row) in text.lines().enumerate() {
            for col in 0..=row.len() {
                assert_eq!(at_row(row, line, col, whole), at(text, line, col));
            }
        }
    }

    #[test]
    fn spans_are_line_byte_ranges() {
        assert_eq!(spans("a\n  #x y #z"), [(1, 2..4), (1, 7..9)]);
    }

    #[test]
    fn index_tree_order_and_counts() {
        let dir = std::env::temp_dir().join(format!("mm-tags-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.md"), dir.join("b.md"));
        fs::write(&a, "#work/project #work-x").unwrap();
        fs::write(&b, "---\ntags: [work, home]\n---\n#home").unwrap();
        let got: Vec<(String, usize)> = index(&[a, b, dir.join("missing.md")])
            .into_iter()
            .map(|(t, n)| (t, n.len()))
            .collect();
        fs::remove_dir_all(&dir).unwrap();
        let want = [("home", 1), ("work", 2), ("work/project", 1), ("work-x", 1)];
        assert_eq!(got, want.map(|(t, n)| (t.to_string(), n)));
    }

    /// Times `index` over a generated folder (best of 5):
    /// `cargo run --release --example generate_notes -- /tmp/notes 10000`, then
    /// `MM_TAGS_NOTES=/tmp/notes cargo test --release tags::tests::time_index -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn time_index() {
        fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "md") {
                    out.push(path);
                }
            }
        }
        let root = PathBuf::from(std::env::var("MM_TAGS_NOTES").expect("MM_TAGS_NOTES"));
        let mut paths = Vec::new();
        walk(&root, &mut paths);
        let mut best = std::time::Duration::MAX;
        let mut count = 0;
        for _ in 0..5 {
            let start = std::time::Instant::now();
            count = index(&paths).len();
            best = best.min(start.elapsed());
        }
        println!("{} notes: {best:?} ({count} tags)", paths.len());
    }
}
