//! Markdown → HTML for the preview (pure Rust, no GTK).

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use crate::aliases::{self, Aliases};
use crate::state::{FrontMatter, TextWidth, Theme};
use crate::{state, tags};

/// Markdown body as HTML: GFM (tables, task lists, strikethrough, autolinks, footnotes).
/// Raw HTML in the note is escaped (shown as text), never rendered — notes may come from
/// anywhere (git, sync), so the preview is safe by default. `javascript:` links are dropped.
/// `[[Note]]` / `[[Note|text]]` (not in code) link to `wiki(target)`'s note, or to
/// `MISSING` + the target when it returns None. Tags (`tags::in_line`, not in code or
/// links) become `<span class="tag">`. Task checkboxes are wrapped in a
/// `<a class="task" href="mm-task:N">`, N the 1-based source line of the item's `[ ]`.
/// Code blocks are wrapped in `<div class="code">` with a `<a class="copy" href="mm-copy:N">`
/// button, N the block's first source line (see `code_block_at`). A `[TOC]` paragraph
/// lists the note's headings (`toc_list`). The preview (`page`) adds `data-sourcepos`; this
/// test-only form leaves it out to keep the expected HTML short.
#[cfg(test)]
fn to_html(md: &str, wiki: &dyn Fn(&str) -> Option<PathBuf>) -> String {
    body_html(md, &wiki, true, true, false, true)
}

/// `to_html`, with syntect code highlighting and formulas marked for KaTeX (`math_html`)
/// only if `rich` (the preview and exports, not Copy as HTML), clickable task checkboxes and
/// code Copy buttons and the front matter (as `front_matter()` says) only if `tasks`,
/// `data-sourcepos="L:C-L:C"` on elements (for scroll sync) only if `sourcepos`, and `[TOC]`
/// as the table of contents only if `toc` (not in Rich Text, which keeps it as written).
pub(crate) fn body_html(
    md: &str,
    wiki: &dyn Links,
    rich: bool,
    tasks: bool,
    sourcepos: bool,
    toc: bool,
) -> String {
    body_html_with(
        md,
        wiki,
        rich,
        tasks,
        sourcepos,
        toc,
        rich,
        crate::smart::enabled(),
        tasks.then(front_matter),
    )
}

/// `body_html`, with code highlighting through the preview's cache only if `cached` (see
/// `Body::cached`), smart punctuation only if `smart` and the front matter as `front_matter`
/// says (None: left out).
#[allow(clippy::too_many_arguments)]
fn body_html_with(
    md: &str,
    wiki: &dyn Links,
    rich: bool,
    tasks: bool,
    sourcepos: bool,
    toc: bool,
    cached: bool,
    smart: bool,
    front_matter: Option<FrontMatter>,
) -> String {
    let body = Body {
        rich,
        tasks,
        sourcepos,
        toc,
        first_line: 1,
        cached,
        next: false,
        smart,
        front_matter,
    };
    let rendered = render_body(md, wiki, &body).expect("whole note");
    if !rendered.toc {
        return rendered.html;
    }
    let mut anchorizer = comrak::Anchorizer::new();
    let ids: Vec<String> = rendered
        .headings
        .iter()
        .map(|h| anchorizer.anchorize(h))
        .collect();
    let list = toc_list(
        rendered
            .titles
            .iter()
            .zip(&ids)
            .map(|((level, title), id)| (*level, title.as_str(), id.as_str())),
    );
    fill_toc(&rendered.html, &list)
}

/// How `render_body` renders.
pub(crate) struct Body {
    /// See `body_html`.
    pub rich: bool,
    pub tasks: bool,
    pub sourcepos: bool,
    pub toc: bool,
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
    /// Smart punctuation (`smart`): curly quotes, dashes and ellipses in the text. Heading ids
    /// stay those of the text as written.
    pub smart: bool,
    /// How the front matter shows before the first block (`frontmatter::html`); None: not at all.
    pub front_matter: Option<FrontMatter>,
}

/// `render_body`'s result.
pub(crate) struct Rendered {
    pub html: String,
    /// The text comrak makes each heading's id from, in order (`Anchorizer::anchorize`, which
    /// adds `-1`, `-2`, … to an id seen before).
    pub headings: Vec<String>,
    /// Each heading's level and its title in a table of contents (`heading_title`), in order.
    pub titles: Vec<(u8, String)>,
    /// Whether `html` holds a `[TOC]` placeholder for `fill_toc`.
    pub toc: bool,
}

/// `body_html` of `md` as `body` says.
pub(crate) fn render_body(md: &str, wiki: &dyn Links, body: &Body) -> Option<Rendered> {
    let Body {
        rich,
        tasks,
        sourcepos,
        toc,
        first_line,
        cached,
        next,
        smart,
        front_matter: front_matter_mode,
    } = *body;
    let mut options = options();
    options.render.sourcepos = sourcepos;
    if smart {
        crate::smart::prepare(&mut options);
    }
    if first_line > 1 {
        options.extension.front_matter_delimiter = None;
    }
    let arena = comrak::Arena::new();
    // Tags are marked with these private-use chars in the AST and turned into spans after
    // escaping; the note's own copies are dropped so it cannot open/close a span.
    let md = md.replace(is_marker, "");
    let md = strip_comments(&md);
    let root = comrak::parse_document(&arena, &md, &options);
    unmark_multiline(&arena, root);
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
    // Heading ids come from the text as written (links to `#a--b` keep working): taken here,
    // before smart punctuation, and written over comrak's ids after rendering.
    let written_headings = smart.then(|| heading_texts(root));
    // Before smart punctuation: embeds are read as written.
    let embeds = mark_embeds(&arena, root, wiki, body);
    if smart {
        crate::smart::apply(root);
    }
    let quotes: Vec<_> = root
        .descendants()
        .filter(|n| matches!(n.data().value, comrak::nodes::NodeValue::BlockQuote))
        .collect();
    // Every quote is tried (`count`, not `any`).
    let alerts = quotes.into_iter().filter(|quote| make_alert(quote)).count() > 0;
    // A `[TOC]` paragraph becomes a placeholder, `<p …>TOC</p>`, that `fill_toc` replaces once
    // the note's headings are known.
    let mut has_toc = false;
    for paragraph in root.children().filter(|_| toc) {
        if is_toc_marker(paragraph) {
            while let Some(child) = paragraph.first_child() {
                child.detach();
            }
            let text = comrak::nodes::NodeValue::Text(TOC.to_string().into());
            paragraph.append(arena.alloc(text.into()));
            has_toc = true;
        }
    }
    let mut titles = Vec::new();
    let mut task_items = Vec::new();
    let mut code_lines = Vec::new();
    let mut front_matter = None;
    let mut formulas = Vec::new();
    let mut code_blocks = Vec::new();
    let base = wiki
        .base()
        .map(|b| percent_encode(b.as_os_str().as_bytes()))
        .filter(|b| !b.is_empty());
    for node in root.descendants() {
        use comrak::nodes::NodeValue;
        // Before its text is marked below (tags, formulas): its descendants come after it.
        if let NodeValue::Heading(heading) = &node.data().value {
            titles.push((heading.level, heading_title(node)));
        }
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
            link.url = wiki_href(wiki, &link.url);
        }
        // An embedded note's relative links and images, made relative to the page.
        if let Some(base) = &base
            && let NodeValue::Link(link) | NodeValue::Image(link) = &mut node.data_mut().value
            && is_relative(&link.url)
        {
            link.url = format!("{base}/{}", link.url);
        }
    }
    // The renderer writes a task item's checkbox right before its children: a marker
    // child carrying the line tells the post-processing below which checkbox is which.
    for (node, line) in task_items.into_iter().filter(|_| tasks) {
        let text = format!("{TASK_OPEN}{line}{TASK_CLOSE}");
        node.prepend(arena.alloc(comrak::nodes::NodeValue::Text(text.into()).into()));
    }
    let headings = written_headings.unwrap_or_else(|| heading_texts(root));
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
    if smart && !headings.is_empty() {
        let mut anchorizer = comrak::Anchorizer::new();
        let ids: Vec<String> = headings.iter().map(|h| anchorizer.anchorize(h)).collect();
        html = crate::incremental::replace_ids(&html, &ids);
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
    // comrak's alert tags → ours (`alert_tags`).
    let html = if alerts { alert_tags(&out) } else { out };
    let html = html
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
    // Front matter (comrak renders nothing for it) is shown in the preview and Export Folder
    // as HTML only, after the code-block pass above, which counts `<pre`s.
    if let (Some(text), Some(mode)) = (front_matter, front_matter_mode) {
        html.insert_str(0, &crate::frontmatter::html(&text, mode, sourcepos));
    }
    // Last: an embedded note's HTML holds headings, code blocks and formulas of its own.
    if !embeds.is_empty() {
        html = put_embeds(&html, &embeds);
    }
    Some(Rendered {
        html,
        headings,
        titles,
        toc: has_toc,
    })
}

/// The `href` of a wiki link to `target`: the note's path (`Links::note`) with the heading's id
/// for `[[Note#Heading]]` (see `heading_at`), the id alone for `[[#Heading]]`, or `MISSING` +
/// the target when no note has that name. An absolute path (no `file:` scheme, which comrak
/// drops) resolves against the note's file:// URI and comes back to `link_action` as a
/// `Link::Note`.
fn wiki_href(links: &dyn Links, target: &str) -> String {
    let anchor = target
        .split_once('#')
        .map(|(_, heading)| format!("#{}", comrak::Anchorizer::new().anchorize(heading)));
    let anchor = anchor.filter(|a| a.len() > 1);
    match (links.note(target), anchor) {
        (_, Some(anchor)) if target.starts_with('#') => anchor,
        (Some(path), anchor) => {
            percent_encode(path.as_os_str().as_bytes()) + anchor.as_deref().unwrap_or("")
        }
        (None, _) => format!("{MISSING}{}", percent_encode(target.as_bytes())),
    }
}

/// Whether a link's URL is relative to the note's folder: no scheme, not absolute, not `#…`.
fn is_relative(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let scheme = path.find(':').is_some_and(|c| !path[..c].contains('/'));
    !path.is_empty() && !path.starts_with(['/', '~']) && !scheme
}

/// What an embed becomes: its HTML and, for a rendered note, the attributes of the
/// `<div class="embed">` around it.
struct Embed {
    html: String,
    block: Option<String>,
}

/// Replaces each `![[…]]` in the text of `root` (not in code, links or headings) with a marker
/// `put_embeds` turns into its HTML. A note rendered inline gets a paragraph of its own. Nothing
/// when `links` leaves embeds as text.
fn mark_embeds<'a>(
    arena: &'a comrak::Arena<'a>,
    root: &'a comrak::nodes::AstNode<'a>,
    links: &dyn Links,
    body: &Body,
) -> Vec<Embed> {
    use comrak::nodes::NodeValue;
    let mut found = Vec::new();
    if links.embeds() == Embeds::Text {
        return found;
    }
    let texts: Vec<_> = root
        .descendants()
        .filter(|n| matches!(&n.data().value, NodeValue::Text(t) if t.contains("![[")))
        .collect();
    for node in texts {
        let skipped = node.ancestors().skip(1).any(|n| {
            matches!(
                n.data().value,
                NodeValue::Link(_)
                    | NodeValue::Image(_)
                    | NodeValue::WikiLink(_)
                    | NodeValue::Heading(_)
            )
        });
        let text = match &node.data().value {
            NodeValue::Text(t) if !skipped => t.to_string(),
            _ => continue,
        };
        let spans = embed_spans(&text);
        if spans.is_empty() {
            continue;
        }
        let in_paragraph = node
            .parent()
            .is_some_and(|p| matches!(p.data().value, NodeValue::Paragraph));
        let text_node = |t: &str| -> &'a comrak::nodes::AstNode<'a> {
            arena.alloc(NodeValue::Text(t.to_string().into()).into())
        };
        let mut blocks = Vec::new();
        let mut done = 0;
        for span in spans {
            if span.start > done {
                node.insert_before(text_node(&text[done..span.start]));
            }
            let index = math_index(found.len());
            let marker = text_node(&format!("{EMBED}{index}{EMBED_END}"));
            node.insert_before(marker);
            let embed = embed_html(&text[span.clone()], links, body, in_paragraph);
            if embed.block.is_some() {
                blocks.push(marker);
            }
            found.push(embed);
            done = span.end;
        }
        if done < text.len() {
            node.data_mut().value = NodeValue::Text(text[done..].to_string().into());
        } else {
            node.detach();
        }
        for marker in blocks {
            own_paragraph(arena, marker);
        }
    }
    found
}

/// The byte ranges of the `![[target]]` / `![[target|option]]` in `text`.
fn embed_spans(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(start) = text[from..].find("![[").map(|i| from + i) {
        let inner = start + 3;
        let Some(len) = text[inner..].find("]]") else {
            break;
        };
        let target = text[inner..inner + len].split('|').next().unwrap_or("");
        if text[inner..inner + len].contains('[') || target.trim().is_empty() {
            from = inner;
            continue;
        }
        found.push(start..inner + len + 2);
        from = inner + len + 2;
    }
    found
}

/// Moves `marker` (in a paragraph) into a paragraph of its own, with the same source position;
/// the line breaks around it go.
fn own_paragraph<'a>(arena: &'a comrak::Arena<'a>, marker: &'a comrak::nodes::AstNode<'a>) {
    use comrak::nodes::NodeValue;
    let Some(paragraph) = marker.parent() else {
        return;
    };
    let pos = paragraph.data().sourcepos;
    let new = || -> &'a comrak::nodes::AstNode<'a> {
        let node = arena.alloc(NodeValue::Paragraph.into());
        node.data_mut().sourcepos = pos;
        node
    };
    let rest: Vec<_> = marker.following_siblings().skip(1).collect();
    if !rest.is_empty() {
        let after = new();
        paragraph.insert_after(after);
        for node in rest {
            after.append(node);
        }
        trim_breaks(after);
    }
    if marker.previous_sibling().is_some() {
        let own = new();
        paragraph.insert_after(own);
        own.append(marker);
        trim_breaks(paragraph);
    }
}

/// Drops the line breaks and spaces at the ends of `paragraph`, and the paragraph itself when
/// nothing is left.
fn trim_breaks<'a>(paragraph: &'a comrak::nodes::AstNode<'a>) {
    use comrak::nodes::NodeValue;
    let blank = |n: &&comrak::nodes::AstNode<'_>| match &n.data().value {
        NodeValue::SoftBreak | NodeValue::LineBreak => true,
        NodeValue::Text(t) => t.trim().is_empty(),
        _ => false,
    };
    while let Some(first) = paragraph.first_child().filter(blank) {
        first.detach();
    }
    while let Some(last) = paragraph.last_child().filter(blank) {
        last.detach();
    }
    let Some(first) = paragraph.first_child() else {
        return paragraph.detach();
    };
    if let NodeValue::Text(t) = &mut first.data_mut().value {
        *t = t.trim_start().to_string().into();
    }
    if let Some(last) = paragraph.last_child()
        && let NodeValue::Text(t) = &mut last.data_mut().value
    {
        *t = t.trim_end().to_string().into();
    }
}

/// HTML-escaped `text`.
fn escaped(text: &str) -> String {
    let mut out = String::new();
    comrak::html::escape(&mut out, text).expect("writing to a String");
    out
}

/// The HTML of `written` (`![[target|option]]`): the image, the note (or its section from
/// `#Heading` to the next heading of the same or a higher level) rendered inline, a wiki link
/// (an embed inside an embedded note, a note embedding itself, one that is not alone in a
/// paragraph's text), or the text dimmed when the target is not found.
fn embed_html(written: &str, links: &dyn Links, body: &Body, in_paragraph: bool) -> Embed {
    let inner = &written[3..written.len() - 2];
    let (target, option) = match inner.split_once('|') {
        Some((target, option)) => (target.trim(), Some(option.trim())),
        None => (inner.trim(), None),
    };
    let inline = |html: String| Embed { html, block: None };
    let missing = || {
        inline(format!(
            "<span class=\"embed-missing\">{}</span>",
            escaped(written)
        ))
    };
    if is_embed_image(target) {
        return match links.image(target, None) {
            Some(path) => inline(image_tag(&links.show(&path), target, option)),
            None => missing(),
        };
    }
    let link = || {
        let mut href = String::new();
        comrak::html::escape_href(&mut href, &wiki_href(links, target), false)
            .expect("writing to a String");
        inline(format!(
            "<a href=\"{href}\" data-wikilink=\"true\">{}</a>",
            escaped(target)
        ))
    };
    let (name, heading) = match target.split_once('#') {
        Some((name, heading)) => (name.trim(), Some(heading.trim()).filter(|h| !h.is_empty())),
        None => (target, None),
    };
    if links.embeds() != Embeds::All || name.is_empty() || !in_paragraph {
        return link();
    }
    let Some(file) = links.file(target) else {
        return missing();
    };
    if links.current() == Some(file.as_path()) {
        return link();
    }
    let Some(content) = embedded_note(links, &file, heading, body) else {
        return missing();
    };
    let title = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let title = match heading {
        Some(heading) => format!("{title} \u{203a} {heading}"),
        None => title,
    };
    let mut href = String::new();
    comrak::html::escape_href(&mut href, &wiki_href(links, target), false)
        .expect("writing to a String");
    Embed {
        html: format!(
            "<p class=\"embed-title\"><a href=\"{href}\" data-wikilink=\"true\">{}</a></p>{content}",
            escaped(&title)
        ),
        block: Some(format!(
            " class=\"embed\" data-embed=\"{}\"",
            escaped(target)
        )),
    }
}

/// `<img>` of the image at `src` (as written into the HTML): `option` is `300` (width) or
/// `300x200` (width and height) in pixels, else its alt text (default: `name`). `src` comes
/// first, as `site::rewrite` and `inline_images` read it.
fn image_tag(src: &Path, name: &str, option: Option<&str>) -> String {
    let size = option.and_then(|o| {
        let (w, h) = match o.split_once(['x', 'X']) {
            Some((w, h)) => (w, Some(h)),
            None => (o, None),
        };
        let w: u32 = w.trim().parse().ok()?;
        let h = match h {
            Some(h) => Some(h.trim().parse::<u32>().ok()?),
            None => None,
        };
        Some((w, h))
    });
    let alt = match option {
        Some(alt) if size.is_none() && !alt.is_empty() => alt,
        _ => name,
    };
    let mut tag = format!(
        "<img src=\"{}\" alt=\"{}\"",
        percent_encode(src.as_os_str().as_bytes()),
        escaped(alt)
    );
    if let Some((w, h)) = size {
        tag.push_str(&format!(" width=\"{w}\""));
        if let Some(h) = h {
            tag.push_str(&format!(" height=\"{h}\""));
        }
    }
    tag.push_str(" />");
    tag
}

/// The HTML of the note `file` (or its section under `heading`) inside a page made with
/// `links`: no front matter, task checkboxes, Copy buttons or table of contents; its headings
/// without ids (the page's own are counted and linked to); its embeds one level deep (`Nested`).
/// None when it cannot be read or has no such heading.
fn embedded_note(
    links: &dyn Links,
    file: &Path,
    heading: Option<&str>,
    body: &Body,
) -> Option<String> {
    let text = links.read(file)?;
    let md = match heading {
        Some(heading) => section(&text, heading)?,
        None => text,
    };
    let nested = Nested {
        outer: links,
        from: file,
    };
    let inner = Body {
        rich: body.rich,
        tasks: false,
        sourcepos: false,
        toc: false,
        first_line: 1,
        cached: false,
        next: false,
        smart: body.smart,
        front_matter: None,
    };
    let html = render_body(&md, &nested, &inner)?.html;
    let mut out = String::with_capacity(html.len());
    let mut done = 0;
    for id in crate::incremental::heading_id_ranges(&html) {
        // ` id="` before the value, `"` after it.
        out.push_str(&html[done..id.start - 5]);
        done = id.end + 1;
    }
    out.push_str(&html[done..]);
    Some(out)
}

/// The lines of `md` from the heading `heading` names (as `heading_at`) to the next heading of
/// the same or a higher level.
fn section(md: &str, heading: &str) -> Option<String> {
    let (line, _) = heading_at(md, heading)?;
    let headings = heading_targets(md);
    let level = headings.iter().find(|h| *h.lines.start() == line)?.level;
    let lines: Vec<&str> = md.split_inclusive('\n').collect();
    let end = headings
        .iter()
        .find(|h| *h.lines.start() > line && h.level <= level)
        .map_or(lines.len(), |h| *h.lines.start() - 1);
    Some(lines.get(line - 1..end)?.concat())
}

/// `html` with each embed marker replaced by its HTML; a rendered note's paragraph (when the
/// renderer wrote one) becomes its `<div class="embed">`, keeping its attributes.
fn put_embeds(html: &str, embeds: &[Embed]) -> String {
    let mut out =
        String::with_capacity(html.len() + embeds.iter().map(|e| e.html.len()).sum::<usize>());
    let mut rest = html;
    while let Some(start) = rest.find(EMBED) {
        let marker = &rest[start + EMBED.len_utf8()..];
        let end = marker.find(EMBED_END).expect("markers come in pairs");
        let index: String = marker[..end]
            .chars()
            .map(|c| char::from_u32(c as u32 - *MATH_DIGITS.start() as u32 + '0' as u32).unwrap())
            .collect();
        let embed = &embeds[index.parse::<usize>().expect("embed index")];
        let mut after = &marker[end + EMBED_END.len_utf8()..];
        let mut before = &rest[..start];
        match &embed.block {
            Some(attrs) => {
                // `<p …>MARK</p>`: the paragraph `own_paragraph` made (none in a tight list).
                let open = before.rfind("<p").filter(|&p| {
                    let tag = &before[p..];
                    tag.ends_with('>')
                        && tag[2..].starts_with([' ', '>'])
                        && !tag[1..].contains('<')
                        && after.starts_with("</p>")
                });
                let mut extra = "";
                if let Some(p) = open {
                    extra = &before[p + 2..before.len() - 1];
                    before = &before[..p];
                    after = &after["</p>".len()..];
                }
                out.push_str(before);
                out.push_str(&format!("<div{attrs}{extra}>{}</div>", embed.html));
            }
            None => {
                out.push_str(before);
                out.push_str(&embed.html);
            }
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The text comrak makes each heading's id from, in order (`Rendered::headings`).
fn heading_texts<'a>(root: &'a comrak::nodes::AstNode<'a>) -> Vec<String> {
    root.descendants()
        .filter(|n| matches!(n.data().value, comrak::nodes::NodeValue::Heading(_)))
        .map(|n| n.collect_text())
        .collect()
}

/// Whether `paragraph` is a `[TOC]` marker: top-level, plain text only (comrak may split it at
/// the brackets), `[TOC]` in any case once trimmed. `[TOC] x`, `*[TOC]*`, a second line, or
/// `[toc]` as a link to a `[toc]: …` definition are not.
fn is_toc_marker<'a>(paragraph: &'a comrak::nodes::AstNode<'a>) -> bool {
    use comrak::nodes::NodeValue;
    if !matches!(paragraph.data().value, NodeValue::Paragraph) {
        return false;
    }
    let mut text = String::new();
    for child in paragraph.children() {
        let NodeValue::Text(piece) = &child.data().value else {
            return false;
        };
        if text.len() + piece.len() > 16 {
            return false;
        }
        text.push_str(piece);
    }
    text.trim().eq_ignore_ascii_case("[toc]")
}

/// A heading's title in a table of contents, as the macOS app's outline gives it: its plain
/// text (code spans keep their backticks, formulas their `$`), trimmed; "Untitled Heading"
/// when empty.
fn heading_title<'a>(heading: &'a comrak::nodes::AstNode<'a>) -> String {
    use comrak::nodes::NodeValue;
    let mut title = String::new();
    for node in heading.descendants().skip(1) {
        match &node.data().value {
            NodeValue::Text(text) => title.push_str(text),
            NodeValue::Code(code) => {
                title.push('`');
                title.push_str(&code.literal);
                title.push('`');
            }
            NodeValue::Math(math) => {
                let dollars = if math.display_math { "$$" } else { "$" };
                title.push_str(dollars);
                title.push_str(&math.literal);
                title.push_str(dollars);
            }
            NodeValue::HtmlInline(html) => title.push_str(html),
            NodeValue::FootnoteReference(note) => {
                title.push_str("[^");
                title.push_str(&note.name);
                title.push(']');
            }
            NodeValue::SoftBreak | NodeValue::LineBreak => title.push(' '),
            _ => {}
        }
    }
    let title = title.replace(is_marker, "");
    match title.trim() {
        "" => "Untitled Heading".to_string(),
        trimmed => trimmed.to_string(),
    }
}

/// The table of contents of headings given as (level, title, id), in order: nested
/// `<ul>\n<li><a href="#id">Title</a>\n<ul>\n…</ul>\n</li>\n</ul>\n`, empty without headings.
/// A deeper heading opens a list inside the item before it (one level, however many it skips);
/// a shallower one closes the lists deeper than itself, and joins the list it lands in when it
/// is still deeper than that list's parent (h1, h3, h2: h2 is h3's sibling). The macOS app
/// makes the same HTML.
pub(crate) fn toc_list<'a>(headings: impl IntoIterator<Item = (u8, &'a str, &'a str)>) -> String {
    let mut out = String::new();
    // The level of each open list.
    let mut levels: Vec<u8> = Vec::new();
    for (level, title, id) in headings {
        match levels.last() {
            None => {
                out.push_str("<ul>\n");
                levels.push(level);
            }
            Some(&last) if level > last => {
                out.push_str("\n<ul>\n");
                levels.push(level);
            }
            Some(_) => {
                out.push_str("</li>\n");
                while levels.len() > 1 && level < levels[levels.len() - 1] {
                    if level > levels[levels.len() - 2] {
                        *levels.last_mut().expect("open list") = level;
                        break;
                    }
                    levels.pop();
                    out.push_str("</ul>\n</li>\n");
                }
            }
        }
        out.push_str("<li><a href=\"#");
        push_escaped(&mut out, id);
        out.push_str("\">");
        push_escaped(&mut out, title);
        out.push_str("</a>");
    }
    if levels.is_empty() {
        return out;
    }
    out.push_str("</li>\n");
    for _ in 1..levels.len() {
        out.push_str("</ul>\n</li>\n");
    }
    out.push_str("</ul>\n");
    out
}

/// `text` with `& < > "` escaped, onto `out`.
fn push_escaped(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// `html` with each `[TOC]` placeholder `render_body` left (`<p …>TOC</p>\n`) replaced by
/// `<nav class="toc" …>\n{list}</nav>\n`, keeping the paragraph's attributes
/// (`data-sourcepos`); by nothing when `list` (`toc_list`) is empty.
pub(crate) fn fill_toc(html: &str, list: &str) -> String {
    const END: &str = "</p>\n";
    let mut out = String::with_capacity(html.len() + list.len());
    let mut rest = html;
    // The note's own copies of the marker were dropped, so each one is a placeholder, right
    // after its `<p …>` tag.
    while let Some(at) = rest.find(TOC) {
        let open = rest[..at].rfind('<').expect("placeholder in a paragraph");
        out.push_str(&rest[..open]);
        if !list.is_empty() {
            let attributes = &rest[open + 2..at - 1];
            out.push_str("<nav class=\"toc\"");
            out.push_str(attributes);
            out.push_str(">\n");
            out.push_str(list);
            out.push_str("</nav>\n");
        }
        let after = &rest[at + TOC.len_utf8()..];
        rest = after.strip_prefix(END).unwrap_or(after);
    }
    out.push_str(rest);
    out
}

/// Alerts (GitHub's and Obsidian's callouts): a block quote whose first line is `[!KIND]`,
/// optionally a fold marker `-`/`+` (ignored: rendered open) and a space and a title, any
/// case, with KIND one of `ALERT_KINDS`, becomes comrak's alert node without that line,
/// rendered as `<div class="alert alert-<kind> alert-group-<group>"><p
/// class="alert-title">Title</p>…</div>` (`render_body` rewrites comrak's tags,
/// `alert_tags`; the five GitHub kinds keep `alert alert-<kind>` without a group class).
/// The kind and title travel in the node's title as `kind ALERT title` (`alert_parts`).
/// Comrak's own `alerts` extension stays off: it only knows GitHub's five kinds and also
/// takes `>>>` fences. Unknown kinds stay quotes. True if `quote` became an alert.
pub(crate) fn make_alert<'a>(quote: &'a comrak::nodes::AstNode<'a>) -> bool {
    use comrak::nodes::{AlertType, NodeAlert, NodeValue};
    let Some(paragraph) = quote
        .first_child()
        .filter(|p| matches!(p.data().value, NodeValue::Paragraph))
    else {
        return false;
    };
    // The inlines of the first line, up to and with the break that ends it. comrak may split
    // `[!NOTE]` at the brackets: the pieces are joined. The marker itself must be plain text;
    // a title may hold inline Markdown (kept as its text).
    let mut line = String::new();
    let mut marker = Vec::new();
    for child in paragraph.children() {
        marker.push(child);
        match &child.data().value {
            NodeValue::SoftBreak | NodeValue::LineBreak => break,
            NodeValue::Text(text) => line.push_str(text),
            _ if line.trim_start().starts_with("[!") && line.contains(']') => {
                for node in child.descendants() {
                    match &node.data().value {
                        NodeValue::Text(t) => line.push_str(t),
                        NodeValue::Code(c) => line.push_str(&c.literal),
                        _ => {}
                    }
                }
            }
            _ => return false,
        }
    }
    let Some((kind, title)) = parse_alert_marker(&line) else {
        return false;
    };
    let alert_type = match kind.as_str() {
        "tip" => AlertType::Tip,
        "important" => AlertType::Important,
        "warning" => AlertType::Warning,
        "caution" => AlertType::Caution,
        _ => AlertType::Note,
    };
    for node in marker {
        node.detach();
    }
    match paragraph.first_child() {
        // The rest of the paragraph starts on the line after the marker.
        Some(first) => paragraph.data_mut().sourcepos.start = first.data().sourcepos.start,
        None => paragraph.detach(),
    }
    quote.data_mut().value = NodeValue::Alert(Box::new(NodeAlert {
        alert_type,
        title: Some(format!("{kind}{ALERT}{title}")),
        multiline: false,
        fence_length: 0,
        fence_offset: 0,
    }));
    true
}

/// `[!KIND]`, `[!KIND]-`, `[!kind]+ Title` → (kind lower-cased, title or the default one);
/// `None` for an unknown kind or another text.
fn parse_alert_marker(line: &str) -> Option<(String, String)> {
    let rest = line.trim().strip_prefix("[!")?;
    let close = rest.find(']')?;
    let kind = rest[..close].to_ascii_lowercase();
    alert_group(&kind)?;
    let mut rest = &rest[close + 1..];
    if let Some(r) = rest.strip_prefix(['-', '+']) {
        rest = r;
    }
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let title = rest.trim();
    let title = if title.is_empty() {
        let mut chars = kind.chars();
        chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
            .unwrap_or_default()
    } else {
        title.to_string()
    };
    Some((kind, title))
}

/// The alert kinds (as written, lower-cased) and the group whose look each one has. The
/// first five are GitHub's (their HTML has no group class).
pub(crate) const ALERT_KINDS: [(&str, &str); 27] = [
    ("note", "note"),
    ("tip", "tip"),
    ("important", "important"),
    ("warning", "warning"),
    ("caution", "caution"),
    ("abstract", "abstract"),
    ("summary", "abstract"),
    ("tldr", "abstract"),
    ("info", "info"),
    ("todo", "todo"),
    ("hint", "tip"),
    ("success", "success"),
    ("check", "success"),
    ("done", "success"),
    ("question", "question"),
    ("help", "question"),
    ("faq", "question"),
    ("attention", "warning"),
    ("failure", "failure"),
    ("fail", "failure"),
    ("missing", "failure"),
    ("danger", "danger"),
    ("error", "danger"),
    ("bug", "bug"),
    ("example", "example"),
    ("quote", "quote"),
    ("cite", "quote"),
];

/// The group of an alert kind (`ALERT_KINDS`).
pub(crate) fn alert_group(kind: &str) -> Option<&'static str> {
    ALERT_KINDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, g)| *g)
}

/// An alert node's kind and title (`make_alert`).
pub(crate) fn alert_parts(alert: &comrak::nodes::NodeAlert) -> (&str, &str) {
    alert
        .title
        .as_deref()
        .and_then(|t| t.split_once(ALERT))
        .unwrap_or(("note", "Note"))
}

/// The colour (light) of an alert kind in Copy as Rich Text and RTF: its group's.
pub(crate) fn alert_colour(kind: &str) -> &'static str {
    let group = alert_group(kind).unwrap_or("note");
    ALERT_COLOURS
        .iter()
        .find(|(g, _)| *g == group)
        .map_or("#0969da", |(_, c)| *c)
}

/// comrak's alert tags (`make_alert`) → ours: `<div class="markdown-alert markdown-alert-…"
/// …>\n<p class="markdown-alert-title">kind ALERT title</p>` → `<div class="alert
/// alert-kind[ alert-group-g]" …>\n<p class="alert-title">title</p>`. The note's text is
/// escaped, so these tags only come from alerts.
fn alert_tags(html: &str) -> String {
    const DIV: &str = "<div class=\"markdown-alert markdown-alert-";
    const TITLE: &str = "<p class=\"markdown-alert-title\">";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(DIV) {
        out.push_str(&rest[..at]);
        let after = &rest[at + DIV.len()..];
        let (Some(quote), Some(title)) = (after.find('"'), after.find(TITLE)) else {
            out.push_str(&rest[at..]);
            return out;
        };
        let attrs = &after[quote + 1..title];
        let text = &after[title + TITLE.len()..];
        let end = text.find("</p>").unwrap_or(text.len());
        let (kind, title_text) = text[..end]
            .split_once(ALERT)
            .unwrap_or(("note", &text[..end]));
        out.push_str("<div class=\"alert alert-");
        out.push_str(kind);
        match alert_group(kind) {
            Some(group) if !GITHUB_ALERTS.contains(&kind) => {
                out.push_str(" alert-group-");
                out.push_str(group);
            }
            _ => {}
        }
        out.push('"');
        out.push_str(attrs);
        out.push_str("<p class=\"alert-title\">");
        out.push_str(title_text);
        rest = &text[end..];
    }
    out.push_str(rest);
    out
}

/// GitHub's five alert kinds: their HTML stays as before Obsidian's callouts.
const GITHUB_ALERTS: [&str; 5] = ["note", "tip", "important", "warning", "caution"];

/// Whether `md` has a line that is only `%%` (a block comment's fence, `strip_comments`).
pub(crate) fn has_block_comment(md: &str) -> bool {
    md.contains("%%") && md.lines().any(|l| l.trim() == "%%")
}

/// `md` without Obsidian's comments: `%%text%%` on one line, and lines from a line that is
/// only `%%` to the next such line (left as empty lines, so line numbers stay). Not inside
/// code spans, code blocks, front matter or math; an unterminated `%%` stays text.
pub(crate) fn strip_comments(md: &str) -> std::borrow::Cow<'_, str> {
    use comrak::nodes::NodeValue;
    if !md.contains("%%") {
        return md.into();
    }
    let starts: Vec<usize> = std::iter::once(0)
        .chain(md.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let byte = |line: usize, column: usize| {
        let start = starts
            .get(line.saturating_sub(1))
            .copied()
            .unwrap_or(md.len());
        (start + column.saturating_sub(1)).min(md.len())
    };
    // Byte ranges comments can't touch: code and math nodes, front matter; blocks by whole
    // lines.
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &options());
    let mut kept: Vec<std::ops::Range<usize>> = Vec::new();
    for node in root.descendants() {
        let data = node.data();
        let pos = data.sourcepos;
        match &data.value {
            NodeValue::CodeBlock(_) | NodeValue::FrontMatter(_) => {
                let end = starts.get(pos.end.line).copied().unwrap_or(md.len());
                kept.push(byte(pos.start.line, 1)..end);
            }
            NodeValue::Code(_) | NodeValue::Math(_) => {
                kept.push(
                    byte(pos.start.line, pos.start.column)..byte(pos.end.line, pos.end.column) + 1,
                );
            }
            _ => {}
        }
    }
    let free = |at: usize| !kept.iter().any(|r| r.contains(&at));
    let mut out = String::with_capacity(md.len());
    let mut line = 0;
    while line < starts.len() {
        let start = starts[line];
        let end = starts.get(line + 1).copied().unwrap_or(md.len());
        let text = &md[start..end];
        if text.trim() == "%%" && free(start + text.find('%').unwrap_or(0)) {
            let close = (line + 1..starts.len()).find(|&l| {
                let s = starts[l];
                let t = &md[s..starts.get(l + 1).copied().unwrap_or(md.len())];
                t.trim() == "%%" && free(s + t.find('%').unwrap_or(0))
            });
            if let Some(close) = close {
                for l in line..=close {
                    let s = starts[l];
                    let e = starts.get(l + 1).copied().unwrap_or(md.len());
                    if md[s..e].ends_with('\n') {
                        out.push('\n');
                    }
                }
                line = close + 1;
                continue;
            }
        }
        // Inline comments on this line.
        let mut rest = 0;
        let mut from = 0;
        while let Some(open) = text[from..].find("%%").map(|i| from + i) {
            if !free(start + open) {
                from = open + 1;
                continue;
            }
            let close = text[open + 2..]
                .match_indices("%%")
                .map(|(i, _)| open + 2 + i)
                .find(|&c| free(start + c));
            let Some(close) = close else { break };
            out.push_str(&text[rest..open]);
            rest = close + 2;
            from = rest;
        }
        out.push_str(&text[rest..]);
        line += 1;
    }
    out.into()
}

/// `==` highlights stay on one line, as in the macOS app and the editor: a pair comrak's
/// `highlight` made across a soft or hard line break goes back to text (its two `==` and its
/// children in place). The `==` in between may still pair on their own line.
pub(crate) fn unmark_multiline<'a>(
    arena: &'a comrak::Arena<'a>,
    root: &'a comrak::nodes::AstNode<'a>,
) {
    use comrak::nodes::NodeValue;
    let spanning: Vec<_> = root
        .descendants()
        .filter(|n| matches!(n.data().value, NodeValue::Highlight))
        .filter(|n| {
            n.descendants()
                .any(|d| matches!(d.data().value, NodeValue::SoftBreak | NodeValue::LineBreak))
        })
        .collect();
    for mark in spanning {
        let pos = mark.data().sourcepos;
        let equals = |line: usize, column: usize| {
            let text = NodeValue::Text("==".into());
            let at = (line, column, line, column + 1).into();
            arena.alloc(comrak::nodes::Ast::new_with_sourcepos(text, at).into())
        };
        mark.insert_before(equals(pos.start.line, pos.start.column));
        while let Some(child) = mark.first_child() {
            mark.insert_before(child);
        }
        mark.insert_before(equals(
            pos.end.line,
            pos.end.column.saturating_sub(1).max(1),
        ));
        mark.detach();
    }
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

/// Preferences ▸ Line Breaks ▸ Every New Line: a single newline in a paragraph renders as `<br>`.
static HARD_BREAKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sets `HARD_BREAKS`, at launch and when the preference changes. Process-wide because every
/// renderer (preview, exports, print, copy) builds its options in `options`.
pub(crate) fn set_hard_breaks(on: bool) {
    HARD_BREAKS.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn hard_breaks() -> bool {
    HARD_BREAKS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Preferences ▸ Preview ▸ Front Matter, as its index in `FrontMatter::ALL`: how the preview
/// and Export Folder as HTML show a note's front matter (print hides it, other exports leave
/// it out).
static FRONT_MATTER: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Sets `FRONT_MATTER`, at launch and when the preference changes (like `set_hard_breaks`).
pub(crate) fn set_front_matter(mode: FrontMatter) {
    let index = FrontMatter::ALL
        .iter()
        .position(|m| *m == mode)
        .unwrap_or(0);
    FRONT_MATTER.store(index as u8, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn front_matter() -> FrontMatter {
    let index = FRONT_MATTER.load(std::sync::atomic::Ordering::Relaxed);
    FrontMatter::ALL
        .get(usize::from(index))
        .copied()
        .unwrap_or_default()
}

/// Markdown options shared by rendering and `code_block_at`.
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
    // `==text==` → `<mark>` (Obsidian, Typora): exactly two `=`, flanking like `~~`, so
    // `a == b`, `== x==` and `===` stay text; see `unmark_multiline` for line breaks.
    ext.highlight = true;
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
    find_heading(&heading_ids(md), fragment).cloned()
}

/// The 1-based line and id of each heading of `md` (outside code blocks and front matter), as
/// `heading_at` matches link fragments against them; read once, matched with `find_heading`.
pub fn heading_ids(md: &str) -> Vec<(usize, String)> {
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &options());
    let mut ids = comrak::Anchorizer::new();
    root.descendants()
        .filter(|n| matches!(n.data().value, comrak::nodes::NodeValue::Heading(_)))
        .map(|n| {
            (
                n.data().sourcepos.start.line,
                ids.anchorize(&n.collect_text()),
            )
        })
        .collect()
}

/// `heading_at` over headings `heading_ids` read: the one whose id is `fragment` or, failing
/// that, the id of `fragment` as a title.
pub fn find_heading<'a>(
    headings: &'a [(usize, String)],
    fragment: &str,
) -> Option<&'a (usize, String)> {
    let text = comrak::Anchorizer::new().anchorize(fragment);
    [fragment, text.as_str()]
        .iter()
        .find_map(|want| headings.iter().find(|(_, id)| id == want))
}

/// The text of the heading `src` is (an ATX line, or a Setext text line and its underline),
/// as `heading_at` reads it before making its id; None unless `src` is exactly one heading.
pub fn heading_text(src: &str) -> Option<String> {
    let arena = comrak::Arena::new();
    let mut options = options();
    options.extension.front_matter_delimiter = None;
    let root = comrak::parse_document(&arena, src, &options);
    let node = root.first_child()?;
    (matches!(node.data().value, comrak::nodes::NodeValue::Heading(_))
        && node.next_sibling().is_none())
    .then(|| node.collect_text())
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

/// A span of the editor's text: 0-based (line, byte) start and exclusive end.
pub type Span = ((usize, usize), (usize, usize));

/// What the editor colours in `md`, by comrak's rules as the preview (so code is never
/// styled), markers included.
#[derive(Debug, Default, PartialEq)]
pub struct EditorSpans {
    /// The formulas the preview renders as math: `$…$`, `$$…$$`, ```math blocks (`$5 và $10`
    /// is not one).
    pub math: Vec<Span>,
    /// `==highlights==`, each on one line, both `==` included.
    pub highlights: Vec<Span>,
}

/// `EditorSpans` of `md`.
pub fn editor_spans(md: &str) -> EditorSpans {
    // Most notes have no formula or highlight: skip the parse.
    if !md.contains('$') && !md.contains("math") && !md.contains("==") {
        return EditorSpans::default();
    }
    use comrak::nodes::NodeValue;
    let arena = comrak::Arena::new();
    let root = comrak::parse_document(&arena, md, &options());
    unmark_multiline(&arena, root);
    let span = |node: &comrak::nodes::AstNode| {
        let p = node.data().sourcepos;
        (
            (p.start.line - 1, p.start.column - 1),
            (p.end.line - 1, p.end.column),
        )
    };
    let mut spans = EditorSpans::default();
    for node in root.descendants() {
        match &node.data().value {
            NodeValue::Math(_) => spans.math.push(span(node)),
            NodeValue::CodeBlock(b) if b.info.split_whitespace().next() == Some("math") => {
                spans.math.push(span(node))
            }
            NodeValue::Highlight => spans.highlights.push(span(node)),
            _ => {}
        }
    }
    spans
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
/// A `[TOC]` paragraph's text in the AST (`fill_toc`).
const TOC: char = '\u{E007}';
/// An embed's place (`mark_embeds`): EMBED, its index in `MATH_DIGITS`, EMBED_END.
const EMBED: char = '\u{E008}';
const EMBED_END: char = '\u{E009}';
/// Between an alert's kind and title in comrak's alert node (`make_alert`).
const ALERT: char = '\u{E00A}';
/// `'0'`…`'9'` of a formula's index.
const MATH_DIGITS: std::ops::RangeInclusive<char> = '\u{E010}'..='\u{E019}';

/// Whether `c` is one of the private-use chars `body_html` marks the AST with; the note's own
/// copies are dropped before parsing.
pub(crate) fn is_marker(c: char) -> bool {
    matches!(c, TAG_OPEN..=ALERT) || MATH_DIGITS.contains(&c)
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

/// URI prefix of a click on a tag chip; the percent-encoded tag follows.
pub(crate) const TAG_URI: &str = "mm-tag:";

/// URI prefix of a code block's Copy button; the block's 1-based first source line follows.
const COPY: &str = "mm-copy:";

/// URI prefix of a wiki link whose note was not found.
const MISSING: &str = "mm-missing:";

/// Every byte but `A-Za-z0-9-_.~/` as `%XX`.
pub(crate) fn percent_encode(bytes: &[u8]) -> String {
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

/// What `![[…]]` embeds become (`Links::embeds`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Embeds {
    /// Left as written: no folder is open.
    Text,
    /// Images shown, note embeds as wiki links: inside an embedded note (one level deep).
    Images,
    /// Images shown, notes rendered inline.
    All,
}

/// How `body_html` resolves a note's wiki links and `![[…]]` embeds. Any
/// `Fn(&str) -> Option<PathBuf>` is one that resolves wiki links only (embeds stay text).
pub trait Links {
    /// The path a `[[target]]` links to, as written into the HTML; None: not found.
    fn note(&self, target: &str) -> Option<PathBuf>;
    /// `note` for a link written in the note `from` (an embedded one).
    fn note_from(&self, target: &str, _from: &Path) -> Option<PathBuf> {
        self.note(target)
    }
    fn embeds(&self) -> Embeds {
        Embeds::Text
    }
    /// The note file `![[target]]` embeds.
    fn file(&self, _target: &str) -> Option<PathBuf> {
        None
    }
    /// The image file `![[target]]` shows, nearest to the note `from` (the rendered one when None).
    fn image(&self, _target: &str, _from: Option<&Path>) -> Option<PathBuf> {
        None
    }
    /// The text of an embedded note.
    fn read(&self, _note: &Path) -> Option<String> {
        None
    }
    /// A file's path as written into the HTML.
    fn show(&self, path: &Path) -> PathBuf {
        path.to_path_buf()
    }
    /// The rendered note (a note embedding itself shows a link).
    fn current(&self) -> Option<&Path> {
        None
    }
    /// The folder relative links and images of the text are written for, when the page is
    /// not that text's (an embedded note), as written into the HTML.
    fn base(&self) -> Option<PathBuf> {
        None
    }
}

impl<F: Fn(&str) -> Option<PathBuf>> Links for F {
    fn note(&self, target: &str) -> Option<PathBuf> {
        self(target)
    }
}

/// The open folder's links, as Ctrl+click and the Find tools resolve them: notes (and their
/// aliases) and embeddable images by name, nearest to `current` first. No `root`: no folder is
/// open, embeds stay text.
pub struct Folder {
    pub notes: Vec<PathBuf>,
    pub images: Vec<PathBuf>,
    pub aliases: std::sync::Arc<Aliases>,
    pub root: Option<PathBuf>,
    pub current: Option<PathBuf>,
}

impl Folder {
    fn resolve(&self, target: &str, from: Option<&Path>) -> Option<PathBuf> {
        let root = self.root.as_deref().unwrap_or(Path::new(""));
        resolve_wiki_with(target, &self.notes, &self.aliases, root, from).cloned()
    }
}

impl Links for Folder {
    fn note(&self, target: &str) -> Option<PathBuf> {
        self.resolve(target, self.current.as_deref())
    }
    fn note_from(&self, target: &str, from: &Path) -> Option<PathBuf> {
        self.resolve(target, Some(from))
    }
    fn embeds(&self) -> Embeds {
        if self.root.is_some() {
            Embeds::All
        } else {
            Embeds::Text
        }
    }
    fn file(&self, target: &str) -> Option<PathBuf> {
        self.note(target)
    }
    fn image(&self, target: &str, from: Option<&Path>) -> Option<PathBuf> {
        let root = self.root.as_deref()?;
        resolve_wiki(target, &self.images, root, from.or(self.current.as_deref())).cloned()
    }
    fn read(&self, note: &Path) -> Option<String> {
        std::fs::read(note)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }
    fn current(&self) -> Option<&Path> {
        self.current.as_deref()
    }
}

/// `inner` for a page meant to sit in `dir` (exports): paths relative to it.
struct Portable<'a> {
    inner: &'a dyn Links,
    dir: &'a Path,
}

impl Links for Portable<'_> {
    fn note(&self, target: &str) -> Option<PathBuf> {
        self.inner.note(target).map(|p| relative(self.dir, &p))
    }
    fn note_from(&self, target: &str, from: &Path) -> Option<PathBuf> {
        self.inner
            .note_from(target, from)
            .map(|p| relative(self.dir, &p))
    }
    fn embeds(&self) -> Embeds {
        self.inner.embeds()
    }
    fn file(&self, target: &str) -> Option<PathBuf> {
        self.inner.file(target)
    }
    fn image(&self, target: &str, from: Option<&Path>) -> Option<PathBuf> {
        self.inner.image(target, from)
    }
    fn read(&self, note: &Path) -> Option<String> {
        self.inner.read(note)
    }
    fn show(&self, path: &Path) -> PathBuf {
        relative(self.dir, &self.inner.show(path))
    }
    fn current(&self) -> Option<&Path> {
        self.inner.current()
    }
}

/// The links of the note `from` embedded in a page made with `outer`.
struct Nested<'a> {
    outer: &'a dyn Links,
    from: &'a Path,
}

impl Links for Nested<'_> {
    fn note(&self, target: &str) -> Option<PathBuf> {
        self.outer.note_from(target, self.from)
    }
    fn note_from(&self, target: &str, from: &Path) -> Option<PathBuf> {
        self.outer.note_from(target, from)
    }
    fn embeds(&self) -> Embeds {
        match self.outer.embeds() {
            Embeds::Text => Embeds::Text,
            _ => Embeds::Images,
        }
    }
    fn image(&self, target: &str, from: Option<&Path>) -> Option<PathBuf> {
        self.outer.image(target, from.or(Some(self.from)))
    }
    fn show(&self, path: &Path) -> PathBuf {
        self.outer.show(path)
    }
    fn current(&self) -> Option<&Path> {
        Some(self.from)
    }
    fn base(&self) -> Option<PathBuf> {
        Some(self.outer.show(self.from.parent()?))
    }
}

/// Extensions `![[name]]` shows as an image (any case).
const EMBED_IMAGES: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tiff", "heic",
];

/// Whether `![[name]]` names an image (by its extension).
pub fn is_embed_image(name: &str) -> bool {
    Path::new(name.trim())
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EMBED_IMAGES.iter().any(|x| e.eq_ignore_ascii_case(x)))
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
pub(crate) fn wiki_key(target: &str) -> Option<String> {
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
    let suffix = format!("/{target}");
    nearest(
        notes.filter(|p| {
            let rel = note_key(p, root);
            rel == target || rel.ends_with(&suffix)
        }),
        current,
    )
}

/// Of several notes a link may mean, the one in `current`'s folder, else the shortest path.
fn nearest<'a>(
    notes: impl Iterator<Item = &'a PathBuf>,
    current: Option<&Path>,
) -> Option<&'a PathBuf> {
    let here = current.and_then(Path::parent);
    notes.min_by_key(|p| (p.parent() != here, p.as_os_str().len(), p.as_os_str()))
}

/// `resolve_wiki`, then, when no note has that name, the note of `notes` with that front-matter
/// alias (several: the same rule as for names).
pub fn resolve_wiki_with<'a>(
    target: &str,
    notes: &'a [PathBuf],
    aliases: &Aliases,
    root: &Path,
    current: Option<&Path>,
) -> Option<&'a PathBuf> {
    resolve_wiki(target, notes, root, current).or_else(|| {
        let found = aliases.notes(target);
        if found.is_empty() {
            return None;
        }
        nearest(notes.iter().filter(|n| found.contains(n)), current)
    })
}

/// `resolve_wiki` for many links against the same notes (Backlinks, link updates, Export
/// Folder): the notes grouped by the last part of their name, so a link only compares the
/// notes of its name instead of every note of the folder. Same results as `resolve_wiki`, or
/// `resolve_wiki_with` when made `with_aliases`.
#[derive(Debug, Clone, Default)]
pub struct WikiIndex {
    root: PathBuf,
    notes: Vec<PathBuf>,
    by_name: HashMap<String, Vec<usize>>,
    /// Alias key (`aliases::key`) → the notes with that alias.
    by_alias: HashMap<String, Vec<usize>>,
}

impl WikiIndex {
    /// Names only: link updates leave `[[alias]]` links alone.
    pub fn new(notes: Vec<PathBuf>, root: &Path) -> Self {
        Self::with_aliases(notes, root, &Aliases::default())
    }

    pub fn with_aliases(notes: Vec<PathBuf>, root: &Path, aliases: &Aliases) -> Self {
        let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
        let mut by_alias: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, note) in notes.iter().enumerate() {
            let key = note_key(note, root);
            by_name
                .entry(last_part(&key).to_string())
                .or_default()
                .push(i);
            for key in aliases.of(note).iter().filter_map(|a| aliases::key(a)) {
                let found = by_alias.entry(key).or_default();
                if found.last() != Some(&i) {
                    found.push(i);
                }
            }
        }
        WikiIndex {
            root: root.to_path_buf(),
            notes,
            by_name,
            by_alias,
        }
    }

    pub fn notes(&self) -> &[PathBuf] {
        &self.notes
    }

    pub fn resolve(&self, target: &str, current: Option<&Path>) -> Option<&PathBuf> {
        let key = wiki_key(target)?;
        let named = self.by_name.get(last_part(&key)).and_then(|found| {
            best_wiki_match(
                found.iter().map(|&i| &self.notes[i]),
                &key,
                &self.root,
                current,
            )
        });
        named.or_else(|| {
            let found = self.by_alias.get(&aliases::key(target)?)?;
            nearest(found.iter().map(|&i| &self.notes[i]), current)
        })
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
pub fn page(md: &str, dark: bool, theme: Theme, web_images: bool, wiki: &dyn Links) -> String {
    preview_page(
        dark,
        theme,
        None,
        web_images,
        &body_html(md, wiki, true, true, true, true),
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
pub fn export_page(md: &str, title: &str, note_dir: &Path, wiki: &dyn Links) -> String {
    let body = portable_body(md, note_dir, wiki, true, true, true);
    standalone_page(title, &body)
}

/// `export_page` for Export Folder as HTML: code is highlighted on the spot, so thousands of
/// notes rendered (on several threads) neither use nor replace the preview's highlighting cache.
/// Front matter shows as Preferences ▸ Preview ▸ Front Matter says (`front_matter`).
pub fn folder_export_page(md: &str, title: &str, note_dir: &Path, wiki: &dyn Links) -> String {
    let body = portable_body_with(md, note_dir, wiki, true, true, false, Some(front_matter()));
    standalone_page(title, &body)
}

/// A standalone page titled `title` around `body`: the preview's CSS in light mode, no script,
/// images from anywhere (`EXPORT_CSP`).
pub fn standalone_page(title: &str, body: &str) -> String {
    let mut escaped = String::new();
    comrak::html::escape(&mut escaped, title).expect("writing to a String");
    html_page(
        false,
        Theme::Default,
        EXPORT_CSP,
        &format!("<title>{escaped}</title>"),
        body,
    )
}

/// Copy as HTML: the note's (or selection's) HTML for pasting into mail/docs, with the same
/// links as `export_page` relative to `note_dir`. No page CSS or script; code is plain
/// `<pre><code>` (the `hl-*` classes mean nothing without our CSS), formulas MathML (no
/// KaTeX), tags plain `#tag` text in a classed span. The `<meta charset>` makes browsers/editors read the clipboard as UTF-8.
pub fn html_fragment(md: &str, note_dir: &Path, wiki: &dyn Links) -> String {
    format!(
        "<meta charset=\"utf-8\">{}",
        portable_body(md, note_dir, wiki, false, true, false)
    )
}

/// Alert groups (`ALERT_KINDS`) with their light-mode colour (as the preview's CSS), for exports
/// that can't use the page style sheet: Copy as Rich Text and RTF.
pub(crate) const ALERT_COLOURS: [(&str, &str); 15] = [
    ("note", "#0969da"),
    ("tip", "#1a7f37"),
    ("important", "#8250df"),
    ("warning", "#9a6700"),
    ("caution", "#d1242f"),
    ("abstract", "#0e8a94"),
    ("info", "#0e8a94"),
    ("todo", "#0e8a94"),
    ("success", "#1a7f37"),
    ("question", "#bc4c00"),
    ("failure", "#d1242f"),
    ("danger", "#d1242f"),
    ("bug", "#d1242f"),
    ("example", "#8250df"),
    ("quote", "#6e7781"),
];

/// `==highlight==` in Copy as Rich Text and RTF: the preview's light yellow.
pub(crate) const MARK_COLOUR: &str = "#fff1a8";

/// Copy as Rich Text: the note's (or selection's) HTML as a whole document for word
/// processors and mail (LibreOffice, Thunderbird, web mail), which paste it formatted. The body
/// is Copy as HTML's (links, MathML, diagrams as code; `[TOC]` kept as written) plus a small style sheet (table borders,
/// monospace code) those apps read; `<img>`s go through `inline_images` with `image`.
pub fn rich_text_page(
    md: &str,
    note_dir: &Path,
    wiki: &dyn Links,
    image: &dyn Fn(&str) -> Option<String>,
) -> String {
    let mut body = inline_images(
        &portable_body(md, note_dir, wiki, false, false, false),
        image,
    );
    // Inline colours per alert kind: mail apps drop class rules. The note's text is escaped, so
    // these tags only come from alerts.
    const DIV: &str = "<div class=\"alert alert-";
    const TITLE: &str = "<p class=\"alert-title\">";
    let mut styled = String::with_capacity(body.len());
    let mut rest = body.as_str();
    while let Some(at) = rest.find(DIV) {
        let after = &rest[at + DIV.len()..];
        let kind_end = after.find([' ', '"']).unwrap_or(after.len());
        let colour = alert_colour(&after[..kind_end]);
        let class_end = after.find('"').unwrap_or(after.len());
        styled.push_str(&rest[..at + DIV.len() + class_end + 1]);
        styled.push_str(&format!(
            " style=\"border-left: 4px solid {colour}; padding-left: 8px;\""
        ));
        rest = &after[(class_end + 1).min(after.len())..];
        if let Some(title) = rest.find(TITLE) {
            styled.push_str(&rest[..title]);
            styled.push_str(&format!(
                "<p class=\"alert-title\" style=\"color: {colour};\">"
            ));
            rest = &rest[title + TITLE.len()..];
        }
    }
    styled.push_str(rest);
    body = styled;
    // Highlights inline too, solid yellow: pasted documents are usually light.
    let body = body.replace(
        "<mark>",
        &format!("<mark style=\"background-color: {MARK_COLOUR};\">"),
    );
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>\
table {{ border-collapse: collapse; }} th, td {{ border: 1px solid #999; padding: 2px 6px; }} \
pre, code {{ font-family: monospace; }} .alert {{ border-left: 4px solid #999; padding-left: 8px; }} \
.alert-title {{ font-weight: bold; }} mark {{ background-color: {MARK_COLOUR}; }}</style></head><body>{body}</body></html>"
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

/// `body_html` with wiki links relative to `note_dir` and unresolved ones as plain text, no
/// front matter.
fn portable_body(
    md: &str,
    note_dir: &Path,
    wiki: &dyn Links,
    rich: bool,
    toc: bool,
    cached: bool,
) -> String {
    portable_body_with(md, note_dir, wiki, rich, toc, cached, None)
}

/// `portable_body` with the front matter as `front_matter` says.
fn portable_body_with(
    md: &str,
    note_dir: &Path,
    wiki: &dyn Links,
    rich: bool,
    toc: bool,
    cached: bool,
    front_matter: Option<FrontMatter>,
) -> String {
    let wiki = Portable {
        inner: wiki,
        dir: note_dir,
    };
    let mut body = body_html_with(
        md,
        &wiki,
        rich,
        false,
        false,
        toc,
        cached && rich,
        crate::smart::enabled(),
        front_matter,
    );
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
const VARS: [&str; 15] = [
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
    "mark-bg",
];

/// Giá trị của `VARS` (cùng thứ tự) cho một theme ở chế độ sáng hoặc tối.
fn theme_values(theme: Theme, dark: bool) -> [&'static str; 15] {
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
            "#fff1a8",
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
            "rgba(210,153,34,.4)",
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
            "#f5e3a3",
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
            "#574719",
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
            "#f5e7a8",
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
            "#51461f",
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
            "#ffec9c",
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
            "#3a360a",
        ],
    }
}

/// The editor's `==highlight==` colours in a theme: the background (the preview's `--mark-bg`,
/// solid where that one is see-through) and the dimmed `==` (`--hl-comment`).
pub fn editor_mark_colours(theme: Theme, dark: bool) -> (&'static str, &'static str) {
    let values = theme_values(theme, dark);
    let fill = match (theme, dark) {
        // The page's 40 % amber over the Default dark background.
        (Theme::Default, true) => "#664f20",
        _ => values[14],
    };
    (fill, values[7])
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
.alert { --alert: #0969da; margin: 0 0 1em; padding: .5em 1em; border-left: .25em solid var(--alert); }
.alert > :last-child { margin-bottom: 0; }
.alert-title { margin: 0 0 .25em; font-weight: 600; line-height: 1; color: var(--alert); }
.alert-tip { --alert: #1a7f37; } .alert-important { --alert: #8250df; } .alert-warning { --alert: #9a6700; } .alert-caution { --alert: #d1242f; }
html.dark .alert-note { --alert: #4493f8; } html.dark .alert-tip { --alert: #3fb950; } html.dark .alert-important { --alert: #ab7df8; }
html.dark .alert-warning { --alert: #d29922; } html.dark .alert-caution { --alert: #f85149; }
.alert-group-tip, .alert-group-success { --alert: #1a7f37; } .alert-group-warning { --alert: #9a6700; }
.alert-group-abstract, .alert-group-info, .alert-group-todo { --alert: #0e8a94; } .alert-group-question { --alert: #bc4c00; }
.alert-group-failure, .alert-group-danger, .alert-group-bug { --alert: #d1242f; } .alert-group-example { --alert: #8250df; } .alert-group-quote { --alert: #6e7781; }
html.dark .alert-group-tip, html.dark .alert-group-success { --alert: #3fb950; } html.dark .alert-group-warning { --alert: #d29922; }
html.dark .alert-group-abstract, html.dark .alert-group-info, html.dark .alert-group-todo { --alert: #39c5cf; } html.dark .alert-group-question { --alert: #f0883e; }
html.dark .alert-group-failure, html.dark .alert-group-danger, html.dark .alert-group-bug { --alert: #f85149; } html.dark .alert-group-example { --alert: #ab7df8; } html.dark .alert-group-quote { --alert: #9198a1; }
table { border-collapse: collapse; } th, td { border: 1px solid rgba(127,127,127,.4); padding: 4px 10px; }
img { max-width: 100%; }
ul:has(> li > input[type=checkbox]), ul:has(> li > a.task) { list-style: none; padding-left: 1.2em; }
a.task { cursor: pointer; }
a.task input { pointer-events: none; appearance: none; width: 1em; height: 1em; margin: 0 .2em 0 0; vertical-align: -.15em; border: 1.5px solid var(--link); border-radius: 3px; background: var(--bg); }
a.task input:checked { background: var(--link) url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16'%3E%3Cpath d='M3.5 8.5l3 3 6-7' fill='none' stroke='white' stroke-width='2.2'/%3E%3C/svg%3E\") center / 85% no-repeat; }
mark { padding: 0 .1em; border-radius: 2px; color: inherit; background: var(--mark-bg); -webkit-print-color-adjust: exact; print-color-adjust: exact; }
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
table.front-matter { margin: 0 0 1.5em; font-size: 0.85em; } .front-matter th, .front-matter td { border: none; border-bottom: 1px solid rgba(127,127,127,.2); padding: 3px 12px 3px 0; vertical-align: top; }
.front-matter th { text-align: left; white-space: nowrap; font-weight: 500; color: var(--hl-comment); } .front-matter td { white-space: pre-line; } .front-matter td code { white-space: pre; }
div.mermaid { margin: 1em 0; } div.mermaid > pre { margin: 0; }
div.mermaid.drawn > pre { display: none; }
div.mermaid .diagram { overflow-x: auto; text-align: center; }
div.mermaid .diagram svg { max-width: 100%; height: auto; }
p.mermaid-error { margin: .3em 0 0; font-size: .85em; color: var(--missing); white-space: pre-wrap; }
code.math-error { color: var(--missing); }
.katex-display { overflow-x: auto; overflow-y: hidden; }
nav.toc { margin: 0 0 1em; } nav.toc ul ul { margin: 0; }
div.embed { margin: 0 0 1em; padding: .3em 1em; border-left: 3px solid rgba(127,127,127,.4); }
p.embed-title { margin: 0 0 .3em; font-size: .8em; opacity: .7; }
.embed-missing { opacity: .55; }
@media print { html { background: none; } body { max-width: none; padding: 0; } pre, table, img, blockquote, .alert, div.mermaid, div.embed, .katex-display { break-inside: avoid; } a.copy, pre.front-matter, table.front-matter { display: none; } }
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
    /// A tag chip (`preview.rs` turns its click into `mm-tag:`): the tag without `#`.
    Tag(String),
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
    } else if let Some(tag) = uri.strip_prefix(TAG_URI) {
        state::unescape(tag)
            .map(|t| t.to_string_lossy().trim_start_matches('#').to_string())
            .filter(|t| !t.is_empty())
            .map_or(Link::Ignore, Link::Tag)
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

/// A folder in the system temp dir for embed tests, removed on drop (also when a test fails).
#[cfg(test)]
pub(crate) struct TestFolder(pub PathBuf);

#[cfg(test)]
impl TestFolder {
    /// The folder with `files` (relative path, text).
    pub(crate) fn new(tag: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("mm-embeds-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, text) in files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        TestFolder(root)
    }

    /// Its links for the note `current`, as the preview resolves them.
    pub(crate) fn links(&self, current: &str) -> Folder {
        let mut images = Vec::new();
        let tree =
            crate::notes::scan_with_images(&self.0, crate::notes::SortOrder::Name, &mut images)
                .unwrap();
        let mut notes = Vec::new();
        crate::quick::note_paths(&tree, &mut notes);
        Folder {
            notes,
            images,
            aliases: Default::default(),
            root: Some(self.0.clone()),
            current: Some(self.0.join(current)),
        }
    }
}

#[cfg(test)]
impl Drop for TestFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_highlighting() {
        let html = to_html(
            "```rust\nfn main() { let s = \"<b>&\"; }\n```\n",
            &|_: &str| None,
        );
        assert!(html.contains("<pre><code class=\"language-rust\">"));
        assert!(html.contains("<span class=\"hl-storage hl-type hl-function hl-rust\">fn</span>"));
        assert!(html.contains("&lt;b&gt;&amp;"));
        assert!(!html.contains("<b>"));
        for md in ["```nosuchlang\nfn <i>\n```\n", "```\nfn <i>\n```\n"] {
            let html = to_html(md, &|_: &str| None);
            assert!(!html.contains("<span"), "{html}");
            assert!(html.contains("fn &lt;i&gt;"));
        }
    }

    #[test]
    fn heading() {
        assert!(to_html("# Title", &|_: &str| None).contains("<h1 id=\"title\">Title</h1>"));
    }

    #[test]
    fn heading_ids() {
        // GitHub-style: lower case, accents kept, space → `-`, punctuation dropped, repeats `-1`.
        let md =
            "# Cài đặt **nhanh**!\n\n## Cài đặt nhanh\n\nỨng dụng\n---\n\n[go](#cài-đặt-nhanh-1)\n";
        let html = to_html(md, &|_: &str| None);
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
        let html = to_html("| a | b |\n|---|---|\n| 1 | 2 |\n", &|_: &str| None);
        assert!(html.contains("<table>"));
        assert!(html.contains("<th>a</th>"));
        assert!(html.contains("<td>2</td>"));
    }

    #[test]
    fn task_list() {
        let md = "# T\n\n- [x] done\n- [ ] todo\n  1. [X] nested\n     - plain\n\n> - [ ] quoted\n\
\nText[^1]\n\n[^1]: - [ ] in note \u{E002}9\u{E003}\n";
        let html = to_html(md, &|_: &str| None);
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
        assert_eq!(
            link_action("mm-tag:work%2Fpro%20j", "file:///a.md"),
            Link::Tag("work/pro j".into())
        );
        assert_eq!(link_action("mm-tag:", "file:///a.md"), Link::Ignore);
    }

    #[test]
    fn code_copy_buttons() {
        let md = "# T\n\n```rust\nfn x() {}\n```\n\n    indented\n    two\n\n- item\n\n  ```\n  in list\n  ```\n\n> ~~~\n> quoted\n> ~~~\n";
        let html = to_html(md, &|_: &str| None);
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
        let export = export_page(md, "T", Path::new("/r"), &|_: &str| None);
        let copy = html_fragment(md, Path::new("/r"), &|_: &str| None);
        for out in [export, copy] {
            assert!(!out.contains("mm-copy:") && !out.contains("class=\"code\""));
            assert!(!out.contains("data-sourcepos"));
        }
    }

    #[test]
    fn alerts() {
        for (marker, kind, title) in [
            ("[!NOTE]", "note", "Note"),
            ("[!TIP]", "tip", "Tip"),
            ("[!IMPORTANT]", "important", "Important"),
            ("[!WARNING]", "warning", "Warning"),
            ("[!CAUTION]", "caution", "Caution"),
            ("[!note]  ", "note", "Note"),
            ("[!Warning]", "warning", "Warning"),
        ] {
            let md = format!("> {marker}\n> Read *this*.\n>\n> - item\n");
            let html = to_html(&md, &|_: &str| None);
            assert_eq!(
                html,
                format!(
                    "<div class=\"alert alert-{kind}\">\n<p class=\"alert-title\">{title}</p>\n\
<p>Read <em>this</em>.</p>\n<ul>\n<li>item</li>\n</ul>\n</div>\n"
                ),
                "{md:?}"
            );
        }
        // The marker alone, a hard break after it, inside a list.
        let html = to_html("> [!TIP]\n", &|_: &str| None);
        assert_eq!(
            html,
            "<div class=\"alert alert-tip\">\n<p class=\"alert-title\">Tip</p>\n</div>\n"
        );
        let html = to_html("> [!TIP]\\\n> text\n", &|_: &str| None);
        assert!(html.contains("Tip</p>\n<p>text</p>"), "{html}");
        let html = to_html("- > [!CAUTION]\n  > hot\n", &|_: &str| None);
        assert!(
            html.contains("<li>\n<div class=\"alert alert-caution\">"),
            "{html}"
        );
    }

    #[test]
    fn alert_lines() {
        let md = "# A\n\n> [!NOTE]\n> one\n> two\n\n> [!TIP]\n>\n> three\n";
        let html = body_html(md, &|_: &str| None, true, true, true, true);
        assert!(
            html.contains(
                "<div class=\"alert alert-note\" data-sourcepos=\"3:1-5:5\">\n\
<p class=\"alert-title\">Note</p>\n<p data-sourcepos=\"4:3-5:5\">one\ntwo</p>"
            ),
            "{html}"
        );
        assert!(
            html.contains("Tip</p>\n<p data-sourcepos=\"9:3-9:7\">three</p>"),
            "{html}"
        );
    }

    #[test]
    fn obsidian_callouts() {
        let render = |md: &str| to_html(md, &|_: &str| None);
        for (marker, kind, group, title) in [
            ("[!abstract]", "abstract", "abstract", "Abstract"),
            ("[!TLDR]", "tldr", "abstract", "Tldr"),
            ("[!info]", "info", "info", "Info"),
            ("[!todo]", "todo", "todo", "Todo"),
            ("[!hint]", "hint", "tip", "Hint"),
            ("[!done]", "done", "success", "Done"),
            ("[!faq]", "faq", "question", "Faq"),
            ("[!attention]", "attention", "warning", "Attention"),
            ("[!missing]", "missing", "failure", "Missing"),
            ("[!error]", "error", "danger", "Error"),
            ("[!bug]", "bug", "bug", "Bug"),
            ("[!example]", "example", "example", "Example"),
            ("[!cite]", "cite", "quote", "Cite"),
            ("[!info]- Folded", "info", "info", "Folded"),
            ("[!info]+", "info", "info", "Info"),
            ("[!bug] A & <b>", "bug", "bug", "A &amp;"),
        ] {
            let html = render(&format!("> {marker}\n> body\n"));
            assert_eq!(
                html,
                format!(
                    "<div class=\"alert alert-{kind} alert-group-{group}\">\n\
<p class=\"alert-title\">{title}</p>\n<p>body</p>\n</div>\n"
                ),
                "{marker}"
            );
        }
        // GitHub's kinds take a title and a fold marker too, without a group class.
        assert!(render("> [!tip]- Keep it *short*\n> x\n").starts_with(
            "<div class=\"alert alert-tip\">\n<p class=\"alert-title\">Keep it short</p>"
        ));
        assert!(render("> [!note]+\n> x\n").contains("<p class=\"alert-title\">Note</p>"));
        // Rich text keeps the group colour.
        let page = rich_text_page(
            "> [!question] Why\n> x\n",
            Path::new("/"),
            &|_: &str| None,
            &|_: &str| None,
        );
        assert!(page.contains("<div class=\"alert alert-question alert-group-question\" style=\"border-left: 4px solid #bc4c00;"), "{page}");
        assert!(page.contains("<p class=\"alert-title\" style=\"color: #bc4c00;\">Why</p>"));
    }

    #[test]
    fn obsidian_comments() {
        assert_eq!(strip_comments("a %%hidden%% b %%x%%\n"), "a  b \n");
        assert_eq!(
            strip_comments("a\n%%\nsecret\n\nmore\n%%\nb\n"),
            "a\n\n\n\n\n\nb\n"
        );
        for kept in [
            "a %%open\n",
            "`%%code%%` and %%\n",
            "```\n%%\nx\n%%\n```\n",
            "    %%indented%%\n",
            "---\ntitle: \"%%x%%\"\n---\n",
            "$%%x%%$\n",
            "%%\nnever closed\n",
        ] {
            assert_eq!(strip_comments(kept), kept, "{kept:?}");
        }
        let html = to_html("Keep %%drop%% this\n\n%%\nsecret\n%%\n", &|_: &str| None);
        assert_eq!(html, "<p>Keep  this</p>\n");
        assert!(has_block_comment("a\n %% \nb\n%%\n"));
        assert!(!has_block_comment("a %%x%%\n"));
    }

    #[test]
    fn quotes_that_are_not_alerts() {
        for md in [
            "> [!NOTE]more\n> body\n",
            "> [!FOO]\n> body\n",
            "> [!FOO] Title\n> body\n",
            "> [!NOTE\n> body\n",
            "> text\n> [!NOTE]\n",
            "> *[!NOTE]*\n> body\n",
            "> # [!NOTE]\n> body\n",
            "[!NOTE]\ntext\n",
        ] {
            let html = to_html(md, &|_: &str| None);
            assert!(!html.contains("alert"), "{md:?}: {html}");
            assert!(html.contains("[!"), "{md:?}: {html}");
        }
        for md in ["```\n> [!NOTE]\n> x\n```\n", "    > [!NOTE]\n    > x\n"] {
            let html = to_html(md, &|_: &str| None);
            assert!(!html.contains("alert"), "{md:?}: {html}");
            assert!(html.contains("&gt; [!NOTE]\n&gt; x"), "{md:?}: {html}");
        }
        // Raw HTML is escaped, so a note can't fake one.
        let html = to_html(
            "<div class=\"markdown-alert markdown-alert-note\">\n",
            &|_: &str| None,
        );
        assert!(!html.contains("<div"), "{html}");
    }

    #[test]
    fn table_of_contents() {
        let md = "# Intro\n\n[TOC]\n\n## Same\n\n### Deep `code` & <b>\n\n## Same\n\n# After\n";
        let html = to_html(md, &|_: &str| None);
        assert_eq!(
            html.split("<h2").next().unwrap(),
            "<h1 id=\"intro\">Intro</h1>\n<nav class=\"toc\">\n<ul>\n\
<li><a href=\"#intro\">Intro</a>\n<ul>\n<li><a href=\"#same\">Same</a>\n<ul>\n\
<li><a href=\"#deep-code--\">Deep `code` &amp; &lt;b&gt;</a></li>\n</ul>\n</li>\n\
<li><a href=\"#same-1\">Same</a></li>\n</ul>\n</li>\n<li><a href=\"#after\">After</a></li>\n\
</ul>\n</nav>\n"
        );
        // Every listed id is a heading's.
        for id in ["intro", "same", "deep-code--", "same-1", "after"] {
            assert!(html.contains(&format!("id=\"{id}\"")), "{id}: {html}");
        }
        // The preview keeps the marker's lines; any case, spaces around, several markers.
        let html = body_html(
            "# A\n\n  [toc]  \n\n[Toc]\n",
            &|_: &str| None,
            true,
            true,
            true,
            true,
        );
        assert!(
            html.contains("<nav class=\"toc\" data-sourcepos=\"3:3-3:9\">\n<ul>\n<li><a href=\"#a\">A</a></li>\n</ul>\n</nav>\n"),
            "{html}"
        );
        assert_eq!(html.matches("<nav").count(), 2, "{html}");
        // No headings, an empty note: nothing. Front matter and code are not headings.
        assert_eq!(to_html("[TOC]\n", &|_: &str| None), "");
        assert_eq!(to_html("", &|_: &str| None), "");
        let html = to_html(
            "---\ntitle: x\n---\n[TOC]\n\n```\n# no\n```\n",
            &|_: &str| None,
        );
        assert!(!html.contains("nav") && !html.contains("[TOC]"), "{html}");
        // Headings in quotes and lists count; an empty one is untitled.
        let html = to_html("[TOC]\n\n> # Quoted\n\n- ## Listed\n\n#\n", &|_: &str| None);
        assert!(
            html.contains(">Quoted</a>") && html.contains(">Listed</a>"),
            "{html}"
        );
        assert!(html.contains(">Untitled Heading</a>"), "{html}");
        // Rich Text keeps it as written.
        assert!(
            rich_text_page(
                "# A\n\n[TOC]\n",
                Path::new("/r"),
                &|_: &str| None,
                &|_: &str| None
            )
            .contains("<p>[TOC]</p>")
        );
        assert!(
            html_fragment("# A\n\n[TOC]\n", Path::new("/r"), &|_: &str| None)
                .contains("<nav class=\"toc\">")
        );
        assert!(
            export_page("# A\n\n[TOC]\n", "t", Path::new("/r"), &|_: &str| None)
                .contains("<nav class=\"toc\">")
        );
    }

    #[test]
    fn toc_markers_left_as_written() {
        for md in [
            "# A\n\n[TOC] x\n",
            "# A\n\n[TOC]\nmore\n",
            "# A\n\n> [TOC]\n",
            "# A\n\n- [TOC]\n",
            "# A\n\n```\n[TOC]\n```\n",
            "# A\n\n    [TOC]\n",
            "# A\n\n*[TOC]*\n",
            "# A\n\n[TOCS]\n",
            "# [TOC]\n",
        ] {
            let html = to_html(md, &|_: &str| None);
            assert!(!html.contains("<nav"), "{md:?}: {html}");
            assert!(html.contains("[TOC"), "{md:?}: {html}");
        }
        // A link to a definition named `toc`.
        let html = to_html("# A\n\n[TOC]\n\n[toc]: /x\n", &|_: &str| None);
        assert!(
            html.contains("<a href=\"/x\">TOC</a>") && !html.contains("<nav"),
            "{html}"
        );
    }

    #[test]
    fn toc_nesting() {
        let list = |levels: &[u8]| {
            let ids: Vec<String> = (0..levels.len()).map(|i| i.to_string()).collect();
            toc_list(
                levels
                    .iter()
                    .zip(&ids)
                    .map(|(l, id)| (*l, id.as_str(), id.as_str())),
            )
            .replace("<a href=\"#", "")
            .replace('\n', "")
        };
        assert_eq!(list(&[]), "");
        // A skipped level opens one list; a shallower heading closes the deeper ones.
        assert_eq!(
            list(&[1, 3, 1]),
            "<ul><li>0\">0</a><ul><li>1\">1</a></li></ul></li><li>2\">2</a></li></ul>"
        );
        // h1, h3, h2: the h2 joins the h3's list.
        assert_eq!(
            list(&[1, 3, 2]),
            "<ul><li>0\">0</a><ul><li>1\">1</a></li><li>2\">2</a></li></ul></li></ul>"
        );
        // A deep first heading: later shallower ones are its siblings.
        assert_eq!(
            list(&[3, 1, 2]),
            "<ul><li>0\">0</a></li><li>1\">1</a></li><li>2\">2</a></li></ul>"
        );
        assert_eq!(
            list(&[1, 2, 3, 2, 1]),
            "<ul><li>0\">0</a><ul><li>1\">1</a><ul><li>2\">2</a></li></ul></li>\
<li>3\">3</a></li></ul></li><li>4\">4</a></li></ul>"
        );
    }

    #[test]
    fn footnote() {
        let html = to_html("Text[^1]\n\n[^1]: Note.\n", &|_: &str| None);
        assert!(html.contains("href=\"#fn-1\""));
        assert!(html.contains("class=\"footnotes\""));
    }

    #[test]
    fn strikethrough_and_autolink() {
        let html = to_html("~~old~~ https://example.com", &|_: &str| None);
        assert!(html.contains("<del>old</del>"));
        assert!(html.contains("<a href=\"https://example.com\">"));
    }

    #[test]
    fn raw_html_is_escaped() {
        let html = to_html(
            "<script>alert(1)</script>\n\nhi <b onclick=\"x\">b</b>",
            &|_: &str| None,
        );
        assert!(!html.contains("<script"));
        assert!(!html.contains("<b "));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn math_spans() {
        let md = "---\nt: 1\n---\nGiá $5 và $10.\n\nCode `$x$` và $a^2$.\n\n> $$\n> \\frac{a}{b}\n> $$\n\n```math\nx\n```\n\n```\n$y$\n```\n";
        let spans = super::editor_spans(md).math;
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
        assert!(super::editor_spans("Giá $5 và $10").math.is_empty());
    }

    #[test]
    fn math() {
        let html = to_html("a $x^2$ b\n\n$$\\frac{a}{b}$$\n", &|_: &str| None);
        assert!(
            html.contains("a <span class=\"math\" data-delim=\"$\" data-tex=\"x^2\"><math><msup><mi>x</mi><mn>2</mn></msup></math></span> b"),
            "{html}"
        );
        assert!(
            html.contains("<span class=\"math display\" data-delim=\"$$\" data-tex=\"\\frac{a}{b}\"><math display=\"block\"><mfrac><mi>a</mi><mi>b</mi></mfrac></math></span>"),
            "{html}"
        );
        let html = to_html("$\\sum_{i=1}^n \\sqrt{i}$ $a<b$", &|_: &str| None);
        assert!(
            html.contains("<mo lspace=\"0\">∑</mo>") && html.contains("<msqrt>"),
            "{html}"
        );
        assert!(html.contains("<mo>&lt;</mo>"), "{html}");
        // Bad LaTeX: the source, escaped (also for KaTeX); no raw HTML gets through.
        let html = to_html("$\\frac{a$ and $$\\nope<b>$$", &|_: &str| None);
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
        let html = to_html(md, &|_: &str| None);
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
        let p = page(md, false, Theme::Default, false, &|_: &str| None);
        assert!(
            p.contains("<p data-sourcepos=\"1:1-3:3\"><span class=\"math display\""),
            "{p}"
        );
        assert!(p.contains("href=\"mm-copy:9\""), "{p}");
        assert_eq!(p.matches("mm-copy:").count(), 1, "{p}");
        // Prices are text; the note's own marker chars are dropped.
        assert_eq!(
            to_html("$5 and $10\u{E004}0\u{E004}", &|_: &str| None),
            "<p>$5 and $100</p>\n"
        );
        // The preview and Export mark formulas for KaTeX around the MathML (kept as the
        // fallback); Copy as HTML has the MathML only. The preview keeps `data-sourcepos`.
        let md = "# T\n\n$x$\n";
        let mathml = "<math><mi>x</mi></math>";
        let marked =
            format!("<span class=\"math\" data-delim=\"$\" data-tex=\"x\">{mathml}</span>");
        let copy = html_fragment(md, Path::new("/r"), &|_: &str| None);
        assert!(copy.contains(mathml) && !has_math(&copy), "{copy}");
        let export = export_page(md, "t", Path::new("/r"), &|_: &str| None);
        assert!(export.contains(&marked) && has_math(&export), "{export}");
        assert!(export.contains("font-src data:"), "{export}");
        let p = page(md, false, Theme::Default, false, &|_: &str| None);
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
                !has_math(&page(md, false, Theme::Default, false, &|_: &str| None)),
                "{md}"
            );
        }
    }

    #[test]
    fn javascript_link_dropped() {
        assert!(!to_html("[x](javascript:alert(1))", &|_: &str| None).contains("javascript:"));
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
        let p = page(md, false, Theme::Default, false, &|_: &str| None);
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
        let export = export_page(md, "D", Path::new("/r"), &|_: &str| None);
        assert!(
            export
                .contains("<div class=\"mermaid\"><pre><code class=\"language-mermaid\">flowchart")
        );
        assert!(has_diagrams(&export) && !export.contains("mm-copy:"));
        let copy = html_fragment(md, Path::new("/r"), &|_: &str| None);
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
                !has_diagrams(&page(md, false, Theme::Default, false, &|_: &str| None)),
                "{md}"
            );
        }
    }

    #[test]
    fn page_blocks_scripts_and_network() {
        let p = page("# a", true, Theme::Default, false, &|_: &str| None);
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
            &|_: &str| None,
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
                assert!(page("a", dark, theme, false, &|_: &str| None).contains(&css));
            }
        }
        // Mọi biến mà CSS trang dùng đều được theme đặt.
        for used in CSS.split("var(--").skip(1) {
            let name = &used[..used.find(')').unwrap()];
            // `--alert` is set per alert kind in the page CSS, not by a theme.
            if name == "alert" {
                continue;
            }
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

    /// The preview's HTML of `md` (no source positions) with smart punctuation on or off.
    fn smart_html(md: &str, smart: bool) -> String {
        body_html_with(
            md,
            &|_: &str| None,
            true,
            true,
            false,
            true,
            false,
            smart,
            Some(FrontMatter::Table),
        )
    }

    #[test]
    fn smart_punctuation_rules() {
        let html = |md: &str| smart_html(md, true);
        assert_eq!(
            html("\"quoted\" it's -- --- ..."),
            "<p>“quoted” it’s – — …</p>\n"
        );
        assert_eq!(
            html("\"**bold**\" and '*it*'"),
            "<p>“<strong>bold</strong>” and ‘<em>it</em>’</p>\n"
        );
        assert_eq!(
            html("(\"a\") [\"b\"] {'c'} x--\"d\""),
            "<p>(“a”) [“b”] {‘c’} x–“d”</p>\n"
        );
        // Each paragraph, heading and cell starts afresh; a line break counts as whitespace.
        assert_eq!(html("a\n\"b\"\n\n\"c\""), "<p>a\n“b”</p>\n<p>“c”</p>\n");
        assert!(html("| x |\n|---|\n| \"y\" |").contains("<td>“y”</td>"));
        // Link text changes, its destination does not.
        assert_eq!(
            html("[it's--here](https://e.com/a--b's \"t--x\")"),
            "<p><a href=\"https://e.com/a--b&#x27;s\" title=\"t--x\">it’s–here</a></p>\n"
        );
    }

    #[test]
    fn smart_punctuation_leaves_code_urls_and_escapes() {
        let html = |md: &str| smart_html(md, true);
        assert_eq!(html("`\"a--b\"`"), "<p><code>&quot;a--b&quot;</code></p>\n");
        assert_eq!(
            html("see https://e.com/a--b's and <https://e.com/x...y>"),
            "<p>see <a href=\"https://e.com/a--b&#x27;s\">https://e.com/a--b's</a> and \
<a href=\"https://e.com/x...y\">https://e.com/x...y</a></p>\n"
        );
        assert_eq!(
            html("www.e.com/a--b and a@b.co"),
            "<p><a href=\"http://www.e.com/a--b\">www.e.com/a--b</a> and \
<a href=\"mailto:a@b.co\">a@b.co</a></p>\n"
        );
        // Escaped characters stay as written and merge back into the text.
        assert_eq!(
            html("\\\"a\\\" \\-\\- \\.\\.\\. \"b\""),
            "<p>&quot;a&quot; -- ... “b”</p>\n"
        );
        assert_eq!(html("say \\\"x\""), "<p>say &quot;x”</p>\n");
        assert!(html("```\n\"a\" -- b\n```").contains("&quot;a&quot; -- b"));
        assert!(html("    \"a\" -- b").contains("&quot;a&quot; -- b"));
        assert!(
            html("<span title=\"a--b\">'x'</span>")
                .contains("&lt;span title=&quot;a--b&quot;&gt;‘x’&lt;/span&gt;")
        );
        assert!(html("<div>\n\"a\" -- b\n</div>").contains("&quot;a&quot; -- b"));
        assert!(html("---\ntitle: \"a--b\"\n---\n\ntext").contains("<td>a--b</td>"));
        assert!(!html("$a--b$ and $$x...y$$").contains('–'));
        assert!(!html("$a--b$ and $$x...y$$").contains('…'));
        // A tag is a name, not prose.
        assert!(html("#a--b").contains("<span class=\"tag\">#a--b</span>"));
    }

    #[test]
    fn smart_punctuation_keeps_heading_ids() {
        let md = "# It's -- \"here\"\n\n[TOC]\n\n## A...B\n";
        let on = smart_html(md, true);
        let off = smart_html(md, false);
        assert!(
            on.contains("<h1 id=\"its----here\">It’s – “here”</h1>"),
            "{on}"
        );
        assert!(
            off.contains("<h1 id=\"its----here\">It's -- &quot;here&quot;</h1>"),
            "{off}"
        );
        assert!(on.contains("<h2 id=\"ab\">A…B</h2>"), "{on}");
        assert!(
            on.contains("<li><a href=\"#its----here\">It’s – “here”</a>"),
            "{on}"
        );
        assert_eq!(heading_at(md, "its----here").map(|h| h.0), Some(1));
    }

    #[test]
    fn smart_punctuation_off_changes_nothing() {
        let md = "# It's -- \"here\"\n\n\"a\" -- b --- c... \\\"d\\\" `e` https://e.com/a--b's\n";
        assert_eq!(
            smart_html(md, false),
            body_html_with(
                md,
                &|_: &str| None,
                true,
                true,
                false,
                true,
                false,
                false,
                Some(FrontMatter::Table)
            )
        );
        assert_eq!(smart_html(md, false), to_html(md, &|_: &str| None));
        let off = smart_html(md, false);
        assert!(!off.contains(['“', '”', '‘', '’', '–', '—', '…']), "{off}");
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
        let off = page(md, false, Theme::Default, false, &|_: &str| None);
        assert!(
            off.contains(PREVIEW_CSP) && !off.contains("http: https:"),
            "{off}"
        );
        assert!(off.contains("Web images are not loaded"), "{off}");
        let on = page(md, false, Theme::Default, true, &|_: &str| None);
        assert!(on.contains("img-src file: data: http: https:;"), "{on}");
        assert!(
            on.contains("default-src 'none';") && !on.contains("script-src"),
            "{on}"
        );
        assert!(!on.contains("Web images are not loaded"), "{on}");
        // Same body either way; only the head differs.
        let body = |p: &str| p[p.find("<body>").unwrap()..].to_string();
        assert_eq!(body(&off), body(&on));
        let export = export_page(md, "t", Path::new("/n"), &|_: &str| None);
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
    fn rich_text_alert_colours() {
        let p = rich_text_page(
            "> [!TIP]\n> Try it\n",
            Path::new("/r"),
            &|_: &str| None,
            &|_: &str| None,
        );
        assert!(
            p.contains("<div class=\"alert alert-tip\" style=\"border-left: 4px solid #1a7f37;"),
            "{p}"
        );
        assert!(
            p.contains("<p class=\"alert-title\" style=\"color: #1a7f37;\">Tip</p>"),
            "{p}"
        );
    }

    #[test]
    fn highlight_marks() {
        let html = |md: &str| to_html(md, &|_: &str| None);
        assert_eq!(html("==text=="), "<p><mark>text</mark></p>\n");
        assert_eq!(
            html("A ==marked **bold**== word"),
            "<p>A <mark>marked <strong>bold</strong></mark> word</p>\n"
        );
        assert_eq!(html("foo==bar==baz"), "<p>foo<mark>bar</mark>baz</p>\n");
        // Next to a space, not exactly two `=`, in code: as written.
        for md in [
            "a == b",
            "== text==",
            "==text ==",
            "===x===",
            "=x=",
            "a ==== b",
        ] {
            assert_eq!(html(md), format!("<p>{md}</p>\n"), "{md}");
        }
        assert_eq!(html("`==x==`"), "<p><code>==x==</code></p>\n");
        assert!(html("```\n==x==\n```\n").contains("<code>==x==\n</code>"));
        assert!(html("    ==x==\n").contains("<code>==x==\n</code>"));
        // No line break inside; the `==` of another line still pair.
        assert_eq!(html("==two\nlines=="), "<p>==two\nlines==</p>\n");
        assert_eq!(html("==a\n==b=="), "<p>==a\n<mark>b</mark></p>\n");
        assert_eq!(html("==a  \nb=="), "<p>==a<br />\nb==</p>\n");
        // Links keep their URL; a bare URL's `==` are part of it.
        assert_eq!(
            html("[==a==](https://x.y/?q==b==)"),
            "<p><a href=\"https://x.y/?q==b==\"><mark>a</mark></a></p>\n"
        );
        let bare = html("see https://x.y/?a==b==c ok");
        assert!(!bare.contains("<mark>"), "{bare}");
        assert!(bare.contains("href=\"https://x.y/?a==b==c\""), "{bare}");
        // Front matter is not Markdown.
        let page = html("---\ntitle: ==x==\n---\n\n# ==Key== idea\n");
        assert!(page.contains("<td>==x==</td>"), "{page}");
        assert!(
            page.contains("<h1 id=\"key-idea\"><mark>Key</mark> idea</h1>"),
            "{page}"
        );
        // Styled in every theme, light and dark, and printed.
        assert!(CSS.contains("mark { padding: 0 .1em; border-radius: 2px; color: inherit; background: var(--mark-bg);"));
        assert!(CSS.contains("print-color-adjust: exact;"));
        for theme in Theme::ALL {
            assert!(theme_vars(theme, false).contains("--mark-bg: #"));
            assert!(theme_vars(theme, true).contains("--mark-bg: "));
            for dark in [false, true] {
                let (fill, marker) = editor_mark_colours(theme, dark);
                assert!(fill.starts_with('#') && marker.starts_with('#'));
            }
        }
        // Copy as HTML: a bare `<mark>`.
        assert_eq!(
            html_fragment("==x==", Path::new("/r"), &|_: &str| None),
            "<meta charset=\"utf-8\"><p><mark>x</mark></p>\n"
        );
    }

    #[test]
    fn editor_highlight_spans() {
        let md = "A ==b== c, Ý ==đ==\n\n- ==x== and `==y==`\n\n==two\nlines==\n\n> ==q *e*==\n";
        let lines: Vec<&str> = md.lines().collect();
        let got: Vec<&str> = super::editor_spans(md)
            .highlights
            .into_iter()
            .map(|((l1, c1), (l2, c2))| {
                assert_eq!(l1, l2);
                &lines[l1][c1..c2]
            })
            .collect();
        assert_eq!(got, ["==b==", "==đ==", "==x==", "==q *e*=="]);
        assert_eq!(
            super::editor_spans("a == b\n\nTitle\n===\n"),
            Default::default()
        );
    }

    #[test]
    fn rich_text_highlights() {
        let p = rich_text_page("==x==", Path::new("/r"), &|_: &str| None, &|_: &str| None);
        assert!(
            p.contains("<mark style=\"background-color: #fff1a8;\">x</mark>"),
            "{p}"
        );
        assert!(p.contains("mark { background-color: #fff1a8; }"), "{p}");
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
            &|_: &str| None,
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
        let html = to_html("[[a\"><script>|x]]", &|_: &str| None);
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
    fn aliases_resolve_when_no_note_has_the_name() {
        let root = Path::new("/r");
        let notes: Vec<PathBuf> = [
            "/r/Note.md",
            "/r/MN.md",
            "/r/Work/Plan.md",
            "/r/Work/Deep/Other.md",
            "/r/Elsewhere/Third.md",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let of =
            |n: &str, a: &[&str]| (PathBuf::from(n), a.iter().map(|s| s.to_string()).collect());
        let aliases = Aliases::new(HashMap::from([
            of("/r/Note.md", &["MN", "Meeting notes", "Café"]),
            of("/r/Work/Deep/Other.md", &["Shared"]),
            of("/r/Elsewhere/Third.md", &["Shared"]),
            of("/r/Gone.md", &["Gone alias"]),
        ]));
        let index = WikiIndex::with_aliases(notes.clone(), root, &aliases);
        let plain = WikiIndex::new(notes.clone(), root);
        for (target, cur, want) in [
            // A real note name wins over an alias.
            ("mn", None, Some("/r/MN.md")),
            ("Meeting notes#Agenda", None, Some("/r/Note.md")),
            ("MEETING NOTES", None, Some("/r/Note.md")),
            ("Cafe\u{301}", None, Some("/r/Note.md")),
            // Several notes with one alias: this folder first, then the shortest path.
            ("Shared", None, Some("/r/Elsewhere/Third.md")),
            (
                "Shared",
                Some("/r/Work/Deep/x.md"),
                Some("/r/Work/Deep/Other.md"),
            ),
            // Only notes of the list; folders don't apply to aliases.
            ("Gone alias", None, None),
            ("Work/Meeting notes", None, None),
            ("Plan", None, Some("/r/Work/Plan.md")),
        ] {
            let cur = cur.map(Path::new);
            let got = resolve_wiki_with(target, &notes, &aliases, root, cur);
            assert_eq!(got.map(|p| p.to_str().unwrap()), want, "{target}");
            assert_eq!(index.resolve(target, cur), got, "{target}");
        }
        assert_eq!(plain.resolve("Meeting notes", None), None);
        // A link to an alias is not missing in the preview.
        let wiki = |t: &str| resolve_wiki_with(t, &notes, &aliases, root, None).cloned();
        let p = page("[[Meeting notes|x]]", false, Theme::Default, false, &wiki);
        assert!(
            p.contains("/r/Note.md") && !p.contains("href=\"mm-missing"),
            "{p}"
        );
    }

    #[test]
    fn front_matter() {
        let md = "---\ntitle: <b>&</b>\ntags: [a]\n---\n# T\n\n- [ ] do\n\n```\nx\n```\n";
        // The preview's default: the properties table, with its lines for scroll sync.
        let p = page(md, false, Theme::Default, false, &|_: &str| None);
        assert!(
            p.contains("<table class=\"front-matter\" data-line=\"1\" data-sourcepos=\"1:1-4:3\">\n<tbody>\n<tr><th>title</th><td>&lt;b&gt;&amp;&lt;/b&gt;</td></tr>\n<tr><th>tags</th><td>a</td></tr>\n</tbody>\n</table>\n"),
            "{p}"
        );
        // Source: the block as before.
        let body = Body {
            rich: true,
            tasks: true,
            sourcepos: true,
            toc: true,
            first_line: 1,
            cached: false,
            next: false,
            smart: false,
            front_matter: Some(FrontMatter::Source),
        };
        let wiki = |_: &str| None;
        let html = render_body(md, &wiki, &body).unwrap().html;
        assert!(
            html.starts_with("<pre class=\"front-matter\">---\ntitle: &lt;b&gt;&amp;&lt;/b&gt;\ntags: [a]\n---</pre>\n<h1"),
            "{html}"
        );
        let hidden = Body {
            front_matter: Some(FrontMatter::Hidden),
            ..body
        };
        let html = render_body(md, &wiki, &hidden).unwrap().html;
        assert!(html.starts_with("<h1"), "{html}");
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
            export_page(md, "t", Path::new("/n"), &|_: &str| None),
            html_fragment(md, Path::new("/n"), &|_: &str| None),
        ] {
            assert!(
                !out.contains("title:") && !out.contains("front-matter\">"),
                "{out}"
            );
            assert!(!out.contains("<hr"), "{out}");
            assert!(out.contains(">T</h1>"), "{out}");
        }
        // Export Folder as HTML follows the setting (the default here: a table, no sourcepos).
        let out = folder_export_page(md, "t", Path::new("/n"), &|_: &str| None);
        assert!(
            out.contains(
                "<table class=\"front-matter\" data-line=\"1\">\n<tbody>\n<tr><th>title</th>"
            ),
            "{out}"
        );
        // `---` not on line 1: no front matter.
        let p = page(
            "x\n---\ny: 1\n---\n",
            false,
            Theme::Default,
            false,
            &|_: &str| None,
        );
        assert!(!p.contains("front-matter\">"), "{p}");
    }

    #[test]
    fn highlighting_is_the_same_cached_or_not() {
        let md = "```rust\nfn a() {}\n```\n\n```python\nx = 1\n```\n\n\
```rust\nfn a() {}\n```\n\n```nosuchlang\n<b>\n```\n\n    indented\n";
        forget_highlighting();
        let cold = to_html(md, &|_: &str| None);
        assert!(cold.contains("hl-") && cold.contains("&lt;b&gt;"));
        assert_eq!(to_html(md, &|_: &str| None), cold);
        let edited = md.replace("x = 1", "x = 2");
        let warm = to_html(&edited, &|_: &str| None);
        forget_highlighting();
        assert_eq!(to_html(&edited, &|_: &str| None), warm);
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
        let _ = page(
            "```rust\nx\n```\n",
            false,
            Theme::Default,
            false,
            &|_: &str| None,
        ); // load syntaxes
        let start = std::time::Instant::now();
        let html = page(&md, false, Theme::Default, false, &|_: &str| None);
        eprintln!(
            "render {} KB -> {} KB in {:?}",
            md.len() / 1024,
            html.len() / 1024,
            start.elapsed()
        );
    }

    /// A folder for embeds: `Home.md` embeds the others.
    fn embeds_folder(tag: &str) -> TestFolder {
        TestFolder::new(
            tag,
            &[
                ("Home.md", ""),
                (
                    "Notes/Note.md",
                    "---\ntitle: Hidden\n---\n# Note\n\nIntro\n\n## Part\n\nPart text \
![[pic.png]] ![[Other]] ![rel](img/r.png) [[Other]]\n\n### Deeper\n\nsub text\n\n\
## Next\n\nnext text\n",
                ),
                ("Other.md", "other"),
                ("assets/pic.png", "png"),
                ("A/a.png", "png"),
                ("B/a.png", "png"),
            ],
        )
    }

    fn embedded(folder: &TestFolder, md: &str) -> String {
        body_html(md, &folder.links("Home.md"), false, true, false, true)
    }

    #[test]
    fn embeds_images() {
        let folder = embeds_folder("images");
        let root = folder.0.to_string_lossy().into_owned();
        let html = embedded(
            &folder,
            "![[pic.png]]\n\n![[pic.png|300]] ![[PIC.PNG|300x200]]\n",
        );
        assert!(
            html.contains(&format!(
                "<p><img src=\"{root}/assets/pic.png\" alt=\"pic.png\" /></p>"
            )),
            "{html}"
        );
        assert!(
            html.contains(&format!(
                "<img src=\"{root}/assets/pic.png\" alt=\"pic.png\" width=\"300\" />"
            )),
            "{html}"
        );
        assert!(
            html.contains("alt=\"PIC.PNG\" width=\"300\" height=\"200\" />"),
            "{html}"
        );
        // Folder/name narrows; a missing image stays its text, dimmed.
        let html = embedded(&folder, "![[B/a.png]] ![[gone.png]]");
        assert!(html.contains(&format!("src=\"{root}/B/a.png\"")), "{html}");
        assert!(
            html.contains("<span class=\"embed-missing\">![[gone.png]]</span>"),
            "{html}"
        );
    }

    #[test]
    fn embeds_notes_one_level_deep() {
        let folder = embeds_folder("notes");
        let root = folder.0.to_string_lossy().into_owned();
        let html = embedded(&folder, "# Home\n\nBefore ![[Note|ignored]] after\n");
        assert!(
            html.contains("<p>Before</p>\n<div class=\"embed\" data-embed=\"Note\">"),
            "{html}"
        );
        assert!(
            html.contains(&format!(
                "<p class=\"embed-title\"><a href=\"{root}/Notes/Note.md\" data-wikilink=\"true\">Note</a></p>"
            )),
            "{html}"
        );
        assert!(
            html.contains("<p>Intro</p>") && html.contains("next text"),
            "{html}"
        );
        assert!(html.contains("<p>after</p>"), "{html}");
        // No front matter; the embedded headings have no ids (the page's own keep theirs).
        assert!(!html.contains("Hidden"), "{html}");
        assert!(
            html.contains("<h1 id=\"home\">") && html.contains("<h1>Note</h1>"),
            "{html}"
        );
        // Inside it: images resolve, a relative image is relative to that note, an embed is a link.
        assert!(
            html.contains(&format!("src=\"{root}/assets/pic.png\"")),
            "{html}"
        );
        assert!(
            html.contains(&format!("src=\"{root}/Notes/img/r.png\"")),
            "{html}"
        );
        assert!(
            html.contains(&format!(
                "<a href=\"{root}/Other.md\" data-wikilink=\"true\">Other</a> <img"
            )),
            "{html}"
        );
        assert!(!html.contains("data-embed=\"Other\""), "{html}");
    }

    #[test]
    fn embeds_a_section() {
        let folder = embeds_folder("section");
        let html = embedded(&folder, "![[Note#Part]]");
        assert!(
            html.contains("<div class=\"embed\" data-embed=\"Note#Part\">"),
            "{html}"
        );
        assert!(html.contains("Note \u{203a} Part</a></p>"), "{html}");
        assert!(
            html.contains("Part text") && html.contains("sub text"),
            "{html}"
        );
        assert!(
            !html.contains("Intro") && !html.contains("next text"),
            "{html}"
        );
        assert!(
            html.contains("href=\"") && html.contains("Note.md#part\""),
            "{html}"
        );
    }

    #[test]
    fn embeds_missing_self_and_code() {
        let folder = embeds_folder("missing");
        let root = folder.0.to_string_lossy().into_owned();
        let html = embedded(&folder, "![[Gone]] ![[Note#Nowhere]]\n");
        assert!(
            html.contains("<span class=\"embed-missing\">![[Gone]]</span>"),
            "{html}"
        );
        assert!(
            html.contains("<span class=\"embed-missing\">![[Note#Nowhere]]</span>"),
            "{html}"
        );
        // A note embedding itself: a link.
        let html = embedded(&folder, "![[Home]]");
        assert_eq!(
            html,
            format!("<p><a href=\"{root}/Home.md\" data-wikilink=\"true\">Home</a></p>\n")
        );
        let html = embedded(
            &folder,
            "`![[Note]]`\n\n```\n![[pic.png]]\n```\n\n    ![[Other]]\n",
        );
        assert!(!html.contains("embed") && !html.contains("<img"), "{html}");
        assert!(html.contains("<code>![[Note]]</code>"), "{html}");
        assert!(
            html.contains("![[pic.png]]\n</code>") && html.contains("![[Other]]\n</code>"),
            "{html}"
        );
    }

    #[test]
    fn embeds_stay_text_without_a_folder() {
        let folder = embeds_folder("nofolder");
        let links = Folder {
            root: None,
            ..folder.links("Home.md")
        };
        let md = "![[pic.png]] ![[Note]]";
        let html = body_html(md, &links, false, true, false, true);
        assert_eq!(html, "<p>![[pic.png]] ![[Note]]</p>\n");
        assert_eq!(
            to_html(md, &|_: &str| None),
            "<p>![[pic.png]] ![[Note]]</p>\n"
        );
    }

    #[test]
    fn exports_inline_embeds() {
        let folder = embeds_folder("export");
        let links = folder.links("Home.md");
        let md = "![[pic.png|300]]\n\n![[Note#Part]]\n";
        for html in [
            export_page(md, "Home", &folder.0, &links),
            html_fragment(md, &folder.0, &links),
        ] {
            assert!(
                html.contains("<img src=\"assets/pic.png\" alt=\"pic.png\" width=\"300\" />"),
                "{html}"
            );
            assert!(html.contains("data-embed=\"Note#Part\""), "{html}");
            assert!(html.contains("href=\"Notes/Note.md#part\""), "{html}");
            assert!(html.contains("src=\"Notes/img/r.png\""), "{html}");
            assert!(html.contains("href=\"Other.md\""), "{html}");
            assert!(
                !html.contains(&folder.0.to_string_lossy().into_owned()),
                "{html}"
            );
        }
    }
}
