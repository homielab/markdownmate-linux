//! Find in Folder (Shift+Ctrl+F): case-insensitive full-text search over note files. Pure
//! Rust (no GTK) so it is testable headless; the window runs `search` on a background thread.
//!
//! Matching is substring, Unicode-aware lowercase (char by char, so query and text fold the
//! same way). Lowercasing can change byte lengths (e.g. 'İ' → "i̇"), so match positions are
//! mapped back to whole characters of the original text: a `Range` always lies on char
//! boundaries of the snippet.

use std::ops::Range;
use std::path::{Path, PathBuf};

/// Most matching lines returned over all files.
pub const LIMIT: usize = 500;
/// Snippets longer than this (in chars) are cut around the first match.
const SNIPPET_CHARS: usize = 160;
/// Chars kept before the first match when a snippet is cut.
const LEAD_CHARS: usize = 40;

/// One matching line.
#[derive(Debug, PartialEq)]
pub struct LineHit {
    /// 1-based line number.
    pub line: usize,
    /// The line (or a window of it, with "…" where cut).
    pub snippet: String,
    /// Byte ranges of the matches within `snippet`.
    pub ranges: Vec<Range<usize>>,
}

#[derive(Debug)]
pub struct FileHits {
    pub path: PathBuf,
    pub lines: Vec<LineHit>,
}

#[derive(Debug, Default)]
pub struct Found {
    pub files: Vec<FileHits>,
    /// More lines matched than `LIMIT`.
    pub more: bool,
}

/// Unicode lowercase, char by char.
pub fn fold(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Byte ranges (in `line`) of the non-overlapping matches of `query` (already `fold`ed,
/// non-empty).
pub fn matches(line: &str, query: &str) -> Vec<Range<usize>> {
    // For each byte of the folded line: the original char it came from.
    let mut folded = String::with_capacity(line.len());
    let mut origin: Vec<Range<usize>> = Vec::with_capacity(line.len());
    for (i, c) in line.char_indices() {
        for lc in c.to_lowercase() {
            folded.push(lc);
            origin.extend(std::iter::repeat_n(i..i + c.len_utf8(), lc.len_utf8()));
        }
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for (at, m) in folded.match_indices(query) {
        let start = origin[at].start;
        let end = origin[at + m.len() - 1].end;
        // Two matches inside one expanded char map to the same original char: merge.
        match out.last_mut() {
            Some(last) if start < last.end => last.end = last.end.max(end),
            _ => out.push(start..end),
        }
    }
    out
}

/// Cuts long lines to a window around the first match; ranges are shifted to match.
pub fn snippet(line: &str, ranges: Vec<Range<usize>>) -> (String, Vec<Range<usize>>) {
    if line.chars().count() <= SNIPPET_CHARS {
        return (line.to_string(), ranges);
    }
    let first = ranges[0].start;
    let lead: usize = line[..first]
        .chars()
        .rev()
        .take(LEAD_CHARS)
        .map(char::len_utf8)
        .sum();
    let start = first - lead;
    let end = line[start..]
        .char_indices()
        .nth(SNIPPET_CHARS)
        .map_or(line.len(), |(i, _)| start + i);
    let prefix = if start > 0 { "…" } else { "" };
    let mut text = format!("{prefix}{}", &line[start..end]);
    if end < line.len() {
        text.push('…');
    }
    let shift = |b: usize| b.clamp(start, end) - start + prefix.len();
    let ranges = ranges
        .into_iter()
        .filter(|r| r.start < end)
        .map(|r| shift(r.start)..shift(r.end))
        .collect();
    (text, ranges)
}

/// Matching lines of `text`, at most `limit`. Returns the hits and whether more matched.
pub fn find_in_text(text: &str, query: &str, limit: usize) -> (Vec<LineHit>, bool) {
    let mut hits = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let ranges = matches(line, query);
        if ranges.is_empty() {
            continue;
        }
        if hits.len() == limit {
            return (hits, true);
        }
        let (snippet, ranges) = snippet(line, ranges);
        hits.push(LineHit {
            line: n + 1,
            snippet,
            ranges,
        });
    }
    (hits, false)
}

/// Searches `paths` for `query` (trimmed, case-insensitive). Files that cannot be read or are
/// not UTF-8 are skipped. `open` is the open note's (path, buffer text): it is searched instead of
/// the disk copy so unsaved edits are found. Returns None as soon as `cancelled()` is true.
pub fn search(
    paths: &[PathBuf],
    open: Option<&(PathBuf, String)>,
    query: &str,
    cancelled: &dyn Fn() -> bool,
) -> Option<Found> {
    let query = fold(query.trim());
    let mut found = Found::default();
    if query.is_empty() {
        return Some(found);
    }
    let mut left = LIMIT;
    for path in paths {
        if cancelled() {
            return None;
        }
        let text = match open {
            Some((p, text)) if p == path => text.clone(),
            _ => match read_utf8(path) {
                Some(text) => text,
                None => continue,
            },
        };
        let (lines, more) = find_in_text(&text, &query, left);
        left -= lines.len();
        if !lines.is_empty() {
            found.files.push(FileHits {
                path: path.clone(),
                lines,
            });
        }
        if more {
            found.more = true;
            break;
        }
    }
    Some(found)
}

pub fn read_utf8(path: &Path) -> Option<String> {
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(text: &str, query: &str) -> Vec<(usize, String, Vec<String>)> {
        find_in_text(text, &fold(query), LIMIT)
            .0
            .into_iter()
            .map(|h| {
                let parts = h.ranges.iter().map(|r| h.snippet[r.clone()].to_string());
                (h.line, h.snippet.clone(), parts.collect())
            })
            .collect()
    }

    #[test]
    fn case_insensitive_unicode() {
        assert_eq!(
            hit("one\nHello WORLD\nnothing", "world"),
            [(2, "Hello WORLD".into(), vec!["WORLD".into()])]
        );
        assert_eq!(
            hit("Tiếng VIỆT", "việt"),
            [(1, "Tiếng VIỆT".into(), vec!["VIỆT".into()])]
        );
        assert_eq!(hit("ΣΟΦΙΑ", "σοφια")[0].2, ["ΣΟΦΙΑ"]);
    }

    #[test]
    fn many_matches_in_one_line() {
        assert_eq!(hit("ab Ab aB xx ab", "ab")[0].2, ["ab", "Ab", "aB", "ab"]);
    }

    #[test]
    fn length_changing_lowercase_does_not_panic() {
        // 'İ' lowercases to "i\u{307}" (3 bytes from 2): ranges cover whole original chars.
        let r = hit("xİy İstanbul", "i");
        assert_eq!(r[0].2, ["İ", "İ"]);
        let r = hit("İİ", "\u{307}i");
        assert_eq!(r[0].2, ["İİ"]);
        assert_eq!(hit("aİb", "\u{307}")[0].2, ["İ"]);
        assert!(hit("İstanbul", "istanbul").is_empty());
        assert_eq!(hit("İstanbul", "i\u{307}stanbul")[0].2, ["İstanbul"]);
    }

    #[test]
    fn long_line_is_cut_around_match() {
        let line = format!("{}needle{}", "ä".repeat(300), "x".repeat(300));
        let r = hit(&line, "NEEDLE");
        let (_, snippet, parts) = &r[0];
        assert_eq!(parts, &["needle"]);
        assert!(snippet.starts_with('…') && snippet.ends_with('…'));
        assert!(snippet.chars().count() <= SNIPPET_CHARS + 2);
    }

    #[test]
    fn limit_and_more() {
        let text = "hit\n".repeat(10);
        let (hits, more) = find_in_text(&text, "hit", 3);
        assert_eq!((hits.len(), more), (3, true));
        let (hits, more) = find_in_text(&text, "hit", 10);
        assert_eq!((hits.len(), more), (10, false));
    }

    #[test]
    fn search_files_skips_non_utf8_and_stops_at_limit() {
        let dir = std::env::temp_dir().join(format!("mm-find-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a.md"), dir.join("b.md"), dir.join("c.md"));
        std::fs::write(&a, "Apple\nno\napple pie").unwrap();
        std::fs::write(&b, b"apple \xff\xfe").unwrap();
        std::fs::write(&c, "apple\n".repeat(LIMIT)).unwrap();
        let paths = vec![a.clone(), b, c.clone(), dir.join("missing.md")];

        let found = search(&paths, None, "  APPLE ", &|| false).unwrap();
        let files: Vec<_> = found.files.iter().map(|f| &f.path).collect();
        assert_eq!(files, [&a, &c]);
        assert_eq!(found.files[0].lines.len(), 2);
        assert_eq!(found.files[0].lines[1].line, 3);
        assert_eq!(found.files[1].lines.len(), LIMIT - 2);
        assert!(found.more);

        assert!(
            search(&paths, None, "", &|| false)
                .unwrap()
                .files
                .is_empty()
        );
        assert!(search(&paths, None, "apple", &|| true).is_none());

        // The open note's unsaved buffer replaces its disk copy.
        let open = (a.clone(), "x\nfresh APPLE".to_string());
        let found = search(&paths[..1], Some(&open), "apple", &|| false).unwrap();
        assert_eq!(found.files[0].lines.len(), 1);
        assert_eq!(found.files[0].lines[0].line, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
