//! Find in Folder (Shift+Ctrl+F): full-text search over note files, with Match Case, Whole
//! Word and Regex toggles. Pure Rust (no GTK) so it is testable headless; the window runs `search` on a
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
//! Regex: the query is a `regex` crate pattern (`pattern`): Match Case off adds `(?i)`, Whole
//! Word wraps it in `\b(?:…)\b`. Matched line by line, as plain text is, so a match never spans
//! lines (`^` / `$` are the line's start and end).
//!
//! Replace All (`replace_edits`, `replace_in_file`) replaces exactly the listed matches: every
//! match of each listed line, only in a note whose text is still the one searched (`version`).
//! Plain text inserts the replacement as typed; Regex expands `$1`…`$9`, `${name}` and `$$` as
//! `Regex::replace_all` does. Bytes outside the matches (line endings, a BOM) stay.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::{Path, PathBuf};

use regex::Regex;

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
    pub regex: bool,
}

/// A search query ready to match: plain text (trimmed, `fold`ed without Match Case) or a
/// compiled pattern. Cheap to clone (a `Regex` shares its program).
#[derive(Debug, Clone)]
pub struct Query {
    /// As typed, trimmed.
    pub text: String,
    pub options: Options,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    /// Empty: matches nothing.
    Empty,
    Text(String),
    Pattern(Regex),
}

impl Query {
    /// `query` (trimmed) with `options`; an invalid pattern gives its error message.
    pub fn new(query: &str, options: Options) -> Result<Self, String> {
        let text = query.trim().to_string();
        let kind = if text.is_empty() {
            Kind::Empty
        } else if options.regex {
            Kind::Pattern(compile(&text, options)?)
        } else if options.match_case {
            Kind::Text(text.clone())
        } else {
            Kind::Text(fold(&text))
        };
        Ok(Self {
            text,
            options,
            kind,
        })
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.kind, Kind::Empty)
    }

    /// Byte ranges (in `line`) of the non-overlapping matches, on char boundaries.
    pub fn matches(&self, line: &str) -> Vec<Range<usize>> {
        match &self.kind {
            Kind::Empty => Vec::new(),
            Kind::Text(query) => matches(line, query, self.options),
            Kind::Pattern(re) => re.find_iter(line).map(|m| m.range()).collect(),
        }
    }

    /// What replaces each match of `line`, in order: `replacement` as typed, or for a pattern
    /// with its `$1` / `${name}` / `$$` expanded.
    fn replacements(&self, line: &str, replacement: &str) -> Vec<(Range<usize>, String)> {
        match &self.kind {
            Kind::Pattern(re) => re
                .captures_iter(line)
                .map(|caps| {
                    let mut text = String::new();
                    caps.expand(replacement, &mut text);
                    (caps.get_match().range(), text)
                })
                .collect(),
            _ => self
                .matches(line)
                .into_iter()
                .map(|r| (r, replacement.to_string()))
                .collect(),
        }
    }
}

/// The `regex` crate pattern for a Regex query: `(?i)` without Match Case, `\b(?:…)\b` with
/// Whole Word.
pub fn pattern(query: &str, options: Options) -> String {
    let mut out = String::new();
    if !options.match_case {
        out.push_str("(?i)");
    }
    if options.whole_word {
        out.push_str(&format!("\\b(?:{query})\\b"));
    } else {
        out.push_str(query);
    }
    out
}

/// Compiles `pattern(query, options)`. `query` is checked on its own first, so a stray `)` in it
/// cannot close the Whole Word group and pass. The error is the parser's one-line reason.
fn compile(query: &str, options: Options) -> Result<Regex, String> {
    let short = |e: regex::Error| {
        let text = e.to_string();
        let last = text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let reason = last
            .trim()
            .trim_start_matches("error: ")
            .trim_end_matches('.');
        format!("Invalid pattern: {reason}")
    };
    Regex::new(query).map_err(short)?;
    Regex::new(&pattern(query, options)).map_err(short)
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
pub fn find_in_text(text: &str, query: &Query, limit: usize) -> (Vec<LineHit>, bool) {
    let mut hits = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let ranges = query.matches(line);
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

/// Searches `paths` for `query`. Files that cannot be read or are not UTF-8 are skipped. `open` is the open note's (path, buffer text): it is searched instead of
/// the disk copy so unsaved edits are found. Returns None as soon as `cancelled()` is true.
pub fn search(
    paths: &[PathBuf],
    open: Option<&(PathBuf, String)>,
    query: &Query,
    cancelled: &dyn Fn() -> bool,
) -> Option<Found> {
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
        let (lines, more) = find_in_text(&text, query, left);
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
/// as listed) with `replacement` (`Query::replacements`), in order.
pub fn replace_edits(text: &str, query: &Query, lines: &[usize], replacement: &str) -> Vec<Edit> {
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
            let found = query.replacements(line, replacement).into_iter();
            edits.extend(found.map(|(r, text)| Edit {
                range: start + r.start..start + r.end,
                text,
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
    query: &Query,
    lines: &[usize],
    replacement: &str,
) -> Result<usize, Skipped> {
    let text = read_utf8(path)
        .filter(|text| self::version(text) == version)
        .ok_or(Skipped::Changed)?;
    let edits = replace_edits(&text, query, lines, replacement);
    if edits.is_empty() {
        return Ok(0);
    }
    save::write_atomic(path, &apply(&text, &edits)).map_err(|_| Skipped::NotWritten)?;
    Ok(edits.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str, options: Options) -> Query {
        Query::new(text, options).unwrap()
    }

    fn hit(text: &str, query: &str) -> Vec<(usize, String, Vec<String>)> {
        find_in_text(text, &q(query, Options::default()), LIMIT)
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
        let (hits, _) = find_in_text(text, &q("note", Options::default()), LIMIT);
        assert_eq!(hits[0].count, 6);

        // Whole word only: "note", "Note.", "(note)", "Note" (4 matches); "notes" & "footnote" excluded
        let ww = Options {
            match_case: false,
            whole_word: true,
            regex: false,
        };
        let (hits, _) = find_in_text(text, &q("note", ww), LIMIT);
        assert_eq!(hits[0].count, 4);
        assert_eq!(hits[0].ranges.len(), 4);

        // Match case only: "note", "(note)", "notes", "footnote" (4 matches); "Note.", "Note" excluded
        let mc = Options {
            match_case: true,
            whole_word: false,
            regex: false,
        };
        let (hits, _) = find_in_text(text, &q("note", mc), LIMIT);
        assert_eq!(hits[0].count, 4);

        // Both: "note", "(note)" (2 matches)
        let both = Options {
            match_case: true,
            whole_word: true,
            regex: false,
        };
        let (hits, _) = find_in_text(text, &q("note", both), LIMIT);
        assert_eq!(hits[0].count, 2);
    }

    #[test]
    fn non_ascii_word_boundaries() {
        // café
        let text = "un café. le café et cafés décafé";
        let ww = Options {
            match_case: false,
            whole_word: true,
            regex: false,
        };
        let (hits, _) = find_in_text(text, &q("café", ww), LIMIT);
        assert_eq!(hits[0].count, 2);

        let mc_ww = Options {
            match_case: true,
            whole_word: true,
            regex: false,
        };
        let (hits, _) = find_in_text("Café café", &q("café", mc_ww), LIMIT);
        assert_eq!(hits[0].count, 1);

        // Turkish uppercase 'İ'
        let text_i = "İ İ. İb aİ";
        let ww = Options {
            match_case: false,
            whole_word: true,
            regex: false,
        };
        let (hits, _) = find_in_text(text_i, &q("İ", ww), LIMIT);
        assert_eq!(hits[0].count, 2);

        let mc_ww = Options {
            match_case: true,
            whole_word: true,
            regex: false,
        };
        let (hits, _) = find_in_text(text_i, &q("İ", mc_ww), LIMIT);
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
        let (hits, more) = find_in_text(&text, &q("hit", Options::default()), 3);
        assert_eq!((hits.len(), more), (3, true));
        let (hits, more) = find_in_text(&text, &q("hit", Options::default()), 10);
        assert_eq!((hits.len(), more), (10, false));
    }

    fn replace(text: &str, query: &str, lines: &[usize], with: &str) -> String {
        apply(
            text,
            &replace_edits(text, &q(query, Options::default()), lines, with),
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
        let (hits, _) = find_in_text(text, &q("apple", Options::default()), LIMIT);
        let lines: Vec<usize> = hits.iter().map(|h| h.line).collect();
        let listed: usize = hits.iter().map(|h| h.count).sum();
        assert_eq!(
            replace_edits(text, &q("apple", Options::default()), &lines, "x").len(),
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
            regex: false,
        };
        let (hits, _) = find_in_text(text, &q("note", both), LIMIT);
        let lines: Vec<usize> = hits.iter().map(|h| h.line).collect();
        let listed: usize = hits.iter().map(|h| h.count).sum();
        let edits = replace_edits(text, &q("note", both), &lines, "memo");
        assert_eq!(edits.len(), listed);
        assert_eq!(listed, 2);
        assert_eq!(apply(text, &edits), "memo notes (memo) footnote Note\n");

        let ww = Options {
            match_case: false,
            whole_word: true,
            regex: false,
        };
        let (hits, _) = find_in_text(text, &q("note", ww), LIMIT);
        let lines: Vec<usize> = hits.iter().map(|h| h.line).collect();
        let listed: usize = hits.iter().map(|h| h.count).sum();
        let edits = replace_edits(text, &q("note", ww), &lines, "memo");
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
            &q("OLD", Options::default()),
            &|| false,
        )
        .unwrap();
        let lines = |f: &FileHits| f.lines.iter().map(|h| h.line).collect::<Vec<_>>();
        let (fa, fb) = (&found.files[0], &found.files[1]);
        // b changed after the search: left alone.
        std::fs::write(&b, "old and new\n").unwrap();
        assert_eq!(
            replace_in_file(
                &a,
                fa.version,
                &q("old", Options::default()),
                &lines(fa),
                "new"
            ),
            Ok(2)
        );
        assert_eq!(
            replace_in_file(
                &b,
                fb.version,
                &q("old", Options::default()),
                &lines(fb),
                "new"
            ),
            Err(Skipped::Changed)
        );
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "new one\r\nnew two\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "old and new\n");
        assert_eq!(
            replace_in_file(
                &dir.join("gone.md"),
                0,
                &q("old", Options::default()),
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

        let found = search(&paths, None, &q("  APPLE ", Options::default()), &|| false).unwrap();
        let files: Vec<_> = found.files.iter().map(|f| &f.path).collect();
        assert_eq!(files, [&a, &c]);
        assert_eq!(found.files[0].lines.len(), 2);
        assert_eq!(found.files[0].lines[1].line, 3);
        assert_eq!(found.files[1].lines.len(), LIMIT - 2);
        assert!(found.more);

        assert!(
            search(&paths, None, &q("", Options::default()), &|| false)
                .unwrap()
                .files
                .is_empty()
        );
        assert!(search(&paths, None, &q("apple", Options::default()), &|| true).is_none());

        // The open note's unsaved buffer replaces its disk copy.
        let open = (a.clone(), "x\nfresh APPLE".to_string());
        let found = search(
            &paths[..1],
            Some(&open),
            &q("apple", Options::default()),
            &|| false,
        )
        .unwrap();
        assert_eq!(found.files[0].lines.len(), 1);
        assert_eq!(found.files[0].lines[0].line, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    const RE: Options = Options {
        match_case: false,
        whole_word: false,
        regex: true,
    };

    fn parts(text: &str, query: &Query) -> Vec<String> {
        find_in_text(text, query, LIMIT)
            .0
            .iter()
            .flat_map(|h| h.ranges.iter().map(|r| h.snippet[r.clone()].to_string()))
            .collect()
    }

    #[test]
    fn regex_pattern_follows_the_toggles() {
        assert_eq!(pattern("a+b", RE), "(?i)a+b");
        let mc = Options {
            match_case: true,
            ..RE
        };
        assert_eq!(pattern("a+b", mc), "a+b");
        let ww = Options {
            whole_word: true,
            ..RE
        };
        assert_eq!(pattern("a|b", ww), r"(?i)\b(?:a|b)\b");
        let both = Options {
            match_case: true,
            whole_word: true,
            regex: true,
        };
        assert_eq!(pattern("a|b", both), r"\b(?:a|b)\b");

        let text = "Note-12 note7 notes NOTE-3 footnote-1";
        assert_eq!(
            parts(text, &q(r"note-?\d+", RE)),
            ["Note-12", "note7", "NOTE-3", "note-1"]
        );
        assert_eq!(parts(text, &q(r"note-?\d+", mc)), ["note7", "note-1"]);
        assert_eq!(
            parts(text, &q(r"note-?\d+", ww)),
            ["Note-12", "note7", "NOTE-3"]
        );
        assert_eq!(parts(text, &q(r"note-?\d+", both)), ["note7"]);
        // Alternation stays inside the Whole Word group.
        assert_eq!(parts("cat dog catalog", &q("cat|dog", ww)), ["cat", "dog"]);
        // Unicode: case folding and word boundaries.
        assert_eq!(parts("Tiếng VIỆT việtnam", &q("việt", ww)), ["VIỆT"]);
        // Line by line: `^` / `$` are the line's ends, no match across lines.
        assert_eq!(parts("a b\nb a\n", &q("^b", RE)), ["b"]);
        assert_eq!(parts("a b\r\nb a\n", &q("b$", RE)), ["b"]);
        assert!(parts("one\ntwo", &q(r"one\ntwo", RE)).is_empty());
        // The query is trimmed, like plain text; an empty one matches nothing.
        assert_eq!(q("  x ", RE).text, "x");
        assert!(q("   ", RE).is_empty());
        assert!(
            search(&[], None, &q("", RE), &|| false)
                .unwrap()
                .files
                .is_empty()
        );
    }

    #[test]
    fn invalid_regex_is_an_error() {
        let err = Query::new("(note", RE).unwrap_err();
        assert!(err.starts_with("Invalid pattern: "), "{err}");
        assert!(!err.contains('\n'), "{err}");
        assert!(Query::new("[a-", RE).is_err());
        assert!(Query::new(r"\p{Nope}", RE).is_err());
        // A stray `)` must not close the Whole Word group and pass.
        let ww = Options {
            whole_word: true,
            ..RE
        };
        assert!(Query::new("a)|(b", ww).is_err());
        // The same text is fine as plain text.
        assert!(Query::new("(note", Options::default()).is_ok());
    }

    #[test]
    fn regex_replace_expands_groups() {
        let text = "\u{feff}Date: 2026-11-01 and 2025-01-31\r\nno date\r\nDATE: 1999-12-31\n";
        let query = q(r"(\d{4})-(\d{2})-(\d{2})", RE);
        let (hits, _) = find_in_text(text, &query, LIMIT);
        let lines: Vec<usize> = hits.iter().map(|h| h.line).collect();
        let listed: usize = hits.iter().map(|h| h.count).sum();
        let edits = replace_edits(text, &query, &lines, "$3/$2/${1}");
        assert_eq!(edits.len(), listed);
        assert_eq!(listed, 3);
        assert_eq!(
            apply(text, &edits),
            "\u{feff}Date: 01/11/2026 and 31/01/2025\r\nno date\r\nDATE: 31/12/1999\n"
        );
        // Only the listed lines.
        assert_eq!(
            apply(text, &replace_edits(text, &query, &[3], "x")),
            text.replace("1999-12-31", "x")
        );
        // `$$` is a literal `$`; named groups; a missing group is empty.
        let price = q(r"(?<n>\d+) USD", RE);
        assert_eq!(
            apply(
                "5 USD, 10 usd",
                &replace_edits("5 USD, 10 usd", &price, &[1], "$$${n}$9")
            ),
            "$5, $10"
        );
        // Same result as `Regex::replace_all` on the line.
        let line = "aaa bab b";
        let star = q("a*", RE);
        assert_eq!(
            apply(line, &replace_edits(line, &star, &[1], "<$0>")),
            Regex::new("(?i)a*").unwrap().replace_all(line, "<$0>")
        );
        // Plain text keeps `$1` as typed.
        let plain = q("x", Options::default());
        assert_eq!(apply("x", &replace_edits("x", &plain, &[1], "$1")), "$1");
    }

    #[test]
    fn regex_replace_in_file_only_if_unchanged() {
        let dir = std::env::temp_dir().join(format!("mm-replace-re-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let (a, b) = (dir.join("a.md"), dir.join("b.md"));
        std::fs::write(&a, "- [ ] one\n- [x] two\n").unwrap();
        std::fs::write(&b, "- [ ] three\n").unwrap();
        let query = q(r"^- \[( |x)\] (\w+)", RE);
        let found = search(&[a.clone(), b.clone()], None, &query, &|| false).unwrap();
        let lines = |f: &FileHits| f.lines.iter().map(|h| h.line).collect::<Vec<_>>();
        let (fa, fb) = (&found.files[0], &found.files[1]);
        std::fs::write(&b, "- [ ] three\nmore\n").unwrap();
        assert_eq!(
            replace_in_file(&a, fa.version, &query, &lines(fa), "* $2 [$1]"),
            Ok(2)
        );
        assert_eq!(
            replace_in_file(&b, fb.version, &query, &lines(fb), "* $2"),
            Err(Skipped::Changed)
        );
        assert_eq!(
            std::fs::read_to_string(&a).unwrap(),
            "* one [ ]\n* two [x]\n"
        );
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "- [ ] three\nmore\n");
    }

    /// Times plain vs regex search over a generated folder (`examples/generate_notes.rs`):
    /// `MM_FIND_NOTES=<dir> cargo test --release find::tests::time_regex -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn time_regex() {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "md") {
                    out.push(path);
                }
            }
        }
        let root = PathBuf::from(std::env::var("MM_FIND_NOTES").expect("MM_FIND_NOTES"));
        let mut paths = Vec::new();
        walk(&root, &mut paths);
        let cases = [
            ("plain", "budget", Options::default()),
            ("regex", "budget", RE),
            ("regex", r"bud\w+t", RE),
            // No match: every note is read and searched to the end.
            ("plain", "zzqx", Options::default()),
            ("regex", "zzqx", RE),
            ("regex", r"zz\w*qx", RE),
            ("regex", r"\bzz\d+\b", RE),
            ("regex", r"(meeting|budget) \w+", RE),
        ];
        for (kind, text, options) in cases {
            let query = q(text, options);
            let mut best = std::time::Duration::MAX;
            let mut lines = 0;
            for _ in 0..5 {
                let start = std::time::Instant::now();
                let found = search(&paths, None, &query, &|| false).unwrap();
                best = best.min(start.elapsed());
                lines = found.files.iter().map(|f| f.lines.len()).sum::<usize>();
            }
            println!(
                "{} notes, {kind} {text:?}: {best:?} ({lines} lines)",
                paths.len()
            );
        }
    }
}
