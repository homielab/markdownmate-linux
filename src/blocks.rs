//! Block references, as Obsidian writes them: `^id` at the end of a block names it, so
//! `[[Note#^id]]` links to it and `![[Note#^id]]` embeds it. The id (ASCII letters, digits, `-`)
//! ends the last line of a paragraph, a list item's line or a quote line after one or more
//! spaces, or stands alone on the line right after the block (no blank line between); a table
//! takes it at the end of its last row or alone on the line after it. Alone after exactly one
//! blank line it names the paragraph, list, quote, code block or table above (two or more blank
//! lines: plain text). A list item's id names
//! the item with its children; a quote's (on its last paragraph) the whole quote. Not on a
//! heading line, in code, math or front matter. The first block with an id wins. The marker
//! stays in the editor and is left out of everything rendered (`strip`). Pure, no GTK.

use std::ops::{Range, RangeInclusive};

use comrak::nodes::{AstNode, NodeValue};

/// A block with an id.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Without the `^`.
    pub id: String,
    /// 1-based lines the block spans (a list item with its children).
    pub lines: RangeInclusive<usize>,
    /// 1-based byte column its first line starts at (after a list's or quote's markers).
    pub column: usize,
    /// Where `^id` is written: 0-based line and its byte range in that line.
    pub marker: (usize, Range<usize>),
}

/// The block id a link's fragment names: `^id` → `id` (trimmed); None for a heading.
pub fn block_ref(fragment: &str) -> Option<&str> {
    fragment.trim().strip_prefix('^').map(str::trim)
}

/// Whether `id` is a block id: one or more ASCII letters, digits or `-`.
pub fn is_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The id a source line ends with (`text ^id`, or `^id` alone), and whether it is alone.
fn line_id(line: &str) -> Option<(&str, bool)> {
    let line = line.trim_end();
    let caret = line.rfind('^')?;
    let id = &line[caret + 1..];
    if !is_id(id) {
        return None;
    }
    let before = &line[..caret];
    if before.trim().is_empty() {
        Some((id, true))
    } else {
        before.ends_with([' ', '\t']).then_some((id, false))
    }
}

/// Whether `md` may hold a block id: a line ending in `^` + id characters. Cheap; `scan` and
/// `strip` parse only then.
pub fn maybe(md: &str) -> bool {
    md.contains('^') && md.lines().any(|l| line_id(l).is_some())
}

/// The candidate ids of `md`'s lines, in order (code and headings not left out): enough to tell
/// whether the same id may be written twice.
pub fn written_ids(md: &str) -> Vec<&str> {
    if !md.contains('^') {
        return Vec::new();
    }
    md.lines()
        .filter_map(|l| line_id(l).map(|(id, _)| id))
        .collect()
}

/// What `strip` removes for an id.
enum Cut<'a> {
    /// The `^id` ending this text node (and the line break before it when nothing is left).
    Text(&'a AstNode<'a>),
    /// This node (a table row or paragraph holding only `^id`).
    Node(&'a AstNode<'a>),
    /// Nothing: comrak dropped it (an extra cell of a table row).
    None,
}

/// An id found in a parse.
struct Found<'a> {
    /// The block it names.
    block: &'a AstNode<'a>,
    /// The paragraph (or table) it ends.
    end: &'a AstNode<'a>,
    id: String,
    /// 1-based line it is written on.
    line: usize,
    cut: Cut<'a>,
}

/// Whether `node` is `that`.
fn is<'a>(node: Option<&'a AstNode<'a>>, that: &'a AstNode<'a>) -> bool {
    node.is_some_and(|n| std::ptr::eq(n, that))
}

/// 1-based line `n` of `lines` (empty past the end).
fn line_at<'s>(lines: &[&'s str], n: usize) -> &'s str {
    lines.get(n.wrapping_sub(1)).copied().unwrap_or("")
}

/// The ids of the document `root` parsed from `md`, in order (repeats included).
fn detect<'a>(root: &'a AstNode<'a>, md: &str) -> Vec<Found<'a>> {
    let lines: Vec<&str> = md.split('\n').collect();
    let mut found = Vec::new();
    for node in root.descendants() {
        let paragraph = matches!(node.data().value, NodeValue::Paragraph);
        let table = matches!(node.data().value, NodeValue::Table(_));
        let hit = if paragraph {
            in_paragraph(node, &lines)
        } else if table {
            in_table(node, &lines)
        } else {
            None
        };
        found.extend(hit);
    }
    found
}

/// The id ending paragraph `p`, if any.
fn in_paragraph<'a>(p: &'a AstNode<'a>, lines: &[&str]) -> Option<Found<'a>> {
    let pos = p.data().sourcepos;
    let (id, alone) = line_id(line_at(lines, pos.end.line))?;
    let last = p.last_child()?;
    let text = match &last.data().value {
        NodeValue::Text(t) => t.to_string(),
        _ => return None,
    };
    if !text.trim_end().ends_with(&format!("^{id}")) {
        return None;
    }
    let found = |block, cut| Found {
        block,
        end: p,
        id: id.to_string(),
        line: pos.end.line,
        cut,
    };
    if alone {
        if pos.start.line == pos.end.line {
            // `^id` alone right under a table (comrak may leave it out of the table), or after
            // one blank line under a paragraph, list, quote, code block or table.
            let block = p.previous_sibling().filter(|b| {
                let end = b.data().sourcepos.end.line;
                let gap = end + 2 == pos.start.line && line_at(lines, end + 1).trim().is_empty();
                match b.data().value {
                    NodeValue::Table(_) => gap || end + 1 == pos.start.line,
                    NodeValue::Paragraph
                    | NodeValue::List(_)
                    | NodeValue::BlockQuote
                    | NodeValue::CodeBlock(_) => gap,
                    _ => false,
                }
            })?;
            return Some(Found {
                end: block,
                ..found(block, Cut::Node(p))
            });
        }
        // On its own line: a line break must come before it.
        let before = last.previous_sibling()?;
        let breaks = match &before.data().value {
            NodeValue::SoftBreak | NodeValue::LineBreak => true,
            NodeValue::Text(t) => t.ends_with([' ', '\t']),
            _ => false,
        };
        if !breaks {
            return None;
        }
    }
    let parent = p.parent()?;
    let block = match parent.data().value {
        NodeValue::Item(_) | NodeValue::TaskItem(_) if is(parent.first_child(), p) => parent,
        NodeValue::BlockQuote if is(parent.last_child(), p) => parent,
        _ => p,
    };
    Some(found(block, Cut::Text(last)))
}

/// The id ending table `table`'s last row (or alone on the line after it, when comrak made
/// that line a row), if any.
fn in_table<'a>(table: &'a AstNode<'a>, lines: &[&str]) -> Option<Found<'a>> {
    let row = table.last_child()?;
    if is(table.first_child(), row) {
        return None; // the header row only
    }
    let at = row.data().sourcepos.start.line;
    let (id, alone) = line_id(line_at(lines, at))?;
    let cut = if alone {
        Cut::Node(row)
    } else {
        let cell = row.children().filter(|c| c.first_child().is_some()).last();
        match cell.and_then(|c| c.last_child()) {
            Some(text)
                if matches!(&text.data().value, NodeValue::Text(t)
                    if t.trim_end().ends_with(&format!("^{id}"))) =>
            {
                Cut::Text(text)
            }
            _ => Cut::None,
        }
    };
    Some(Found {
        block: table,
        end: table,
        id: id.to_string(),
        line: at,
        cut,
    })
}

/// The blocks of `md` that have an id, in order; an id written twice names the first block.
pub fn scan(md: &str) -> Vec<Block> {
    if !maybe(md) {
        return Vec::new();
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &crate::render::options());
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out: Vec<Block> = Vec::new();
    for found in detect(root, md) {
        if out.iter().any(|b| b.id == found.id) {
            continue;
        }
        let pos = found.block.data().sourcepos;
        let text = lines.get(found.line - 1).copied().unwrap_or("").trim_end();
        let marker = text.len() - found.id.len() - 1..text.len();
        out.push(Block {
            id: found.id,
            // An id after a blank line is not part of its block's lines.
            lines: if found.line > pos.end.line + 1 {
                pos.start.line..=pos.end.line
            } else {
                pos.start.line..=pos.end.line.max(found.line)
            },
            column: pos.start.column,
            marker: (found.line - 1, marker),
        });
    }
    out
}

/// The block of `md` with id `id`.
pub fn find(md: &str, id: &str) -> Option<Block> {
    if !is_id(id) {
        return None;
    }
    scan(md).into_iter().find(|b| b.id == id)
}

/// The Markdown of block `block` of `md`: its lines, the first from its column, the others
/// without the list or quote markers its first line had before that column.
pub fn source(md: &str, block: &Block) -> String {
    let lines: Vec<&str> = md.split_inclusive('\n').collect();
    let skip = block.column.saturating_sub(1);
    let mut out = String::new();
    for (i, n) in block.lines.clone().enumerate() {
        let Some(text) = lines.get(n - 1) else { break };
        let cut = if i == 0 {
            skip.min(text.len())
        } else {
            text.bytes()
                .take(skip)
                .take_while(|b| matches!(b, b' ' | b'\t' | b'>'))
                .count()
        };
        out.push_str(text.get(cut..).unwrap_or(text));
    }
    out
}

/// `line` as a hover preview shows it: without a ` ^id` at its end; None when it is only `^id`.
pub fn strip_line(line: &str) -> Option<&str> {
    match line_id(line) {
        Some((_, true)) => None,
        Some((id, false)) => {
            Some(line.trim_end()[..line.trim_end().len() - id.len() - 1].trim_end())
        }
        None => Some(line),
    }
}

/// Removes every block id from the document `root` parsed from `md` (before its source lines
/// shift), so nothing shows it. Returns the blocks the ids name, each id once (the first),
/// with the node that ends with it.
pub fn strip<'a>(
    root: &'a AstNode<'a>,
    md: &str,
) -> Vec<(&'a AstNode<'a>, &'a AstNode<'a>, String)> {
    if !maybe(md) {
        return Vec::new();
    }
    let mut named: Vec<(&AstNode, &AstNode, String)> = Vec::new();
    for found in detect(root, md) {
        match found.cut {
            Cut::Node(node) => node.detach(),
            Cut::Text(text) => cut_text(text),
            Cut::None => {}
        }
        if !named.iter().any(|(_, _, id)| *id == found.id) {
            named.push((found.block, found.end, found.id));
        }
    }
    named
}

/// Drops the `^id` ending text node `text`, the spaces before it and, when nothing is left,
/// the node and the line break before it.
fn cut_text(text: &AstNode) {
    let empty = match &mut text.data_mut().value {
        NodeValue::Text(t) => {
            let caret = t.rfind('^').unwrap_or(t.len());
            *t = t[..caret].trim_end().to_string().into();
            t.is_empty()
        }
        _ => false,
    };
    if !empty {
        return;
    }
    let mut before = text.previous_sibling();
    text.detach();
    while let Some(node) = before {
        before = node.previous_sibling();
        let drop = match &mut node.data_mut().value {
            NodeValue::SoftBreak | NodeValue::LineBreak => true,
            NodeValue::Text(t) => {
                *t = t.trim_end().to_string().into();
                if !t.is_empty() {
                    break;
                }
                true
            }
            _ => break,
        };
        if drop {
            node.detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(md: &str) -> Vec<(String, RangeInclusive<usize>)> {
        scan(md).into_iter().map(|b| (b.id, b.lines)).collect()
    }

    fn html(md: &str) -> String {
        crate::render::body_html(
            md,
            &|_: &str| None::<std::path::PathBuf>,
            false,
            false,
            false,
            false,
        )
    }

    #[test]
    fn each_block_kind() {
        // Paragraph, its id at the end or on the next line.
        assert_eq!(ids("Some text ^p1\n"), [("p1".into(), 1..=1)]);
        assert_eq!(ids("one\ntwo\n^p2\n\nnext\n"), [("p2".into(), 1..=3)]);
        // A list item with its children.
        let md = "- a\n- b ^li\n  - child\n  - more\n- c\n";
        assert_eq!(ids(md), [("li".into(), 2..=4)]);
        assert_eq!(ids("1. [ ] task ^t1\n"), [("t1".into(), 1..=1)]);
        // A quote, on its last line or under it.
        assert_eq!(ids("> one\n> two ^q\n\nafter\n"), [("q".into(), 1..=2)]);
        assert_eq!(ids("> one\n^q2\n"), [("q2".into(), 1..=2)]);
        // A table: its last row, or alone on the line after it.
        let table = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        assert_eq!(
            ids(&format!("{}| 3 | 4 ^t\n", table)),
            [("t".into(), 1..=4)]
        );
        assert_eq!(ids(&format!("{table}^t2\n")), [("t2".into(), 1..=4)]);
        assert_eq!(
            ids(&format!("{table}| 3 | 4 | ^t3\n")),
            [("t3".into(), 1..=4)]
        );
        // Where the marker is written.
        assert_eq!(scan("Text ^abc  \n")[0].marker, (0, 5..9));
        assert_eq!(scan("x\n  ^a-1\n")[0].marker, (1, 2..6));
    }

    #[test]
    fn alone_after_one_blank_line() {
        let table = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        for (md, lines) in [
            ("one\ntwo\n\n^x\n".to_string(), 1..=2),
            ("- a\n- b\n  - c\n\n^x\n".to_string(), 1..=3),
            ("1. a\n2. b\n\n^x\n".to_string(), 1..=2),
            ("> one\n> two\n\n^x\n".to_string(), 1..=2),
            ("```\ncode\n```\n\n^x\n".to_string(), 1..=3),
            (format!("{table}\n^x\n"), 1..=3),
        ] {
            assert_eq!(ids(&md), [("x".into(), lines)], "{md}");
            assert!(!html(&md).contains("^x<"), "{md}");
            assert!(html(&md).contains("id=\"^x\" data-block=\"x\""), "{md}");
        }
        assert!(html("- a\n\n^x\n").contains("<ul id=\"^x\""));
        assert!(html("1. a\n\n^x\n").contains("<ol id=\"^x\""));
        assert!(html("```\ncode\n```\n\n^x\n").starts_with("<pre id=\"^x\""));
        // Two blank lines: plain text.
        assert!(ids("text\n\n\n^x\n").is_empty());
        assert!(ids("- a\n\n\n^x\n").is_empty());
        assert!(html("text\n\n\n^x\n").contains("^x"));
        // Under a heading: plain text.
        assert!(ids("# Title\n\n^x\n").is_empty());
    }

    #[test]
    fn hover_lines_without_ids() {
        assert_eq!(strip_line("Some text ^abc  "), Some("Some text"));
        assert_eq!(strip_line("  ^abc"), None);
        assert_eq!(strip_line("x^abc"), Some("x^abc"));
        assert_eq!(strip_line("plain"), Some("plain"));
    }

    #[test]
    fn not_in_code_front_matter_or_headings() {
        for md in [
            "# Heading ^h\n",
            "Setext ^h\n===\n",
            "```\ncode ^c\n```\n",
            "    indented ^c\n",
            "`code ^c`\n",
            "text `code ^c`\n",
            "$x ^c$\n",
            "---\ntitle: x ^f\n---\n\ntext\n",
            "^alone\n",
            "text\n\n\n^after-two-blanks\n",
            "# Heading\n^h\n",
            "```\ncode\n```\n^c\n",
            "text^glued\n",
            "text ^bad_id\n",
            "text ^\n",
            "<div>\nhtml ^h\n</div>\n",
        ] {
            assert_eq!(ids(md), [], "{md:?}");
        }
    }

    #[test]
    fn first_id_wins() {
        let md = "one ^x\n\ntwo ^x\n\nthree ^y\n";
        assert_eq!(ids(md), [("x".into(), 1..=1), ("y".into(), 5..=5)]);
        assert_eq!(find(md, "x").map(|b| b.lines), Some(1..=1));
        assert_eq!(find(md, "z"), None);
        assert_eq!(find(md, "bad id"), None);
        assert_eq!(written_ids(md), ["x", "x", "y"]);
    }

    #[test]
    fn strip_keeps_the_rest_of_the_line() {
        assert_eq!(
            html("Some *text* ^p1\n"),
            "<p id=\"^p1\" data-block=\"p1\">Some <em>text</em></p>\n"
        );
        assert_eq!(
            html("one\n^p2\n"),
            "<p id=\"^p2\" data-block=\"p2\">one</p>\n"
        );
        assert_eq!(
            html("one  \n^p2\n"),
            "<p id=\"^p2\" data-block=\"p2\">one</p>\n"
        );
        assert_eq!(
            html("[[Note]] ^w\n"),
            "<p id=\"^w\" data-block=\"w\"><a href=\"mm-missing:Note\" data-wikilink=\"true\">Note</a></p>\n"
        );
        // Repeats are hidden too, only the first gets the id.
        assert_eq!(
            html("a ^x\n\nb ^x\n"),
            "<p id=\"^x\" data-block=\"x\">a</p>\n<p>b</p>\n"
        );
        // Where it is no id, it stays.
        assert_eq!(html("# H ^h\n"), "<h1 id=\"h-h\">H ^h</h1>\n");
        assert_eq!(html("`x ^c`\n"), "<p><code>x ^c</code></p>\n");
    }

    #[test]
    fn ids_on_list_items_quotes_and_tables() {
        assert_eq!(
            html("- a\n- b ^li\n  - c\n"),
            "<ul>\n<li>a</li>\n<li id=\"^li\" data-block=\"li\">b\n<ul>\n<li>c</li>\n</ul>\n</li>\n</ul>\n"
        );
        assert_eq!(
            html("> q ^q\n"),
            "<blockquote id=\"^q\" data-block=\"q\">\n<p>q</p>\n</blockquote>\n"
        );
        let table = "| a |\n|---|\n| 1 |\n";
        let want = "<table id=\"^t\" data-block=\"t\">\n<thead>\n<tr>\n<th>a</th>\n</tr>\n</thead>\n\
<tbody>\n<tr>\n<td>1</td>\n</tr>\n</tbody>\n</table>\n";
        assert_eq!(html(&format!("{table}^t\n")), want);
        assert_eq!(html("| a |\n|---|\n| 1 ^t\n"), want);
    }

    #[test]
    fn plain_text_and_rich_text_leave_it_out() {
        assert_eq!(
            crate::plain::plain_text("Text ^abc\n\n- item ^i\n"),
            "Text\n\n• item\n"
        );
        assert!(!crate::rtf::rtf("Text ^abc\n").contains("abc"));
    }

    #[test]
    fn source_of_a_block() {
        let md = "intro\n\n- a\n  - b ^li\n    - child\n\n      more\n  - next\n";
        let block = find(md, "li").unwrap();
        assert_eq!(source(md, &block), "- b ^li\n  - child\n\n    more\n");
        let md = "> one\n> two ^q\n";
        assert_eq!(source(md, &find(md, "q").unwrap()), "> one\n> two ^q\n");
    }

    #[test]
    fn link_fragments() {
        assert_eq!(block_ref("^abc"), Some("abc"));
        assert_eq!(block_ref(" ^abc "), Some("abc"));
        assert_eq!(block_ref("^"), Some(""));
        assert_eq!(block_ref("Heading"), None);
    }
}
