//! Outline (Alt+Ctrl+O): the headings of a note. Pure logic, no GTK.

use crate::lists;

#[derive(Debug, PartialEq)]
pub struct Heading {
    /// 1 (`#`) to 6 (`######`).
    pub level: usize,
    pub title: String,
    /// 1-based line of the heading text.
    pub line: usize,
}

/// ATX (`#` … `######`) and Setext (`===` / `---` under a text line) headings, skipping
/// front matter, fenced code blocks and empty headings. `#tag` (no space after `#`) is not a heading.
pub fn headings(text: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut open = None;
    // The previous line, if it can be the text of a Setext heading.
    let mut prev: Option<&str> = None;
    let skip = lists::front_matter_lines(text);
    for (i, line) in text.lines().enumerate().skip(skip) {
        if lists::fence_step(&mut open, line) || open.is_some() {
            prev = None;
            continue;
        }
        if let Some((level, title)) = atx(line) {
            if !title.is_empty() {
                out.push(Heading {
                    level,
                    title: title.to_string(),
                    line: i + 1,
                });
            }
            prev = None;
            continue;
        }
        if let (Some(level), Some(title)) = (setext(line), prev) {
            out.push(Heading {
                level,
                title: title.trim().to_string(),
                line: i,
            });
            prev = None;
            continue;
        }
        // shortcut: only the last line of a multi-line paragraph becomes the Setext title.
        let indent = line.len() - line.trim_start_matches(' ').len();
        prev = (!line.trim().is_empty()
            && indent < 4
            && !lists::is_item(line)
            && !line.trim_start().starts_with('>'))
        .then_some(line);
    }
    out
}

/// Index of the heading whose section holds 1-based `line`: the last heading at or above it.
/// `lines` are the heading lines in order; `None` before the first heading.
pub fn current(lines: &[usize], line: usize) -> Option<usize> {
    lines.partition_point(|&l| l <= line).checked_sub(1)
}

/// `## Title ##` → (2, "Title"): at most 3 spaces of indent, 1–6 `#`, then a space/tab or
/// the line end; an optional closing `#` run (after a space) is dropped.
fn atx(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let level = t.len() - t.trim_start_matches('#').len();
    let rest = &t[level..];
    if !(1..=6).contains(&level) || !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let rest = rest.trim();
    let closed = rest.trim_end_matches('#');
    Some((
        level,
        if closed.is_empty() || closed.ends_with([' ', '\t']) {
            closed.trim_end()
        } else {
            rest
        },
    ))
}

/// `===` → 1, `---` → 2 (at most 3 spaces of indent, trailing spaces allowed).
fn setext(line: &str) -> Option<usize> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return None;
    }
    let t = t.trim_end();
    match t.chars().next()? {
        '=' if t.chars().all(|c| c == '=') => Some(1),
        '-' if t.chars().all(|c| c == '-') => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(text: &str) -> Vec<(usize, String, usize)> {
        headings(text)
            .into_iter()
            .map(|h| (h.level, h.title, h.line))
            .collect()
    }

    #[test]
    fn atx_levels() {
        let text =
            "# One\ntext\n## Two\n### Three\n#### Four\n##### Five\n###### Six\n####### Seven";
        let got = h(text);
        assert_eq!(got.len(), 6);
        assert_eq!(got[0], (1, "One".into(), 1));
        assert_eq!(got[1], (2, "Two".into(), 3));
        assert_eq!(got[5], (6, "Six".into(), 7));
    }

    #[test]
    fn atx_details() {
        assert_eq!(h("   # Indented"), [(1, "Indented".into(), 1)]);
        assert!(h("    # code").is_empty());
        assert_eq!(h("## Closed ##"), [(2, "Closed".into(), 1)]);
        assert_eq!(h("# C#"), [(1, "C#".into(), 1)]);
        assert_eq!(h("#\ttab"), [(1, "tab".into(), 1)]);
        assert!(h("#").is_empty());
        assert!(h("# ##").is_empty());
    }

    #[test]
    fn tags_are_not_headings() {
        assert!(h("#tag #other\n#123").is_empty());
    }

    #[test]
    fn special_characters() {
        assert_eq!(
            h("# Tiếng Việt <b>&amp; \"quotes\" `code` 🚀"),
            [(1, "Tiếng Việt <b>&amp; \"quotes\" `code` 🚀".into(), 1)]
        );
    }

    #[test]
    fn code_blocks_skipped() {
        let text = "# A\n```\n# not\n```\n~~~sh\n## nor\n```\n## still not\n~~~\n## B";
        assert_eq!(h(text), [(1, "A".into(), 1), (2, "B".into(), 10)]);
    }

    #[test]
    fn setext() {
        let text = "Title\n=====\n\nSub\n---\n\n---\n- item\n---\n> q\n---";
        assert_eq!(h(text), [(1, "Title".into(), 1), (2, "Sub".into(), 4)]);
    }

    #[test]
    fn current_heading() {
        let lines = [3, 7, 12];
        assert_eq!(current(&lines, 1), None);
        assert_eq!(current(&lines, 3), Some(0));
        assert_eq!(current(&lines, 6), Some(0));
        assert_eq!(current(&lines, 7), Some(1));
        assert_eq!(current(&lines, 12), Some(2));
        assert_eq!(current(&lines, 99), Some(2));
        assert_eq!(current(&[], 5), None);
    }

    #[test]
    fn front_matter_skipped() {
        let text = "---\ntitle: x\n---\n# A\n";
        assert_eq!(h(text), [(1, "A".into(), 4)]);
        assert_eq!(h("text\n---\ntitle: x\n---\n")[0].1, "text");
    }
}
