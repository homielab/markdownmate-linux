//! Find in Folder (Shift+Ctrl+F): full-text search over note files, with Match Case and Whole
//! Word toggles. Pure Rust (no GTK) so it is testable headless; the window runs `search` on a
//! background thread.
//!
//! Match Case searches as-is; without it, Unicode-aware lowercase (char by char, so query and
//! text fold the same way). Lowercasing can change byte lengths (e.g. 'İ' → "i̇"), so match
//! positions are mapped back to whole characters of the original text: a `Range` always lies on
//! char boundaries of the snippet.
//!
//! Whole Word rejects any candidate match that is preceded or followed by a word character
//! (Unicode letter, digit or `_`).
//!
//! Replace All (`replace_edits`, `replace_in_file`) replaces exactly the listed matches: every
//! match of each listed line, the replacement inserted as typed, only in a note whose text is
//! still the one searched (`version`). Bytes outside the matches (line endings, a BOM) stay.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::relink::{Edit, apply};
use crate::save;

/// Most matching lines returned over all files.
pub const LIMIT: usize = 500;
/// Snippets longer than this (in chars) are cut around the first match.
const SNIPPET_CHARS: usize = 160;
/// Chars kept before the first match when a snippet is cut.
const LEAD_CHARS: usize = 40;

/// Search toggles for Find in Folder and Replace All.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub match_case: bool,
    pub whole_word: bool,
}

/// A Unicode letter, digit or `_` (used for the Whole Word boundary check).
pub fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// One matching line.
#[derive(Debug, PartialEq)]
pub struct LineHit {
    /// 1-based line number.
    pub line: usize,
    /// The line (or a window of it, with "…" where cut).
    pub snippet: String,
    /// Byte ranges of the matches within `snippet`.
    pub ranges: Vec<Range<usize>>,
    /// How many matches the whole line has (a cut snippet may show fewer).
    pub count: usize,
}

#[derive(Debug)]
pub struct FileHits {
    pub path: PathBuf,
    pub lines: Vec<LineHit>,
    /// `version` of the text searched.
    pub version: u64,
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

/// Byte ranges (in `line`) of the non-overlapping matches of `query` according to `options`.
/// If `!options.match_case`, `query` is expected to be already `fold`ed.
pub fn matches(line: &str, query: &str, options: Options) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    if options.match_case {
        let mut out = Vec::new();
        let mut search_start = 0;
        while search_start < line.len() {
            let Some(rel_start) = line[search_start..].find(query) else {
                break;
            };
            let start = search_start + rel_start;
            let end = start + query.len();
            if options.whole_word {
                let before = line[..start].chars().next_back();
                let after = line[end..].chars().next();
                if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
                    let next_char_len = line[start..].chars().next().map_or(1, char::len_utf8);
                    search_start = start + next_char_len;
                    continue;
                }
            }
            out.push(start..end);
            search_start = end;
        }
        out
    } else {
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
        let mut search_start = 0;
        while search_start < folded.len() {
            let Some(rel_start) = folded[search_start..].find(query) else {
                break;
            };
            let at = search_start + rel_start;
            let m_len = query.len();
            let start = origin[at].start;
            let end = origin[at + m_len - 1].end;
            if options.whole_word {
                let before = line[..start].chars().next_back();
                let after = line[end..].chars().next();
                if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
                    let next_char_len = folded[at..].chars().next().map_or(1, char::len_utf8);
                    search_start = at + next_char_len;
                    continue;
                }
            }
            // Two matches inside one expanded char map to the same original char: merge.
            match out.last_mut() {
                Some(last) if start < last.end => last.end = last.end.max(end),
                _ => out.push(start..end),
            }
            search_start = at + m_len;
        }
        out
    }
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
/// `query` is expected to be already prepared (folded if `!options.match_case`, non-empty).
pub fn find_in_text(
    text: &str,
    query: &str,
    options: Options,
    limit: usize,
) -> (Vec<LineHit>, bool) {
    let mut hits = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let ranges = matches(line, query, options);
        if ranges.is_empty() {
            continue;
        }
        if hits.len() == limit {
            return (hits, true);
        }
        let count = ranges.len();
        let (snippet, ranges) = snippet(line, ranges);
        hits.push(LineHit {
            line: n + 1,
            snippet,
            ranges,
            count,
        });
    }
    (hits, false)
}

/// Searches `paths` for `query` (trimmed, according to `options`). Files that cannot be read or are
/// not UTF-8 are skipped. `open` is the open note's (path, buffer text): it is searched instead of
/// the disk copy so unsaved edits are found. Returns None as soon as `cancelled()` is true.
pub fn search(
    paths: &[PathBuf],
    open: Option<&(PathBuf, String)>,
    query: &str,
    options: Options,
    cancelled: &dyn Fn() -> bool,
) -> Option<Found> {
    let query = if options.match_case {
        query.trim().to_string()
    } else {
        fold(query.trim())
    };
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
        let (lines, more) = find_in_text(&text, &query, options, left);
        left -= lines.len();
        if !lines.is_empty() {
            found.files.push(FileHits {
                path: path.clone(),
                lines,
                version: version(&text),
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

/// A fingerprint of a note's text: equal versions = the text the search saw.
pub fn version(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// The edits replacing every match of `query` on the 1-based `lines` of `text` (ascending,
/// as listed) with `replacement`, in order, following `options`.
pub fn replace_edits(
    text: &str,
    query: &str,
    options: Options,
    lines: &[usize],
    replacement: &str,
) -> Vec<Edit> {
    let query = if options.match_case {
        query.trim().to_string()
    } else {
        fold(query.trim())
    };
    if query.is_empty() {
        return Vec::new();
    }
    let mut edits = Vec::new();
    let mut wanted = lines.iter().copied().peekable();
    let mut start = 0;
    for (n, line) in text.split('\n').enumerate() {
        let number = n + 1;
        while wanted.next_if(|&l| l < number).is_some() {}
        if wanted.peek().is_none() {
            break;
        }
        if wanted.next_if_eq(&number).is_some() {
            // As `str::lines`: a line's `\r` before the `\n` is not part of it.
            let line = line.strip_suffix('\r').unwrap_or(line);
            edits.extend(matches(line, &query, options).into_iter().map(|r| Edit {
                range: start + r.start..start + r.end,
                text: replacement.to_string(),
            }));
        }
        start += line.len() + 1;
    }
    edits
}

/// Why `replace_in_file` left a note alone.
#[derive(Debug, PartialEq, Eq)]
pub enum Skipped {
    /// Unreadable, or its text is no longer the one searched.
    Changed,
    /// Writing failed.
    NotWritten,
}

/// Replace All in a closed note: `replace_edits` made and written atomically, only if the note
/// on disk is still the text searched (`version`). Returns how many matches were replaced.
pub fn replace_in_file(
    path: &Path,
    version: u64,
    query: &str,
    options: Options,
    lines: &[usize],
    replacement: &str,
) -> Result<usize, Skipped> {
    let text = read_utf8(path)
        .filter(|text| self::version(text) == version)
        .ok_or(Skipped::Changed)?;
    let edits = replace_edits(&text, query, options, lines, replacement);
    if edits.is_empty() {
        return Ok(0);
    }
    save::write_atomic(path, &apply(&text, &edits)).map_err(|_| Skipped::NotWritten)?;
    Ok(edits.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(text: &str, query: &str) -> Vec<(usize, String, Vec<String>)> {
        find_in_text(text, &fold(query), Options::default(), LIMIT)
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
    fn whole_word_and_match_case_options() {
        let text = "note Note. (note) notes footnote Note\n";

        // Default: case-insensitive, partial word
        let (hits, _) = find_in_text(text, &fold("note"), Options::default(), LIMIT);
        assert_eq!(hits[0].count, 6);

        // Whole word only: "note", "Note.", "(note)", "Note" (4 matches); "notes" & "footnote" excluded
        let ww = Options {
            match_case: false,
            whole_word: true,
        };
        let (hits, _) = find_in_text(text, &fold("note"), ww, LIMIT);
        assert_eq!(hits[0].count, 4);
        assert_eq!(hits[0].ranges.len(), 4);

        // Match case only: "note", "(note)", "notes", "footnote" (4 matches); "Note.", "Note" excluded
        let mc = Options {
            match_case: true,
            whole_word: false,
        };
        let (hits, _) = find_in_text(text, "note", mc, LIMIT);
        assert_eq!(hits[0].count, 4);

        // Both: "note", "(note)" (2 matches)
        let both = Options {
            match_case: true,
            whole_word: true,
        };
        let (hits, _) = find_in_text(text, "note", both, LIMIT);
        assert_eq!(hits[0].count, 2);
    }

    #[test]
    fn non_ascii_word_boundaries() {
        // café
        let text = "un café. le café et cafés décafé";
        let ww = Options {
            match_case: false,
            whole_word: true,
        };
        let (hits, _) = find_in_text(text, &fold("café"), ww, LIMIT);
        assert_eq!(hits[0].count, 2);

        let mc_ww = Options {
            match_case: true,
            whole_word: true,
        };
        let (hits, _) = find_in_text("Café café", "café", mc_ww, LIMIT);
        assert_eq!(hits[0].count, 1);

        // Turkish uppercase 'İ'
        let text_i = "İ İ. İb aİ";
        let ww = Options {
            match_case: false,
            whole_word: true,
        };
        let (hits, _) = find_in_text(text_i, &fold("İ"), ww, LIMIT);
        assert_eq!(hits[0].count, 2);

        let mc_ww = Options {
            match_case: true,
            whole_word: true,
        };
        let (hits, _) = find_in_text(text_i, "İ", mc_ww, LIMIT);
        assert_eq!(hits[0].count, 2);
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
        let (hits, more) = find_in_text(&text, "hit", Options::default(), 3);
        assert_eq!((hits.len(), more), (3, true));
        let (hits, more) = find_in_text(&text, "hit", Options::default(), 10);
        assert_eq!((hits.len(), more), (10, false));
    }

    fn replace(text: &str, query: &str, lines: &[usize], with: &str) -> String {
        apply(
            text,
            &replace_edits(text, query, Options::default(), lines, with),
        )
    }

    #[test]
    fn replaces_the_listed_lines_only() {
        let text = "\u{feff}Apple apple\r\nno\r\nAPPLE pie\r\napple\n";
        assert_eq!(
            replace(text, "apple", &[1, 3], "Pear"),
            "\u{feff}Pear Pear\r\nno\r\nPear pie\r\napple\n"
        );
        assert_eq!(
            replace(text, "apple", &[4], ""),
            text.replace("\r\napple\n", "\r\n\n")
        );
        assert_eq!(replace(text, "apple", &[], "x"), text);
        // Accents and case fold as in the search; the replacement is inserted as typed.
        assert_eq!(
            replace("Tiếng VIỆT và việt\nxİy", "việt", &[1], "Việt Nam"),
            "Tiếng Việt Nam và Việt Nam\nxİy"
        );
        assert_eq!(replace("xİy", "i", &[1], "I"), "xIy");
        // The counts the search lists are the matches replaced.
        let (hits, _) = find_in_text(text, &fold("apple"), Options::default(), LIMIT);
        let lines: Vec<usize> = hits.iter().map(|h| h.line).collect();
        let listed: usize = hits.iter().map(|h| h.count).sum();
        assert_eq!(
            replace_edits(text, "apple", Options::default(), &lines, "x").len(),
            listed
        );
        assert_eq!(listed, 4);
    }

    #[test]
    fn replace_all_follows_options() {
        let text = "note notes (note) footnote Note\n";
        let both = Options {
            match_case: true,
            whole_word: true,
        };
        let (hits, _) = find_in_text(text, "note", both, LIMIT);
        let lines: Vec<usize> = hits.iter().map(|h| h.line).collect();
        let listed: usize = hits.iter().map(|h| h.count).sum();
        let edits = replace_edits(text, "note", both, &lines, "memo");
        assert_eq!(edits.len(), listed);
        assert_eq!(listed, 2);
        assert_eq!(apply(text, &edits), "memo notes (memo) footnote Note\n");

        let ww = Options {
            match_case: false,
            whole_word: true,
        };
        let (hits, _) = find_in_text(text, &fold("note"), ww, LIMIT);
        let lines: Vec<usize> = hits.iter().map(|h| h.line).collect();
        let listed: usize = hits.iter().map(|h| h.count).sum();
        let edits = replace_edits(text, "note", ww, &lines, "memo");
        assert_eq!(edits.len(), listed);
        assert_eq!(listed, 3);
        assert_eq!(apply(text, &edits), "memo notes (memo) footnote memo\n");
    }

    #[test]
    fn replace_in_file_only_if_unchanged() {
        let dir = std::env::temp_dir().join(format!("mm-replace-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let (a, b) = (dir.join("a.md"), dir.join("b.md"));
        std::fs::write(&a, "old one\r\nold two\n").unwrap();
        std::fs::write(&b, "old\n").unwrap();
        let found = search(
            &[a.clone(), b.clone()],
            None,
            "OLD",
            Options::default(),
            &|| false,
        )
        .unwrap();
        let lines = |f: &FileHits| f.lines.iter().map(|h| h.line).collect::<Vec<_>>();
        let (fa, fb) = (&found.files[0], &found.files[1]);
        // b changed after the search: left alone.
        std::fs::write(&b, "old and new\n").unwrap();
        assert_eq!(
            replace_in_file(&a, fa.version, "old", Options::default(), &lines(fa), "new"),
            Ok(2)
        );
        assert_eq!(
            replace_in_file(&b, fb.version, "old", Options::default(), &lines(fb), "new"),
            Err(Skipped::Changed)
        );
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "new one\r\nnew two\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "old and new\n");
        assert_eq!(
            replace_in_file(
                &dir.join("gone.md"),
                0,
                "old",
                Options::default(),
                &[1],
                "new"
            ),
            Err(Skipped::Changed)
        );
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

        let found = search(&paths, None, "  APPLE ", Options::default(), &|| false).unwrap();
        let files: Vec<_> = found.files.iter().map(|f| &f.path).collect();
        assert_eq!(files, [&a, &c]);
        assert_eq!(found.files[0].lines.len(), 2);
        assert_eq!(found.files[0].lines[1].line, 3);
        assert_eq!(found.files[1].lines.len(), LIMIT - 2);
        assert!(found.more);

        assert!(
            search(&paths, None, "", Options::default(), &|| false)
                .unwrap()
                .files
                .is_empty()
        );
        assert!(search(&paths, None, "apple", Options::default(), &|| true).is_none());

        // The open note's unsaved buffer replaces its disk copy.
        let open = (a.clone(), "x\nfresh APPLE".to_string());
        let found = search(
            &paths[..1],
            Some(&open),
            "apple",
            Options::default(),
            &|| false,
        )
        .unwrap();
        assert_eq!(found.files[0].lines.len(), 1);
        assert_eq!(found.files[0].lines[0].line, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
