//! Tags: `#tag` and nested `#work/project` anywhere in a note. Pure logic, no GTK.
//!
//! A tag is `#` then a letter, then letters/digits/`_`/`-`/`/` (a trailing `/` or `-` is not
//! part of it). The `#` must start the line or follow whitespace or punctuation other than
//! `/`, `&`, `#`, `\` (so `## Heading`, `#1`, `x#y`, `http://x/#a` are not tags). Front matter,
//! fenced code blocks, code spans and link destinations `](…)` are skipped.
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

/// (0-based line, byte range in that line) of every tag in `text`, outside front matter and
/// fenced code.
pub fn spans(text: &str) -> Vec<(usize, Range<usize>)> {
    let mut open = None;
    let mut out = Vec::new();
    let skip = lists::front_matter_lines(text);
    for (n, line) in text.lines().enumerate().skip(skip) {
        // shortcut: indented (4-space) code blocks are not skipped; fine for notes.
        if lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        out.extend(in_line(line).into_iter().map(|r| (n, r)));
    }
    out
}

/// The tags of `text` without `#`, each once, sorted.
pub fn names(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = spans(text)
        .into_iter()
        .map(|(n, r)| lines[n][r.start + 1..r.end].to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(text: &str) -> Vec<String> {
        names(text)
    }

    #[test]
    fn front_matter_skipped() {
        assert_eq!(tags("---\ntitle: '#x'\n---\n#y"), ["y"]);
        assert_eq!(spans("---\n#x\n---\n#y"), [(3, 0..2)]);
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
    fn spans_are_line_byte_ranges() {
        assert_eq!(spans("a\n  #x y #z"), [(1, 2..4), (1, 7..9)]);
    }

    #[test]
    fn index_tree_order_and_counts() {
        let dir = std::env::temp_dir().join(format!("mm-tags-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.md"), dir.join("b.md"));
        fs::write(&a, "#work/project #work-x").unwrap();
        fs::write(&b, "#work #home").unwrap();
        let got: Vec<(String, usize)> = index(&[a, b, dir.join("missing.md")])
            .into_iter()
            .map(|(t, n)| (t, n.len()))
            .collect();
        fs::remove_dir_all(&dir).unwrap();
        let want = [("home", 1), ("work", 2), ("work/project", 1), ("work-x", 1)];
        assert_eq!(got, want.map(|(t, n)| (t.to_string(), n)));
    }
}
