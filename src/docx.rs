//! Export as Word: the note as a `.docx` (Office Open XML) with the RTF export's content rules
//! (`rtf::document`): headings in Word's Heading 1–6 styles, bold, italic, strikethrough,
//! `==highlights==` yellow, inline code and code blocks in a monospace style, links as
//! hyperlinks, bullet / numbered / to-do lists (☐ / ☑) nested per level, quotes indented,
//! callouts under their coloured title, tables with borders and a bold header row, footnotes
//! as `[n]`, formulas as their TeX, front matter and raw HTML left out. PNG, JPEG and GIF
//! images (relative to the note, or `![[name]]` in the open folder) are embedded; others and
//! missing ones become their alt text. Written as a store-only ZIP (`zip`), so no compression
//! crate is needed. Pure apart from reading the images; `preview::export_word` runs it.

use crate::render::Links;
use comrak::nodes::{AstNode, ListType, NodeValue, TableAlignment};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Twips per list level.
const STEP: i32 = 360;
/// Twips a quote or callout is indented by.
const QUOTE: i32 = 720;
/// Width of the text on an A4 page with 1-inch margins, in twips.
const TEXT_WIDTH: i32 = 9026;
/// EMUs (DrawingML's unit) per twip and per pixel at 96 dpi.
const EMU_PER_TWIP: u64 = 635;
const EMU_PER_PIXEL: u64 = 9525;
/// Numbering ids (`numbering.xml`): bullets, open and done to-dos; ordered lists from `FIRST_ORDERED`.
const BULLETS: usize = 1;
const TASK_OPEN: usize = 2;
const TASK_DONE: usize = 3;
const FIRST_ORDERED: usize = 4;
/// Grey of a plain quote's left border.
const QUOTE_BORDER: &str = "D0D7DE";

/// `md` (the note in `note_dir`) as `.docx` bytes, with smart punctuation if it is on;
/// `links` finds `![[name]]` images.
pub fn docx(md: &str, note_dir: &Path, links: &dyn Links) -> Vec<u8> {
    docx_with(md, crate::smart::enabled(), note_dir, links)
}

/// `docx`, with smart punctuation only if `smart`.
fn docx_with(md: &str, smart: bool, note_dir: &Path, links: &dyn Links) -> Vec<u8> {
    let arena = comrak::Arena::new();
    let root = crate::rtf::document(&arena, md, smart);
    let mut doc = Doc {
        note_dir,
        links,
        body: String::new(),
        rels: Vec::new(),
        media: Vec::new(),
        loaded: HashMap::new(),
        ordered: Vec::new(),
        drawings: 0,
    };
    doc.blocks(root, &Ctx::default(), &mut None);
    doc.package()
}

/// Where a block sits: its left indent (twips), list depth, and the left border of the quote
/// or callout around it (hex colour).
#[derive(Clone, Default)]
struct Ctx {
    left: i32,
    depth: usize,
    border: Option<String>,
    quote: bool,
}

/// A paragraph property beyond its style, marker and indent.
#[derive(Clone, Copy, PartialEq)]
enum Extra {
    None,
    /// Kept with the next paragraph (a callout's title).
    KeepNext,
    /// A horizontal rule: a bottom border.
    Rule,
}

/// What the first paragraph of a list item or footnote starts with.
enum Marker {
    /// A numbered paragraph: numbering id and level, and text before the item's (an ordered
    /// to-do's ☐ / ☑).
    Number(usize, usize, &'static str),
    /// A footnote's `[name]`.
    Text(String),
}

/// An image file in `word/media`.
struct Media {
    name: String,
    bytes: Vec<u8>,
}

/// An image's data, kind and size in pixels.
struct Picture {
    bytes: Vec<u8>,
    ext: &'static str,
    width: u32,
    height: u32,
}

/// Inline formatting in effect.
#[derive(Clone, Copy, Default)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    highlight: bool,
    code: bool,
    link: bool,
    /// `superscript` or `subscript`.
    vertical: Option<&'static str>,
    /// Hex colour (callout titles).
    colour: Option<&'static str>,
}

struct Doc<'d> {
    note_dir: &'d Path,
    links: &'d dyn Links,
    body: String,
    /// `word/_rels/document.xml.rels` after the styles and numbering: (type, target, external).
    rels: Vec<(&'static str, String, bool)>,
    media: Vec<Media>,
    /// Image files already in `media`, by path: their relationship id.
    loaded: HashMap<PathBuf, (String, u32, u32)>,
    /// Start number of each ordered list (numbering id `FIRST_ORDERED + index`) and its level.
    ordered: Vec<(usize, usize)>,
    drawings: usize,
}

impl Doc<'_> {
    /// A new relationship's id (`rId1` styles and `rId2` numbering come first).
    fn rel(&mut self, kind: &'static str, target: String, external: bool) -> String {
        self.rels.push((kind, target, external));
        format!("rId{}", self.rels.len() + 2)
    }

    fn blocks<'a>(&mut self, node: &'a AstNode<'a>, ctx: &Ctx, marker: &mut Option<Marker>) {
        for child in node.children() {
            self.block(child, ctx, marker);
        }
    }

    /// One `<w:p>`: `style` its paragraph style, `runs` its content. Its properties go in the
    /// schema's order (Word refuses others).
    fn paragraph(
        &mut self,
        ctx: &Ctx,
        style: Option<&str>,
        marker: Option<Marker>,
        extra: Extra,
        runs: &str,
    ) {
        let mut ppr = String::new();
        if let Some(style) = style {
            ppr.push_str(&format!("<w:pStyle w:val=\"{style}\"/>"));
        }
        if extra == Extra::KeepNext {
            ppr.push_str("<w:keepNext/>");
        }
        let (mut prefix, mut hanging) = (String::new(), false);
        match marker {
            Some(Marker::Number(id, level, before)) => {
                ppr.push_str(&format!(
                    "<w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{id}\"/></w:numPr>"
                ));
                prefix = run(&Style::default(), before);
                hanging = true;
            }
            Some(Marker::Text(text)) => {
                prefix = run(&Style::default(), &format!("{text}\t"));
                hanging = true;
            }
            None => {}
        }
        let left = ctx.border.as_ref().map(|colour| {
            format!("<w:left w:val=\"single\" w:sz=\"18\" w:space=\"8\" w:color=\"{colour}\"/>")
        });
        let bottom = (extra == Extra::Rule)
            .then_some("<w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"999999\"/>");
        if left.is_some() || bottom.is_some() {
            ppr.push_str(&format!(
                "<w:pBdr>{}{}</w:pBdr>",
                left.unwrap_or_default(),
                bottom.unwrap_or_default()
            ));
        }
        if hanging {
            ppr.push_str(&format!(
                "<w:ind w:left=\"{}\" w:hanging=\"{STEP}\"/>",
                ctx.left + STEP
            ));
        } else if ctx.left > 0 {
            ppr.push_str(&format!("<w:ind w:left=\"{}\"/>", ctx.left));
        }
        self.body
            .push_str(&format!("<w:p><w:pPr>{ppr}</w:pPr>{prefix}{runs}</w:p>"));
    }

    /// The pending `marker` as an empty paragraph of its own (an item starting with another block).
    fn flush(&mut self, ctx: &Ctx, marker: &mut Option<Marker>) {
        if let Some(m) = marker.take() {
            self.paragraph(ctx, None, Some(m), Extra::None, "");
        }
    }

    fn body_style(ctx: &Ctx) -> Option<&'static str> {
        ctx.quote.then_some("Quote")
    }

    fn block<'a>(&mut self, node: &'a AstNode<'a>, ctx: &Ctx, marker: &mut Option<Marker>) {
        match &node.data().value {
            NodeValue::FrontMatter(_) | NodeValue::HtmlBlock(_) => {}
            NodeValue::Paragraph | NodeValue::Subtext => {
                let runs = self.inline(node, Style::default());
                let m = marker.take();
                // A list item's text sits at the item's indent, the marker hanging before it.
                self.paragraph(ctx, Self::body_style(ctx), m, Extra::None, &runs);
            }
            NodeValue::Heading(h) => {
                self.flush(ctx, marker);
                let runs = self.inline(node, Style::default());
                let style = format!("Heading{}", h.level.clamp(1, 6));
                self.paragraph(ctx, Some(&style), None, Extra::None, &runs);
            }
            NodeValue::ThematicBreak => {
                self.flush(ctx, marker);
                self.paragraph(ctx, None, None, Extra::Rule, "");
            }
            NodeValue::Alert(alert) => {
                self.flush(ctx, marker);
                let (kind, title) = crate::render::alert_parts(alert);
                let colour = crate::render::alert_colour(kind).trim_start_matches('#');
                let inner = Ctx {
                    left: ctx.left + QUOTE,
                    border: Some(colour.to_uppercase()),
                    quote: true,
                    ..ctx.clone()
                };
                let style = Style {
                    bold: true,
                    colour: Some(colour),
                    ..Style::default()
                };
                let title = run(&style, title);
                self.paragraph(&inner, None, None, Extra::KeepNext, &title);
                self.blocks(node, &inner, &mut None);
            }
            NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                self.flush(ctx, marker);
                let inner = Ctx {
                    left: ctx.left + QUOTE,
                    border: Some(QUOTE_BORDER.into()),
                    quote: true,
                    ..ctx.clone()
                };
                self.blocks(node, &inner, &mut None);
            }
            NodeValue::CodeBlock(code) => {
                self.flush(ctx, marker);
                let literal = code.literal.trim_end_matches('\n');
                let text = if code.info.split_whitespace().next() == Some("math") {
                    format!("$$\n{literal}\n$$")
                } else {
                    literal.to_string()
                };
                let runs = run(&Style::default(), &text);
                self.paragraph(ctx, Some("Code"), None, Extra::None, &runs);
            }
            NodeValue::List(list) => {
                self.flush(ctx, marker);
                let level = ctx.depth.min(8);
                let ordered = matches!(list.list_type, ListType::Ordered);
                let id = if ordered {
                    self.ordered.push((list.start, level));
                    FIRST_ORDERED + self.ordered.len() - 1
                } else {
                    BULLETS
                };
                let item_ctx = Ctx {
                    left: ctx.left + STEP,
                    depth: ctx.depth + 1,
                    ..ctx.clone()
                };
                // `paragraph` hangs the marker one step left of the item's text.
                let marker_ctx = ctx.clone();
                for item in node.children() {
                    let task = match &item.data().value {
                        NodeValue::TaskItem(task) => Some(task.symbol.is_some()),
                        _ => None,
                    };
                    let m = match (task, ordered) {
                        (Some(true), false) => Marker::Number(TASK_DONE, level, ""),
                        (Some(false), false) => Marker::Number(TASK_OPEN, level, ""),
                        (Some(true), true) => Marker::Number(id, level, "\u{2611} "),
                        (Some(false), true) => Marker::Number(id, level, "\u{2610} "),
                        (None, _) => Marker::Number(id, level, ""),
                    };
                    let mut pending = Some(m);
                    for child in item.children() {
                        let paragraph = matches!(
                            child.data().value,
                            NodeValue::Paragraph | NodeValue::Subtext
                        );
                        if pending.is_some() && paragraph {
                            let runs = self.inline(child, Style::default());
                            let style = Self::body_style(ctx);
                            self.paragraph(&marker_ctx, style, pending.take(), Extra::None, &runs);
                        } else {
                            if pending.is_some() {
                                self.flush(&marker_ctx, &mut pending);
                            }
                            self.block(child, &item_ctx, &mut None);
                        }
                    }
                    self.flush(&marker_ctx, &mut pending);
                }
            }
            NodeValue::Table(table) => {
                self.flush(ctx, marker);
                self.table(node, &table.alignments, ctx);
            }
            NodeValue::FootnoteDefinition(def) => {
                let mut m = Some(Marker::Text(format!("[{}]", def.name)));
                for child in node.children() {
                    if m.is_some() && matches!(child.data().value, NodeValue::Paragraph) {
                        let runs = self.inline(child, Style::default());
                        self.paragraph(ctx, None, m.take(), Extra::None, &runs);
                    } else {
                        self.flush(ctx, &mut m);
                        let inner = Ctx {
                            left: ctx.left + STEP,
                            ..ctx.clone()
                        };
                        self.block(child, &inner, &mut None);
                    }
                }
                self.flush(ctx, &mut m);
            }
            _ => self.blocks(node, ctx, marker),
        }
    }

    /// A table with single borders, as wide as the text; the header row bold, shaded and
    /// repeated on each page.
    fn table<'a>(&mut self, node: &'a AstNode<'a>, alignments: &[TableAlignment], ctx: &Ctx) {
        let columns = node
            .children()
            .map(|r| r.children().count())
            .max()
            .unwrap_or(0)
            .max(1);
        let width = ((TEXT_WIDTH - ctx.left).max(STEP * 4)) / columns as i32;
        let border = |side: &str| {
            format!("<w:{side} w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"999999\"/>")
        };
        let borders: String = ["top", "left", "bottom", "right", "insideH", "insideV"]
            .into_iter()
            .map(border)
            .collect();
        let mut out = format!(
            "<w:tbl><w:tblPr><w:tblW w:w=\"{}\" w:type=\"dxa\"/><w:tblInd w:w=\"{}\" w:type=\"dxa\"/>\
             <w:tblBorders>{borders}</w:tblBorders><w:tblLayout w:type=\"fixed\"/>\
             <w:tblLook w:val=\"0020\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"1\" w:noVBand=\"1\"/></w:tblPr><w:tblGrid>",
            width * columns as i32,
            ctx.left
        );
        for _ in 0..columns {
            out.push_str(&format!("<w:gridCol w:w=\"{width}\"/>"));
        }
        out.push_str("</w:tblGrid>");
        for row in node.children() {
            let header = matches!(row.data().value, NodeValue::TableRow(true));
            out.push_str("<w:tr>");
            if header {
                out.push_str("<w:trPr><w:tblHeader/></w:trPr>");
            }
            let mut cells = row.children();
            for i in 0..columns {
                let style = Style {
                    bold: header,
                    ..Style::default()
                };
                let runs = cells
                    .next()
                    .map(|c| self.inline(c, style))
                    .unwrap_or_default();
                let jc = match alignments.get(i) {
                    Some(TableAlignment::Center) => "<w:jc w:val=\"center\"/>",
                    Some(TableAlignment::Right) => "<w:jc w:val=\"right\"/>",
                    _ => "",
                };
                let shading = if header {
                    "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F0F0F0\"/>"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "<w:tc><w:tcPr><w:tcW w:w=\"{width}\" w:type=\"dxa\"/>{shading}</w:tcPr>\
                     <w:p><w:pPr><w:spacing w:after=\"0\"/>{jc}</w:pPr>{runs}</w:p></w:tc>"
                ));
            }
            out.push_str("</w:tr>");
        }
        out.push_str("</w:tbl>");
        self.body.push_str(&out);
        // Word needs a paragraph between two tables; it also spaces the table from what follows.
        self.body.push_str("<w:p/>");
    }

    /// The runs of `node`'s inline children; neighbouring text in one run.
    fn inline<'a>(&mut self, node: &'a AstNode<'a>, style: Style) -> String {
        let (mut out, mut text) = (String::new(), String::new());
        for child in node.children() {
            if let NodeValue::Text(t) = &child.data().value {
                text.push_str(t);
                continue;
            }
            self.text(&mut text, style, &mut out);
            out.push_str(&self.span(child, style));
        }
        self.text(&mut text, style, &mut out);
        out
    }

    /// `text` (then emptied) as runs at the end of `out`, with `![[image]]` embeds outside links.
    fn text(&mut self, text: &mut String, style: Style, out: &mut String) {
        if !style.link && text.contains("![[") {
            out.push_str(&self.text_with_embeds(text, style));
        } else {
            out.push_str(&run(&style, text));
        }
        text.clear();
    }

    fn span<'a>(&mut self, node: &'a AstNode<'a>, style: Style) -> String {
        let with = |f: fn(&mut Style)| {
            let mut s = style;
            f(&mut s);
            s
        };
        match &node.data().value {
            NodeValue::Text(text) => run(&style, text),
            NodeValue::Code(code) => run(&with(|s| s.code = true), &code.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => run(&style, "\n"),
            NodeValue::HtmlInline(_) => String::new(),
            NodeValue::Strong => self.inline(node, with(|s| s.bold = true)),
            NodeValue::Emph => self.inline(node, with(|s| s.italic = true)),
            NodeValue::Strikethrough => self.inline(node, with(|s| s.strike = true)),
            NodeValue::Highlight => self.inline(node, with(|s| s.highlight = true)),
            NodeValue::Underline => self.inline(node, with(|s| s.underline = true)),
            NodeValue::Superscript => self.inline(node, with(|s| s.vertical = Some("superscript"))),
            NodeValue::Subscript => self.inline(node, with(|s| s.vertical = Some("subscript"))),
            NodeValue::Math(math) => {
                let d = if math.display_math { "$$" } else { "$" };
                run(&with(|s| s.code = true), &format!("{d}{}{d}", math.literal))
            }
            NodeValue::FootnoteReference(r) => run(
                &with(|s| s.vertical = Some("superscript")),
                &format!("[{}]", r.name),
            ),
            NodeValue::Image(link) => {
                let alt = alt_text(node);
                let (alt, size) = match alt.rsplit_once('|') {
                    Some((text, option)) => match crate::render::image_size(option) {
                        Some(size) => (text.trim().to_string(), Some(size)),
                        None => (alt.clone(), None),
                    },
                    None => (alt.clone(), None),
                };
                match self.local_image(&link.url) {
                    Some(path) => {
                        if let Some(drawing) = self.drawing(&path, &alt, size) {
                            return drawing;
                        }
                        missing(&alt, &link.url, style)
                    }
                    None => missing(&alt, &link.url, style),
                }
            }
            NodeValue::Link(link) => {
                let url = link.url.as_str();
                if url.is_empty() || style.link {
                    return self.inline(node, style);
                }
                let mut runs = self.inline(node, with(|s| s.link = true));
                if runs.is_empty() {
                    runs = run(&with(|s| s.link = true), url);
                }
                let id = self.rel(
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink",
                    url.to_string(),
                    true,
                );
                format!("<w:hyperlink r:id=\"{id}\" w:history=\"1\">{runs}</w:hyperlink>")
            }
            _ => self.inline(node, style),
        }
    }

    /// `text` with each `![[image]]` of the open folder as the image (`|300`, `|300x200`: its
    /// size), a missing one as its name; other embeds stay as written, as in the RTF export.
    fn text_with_embeds(&mut self, text: &str, style: Style) -> String {
        let mut out = String::new();
        let mut done = 0;
        for span in crate::render::embed_spans(text) {
            let inner = &text[span.start + 3..span.end - 2];
            let (target, option) = match inner.split_once('|') {
                Some((t, o)) => (t.trim(), Some(o.trim())),
                None => (inner.trim(), None),
            };
            if !crate::render::is_embed_image(target) {
                continue;
            }
            out.push_str(&run(&style, &text[done..span.start]));
            done = span.end;
            let size = option.and_then(crate::render::image_size);
            let alt = match option {
                Some(alt) if size.is_none() && !alt.is_empty() => alt,
                _ => target,
            };
            let drawing = self
                .links
                .image(target, None)
                .and_then(|path| self.drawing(&path, alt, size));
            out.push_str(&drawing.unwrap_or_else(|| run(&style, alt)));
        }
        out.push_str(&run(&style, &text[done..]));
        out
    }

    /// The file a Markdown image's `url` names: a path relative to the note (or absolute),
    /// %-escapes decoded. None for web and other URLs.
    fn local_image(&self, url: &str) -> Option<PathBuf> {
        let path = url.split(['?', '#']).next().unwrap_or("");
        let scheme = path.find(':').is_some_and(|c| !path[..c].contains('/'));
        if path.is_empty() || scheme {
            return None;
        }
        let path = PathBuf::from(percent_decode(path));
        Some(if path.is_absolute() {
            path
        } else {
            self.note_dir.join(path)
        })
    }

    /// The `<w:drawing>` run of the image at `path`, added to `word/media` once; None when it
    /// is no PNG, JPEG or GIF (or unreadable). `size`: width and height in pixels as written.
    fn drawing(
        &mut self,
        path: &Path,
        alt: &str,
        size: Option<(u32, Option<u32>)>,
    ) -> Option<String> {
        let (id, width, height) = match self.loaded.get(path) {
            Some(found) => found.clone(),
            None => {
                let picture = picture(std::fs::read(path).ok()?)?;
                let name = format!("image{}.{}", self.media.len() + 1, picture.ext);
                let id = self.rel(
                    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image",
                    format!("media/{name}"),
                    false,
                );
                self.media.push(Media {
                    name,
                    bytes: picture.bytes,
                });
                let found = (id, picture.width, picture.height);
                self.loaded.insert(path.to_path_buf(), found.clone());
                found
            }
        };
        let (cx, cy) = extent(width, height, size);
        self.drawings += 1;
        let n = self.drawings;
        let alt = xml(alt);
        Some(format!(
            "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
             <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"Picture {n}\" descr=\"{alt}\"/>\
             <wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect=\"1\"/></wp:cNvGraphicFramePr>\
             <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
             <pic:pic><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"Picture {n}\" descr=\"{alt}\"/><pic:cNvPicPr/></pic:nvPicPr>\
             <pic:blipFill><a:blip r:embed=\"{id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
             <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>\
             </wp:inline></w:drawing></w:r>"
        ))
    }

    /// The `.docx` file: every part in a store-only ZIP.
    fn package(self) -> Vec<u8> {
        const REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
        const DOC_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let content_types = format!(
            "{XML_DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
             <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
             <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
             <Default Extension=\"png\" ContentType=\"image/png\"/>\
             <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>\
             <Default Extension=\"gif\" ContentType=\"image/gif\"/>\
             <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
             <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
             <Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>\
             </Types>"
        );
        let package_rels = format!(
            "{XML_DECL}<Relationships xmlns=\"{REL}\"><Relationship Id=\"rId1\" \
             Type=\"{DOC_REL}/officeDocument\" Target=\"word/document.xml\"/></Relationships>"
        );
        let mut rels = format!(
            "{XML_DECL}<Relationships xmlns=\"{REL}\">\
             <Relationship Id=\"rId1\" Type=\"{DOC_REL}/styles\" Target=\"styles.xml\"/>\
             <Relationship Id=\"rId2\" Type=\"{DOC_REL}/numbering\" Target=\"numbering.xml\"/>"
        );
        for (i, (kind, target, external)) in self.rels.iter().enumerate() {
            rels.push_str(&format!(
                "<Relationship Id=\"rId{}\" Type=\"{kind}\" Target=\"{}\"{}/>",
                i + 3,
                xml(target),
                if *external {
                    " TargetMode=\"External\""
                } else {
                    ""
                }
            ));
        }
        rels.push_str("</Relationships>");
        let document = format!(
            "{XML_DECL}<w:document \
             xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
             xmlns:r=\"{DOC_REL}\" \
             xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
             xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
             xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
             <w:body>{}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/>\
             <w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/>\
             </w:sectPr></w:body></w:document>",
            self.body
        );
        let mut parts = vec![
            (
                "[Content_Types].xml".to_string(),
                content_types.into_bytes(),
            ),
            ("_rels/.rels".to_string(), package_rels.into_bytes()),
            ("word/document.xml".to_string(), document.into_bytes()),
            ("word/styles.xml".to_string(), styles().into_bytes()),
            (
                "word/numbering.xml".to_string(),
                numbering(&self.ordered).into_bytes(),
            ),
            (
                "word/_rels/document.xml.rels".to_string(),
                rels.into_bytes(),
            ),
        ];
        for media in self.media {
            parts.push((format!("word/media/{}", media.name), media.bytes));
        }
        zip(&parts)
    }
}

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";

/// The plain text of an image's alt (its inline children).
fn alt_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut alt = String::new();
    for n in node.descendants().skip(1) {
        match &n.data().value {
            NodeValue::Text(t) => alt.push_str(t),
            NodeValue::Code(c) => alt.push_str(&c.literal),
            NodeValue::Math(m) => alt.push_str(&m.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => alt.push(' '),
            _ => {}
        }
    }
    alt
}

/// A missing or web image: its alt text, else its URL.
fn missing(alt: &str, url: &str, style: Style) -> String {
    run(&style, if alt.is_empty() { url } else { alt })
}

/// `%XX` escapes decoded (bad ones kept as written; invalid UTF-8 replaced).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `bytes` as a PNG, JPEG or GIF with its size in pixels; None for anything else.
fn picture(bytes: Vec<u8>) -> Option<Picture> {
    let (ext, width, height) = pixel_size(&bytes)?;
    (width > 0 && height > 0).then_some(Picture {
        bytes,
        ext,
        width,
        height,
    })
}

/// The kind (file extension) and width × height in pixels of a PNG, JPEG or GIF.
fn pixel_size(b: &[u8]) -> Option<(&'static str, u32, u32)> {
    let be16 = |i: usize| Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
    let be32 = |i: usize| Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?));
    if b.starts_with(b"\x89PNG\r\n\x1a\n") && b.get(12..16) == Some(b"IHDR") {
        return Some(("png", be32(16)?, be32(20)?));
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        let le16 = |i: usize| Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32);
        return Some(("gif", le16(6)?, le16(8)?));
    }
    if b.starts_with(&[0xff, 0xd8]) {
        // Walk the segments to the first start-of-frame (SOF0–SOF15 but DHT, JPG, DAC).
        let mut i = 2;
        while i + 3 < b.len() {
            if b[i] != 0xff {
                return None;
            }
            let marker = b[i + 1];
            if marker == 0xff {
                i += 1;
                continue;
            }
            if marker == 0x01 || (0xd0..=0xd9).contains(&marker) {
                i += 2;
                continue;
            }
            if (0xc0..=0xcf).contains(&marker) && ![0xc4, 0xc8, 0xcc].contains(&marker) {
                return Some(("jpeg", be16(i + 7)?, be16(i + 5)?));
            }
            i += 2 + be16(i + 2)? as usize;
        }
    }
    None
}

/// An image's size in EMUs: `size` (pixels as written, height kept in proportion when only
/// the width is given) or its own, at 96 dpi, scaled down to the text width.
fn extent(width: u32, height: u32, size: Option<(u32, Option<u32>)>) -> (u64, u64) {
    let (w, h) = match size {
        Some((w, Some(h))) => (w as u64, h as u64),
        Some((w, None)) => (w as u64, (height as u64 * w as u64 / width as u64).max(1)),
        None => (width as u64, height as u64),
    };
    let (cx, cy) = (w * EMU_PER_PIXEL, h * EMU_PER_PIXEL);
    let max = TEXT_WIDTH as u64 * EMU_PER_TWIP;
    if cx > max {
        (max, (cy * max / cx).max(1))
    } else {
        (cx, cy)
    }
}

/// `text` as XML text and attribute values: `&`, `<`, `>`, `"` escaped, characters XML 1.0
/// forbids dropped.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

/// A `<w:r>` of `text` in `style`: tabs as `<w:tab/>`, newlines as `<w:br/>`. Empty text: nothing.
fn run(style: &Style, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // Run properties in the schema's order.
    let mut rpr = String::new();
    if style.link {
        rpr.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
    } else if style.code {
        rpr.push_str("<w:rStyle w:val=\"CodeChar\"/>");
    }
    if style.link && style.code {
        rpr.push_str(MONO);
    }
    if style.bold {
        rpr.push_str("<w:b/>");
    }
    if style.italic {
        rpr.push_str("<w:i/>");
    }
    if style.strike {
        rpr.push_str("<w:strike/>");
    }
    if let Some(colour) = style.colour {
        rpr.push_str(&format!("<w:color w:val=\"{}\"/>", colour.to_uppercase()));
    }
    if style.highlight {
        rpr.push_str("<w:highlight w:val=\"yellow\"/>");
    }
    if style.underline {
        rpr.push_str("<w:u w:val=\"single\"/>");
    }
    if let Some(v) = style.vertical {
        rpr.push_str(&format!("<w:vertAlign w:val=\"{v}\"/>"));
    }
    let mut out = String::from("<w:r>");
    if !rpr.is_empty() {
        out.push_str(&format!("<w:rPr>{rpr}</w:rPr>"));
    }
    let text = text.replace("\r\n", "\n");
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push_str("<w:br/>");
        }
        for (j, piece) in line.split('\t').enumerate() {
            if j > 0 {
                out.push_str("<w:tab/>");
            }
            if !piece.is_empty() {
                out.push_str(&format!("<w:t xml:space=\"preserve\">{}</w:t>", xml(piece)));
            }
        }
    }
    out.push_str("</w:r>");
    out
}

/// The monospace font of code.
const MONO: &str = "<w:rFonts w:ascii=\"Courier New\" w:hAnsi=\"Courier New\" w:cs=\"Courier New\" w:eastAsia=\"Courier New\"/>";

/// `word/styles.xml`: Normal, Heading 1–6 (Word's built-in names, so its navigation pane and
/// table of contents work), Quote, Code, Code Char and Hyperlink.
fn styles() -> String {
    let mut out = format!(
        "{XML_DECL}<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
         <w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\" w:cs=\"Arial\" w:eastAsia=\"Arial\"/>\
         <w:sz w:val=\"24\"/><w:szCs w:val=\"24\"/><w:lang w:val=\"en-US\"/></w:rPr></w:rPrDefault>\
         <w:pPrDefault><w:pPr><w:spacing w:after=\"120\" w:line=\"264\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>\
         <w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:qFormat/></w:style>"
    );
    for (level, size) in [40, 34, 30, 26, 24, 22].into_iter().enumerate() {
        let n = level + 1;
        out.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{n}\"><w:name w:val=\"heading {n}\"/>\
             <w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:uiPriority w:val=\"9\"/><w:qFormat/>\
             <w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before=\"240\" w:after=\"120\"/><w:outlineLvl w:val=\"{level}\"/></w:pPr>\
             <w:rPr><w:b/><w:bCs/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/></w:rPr></w:style>"
        ));
    }
    out.push_str(&format!(
        "<w:style w:type=\"paragraph\" w:styleId=\"Quote\"><w:name w:val=\"Quote\"/>\
         <w:basedOn w:val=\"Normal\"/><w:qFormat/><w:pPr><w:ind w:left=\"{QUOTE}\"/></w:pPr></w:style>\
         <w:style w:type=\"paragraph\" w:styleId=\"Code\"><w:name w:val=\"Code\"/><w:basedOn w:val=\"Normal\"/>\
         <w:qFormat/><w:pPr><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"F0F0F0\"/><w:spacing w:after=\"120\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
         <w:rPr>{MONO}<w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/></w:rPr></w:style>\
         <w:style w:type=\"character\" w:styleId=\"CodeChar\"><w:name w:val=\"Code Char\"/>\
         <w:rPr>{MONO}<w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/></w:rPr></w:style>\
         <w:style w:type=\"character\" w:styleId=\"Hyperlink\"><w:name w:val=\"Hyperlink\"/>\
         <w:uiPriority w:val=\"99\"/><w:rPr><w:color w:val=\"0052CC\"/><w:u w:val=\"single\"/></w:rPr></w:style>\
         </w:styles>"
    ));
    out
}

/// `word/numbering.xml`: bullets (• ◦ ▪ per level), open and done to-dos (☐ / ☑), and one
/// numbering per ordered list (`ordered`: its start and level), so each counts from its own start.
fn numbering(ordered: &[(usize, usize)]) -> String {
    let levels = |text: &dyn Fn(usize) -> (String, &'static str)| -> String {
        (0..9)
            .map(|l| {
                let (lvl_text, format) = text(l);
                format!(
                    "<w:lvl w:ilvl=\"{l}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{format}\"/>\
                     <w:lvlText w:val=\"{lvl_text}\"/><w:lvlJc w:val=\"left\"/>\
                     <w:pPr><w:ind w:left=\"{}\" w:hanging=\"{STEP}\"/></w:pPr></w:lvl>",
                    STEP * (l as i32 + 1)
                )
            })
            .collect()
    };
    let abstract_num = |id: usize, body: String| {
        format!(
            "<w:abstractNum w:abstractNumId=\"{id}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>{body}</w:abstractNum>"
        )
    };
    let mut out = format!(
        "{XML_DECL}<w:numbering xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">"
    );
    out.push_str(&abstract_num(
        0,
        levels(&|l| (["\u{2022}", "\u{25e6}", "\u{25aa}"][l % 3].into(), "bullet")),
    ));
    out.push_str(&abstract_num(
        1,
        levels(&|l| (format!("%{}.", l + 1), "decimal")),
    ));
    out.push_str(&abstract_num(2, levels(&|_| ("\u{2610}".into(), "bullet"))));
    out.push_str(&abstract_num(3, levels(&|_| ("\u{2611}".into(), "bullet"))));
    for (id, abs) in [(BULLETS, 0), (TASK_OPEN, 2), (TASK_DONE, 3)] {
        out.push_str(&format!(
            "<w:num w:numId=\"{id}\"><w:abstractNumId w:val=\"{abs}\"/></w:num>"
        ));
    }
    for (i, (start, level)) in ordered.iter().enumerate() {
        out.push_str(&format!(
            "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"1\"/>\
             <w:lvlOverride w:ilvl=\"{level}\"><w:startOverride w:val=\"{start}\"/></w:lvlOverride></w:num>",
            FIRST_ORDERED + i
        ));
    }
    out.push_str("</w:numbering>");
    out
}

/// CRC-32 (IEEE, as ZIP uses it) lookup table.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |c, &b| {
        CRC_TABLE[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8)
    })
}

/// `files` (name, contents) as a ZIP archive, stored without compression: a local header and
/// the data per file, then the central directory and its end record. Dated 1980-01-01, so the
/// same note gives the same bytes.
fn zip(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    const DATE: u16 = (1 << 5) | 1;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        // Version 2.0, no flags, stored, time 0, date, CRC, sizes, name length, no extra field.
        let fields = |v: &mut Vec<u8>| {
            v.extend_from_slice(&20u16.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&DATE.to_le_bytes());
            v.extend_from_slice(&crc.to_le_bytes());
            v.extend_from_slice(&size.to_le_bytes());
            v.extend_from_slice(&size.to_le_bytes());
            v.extend_from_slice(&(name.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        fields(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        fields(&mut central);
        // No comment, disk 0, no attributes, then where the local header is.
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let start = out.len() as u32;
    let count = files.len() as u16;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::TestFolder;

    fn u16_at(b: &[u8], i: usize) -> usize {
        u16::from_le_bytes([b[i], b[i + 1]]) as usize
    }
    fn u32_at(b: &[u8], i: usize) -> usize {
        u32::from_le_bytes(b[i..i + 4].try_into().unwrap()) as usize
    }

    /// A tiny ZIP reader: checks the end record, each central entry and its local header
    /// (signatures, stored, sizes, CRC, offsets) and returns the files in order.
    fn unzip(b: &[u8]) -> Vec<(String, Vec<u8>)> {
        let end = b.len() - 22;
        assert_eq!(u32_at(b, end), 0x0605_4b50, "end record");
        let count = u16_at(b, end + 10);
        assert_eq!(u16_at(b, end + 8), count);
        let (size, mut at) = (u32_at(b, end + 12), u32_at(b, end + 16));
        assert_eq!(at + size, end, "central directory ends at the end record");
        let mut files = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(b, at), 0x0201_4b50, "central entry");
            assert_eq!(u16_at(b, at + 10), 0, "stored");
            let crc = u32_at(b, at + 16) as u32;
            let (packed, plain) = (u32_at(b, at + 20), u32_at(b, at + 24));
            assert_eq!(packed, plain);
            let name_len = u16_at(b, at + 28);
            let extra = u16_at(b, at + 30) + u16_at(b, at + 32);
            let local = u32_at(b, at + 42);
            let name = String::from_utf8(b[at + 46..at + 46 + name_len].to_vec()).unwrap();
            assert_eq!(u32_at(b, local), 0x0403_4b50, "local header of {name}");
            assert_eq!(u32_at(b, local + 14) as u32, crc);
            assert_eq!(&b[local + 30..local + 30 + name_len], name.as_bytes());
            let data_at = local + 30 + name_len + u16_at(b, local + 28);
            let data = b[data_at..data_at + plain].to_vec();
            assert_eq!(crc32(&data), crc, "CRC of {name}");
            files.push((name, data));
            at += 46 + name_len + extra;
        }
        assert_eq!(at, end);
        files
    }

    fn part(files: &[(String, Vec<u8>)], name: &str) -> String {
        let (_, data) = files.iter().find(|(n, _)| n == name).expect(name);
        String::from_utf8(data.clone()).unwrap()
    }

    /// Asserts every element of `xml` is closed in order.
    fn assert_well_formed(xml: &str) {
        let mut open: Vec<&str> = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find('<') {
            let end = start + rest[start..].find('>').expect("tag ends");
            let tag = &rest[start + 1..end];
            rest = &rest[end + 1..];
            if tag.starts_with('?') || tag.ends_with('/') {
                continue;
            }
            let name = tag.split_whitespace().next().unwrap();
            match name.strip_prefix('/') {
                Some(close) => assert_eq!(open.pop(), Some(close), "in {xml}"),
                None => open.push(name),
            }
        }
        assert!(open.is_empty(), "unclosed {open:?}");
    }

    fn no_links(_: &str) -> Option<PathBuf> {
        None
    }

    fn document(md: &str) -> String {
        let files = unzip(&docx_with(md, false, Path::new("/nonexistent"), &no_links));
        let doc = part(&files, "word/document.xml");
        assert_well_formed(&doc);
        doc
    }

    /// A PNG's signature and IHDR (enough for its size), `w` × `h`.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b
    }

    #[test]
    fn crc_and_zip_structure() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
        let files = vec![
            ("a.txt".to_string(), b"hello".to_vec()),
            ("dir/b.bin".to_string(), vec![0, 1, 2, 255]),
            ("empty".to_string(), Vec::new()),
        ];
        assert_eq!(unzip(&zip(&files)), files);
    }

    #[test]
    fn package_parts_are_listed_and_well_formed() {
        let files = unzip(&docx_with("# Hi", false, Path::new("/"), &no_links));
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "[Content_Types].xml",
                "_rels/.rels",
                "word/document.xml",
                "word/styles.xml",
                "word/numbering.xml",
                "word/_rels/document.xml.rels"
            ]
        );
        for (name, _) in &files {
            assert_well_formed(&part(&files, name));
        }
        let types = part(&files, "[Content_Types].xml");
        assert!(types.contains("PartName=\"/word/document.xml\""));
        assert!(part(&files, "_rels/.rels").contains("Target=\"word/document.xml\""));
        let styles = part(&files, "word/styles.xml");
        for n in 1..=6 {
            assert!(styles.contains(&format!(
                "w:styleId=\"Heading{n}\"><w:name w:val=\"heading {n}\"/>"
            )));
        }
        assert!(styles.contains("w:styleId=\"Code\""));
        assert!(styles.contains("w:styleId=\"Quote\""));
        assert!(styles.contains("Courier New"));
    }

    #[test]
    fn headings_emphasis_and_links() {
        let doc = document(
            "# Big\n\n###### Small\n\n**b** *i* ~~s~~ `c` ==m== <b>x</b> & <\n\n[site](https://a.b/c?d=1&e=2)",
        );
        assert!(doc.contains(
            "<w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t xml:space=\"preserve\">Big</w:t>"
        ));
        assert!(doc.contains("<w:pStyle w:val=\"Heading6\"/>"));
        assert!(doc.contains("<w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">b</w:t>"));
        assert!(doc.contains("<w:rPr><w:i/></w:rPr>"));
        assert!(doc.contains("<w:rPr><w:strike/></w:rPr>"));
        assert!(
            doc.contains(
                "<w:rStyle w:val=\"CodeChar\"/></w:rPr><w:t xml:space=\"preserve\">c</w:t>"
            )
        );
        assert!(doc.contains(
            "<w:highlight w:val=\"yellow\"/></w:rPr><w:t xml:space=\"preserve\">m</w:t>"
        ));
        assert!(doc.contains("&amp; &lt;"));
        assert!(!doc.contains("<b>"));
        assert!(doc.contains("<w:hyperlink r:id=\"rId3\" w:history=\"1\"><w:r><w:rPr><w:rStyle w:val=\"Hyperlink\"/></w:rPr><w:t xml:space=\"preserve\">site</w:t>"));
        let files = unzip(&docx_with(
            "[site](https://a.b/c?d=1&e=2)",
            false,
            Path::new("/"),
            &no_links,
        ));
        let rels = part(&files, "word/_rels/document.xml.rels");
        assert!(rels.contains("Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"https://a.b/c?d=1&amp;e=2\" TargetMode=\"External\""));
    }

    #[test]
    fn lists_nest_and_number() {
        let files = unzip(&docx_with(
            "- one\n  - inner\n\n3. x\n4. y\n\n- [ ] open\n- [x] done\n\n1. [ ] num task\n",
            false,
            Path::new("/"),
            &no_links,
        ));
        let doc = part(&files, "word/document.xml");
        assert_well_formed(&doc);
        assert!(doc.contains("<w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr><w:ind w:left=\"360\" w:hanging=\"360\"/></w:pPr><w:r><w:t xml:space=\"preserve\">one"));
        assert!(doc.contains("<w:numPr><w:ilvl w:val=\"1\"/><w:numId w:val=\"1\"/></w:numPr><w:ind w:left=\"720\" w:hanging=\"360\"/></w:pPr><w:r><w:t xml:space=\"preserve\">inner"));
        assert!(doc.contains("<w:numId w:val=\"4\"/></w:numPr><w:ind w:left=\"360\" w:hanging=\"360\"/></w:pPr><w:r><w:t xml:space=\"preserve\">x"));
        assert!(doc.contains("<w:numId w:val=\"2\"/></w:numPr><w:ind w:left=\"360\" w:hanging=\"360\"/></w:pPr><w:r><w:t xml:space=\"preserve\">open"));
        assert!(doc.contains("<w:numId w:val=\"3\"/></w:numPr><w:ind w:left=\"360\" w:hanging=\"360\"/></w:pPr><w:r><w:t xml:space=\"preserve\">done"));
        assert!(doc.contains("<w:numId w:val=\"5\"/></w:numPr><w:ind w:left=\"360\" w:hanging=\"360\"/></w:pPr><w:r><w:t xml:space=\"preserve\">\u{2610} </w:t></w:r><w:r><w:t xml:space=\"preserve\">num task"));
        let numbering = part(&files, "word/numbering.xml");
        assert!(numbering.contains("<w:num w:numId=\"4\"><w:abstractNumId w:val=\"1\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"3\"/>"));
        assert!(numbering.contains("<w:lvlText w:val=\"\u{2610}\"/>"));
        assert!(numbering.contains("<w:lvlText w:val=\"\u{2611}\"/>"));
        assert!(numbering.contains("<w:numFmt w:val=\"decimal\"/><w:lvlText w:val=\"%1.\"/>"));
        assert!(numbering.contains("<w:numFmt w:val=\"bullet\"/><w:lvlText w:val=\"\u{2022}\"/>"));
    }

    #[test]
    fn quotes_callouts_code_math_tables_rules() {
        let doc = document(
            "> quoted\n\n> [!WARNING]\n> Mind the gap\n\n```\na {\n\tb\n}\n```\n\n$x^2$\n\n| h1 | h2 |\n|----|---:|\n| a | b |\n\n---\n\nend[^1]\n\n[^1]: note\n",
        );
        assert!(doc.contains("<w:pStyle w:val=\"Quote\"/><w:pBdr><w:left w:val=\"single\" w:sz=\"18\" w:space=\"8\" w:color=\"D0D7DE\"/></w:pBdr><w:ind w:left=\"720\"/></w:pPr><w:r><w:t xml:space=\"preserve\">quoted"));
        // The warning's title in its colour (#9a6700), its text in a border of that colour.
        assert!(doc.contains("<w:rPr><w:b/><w:color w:val=\"9A6700\"/></w:rPr><w:t xml:space=\"preserve\">Warning</w:t>"));
        assert!(doc.contains("w:color=\"9A6700\"/></w:pBdr><w:ind w:left=\"720\"/></w:pPr><w:r><w:t xml:space=\"preserve\">Mind the gap"));
        assert!(!doc.contains("[!WARNING]"));
        assert!(doc.contains("<w:pStyle w:val=\"Code\"/></w:pPr><w:r><w:t xml:space=\"preserve\">a {</w:t><w:br/><w:tab/><w:t xml:space=\"preserve\">b</w:t><w:br/><w:t xml:space=\"preserve\">}</w:t></w:r>"));
        assert!(doc.contains(">$x^2$</w:t>"));
        assert_eq!(doc.matches("<w:tbl>").count(), 1);
        assert_eq!(doc.matches("<w:tr>").count(), 2);
        assert_eq!(doc.matches("<w:tc>").count(), 4);
        assert!(doc.contains("<w:tblBorders><w:top w:val=\"single\""));
        assert!(doc.contains("<w:trPr><w:tblHeader/></w:trPr>"));
        assert!(doc.contains("<w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">h1</w:t>"));
        assert!(
            doc.contains("<w:jc w:val=\"right\"/></w:pPr><w:r><w:t xml:space=\"preserve\">b</w:t>")
        );
        assert!(doc.contains("<w:pBdr><w:bottom w:val=\"single\""));
        assert!(doc.contains(
            "<w:vertAlign w:val=\"superscript\"/></w:rPr><w:t xml:space=\"preserve\">[1]</w:t>"
        ));
        assert!(doc.contains(">[1]\t</w:t>") || doc.contains(">[1]</w:t><w:tab/>"));
        // An item that starts with a code block: its marker on an empty paragraph first.
        let doc = document("- ```\n  x\n  ```\n");
        assert!(doc.contains("<w:numId w:val=\"1\"/></w:numPr><w:ind w:left=\"360\" w:hanging=\"360\"/></w:pPr></w:p><w:p><w:pPr><w:pStyle w:val=\"Code\"/><w:ind w:left=\"360\"/>"));
    }

    #[test]
    fn smart_punctuation_front_matter_and_html() {
        let md = "---\ntitle: x\n---\n\n<div>raw</div>\n\n\"Quoted\" -- it's";
        let on = unzip(&docx_with(md, true, Path::new("/"), &no_links));
        let doc = part(&on, "word/document.xml");
        assert!(doc.contains("\u{201c}Quoted\u{201d} \u{2013} it\u{2019}s"));
        assert!(!doc.contains("title:"));
        assert!(!doc.contains(">raw<"));
        assert!(document(md).contains("&quot;Quoted&quot; -- it's"));
    }

    #[test]
    fn images_are_embedded_once_and_listed() {
        let folder = TestFolder::new(
            "docx",
            &[
                ("Note.md", ""),
                ("img/a b.png", ""),
                ("pics/pic.png", ""),
                ("img/fake.png", "text"),
            ],
        );
        std::fs::write(folder.0.join("img/a b.png"), png(200, 100)).unwrap();
        std::fs::write(folder.0.join("pics/pic.png"), png(9600, 100)).unwrap();
        let links = folder.links("Note.md");
        let md = "![Alt](img/a%20b.png) ![Alt|50](img/a%20b.png) ![[pic.png]] ![[gone.png]] \
                  ![[Note]] ![web](https://e.com/x.png) ![nope](img/missing.png) ![f](img/fake.png)";
        let files = unzip(&docx_with(md, false, &folder.0, &links));
        let doc = part(&files, "word/document.xml");
        assert_well_formed(&doc);
        // 200 × 100 px at 96 dpi; `|50` → 50 × 25; 9600 px wide → the text width.
        assert!(doc.contains("<wp:extent cx=\"1905000\" cy=\"952500\"/>"));
        assert!(doc.contains("<wp:extent cx=\"476250\" cy=\"238125\"/>"));
        assert!(doc.contains("<wp:extent cx=\"5731510\" cy=\"59703\"/>"));
        assert_eq!(doc.matches("<w:drawing>").count(), 3);
        assert!(doc.contains("descr=\"Alt\""));
        assert!(doc.contains("<a:blip r:embed=\"rId3\"/>"));
        assert!(doc.contains("<a:blip r:embed=\"rId4\"/>"));
        // Missing, web and fake images as their alt text; note embeds as written.
        assert!(doc.contains(">gone.png</w:t>"));
        assert!(!doc.contains("![[gone.png]]"));
        assert!(doc.contains("![[Note]]"));
        assert!(doc.contains(">web</w:t>"));
        assert!(doc.contains(">nope</w:t>"));
        assert!(doc.contains(">f</w:t>"));
        let rels = part(&files, "word/_rels/document.xml.rels");
        assert!(rels.contains("Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/image1.png\"/>"));
        assert!(rels.contains("Id=\"rId4\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"media/image2.png\"/>"));
        assert!(!rels.contains("rId5"));
        assert!(
            part(&files, "[Content_Types].xml")
                .contains("<Default Extension=\"png\" ContentType=\"image/png\"/>")
        );
        let (_, media) = files
            .iter()
            .find(|(n, _)| n == "word/media/image1.png")
            .unwrap();
        assert_eq!(media, &png(200, 100));
        assert!(files.iter().any(|(n, _)| n == "word/media/image2.png"));
    }

    #[test]
    fn picture_sizes() {
        assert_eq!(pixel_size(&png(3, 4)), Some(("png", 3, 4)));
        assert_eq!(pixel_size(b"GIF89a\x05\x00\x07\x00"), Some(("gif", 5, 7)));
        // SOI, an APP0 segment (length 4), then SOF0: precision, height 0x0102, width 0x0304.
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 1, 2, 3, 4, 3,
        ];
        assert_eq!(pixel_size(&jpeg), Some(("jpeg", 0x0304, 0x0102)));
        assert_eq!(pixel_size(b"<svg/>"), None);
        assert_eq!(pixel_size(&[0xff, 0xd8, 0xff]), None);
        assert_eq!(percent_decode("a%20b%zz%C3%A9"), "a b%zzé");
        assert_eq!(xml("a\u{1}<\"b\">"), "a&lt;&quot;b&quot;&gt;");
    }
}
