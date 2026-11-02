//! Export as Plain Text: the note without Markdown syntax, built from the comrak AST. Words;
//! headings as their text; bullets as `• ` (numbers kept, to-dos as `[ ]` / `[x]`); links as
//! `text (url)`; images as `alt (url)`; code kept as written; formulas as their TeX between the
//! `$` / `$$` delimiters; `==highlights==` with their `==`; tables as tab-separated rows; quotes
//! with `> `; front matter and raw HTML left out. Pure; `preview::export_text` runs it and writes the file.

use comrak::nodes::{AstNode, ListType, NodeValue};

/// The plain text of `md`, ending with one newline (empty for an empty note).
pub fn plain_text(md: &str) -> String {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &crate::render::options());
    crate::blocks::strip(root, md);
    let text = blocks(root, "\n\n");
    if text.is_empty() { text } else { text + "\n" }
}

/// The children of `node` as blocks, each one rendered and joined by `sep`.
fn blocks<'a>(node: &'a AstNode<'a>, sep: &str) -> String {
    let parts: Vec<String> = node
        .children()
        .map(block)
        .filter(|s| !s.is_empty())
        .collect();
    parts.join(sep)
}

/// Prefix `first` to the first line of `text` and `rest` to every other non-empty line.
fn indent(text: &str, first: &str, rest: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let pre = if i == 0 { first } else { rest };
        if !line.is_empty() || i == 0 {
            out += pre;
        }
        out += line;
    }
    out
}

fn block<'a>(node: &'a AstNode<'a>) -> String {
    match &node.data().value {
        NodeValue::FrontMatter(_) | NodeValue::HtmlBlock(_) => String::new(),
        NodeValue::Paragraph | NodeValue::Heading(_) | NodeValue::Subtext => inline(node),
        NodeValue::ThematicBreak => "---".into(),
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) | NodeValue::Alert(_) => {
            indent(&blocks(node, "\n\n"), "> ", "> ")
        }
        NodeValue::CodeBlock(code) => {
            let literal = code.literal.trim_end_matches('\n');
            if code.info.split_whitespace().next() == Some("math") {
                format!("$$\n{literal}\n$$")
            } else {
                literal.to_string()
            }
        }
        NodeValue::List(list) => {
            let sep = if list.tight { "\n" } else { "\n\n" };
            let mut number = list.start;
            let mut items = vec![];
            for item in node.children() {
                let marker = match list.list_type {
                    ListType::Bullet => "• ".to_string(),
                    ListType::Ordered => {
                        number += 1;
                        format!("{}. ", number - 1)
                    }
                };
                let marker = match &item.data().value {
                    NodeValue::TaskItem(task) if task.symbol.is_some() => marker + "[x] ",
                    NodeValue::TaskItem(_) => marker + "[ ] ",
                    _ => marker,
                };
                let body = blocks(item, sep);
                items.push(indent(&body, &marker, &" ".repeat(marker.chars().count())));
            }
            items.join(sep)
        }
        NodeValue::Table(_) => node
            .children()
            .map(|row| row.children().map(inline).collect::<Vec<_>>().join("\t"))
            .collect::<Vec<_>>()
            .join("\n"),
        NodeValue::FootnoteDefinition(def) => {
            let body = blocks(node, "\n\n");
            indent(&body, &format!("[{}] ", def.name), "    ")
        }
        _ => blocks(node, "\n\n"),
    }
}

/// The text of `node`'s inline children.
fn inline<'a>(node: &'a AstNode<'a>) -> String {
    node.children().map(span).collect()
}

fn span<'a>(node: &'a AstNode<'a>) -> String {
    match &node.data().value {
        NodeValue::Text(text) => text.to_string(),
        NodeValue::Code(code) => code.literal.clone(),
        NodeValue::SoftBreak | NodeValue::LineBreak => "\n".into(),
        NodeValue::HtmlInline(_) => String::new(),
        NodeValue::Math(math) => {
            let d = if math.display_math { "$$" } else { "$" };
            format!("{d}{}{d}", math.literal)
        }
        NodeValue::FootnoteReference(r) => format!("[{}]", r.name),
        // Plain text has no highlight: the `==` stay, as written.
        NodeValue::Highlight => format!("=={}==", inline(node)),
        NodeValue::Link(link) | NodeValue::Image(link) => {
            let text = inline(node);
            let url = link.url.as_str();
            let same = text == url || url.strip_prefix("mailto:") == Some(text.as_str());
            match (text.is_empty(), url.is_empty() || same) {
                (true, _) => url.to_string(),
                (false, true) => text,
                (false, false) => format!("{text} ({url})"),
            }
        }
        _ => inline(node),
    }
}

#[cfg(test)]
mod tests {
    use super::plain_text as p;

    #[test]
    fn highlights_keep_their_marks() {
        assert_eq!(
            p("A ==marked **bold**== word, a == b, `==code==`."),
            "A ==marked bold== word, a == b, ==code==.\n"
        );
    }

    #[test]
    fn words_headings_emphasis_code() {
        assert_eq!(
            p("# Title\n\nSome **bold**, *it*, ~~gone~~ and `code`.\n\n## Sub"),
            "Title\n\nSome bold, it, gone and code.\n\nSub\n"
        );
        assert_eq!(
            p("Title\n=====\n\nline one\nline two"),
            "Title\n\nline one\nline two\n"
        );
        assert_eq!(p(""), "");
    }

    #[test]
    fn lists() {
        assert_eq!(p("- one\n  - inner\n- two"), "• one\n  • inner\n• two\n");
        assert_eq!(p("3. a\n4. b"), "3. a\n4. b\n");
        assert_eq!(p("- [ ] open\n- [x] done"), "• [ ] open\n• [x] done\n");
        assert_eq!(p("- a\n\n  more\n\n- b"), "• a\n\n  more\n\n• b\n");
    }

    #[test]
    fn links_and_images() {
        assert_eq!(
            p("See [docs](https://x.org/a)."),
            "See docs (https://x.org/a).\n"
        );
        assert_eq!(
            p("<https://x.org> and [https://y.org](https://y.org)"),
            "https://x.org and https://y.org\n"
        );
        assert_eq!(
            p("![pic](a.png) [[Note|text]] [[Other]]"),
            "pic (a.png) text Other\n"
        );
    }

    #[test]
    fn code_is_kept() {
        assert_eq!(
            p("```rust\nfn main() {\n    *x* | y\n}\n```"),
            "fn main() {\n    *x* | y\n}\n"
        );
        assert_eq!(p("    indented\n    code"), "indented\ncode\n");
    }

    #[test]
    fn tables_are_tab_separated() {
        assert_eq!(
            p("| A | B |\n| --- | :-: |\n| 1 | 2 |\n| x \\| y | `c` |"),
            "A\tB\n1\t2\nx | y\tc\n"
        );
    }

    #[test]
    fn front_matter_and_html_left_out() {
        assert_eq!(p("---\ntitle: T\n---\n\nBody"), "Body\n");
        assert_eq!(p("a <b>x</b>\n\n<div>\nhidden\n</div>\n\nz"), "a x\n\nz\n");
    }

    #[test]
    fn math_keeps_its_tex() {
        assert_eq!(p("Let $x^2$ be."), "Let $x^2$ be.\n");
        assert_eq!(p("$$\\frac{a}{b}$$"), "$$\\frac{a}{b}$$\n");
        assert_eq!(p("```math\na+b\n```"), "$$\na+b\n$$\n");
        assert_eq!(p("costs $5 and $10"), "costs $5 and $10\n");
    }

    #[test]
    fn quotes_rules_footnotes() {
        assert_eq!(p("> quoted\n> more"), "> quoted\n> more\n");
        assert_eq!(p("a\n\n---\n\nb"), "a\n\n---\n\nb\n");
        assert_eq!(p("Hi[^1]\n\n[^1]: Note."), "Hi[1]\n\n[1] Note.\n");
    }
}
