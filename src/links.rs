//! Ctrl+click in the editor: which link is at a position in a line. Pure logic, no GTK; the
//! click handler and what opening does are in `main.rs` (shared with the preview).

use std::ops::Range;

use crate::lists;

/// A link under the pointer.
#[derive(Debug, PartialEq)]
pub enum Target {
    /// Destination as written: `[t](dest)`, `![a](src)`, `<url>` or a bare http(s) URL.
    /// Relative to the note; not checked here.
    Href(String),
    /// `[[name]]` / `[[name|text]]`: the name.
    Wiki(String),
}

/// The link covering character `col` (0-based) of `line`; `above` is the text before the line
/// (to skip fenced code). None outside links and inside code spans or code blocks.
pub fn at(above: &str, line: &str, col: usize) -> Option<Target> {
    if lists::in_fence(above) {
        return None;
    }
    let pos = line.char_indices().nth(col)?.0;
    spans(line)
        .into_iter()
        .find(|(r, _)| r.contains(&pos))
        .map(|(_, t)| t)
}

/// The links of one line (outside code spans), with their byte ranges, in order.
pub fn spans(line: &str) -> Vec<(Range<usize>, Target)> {
    scan(line, &mut Vec::new())
}

/// Whether the wiki link at `range` of `line` (from `spans`) is an embed: `![[name]]`.
pub fn is_embed(line: &str, range: &Range<usize>) -> bool {
    line[..range.start].ends_with('!')
}

/// Byte ranges of `line` covered by a code span, in order.
pub fn code_spans(line: &str) -> Vec<Range<usize>> {
    let mut code = Vec::new();
    scan(line, &mut code);
    code
}

/// Byte ranges of `line` covered by a code span or a link (any kind), in order.
pub fn code_and_links(line: &str) -> Vec<Range<usize>> {
    let mut code = Vec::new();
    let mut all: Vec<Range<usize>> = scan(line, &mut code).into_iter().map(|(r, _)| r).collect();
    all.extend(code);
    all.sort_by_key(|r| r.start);
    all
}

/// The links of `line`; the code spans' ranges go to `code`.
fn scan(line: &str, code: &mut Vec<Range<usize>>) -> Vec<(Range<usize>, Target)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let rest = &line[i..];
        let len = if rest.starts_with('`') {
            // Code span to the closing run of the same length (unclosed: just the run).
            let run = rest.len() - rest.trim_start_matches('`').len();
            let len = rest[run..].find(&rest[..run]).map_or(run, |c| 2 * run + c);
            code.push(i..i + len);
            len
        } else if let Some((len, target)) = wiki(rest)
            .or_else(|| md_link(rest))
            .or_else(|| url(line, i))
        {
            out.push((i..i + len, target));
            len
        } else {
            rest.chars().next().map_or(1, char::len_utf8)
        };
        i += len;
    }
    out
}

/// `[[name|text]]` at the start of `s`: (length, target).
fn wiki(s: &str) -> Option<(usize, Target)> {
    let body = s.strip_prefix("[[")?;
    let end = body.find("]]")?;
    let name = body[..end].split('|').next()?.trim();
    (!name.is_empty()).then(|| (end + 4, Target::Wiki(name.to_string())))
}

/// `[text](dest "title")` or `![alt](src)` at the start of `s`.
// shortcut: no nested brackets/parens in the text or destination; fine for notes.
fn md_link(s: &str) -> Option<(usize, Target)> {
    let open = usize::from(s.starts_with('!'));
    let text = s[open..].strip_prefix('[')?;
    let close = text.find(']')?;
    let dest = text[close + 1..].strip_prefix('(')?;
    let end = dest.find(')')?;
    let raw = dest[..end].trim();
    let href = match raw.strip_prefix('<') {
        Some(r) => r.split('>').next()?,
        None => raw.split_whitespace().next()?,
    };
    let len = open + 1 + close + 2 + end + 1;
    Some((len, Target::Href(href.to_string())))
}

/// `<scheme:…>` autolink or a bare `http(s)://…` URL starting at byte `i` of `line`.
fn url(line: &str, i: usize) -> Option<(usize, Target)> {
    let rest = &line[i..];
    if let Some(inner) = rest.strip_prefix('<') {
        let end = inner.find('>')?;
        let href = &inner[..end];
        let scheme = href.split(':').next()?;
        return (href.contains(':') && !href.contains(char::is_whitespace) && !scheme.is_empty())
            .then(|| (end + 2, Target::Href(href.to_string())));
    }
    let lower = rest.get(..8)?.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    // Only at a word start: not the tail of `xhttp://`.
    if line[..i].chars().last().is_some_and(char::is_alphanumeric) {
        return None;
    }
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '<')
        .unwrap_or(rest.len());
    let href = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '"', '\'']);
    Some((href.len(), Target::Href(href.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn href(s: &str) -> Option<Target> {
        Some(Target::Href(s.into()))
    }

    #[test]
    fn markdown_links_and_images() {
        let line = "see [Ý tưởng](Ideas/Idea.md \"t\") and ![pic](assets/a%20b.png).";
        assert_eq!(at("", line, 4), href("Ideas/Idea.md"));
        assert_eq!(at("", line, 30), href("Ideas/Idea.md"));
        assert_eq!(at("", line, 31), href("Ideas/Idea.md"));
        assert_eq!(at("", line, 32), None);
        assert_eq!(at("", line, 37), href("assets/a%20b.png"));
        assert_eq!(at("", "[a](<my note.md>)", 1), href("my note.md"));
        assert_eq!(at("", "[a](https://x.y/p)", 6), href("https://x.y/p"));
    }

    #[test]
    fn urls() {
        let line = "go https://example.com/a?b=1. or <mailto:a@b.c>";
        assert_eq!(at("", line, 3), href("https://example.com/a?b=1"));
        assert_eq!(at("", line, 27), href("https://example.com/a?b=1"));
        assert_eq!(at("", line, 28), None);
        assert_eq!(at("", line, 40), href("mailto:a@b.c"));
        assert_eq!(at("", "xhttps://a.b", 3), None);
        assert_eq!(at("", "a <b> c", 3), None);
    }

    #[test]
    fn code_and_link_ranges() {
        let line = "a `x` [[W]] [t](u) b";
        assert_eq!(code_and_links(line), [2..5, 6..11, 12..18]);
        assert!(code_and_links("plain").is_empty());
    }

    #[test]
    fn wiki_links() {
        let line = "[[Idea]] and [[Projects/Ideas/Idea|ý tưởng]]";
        assert_eq!(at("", line, 0), Some(Target::Wiki("Idea".into())));
        assert_eq!(at("", line, 7), Some(Target::Wiki("Idea".into())));
        assert_eq!(at("", line, 9), None);
        assert_eq!(
            at("", line, 40),
            Some(Target::Wiki("Projects/Ideas/Idea".into()))
        );
        assert_eq!(at("", "[[]]", 1), None);
    }

    #[test]
    fn outside_and_code() {
        assert_eq!(at("", "plain text", 2), None);
        assert_eq!(at("", "[[a]]", 9), None);
        assert_eq!(at("", "", 0), None);
        let line = "`[[Idea]]` ``https://a.b`` [[Idea]]";
        assert_eq!(at("", line, 3), None);
        assert_eq!(at("", line, 15), None);
        assert_eq!(at("", line, 30), Some(Target::Wiki("Idea".into())));
        assert_eq!(at("", "`x [[a]]", 4), Some(Target::Wiki("a".into())));
        assert_eq!(at("```\n", "[[Idea]]", 2), None);
        assert_eq!(
            at("```\n```\n", "[[Idea]]", 2),
            Some(Target::Wiki("Idea".into()))
        );
    }

    #[test]
    fn embeds() {
        let line = "![[a.png|300]] [[b]] ![x](y.png)";
        let found: Vec<_> = spans(line)
            .into_iter()
            .map(|(r, t)| (is_embed(line, &r), t))
            .collect();
        assert_eq!(
            found,
            [
                (true, Target::Wiki("a.png".into())),
                (false, Target::Wiki("b".into())),
                (false, Target::Href("y.png".into())),
            ]
        );
    }
}
