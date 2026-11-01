//! YAML front matter in the preview (Preferences ▸ Preview ▸ Front Matter): a properties table,
//! the block as written, or nothing. The table has one row per top-level key, the key as
//! written; a scalar shows its text, a flow or block list its items joined with ", ", a nested
//! map (or a list of maps, nested lists) its YAML in one `<code>` cell. Values stay as written
//! (booleans, numbers, dates), escaped, no Markdown. The YAML is read line by line with the tags
//! reader's rules (`tags::uncommented`, `tags::unquoted`), not a YAML parser; what this reader
//! cannot place (a line without `key:`, an unclosed `[`) shows the block as written instead.

use crate::state::FrontMatter;
use crate::tags;

/// A table row's value cell.
#[derive(Debug, PartialEq)]
pub(crate) enum Value {
    /// Shown as text.
    Text(String),
    /// Shown as YAML in a `<code>` cell.
    Code(String),
}

/// The HTML of `front` (comrak's front matter: `---`, the YAML, `---`, blank lines) as `mode`
/// says, before the note's first block. The table carries `data-line="1"` (as the macOS app)
/// and, with `sourcepos`, the front matter's lines in `data-sourcepos` for scroll sync. Front
/// matter with no keys shows nothing as a table.
pub(crate) fn html(front: &str, mode: FrontMatter, sourcepos: bool) -> String {
    match mode {
        FrontMatter::Hidden => String::new(),
        FrontMatter::Table => match rows(front) {
            Some(rows) => table(
                &rows,
                sourcepos.then(|| crate::lists::front_matter_lines(front)),
            ),
            None => source(front),
        },
        FrontMatter::Source => source(front),
    }
}

/// The front matter as written in a dimmed block.
fn source(front: &str) -> String {
    let mut out = String::from("<pre class=\"front-matter\">");
    escape(&mut out, front.trim_end());
    out + "</pre>\n"
}

fn table(rows: &[(String, Value)], lines: Option<usize>) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut out = String::from("<table class=\"front-matter\" data-line=\"1\"");
    if let Some(lines) = lines.filter(|&l| l > 0) {
        out += &format!(" data-sourcepos=\"1:1-{lines}:3\"");
    }
    out += ">\n<tbody>\n";
    for (key, value) in rows {
        out += "<tr><th>";
        escape(&mut out, key);
        out += "</th><td>";
        match value {
            Value::Text(text) => escape(&mut out, text),
            Value::Code(code) => {
                out += "<code>";
                escape(&mut out, code);
                out += "</code>";
            }
        }
        out += "</td></tr>\n";
    }
    out + "</tbody>\n</table>\n"
}

fn escape(out: &mut String, text: &str) {
    comrak::html::escape(out, text).expect("writing to a String");
}

/// The rows of `front` (see `html`), None if it is not YAML this reader understands.
pub(crate) fn rows(front: &str) -> Option<Vec<(String, Value)>> {
    // The lines between the two `---`.
    let body: Vec<&str> = front
        .lines()
        .skip(1)
        .take_while(|l| l.trim_end() != "---")
        .collect();
    let mut rows = Vec::new();
    let mut n = 0;
    while n < body.len() {
        let line = body[n];
        n += 1;
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        // Indented lines and items belong to the key above them.
        if indented(line) || is_item(line) {
            return None;
        }
        let (key, from) = key(line)?;
        let start = n;
        while body
            .get(n)
            .is_some_and(|l| l.trim().is_empty() || indented(l) || is_item(l))
        {
            n += 1;
        }
        let mut below = &body[start..n];
        while let [rest @ .., last] = below
            && last.trim().is_empty()
        {
            below = rest;
        }
        rows.push((key.to_string(), value(line, from, below)?));
    }
    Some(rows)
}

fn indented(line: &str) -> bool {
    line.starts_with(char::is_whitespace)
}

/// `- item` (or a lone `-`) with no indent.
fn is_item(line: &str) -> bool {
    line.strip_prefix('-')
        .is_some_and(|r| r.is_empty() || r.starts_with(char::is_whitespace))
}

/// The key of a `key: value` line (as written, quotes kept) and where its value starts, after
/// the `:`. A `:` counts when a space or the line's end follows it, outside a quoted key.
fn key(line: &str) -> Option<(&str, usize)> {
    let from = match line.as_bytes().first() {
        Some(&q @ (b'"' | b'\'')) => line[1..].find(q as char)? + 2,
        _ => 0,
    };
    let colon = line[from..]
        .match_indices(':')
        .map(|(i, _)| from + i)
        .find(|&i| line[i + 1..].is_empty() || line[i + 1..].starts_with(char::is_whitespace))?;
    let key = line[..colon].trim_end();
    (!key.is_empty()).then_some((key, colon + 1))
}

/// The value of the key whose line is `line`, its value from byte `from`, with the lines
/// `below` it that belong to it (indented, or `- ` items).
fn value(line: &str, from: usize, below: &[&str]) -> Option<Value> {
    let first = line[from..scalar_end(line, from)].trim();
    // Without blank lines and whole-line comments.
    let lines: Vec<&str> = below
        .iter()
        .copied()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .collect();
    if let Some(close) = match first.as_bytes().first() {
        Some(b'[') => Some(']'),
        Some(b'{') => Some('}'),
        _ => None,
    } {
        // Flow list or map, which may go on over the lines below up to its bracket.
        let mut all = first.to_string();
        for l in &lines {
            all.push(' ');
            all.push_str(l[..tags::uncommented(l, 0)].trim());
        }
        if all.len() < 2 || !all.ends_with(close) {
            return None;
        }
        let inner = &all[1..all.len() - 1];
        if close == '}' || inner.contains(['[', '{']) {
            return Some(Value::Code(all));
        }
        return Some(Value::Text(joined(inner.split(',').map(unquote))));
    }
    if first.starts_with('|') || first.starts_with('>') {
        // Block scalar: its lines, kept apart (`|`) or folded into one (`>`).
        let sep = if first.starts_with('|') { "\n" } else { " " };
        let text: Vec<&str> = below.iter().map(|l| l.trim()).collect();
        return Some(Value::Text(text.join(sep)));
    }
    if !first.is_empty() {
        // A plain scalar may go on over indented lines.
        let mut text = unquote(first).to_string();
        for l in &lines {
            text.push(' ');
            text.push_str(l.trim());
        }
        return Some(Value::Text(text));
    }
    let Some(&head) = lines.first() else {
        return Some(Value::Text(String::new()));
    };
    // Block list: `- item` lines at one indent, each a plain value.
    let indent = |l: &str| l.len() - l.trim_start().len();
    let at = indent(head);
    let items: Option<Vec<&str>> = lines
        .iter()
        .map(|l| (indent(l) == at).then(|| plain_item(&l[at..])).flatten())
        .collect();
    if let Some(items) = items {
        return Some(Value::Text(joined(items.into_iter())));
    }
    // A nested map, a list of maps, nested lists: the YAML below the key, dedented.
    let cut = lines.iter().map(|l| indent(l)).min().unwrap_or(0);
    let yaml: Vec<&str> = below
        .iter()
        .map(|l| l.get(cut..).unwrap_or("").trim_end())
        .collect();
    Some(Value::Code(yaml.join("\n")))
}

/// The value of `- value` when it is a plain scalar (not a map, a nested list or a flow
/// collection); a lone `-` is an empty value.
fn plain_item(line: &str) -> Option<&str> {
    if !is_item(line) {
        return None;
    }
    let rest = &line[1..];
    let item = rest[..scalar_end(rest, 0)].trim();
    let nested = item.starts_with(['[', '{', '|', '>']) || is_item(item) || key(item).is_some();
    (!nested).then(|| unquote(item))
}

/// Where the scalar starting at `from` in `line` ends: after its closing quote when quoted
/// (what follows, a comment, is dropped), else at a comment (`tags::uncommented`).
fn scalar_end(line: &str, from: usize) -> usize {
    let at = from + line[from..].len() - line[from..].trim_start().len();
    let bytes = line.as_bytes();
    if let Some(&q @ (b'"' | b'\'')) = bytes.get(at) {
        let mut i = at + 1;
        while i < bytes.len() {
            if q == b'"' && bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                if q == b'\'' && bytes.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                return i + 1;
            }
            i += 1;
        }
    }
    tags::uncommented(line, from)
}

/// `value` trimmed, without a pair of quotes around it.
fn unquote(value: &str) -> &str {
    let (from, to) = tags::unquoted(value, 0, value.len());
    &value[from..to]
}

/// The non-empty `items` (trimmed) joined with ", ".
fn joined<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    fn rows_of(yaml: &str) -> Option<Vec<(String, Value)>> {
        rows(&format!("---\n{yaml}---\n\n"))
    }

    #[test]
    fn scalars_as_written() {
        let rows = rows_of(
            "title: My note\ndone: true\ncount: 3\ndate: 2026-11-02\nquoted: \"a # b\" # note\nurl: https://x.org/a#b\nempty:\n",
        )
        .unwrap();
        let want = [
            ("title", "My note"),
            ("done", "true"),
            ("count", "3"),
            ("date", "2026-11-02"),
            ("quoted", "a # b"),
            ("url", "https://x.org/a#b"),
            ("empty", ""),
        ];
        assert_eq!(
            rows,
            want.map(|(k, v)| (k.to_string(), text(v)))
                .into_iter()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn lists_joined() {
        let rows = rows_of(
            "tags: [a, \"b c\", ]\naliases:\n  - One\n  - 'Two'\n# comment\nlist:\n- x\n-\n- y # cut\nflow: [a,\n  b]\n",
        )
        .unwrap();
        assert_eq!(rows[0], ("tags".into(), text("a, b c")));
        assert_eq!(rows[1], ("aliases".into(), text("One, Two")));
        assert_eq!(rows[2], ("list".into(), text("x, y")));
        assert_eq!(rows[3], ("flow".into(), text("a, b")));
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn nested_map_as_code() {
        let rows = rows_of(
            "author:\n  name: Ann\n  # who\n  links:\n    - a\npeople:\n  - name: B\n    age: 3\nmap: {a: 1}\nnested: [[a], b]\nnext: 1\n",
        )
        .unwrap();
        assert_eq!(
            rows[0],
            (
                "author".into(),
                Value::Code("name: Ann\n# who\nlinks:\n  - a".into())
            )
        );
        assert_eq!(
            rows[1],
            ("people".into(), Value::Code("- name: B\n  age: 3".into()))
        );
        assert_eq!(rows[2], ("map".into(), Value::Code("{a: 1}".into())));
        assert_eq!(rows[3], ("nested".into(), Value::Code("[[a], b]".into())));
        assert_eq!(rows[4], ("next".into(), text("1")));
    }

    #[test]
    fn block_scalars_and_continued_lines() {
        let rows = rows_of("a: |\n  one\n  two\nb: >\n  one\n  two\nc: long\n  line\n").unwrap();
        assert_eq!(rows[0], ("a".into(), text("one\ntwo")));
        assert_eq!(rows[1], ("b".into(), text("one two")));
        assert_eq!(rows[2], ("c".into(), text("long line")));
    }

    #[test]
    fn invalid_yaml_is_none() {
        assert_eq!(rows_of("just some text\n"), None);
        assert_eq!(rows_of("tags: [a, b\n"), None);
        assert_eq!(rows_of("  indented: 1\n"), None);
        assert_eq!(rows_of("- a\n- b\n"), None);
        assert_eq!(rows_of("key:value\n"), None);
        // Empty and comment-only front matter: no rows.
        assert_eq!(rows_of(""), Some(vec![]));
        assert_eq!(rows_of("# only\n"), Some(vec![]));
        // A quoted key keeps its quotes; a `:` inside it doesn't end it.
        assert_eq!(
            rows_of("\"a: b\": c\n").unwrap(),
            [("\"a: b\"".into(), text("c"))]
        );
    }

    #[test]
    fn three_settings() {
        let front = "---\ntitle: <b>&</b>\ntags: [a, b]\nmeta:\n  x: 1\n---\n\n";
        assert_eq!(
            html(front, FrontMatter::Table, true),
            "<table class=\"front-matter\" data-line=\"1\" data-sourcepos=\"1:1-6:3\">\n<tbody>\n\
<tr><th>title</th><td>&lt;b&gt;&amp;&lt;/b&gt;</td></tr>\n\
<tr><th>tags</th><td>a, b</td></tr>\n\
<tr><th>meta</th><td><code>x: 1</code></td></tr>\n\
</tbody>\n</table>\n"
        );
        assert!(
            html(front, FrontMatter::Table, false)
                .starts_with("<table class=\"front-matter\" data-line=\"1\">\n")
        );
        assert_eq!(
            html(front, FrontMatter::Source, true),
            "<pre class=\"front-matter\">---\ntitle: &lt;b&gt;&amp;&lt;/b&gt;\ntags: [a, b]\nmeta:\n  x: 1\n---</pre>\n"
        );
        assert_eq!(html(front, FrontMatter::Hidden, true), "");
        // Invalid YAML: the block as written; no keys: nothing.
        let bad = "---\nnot yaml\n---\n";
        assert_eq!(
            html(bad, FrontMatter::Table, true),
            html(bad, FrontMatter::Source, true)
        );
        assert_eq!(html("---\n---\n", FrontMatter::Table, true), "");
    }
}
