//! Markdown → HTML for the preview (pure Rust, no GTK).

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::state::{TextWidth, Theme};
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

/// `to_html`, with syntect code highlighting and formulas marked for KaTeX (`math_html`)
/// only if `rich` (the preview and exports, not Copy as HTML), clickable task checkboxes and
/// code Copy buttons only if `tasks`, and `data-sourcepos="L:C-L:C"` on elements (for scroll
/// sync) only if `sourcepos`.
pub(crate) fn body_html(
    md: &str,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
    rich: bool,
    tasks: bool,
    sourcepos: bool,
) -> String {
    let body = Body {
        rich,
        tasks,
        sourcepos,
        first_line: 1,
        cached: rich,
        next: false,
    };
    render_body(md, wiki, &body).expect("whole note").html
}

/// How `render_body` renders.
pub(crate) struct Body {
    /// See `body_html`.
    pub rich: bool,
    pub tasks: bool,
    pub sourcepos: bool,
    /// The 1-based line of the note `md` starts at: source lines (`data-sourcepos`, task and
    /// Copy links) count from there, and front matter is only read at line 1. A part of a note
    /// cut between top-level blocks renders as that part of the whole note's HTML
    /// (`incremental`), but for heading ids, which count repeats from the note's start (see
    /// `Rendered::headings`).
    pub first_line: usize,
    /// Code highlighting reuses the last cached render's blocks and replaces them with its own
    /// (`HIGHLIGHTED`); otherwise each block is highlighted on the spot, nothing kept.
    pub cached: bool,
    /// The last line of `md` is the first line of the note's next top-level block: parsed, as
    /// it closes the blocks before it (where they end), but not rendered. `render_body` gives
    /// None if it does not start a top-level block there.
    pub next: bool,
}

/// `render_body`'s result.
pub(crate) struct Rendered {
    pub html: String,
    /// The text comrak makes each heading's id from, in order (`Anchorizer::anchorize`, which
    /// adds `-1`, `-2`, … to an id seen before).
    pub headings: Vec<String>,
}

/// `body_html` of `md` as `body` says.
pub(crate) fn render_body(
    md: &str,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
    body: &Body,
) -> Option<Rendered> {
    let Body {
        rich,
        tasks,
        sourcepos,
        first_line,
        cached,
        next,
    } = *body;
    let mut options = options();
    options.render.sourcepos = sourcepos;
    if first_line > 1 {
        options.extension.front_matter_delimiter = None;
    }
    let arena = comrak::Arena::new();
    // Tags are marked with these private-use chars in the AST and turned into spans after
    // escaping; the note's own copies are dropped so it cannot open/close a span.
    let md = md.replace(is_marker, "");
    let root = comrak::parse_document(&arena, &md, &options);
    if next {
        let lines = md.lines().count();
        let last = root.last_child()?;
        if last.data().sourcepos.start.line != lines {
            return None;
        }
        last.detach();
    }
    if first_line > 1 {
        let shift = |p: &mut comrak::nodes::Sourcepos| {
            p.start.line += first_line - 1;
            p.end.line += first_line - 1;
        };
        for node in root.descendants() {
            let mut data = node.data_mut();
            shift(&mut data.sourcepos);
            if let comrak::nodes::NodeValue::TaskItem(item) = &mut data.value {
                shift(&mut item.symbol_sourcepos);
            }
        }
    }
    let mut task_items = Vec::new();
    let mut code_lines = Vec::new();
    let mut front_matter = None;
    let mut formulas = Vec::new();
    let mut code_blocks = Vec::new();
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
        // Formulas become a marker in the AST (raw HTML would be escaped) and their HTML
        // is put back after rendering.
        let formula = match &node.data().value {
            NodeValue::Math(math) => {
                let dollars = if math.display_math { "$$" } else { "$" };
                let html = math_html(&math.literal, math.display_math, dollars, rich);
                Some((html, false))
            }
            // A ```math block (GitHub/GitLab) becomes a paragraph holding the formula, so
            // it keeps `data-sourcepos` and gets no Copy button.
            NodeValue::CodeBlock(block) if block.info.split_whitespace().next() == Some("math") => {
                Some((math_html(block.literal.trim_end(), true, "", rich), true))
            }
            _ => None,
        };
        if let Some((formula, block)) = formula {
            let source = match &node.data().value {
                NodeValue::Math(math) => math.literal.clone(),
                NodeValue::CodeBlock(block) => block.literal.trim_end().to_string(),
                _ => String::new(),
            };
            let index = math_index(formulas.len());
            let text = format!("{MATH}{index}{MATH_TEXT}{source}{MATH_END}");
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
        if let NodeValue::CodeBlock(block) = &node.data().value {
            let diagram = block.fenced && is_diagram(&block.info);
            code_lines.push((node.data().sourcepos.start.line, diagram));
            if rich && cached {
                let lang = block.info.split(|c: char| c.is_ascii_whitespace()).next();
                code_blocks.push((lang_token(lang).to_string(), block.literal.clone()));
            }
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
    let headings = root
        .descendants()
        .filter(|n| matches!(n.data().value, comrak::nodes::NodeValue::Heading(_)))
        .map(|n| n.collect_text())
        .collect();
    let highlighter = Highlighter(if cached {
        highlight_all(code_blocks)
    } else {
        Highlighted::new()
    });
    let mut plugins = comrak::options::Plugins::default();
    if rich {
        plugins.render.codefence_syntax_highlighter = Some(&highlighter);
    }
    let mut html = String::new();
    comrak::format_html_with_plugins(root, &options, &mut html, &plugins)
        .expect("writing to a String");
    if rich && cached {
        *HIGHLIGHTED.lock().unwrap_or_else(PoisonError::into_inner) = highlighter.0;
    }
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
    // chars were dropped, so each `MATH` starts a formula's marker and `MATH_END` ends it.
    let mut out = String::with_capacity(html.len());
    let mut rest = html.as_str();
    while let Some(start) = rest.find(MATH) {
        out.push_str(&rest[..start]);
        let marker = &rest[start + MATH.len_utf8()..];
        let end = marker.find(MATH_END).expect("markers come in pairs");
        let index: String = marker[..end]
            .chars()
            .take_while(|c| MATH_DIGITS.contains(c))
            .map(|c| char::from_u32(c as u32 - *MATH_DIGITS.start() as u32 + '0' as u32).unwrap())
            .collect();
        out.push_str(&formulas[index.parse::<usize>().expect("formula index")]);
        rest = &marker[end + MATH_END.len_utf8()..];
    }
    out.push_str(rest);
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
    // `<pre` is the n-th code block and the next `</pre>` closes it. A ```mermaid block goes
    // into a `<div class="mermaid">` (always: without a script it stays a code block), which
    // takes over its `data-sourcepos`: the diagram drawn there replaces the code (`diagrams`).
    if (tasks || code_lines.iter().any(|&(_, diagram)| diagram)) && !code_lines.is_empty() {
        let mut out = String::with_capacity(html.len() + code_lines.len() * 80);
        let mut done = 0;
        for (line, diagram) in code_lines {
            let Some(pre) = html[done..].find("<pre").map(|i| done + i) else {
                break;
            };
            let close = pre + html[pre..].find("</pre>").expect("closed") + 6;
            out.push_str(&html[done..pre]);
            if diagram {
                let block = &html[pre..close];
                let tag_end = block.find('>').expect("closed tag");
                let attr = " data-sourcepos=\"";
                let pos = block[..tag_end].find(attr).map(|start| {
                    let end =
                        start + attr.len() + block[start + attr.len()..].find('"').expect("quoted");
                    (start, end + 1)
                });
                match pos {
                    Some((start, end)) => {
                        out.push_str(&format!("<div class=\"mermaid\"{}>", &block[start..end]));
                        out.push_str(&block[..start]);
                        out.push_str(&block[end..]);
                    }
                    None => {
                        out.push_str("<div class=\"mermaid\">");
                        out.push_str(block);
                    }
                }
            } else if tasks {
                out.push_str(&format!(
                    "<div class=\"code\"><a class=\"copy\" href=\"{COPY}{line}\">Copy</a>"
                ));
                out.push_str(&html[pre..close]);
            } else {
                out.push_str(&html[pre..close]);
                done = close;
                continue;
            }
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
    Some(Rendered { html, headings })
}

/// Whether a code fence's info string (` ```mermaid `) asks for a Mermaid diagram.
fn is_diagram(info: &str) -> bool {
    info.split_whitespace()
        .next()
        .is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid"))
}

/// Whether a page made by `page` / `export_page` has a Mermaid block, so it needs the
/// diagram script (`diagrams`). The note's text is escaped, so it cannot fake this markup.
pub fn has_diagrams(html: &str) -> bool {
    html.contains("<div class=\"mermaid\"")
}

/// Whether a page made by `page` / `export_page` has a formula, so it needs KaTeX (`math`).
/// The note's text is escaped, so it cannot fake this markup.
pub fn has_math(html: &str) -> bool {
    html.contains("<span class=\"math")
}

/// LaTeX as MathML (WebKit renders it natively, no script), or, if math-core cannot
/// convert it, the source between `delim`s as escaped `<code class="math-error">`. If
/// `katex`, that goes into a `<span class="math[ display]" data-delim data-tex>` holding the
/// source, which KaTeX typesets in the preview and exports (`math`); without a script (or
/// in Copy as HTML, no span) the MathML stays.
fn math_html(latex: &str, display: bool, delim: &str, katex: bool) -> String {
    let html = mathml(latex, display, delim);
    if !katex {
        return html;
    }
    let class = if display { "math display" } else { "math" };
    let mut out = format!("<span class=\"{class}\" data-delim=\"{delim}\" data-tex=\"");
    comrak::html::escape(&mut out, latex).expect("writing to a String");
    out + "\">" + &html + "</span>"
}

/// `math_html`'s MathML (or error) part.
fn mathml(latex: &str, display: bool, delim: &str) -> String {
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
/// Preferences ▸ Line breaks ▸ Every New Line: a single newline inside a paragraph renders
/// as `<br>`. Process-wide because every renderer (preview, exports, print, copy) builds its
/// options here; set at launch and when the preference changes.
static HARD_BREAKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_hard_breaks(on: bool) {
    HARD_BREAKS.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn hard_breaks() -> bool {
    HARD_BREAKS.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn options() -> comrak::Options<'static> {
    options_with(hard_breaks())
}

/// The comrak options of every render; `hard_breaks`: see `HARD_BREAKS`.
fn options_with(hard_breaks: bool) -> comrak::Options<'static> {
    let mut options = comrak::Options::default();
    options.render.hardbreaks = hard_breaks;
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

/// A heading as a link target (Outline ▸ Copy Link, `[[Note#` completion).
#[derive(Debug, PartialEq)]
pub struct HeadingTarget {
    /// 1 (`#`) to 6.
    pub level: u8,
    /// Plain text, whitespace collapsed.
    pub title: String,
    /// What follows `#` in a wiki link to it: its text, which `heading_at` matches, or its id
    /// when an earlier heading has the same text or the text holds `]`/`|` (which would end
    /// the link).
    pub fragment: String,
    /// 1-based lines it spans (two for a Setext heading).
    pub lines: std::ops::RangeInclusive<usize>,
}

/// The headings of `md` in order, outside code blocks and front matter.
pub fn heading_targets(md: &str) -> Vec<HeadingTarget> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &options());
    let mut ids = comrak::Anchorizer::new();
    let found: Vec<_> = root
        .descendants()
        .filter_map(|n| {
            let level = match &n.data().value {
                comrak::nodes::NodeValue::Heading(h) => h.level,
                _ => return None,
            };
            let text = n.collect_text();
            let pos = n.data().sourcepos;
            let title = text.split_whitespace().collect::<Vec<_>>().join(" ");
            Some((
                level,
                title,
                ids.anchorize(&text),
                pos.start.line..=pos.end.line,
            ))
        })
        .collect();
    // `heading_at`'s rule over the same list: by id, then by the text's id.
    let resolve = |want: &str| {
        let text = comrak::Anchorizer::new().anchorize(want);
        [want, text.as_str()]
            .iter()
            .find_map(|w| found.iter().position(|f| f.2 == *w))
    };
    found
        .iter()
        .enumerate()
        .map(|(i, (level, title, id, lines))| {
            let by_text = resolve(title) == Some(i) && !title.contains([']', '|']);
            HeadingTarget {
                level: *level,
                title: title.clone(),
                fragment: if by_text { title.clone() } else { id.clone() },
                lines: lines.clone(),
            }
        })
        .collect()
}

/// The `#fragment` of a wiki link to the heading spanning 1-based `line` of `md` (Outline ▸
/// Copy Link, `HeadingTarget::fragment`). None if no heading spans `line`.
pub fn heading_fragment(md: &str, line: usize) -> Option<String> {
    heading_targets(md)
        .into_iter()
        .find(|h| h.lines.contains(&line))
        .map(|h| h.fragment)
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

/// Highlighted HTML of code blocks by language token, then code.
type Highlighted = HashMap<String, HashMap<String, String>>;

/// The code blocks highlighted by the last render: a re-render after a keystroke highlights
/// only the blocks that changed (highlighting is most of a large note's render time).
static HIGHLIGHTED: LazyLock<Mutex<Highlighted>> = LazyLock::new(Default::default);

/// Code blocks whose language syntect knows get `<span class="hl-…">` per scope (coloured by
/// `CSS`, light and dark); unknown or no language → escaped as before. Class-based so the
/// theme follows the page's light/dark class without re-rendering.
///
/// Writes code blocks from `highlight_all`'s result; a block missing there is highlighted
/// on the spot, so the HTML never depends on what was cached.
struct Highlighter(Highlighted);

/// syntect's bundled syntaxes plus TypeScript and TSX (`resources/syntaxes`, compiled into
/// one dump by its `build-dump.sh`), loaded on the first highlighted code block only (≈ 4 ms).
static SYNTAXES: LazyLock<syntect::parsing::SyntaxSet> = LazyLock::new(|| {
    syntect::dumps::from_binary(include_bytes!("../resources/syntaxes/syntaxes.packdump"))
});

/// The language token syntect looks up for a fence's language (` ```rust,ignore ` → `rust`).
fn lang_token(lang: Option<&str>) -> &str {
    lang.unwrap_or("").split(',').next().unwrap_or("").trim()
}

/// Whether code blocks tagged `token` are highlighted.
#[cfg(test)]
pub fn highlights(token: &str) -> bool {
    SYNTAXES.find_syntax_by_token(token).is_some()
}

/// `code` as highlighted HTML (`hl-*` classes), or escaped when `token` names no known
/// syntax or highlighting fails.
fn highlight_code(token: &str, code: &str) -> String {
    use syntect::html::{ClassStyle, ClassedHTMLGenerator};
    let syntax = (!token.is_empty())
        .then(|| SYNTAXES.find_syntax_by_token(token))
        .flatten();
    let escaped = || {
        let mut out = String::with_capacity(code.len());
        comrak::html::escape(&mut out, code).expect("writing to a String");
        out
    };
    let Some(syntax) = syntax else {
        return escaped();
    };
    let style = ClassStyle::SpacedPrefixed { prefix: "hl-" };
    let mut generator = ClassedHTMLGenerator::new_with_class_style(syntax, &SYNTAXES, style);
    for line in syntect::util::LinesWithEndings::from(code) {
        if generator
            .parse_html_for_line_which_includes_newline(line)
            .is_err()
        {
            return escaped();
        }
    }
    generator.finalize()
}

/// The highlighted HTML of `blocks` (language token, code) with a known syntax: taken from
/// the last render where the same code was highlighted, the rest highlighted in parallel,
/// one thread per core.
fn highlight_all(blocks: Vec<(String, String)>) -> Highlighted {
    let mut blocks: Vec<_> = blocks
        .into_iter()
        .filter(|(token, _)| !token.is_empty() && SYNTAXES.find_syntax_by_token(token).is_some())
        .collect();
    let mut done = Highlighted::new();
    if blocks.is_empty() {
        return done;
    }
    let mut last = std::mem::take(&mut *HIGHLIGHTED.lock().unwrap_or_else(PoisonError::into_inner));
    blocks.sort_unstable();
    blocks.dedup();
    blocks.retain(|(token, code)| {
        let Some(html) = last.get_mut(token).and_then(|codes| codes.remove(code)) else {
            return true;
        };
        let codes = done.entry(token.clone()).or_default();
        codes.insert(code.clone(), html);
        false
    });
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(blocks.len());
    let highlighted: Vec<String> = if threads <= 1 {
        blocks.iter().map(|(t, c)| highlight_code(t, c)).collect()
    } else {
        // Thread `t` takes blocks t, t + threads, …; the results go back in block order.
        let blocks = &blocks;
        let parts: Vec<Vec<String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    scope.spawn(move || {
                        blocks
                            .iter()
                            .skip(t)
                            .step_by(threads)
                            .map(|(token, code)| highlight_code(token, code))
                            .collect()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("highlighting thread"))
                .collect()
        });
        let mut parts: Vec<_> = parts.into_iter().map(Vec::into_iter).collect();
        (0..blocks.len())
            .map(|i| parts[i % threads].next().expect("one result per block"))
            .collect()
    };
    for ((token, code), html) in blocks.into_iter().zip(highlighted) {
        done.entry(token).or_default().insert(code, html);
    }
    done
}

/// Forgets the cached highlighting, so the next render is a cold one (measurements).
#[cfg(test)]
pub(crate) fn forget_highlighting() {
    HIGHLIGHTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

impl comrak::adapters::SyntaxHighlighterAdapter for Highlighter {
    fn write_highlighted(
        &self,
        output: &mut dyn std::fmt::Write,
        lang: Option<&str>,
        code: &str,
    ) -> std::fmt::Result {
        let token = lang_token(lang);
        match self.0.get(token).and_then(|codes| codes.get(code)) {
            Some(html) => output.write_str(html),
            None => output.write_str(&highlight_code(token, code)),
        }
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

/// A formula in the AST: `MATH`, its index in `body_html`'s `formulas` in `MATH_DIGITS`,
/// `MATH_TEXT`, its source, `MATH_END`. The private-use chars drop out of a heading's id, so
/// a heading holding a formula gets the id of its text with the source (as `heading_at`
/// sees it), not one depending on how many formulas come before it.
const MATH: char = '\u{E004}';
const MATH_TEXT: char = '\u{E005}';
const MATH_END: char = '\u{E006}';
/// `'0'`…`'9'` of a formula's index.
const MATH_DIGITS: std::ops::RangeInclusive<char> = '\u{E010}'..='\u{E019}';

/// Whether `c` is one of the private-use chars `body_html` marks the AST with; the note's own
/// copies are dropped before parsing.
pub(crate) fn is_marker(c: char) -> bool {
    matches!(c, TAG_OPEN..=MATH_END) || MATH_DIGITS.contains(&c)
}

/// `n` in `MATH_DIGITS`.
fn math_index(n: usize) -> String {
    n.to_string()
        .chars()
        .map(|d| char::from_u32(*MATH_DIGITS.start() as u32 + (d as u32 - '0' as u32)).unwrap())
        .collect()
}

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
    let target = wiki_key(target)?;
    best_wiki_match(notes.iter(), &target, root, current)
}

/// `target` as `resolve_wiki` compares it: no `#heading`, no outer `/`, no extension, lower
/// case; None when nothing is left.
fn wiki_key(target: &str) -> Option<String> {
    let target = target
        .split('#')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('/');
    let target = strip_note_ext(target).to_lowercase();
    (!target.is_empty()).then_some(target)
}

/// Note `path` as wiki keys are matched against it: relative to `root`, no extension, lower case.
fn note_key(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy();
    strip_note_ext(&rel).to_lowercase()
}

/// The last part of a key (after its last `/`). A note matches a key only when theirs are
/// equal: it is the key itself, or it ends with `/` + the key.
fn last_part(key: &str) -> &str {
    key.rsplit('/').next().unwrap_or(key)
}

fn best_wiki_match<'a>(
    notes: impl Iterator<Item = &'a PathBuf>,
    target: &str,
    root: &Path,
    current: Option<&Path>,
) -> Option<&'a PathBuf> {
    let here = current.and_then(Path::parent);
    let suffix = format!("/{target}");
    notes
        .filter(|p| {
            let rel = note_key(p, root);
            rel == target || rel.ends_with(&suffix)
        })
        .min_by_key(|p| (p.parent() != here, p.as_os_str().len(), p.as_os_str()))
}

/// `resolve_wiki` for many links against the same notes (Backlinks, link updates): the notes
/// grouped by the last part of their name, so a link only compares the notes of its name
/// instead of every note of the folder. Same results as `resolve_wiki`.
#[derive(Debug, Clone, Default)]
pub struct WikiIndex {
    root: PathBuf,
    notes: Vec<PathBuf>,
    by_name: HashMap<String, Vec<usize>>,
}

impl WikiIndex {
    pub fn new(notes: Vec<PathBuf>, root: &Path) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, note) in notes.iter().enumerate() {
            let key = note_key(note, root);
            by_name
                .entry(last_part(&key).to_string())
                .or_default()
                .push(i);
        }
        WikiIndex {
            root: root.to_path_buf(),
            notes,
            by_name,
        }
    }

    pub fn notes(&self) -> &[PathBuf] {
        &self.notes
    }

    pub fn resolve(&self, target: &str, current: Option<&Path>) -> Option<&PathBuf> {
        let target = wiki_key(target)?;
        let found = self.by_name.get(last_part(&target))?;
        best_wiki_match(
            found.iter().map(|&i| &self.notes[i]),
            &target,
            &self.root,
            current,
        )
    }
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

/// Full preview page. The CSP blocks the page's scripts and every network load: only local
/// files (images next to the note), inline styles and `data:` fonts (KaTeX's), plus `http(s)`
/// images if `web_images` (Preferences ▸ Load Images from the Web). Mermaid diagrams and KaTeX
/// formulas are drawn by the app's own script world (`diagrams`, `math`), which the page
/// cannot reach.
pub fn page(
    md: &str,
    dark: bool,
    theme: Theme,
    web_images: bool,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
) -> String {
    preview_page(
        dark,
        theme,
        None,
        web_images,
        &body_html(md, wiki, true, true, true),
    )
}

/// `page` around a body `body_html` (or `incremental`) made, its text at most `width` wide
/// (Full Width: no limit; `None`: the page's own reading width, as exports and print). Without `web_images`, a page
/// showing a web image says on screen (not in print) why it is missing.
pub(crate) fn preview_page(
    dark: bool,
    theme: Theme,
    width: Option<TextWidth>,
    web_images: bool,
    body: &str,
) -> String {
    let (csp, hint) = if web_images {
        (WEB_IMAGES_CSP, "")
    } else {
        (PREVIEW_CSP, WEB_IMAGES_HINT)
    };
    html_page(
        dark,
        theme,
        csp,
        &format!("{}{hint}", width_css(width)),
        body,
    )
}

/// The preview's CSP: local and `data:` images only, no script, nothing from the network.
pub(crate) const PREVIEW_CSP: &str =
    "default-src 'none'; img-src file: data:; style-src 'unsafe-inline'; font-src data:";
/// `PREVIEW_CSP` with web images allowed (and nothing else from the network).
const WEB_IMAGES_CSP: &str = "default-src 'none'; img-src file: data: http: https:; \
style-src 'unsafe-inline'; font-src data:";
/// Export as HTML: images from anywhere when the file is opened elsewhere.
pub(crate) const EXPORT_CSP: &str =
    "default-src 'none'; img-src * data: file:; style-src 'unsafe-inline'; font-src data:";

/// A line above the page while it has a web image that is not loaded (a CSS rule, so it
/// follows the page as it is patched while typing). `:has` needs WebKitGTK 2.42+; older ones
/// drop the rule and show the broken image alone.
const WEB_IMAGES_HINT: &str = "<style>@media screen { body:has(img[src^=\"http:\" i], \
img[src^=\"https:\" i])::before { content: \"Web images are not loaded \\2014  Preferences \\25B8  \
Load Images from the Web\"; display: block; margin-bottom: 1em; font-size: 0.8em; \
opacity: 0.6; } }</style>";

/// The `<style>` that sets the preview's text width: the pixels of Narrow / Medium / Wide, no
/// limit for Full Width (the padding stays); empty for `None`. Placed before the page's CSS,
/// so it is more specific than its `body` rule; print keeps the whole page.
fn width_css(width: Option<TextWidth>) -> String {
    let Some(width) = width else {
        return String::new();
    };
    let max = match width.pixels() {
        Some(px) => format!("{px}px"),
        None => "none".to_string(),
    };
    format!("<style>@media screen {{ html body {{ max-width: {max}; }} }}</style>")
}

/// Standalone HTML file of a note (Export as HTML): the preview's CSS in light mode, code
/// highlighting, formulas marked for KaTeX (`math_html`), no script. Meant to sit next to the note (`note_dir`): relative images and
/// `.md` links stay relative, wiki links become relative links to the target note (no local
/// absolute paths in the file), unresolved wiki links become plain text. Images may load
/// from the web when the file is opened elsewhere (`EXPORT_CSP`).
pub fn export_page(
    md: &str,
    title: &str,
    note_dir: &Path,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
) -> String {
    let body = portable_body(md, note_dir, wiki, true);
    let mut escaped = String::new();
    comrak::html::escape(&mut escaped, title).expect("writing to a String");
    html_page(
        false,
        Theme::Default,
        EXPORT_CSP,
        &format!("<title>{escaped}</title>"),
        &body,
    )
}

/// Copy as HTML: the note's (or selection's) HTML for pasting into mail/docs, with the same
/// links as `export_page` relative to `note_dir`. No page CSS or script; code is plain
/// `<pre><code>` (the `hl-*` classes mean nothing without our CSS), formulas MathML (no
/// KaTeX), tags plain `#tag` text in a classed span. The `<meta charset>` makes browsers/editors read the clipboard as UTF-8.
pub fn html_fragment(md: &str, note_dir: &Path, wiki: &dyn Fn(&str) -> Option<PathBuf>) -> String {
    format!(
        "<meta charset=\"utf-8\">{}",
        portable_body(md, note_dir, wiki, false)
    )
}

/// Copy as Rich Text: the note's (or selection's) HTML as a whole document for word
/// processors and mail (LibreOffice, Thunderbird, web mail), which paste it formatted. The body
/// is Copy as HTML's (links, MathML, diagrams as code) plus a small style sheet (table borders,
/// monospace code) those apps read; `<img>`s go through `inline_images` with `image`.
pub fn rich_text_page(
    md: &str,
    note_dir: &Path,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
    image: &dyn Fn(&str) -> Option<String>,
) -> String {
    let body = inline_images(&portable_body(md, note_dir, wiki, false), image);
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
table {{ border-collapse: collapse; }} th, td {{ border: 1px solid #999; padding: 2px 6px; }} \
pre, code {{ font-family: monospace; }}</style></head><body>{body}</body></html>"
    )
}

/// `body` with each `<img>` whose `src` is neither a `data:` nor a web URL replaced: its `src`
/// (entities decoded) becomes `image(src)` (a `data:` URL), or the tag is dropped when that
/// gives None. A pasted relative path points nowhere; web images stay for the target app.
fn inline_images(body: &str, image: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find("<img ") {
        out.push_str(&rest[..start]);
        let Some(len) = rest[start..].find('>') else {
            rest = &rest[start..];
            break;
        };
        let tag = &rest[start..=start + len];
        rest = &rest[start + len + 1..];
        let Some((before, src, after)) = tag
            .split_once(" src=\"")
            .and_then(|(b, r)| r.split_once('"').map(|(s, a)| (b, s, a)))
        else {
            continue;
        };
        let src = src
            .replace("&quot;", "\"")
            .replace("&#x27;", "'")
            .replace("&#39;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        let lower = src.to_ascii_lowercase();
        if ["data:", "http://", "https://"]
            .iter()
            .any(|p| lower.starts_with(p))
        {
            out.push_str(tag);
        } else if let Some(data) = image(&src) {
            out.push_str(&format!("{before} src=\"{data}\"{after}"));
        }
    }
    out.push_str(rest);
    out
}

/// `body_html` with wiki links relative to `note_dir` and unresolved ones as plain text.
fn portable_body(
    md: &str,
    note_dir: &Path,
    wiki: &dyn Fn(&str) -> Option<PathBuf>,
    rich: bool,
) -> String {
    let wiki = |t: &str| wiki(t).map(|p| relative(note_dir, &p));
    let mut body = body_html(md, &wiki, rich, false, false);
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
div.mermaid { margin: 1em 0; } div.mermaid > pre { margin: 0; }
div.mermaid.drawn > pre { display: none; }
div.mermaid .diagram { overflow-x: auto; text-align: center; }
div.mermaid .diagram svg { max-width: 100%; height: auto; }
p.mermaid-error { margin: .3em 0 0; font-size: .85em; color: var(--missing); white-space: pre-wrap; }
code.math-error { color: var(--missing); }
.katex-display { overflow-x: auto; overflow-y: hidden; }
@media print { html { background: none; } body { max-width: none; padding: 0; } pre, table, img, blockquote, div.mermaid, .katex-display { break-inside: avoid; } a.copy, pre.front-matter { display: none; } }
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
    fn heading_targets_text_or_id() {
        // Repeated titles: the first by text, the next by id; `]` / `|` force the id; a title
        // equal to an earlier heading's id takes the id too.
        let md = "# Plan\n\n## Plan\n\n### a ] b\n\n# plan-1\n\n```\n# not\n```\n";
        let got: Vec<_> = heading_targets(md)
            .into_iter()
            .map(|h| (h.level, h.title, h.fragment))
            .collect();
        assert_eq!(
            got,
            [
                (1, "Plan".to_string(), "Plan".to_string()),
                (2, "Plan".to_string(), "plan-1".to_string()),
                (3, "a ] b".to_string(), "a--b".to_string()),
                (1, "plan-1".to_string(), "plan-1-1".to_string()),
            ]
        );
        for h in heading_targets(md) {
            let line = *h.lines.start();
            assert_eq!(heading_at(md, &h.fragment).map(|(l, _)| l), Some(line));
        }
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
            html.contains("a <span class=\"math\" data-delim=\"$\" data-tex=\"x^2\"><math><msup><mi>x</mi><mn>2</mn></msup></math></span> b"),
            "{html}"
        );
        assert!(
            html.contains("<span class=\"math display\" data-delim=\"$$\" data-tex=\"\\frac{a}{b}\"><math display=\"block\"><mfrac><mi>a</mi><mi>b</mi></mfrac></math></span>"),
            "{html}"
        );
        let html = to_html("$\\sum_{i=1}^n \\sqrt{i}$ $a<b$", &|_| None);
        assert!(
            html.contains("<mo lspace=\"0\">∑</mo>") && html.contains("<msqrt>"),
            "{html}"
        );
        assert!(html.contains("<mo>&lt;</mo>"), "{html}");
        // Bad LaTeX: the source, escaped (also for KaTeX); no raw HTML gets through.
        let html = to_html("$\\frac{a$ and $$\\nope<b>$$", &|_| None);
        assert!(
            html.contains(
                "data-tex=\"\\frac{a\"><code class=\"math-error\">$\\frac{a$</code></span>"
            ),
            "{html}"
        );
        assert!(
            html.contains("data-tex=\"\\nope&lt;b&gt;\"><code class=\"math-error\">$$\\nope&lt;b&gt;$$</code></span>"),
            "{html}"
        );
        assert!(!html.contains("<b>"), "{html}");
        // ```math blocks: block MathML in a paragraph that keeps its sourcepos, no Copy
        // button; bad LaTeX shows the block's source; other code blocks are unchanged.
        let md = "```math\n\\frac{a}{b}\n```\n\n```math\n\\frac{a\n```\n\n```\nx\n```\n";
        let html = to_html(md, &|_| None);
        assert!(
            html.starts_with(
                "<p><span class=\"math display\" data-delim=\"\" data-tex=\"\\frac{a}{b}\"><math display=\"block\"><mfrac><mi>a</mi><mi>b</mi></mfrac></math></span></p>"
            ),
            "{html}"
        );
        assert!(
            html.contains("<code class=\"math-error\">\\frac{a</code></span></p>"),
            "{html}"
        );
        assert!(html.contains("<pre><code>x\n</code></pre>"), "{html}");
        let p = page(md, false, Theme::Default, false, &|_| None);
        assert!(
            p.contains("<p data-sourcepos=\"1:1-3:3\"><span class=\"math display\""),
            "{p}"
        );
        assert!(p.contains("href=\"mm-copy:9\""), "{p}");
        assert_eq!(p.matches("mm-copy:").count(), 1, "{p}");
        // Prices are text; the note's own marker chars are dropped.
        assert_eq!(
            to_html("$5 and $10\u{E004}0\u{E004}", &|_| None),
            "<p>$5 and $100</p>\n"
        );
        // The preview and Export mark formulas for KaTeX around the MathML (kept as the
        // fallback); Copy as HTML has the MathML only. The preview keeps `data-sourcepos`.
        let md = "# T\n\n$x$\n";
        let mathml = "<math><mi>x</mi></math>";
        let marked =
            format!("<span class=\"math\" data-delim=\"$\" data-tex=\"x\">{mathml}</span>");
        let copy = html_fragment(md, Path::new("/r"), &|_| None);
        assert!(copy.contains(mathml) && !has_math(&copy), "{copy}");
        let export = export_page(md, "t", Path::new("/r"), &|_| None);
        assert!(export.contains(&marked) && has_math(&export), "{export}");
        assert!(export.contains("font-src data:"), "{export}");
        let p = page(md, false, Theme::Default, false, &|_| None);
        assert!(
            p.contains(&format!("<p data-sourcepos=\"3:1-3:3\">{marked}</p>")),
            "{p}"
        );
        assert!(has_math(&p) && p.contains("font-src data:"), "{p}");
        // No formula, no KaTeX: prices, code, raw HTML that looks like the markup.
        for md in [
            "$5 and $10\n",
            "`$x$`\n",
            "<span class=\"math\" data-tex=\"x\">x</span>\n",
        ] {
            assert!(
                !has_math(&page(md, false, Theme::Default, false, &|_| None)),
                "{md}"
            );
        }
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
    fn mermaid_blocks() {
        let md = "# D\n\n```mermaid\nflowchart LR\n  A --> B<C>\n```\n\n```rust\nfn x() {}\n```\n\n- item\n\n  ~~~ Mermaid\n  pie\n  ~~~\n";
        let p = page(md, false, Theme::Default, false, &|_| None);
        assert!(has_diagrams(&p));
        assert!(
            p.contains("<div class=\"mermaid\" data-sourcepos=\"3:1-6:3\"><pre><code class=\"language-mermaid\">flowchart LR\n  A --&gt; B&lt;C&gt;\n</code></pre></div>"),
            "{p}"
        );
        assert!(
            p.contains("<div class=\"mermaid\" data-sourcepos=\"14:3-16:5\"><pre>"),
            "{p}"
        );
        assert_eq!(p.matches("<div class=\"mermaid\"").count(), 2);
        // Other code keeps its Copy button; a diagram gets none.
        assert_eq!(p.matches("mm-copy:").count(), 1);
        assert!(p.contains("href=\"mm-copy:8\">Copy</a><pre data-sourcepos=\"8:1-10:3\""));
        // Export and Copy as HTML: the same block (a code block without the script).
        let export = export_page(md, "D", Path::new("/r"), &|_| None);
        assert!(
            export
                .contains("<div class=\"mermaid\"><pre><code class=\"language-mermaid\">flowchart")
        );
        assert!(has_diagrams(&export) && !export.contains("mm-copy:"));
        let copy = html_fragment(md, Path::new("/r"), &|_| None);
        assert!(
            copy.contains("<div class=\"mermaid\"><pre><code class=\"language-mermaid\">flowchart")
        );
        // Not a diagram: other languages, an indented block, the word in text, raw HTML.
        for md in [
            "```rust\nmermaid\n```\n",
            "    mermaid\n",
            "mermaid and `mermaid`\n",
            "<div class=\"mermaid\">x</div>\n",
            "```mermaidx\nx\n```\n",
        ] {
            assert!(
                !has_diagrams(&page(md, false, Theme::Default, false, &|_| None)),
                "{md}"
            );
        }
    }

    #[test]
    fn page_blocks_scripts_and_network() {
        let p = page("# a", true, Theme::Default, false, &|_| None);
        assert!(p.contains("default-src 'none'"));
        assert!(p.contains("class=\"dark\""));
        assert!(
            p.contains("<h1 id=\"a\" data-sourcepos=\"1:1-1:3\">a</h1>"),
            "{p}"
        );
        // Scroll sync markup keeps the task and Copy links working.
        let p = page(
            "- [ ] t\n\n```\nx\n```\n",
            false,
            Theme::Default,
            false,
            &|_| None,
        );
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
                assert!(page("a", dark, theme, false, &|_| None).contains(&css));
            }
        }
        // Mọi biến mà CSS trang dùng đều được theme đặt.
        for used in CSS.split("var(--").skip(1) {
            let name = &used[..used.find(')').unwrap()];
            assert!(VARS.contains(&name), "{name}");
        }
    }

    #[test]
    fn preview_text_width() {
        let page = |w| preview_page(false, Theme::Default, w, false, "<p>a</p>");
        let narrow = page(Some(TextWidth::Narrow));
        assert!(
            narrow.contains("<style>@media screen { html body { max-width: 680px; } }</style>")
        );
        // Full Width: no limit, so it is wider than Wide; the page's own CSS keeps its padding.
        let full = page(Some(TextWidth::Full));
        assert!(full.contains("<style>@media screen { html body { max-width: none; } }</style>"));
        // No width (print, exports): the page's own reading width.
        let own = page(None);
        assert!(!own.contains("html body") && own.contains("max-width: 46em"));
    }

    #[test]
    fn hard_breaks_only_when_asked() {
        let html = |md: &str, on: bool| comrak::markdown_to_html(md, &options_with(on));
        let note = "TITLE\nURL\n\nTEXT\n";
        assert_eq!(html(note, false), "<p>TITLE\nURL</p>\n<p>TEXT</p>\n");
        assert_eq!(html(note, true), "<p>TITLE<br />\nURL</p>\n<p>TEXT</p>\n");
        // Hard breaks, code, tables and HTML blocks are the same either way.
        for md in [
            "a  \nb\\\nc\n",
            "```\nx\ny\n```\n",
            "| a |\n|---|\n| b |\n",
            "<div>\nx\ny\n</div>\n",
        ] {
            assert_eq!(html(md, false), html(md, true), "{md}");
        }
    }

    #[test]
    fn web_images_only_when_asked() {
        let md = "![w](https://x.org/w.png) ![l](assets/l.png)";
        let off = page(md, false, Theme::Default, false, &|_| None);
        assert!(
            off.contains(PREVIEW_CSP) && !off.contains("http: https:"),
            "{off}"
        );
        assert!(off.contains("Web images are not loaded"), "{off}");
        let on = page(md, false, Theme::Default, true, &|_| None);
        assert!(on.contains("img-src file: data: http: https:;"), "{on}");
        assert!(
            on.contains("default-src 'none';") && !on.contains("script-src"),
            "{on}"
        );
        assert!(!on.contains("Web images are not loaded"), "{on}");
        // Same body either way; only the head differs.
        let body = |p: &str| p[p.find("<body>").unwrap()..].to_string();
        assert_eq!(body(&off), body(&on));
        let export = export_page(md, "t", Path::new("/n"), &|_| None);
        assert!(export.contains(EXPORT_CSP), "{export}");
        assert_eq!(export.matches(EXPORT_CSP).count(), 1);
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
    fn rich_text_document() {
        let found = |t: &str| (t == "A").then(|| PathBuf::from("/r/A.md"));
        let asked = std::cell::RefCell::new(Vec::new());
        let image = |src: &str| {
            asked.borrow_mut().push(src.to_string());
            (src != "gone.png").then(|| "data:image/png;base64,Zm9v".to_string())
        };
        let md = "# Title\n\n**bold** *it* [web](https://x.org) [[A]]\n\n- one\n- two\n\n\
| a | b |\n|---|---|\n| 1 | 2 |\n\n$x^2$\n\n```mermaid\ngraph TD\n```\n\n\
![p](assets/my%20p.png?a&b) ![g](gone.png) ![w](https://x.org/w.png)\n";
        let p = rich_text_page(md, Path::new("/r"), &found, &image);
        assert!(
            p.starts_with("<!doctype html><html><head><meta charset=\"utf-8\"><style>"),
            "{p}"
        );
        assert!(p.ends_with("</body></html>"));
        for want in [
            "<h1 id=\"title\">Title</h1>",
            "<strong>bold</strong>",
            "<em>it</em>",
            "<a href=\"https://x.org\">web</a>",
            "<a href=\"A.md\"",
            "<li>one</li>",
            "<table>",
            "<td>1</td>",
            "<math",
            "graph TD",
            "border-collapse",
            "<img src=\"data:image/png;base64,Zm9v\" alt=\"p\"",
            "<img src=\"https://x.org/w.png\" alt=\"w\"",
        ] {
            assert!(p.contains(want), "{want} missing in {p}");
        }
        assert!(!p.contains("gone.png") && !p.contains("alt=\"g\""), "{p}");
        assert!(!p.contains("<script") && !p.contains("class=\"math"), "{p}");
        // The source as written, entities decoded; web images not asked for.
        assert_eq!(*asked.borrow(), ["assets/my%20p.png?a&b", "gone.png"]);
    }

    #[test]
    fn inline_images_keeps_text_around() {
        let none = |_: &str| None;
        assert_eq!(inline_images("a <img src=\"x.png\" /> b", &none), "a  b");
        assert_eq!(inline_images("no images", &none), "no images");
        assert_eq!(inline_images("cut <img src=\"x", &none), "cut <img src=\"x");
        let data = |_: &str| Some("data:image/gif;base64,R0".to_string());
        assert_eq!(
            inline_images("<p><img src=\"a.gif\" alt=\"A\" /></p>", &data),
            "<p><img src=\"data:image/gif;base64,R0\" alt=\"A\" /></p>"
        );
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
    fn wiki_index_resolves_like_resolve_wiki() {
        let root = Path::new("/r");
        let notes: Vec<PathBuf> = [
            "/r/Plan.md",
            "/r/Work/plan.markdown",
            "/r/Work/Deep/Note.md",
            "/r/Other/Note.md",
            "/r/Long/Note.md",
            "/r/a/b.md",
            "/r/x/a/b.md",
            "/r/Ünïcode/ΣΊΣΥΦΟΣ.md",
            "/elsewhere/Note.md",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let index = WikiIndex::new(notes.clone(), root);
        let targets = [
            "plan",
            "PLAN.md",
            "Work/Plan",
            "Note",
            "Deep/Note#Heading",
            "ork/Plan",
            "Nope",
            "",
            "#Heading",
            "/Note/",
            "b",
            "a/b",
            "x/a/b",
            "y/a/b",
            "σίσυφος",
            "ünïcode/ΣΊΣΥΦΟΣ.md",
            "elsewhere/Note",
            "Note.markdown",
            "a/",
            "/",
        ];
        let currents = [
            None,
            Some("/r/Work/x.md"),
            Some("/r/x/a/c.md"),
            Some("/r/Other/n.md"),
        ];
        for t in targets {
            for cur in currents {
                let cur = cur.map(Path::new);
                assert_eq!(
                    index.resolve(t, cur),
                    resolve_wiki(t, &notes, root, cur),
                    "{t:?} from {cur:?}"
                );
            }
        }
        assert_eq!(index.notes(), &notes[..]);
    }

    #[test]
    fn front_matter() {
        let md = "---\ntitle: <b>&</b>\ntags: [a]\n---\n# T\n\n- [ ] do\n\n```\nx\n```\n";
        let p = page(md, false, Theme::Default, false, &|_| None);
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
        let p = page("x\n---\ny: 1\n---\n", false, Theme::Default, false, &|_| {
            None
        });
        assert!(!p.contains("front-matter\">"), "{p}");
    }

    #[test]
    fn highlighting_is_the_same_cached_or_not() {
        let md = "```rust\nfn a() {}\n```\n\n```python\nx = 1\n```\n\n\
```rust\nfn a() {}\n```\n\n```nosuchlang\n<b>\n```\n\n    indented\n";
        forget_highlighting();
        let cold = to_html(md, &|_| None);
        assert!(cold.contains("hl-") && cold.contains("&lt;b&gt;"));
        assert_eq!(to_html(md, &|_| None), cold);
        let edited = md.replace("x = 1", "x = 2");
        let warm = to_html(&edited, &|_| None);
        forget_highlighting();
        assert_eq!(to_html(&edited, &|_| None), warm);
        assert_ne!(warm, cold);
    }

    #[test]
    fn typescript_is_highlighted() {
        let code = "interface A<T> { x: readonly T[] }\nconst f = (a: A<number>): void => {};\n";
        for token in ["ts", "typescript", "tsx", "TypeScript"] {
            assert!(highlights(token), "{token}");
            let html = highlight_code(token, code);
            assert!(html.contains("<span class=\"hl-storage"), "{token}: {html}");
        }
        let tsx = highlight_code("tsx", "const e = <div className=\"x\">{n}</div>;\n");
        assert!(tsx.contains("hl-tag"), "{tsx}");
    }

    #[test]
    fn parallel_highlighting_keeps_each_block() {
        let blocks: Vec<_> = (0..50)
            .map(|i| {
                let token = ["rust", "python", "js"][i % 3].to_string();
                (token, format!("let x{i} = \"{i}\";\n"))
            })
            .collect();
        let done = highlight_all(blocks.clone());
        for (token, code) in blocks {
            assert_eq!(done[&token][&code], highlight_code(&token, &code));
        }
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
        let _ = page("```rust\nx\n```\n", false, Theme::Default, false, &|_| None); // load syntaxes
        let start = std::time::Instant::now();
        let html = page(&md, false, Theme::Default, false, &|_| None);
        eprintln!(
            "render {} KB -> {} KB in {:?}",
            md.len() / 1024,
            html.len() / 1024,
            start.elapsed()
        );
    }
}
