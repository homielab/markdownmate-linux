//! Rich paste: HTML from the clipboard (`text/html`, browser / LibreOffice) → Markdown (`htmd`).

/// Tags that carry Markdown meaning. HTML without any of them (a paragraph from gedit, a `<pre>`
/// from GNOME Terminal, …) is pasted as its `text/plain`, so nothing gets escaped needlessly.
const RICH: &[&str] = &[
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "a",
    "ul",
    "ol",
    "li",
    "b",
    "strong",
    "i",
    "em",
    "code",
    "blockquote",
    "table",
    "img",
    "hr",
];

/// Markdown for clipboard HTML; None: paste the plain text instead (no Markdown-bearing tag,
/// empty result or a parse error).
pub fn html_to_markdown(html: &str) -> Option<String> {
    if !has_rich_tag(html) {
        return None;
    }
    use htmd::options::{BulletListMarker, HrStyle, Options};
    // `- item` / `1. item` / `---`, as the smart lists type them.
    let options = Options {
        bullet_list_marker: BulletListMarker::Dash,
        ul_bullet_spacing: 1,
        ol_number_spacing: 1,
        hr_style: HrStyle::Dashes,
        ..Options::default()
    };
    let converter = htmd::HtmlToMarkdown::builder()
        .options(options)
        .skip_tags(vec!["script", "style", "head", "noscript", "template"])
        // Only web images become `![](url)`; `data:`/relative sources would be huge or broken.
        .add_handler(
            vec!["img"],
            |handlers: &dyn htmd::element_handler::Handlers, element: htmd::Element| {
                let web = element.attrs.iter().any(|a| {
                    &*a.name.local == "src"
                        && (a.value.starts_with("http://") || a.value.starts_with("https://"))
                });
                if web {
                    handlers.fallback(element)
                } else {
                    None
                }
            },
        )
        .build();
    let md = converter.convert(html).ok()?;
    let md = md.trim();
    (!md.is_empty()).then(|| drop_needless_escapes(md))
}

/// Spreadsheet cells (tab-separated plain text) as an aligned Markdown table: at least two
/// rows with the same number (at least two) of cells; the first row is the header and `|` is
/// escaped. None for anything else, which is then pasted as it is.
pub fn tsv_to_table(text: &str) -> Option<String> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let rows: Vec<Vec<String>> = text
        .split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            line.split('\t')
                .map(|cell| cell.trim().replace('|', "\\|"))
                .collect()
        })
        .collect();
    let n = rows.first()?.len();
    (rows.len() >= 2 && n >= 2 && rows.iter().all(|r| r.len() == n))
        .then(|| crate::table::from_rows(&rows))
}

/// Trial renders above this many escapes would be slow on a big clipboard; then only
/// intraword `_` (never emphasis in CommonMark) is unescaped.
const MAX_TRIALS: usize = 64;

/// htmd escapes every `*`, `_`, `[`, … in text (`snake\_case`, `a\*b`). Drop each `\` whose
/// removal leaves the rendered HTML unchanged, so the note reads like the source; escapes
/// that matter (literal `\*x\*`, code spans, …) are kept because removing them would not.
fn drop_needless_escapes(md: &str) -> String {
    let html = |s: &str| crate::render::body_html(s, &|_| None, false, false, false);
    let base = html(md);
    // Byte offsets of `\` before ASCII punctuation; `\\` (a literal backslash) is skipped.
    let mut escapes = Vec::new();
    let mut bytes = md.bytes().enumerate();
    while let Some((i, b)) = bytes.next() {
        if b == b'\\'
            && let Some((_, next)) = bytes.next()
            && next.is_ascii_punctuation()
            && next != b'\\'
        {
            escapes.push(i);
        }
    }
    let without = |drop: &[usize]| {
        let mut out = md.to_string();
        for &i in drop.iter().rev() {
            out.remove(i);
        }
        out
    };
    let all = without(&escapes);
    if html(&all) == base {
        return all;
    }
    if escapes.len() > MAX_TRIALS {
        let alnum = |i: usize| md.as_bytes().get(i).is_some_and(u8::is_ascii_alphanumeric);
        escapes.retain(|&i| md.as_bytes()[i + 1] == b'_' && i > 0 && alnum(i - 1) && alnum(i + 2));
        let intraword = without(&escapes);
        return if html(&intraword) == base {
            intraword
        } else {
            md.to_string()
        };
    }
    // Greedy: each escape is tried on top of the ones already dropped.
    let mut dropped = Vec::new();
    for i in escapes {
        dropped.push(i);
        if html(&without(&dropped)) != base {
            dropped.pop();
        }
    }
    without(&dropped)
}

/// Clipboard HTML bytes as text: UTF-16LE with a BOM (some Firefox versions), else UTF-8.
pub fn decode(bytes: &[u8]) -> String {
    match bytes.strip_prefix(&[0xFF, 0xFE]) {
        Some(utf16) => {
            let units: Vec<u16> = utf16
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .collect();
            String::from_utf16_lossy(&units)
        }
        None => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Some `<tag` in `html` is one of `RICH` (a cheap scan: a match inside a script only means
/// the converter runs and drops the script).
fn has_rich_tag(html: &str) -> bool {
    html.split('<').skip(1).any(|s| {
        let name: String = s
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase();
        RICH.contains(&name.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(html: &str) -> String {
        html_to_markdown(html).unwrap()
    }

    #[test]
    fn headings_emphasis_links() {
        assert_eq!(
            md("<h1>Title</h1><p>Some <b>bold</b> and <em>it</em></p>"),
            "# Title\n\nSome **bold** and *it*"
        );
        assert_eq!(md("<h2>Sub</h2>"), "## Sub");
        assert_eq!(
            md(r#"<p>See <a href="https://x.org/a">docs</a>.</p>"#),
            "See [docs](https://x.org/a)."
        );
    }

    #[test]
    fn nested_lists() {
        let html = "<ul><li>one<ul><li>inner</li></ul></li><li>two</li></ul>";
        assert_eq!(md(html), "- one\n  - inner\n- two");
        assert_eq!(md("<ol><li>a</li><li>b</li></ol>"), "1. a\n2. b");
    }

    #[test]
    fn code_quote_table() {
        assert_eq!(md("<p>Run <code>ls -a</code></p>"), "Run `ls -a`");
        assert_eq!(
            md("<pre><code>fn main() {}\n</code></pre>"),
            "```\nfn main() {}\n```"
        );
        assert_eq!(md("<blockquote><p>quoted</p></blockquote>"), "> quoted");
        let table = md("<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>");
        assert!(table.contains("| A") && table.contains("| 1"), "{table}");
    }

    #[test]
    fn drops_script_style_attributes_and_local_images() {
        let html = r#"<html><head><style>p{color:red}</style><title>T</title></head><body>
            <script>alert(1)</script><p class="x" style="color:red"><strong>hi</strong></p>
            <img src="data:image/png;base64,AAAA"><img src="https://x.org/a.png" alt="pic"></body></html>"#;
        assert_eq!(md(html), "**hi**\n\n![pic](https://x.org/a.png)");
    }

    #[test]
    fn tsv_becomes_a_table() {
        assert_eq!(
            tsv_to_table("Name\tQty\nApple\t3\nFig\t12\n").unwrap(),
            "| Name  | Qty |\n| ----- | --- |\n| Apple | 3   |\n| Fig   | 12  |"
        );
        // CRLF, empty cells, padding trimmed.
        assert_eq!(
            tsv_to_table("a\tb\r\n 1 \t\r\n").unwrap(),
            "| a   | b   |\n| --- | --- |\n| 1   |     |"
        );
    }

    #[test]
    fn tsv_escapes_pipes_and_aligns_wide_chars() {
        let t = tsv_to_table("x|y\tz\n日本\tw").unwrap();
        assert_eq!(t, "| x\\|y | z   |\n| ---- | --- |\n| 日本 | w   |");
    }

    #[test]
    fn tsv_other_text_is_left_alone() {
        assert_eq!(tsv_to_table("a\tb\nc"), None); // ragged
        assert_eq!(tsv_to_table("a\tb\nc\td\te"), None);
        assert_eq!(tsv_to_table("a\nb\nc"), None); // one column
        assert_eq!(tsv_to_table("a\tb\tc"), None); // one row
        assert_eq!(tsv_to_table("a\tb\tc\n"), None);
        assert_eq!(tsv_to_table("plain text"), None);
        assert_eq!(tsv_to_table(""), None);
    }

    #[test]
    fn decodes_utf8_and_utf16() {
        assert_eq!(decode("<b>é</b>".as_bytes()), "<b>é</b>");
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("<b>é</b>".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(decode(&utf16), "<b>é</b>");
    }

    #[test]
    fn drops_needless_escapes() {
        let html = "<p><b>Note</b>: snake_case, a*b, file_name.md, [x]</p>";
        assert_eq!(md(html), "**Note**: snake_case, a*b, file_name.md, [x]");
        // Real emphasis stays emphasis; literal asterisks keep the one escape that stops them
        // becoming one (a lone `*` cannot pair).
        assert_eq!(
            md("<p><em>nhấn</em> and *literal*</p>"),
            "*nhấn* and *literal\\*"
        );
        assert_eq!(md("<p><b>x</b> _a_ [y](z)</p>"), "**x** _a\\_ [y\\](z)");
        // Over MAX_TRIALS escapes with one that matters: only intraword `_` is unescaped.
        let many = format!("<p><b>x</b> *k* {}</p>", "a_b ".repeat(70));
        let out = md(&many);
        assert!(out.starts_with("**x** \\*k\\* a_b a_b"), "{out}");
    }

    #[test]
    fn plain_text_html_falls_back() {
        // gedit / GNOME Terminal style: only wrappers, so the plain text is used (no `\*`, `\_`).
        assert_eq!(html_to_markdown("<p>snake_case 2*3 [x]</p>"), None);
        assert_eq!(
            html_to_markdown(r#"<pre><span style="color:#fff">$ ls *_x</span></pre>"#),
            None
        );
        assert_eq!(
            html_to_markdown("<meta charset=utf-8><div><span>a</span><br>b</div>"),
            None
        );
        assert_eq!(html_to_markdown("<b></b>"), None);
        assert_eq!(html_to_markdown(""), None);
        assert!(!has_rich_tag("<abbr>x</abbr><bdi>y</bdi>"));
        assert!(has_rich_tag("<P>x <B>y</B>"));
    }
}
