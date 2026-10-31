//! Completion in the editor — `[[` note names, `[[Note#` headings, `#tag`s and code fence
//! languages: finds what is being typed, ranks suggestions and builds the text to insert.
//! Pure logic; the list is `input::Completer`.

use std::path::{Path, PathBuf};

use crate::render::HeadingTarget;
use crate::{lists, tags};

/// Most suggestions shown.
pub const LIMIT: usize = 8;

/// The query typed after an unclosed `[[` on the cursor's line: `line` is the line up to the
/// cursor, `above` the text before the line (to skip fenced code). `None` when the link is
/// closed, has a `|label`, is inside `` `code` `` or a code block.
pub fn query<'a>(above: &str, line: &'a str) -> Option<&'a str> {
    let start = line.rfind("[[")?;
    let q = &line[start + 2..];
    let in_code = line[..start].matches('`').count() % 2 == 1;
    (!q.contains(']') && !q.contains('|') && !in_code && !lists::in_fence(above)).then_some(q)
}

/// Link name for `path`: file name without `.md`, or `Folder/Name` (relative to `root`) when
/// another note has the same name (any case, `.md` or `.markdown`). Used by `[[` completion,
/// Outline ▸ Copy Link and the sidebar's Copy Wiki Link.
pub fn link_name(path: &Path, notes: &[PathBuf], root: &Path) -> String {
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_lowercase());
    let dup = notes.iter().any(|n| n != path && stem(n) == stem(path));
    let name = if dup {
        path.strip_prefix(root).unwrap_or(path).with_extension("")
    } else {
        PathBuf::from(path.file_stem().unwrap_or_default())
    };
    name.to_string_lossy().into_owned()
}

/// Text replacing the query: the name, then `]]` unless `after` (the line after the cursor)
/// already starts with it.
pub fn insertion(name: &str, after: &str) -> String {
    if after.starts_with("]]") {
        name.to_string()
    } else {
        format!("{name}]]")
    }
}

/// The tag typed right before the cursor, without `#` (`line` is the line up to the cursor,
/// `above` the text before it): only where `tags::in_line` sees a tag, so not in code, a
/// heading's `# `, a URL or `#1`. A trailing `/` or `-` ends the query (no completion).
pub fn tag_query<'a>(above: &str, line: &'a str) -> Option<&'a str> {
    let r = tags::in_line(line).pop().filter(|r| r.end == line.len())?;
    // `in_line` reads an unclosed `` ` `` or `](` as text; while typing they are code / a link.
    let before = &line[..r.start];
    let in_code = before.matches('`').count() % 2 == 1;
    let in_link = before
        .rfind("](")
        .is_some_and(|i| !before[i..].contains(')'));
    (!in_code && !in_link && !lists::in_fence(above)).then_some(&line[r.start + 1..])
}

/// Tags from `index` (`tags::index`) matching `query`: starting with it, or holding its
/// letters in order; ignoring case. Most notes first, then by name; the tag equal to the query
/// is left out (inserting it adds nothing); at most `LIMIT`.
pub fn tag_suggestions(query: &str, index: &[(String, Vec<PathBuf>)]) -> Vec<String> {
    let q = query.to_lowercase();
    let in_order = |tag: &str| {
        let mut t = tag.chars();
        q.chars().all(|c| t.any(|x| x == c))
    };
    let mut found: Vec<(bool, usize, &str)> = index
        .iter()
        .filter(|(tag, _)| tag.as_str() != query)
        .filter_map(|(tag, notes)| {
            let lower = tag.to_lowercase();
            let prefix = lower.starts_with(&q);
            (prefix || in_order(&lower)).then_some((!prefix, notes.len(), tag.as_str()))
        })
        .collect();
    found.sort_by(|a, b| (a.0, b.1, a.2).cmp(&(b.0, a.1, b.2)));
    found
        .into_iter()
        .take(LIMIT)
        .map(|f| f.2.to_string())
        .collect()
}

/// Languages offered after ```` ``` ```` / `~~~`: tokens the preview highlights (syntect),
/// short names where people use them, plus `math` and `mermaid`; sorted.
pub const FENCE_LANGUAGES: &[&str] = &[
    "applescript",
    "bash",
    "bat",
    "c",
    "clojure",
    "cpp",
    "cs",
    "css",
    "d",
    "diff",
    "dot",
    "erlang",
    "go",
    "groovy",
    "haskell",
    "html",
    "java",
    "js",
    "json",
    "latex",
    "lisp",
    "lua",
    "makefile",
    "markdown",
    "math",
    "matlab",
    "mermaid",
    "objective-c",
    "ocaml",
    "pascal",
    "perl",
    "php",
    "py",
    "r",
    "ruby",
    "rust",
    "scala",
    "sh",
    "sql",
    "tcl",
    "ts",
    "tsx",
    "typescript",
    "xml",
    "yml",
];

/// The language typed after an opening code fence (```` ``` ```` or `~~~`, up to 3 spaces
/// in) on the cursor's line: `line` is the line up to the cursor, `above` the text before it.
/// Starts with a letter. None for a fence that closes a block, or with nothing typed yet (so
/// ```` ``` ```` + Enter stays a new line).
pub fn fence_query<'a>(above: &str, line: &'a str) -> Option<&'a str> {
    let t = line.trim_start_matches(' ');
    let fence = t.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = t.len() - t.trim_start_matches(fence).len();
    let q = &t[run..];
    let word = q.starts_with(char::is_alphabetic)
        && q.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '+' | '#' | '-' | '_' | '.'));
    (line.len() - t.len() < 4 && run >= 3 && word && !lists::in_fence(above)).then_some(q)
}

/// `names` matching `query`: those starting with it, then those holding its letters in order;
/// ignoring case; in the given order within each group.
fn matching<'a, T>(query: &str, items: &'a [T], name: impl Fn(&T) -> &str) -> Vec<&'a T> {
    let q = query.to_lowercase();
    let in_order = |s: &str| {
        let mut t = s.chars();
        q.chars().all(|c| t.any(|x| x == c))
    };
    let (mut prefix, mut rest) = (Vec::new(), Vec::new());
    for item in items {
        let lower = name(item).to_lowercase();
        if lower.starts_with(&q) {
            prefix.push(item);
        } else if in_order(&lower) {
            rest.push(item);
        }
    }
    prefix.extend(rest);
    prefix
}

/// Fence languages matching `query`, the one already typed left out; at most `LIMIT`.
pub fn fence_suggestions(query: &str) -> Vec<&'static str> {
    matching(query, FENCE_LANGUAGES, |l| l)
        .into_iter()
        .filter(|l| **l != query)
        .take(LIMIT)
        .copied()
        .collect()
}

/// `[[Note#query` split into the note name and the heading query (`query`'s result); the
/// note is empty for `[[#query` (this note).
pub fn heading_query(link: &str) -> Option<(&str, &str)> {
    link.split_once('#')
}

/// Headings (`render::heading_targets`) whose title matches `query`, in document order
/// within the starting-with / letters-in-order groups; at most `LIMIT`.
pub fn heading_suggestions<'a>(
    query: &str,
    headings: &'a [HeadingTarget],
) -> Vec<&'a HeadingTarget> {
    matching(query, headings, |h| &h.title)
        .into_iter()
        .take(LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(tags: &[(&str, usize)]) -> Vec<(String, Vec<PathBuf>)> {
        tags.iter()
            .map(|&(t, n)| (t.to_string(), vec![PathBuf::new(); n]))
            .collect()
    }

    #[test]
    fn finds_tag_being_typed() {
        assert_eq!(tag_query("", "#wo"), Some("wo"));
        assert_eq!(tag_query("", "see (#work/pr"), Some("work/pr"));
        assert_eq!(tag_query("", "x #việc-l"), Some("việc-l"));
        assert_eq!(tag_query("", "#wo "), None);
        assert_eq!(tag_query("", "#"), None);
        assert_eq!(tag_query("", "#1"), None);
        assert_eq!(tag_query("", "# Head"), None);
        assert_eq!(tag_query("", "## H"), None);
        assert_eq!(tag_query("", "abc#wo"), None);
        assert_eq!(tag_query("", "http://x/#wo"), None);
        assert_eq!(tag_query("", "[a](#wo"), None);
        assert_eq!(tag_query("", "`#wo"), None);
        assert_eq!(tag_query("", "`x` #wo"), Some("wo"));
        assert_eq!(tag_query("```\n", "#wo"), None);
    }

    #[test]
    fn ranks_tags() {
        let index = idx(&[
            ("home", 1),
            ("work", 2),
            ("work/project", 1),
            ("Wiki", 3),
            ("việc-làm", 4),
        ]);
        assert_eq!(tag_suggestions("wo", &index), ["work", "work/project"]);
        assert_eq!(
            tag_suggestions("w", &index),
            ["Wiki", "work", "work/project"]
        );
        assert_eq!(tag_suggestions("wp", &index), ["work/project"]);
        assert_eq!(tag_suggestions("vi", &index), ["việc-làm"]);
        assert_eq!(tag_suggestions("work", &index), ["work/project"]);
        assert!(tag_suggestions("zz", &index).is_empty());
    }

    #[test]
    fn finds_open_link() {
        assert_eq!(query("", "see [[abc"), Some("abc"));
        assert_eq!(query("", "[["), Some(""));
        assert_eq!(query("", "[[a]] and [[Pro/x y"), Some("Pro/x y"));
    }

    #[test]
    fn ignores_closed_label_and_code() {
        assert_eq!(query("", "[[abc]]"), None);
        assert_eq!(query("", "[[abc]] x"), None);
        assert_eq!(query("", "[[abc|text"), None);
        assert_eq!(query("", "no link"), None);
        assert_eq!(query("", "`[[abc"), None);
        assert_eq!(query("", "`x` [[abc"), Some("abc"));
        assert_eq!(query("```\n", "[[abc"), None);
        assert_eq!(query("```\ncode\n```\n", "[[abc"), Some("abc"));
    }

    #[test]
    fn names_notes() {
        let root = Path::new("/n");
        let notes: Vec<PathBuf> = [
            "a/Plan.md",
            "b/plan.md",
            "Idea.md",
            "x/Sub/Page.md",
            "Page.markdown",
            "c/d/Deep.md",
        ]
        .iter()
        .map(|p| root.join(p))
        .collect();
        assert_eq!(link_name(&notes[2], &notes, root), "Idea");
        assert_eq!(link_name(&notes[0], &notes, root), "a/Plan");
        assert_eq!(link_name(&notes[1], &notes, root), "b/plan");
        // Any extension of a note, folders from the root; a top-level note has none to add.
        assert_eq!(link_name(&notes[3], &notes, root), "x/Sub/Page");
        assert_eq!(link_name(&notes[4], &notes, root), "Page");
        assert_eq!(link_name(&notes[5], &notes, root), "Deep");
    }

    #[test]
    fn fence_trigger() {
        assert_eq!(fence_query("", "```p"), Some("p"));
        assert_eq!(fence_query("text\n", "~~~~ru"), Some("ru"));
        assert_eq!(fence_query("", "   ```js"), Some("js"));
        assert_eq!(fence_query("", "```objective-c"), Some("objective-c"));
        // Nothing typed yet, not a letter, too few marks, indented code, inline code.
        assert_eq!(fence_query("", "```"), None);
        assert_eq!(fence_query("", "```1"), None);
        assert_eq!(fence_query("", "``py"), None);
        assert_eq!(fence_query("", "    ```py"), None);
        assert_eq!(fence_query("", "x ```py"), None);
        assert_eq!(fence_query("", "```py x"), None);
        // A fence closing a block (or inside one) is not an opening.
        assert_eq!(fence_query("```py\ncode\n", "```p"), None);
        assert_eq!(fence_query("~~~\n", "```p"), None);
        assert_eq!(fence_query("```\ncode\n```\n", "```p"), Some("p"));
    }

    #[test]
    fn fence_languages() {
        let sorted = FENCE_LANGUAGES.windows(2).all(|w| w[0] < w[1]);
        assert!(sorted, "sorted, no repeats");
        // Every entry is highlighted by the preview, except the two rendered otherwise.
        for lang in FENCE_LANGUAGES {
            let known = crate::render::highlights(lang);
            assert!(known || ["math", "mermaid"].contains(lang), "{lang}");
        }
        for short in ["js", "sh", "yml", "py", "ts", "tsx"] {
            assert!(FENCE_LANGUAGES.contains(&short), "{short}");
        }
        assert_eq!(
            fence_suggestions("p"),
            [
                "pascal",
                "perl",
                "php",
                "py",
                "applescript",
                "cpp",
                "lisp",
                "typescript"
            ]
        );
        assert_eq!(fence_suggestions("PY"), ["py"]);
        assert_eq!(fence_suggestions("py"), Vec::<&str>::new());
        assert_eq!(&fence_suggestions("me")[..1], ["mermaid"]);
        assert_eq!(fence_suggestions("rst"), ["rust"]);
        assert_eq!(
            &fence_suggestions("t")[..4],
            ["tcl", "ts", "tsx", "typescript"]
        );
        assert!(fence_suggestions("zz").is_empty());
    }

    #[test]
    fn heading_links() {
        assert_eq!(heading_query("Plan#Go"), Some(("Plan", "Go")));
        assert_eq!(heading_query("#"), Some(("", "")));
        assert_eq!(heading_query("Plan"), None);
        let md = "# Setup\n\n## Install\n\n## Setup\n\n## Use it\n";
        let all = crate::render::heading_targets(md);
        let got = |q| {
            heading_suggestions(q, &all)
                .into_iter()
                .map(|h| h.fragment.as_str())
                .collect::<Vec<_>>()
        };
        // A repeated title is offered twice: the second inserts its id.
        assert_eq!(got(""), ["Setup", "Install", "setup-1", "Use it"]);
        assert_eq!(got("se"), ["Setup", "setup-1", "Use it"]);
        assert_eq!(got("in"), ["Install"]);
        assert!(got("zz").is_empty());
    }

    #[test]
    fn closes_link_once() {
        assert_eq!(insertion("Idea", ""), "Idea]]");
        assert_eq!(insertion("Idea", " more"), "Idea]]");
        assert_eq!(insertion("Idea", "]] more"), "Idea");
    }
}
