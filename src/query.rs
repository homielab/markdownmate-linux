//! Find in Folder's search operators. Pure logic, no GTK.
//!
//! A query may hold `tag:<name>` and `path:<text>` words anywhere; `parse` takes them out and
//! the rest is the text searched as before (Match Case, Whole Word and Regex apply to it only).
//!
//! - `tag:work`: notes with the tag `#work` (inline or in the front matter's `tags:`) or a
//!   nested one such as `#work/urgent`; case-insensitive; a leading `#` is allowed.
//! - `path:Archive/`: notes whose path relative to the open folder contains the text,
//!   case-insensitive, `/` as written.
//! - `-tag:` / `-path:` exclude. Several includes of one kind must all hold.
//! - Quotes allow spaces in a value: `path:"Old Notes/"`. Inside the word, `"` opens or closes
//!   a quoted part, as in a shell.
//! - Operator names match in any case (`TAG:x`, `Path:x`).
//! - `tag:` or `path:` without a value is plain text.

use crate::find::fold;

/// The operators of a query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Operators {
    /// Tags every note must have (folded, no `#`, no outer `/`).
    pub tags: Vec<String>,
    /// Tags no note may have.
    pub not_tags: Vec<String>,
    /// Texts every note's relative path must contain (folded).
    pub paths: Vec<String>,
    /// Texts no note's relative path may contain.
    pub not_paths: Vec<String>,
}

/// A query split into its text and its operators.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    /// What is left to search, trimmed.
    pub text: String,
    pub operators: Operators,
}

impl Operators {
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
            && self.not_tags.is_empty()
            && self.paths.is_empty()
            && self.not_paths.is_empty()
    }

    /// Whether the notes' tags are needed (`tags_ok` is always true otherwise).
    pub fn has_tags(&self) -> bool {
        !self.tags.is_empty() || !self.not_tags.is_empty()
    }

    /// Whether a note at `relative` (to the open folder, `/`-separated) passes the `path:`
    /// operators.
    pub fn path_ok(&self, relative: &str) -> bool {
        if self.paths.is_empty() && self.not_paths.is_empty() {
            return true;
        }
        let relative = fold(relative);
        self.paths.iter().all(|p| relative.contains(p.as_str()))
            && !self.not_paths.iter().any(|p| relative.contains(p.as_str()))
    }

    /// Whether a note with `tags` (as `tags::names` gives them: no `#`) passes the `tag:`
    /// operators.
    pub fn tags_ok(&self, tags: &[String]) -> bool {
        if !self.has_tags() {
            return true;
        }
        let tags: Vec<String> = tags.iter().map(|t| fold(t)).collect();
        let has = |wanted: &String| tags.iter().any(|t| tag_matches(t, wanted));
        self.tags.iter().all(has) && !self.not_tags.iter().any(has)
    }
}

/// Whether the (folded) tag `tag` is `wanted` or nested under it: `work`, `work/urgent` for
/// `work`; not `workout`.
pub fn tag_matches(tag: &str, wanted: &str) -> bool {
    let tag = tag.trim_matches('/');
    tag == wanted
        || tag
            .strip_prefix(wanted)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Splits `input` into its operators and the text left (the parts between operators, each
/// trimmed, joined by one space; `input` itself when it has none).
pub fn parse(input: &str) -> Parsed {
    let mut operators = Operators::default();
    // Byte ranges of the operator words, in order.
    let mut taken: Vec<std::ops::Range<usize>> = Vec::new();
    let mut at = 0;
    while at < input.len() {
        let rest = &input[at..];
        let Some(c) = rest.chars().next() else { break };
        if c.is_whitespace() {
            at += c.len_utf8();
            continue;
        }
        let (value, len) = word(rest);
        if let Some((kind, exclude, raw)) = operator(&value) {
            let target = match (kind, exclude) {
                (Kind::Tag, false) => &mut operators.tags,
                (Kind::Tag, true) => &mut operators.not_tags,
                (Kind::Path, false) => &mut operators.paths,
                (Kind::Path, true) => &mut operators.not_paths,
            };
            let normal = match kind {
                Kind::Tag => fold(raw.trim_start_matches('#').trim_matches('/')),
                Kind::Path => fold(raw),
            };
            if !normal.is_empty() {
                target.push(normal);
                taken.push(at..at + len);
            }
        }
        at += len;
    }
    let text = if taken.is_empty() {
        input.trim().to_string()
    } else {
        let mut parts = Vec::new();
        let mut from = 0;
        for r in &taken {
            parts.push(&input[from..r.start]);
            from = r.end;
        }
        parts.push(&input[from..]);
        let parts: Vec<&str> = parts
            .into_iter()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        parts.join(" ")
    };
    Parsed { text, operators }
}

#[derive(Clone, Copy)]
enum Kind {
    Tag,
    Path,
}

/// The word at the start of `text` (up to whitespace outside quotes) with its quotes removed,
/// and its length in bytes. An unclosed quote runs to the end.
fn word(text: &str) -> (String, usize) {
    let mut out = String::new();
    let mut quoted = false;
    for (i, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if c.is_whitespace() && !quoted {
            return (out, i);
        } else {
            out.push(c);
        }
    }
    (out, text.len())
}

/// `word` as an operator: its kind, whether it excludes, and its value (not empty).
fn operator(word: &str) -> Option<(Kind, bool, &str)> {
    let (exclude, rest) = match word.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, word),
    };
    let (kind, value) = if let Some(value) = strip_key(rest, "tag:") {
        (Kind::Tag, value)
    } else {
        (Kind::Path, strip_key(rest, "path:")?)
    };
    (!value.is_empty()).then_some((kind, exclude, value))
}

/// `text` after `key` (ASCII, matched in any case: `TAG:`, `Path:`).
fn strip_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let head = text.get(..key.len())?;
    head.eq_ignore_ascii_case(key).then(|| &text[key.len()..])
}

/// The row shown for a note listed by operators alone: its first non-empty line after the
/// front matter, as (1-based line, line trimmed); line 1 and "" for a note without one.
pub fn first_line(text: &str) -> (usize, &str) {
    let skip = crate::lists::front_matter_lines(text);
    text.lines()
        .enumerate()
        .skip(skip)
        .find(|(_, l)| !l.trim().is_empty())
        .map_or((1, ""), |(n, l)| (n + 1, l.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_text_includes_and_excludes() {
        let parsed = parse("budget tag:work -path:Archive/");
        assert_eq!(parsed.text, "budget");
        assert_eq!(parsed.operators.tags, ["work"]);
        assert_eq!(parsed.operators.not_paths, ["archive/"]);
        assert!(parsed.operators.paths.is_empty() && parsed.operators.not_tags.is_empty());

        let parsed = parse("tag:Work/Urgent q1  plan -tag:#done path:Clients/ report");
        assert_eq!(parsed.text, "q1  plan report");
        assert_eq!(parsed.operators.tags, ["work/urgent"]);
        assert_eq!(parsed.operators.not_tags, ["done"]);
        assert_eq!(parsed.operators.paths, ["clients/"]);
    }

    #[test]
    fn operator_names_match_in_any_case() {
        let parsed = parse("TAG:Work Path:Notes/ -PATH:x -Tag:#done/ plan");
        assert_eq!(parsed.text, "plan");
        assert_eq!(parsed.operators.tags, ["work"]);
        assert_eq!(parsed.operators.paths, ["notes/"]);
        assert_eq!(parsed.operators.not_paths, ["x"]);
        assert_eq!(parsed.operators.not_tags, ["done"]);
        // `tag:#work/` is `work`.
        assert_eq!(parse("tag:#work/").operators.tags, ["work"]);
    }

    #[test]
    fn quotes_allow_spaces() {
        let parsed = parse("path:\"Old Notes/\" plan");
        assert_eq!(parsed.text, "plan");
        assert_eq!(parsed.operators.paths, ["old notes/"]);
        let parsed = parse("-path:\"My Archive\"/2024 tag:\"big project\"");
        assert_eq!(parsed.text, "");
        assert_eq!(parsed.operators.not_paths, ["my archive/2024"]);
        assert_eq!(parsed.operators.tags, ["big project"]);
        // An unclosed quote runs to the end.
        let parsed = parse("x path:\"a b tag:c");
        assert_eq!(parsed.text, "x");
        assert_eq!(parsed.operators.paths, ["a b tag:c"]);
        assert!(parsed.operators.tags.is_empty());
    }

    #[test]
    fn bare_operator_and_other_words_stay_text() {
        let parsed = parse("tag: x");
        assert_eq!(parsed.text, "tag: x");
        assert!(parsed.operators.is_empty());
        for text in ["path:", "-tag:", "Tag:", "tag:\"\"", "mytag:x", "a-path:b"] {
            let parsed = parse(text);
            assert_eq!(parsed.text, text, "{text}");
            assert!(parsed.operators.is_empty(), "{text}");
        }
        // Without operators the text is kept as typed (trimmed), inner spaces included.
        assert_eq!(parse("  a  (b|c) \"q\" ").text, "a  (b|c) \"q\"");
        assert_eq!(parse("").text, "");
    }

    #[test]
    fn nested_tags_match_case_insensitively() {
        let ops = parse("tag:work").operators;
        assert!(ops.tags_ok(&strings(&["work"])));
        assert!(ops.tags_ok(&strings(&["Work/Urgent"])));
        assert!(ops.tags_ok(&strings(&["home", "WORK/a/b"])));
        assert!(!ops.tags_ok(&strings(&["workout"])));
        assert!(!ops.tags_ok(&strings(&["home/work"])));
        assert!(!ops.tags_ok(&[]));

        let ops = parse("tag:work/urgent").operators;
        assert!(ops.tags_ok(&strings(&["work/urgent/today"])));
        assert!(!ops.tags_ok(&strings(&["work"])));

        // Several tags: all needed; -tag: excludes the tag and its children.
        let ops = parse("tag:work tag:q1 -tag:done").operators;
        assert!(ops.tags_ok(&strings(&["work", "q1"])));
        assert!(!ops.tags_ok(&strings(&["work"])));
        assert!(!ops.tags_ok(&strings(&["work", "q1", "done/2024"])));
        assert!(Operators::default().tags_ok(&[]));
    }

    #[test]
    fn paths_match_relative_path_case_insensitively() {
        let ops = parse("-path:Archive/").operators;
        assert!(ops.path_ok("Work/a.md"));
        assert!(!ops.path_ok("archive/a.md"));
        assert!(!ops.path_ok("Work/Archive/a.md"));
        let ops = parse("path:\"Old Notes/\" path:2024").operators;
        assert!(ops.path_ok("Old Notes/2024 plan.md"));
        assert!(!ops.path_ok("Old Notes/plan.md"));
        assert!(!ops.path_ok("OldNotes/2024.md"));
    }

    #[test]
    fn first_line_skips_blank_lines_and_front_matter() {
        assert_eq!(first_line("\n  \n# Title\nbody"), (3, "# Title"));
        assert_eq!(first_line("---\ntags: [a]\n---\n\nHello  "), (5, "Hello"));
        assert_eq!(first_line("\n\n"), (1, ""));
    }
}
