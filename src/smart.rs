//! Smart punctuation (Preferences ▸ Preview ▸ Smart Punctuation, off by default): the rendered
//! note gets “curly quotes”, en and em dashes and ellipses where its text has straight ones.
//! Only the comrak AST's text changes, so the preview, print, HTML / PDF / RTF export and Copy
//! as HTML / Rich Text follow, while the editor text and the plain-text export never do.
//!
//! Rules, on text nodes only: `---` → —, `--` → –, `...` → …, `"` → “ when the previous
//! character of the paragraph's rendered text is its start, whitespace, `(`, `[`, `{` or a dash,
//! else ”; `'` → ‘ / ’ the same way (so `it's` → it’s). Code spans and blocks, math, raw HTML,
//! front matter and link destinations are other nodes, so they never change; the text of
//! autolinks and bare URLs, tags and backslash-escaped characters stay as written. comrak's own
//! `smart` option is not used: it also changes bare URLs (`a--b's`) and pairs quotes by
//! flanking rules instead.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use comrak::nodes::{AstNode, NodeValue};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Sets the preference, at launch and when it changes. Process-wide like
/// `render::set_hard_breaks`: every renderer (preview, exports, print, copy) reads it.
pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Parse options for a document `apply` will run on: backslash-escaped characters are kept as
/// their own nodes so they stay as written (`apply` merges them back into the text).
pub(crate) fn prepare(options: &mut comrak::Options) {
    options.parse.escaped_char_spans = true;
}

/// Applies the rules to every paragraph, heading and table cell of a document parsed with
/// `prepare`'s options, then turns its escaped characters back into plain text, so the tree
/// has the shape a parse without `prepare` gives.
pub(crate) fn apply<'a>(root: &'a AstNode<'a>) {
    for block in root.descendants() {
        let inline = matches!(
            block.data().value,
            NodeValue::Paragraph | NodeValue::Heading(_) | NodeValue::TableCell
        );
        if inline {
            walk(block, &mut None, false);
        }
    }
    let escaped: Vec<_> = root
        .descendants()
        .filter(|n| matches!(n.data().value, NodeValue::Escaped))
        .collect();
    for node in escaped {
        unwrap_escaped(node);
    }
}

/// Applies the rules to the inlines under `node`; `prev` is the last character of the
/// paragraph's rendered text so far (None at its start), `in_link` whether `node` is inside a
/// link, image or wiki link (no tags there).
fn walk<'a>(node: &'a AstNode<'a>, prev: &mut Option<char>, in_link: bool) {
    for child in node.children() {
        let smart = match &child.data().value {
            NodeValue::Text(text) => {
                let keep = if in_link {
                    Vec::new()
                } else {
                    crate::tags::in_line(text)
                };
                let smart = smarten(text, prev, &keep);
                (smart.as_str() != text.as_ref()).then_some(smart)
            }
            NodeValue::Escaped => {
                *prev = child.collect_text().chars().last().or(*prev);
                None
            }
            NodeValue::Code(code) => {
                *prev = code.literal.chars().last().or(*prev);
                None
            }
            NodeValue::Math(math) => {
                *prev = math.literal.chars().last().or(*prev);
                None
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => {
                *prev = Some('\n');
                None
            }
            NodeValue::FootnoteReference(note) => {
                *prev = note.ix.to_string().chars().last();
                None
            }
            NodeValue::Link(link) if is_autolink(child, &link.url) => {
                *prev = child.collect_text().chars().last().or(*prev);
                None
            }
            NodeValue::Link(_) | NodeValue::Image(_) | NodeValue::WikiLink(_) => {
                walk(child, prev, true);
                None
            }
            // Raw HTML is not rendered text; emphasis, strong, … hold more inlines.
            NodeValue::HtmlInline(_) => None,
            _ => {
                walk(child, prev, in_link);
                None
            }
        };
        if let Some(smart) = smart {
            child.data_mut().value = NodeValue::Text(smart.into());
        }
    }
}

/// Whether `link` shows its own destination: an autolink (`<https://…>`) or a bare URL or
/// email address comrak's autolink extension found.
fn is_autolink<'a>(link: &'a AstNode<'a>, url: &str) -> bool {
    let text = link.collect_text();
    url == text
        || url.strip_prefix("mailto:") == Some(&text)
        || url.strip_prefix("http://") == Some(&text)
}

/// `text` with the rules applied, the byte ranges in `keep` (tags) left as written; `prev`
/// as in `walk`, updated to the result's last character.
fn smarten(text: &str, prev: &mut Option<char>, keep: &[Range<usize>]) -> String {
    fn emit(out: &mut String, prev: &mut Option<char>, c: char) {
        out.push(c);
        *prev = Some(c);
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        if let Some(range) = keep.iter().find(|r| r.start == i) {
            for c in text[range.clone()].chars() {
                emit(&mut out, prev, c);
            }
            i = range.end;
            continue;
        }
        match c {
            // Runs of three first: `---` → —, `...` → …; then `--` → –.
            '-' | '.' => {
                let run = text[i..].len() - text[i..].trim_start_matches(c).len();
                let long = if c == '-' { '—' } else { '…' };
                for _ in 0..run / 3 {
                    emit(&mut out, prev, long);
                }
                match (c, run % 3) {
                    ('-', 2) => emit(&mut out, prev, '–'),
                    (_, rest) => (0..rest).for_each(|_| emit(&mut out, prev, c)),
                }
                i += run;
                continue;
            }
            '"' | '\'' => {
                let opens = prev.is_none_or(|p| {
                    p.is_whitespace() || matches!(p, '(' | '[' | '{' | '-' | '–' | '—')
                });
                let curly = match (c, opens) {
                    ('"', true) => '“',
                    ('"', false) => '”',
                    (_, true) => '‘',
                    (_, false) => '’',
                };
                emit(&mut out, prev, curly);
            }
            _ => emit(&mut out, prev, c),
        }
        i += c.len_utf8();
    }
    out
}

/// Replaces an `Escaped` node by its text, merged with the text right before and after it (as
/// comrak does when it does not keep escapes).
fn unwrap_escaped<'a>(node: &'a AstNode<'a>) {
    let Some(text) = node.first_child() else {
        node.detach();
        return;
    };
    node.insert_before(text);
    node.detach();
    let mut target = text;
    if let Some(before) = target.previous_sibling()
        && merge(before, target)
    {
        target = before;
    }
    if let Some(after) = target.next_sibling() {
        merge(target, after);
    }
}

/// Appends `next`'s text to `node`'s and detaches `next`, if both are text nodes.
fn merge<'a>(node: &'a AstNode<'a>, next: &'a AstNode<'a>) -> bool {
    let (text, end) = match &next.data().value {
        NodeValue::Text(text) => (text.to_string(), next.data().sourcepos.end),
        _ => return false,
    };
    let mut data = node.data_mut();
    let NodeValue::Text(own) = &mut data.value else {
        return false;
    };
    own.to_mut().push_str(&text);
    data.sourcepos.end = end;
    drop(data);
    next.detach();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smart(text: &str) -> String {
        smarten(text, &mut None, &[])
    }

    #[test]
    fn dashes_and_ellipses() {
        assert_eq!(smart("a--b"), "a–b");
        assert_eq!(smart("a---b"), "a—b");
        assert_eq!(smart("a - b"), "a - b");
        assert_eq!(smart("a----b"), "a—-b");
        assert_eq!(smart("a-----b"), "a—–b");
        assert_eq!(smart("wait..."), "wait…");
        assert_eq!(smart("a.b.."), "a.b..");
        assert_eq!(smart("...."), "….");
    }

    #[test]
    fn quotes_open_after_start_space_brackets_and_dashes() {
        assert_eq!(smart("\"quoted\""), "“quoted”");
        assert_eq!(smart("it's"), "it’s");
        assert_eq!(smart("'single'"), "‘single’");
        assert_eq!(smart("(\"a\") [\"b\"] {'c'}"), "(“a”) [“b”] {‘c’}");
        assert_eq!(smart("x--\"y\""), "x–“y”");
        assert_eq!(smart("a\n\"b\""), "a\n“b”");
        assert_eq!(smart("say \"hi\"."), "say “hi”.");
        assert_eq!(smart("rock 'n' roll"), "rock ‘n’ roll");
    }

    #[test]
    fn previous_character_carries_over() {
        let mut prev = Some('a');
        assert_eq!(smarten("\"", &mut prev, &[]), "”");
        assert_eq!(prev, Some('”'));
        let mut prev = Some(' ');
        assert_eq!(smarten("\"", &mut prev, &[]), "“");
        let mut prev = None;
        assert_eq!(smarten("", &mut prev, &[]), "");
        assert_eq!(prev, None);
    }

    #[test]
    fn kept_ranges_stay_as_written() {
        assert_eq!(
            smarten("a #x--y \"z\" #b", &mut None, &[2..7, 12..14]),
            "a #x--y “z” #b"
        );
    }
}
