//! Markdown → HTML for the preview (pure Rust, no GTK).

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::state::Theme;
use crate::{state, tags};

/// Markdown body as HTML: GFM (tables, task lists, strikethrough, autolinks, footnotes).
/// Raw HTML in the note is escaped (shown as text), never rendered — notes may come from
/// anywhere (git, sync), so the preview is safe by default. `javascript:` links are dropped.
/// `[[Note]]` / `[[Note|text]]` (not in code) link to `wiki(target)`'s note, or to
/// `MISSING` + the target when it returns None. Tags (`tags::in_line`, not in code or
/// links) become `<span class="tag">`. Task checkboxes are wrapped in a
/// `<a class="task" href="mm-task:N">`, N the 1-based source line of the item's `[ ]`.
/// Code blocks are wrapped in `<div class="code">` with a `<a class="copy" href="mm-copy:N">`
/// button, N the block's first source line (see `code_block_at`). The preview (`page`)
/// adds `data-sourcepos`; this test-only form leaves it out to keep the expected HTML short.
#[cfg(test)]
fn to_html(md: &str, wiki: &dyn Fn(&str) -> Option<PathBuf>) -> String {
    body_html(md, wiki, true, true, false)
}

/// `to_html`, with syntect code highlighting only if `highlight`, clickable task
/// checkboxes and code Copy buttons only if `tasks`, and `data-sourcepos="L:C-L:C"` on
/// elements (for scroll sync) only if `sourcepos`.
pub(crate) fn body_html(
    md: &str,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
    highlight: bool,
    tasks: bool,
    sourcepos: bool,
) -> String {
    let mut options = options();
    options.render.sourcepos = sourcepos;
    let arena = comrak::Arena::new();
    // Tags are marked with these private-use chars in the AST and turned into spans after
    // escaping; the note's own copies are dropped so it cannot open/close a span.
    let md = md.replace([TAG_OPEN, TAG_CLOSE, TASK_OPEN, TASK_CLOSE, MATH], "");
    let root = comrak::parse_document(&arena, &md, &options);
    let mut task_items = Vec::new();
    let mut code_lines = Vec::new();
    let mut front_matter = None;
    let mut formulas = Vec::new();
    for node in root.descendants() {
        use comrak::nodes::NodeValue;
        let in_link = || {
            node.ancestors().any(|n| {
                matches!(
                    n.data().value,
                    NodeValue::Link(_) | NodeValue::WikiLink(_) | NodeValue::Image(_)
                )
            })
        };
        let marked = match &node.data().value {
            NodeValue::Text(text) if !in_link() => mark_tags(text),
            _ => None,
        };
        if let Some(marked) = marked {
            node.data_mut().value = NodeValue::Text(marked.into());
            continue;
        }
        // Formulas become a marker in the AST (raw HTML would be escaped) and their MathML
        // is put back after rendering.
        let formula = match &node.data().value {
            NodeValue::Math(math) => {
                let dollars = if math.display_math { "$$" } else { "$" };
                Some((math_html(&math.literal, math.display_math, dollars), false))
            }
            // A ```math block (GitHub/GitLab) becomes a paragraph holding the formula, so
            // it keeps `data-sourcepos` and gets no Copy button.
            NodeValue::CodeBlock(block) if block.info.split_whitespace().next() == Some("math") => {
                Some((math_html(block.literal.trim_end(), true, ""), true))
            }
            _ => None,
        };
        if let Some((formula, block)) = formula {
            let text = format!("{MATH}{}{MATH}", formulas.len());
            formulas.push(formula);
            let text = NodeValue::Text(text.into());
            if block {
                node.data_mut().value = NodeValue::Paragraph;
                node.append(arena.alloc(text.into()));
            } else {
                node.data_mut().value = text;
            }
            continue;
        }
        if let NodeValue::TaskItem(item) = &node.data().value {
            task_items.push((node, item.symbol_sourcepos.start.line));
        }
        if let NodeValue::CodeBlock(_) = &node.data().value {
            code_lines.push(node.data().sourcepos.start.line);
        }
        if let NodeValue::FrontMatter(text) = &node.data().value {
            front_matter = Some(text.to_string());
        }
        if let NodeValue::WikiLink(link) = &mut node.data_mut().value {
            // `[[Note#Heading]]` / `[[#Heading]]`: the heading's id (see `heading_at`).
            let anchor = link
                .url
                .split_once('#')
                .map(|(_, heading)| format!("#{}", comrak::Anchorizer::new().anchorize(heading)));
            let anchor = anchor.filter(|a| a.len() > 1);
            // An absolute path (no `file:` scheme, which comrak drops) resolves against the
            // note's file:// URI and comes back to `link_action` as a `Link::Note`.
            link.url = match (wiki(&link.url), anchor) {
                (_, Some(anchor)) if link.url.starts_with('#') => anchor,
                (Some(path), anchor) => {
                    percent_encode(path.as_os_str().as_bytes()) + anchor.as_deref().unwrap_or("")
                }
                (None, _) => format!("{MISSING}{}", percent_encode(link.url.as_bytes())),
            };
        }
    }
    // The renderer writes a task item's checkbox right before its children: a marker
    // child carrying the line tells the post-processing below which checkbox is which.
    for (node, line) in task_items.into_iter().filter(|_| tasks) {
        let text = format!("{TASK_OPEN}{line}{TASK_CLOSE}");
        node.prepend(arena.alloc(comrak::nodes::NodeValue::Text(text.into()).into()));
    }
    let mut plugins = comrak::options::Plugins::default();
    if highlight {
        plugins.render.codefence_syntax_highlighter = Some(&Highlighter);
    }
    let mut html = String::new();
    comrak::format_html_with_plugins(root, &options, &mut html, &plugins)
        .expect("writing to a String");
    // comrak ends each heading with an empty `<a href="#id" … class="anchor"></a>`: dropped,
    // the id on the heading is enough. The note's text is escaped, so it has no such `"`.
    let anchor_end = "\" class=\"anchor\"></a>";
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    while let Some(end) = rest.find(anchor_end) {
        let start = rest[..end].rfind("<a href=\"#").expect("anchor start");
        out.push_str(&rest[..start]);
        rest = &rest[end + anchor_end.len()..];
    }
    out.push_str(rest);
    let html = out
        .replace(TAG_OPEN, "<span class=\"tag\">")
        .replace(TAG_CLOSE, "</span>");
    // Each pass below copies the HTML once (no per-item replace/insert: a large note has
    // thousands of formulas, tasks and code blocks). The note's own copies of the marker
    // chars were dropped, so `MATH{i}MATH` splits into text, index, text, index, …
    let mut out = String::with_capacity(html.len());
    for (i, part) in html.split(MATH).enumerate() {
        match (i % 2 == 1).then(|| part.parse::<usize>().ok()).flatten() {
            Some(n) => out.push_str(&formulas[n]),
            None => out.push_str(part),
        }
    }
    let html = out;
    // `<input …/> MARK` → `<a …><input …/></a> `; the note's text is escaped, so the last
    // `<input` before a marker is its checkbox.
    let mut out = String::with_capacity(html.len());
    let mut done = 0;
    while let Some(open) = html[done..].find(TASK_OPEN).map(|i| done + i) {
        let close = open
            + html[open..]
                .find(TASK_CLOSE)
                .expect("markers come in pairs");
        let line = &html[open + TASK_OPEN.len_utf8()..close];
        let input = done
            + html[done..open]
                .rfind("<input")
                .expect("checkbox before marker");
        let checkbox = html[input..open].trim_end();
        out.push_str(&html[done..input]);
        out.push_str(&format!(
            "<a class=\"task\" href=\"{TASK}{line}\">{checkbox}</a> "
        ));
        done = close + TASK_CLOSE.len_utf8();
    }
    out.push_str(&html[done..]);
    let mut html = out;
    // Code blocks render in document order and the note's text is escaped, so the n-th
    // `<pre` is the n-th code block and the next `</pre>` closes it.
    if tasks && !code_lines.is_empty() {
        let mut out = String::with_capacity(html.len() + code_lines.len() * 80);
        let mut done = 0;
        for line in code_lines {
            let Some(pre) = html[done..].find("<pre").map(|i| done + i) else {
                break;
            };
            let close = pre + html[pre..].find("</pre>").expect("closed") + 6;
            out.push_str(&html[done..pre]);
            out.push_str(&format!(
                "<div class=\"code\"><a class=\"copy\" href=\"{COPY}{line}\">Copy</a>"
            ));
            out.push_str(&html[pre..close]);
            out.push_str("</div>");
            done = close;
        }
        out.push_str(&html[done..]);
        html = out;
    }
    // Front matter (comrak renders nothing for it) is shown in the preview only, after the
    // code-block pass above, which counts `<pre`s.
    if let Some(text) = front_matter.filter(|_| tasks) {
        let mut pre = String::from("<pre class=\"front-matter\">");
        comrak::html::escape(&mut pre, text.trim_end()).expect("writing to a String");
        html.insert_str(0, &format!("{pre}</pre>\n"));
    }
    html
}

/// LaTeX as MathML (WebKit renders it natively, no script), or, if math-core cannot
/// convert it, the source between `delim`s as escaped `<code class="math-error">`.
fn math_html(latex: &str, display: bool, delim: &str) -> String {
    use math_core::{LatexToMathML, MathCoreConfig, MathDisplay};
    let mode = if display {
        MathDisplay::Block
    } else {
        MathDisplay::Inline
    };
    let converted = LatexToMathML::new(MathCoreConfig::default())
        .ok()
        .and_then(|c| c.convert_with_local_state(latex, mode).ok());
    if let Some(result) = converted {
        return result.mathml;
    }
    let mut html = String::from("<code class=\"math-error\">");
    comrak::html::escape(&mut html, &format!("{delim}{latex}{delim}"))
        .expect("writing to a String");
    html + "</code>"
}

/// Markdown options shared by rendering and `code_block_at`.
fn options() -> comrak::Options<'static> {
    let mut options = comrak::Options::default();
    let ext = &mut options.extension;
    ext.table = true;
    ext.tasklist = true;
    ext.strikethrough = true;
    ext.autolink = true;
    ext.footnotes = true;
    // `$…$` / `$$…$$` (comrak's rules: `$5 and $10` stays text), see `math_html`.
    ext.math_dollars = true;
    ext.wikilinks_title_after_pipe = true;
    // YAML front matter (Obsidian/Jekyll/Hugo): only when `---` is the first line.
    ext.front_matter_delimiter = Some("---".into());
    // Headings get GitHub-style ids (`# Cài đặt` → `cài-đặt`, repeats `-1`, …) so `#…` links
    // work in the preview and in exported files; see `heading_at`.
    ext.header_id_prefix = Some(String::new());
    options.render.escape = true;
    options
}

/// The 1-based line and id of the heading a link fragment names: its id (as rendered, with
/// the `-1` of repeated titles) or, failing that, its text (`[[Note#Heading]]`). None if no
/// heading matches.
pub fn heading_at(md: &str, fragment: &str) -> Option<(usize, String)> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &options());
    // shortcut: the rendered id of a heading holding a formula or tag may differ (markers);
    // upgrade if such links show up.
    let mut ids = comrak::Anchorizer::new();
    let headings: Vec<_> = root
        .descendants()
        .filter(|n| matches!(n.data().value, comrak::nodes::NodeValue::Heading(_)))
        .map(|n| {
            (
                n.data().sourcepos.start.line,
                ids.anchorize(&n.collect_text()),
            )
        })
        .collect();
    let text = comrak::Anchorizer::new().anchorize(fragment);
    [fragment, text.as_str()]
        .iter()
        .find_map(|want| headings.iter().find(|(_, id)| id == want))
        .cloned()
}

/// The `#fragment` of a wiki link to the heading spanning 1-based `line` of `md` (Outline ▸
/// Copy Link): its plain text, which `heading_at` matches, or its id when an earlier heading
/// has the same text or the text holds `]`/`|` (which would end the link). None if no heading
/// spans `line`.
pub fn heading_fragment(md: &str, line: usize) -> Option<String> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &options());
    let node = root.descendants().find(|n| {
        let pos = n.data().sourcepos;
        matches!(n.data().value, comrak::nodes::NodeValue::Heading(_))
            && (pos.start.line..=pos.end.line).contains(&line)
    })?;
    let start = node.data().sourcepos.start.line;
    let text = node.collect_text();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match heading_at(md, &text) {
        Some((l, _)) if l == start && !text.contains([']', '|']) => Some(text),
        _ => root
            .descendants()
            .filter(|n| matches!(n.data().value, comrak::nodes::NodeValue::Heading(_)))
            .scan(comrak::Anchorizer::new(), |ids, n| {
                Some((
                    n.data().sourcepos.start.line,
                    ids.anchorize(&n.collect_text()),
                ))
            })
            .find_map(|(l, id)| (l == start).then_some(id)),
    }
}

/// The content of the code block (fenced or indented, at any depth) starting on 1-based
/// `line` of `md`, as the preview's Copy button gives it; None if no block starts there.
pub fn code_block_at(md: &str, line: usize) -> Option<String> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &options());
    root.descendants()
        .find_map(|node| match &node.data().value {
            comrak::nodes::NodeValue::CodeBlock(block)
                if node.data().sourcepos.start.line == line =>
            {
                Some(block.literal.clone())
            }
            _ => None,
        })
}

/// The formulas of `md` the preview renders as math (`$…$`, `$$…$$`, ```math blocks; by
/// comrak's rules, so `$5 và $10` and code are not), for the editor's colour: 0-based
/// (line, byte) start and exclusive end, markers included.
pub fn math_spans(md: &str) -> Vec<((usize, usize), (usize, usize))> {
    // Most notes have no formula: skip the parse.
    if !md.contains('$') && !md.contains("math") {
        return vec![];
    }
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &options());
    root.descendants()
        .filter(|node| match &node.data().value {
            comrak::nodes::NodeValue::Math(_) => true,
            comrak::nodes::NodeValue::CodeBlock(b) => {
                b.info.split_whitespace().next() == Some("math")
            }
            _ => false,
        })
        .map(|node| {
            let p = node.data().sourcepos;
            (
                (p.start.line - 1, p.start.column - 1),
                (p.end.line - 1, p.end.column),
            )
        })
        .collect()
}

/// Code blocks whose language syntect knows get `<span class="hl-…">` per scope (coloured by
/// `CSS`, light and dark); unknown or no language → escaped as before. Class-based so the
/// theme follows the page's light/dark class without re-rendering.
struct Highlighter;

/// syntect's bundled syntaxes, loaded on the first highlighted code block only.
static SYNTAXES: std::sync::LazyLock<syntect::parsing::SyntaxSet> =
    std::sync::LazyLock::new(syntect::parsing::SyntaxSet::load_defaults_newlines);

impl comrak::adapters::SyntaxHighlighterAdapter for Highlighter {
    fn write_highlighted(
        &self,
        output: &mut dyn std::fmt::Write,
        lang: Option<&str>,
        code: &str,
    ) -> std::fmt::Result {
        use syntect::html::{ClassStyle, ClassedHTMLGenerator};
        let token = lang.unwrap_or("").split(',').next().unwrap_or("").trim();
        let syntax = (!token.is_empty())
            .then(|| SYNTAXES.find_syntax_by_token(token))
            .flatten();
        let Some(syntax) = syntax else {
            return comrak::html::escape(output, code);
        };
        let style = ClassStyle::SpacedPrefixed { prefix: "hl-" };
        let mut generator = ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAXES, style);
        for line in syntect::util::LinesWithEndings::from(code) {
            if generator
                .parse_html_for_line_which_includes_newline(line)
                .is_err()
            {
                return comrak::html::escape(output, code);
            }
        }
        output.write_str(&generator.finalize())
    }

    fn write_pre_tag(
        &self,
        output: &mut dyn std::fmt::Write,
        attributes: std::collections::HashMap<&'static str, std::borrow::Cow<'_, str>>,
    ) -> std::fmt::Result {
        comrak::html::write_opening_tag(output, "pre", attributes)
    }

    fn write_code_tag(
        &self,
        output: &mut dyn std::fmt::Write,
        attributes: std::collections::HashMap<&'static str, std::borrow::Cow<'_, str>>,
    ) -> std::fmt::Result {
        comrak::html::write_opening_tag(output, "code", attributes)
    }
}

/// `text` with each tag wrapped in TAG_OPEN/TAG_CLOSE; None if it has no tag.
fn mark_tags(text: &str) -> Option<String> {
    let spans = tags::in_line(text);
    let mut marked = text.to_string();
    for r in spans.iter().rev() {
        marked.insert(r.end, TAG_CLOSE);
        marked.insert(r.start, TAG_OPEN);
    }
    (!spans.is_empty()).then_some(marked)
}

const TAG_OPEN: char = '\u{E000}';
const TAG_CLOSE: char = '\u{E001}';

const TASK_OPEN: char = '\u{E002}';
const TASK_CLOSE: char = '\u{E003}';

/// Wraps the index of a formula in `body_html`'s `formulas`.
const MATH: char = '\u{E004}';

/// URI prefix of a task checkbox; the item's 1-based source line follows.
const TASK: &str = "mm-task:";

/// URI prefix of a code block's Copy button; the block's 1-based first source line follows.
const COPY: &str = "mm-copy:";

/// URI prefix of a wiki link whose note was not found.
const MISSING: &str = "mm-missing:";

/// Every byte but `A-Za-z0-9-_.~/` as `%XX`.
fn percent_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The note a wiki link `target` points to: matched by name without `.md`/`.markdown`,
/// case-insensitive, optionally with folders (`Folder/Note`, relative to `root`) and an
/// ignored `#heading`. Several matches: one in `current`'s folder first, then the shortest
/// path.
pub fn resolve_wiki<'a>(
    target: &str,
    notes: &'a [PathBuf],
    root: &Path,
    current: Option<&Path>,
) -> Option<&'a PathBuf> {
    let target = target
        .split('#')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('/');
    let target = strip_note_ext(target).to_lowercase();
    if target.is_empty() {
        return None;
    }
    let here = current.and_then(Path::parent);
    notes
        .iter()
        .filter(|p| {
            let rel = p.strip_prefix(root).unwrap_or(p).to_string_lossy();
            let rel = strip_note_ext(&rel).to_lowercase();
            rel == target || rel.ends_with(&format!("/{target}"))
        })
        .min_by_key(|p| (p.parent() != here, p.as_os_str().len(), p.as_os_str()))
}

fn strip_note_ext(name: &str) -> &str {
    for ext in [".md", ".markdown"] {
        if name.len() > ext.len()
            && name.is_char_boundary(name.len() - ext.len())
            && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext)
        {
            return &name[..name.len() - ext.len()];
        }
    }
    name
}

/// Full preview page. The CSP blocks scripts and every network load: only local files
/// (images next to the note) and inline styles.
pub fn page(md: &str, dark: bool, theme: Theme, wiki: &dyn Fn(&str) -> Option<PathBuf>) -> String {
    let csp = "default-src 'none'; img-src file: data:; style-src 'unsafe-inline'";
    html_page(dark, theme, csp, "", &body_html(md, wiki, true, true, true))
}

/// Standalone HTML file of a note (Export as HTML): the preview's CSS in light mode, code
/// highlighting, no script. Meant to sit next to the note (`note_dir`): relative images and
/// `.md` links stay relative, wiki links become relative links to the target note (no local
/// absolute paths in the file), unresolved wiki links become plain text. Images may load
/// from the web when the file is opened elsewhere (`img-src *`).
pub fn export_page(
    md: &str,
    title: &str,
    note_dir: &Path,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
) -> String {
    let body = portable_body(md, note_dir, wiki, true);
    let mut escaped = String::new();
    comrak::html::escape(&mut escaped, title).expect("writing to a String");
    let csp = "default-src 'none'; img-src * data: file:; style-src 'unsafe-inline'";
    html_page(
        false,
        Theme::Default,
        csp,
        &format!("<title>{escaped}</title>"),
        &body,
    )
}

/// Copy as HTML: the note's (or selection's) HTML for pasting into mail/docs, with the same
/// links as `export_page` relative to `note_dir`. No page CSS or script; code is plain
/// `<pre><code>` (the `hl-*` classes mean nothing without our CSS), tags plain `#tag` text in
/// a classed span. The `<meta charset>` makes browsers/editors read the clipboard as UTF-8.
pub fn html_fragment(md: &str, note_dir: &Path, wiki: &dyn Fn(&str) -> Option<PathBuf>) -> String {
    format!(
        "<meta charset=\"utf-8\">{}",
        portable_body(md, note_dir, wiki, false)
    )
}

/// `body_html` with wiki links relative to `note_dir` and unresolved ones as plain text.
fn portable_body(
    md: &str,
    note_dir: &Path,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
    highlight: bool,
) -> String {
    let wiki = |t: &str| wiki(t).map(|p| relative(note_dir, &p));
    let mut body = body_html(md, &wiki, highlight, false, false);
    // A link cannot contain a link, so the next `</a>` closes it; the note's own text is
    // escaped and cannot contain this markup.
    let open = format!("<a href=\"{MISSING}");
    while let Some(start) = body.find(&open) {
        let Some(gt) = body[start..].find('>').map(|i| start + i) else {
            break;
        };
        let Some(close) = body[gt..].find("</a>").map(|i| gt + i) else {
            break;
        };
        body.replace_range(close..close + 4, "");
        body.replace_range(start..=gt, "");
    }
    body
}

/// `s` as a double-quoted JavaScript string literal, safe to splice into a script: quotes,
/// backslashes, control characters, U+2028/U+2029 and `<` (no `</script>`) are escaped.
pub fn js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '<' | '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn html_page(dark: bool, theme: Theme, csp: &str, head: &str, body: &str) -> String {
    let scheme = if dark { "dark" } else { "light" };
    let vars = theme_vars(theme, dark);
    format!(
        "<!doctype html><html class=\"{scheme}\"><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"{csp}\">{head}\
<style>{vars}{CSS}</style></head><body>{body}</body></html>"
    )
}

/// `to` relative to the folder `from` (both absolute), e.g. `/r/a` + `/r/b/c.md` → `../b/c.md`.
fn relative(from: &Path, to: &Path) -> PathBuf {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    from[common..]
        .iter()
        .map(|_| std::path::Component::ParentDir)
        .chain(to[common..].iter().copied())
        .collect()
}

/// Biến CSS mỗi theme phải đặt (`theme_vars`), `CSS` dùng chúng.
const VARS: [&str; 14] = [
    "bg",
    "fg",
    "link",
    "missing",
    "code-bg",
    "tag",
    "tag-bg",
    "hl-comment",
    "hl-string",
    "hl-constant",
    "hl-keyword",
    "hl-function",
    "hl-type",
    "hl-tag",
];

/// Giá trị của `VARS` (cùng thứ tự) cho một theme ở chế độ sáng hoặc tối.
fn theme_values(theme: Theme, dark: bool) -> [&'static str; 14] {
    match (theme, dark) {
        (Theme::Default, false) => [
            "#fff",
            "#1e1e1e",
            "#1c71d8",
            "#c01c28",
            "rgba(127,127,127,.15)",
            "#1c71d8",
            "rgba(53,132,228,.15)",
            "#6e7781",
            "#0a3069",
            "#0550ae",
            "#cf222e",
            "#8250df",
            "#953800",
            "#116329",
        ],
        (Theme::Default, true) => [
            "#1e1e1e",
            "#dedede",
            "#78aeed",
            "#f66151",
            "rgba(127,127,127,.15)",
            "#99c1f1",
            "rgba(120,174,237,.2)",
            "#8b949e",
            "#a5d6ff",
            "#79c0ff",
            "#ff7b72",
            "#d2a8ff",
            "#ffa657",
            "#7ee787",
        ],
        (Theme::Paper, false) => [
            "#f8f4e8",
            "#3b3228",
            "#9c4f1b",
            "#b3261e",
            "rgba(120,100,60,.12)",
            "#7a5a1e",
            "rgba(160,120,40,.15)",
            "#8c7f6a",
            "#5b7a1f",
            "#8f3f71",
            "#a33b2b",
            "#2f5f8a",
            "#8a5a00",
            "#3f6f4f",
        ],
        (Theme::Paper, true) => [
            "#2a2620",
            "#e8e0d0",
            "#e0a96d",
            "#f28b82",
            "rgba(255,240,200,.08)",
            "#e5c07b",
            "rgba(229,192,123,.18)",
            "#998f7d",
            "#b5c97a",
            "#d3869b",
            "#f08c6c",
            "#8ec0e0",
            "#e5b567",
            "#9cc29a",
        ],
        (Theme::Graphite, false) => [
            "#f2f2f2",
            "#2b2b2b",
            "#3d5a78",
            "#b3261e",
            "rgba(0,0,0,.07)",
            "#4a4a4a",
            "rgba(0,0,0,.08)",
            "#8a8a8a",
            "#4f6f4f",
            "#5a5a8a",
            "#1a1a1a",
            "#3d5a78",
            "#6a5a4a",
            "#4a6a5a",
        ],
        (Theme::Graphite, true) => [
            "#232323",
            "#d0d0d0",
            "#a8b8c8",
            "#f28b82",
            "rgba(255,255,255,.07)",
            "#c0c0c0",
            "rgba(255,255,255,.1)",
            "#7a7a7a",
            "#a8c0a0",
            "#b0b0d8",
            "#f0f0f0",
            "#a0b8d0",
            "#d0c0a8",
            "#a0c8b8",
        ],
        (Theme::Solarized, false) => [
            "#fdf6e3",
            "#657b83",
            "#268bd2",
            "#dc322f",
            "#eee8d5",
            "#2aa198",
            "rgba(42,161,152,.15)",
            "#93a1a1",
            "#2aa198",
            "#d33682",
            "#859900",
            "#268bd2",
            "#b58900",
            "#cb4b16",
        ],
        (Theme::Solarized, true) => [
            "#002b36",
            "#839496",
            "#268bd2",
            "#dc322f",
            "#073642",
            "#2aa198",
            "rgba(42,161,152,.2)",
            "#586e75",
            "#2aa198",
            "#d33682",
            "#859900",
            "#268bd2",
            "#b58900",
            "#cb4b16",
        ],
    }
}

/// `:root { --bg: …; … }` của một theme ở chế độ sáng hoặc tối.
fn theme_vars(theme: Theme, dark: bool) -> String {
    let decls: String = VARS
        .iter()
        .zip(theme_values(theme, dark))
        .map(|(k, v)| format!("--{k}: {v}; "))
        .collect();
    format!(":root {{ {decls}}}")
}

const CSS: &str = "
html { color-scheme: light; background: var(--bg); color: var(--fg); }
html.dark { color-scheme: dark; }
body { font: 16px/1.6 sans-serif; max-width: 46em; margin: 0 auto; padding: 24px; }
a { color: var(--link); }
a[href^='mm-missing:'] { color: var(--missing); text-decoration-style: dashed; }
code, pre { font-family: monospace; font-size: 0.9em; background: var(--code-bg); border-radius: 4px; }
code { padding: .1em .3em; } pre { padding: 12px; overflow-x: auto; } pre code { background: none; padding: 0; }
blockquote { margin: 0; padding-left: 1em; border-left: 4px solid rgba(127,127,127,.4); opacity: .85; }
table { border-collapse: collapse; } th, td { border: 1px solid rgba(127,127,127,.4); padding: 4px 10px; }
img { max-width: 100%; }
ul:has(> li > input[type=checkbox]), ul:has(> li > a.task) { list-style: none; padding-left: 1.2em; }
a.task { cursor: pointer; }
a.task input { pointer-events: none; appearance: none; width: 1em; height: 1em; margin: 0 .2em 0 0; vertical-align: -.15em; border: 1.5px solid var(--link); border-radius: 3px; background: var(--bg); }
a.task input:checked { background: var(--link) url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Cpath d='M3.5 8.5l3 3 6-7' fill='none' stroke='white' stroke-width='2.2'/%3E%3C/svg%3E\") center / 85% no-repeat; }
.tag { color: var(--tag); background: var(--tag-bg); border-radius: 1em; padding: 0 .45em; white-space: nowrap; }
.hl-comment { color: var(--hl-comment); font-style: italic; } .hl-string { color: var(--hl-string); }
.hl-constant { color: var(--hl-constant); } .hl-keyword, .hl-storage { color: var(--hl-keyword); }
.hl-entity.hl-name.hl-function, .hl-support.hl-function { color: var(--hl-function); }
.hl-entity.hl-name.hl-type, .hl-entity.hl-name.hl-class, .hl-support.hl-type, .hl-support.hl-class { color: var(--hl-type); }
.hl-entity.hl-name.hl-tag { color: var(--hl-tag); }
div.code { position: relative; }
a.copy { visibility: hidden; position: absolute; top: 6px; right: 6px; font: 12px sans-serif; padding: 2px 8px; border-radius: 4px; color: var(--fg); background: var(--bg); border: 1px solid rgba(127,127,127,.4); text-decoration: none; }
div.code:hover a.copy { visibility: visible; }
hr { border: none; border-top: 1px solid rgba(127,127,127,.4); }
pre.front-matter { font-size: 0.8em; color: var(--hl-comment); background: none; border: 1px solid rgba(127,127,127,.25); padding: 6px 10px; opacity: .8; }
@media print { html { background: none; } body { max-width: none; padding: 0; } pre, table, img, blockquote { break-inside: avoid; } a.copy, pre.front-matter { display: none; } }
";

/// What the preview does with a navigation to `uri`.
#[derive(Debug, PartialEq)]
pub enum Link {
    /// Load it: the page itself or an anchor in it (footnotes).
    Follow,
    /// Open with the system's default app; the preview stays put.
    External,
    /// A link to another note (`.md`/`.markdown` file, percent-decoded, anchor dropped);
    /// still to be checked with `note_in_folder`.
    Note(PathBuf),
    /// A wiki link whose note was not found; the target as written.
    Missing(String),
    /// A task checkbox: toggle the to-do on this 1-based line.
    Task(usize),
    /// A code block's Copy button: copy the block starting on this 1-based line.
    Copy(usize),
    /// Do nothing (other files, unknown schemes).
    Ignore,
}

/// `page_uri` is the base URI the page was loaded with (the note's file:// URI).
pub fn link_action(uri: &str, page_uri: &str) -> Link {
    let without_fragment = uri.split('#').next().unwrap_or(uri);
    if without_fragment == page_uri {
        Link::Follow
    } else if let Some(line) = uri.strip_prefix(TASK) {
        line.parse().map_or(Link::Ignore, Link::Task)
    } else if let Some(line) = uri.strip_prefix(COPY) {
        line.parse().map_or(Link::Ignore, Link::Copy)
    } else if let Some(target) = uri.strip_prefix(MISSING) {
        Link::Missing(
            state::unescape(target)
                .map_or_else(|| target.to_string(), |p| p.to_string_lossy().into_owned()),
        )
    } else if ["http://", "https://", "mailto:"].iter().any(|s| {
        uri.get(..s.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(s))
    }) {
        Link::External
    } else if let Some(path) = without_fragment
        .split('?')
        .next()
        .and_then(|u| u.strip_prefix("file://"))
        .and_then(|u| u.strip_prefix("localhost").or(Some(u)))
        .filter(|u| u.starts_with('/'))
        .and_then(state::unescape)
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"))
        })
    {
        Link::Note(path)
    } else {
        Link::Ignore
    }
}

/// `path` as a note inside `root` to open, or None if it does not exist, is not a file, or
/// lies outside `root` once `..` and symlinks are resolved. The result keeps `root`'s
/// spelling (the sidebar's paths) with the canonical rest of the path.
pub fn note_in_folder(path: &Path, root: &Path) -> Option<PathBuf> {
    let real = path.canonicalize().ok()?;
    let rest = real.strip_prefix(root.canonicalize().ok()?).ok()?;
    real.is_file().then(|| root.join(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_highlighting() {
        let html = to_html("```rust\nfn main() { let s = \"<b>&\"; }\n```\n", &|_| None);
        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(html.contains("<span class=\"hl-storage hl-type hl-function hl-rust\">fn</span>"));
        assert!(html.contains("&lt;b&gt;&amp;"));
        assert!(!html.contains("<b>"));
        for md in ["```nosuchlang\nfn <i>\n```\n", "```\nfn <i>\n```\n"] {
            let html = to_html(md, &|_| None);
            assert!(!html.contains("<span"), "{html}");
            assert!(html.contains("fn &lt;i&gt;"));
        }
    }

    #[test]
    fn heading() {
        assert!(to_html("# Title", &|_| None).contains("<h1 id=\"title\">Title</h1>"));
    }

    #[test]
    fn heading_ids() {
        // GitHub-style: lower case, accents kept, space → `-`, punctuation dropped, repeats `-1`.
        let md =
            "# Cài đặt **nhanh**!\n\n## Cài đặt nhanh\n\nỨng dụng\n---\n\n[go](#cài-đặt-nhanh-1)\n";
        let html = to_html(md, &|_| None);
        assert!(
            html.contains("<h1 id=\"cài-đặt-nhanh\">Cài đặt <strong>nhanh</strong>!</h1>"),
            "{html}"
        );
        assert!(
            html.contains("<h2 id=\"cài-đặt-nhanh-1\">Cài đặt nhanh</h2>"),
            "{html}"
        );
        assert!(html.contains("<h2 id=\"ứng-dụng\">Ứng dụng</h2>"), "{html}");
        assert!(!html.contains("class=\"anchor\""), "{html}");
        // Links into the note and to other notes' headings survive export.
        let wiki = |t: &str| t.starts_with("Other").then(|| PathBuf::from("/n/Other.md"));
        let page = export_page(
            &format!("{md}[[Other#Mục Hai]] [[#Ứng dụng]] [x](Other.md#mục-hai)\n"),
            "t",
            Path::new("/n"),
            &wiki,
        );
        assert!(page.contains("<h1 id=\"cài-đặt-nhanh\">"), "{page}");
        for href in [
            "#c%C3%A0i-%C4%91%E1%BA%B7t-nhanh-1",
            "Other.md#m%E1%BB%A5c-hai",
            "#%E1%BB%A9ng-d%E1%BB%A5ng",
        ] {
            assert!(
                page.contains(&format!("href=\"{href}\"")),
                "{href} in {page}"
            );
        }
        assert_eq!(
            heading_at(md, "cài-đặt-nhanh-1"),
            Some((3, "cài-đặt-nhanh-1".into()))
        );
        assert_eq!(heading_at(md, "Ứng dụng"), Some((5, "ứng-dụng".into())));
        assert_eq!(
            heading_at(md, "Cài đặt nhanh"),
            Some((1, "cài-đặt-nhanh".into()))
        );
        assert_eq!(heading_at(md, "nope"), None);
    }

    #[test]
    fn heading_fragments() {
        let md = "---\ntitle: x\n---\n# **Cài đặt** `nhanh`\n\nSetext *Ứng*\ndụng\n---\n\n## Cài đặt nhanh\n\n## a ] b | c\n";
        for (line, want) in [
            (4, "Cài đặt nhanh"),
            (6, "Setext Ứng dụng"),
            (7, "Setext Ứng dụng"),
            (10, "cài-đặt-nhanh-1"),
            (12, "a--b--c"),
        ] {
            let got = heading_fragment(md, line).unwrap();
            assert_eq!(got, want);
            let start = if line == 7 { 6 } else { line };
            assert_eq!(heading_at(md, &got).map(|(l, _)| l), Some(start), "{got}");
        }
        assert_eq!(heading_fragment(md, 2), None);
        assert_eq!(heading_fragment(md, 9), None);
    }

    #[test]
    fn table() {
        let html = to_html("| a | b |\n|---|---|\n| 1 | 2 |\n", &|_| None);
        assert!(html.contains("<table>"));
        assert!(html.contains("<th>a</th>"));
        assert!(html.contains("<td>2</td>"));
    }

    #[test]
    fn task_list() {
        let md = "# T\n\n- [x] done\n- [ ] todo\n  1. [X] nested\n     - plain\n\n> - [ ] quoted\n\
\nText[^1]\n\n[^1]: - [ ] in note \u{E002}9\u{E003}\n";
        let html = to_html(md, &|_| None);
        let a = |line: usize, checked: bool| {
            let c = if checked { " checked=\"\"" } else { "" };
            format!(
                "<a class=\"task\" href=\"mm-task:{line}\"><input type=\"checkbox\"{c} disabled=\"\" /></a> "
            )
        };
        for (line, checked) in [(3, true), (4, false), (5, true), (8, false), (12, false)] {
            assert!(html.contains(&a(line, checked)), "{line}: {html}");
        }
        assert_eq!(html.matches("<a class=\"task\"").count(), 5);
        assert!(html.contains("nested\n<ul>\n<li>plain</li>"), "{html}");
        assert!(!html.contains('\u{E002}') && !html.contains("mm-task:9"));
        assert_eq!(link_action("mm-task:12", "file:///a.md"), Link::Task(12));
        assert_eq!(link_action("mm-task:x", "file:///a.md"), Link::Ignore);
    }

    #[test]
    fn code_copy_buttons() {
        let md = "# T\n\n```rust\nfn x() {}\n```\n\n    indented\n    two\n\n- item\n\n  ```\n  in list\n  ```\n\n> ~~~\n> quoted\n> ~~~\n";
        let html = to_html(md, &|_| None);
        for line in [3, 7, 12, 16] {
            assert!(html.contains(&format!(
                "<div class=\"code\"><a class=\"copy\" href=\"mm-copy:{line}\">Copy</a><pre"
            )));
        }
        assert_eq!(html.matches("</pre></div>").count(), 4);
        assert!(html.contains("<span class=\"hl-")); // highlighting still applies
        assert_eq!(code_block_at(md, 3).as_deref(), Some("fn x() {}\n"));
        assert_eq!(code_block_at(md, 7).as_deref(), Some("indented\ntwo\n"));
        assert_eq!(code_block_at(md, 12).as_deref(), Some("in list\n"));
        assert_eq!(code_block_at(md, 16).as_deref(), Some("quoted\n"));
        assert_eq!(code_block_at(md, 4), None);
        assert_eq!(code_block_at(md, 1), None);
        assert_eq!(link_action("mm-copy:7", "file:///a.md"), Link::Copy(7));
        assert_eq!(link_action("mm-copy:", "file:///a.md"), Link::Ignore);
        let export = export_page(md, "T", Path::new("/r"), &|_| None);
        let copy = html_fragment(md, Path::new("/r"), &|_| None);
        for out in [export, copy] {
            assert!(!out.contains("mm-copy:") && !out.contains("class=\"code\""));
            assert!(!out.contains("data-sourcepos"));
        }
    }

    #[test]
    fn footnote() {
        let html = to_html("Text[^1]\n\n[^1]: Note.\n", &|_| None);
        assert!(html.contains("href=\"#fn-1\""));
        assert!(html.contains("class=\"footnotes\""));
    }

    #[test]
    fn strikethrough_and_autolink() {
        let html = to_html("~~old~~ https://example.com", &|_| None);
        assert!(html.contains("<del>old</del>"));
        assert!(html.contains("<a href=\"https://example.com\">"));
    }

    #[test]
    fn raw_html_is_escaped() {
        let html = to_html(
            "<script>alert(1)</script>\n\nhi <b onclick=\"x\">b</b>",
            &|_| None,
        );
        assert!(!html.contains("<script"));
        assert!(!html.contains("<b "));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn math_spans() {
        let md = "---\nt: 1\n---\nGiá $5 và $10.\n\nCode `$x$` và $a^2$.\n\n> $$\n> \\frac{a}{b}\n> $$\n\n```math\nx\n```\n\n```\n$y$\n```\n";
        let spans = super::math_spans(md);
        let lines: Vec<&str> = md.lines().collect();
        let text = |((l1, c1), (l2, c2)): ((usize, usize), (usize, usize))| {
            if l1 == l2 {
                lines[l1][c1..c2].to_string()
            } else {
                format!("{}…{}", &lines[l1][c1..], &lines[l2][..c2])
            }
        };
        let got: Vec<String> = spans.into_iter().map(text).collect();
        assert_eq!(got, ["$a^2$", "$$…> $$", "```math…```"]);
        assert!(super::math_spans("Giá $5 và $10").is_empty());
    }

    #[test]
    fn math() {
        let html = to_html("a $x^2$ b\n\n$$\\frac{a}{b}$$\n", &|_| None);
        assert!(
            html.contains("a <math><msup><mi>x</mi><mn>2</mn></msup></math> b"),
            "{html}"
        );
        assert!(
            html.contains("<math display=\"block\"><mfrac><mi>a</mi><mi>b</mi></mfrac></math>"),
            "{html}"
        );
        let html = to_html("$\\sum_{i=1}^n \\sqrt{i}$ $a<b$", &|_| None);
        assert!(
            html.contains("<mo lspace=\"0\">∑</mo>") && html.contains("<msqrt>"),
            "{html}"
        );
        assert!(html.contains("<mo>&lt;</mo>"), "{html}");
        // Bad LaTeX: the source, escaped; no raw HTML gets through.
        let html = to_html("$\\frac{a$ and $$\\nope<b>$$", &|_| None);
        assert!(
            html.contains("<code class=\"math-error\">$\\frac{a$</code>"),
            "{html}"
        );
        assert!(
            html.contains("<code class=\"math-error\">$$\\nope&lt;b&gt;$$</code>"),
            "{html}"
        );
        assert!(!html.contains("<b>"), "{html}");
        // ```math blocks: block MathML in a paragraph that keeps its sourcepos, no Copy
        // button; bad LaTeX shows the block's source; other code blocks are unchanged.
        let md = "```math\n\\frac{a}{b}\n```\n\n```math\n\\frac{a\n```\n\n```\nx\n```\n";
        let html = to_html(md, &|_| None);
        assert!(
            html.starts_with(
                "<p><math display=\"block\"><mfrac><mi>a</mi><mi>b</mi></mfrac></math></p>"
            ),
            "{html}"
        );
        assert!(
            html.contains("<p><code class=\"math-error\">\\frac{a</code></p>"),
            "{html}"
        );
        assert!(html.contains("<pre><code>x\n</code></pre>"), "{html}");
        let p = page(md, false, Theme::Default, &|_| None);
        assert!(
            p.contains("<p data-sourcepos=\"1:1-3:3\"><math display"),
            "{p}"
        );
        assert!(p.contains("href=\"mm-copy:9\""), "{p}");
        assert_eq!(p.matches("mm-copy:").count(), 1, "{p}");
        // Prices are text; the note's own marker chars are dropped.
        assert_eq!(
            to_html("$5 and $10\u{E004}0\u{E004}", &|_| None),
            "<p>$5 and $100</p>\n"
        );
        // Export and Copy as HTML keep the MathML; the preview keeps `data-sourcepos`.
        let md = "# T\n\n$x$\n";
        let mathml = "<math><mi>x</mi></math>";
        assert!(html_fragment(md, Path::new("/r"), &|_| None).contains(mathml));
        assert!(export_page(md, "t", Path::new("/r"), &|_| None).contains(mathml));
        let p = page(md, false, Theme::Default, &|_| None);
        assert!(
            p.contains(&format!("<p data-sourcepos=\"3:1-3:3\">{mathml}</p>")),
            "{p}"
        );
    }

    #[test]
    fn javascript_link_dropped() {
        assert!(!to_html("[x](javascript:alert(1))", &|_| None).contains("javascript:"));
    }

    #[test]
    fn js_string_escapes() {
        assert_eq!(js_string(""), "\"\"");
        assert_eq!(
            js_string("</script>`${x}` \\ \"q\"\n\t"),
            "\"\\u003c/script>`${x}` \\\\ \\\"q\\\"\\n\\u0009\""
        );
        assert_eq!(
            js_string("Tiếng Việt 😀\u{2028}"),
            "\"Tiếng Việt 😀\\u2028\""
        );
    }

    #[test]
    fn page_blocks_scripts_and_network() {
        let p = page("# a", true, Theme::Default, &|_| None);
        assert!(p.contains("default-src 'none'"));
        assert!(p.contains("class=\"dark\""));
        assert!(
            p.contains("<h1 id=\"a\" data-sourcepos=\"1:1-1:3\">a</h1>"),
            "{p}"
        );
        // Scroll sync markup keeps the task and Copy links working.
        let p = page("- [ ] t\n\n```\nx\n```\n", false, Theme::Default, &|_| None);
        assert!(
            p.contains("<a class=\"task\" href=\"mm-task:1\"><input"),
            "{p}"
        );
        assert!(
            p.contains("href=\"mm-copy:3\">Copy</a><pre data-sourcepos=\"3:1-5:3\""),
            "{p}"
        );
    }

    #[test]
    fn every_theme_sets_every_variable() {
        for theme in Theme::ALL {
            for dark in [false, true] {
                let css = theme_vars(theme, dark);
                for (k, v) in VARS.iter().zip(theme_values(theme, dark)) {
                    assert!(!v.is_empty() && css.contains(&format!("--{k}: {v};")));
                }
                assert!(page("a", dark, theme, &|_| None).contains(&css));
            }
        }
        // Mọi biến mà CSS trang dùng đều được theme đặt.
        for used in CSS.split("var(--").skip(1) {
            let name = &used[..used.find(')').unwrap()];
            assert!(VARS.contains(&name), "{name}");
        }
    }

    #[test]
    fn export_is_standalone() {
        let found = |t: &str| (t != "Gone").then(|| PathBuf::from(format!("/r/{t}.md")));
        let md = "# Hi\n\n![p](assets/p.png) [b](b.md) [[Sub/A|a]] [[Gone|g]] #work\n\n\
```rust\nfn x() {}\n```\n\n<script>alert(1)</script>\n";
        let p = export_page(md, "My <Note>", Path::new("/r/Here"), &found);
        assert!(p.starts_with("<!doctype html><html class=\"light\">"));
        assert!(p.contains(&theme_vars(Theme::Default, false)));
        assert!(p.contains("<title>My &lt;Note&gt;</title>"));
        assert!(p.contains("<style>") && p.contains(".hl-keyword"));
        assert!(p.contains("default-src 'none'"));
        assert!(!p.contains("<script"));
        assert!(p.contains("<h1 id=\"hi\">Hi</h1>"));
        assert!(p.contains("<img src=\"assets/p.png\""));
        assert!(p.contains("<a href=\"b.md\">b</a>"));
        assert!(p.contains("<a href=\"../Sub/A.md\" data-wikilink=\"true\">a</a>"));
        assert!(p.contains("</a> g <span class=\"tag\">#work</span>"), "{p}");
        assert!(!p.contains("href=\"mm-missing") && !p.contains("/r/"));
        assert!(p.contains("<span class=\"hl-"));
        let p = export_page("- [x] a\n- [ ] b\n", "t", Path::new("/r"), &found);
        assert!(p.contains("<li><input type=\"checkbox\" checked=\"\" disabled=\"\" /> a</li>"));
        assert!(!p.contains("mm-task:") && !p.contains("class=\"task\""));
    }

    #[test]
    fn copy_fragment() {
        let found = |t: &str| (t != "Gone").then(|| PathBuf::from(format!("/r/{t}.md")));
        let md = "# Hi\n\n[[Sub/A|a]] [[Gone|g]] #work [x](javascript:alert(1))\n\n\
```rust\nfn x() {}\n```\n\n<script>alert(1)</script>\n";
        let f = html_fragment(md, Path::new("/r/Here"), &found);
        let t = html_fragment("- [ ] a\n", Path::new("/r"), &found);
        assert!(
            t.contains("<li><input type=\"checkbox\" disabled=\"\" /> a</li>"),
            "{t}"
        );
        assert!(!t.contains("mm-task:"));
        assert!(
            f.starts_with("<meta charset=\"utf-8\"><h1 id=\"hi\">Hi</h1>"),
            "{f}"
        );
        assert!(f.contains("<a href=\"../Sub/A.md\" data-wikilink=\"true\">a</a> g"));
        assert!(f.contains("<pre><code class=\"language-rust\">fn x() {}\n</code></pre>"));
        assert!(f.contains("&lt;script&gt;"));
        for bad in [
            "<script",
            "mm-missing:",
            "/r/",
            "<style",
            "hl-",
            "javascript:",
        ] {
            assert!(!f.contains(bad), "{bad} in {f}");
        }
    }

    #[test]
    fn relative_paths() {
        let r = |a: &str, b: &str| relative(Path::new(a), Path::new(b));
        assert_eq!(r("/r", "/r/a.md"), PathBuf::from("a.md"));
        assert_eq!(r("/r/x/y", "/r/z/a.md"), PathBuf::from("../../z/a.md"));
        assert_eq!(r("/r/x", "/r/x/s/a.md"), PathBuf::from("s/a.md"));
    }

    #[test]
    fn links() {
        let page_uri = "file:///notes/a.md";
        assert_eq!(link_action(page_uri, page_uri), Link::Follow);
        assert_eq!(
            link_action("file:///notes/a.md#fn-1", page_uri),
            Link::Follow
        );
        assert_eq!(link_action("https://example.com", page_uri), Link::External);
        assert_eq!(link_action("HTTP://example.com", page_uri), Link::External);
        assert_eq!(link_action("mailto:a@b.c", page_uri), Link::External);
        let note = |p: &str| Link::Note(PathBuf::from(p));
        assert_eq!(
            link_action("file:///notes/b.md", page_uri),
            note("/notes/b.md")
        );
        assert_eq!(
            link_action("file:///notes/My%20Note.MD#part", page_uri),
            note("/notes/My Note.MD")
        );
        assert_eq!(
            link_action("file://localhost/n/Ghi%20ch%C3%BA.markdown?x", page_uri),
            note("/n/Ghi chú.markdown")
        );
        assert_eq!(link_action("file:///notes/pic.png", page_uri), Link::Ignore);
        assert_eq!(
            link_action("file:///notes/bad%G1.md", page_uri),
            Link::Ignore
        );
        assert_eq!(link_action("file:///etc/passwd", page_uri), Link::Ignore);
        assert_eq!(link_action("about:blank", page_uri), Link::Ignore);
    }

    #[test]
    fn note_in_folder_stays_inside_root() {
        let base = std::env::temp_dir().join(format!("mm-render-{}", std::process::id()));
        let root = base.join("root");
        std::fs::create_dir_all(root.join("Projects")).unwrap();
        std::fs::write(root.join("a.md"), "").unwrap();
        std::fs::write(root.join("Projects/Plan.md"), "").unwrap();
        std::fs::write(base.join("outside.md"), "").unwrap();
        std::os::unix::fs::symlink(base.join("outside.md"), root.join("link.md")).unwrap();
        std::os::unix::fs::symlink(root.join("a.md"), root.join("Projects/in.md")).unwrap();

        let ok = |p: &Path| note_in_folder(p, &root);
        assert_eq!(ok(&root.join("a.md")), Some(root.join("a.md")));
        assert_eq!(
            ok(&root.join("Projects/../Projects/Plan.md")),
            Some(root.join("Projects/Plan.md"))
        );
        assert_eq!(ok(&root.join("Projects/in.md")), Some(root.join("a.md")));
        assert_eq!(ok(&root.join("../outside.md")), None);
        assert_eq!(ok(&root.join("link.md")), None);
        assert_eq!(ok(&root.join("missing.md")), None);
        assert_eq!(ok(&root.join("Projects")), None);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn tags_become_chips() {
        let html = to_html(
            "Hi #work/x and `#code` [#l](u) #1\n\n```\n#block\n```\n\n\u{E001}",
            &|_| None,
        );
        assert!(html.contains("Hi <span class=\"tag\">#work/x</span> and <code>#code</code>"));
        assert!(html.contains("<a href=\"u\">#l</a> #1"));
        assert!(html.contains("#block\n</code>"));
        assert_eq!(html.matches("<span").count(), 1);
        assert!(!html.contains('\u{E001}'));
    }

    #[test]
    fn wiki_links() {
        let found = |t: &str| (t != "Missing").then(|| PathBuf::from(format!("/n/{t}.md")));
        let html = to_html(
            "[[A]] [[A|the text]] [[Folder/A]] `[[Code]]` [[Missing]]\n\n```\n[[Block]]\n```\n",
            &found,
        );
        assert!(html.contains("<a href=\"/n/A.md\" data-wikilink=\"true\">A</a>"));
        assert!(html.contains("<a href=\"/n/A.md\" data-wikilink=\"true\">the text</a>"));
        assert!(html.contains("href=\"/n/Folder/A.md\""));
        assert!(html.contains("<code>[[Code]]</code>"));
        assert!(html.contains("[[Block]]\n</code>"));
        assert!(html.contains("<a href=\"mm-missing:Missing\" data-wikilink=\"true\">Missing</a>"));
    }

    #[test]
    fn wiki_link_special_characters() {
        let html = to_html("[[<b>\"x\" & 100%#?]]", &|_| {
            Some(PathBuf::from("/n/a b&\"<%.md"))
        });
        assert!(html.contains("href=\"/n/a%20b%26%22%3C%25.md\""));
        assert!(html.contains(">&lt;b&gt;&quot;x&quot; &amp; 100%#?</a>"));
        assert!(!html.contains("<b>"));
        let html = to_html("[[a\"><script>|x]]", &|_| None);
        assert!(!html.contains("<script"));
        let href = html
            .split("href=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(
            link_action(href, "file:///n/x.md"),
            Link::Missing("a\"><script>".into())
        );
        assert_eq!(
            link_action("file:///n/a%20b%26%22%3C%25.md", "file:///n/x.md"),
            Link::Note(PathBuf::from("/n/a b&\"<%.md"))
        );
    }

    #[test]
    fn resolve_wiki_names() {
        let root = Path::new("/r");
        let notes: Vec<PathBuf> = [
            "/r/Plan.md",
            "/r/Work/plan.markdown",
            "/r/Work/Deep/Note.md",
            "/r/Other/Note.md",
            "/r/Long/Note.md",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let r = |t: &str, cur: Option<&str>| {
            resolve_wiki(t, &notes, root, cur.map(Path::new)).map(|p| p.to_str().unwrap())
        };
        assert_eq!(r("plan", None), Some("/r/Plan.md"));
        assert_eq!(r("PLAN.md", None), Some("/r/Plan.md"));
        assert_eq!(
            r("plan", Some("/r/Work/x.md")),
            Some("/r/Work/plan.markdown")
        );
        assert_eq!(r("Work/Plan", None), Some("/r/Work/plan.markdown"));
        assert_eq!(r("Note", None), Some("/r/Long/Note.md"));
        assert_eq!(
            r("Note", Some("/r/Work/Deep/y.md")),
            Some("/r/Work/Deep/Note.md")
        );
        assert_eq!(r("Deep/Note#Heading", None), Some("/r/Work/Deep/Note.md"));
        assert_eq!(r("ork/Plan", None), None);
        assert_eq!(r("Nope", None), None);
        assert_eq!(r("", None), None);
    }

    #[test]
    fn front_matter() {
        let md = "---\ntitle: <b>&</b>\ntags: [a]\n---\n# T\n\n- [ ] do\n\n```\nx\n```\n";
        let p = page(md, false, Theme::Default, &|_| None);
        assert!(
            p.contains("<pre class=\"front-matter\">---\ntitle: &lt;b&gt;&amp;&lt;/b&gt;\ntags: [a]\n---</pre>"),
            "{p}"
        );
        assert!(
            p.contains("<h1 id=\"t\" data-sourcepos=\"5:1-5:3\">T</h1>"),
            "{p}"
        );
        assert!(p.contains("href=\"mm-task:7\""), "{p}");
        assert!(
            p.contains("href=\"mm-copy:9\">Copy</a><pre data-sourcepos=\"9:1-11:3\""),
            "{p}"
        );
        assert_eq!(code_block_at(md, 9).as_deref(), Some("x\n"));
        assert_eq!(crate::lists::front_matter_lines(md), 4);
        for out in [
            export_page(md, "t", Path::new("/n"), &|_| None),
            html_fragment(md, Path::new("/n"), &|_| None),
        ] {
            assert!(!out.contains("title:") && !out.contains("<hr"), "{out}");
            assert!(out.contains(">T</h1>"), "{out}");
        }
        // `---` not on line 1: no front matter.
        let p = page("x\n---\ny: 1\n---\n", false, Theme::Default, &|_| None);
        assert!(!p.contains("front-matter\">"), "{p}");
    }

    /// Render time of a ~1MB mixed note (#103). `cargo test --release render_large -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn render_large_note() {
        let chunk = "## Section #tag\n\nSome *text* with **bold**, `code`, a [link](https://x.y) and \
$x^2 + \\frac{a}{b}$ math. [[Other note]] here.\n\n- item one\n- [ ] task\n  1. nested\n\n\
| a | b | c |\n|---|---|---|\n| 1 | 2 | 3 |\n| 4 | 5 | 6 |\n\n\
```rust\nfn main() {\n    let v: Vec<u32> = (0..10).map(|x| x * 2).collect();\n    println!(\"{v:?}\");\n}\n```\n\n\
```python\ndef f(x):\n    return [i ** 2 for i in range(x) if i % 2]\n```\n\n\
```javascript\nconst f = (a, b) => a.map(x => x + b).filter(Boolean);\n```\n\n\
$$\n\\sum_{i=1}^{n} i = \\frac{n(n+1)}{2}\n$$\n\n> quote line\n\n";
        let md = chunk.repeat(1_000_000 / chunk.len() + 1);
        let _ = page("```rust\nx\n```\n", false, Theme::Default, &|_| None); // load syntaxes
        let start = std::time::Instant::now();
        let html = page(&md, false, Theme::Default, &|_| None);
        eprintln!(
            "render {} KB -> {} KB in {:?}",
            md.len() / 1024,
            html.len() / 1024,
            start.elapsed()
        );
    }
}
