//! Export as RTF: the note as Rich Text Format, built from the comrak AST (as `plain.rs`).
//! Headings bold and larger; bold, italic, strikethrough; `==highlights==` on yellow; inline
//! code and code blocks in a monospace font; links as `HYPERLINK` fields; bullet, numbered and
//! to-do lists indented per level; quotes indented; tables as `\trowd` rows with borders; images as `alt (path)` text;
//! formulas as their TeX; front matter and raw HTML left out. Non-ASCII text is written as
//! `\uN?` (UTF-16 units), so the file is plain ASCII. Pure; `preview::export_rtf` runs it.

use comrak::nodes::{AstNode, ListType, NodeValue};

/// Twips per indent step (a quarter inch).
const STEP: i32 = 360;
/// Width of a table, in twips.
const TABLE_WIDTH: i32 = 9000;
/// Colour table index of `==highlight==`'s background (`render::MARK_COLOUR`).
const HIGHLIGHT: usize = 3 + crate::render::ALERT_COLOURS.len();

/// `md` as an RTF document, with smart punctuation if it is on (`smart`).
pub fn rtf(md: &str) -> String {
    rtf_with(md, crate::smart::enabled())
}

/// `rtf`, with smart punctuation only if `smart`.
fn rtf_with(md: &str, smart: bool) -> String {
    let arena = comrak::Arena::new();
    let mut options = crate::render::options();
    if smart {
        crate::smart::prepare(&mut options);
    }
    let md = crate::render::strip_comments(md);
    let root = comrak::parse_document(&arena, &md, &options);
    crate::blocks::strip(root, &md);
    if smart {
        crate::smart::apply(root);
    }
    crate::render::unmark_multiline(&arena, root);
    let quotes: Vec<_> = root
        .descendants()
        .filter(|n| matches!(n.data().value, NodeValue::BlockQuote))
        .collect();
    for quote in quotes {
        crate::render::make_alert(quote);
    }
    // Colours: 1 link, 2 code shading, then one per alert group (`ALERT_COLOURS` order), then
    // `HIGHLIGHT`.
    let mut colours = String::from("\\red0\\green82\\blue204;\\red240\\green240\\blue240;");
    let alerts = crate::render::ALERT_COLOURS.map(|(_, hex)| hex);
    for hex in alerts.into_iter().chain([crate::render::MARK_COLOUR]) {
        let rgb = u32::from_str_radix(&hex[1..], 16).unwrap_or(0);
        colours.push_str(&format!(
            "\\red{}\\green{}\\blue{};",
            rgb >> 16,
            (rgb >> 8) & 0xff,
            rgb & 0xff
        ));
    }
    let mut out = format!(
        "{{\\rtf1\\ansi\\ansicpg1252\\uc1\\deff0\n\
         {{\\fonttbl{{\\f0\\fswiss Arial;}}{{\\f1\\fmodern Courier New;}}}}\n\
         {{\\colortbl;{colours}}}\n\
         \\f0\\fs24\n",
    );
    blocks(root, 0, &mut None, &mut out);
    out.push('}');
    out.push('\n');
    out
}

/// `text` as RTF text: `\`, `{`, `}` escaped, non-ASCII as `\uN?`, newline as `\line`, tab as `\tab`.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\line "),
            '\t' => out.push_str("\\tab "),
            '\r' => {}
            c if c.is_ascii_control() => {}
            c if c.is_ascii() => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for u in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{}?", *u as i16));
                }
            }
        }
    }
    out
}

/// A link target inside a `HYPERLINK "…"` field.
fn field_url(url: &str) -> String {
    escape(&url.replace('"', "%22").replace(['\n', '\r'], ""))
}

/// The children of `node` as blocks at left indent `indent`; `marker` is the list marker the
/// first block of an item starts with.
fn blocks<'a>(node: &'a AstNode<'a>, indent: i32, marker: &mut Option<String>, out: &mut String) {
    for child in node.children() {
        block(child, indent, marker, out);
    }
}

/// One paragraph: `props` are paragraph control words, `body` its RTF text.
fn paragraph(props: &str, body: &str, out: &mut String) {
    out.push_str(&format!("{{\\pard{props} {body}\\par}}\n"));
}

/// The pending list `marker` as a paragraph of its own (the item starts with a nested block).
fn flush_marker(indent: i32, marker: &mut Option<String>, out: &mut String) {
    if let Some(m) = marker.take() {
        paragraph(&item_props(indent), &format!("{m}\\tab "), out);
    }
}

fn item_props(indent: i32) -> String {
    format!("\\li{indent}\\fi-{STEP}\\tx{indent}\\sa60")
}

fn block<'a>(node: &'a AstNode<'a>, indent: i32, marker: &mut Option<String>, out: &mut String) {
    match &node.data().value {
        NodeValue::FrontMatter(_) | NodeValue::HtmlBlock(_) => {}
        NodeValue::Paragraph | NodeValue::Subtext => {
            let body = inline(node);
            match marker.take() {
                Some(m) => paragraph(&item_props(indent), &format!("{m}\\tab {body}"), out),
                None => paragraph(&format!("\\li{indent}\\sa120"), &body, out),
            }
        }
        NodeValue::Heading(h) => {
            flush_marker(indent, marker, out);
            let size = match h.level {
                1 => 40,
                2 => 34,
                3 => 30,
                4 => 26,
                5 => 24,
                _ => 22,
            };
            paragraph(
                &format!("\\li{indent}\\sb240\\sa120\\keepn"),
                &format!("{{\\b\\fs{size} {}}}", inline(node)),
                out,
            );
        }
        NodeValue::ThematicBreak => {
            flush_marker(indent, marker, out);
            paragraph(
                &format!("\\li{indent}\\brdrb\\brdrs\\brdrw10\\brsp20\\sa120"),
                "",
                out,
            );
        }
        NodeValue::Alert(alert) => {
            flush_marker(indent, marker, out);
            let (kind, title) = crate::render::alert_parts(alert);
            let group = crate::render::alert_group(kind).unwrap_or("note");
            let colour = crate::render::ALERT_COLOURS
                .iter()
                .position(|(g, _)| *g == group)
                .unwrap_or(0);
            let title = escape(title);
            paragraph(
                &format!("\\li{}\\sa60\\keepn", indent + STEP * 2),
                &format!("{{\\b\\cf{} {title}}}", colour + 3),
                out,
            );
            blocks(node, indent + STEP * 2, &mut None, out);
        }
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
            flush_marker(indent, marker, out);
            blocks(node, indent + STEP * 2, &mut None, out);
        }
        NodeValue::CodeBlock(code) => {
            flush_marker(indent, marker, out);
            let literal = code.literal.trim_end_matches('\n');
            let text = if code.info.split_whitespace().next() == Some("math") {
                format!("$$\n{literal}\n$$")
            } else {
                literal.to_string()
            };
            paragraph(
                &format!("\\li{indent}\\cbpat2\\sa120"),
                &format!("{{\\f1\\fs20 {}}}", escape(&text)),
                out,
            );
        }
        NodeValue::List(list) => {
            flush_marker(indent, marker, out);
            let mut number = list.start;
            for item in node.children() {
                let mut m = match list.list_type {
                    ListType::Bullet => "\\u8226?".to_string(),
                    ListType::Ordered => {
                        number += 1;
                        format!("{}.", number - 1)
                    }
                };
                match &item.data().value {
                    NodeValue::TaskItem(task) if task.symbol.is_some() => m.push_str(" [x]"),
                    NodeValue::TaskItem(_) => m.push_str(" [ ]"),
                    _ => {}
                }
                let mut pending = Some(m);
                // Nested blocks sit one step in; the marker hangs in the step before the text.
                blocks(item, indent + STEP, &mut pending, out);
                flush_marker(indent + STEP, &mut pending, out);
            }
        }
        NodeValue::Table(_) => {
            flush_marker(indent, marker, out);
            table(node, indent, out);
        }
        NodeValue::FootnoteDefinition(def) => {
            let mut m = Some(format!("[{}]", escape(&def.name)));
            blocks(node, indent, &mut m, out);
            flush_marker(indent, &mut m, out);
        }
        _ => blocks(node, indent, marker, out),
    }
}

/// A table as `\trowd` rows with thin borders; the header row bold.
fn table<'a>(node: &'a AstNode<'a>, indent: i32, out: &mut String) {
    let columns = node
        .children()
        .map(|r| r.children().count())
        .max()
        .unwrap_or(0)
        .max(1) as i32;
    let width = (TABLE_WIDTH - indent) / columns;
    for row in node.children() {
        let header = matches!(row.data().value, NodeValue::TableRow(true));
        out.push_str(&format!("\\trowd\\trgaph108\\trleft{indent}\n"));
        for i in 1..=columns {
            out.push_str(&format!(
                "\\clbrdrt\\brdrs\\brdrw10\\clbrdrl\\brdrs\\brdrw10\\clbrdrb\\brdrs\\brdrw10\\clbrdrr\\brdrs\\brdrw10{}\\cellx{}\n",
                if header { "\\clcbpat2" } else { "" },
                indent + width * i
            ));
        }
        let mut cells = row.children();
        for _ in 0..columns {
            let body = cells.next().map(inline).unwrap_or_default();
            let body = if header && !body.is_empty() {
                format!("{{\\b {body}}}")
            } else {
                body
            };
            out.push_str(&format!("\\pard\\intbl {body}\\cell\n"));
        }
        out.push_str("\\row\n");
    }
    paragraph("\\sa120", "", out);
}

/// The RTF text of `node`'s inline children.
fn inline<'a>(node: &'a AstNode<'a>) -> String {
    node.children().map(span).collect()
}

fn group(control: &str, body: String) -> String {
    format!("{{{control} {body}}}")
}

fn span<'a>(node: &'a AstNode<'a>) -> String {
    match &node.data().value {
        NodeValue::Text(text) => escape(text),
        NodeValue::Code(code) => group("\\f1\\fs20", escape(&code.literal)),
        NodeValue::SoftBreak | NodeValue::LineBreak => "\\line ".into(),
        NodeValue::HtmlInline(_) => String::new(),
        NodeValue::Strong => group("\\b", inline(node)),
        NodeValue::Emph => group("\\i", inline(node)),
        NodeValue::Strikethrough => group("\\strike", inline(node)),
        // Word and LibreOffice read `\highlight` / `\chcbpat`, Apple's apps `\cb`.
        NodeValue::Highlight => group(
            &format!("\\highlight{HIGHLIGHT}\\chcbpat{HIGHLIGHT}\\cb{HIGHLIGHT}"),
            inline(node),
        ),
        NodeValue::Underline => group("\\ul", inline(node)),
        NodeValue::Superscript => group("\\super", inline(node)),
        NodeValue::Subscript => group("\\sub", inline(node)),
        NodeValue::Math(math) => {
            let d = if math.display_math { "$$" } else { "$" };
            group("\\f1\\fs20", escape(&format!("{d}{}{d}", math.literal)))
        }
        NodeValue::FootnoteReference(r) => group("\\super", format!("[{}]", escape(&r.name))),
        NodeValue::Image(link) => {
            let alt = inline(node);
            match (alt.is_empty(), link.url.is_empty()) {
                (_, true) => alt,
                (true, false) => escape(&link.url),
                (false, false) => format!("{alt} ({})", escape(&link.url)),
            }
        }
        NodeValue::Link(link) => {
            let text = inline(node);
            let url = link.url.as_str();
            if url.is_empty() {
                return text;
            }
            let text = if text.is_empty() { escape(url) } else { text };
            format!(
                "{{\\field{{\\*\\fldinst{{HYPERLINK \"{}\"}}}}{{\\fldrslt{{\\cf1\\ul {text}}}}}}}",
                field_url(url)
            )
        }
        _ => inline(node),
    }
}

#[cfg(test)]
mod tests {
    use super::{escape, rtf, rtf_with};

    #[test]
    fn escapes_specials_and_unicode() {
        assert_eq!(escape(r"a\b{c}"), r"a\\b\{c\}");
        assert_eq!(escape("é"), "\\u233?");
        assert_eq!(escape("Việt"), "Vi\\u7879?t");
        // Beyond the BMP: two UTF-16 units, as signed 16-bit numbers.
        assert_eq!(escape("😀"), "\\u-10179?\\u-8704?");
        assert_eq!(escape("a\nb\tc"), "a\\line b\\tab c");
        assert!(escape("日本語 ✓").is_ascii());
    }

    #[test]
    fn document_is_balanced_ascii() {
        let out =
            rtf("# Title\n\nText with **b**, *i*, ~~s~~ and `c` {x} \\ é.\n\n- a\n  - b\n\n> q\n");
        assert!(out.starts_with("{\\rtf1"));
        assert!(out.is_ascii());
        let (mut depth, mut chars) = (0i32, out.chars());
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            assert!(depth >= 0);
        }
        assert_eq!(depth, 0);
    }

    #[test]
    fn headings_and_emphasis() {
        let out = rtf("# Big\n\n###### Small\n\n**b** *i* ~~s~~ `c`");
        assert!(out.contains("{\\b\\fs40 Big}"));
        assert!(out.contains("{\\b\\fs22 Small}"));
        assert!(out.contains("{\\b b}"));
        assert!(out.contains("{\\i i}"));
        assert!(out.contains("{\\strike s}"));
        assert!(out.contains("{\\f1\\fs20 c}"));
    }

    #[test]
    fn links_and_images() {
        let out = rtf("[site](https://a.b/c?d=1) ![alt](img/x.png)");
        assert!(out.contains("HYPERLINK \"https://a.b/c?d=1\""));
        assert!(out.contains("\\fldrslt{\\cf1\\ul site}"));
        assert!(out.contains("alt (img/x.png)"));
        assert!(!out.contains("HYPERLINK \"img"));
    }

    #[test]
    fn lists_are_indented_per_level() {
        let out = rtf("- one\n  - inner\n3. x\n4. y\n\n- [ ] open\n- [x] done");
        assert!(out.contains("\\li360\\fi-360\\tx360"));
        assert!(out.contains("\\u8226?\\tab one"));
        assert!(out.contains("\\li720\\fi-360\\tx720"));
        assert!(out.contains("3.\\tab x"));
        assert!(out.contains("4.\\tab y"));
        assert!(out.contains("[ ]\\tab open"));
        assert!(out.contains("[x]\\tab done") || out.contains("\\u8226? [x]\\tab done"));
    }

    #[test]
    fn quotes_code_math_tables() {
        let out = rtf(
            "> quoted\n\n```\na {\n  b\n}\n```\n\n$x^2$\n\n| h1 | h2 |\n|----|----|\n| a | b |\n",
        );
        assert!(out.contains("\\li720\\sa120 quoted"));
        assert!(out.contains("{\\f1\\fs20 a \\{\\line   b\\line \\}}"));
        assert!(out.contains("{\\f1\\fs20 $x^2$}"));
        assert_eq!(out.matches("\\trowd").count(), 2);
        assert_eq!(out.matches("\\cell\n").count(), 4);
        assert_eq!(out.matches("\\row\n").count(), 2);
        assert!(out.contains("{\\b h1}"));
    }

    #[test]
    fn alerts_keep_title_and_colour() {
        let out = rtf("> [!WARNING]\n> Mind the gap\n\n> plain quote\n");
        // Colour 6 = warning, #9a6700.
        assert!(out.contains("\\red154\\green103\\blue0;"));
        let table = out.split("\\colortbl;").nth(1).unwrap();
        assert_eq!(table.split(';').nth(5), Some("\\red154\\green103\\blue0"));
        assert!(out.contains("\\li720\\sa60\\keepn {\\b\\cf6 Warning}"));
        assert!(out.contains("\\li720\\sa120 Mind the gap"));
        assert!(!out.contains("[!WARNING]"));
        assert!(out.contains("\\li720\\sa120 plain quote"));
        // An Obsidian callout: its title, its group's colour (14 = danger).
        let out = rtf("> [!error]- Broken pipe\n> body\n");
        assert!(out.contains("{\\b\\cf14 Broken pipe}"), "{out}");
        assert!(rtf("%%hidden%% shown\n").contains("shown"));
        assert!(!rtf("%%hidden%% shown\n").contains("hidden"));
    }

    #[test]
    fn highlights_on_yellow() {
        let out = rtf("A ==marked **bold**== word, a == b, ==two\nlines==.");
        // Colour 18 = `render::MARK_COLOUR`, #fff1a8.
        let table = out.split("\\colortbl;").nth(1).unwrap();
        assert_eq!(
            table.split(';').nth(17),
            Some("\\red255\\green241\\blue168")
        );
        assert!(out.contains("{\\highlight18\\chcbpat18\\cb18 marked {\\b bold}}"));
        assert!(out.contains("a == b, ==two\\line lines==."));
        assert_eq!(out.matches("\\highlight").count(), 1);
    }

    #[test]
    fn smart_punctuation_only_when_on() {
        let md = "\"Quoted\" -- it's `a--b` \\\"x\" https://e.com/a--b's";
        let out = rtf_with(md, true);
        // “Quoted” – it’s, then the code span, the escaped quote and the URL as written.
        assert!(out.contains("\\u8220?Quoted\\u8221? \\u8211? it\\u8217?s"));
        assert!(out.contains("a--b"));
        assert!(out.contains(" \"x\\u8221? "));
        assert!(out.contains("https://e.com/a--b's"));
        assert_eq!(rtf_with(md, false), rtf(md));
        assert!(!rtf_with(md, false).contains("\\u8220?"));
    }

    #[test]
    fn front_matter_and_html_are_dropped() {
        let out = rtf("---\ntitle: x\n---\n\n<div>raw</div>\n\nbody <b>t</b>");
        assert!(!out.contains("title"));
        assert!(!out.contains("raw"));
        assert!(out.contains("body"));
        assert_eq!(rtf("").matches("\\par").count(), 0);
    }
}
