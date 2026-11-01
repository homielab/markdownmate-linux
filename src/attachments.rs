//! Find Unused Attachments (main menu, or the Unused Attachments toggle of Find in Folder): the
//! files in `assets` folders that no note of the open folder uses, so the user can clean them up
//! in Files. The app only lists and reveals them; it never deletes anything.
//!
//! Attachments: every regular file (not a symlink, not hidden, not a note) inside a folder named
//! `assets` at any depth of the open folder (or of the Find in “Name”… folder). Hidden folders and
//! `node_modules` are skipped, symlinks are neither followed nor listed.
//!
//! Used: a note of the open folder links to it, outside fenced code, code spans and the front
//! matter, through a relative link or image as Find Broken Links reads them (`[t](path)`,
//! `![a](path)`, `[t](<path>)`) or a `[ref]: path` definition (`#…` and `?…` dropped,
//! percent-decoded and also as written, relative to the note), or a raw HTML `<img src="path">`
//! with a relative path, or an `![[name.png]]` embed (every attachment whose path ends with that
//! name). `[[wiki links]]` don't count. Paths compare ignoring case and Unicode normalization, so
//! a doubt keeps a file off the list. Pure logic, no GTK; the dialog runs `find` on a background
//! thread.

use std::collections::HashSet;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::links::{self, Target};
use crate::{images, lists, notes, relink, state};

/// A file in an `assets` folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub path: PathBuf,
    /// In bytes.
    pub size: u64,
}

/// What `find` found: the unused attachments (in path order), their total size, and how many
/// notes could not be read (a file only they use is listed too).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub files: Vec<Attachment>,
    pub total: u64,
    pub unread: usize,
}

/// The attachments under `base` (the open folder `root`, or a folder inside it), sorted by their
/// path relative to `root` as the sidebar sorts names. None as soon as `cancelled` turns true.
pub fn attachments(
    base: &Path,
    root: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Option<Vec<Attachment>> {
    let mut found = Vec::new();
    let mut dirs = vec![base.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        if cancelled() {
            return None;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            // `file_type` does not follow symlinks: a link is neither a dir nor a file here.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() {
                if name != "node_modules" {
                    dirs.push(path);
                }
                continue;
            }
            if !kind.is_file() || notes::is_note(&path) || !in_assets(&path, root) {
                continue;
            }
            let size = entry.metadata().map_or(0, |m| m.len());
            found.push(Attachment { path, size });
        }
    }
    let relative = |p: &Path| {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .into_owned()
    };
    found.sort_by(|a, b| notes::compare_names(&relative(&a.path), &relative(&b.path)));
    Some(found)
}

/// Whether one of the folders between `root` and `file` is named `assets`.
fn in_assets(file: &Path, root: &Path) -> bool {
    let Some(dir) = file.parent() else {
        return false;
    };
    dir.strip_prefix(root)
        .is_ok_and(|rel| rel.components().any(|c| c.as_os_str() == images::ASSETS))
}

/// The files `text` (the note at `note`) links to or shows, as `key`s.
pub fn references(text: &str, note: &Path) -> HashSet<String> {
    let mut keys = HashSet::new();
    if !text.contains('[') && !text.contains('<') {
        return keys;
    }
    let skip = lists::front_matter_lines(text);
    let mut open = None;
    for (i, line) in text.lines().enumerate() {
        if i < skip || lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        let has_link = line.contains('[');
        let has_image = line.contains('<') && line.to_ascii_lowercase().contains("<img");
        if !has_link && !has_image {
            continue;
        }
        let mut targets: Vec<String> = Vec::new();
        if has_link {
            targets.extend(links::spans(line).into_iter().filter_map(|(_, t)| match t {
                Target::Href(href) => Some(href),
                Target::Wiki(_) => None,
            }));
            targets.extend(definition(line).map(str::to_string));
        }
        if has_image {
            let code = links::code_spans(line);
            let in_code =
                |r: &Range<usize>| code.iter().any(|c| c.start < r.end && r.start < c.end);
            for caps in IMG_SRC.captures_iter(line) {
                if caps.get(0).is_some_and(|m| in_code(&m.range())) {
                    continue;
                }
                if let Some(src) = (1..=3).find_map(|g| caps.get(g)) {
                    targets.push(src.as_str().trim().to_string());
                }
            }
        }
        let dir = note.parent().unwrap_or(Path::new(""));
        for target in targets {
            for path in paths(&target) {
                keys.insert(key(&relink::clean(&dir.join(path))));
            }
        }
    }
    keys
}

/// The images `text` embeds (`![[name.png]]`, `![[Folder/name.png|300]]`) outside fenced code,
/// code spans and the front matter, as `embed_key`s.
pub fn embeds(text: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    if !text.contains("![[") {
        return names;
    }
    let skip = lists::front_matter_lines(text);
    let mut open = None;
    for (i, line) in text.lines().enumerate() {
        if i < skip || lists::fence_step(&mut open, line) || open.is_some() {
            continue;
        }
        if !line.contains("![[") {
            continue;
        }
        for (range, target) in links::spans(line) {
            if let Target::Wiki(name) = target
                && links::is_embed(line, &range)
                && crate::render::is_embed_image(name.split('#').next().unwrap_or(""))
                && let Some(name) = embed_key(&name)
            {
                names.insert(name);
            }
        }
    }
    names
}

/// An embed's name as `embeds` keeps it: no `#…`, no leading or trailing `/`, NFC, lowercased.
fn embed_key(name: &str) -> Option<String> {
    let name = name.split('#').next()?.trim().trim_matches('/');
    let name = name.nfc().collect::<String>().to_lowercase();
    (!name.is_empty()).then_some(name)
}

/// Whether an `embeds` name may mean the file with this `key`: the key ends with `/` + it.
fn embedded(key: &str, names: &HashSet<String>) -> bool {
    !names.is_empty()
        && key
            .match_indices('/')
            .any(|(i, _)| names.contains(&key[i + 1..]))
}

/// `<img … src="…">`, `src='…'` or `src=…`; groups 1–3: the value.
static IMG_SRC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)<img\b[^>]*?\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#)
        .expect("constant pattern")
});

/// The destination of a link reference definition line (`[label]: path "title"`,
/// `[label]: <path>`); None for other lines and footnotes (`[^1]: text`).
fn definition(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(' ').strip_prefix('[')?;
    if rest.starts_with(['^', '[']) {
        return None;
    }
    let close = rest.find("]:")?;
    if close == 0 || rest[..close].contains(['[', ']']) {
        return None;
    }
    let dest = rest[close + 2..].trim_start();
    let dest = match dest.strip_prefix('<') {
        Some(inner) => &inner[..inner.find('>')?],
        None => dest.split_whitespace().next()?,
    };
    (!dest.is_empty()).then_some(dest)
}

/// The relative paths a link target names: percent-decoded, and also as written when that
/// differs (a `%` in a real file name); `#…` and `?…` dropped. None for URLs (`scheme:`),
/// absolute and `~` paths and `#anchor`-only links.
fn paths(target: &str) -> Vec<PathBuf> {
    let Some(raw) = target.split(['#', '?']).next() else {
        return Vec::new();
    };
    if raw.is_empty()
        || raw.starts_with(['/', '~'])
        || raw.split('/').next().is_some_and(|s| s.contains(':'))
    {
        return Vec::new();
    }
    let mut out = vec![PathBuf::from(raw)];
    if let Some(decoded) = state::unescape(raw)
        && decoded != out[0]
    {
        out.push(decoded);
    }
    out
}

/// A path's comparison key: NFC, lowercased.
pub fn key(path: &Path) -> String {
    path.to_string_lossy()
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

/// The `attachments` no note among `notes` uses. `read` gives a note's text (None: unreadable,
/// counted in `Found::unread`). None as soon as `cancelled` turns true.
pub fn unused(
    attachments: Vec<Attachment>,
    notes: &[PathBuf],
    read: &dyn Fn(&Path) -> Option<String>,
    cancelled: &dyn Fn() -> bool,
) -> Option<Found> {
    let mut found = Found::default();
    if attachments.is_empty() {
        return Some(found);
    }
    let mut used = HashSet::new();
    let mut names = HashSet::new();
    for note in notes {
        if cancelled() {
            return None;
        }
        match read(note) {
            Some(text) => {
                used.extend(references(&text, note));
                names.extend(embeds(&text));
            }
            None => found.unread += 1,
        }
    }
    for file in attachments {
        let key = key(&file.path);
        if !used.contains(&key) && !embedded(&key, &names) {
            found.total += file.size;
            found.files.push(file);
        }
    }
    Some(found)
}

/// The unused attachments under `base` (the open folder `root`, or a folder inside it), checked
/// against all of the open folder's `notes`; the open note (path, editor text) is read as in the
/// editor. None as soon as `cancelled` turns true.
pub fn find(
    base: &Path,
    root: &Path,
    notes: &[PathBuf],
    open: Option<&(PathBuf, String)>,
    cancelled: &dyn Fn() -> bool,
) -> Option<Found> {
    let files = attachments(base, root, cancelled)?;
    unused(
        files,
        notes,
        &|path| match open {
            Some((p, text)) if p == path => Some(text.clone()),
            _ => crate::find::read_utf8(path),
        },
        cancelled,
    )
}

/// "N unused files (S)" (`size` formats S), or "No unused attachments"; with "(first L listed)"
/// when more than `limit`. Notes that could not be read are named, since files they use are
/// listed too.
pub fn summary(found: &Found, limit: usize, size: &dyn Fn(u64) -> String) -> String {
    let n = found.files.len();
    let mut out = match n {
        0 => "No unused attachments".to_string(),
        1 => format!("1 unused file ({})", size(found.total)),
        _ => format!("{n} unused files ({})", size(found.total)),
    };
    if n > limit {
        out.push_str(&format!(", first {limit} listed"));
    }
    match found.unread {
        0 => {}
        1 => out.push_str(" · 1 note could not be read; files it uses may be listed"),
        u => out.push_str(&format!(
            " · {u} notes could not be read; files they use may be listed"
        )),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const ROOT: &str = "/n";

    fn attachment(path: &str, size: u64) -> Attachment {
        Attachment {
            path: Path::new(ROOT).join(path),
            size,
        }
    }

    /// The paths (relative to the root) of `files` no note in `notes` (path → text) uses.
    fn unused_of(files: &[&str], notes: &[(&str, &str)]) -> Vec<String> {
        let texts: HashMap<PathBuf, String> = notes
            .iter()
            .map(|(p, t)| (Path::new(ROOT).join(p), t.to_string()))
            .collect();
        let paths: Vec<PathBuf> = notes.iter().map(|(p, _)| Path::new(ROOT).join(p)).collect();
        let files = files.iter().map(|f| attachment(f, 1)).collect();
        let found = unused(files, &paths, &|p| texts.get(p).cloned(), &|| false).unwrap();
        let rel = |a: &Attachment| a.path.strip_prefix(ROOT).unwrap().display().to_string();
        found.files.iter().map(rel).collect()
    }

    #[test]
    fn referenced_image_is_kept_out() {
        let notes = [("Home.md", "![a](assets/a.png)")];
        assert_eq!(
            unused_of(&["assets/a.png", "assets/b.png"], &notes),
            ["assets/b.png"]
        );
    }

    #[test]
    fn embedded_image_is_kept_out() {
        let notes = [(
            "Notes/Home.md",
            "![[a.png]] ![[Sub/B.PNG|300]] [[c.png]] `![[d.png]]`\n```\n![[e.png]]\n```\n",
        )];
        let files = [
            "assets/a.png",
            "other/assets/a.png",
            "assets/Sub/b.png",
            "assets/b.png",
            "assets/c.png",
            "assets/d.png",
            "assets/e.png",
        ];
        assert_eq!(
            unused_of(&files, &notes),
            [
                "assets/b.png",
                "assets/c.png",
                "assets/d.png",
                "assets/e.png"
            ]
        );
    }

    #[test]
    fn unreferenced_is_listed() {
        let notes = [("Home.md", "No links here."), ("Other.md", "[x](Home.md)")];
        assert_eq!(unused_of(&["assets/a.png"], &notes), ["assets/a.png"]);
    }

    #[test]
    fn links_of_every_kind_count() {
        let text = "[a pdf](assets/doc.pdf \"Title\")\n\
                    [raw](<assets/my file.png>)\n\
                    ![ref][logo]\n\
                    \n\
                    [logo]: assets/logo.svg\n\
                    [other]: <assets/with space.gif> \"t\"\n\
                    [^1]: assets/foot.png\n\
                    ![q](assets/q.png?v=2#top)\n";
        let files = [
            "assets/doc.pdf",
            "assets/my file.png",
            "assets/logo.svg",
            "assets/with space.gif",
            "assets/foot.png",
            "assets/q.png",
            "assets/z.png",
        ];
        assert_eq!(
            unused_of(&files, &[("Home.md", text)]),
            ["assets/foot.png", "assets/z.png"]
        );
    }

    #[test]
    fn percent_encoded_name_resolves() {
        let text = "![](assets/my%20photo.png) ![](assets/caf%C3%A9.png) ![](assets/100%.png) \
                    ![](assets/50%25.png)";
        let files = [
            "assets/my photo.png",
            "assets/café.png",
            "assets/100%.png",
            "assets/50%25.png",
        ];
        assert!(unused_of(&files, &[("Home.md", text)]).is_empty());
    }

    #[test]
    fn case_and_normalization_do_not_matter() {
        // "é" precomposed in the note, decomposed on disk.
        let notes = [("Home.md", "![](Assets/caf\u{e9}.png)")];
        assert!(unused_of(&["assets/Cafe\u{301}.PNG"], &notes).is_empty());
    }

    #[test]
    fn image_tag_counts() {
        let text = "<img src=\"assets/a.png\" width=\"200\">\n\
                    <IMG alt=\"b\" SRC='./assets/b.png'>\n\
                    <p><img src=assets/c.png></p>\n\
                    <img src=\"https://example.com/assets/d.png\">\n";
        let files = [
            "assets/a.png",
            "assets/b.png",
            "assets/c.png",
            "assets/d.png",
        ];
        assert_eq!(unused_of(&files, &[("Home.md", text)]), ["assets/d.png"]);
    }

    #[test]
    fn references_in_code_do_not_count() {
        let text = "```\n![](assets/fenced.png)\n```\n\
                    `![](assets/span.png)` and `<img src=\"assets/tag.png\">`\n\
                    ~~~html\n<img src=\"assets/tilde.png\">\n~~~\n";
        let files = [
            "assets/fenced.png",
            "assets/span.png",
            "assets/tag.png",
            "assets/tilde.png",
        ];
        assert_eq!(unused_of(&files, &[("Home.md", text)]), files);
    }

    #[test]
    fn front_matter_does_not_count() {
        let notes = [("Home.md", "---\ncover: ![](assets/cover.png)\n---\nBody")];
        assert_eq!(
            unused_of(&["assets/cover.png"], &notes),
            ["assets/cover.png"]
        );
    }

    #[test]
    fn web_absolute_and_wiki_links_do_not_count() {
        let text = "![](https://example.com/assets/a.png) ![](/n/assets/b.png) [[assets/c.png]] \
                    ![](~/assets/d.png) [e](#assets/e.png)";
        let files = [
            "assets/a.png",
            "assets/b.png",
            "assets/c.png",
            "assets/d.png",
            "assets/e.png",
        ];
        assert_eq!(unused_of(&files, &[("Home.md", text)]), files);
    }

    #[test]
    fn paths_resolve_relative_to_the_note() {
        let notes = [(
            "Projects/Plan.md",
            "![](assets/chart.png) ![](../assets/logo.png) ![](./assets/logo.png)",
        )];
        let files = [
            "Projects/assets/chart.png",
            "assets/logo.png",
            "assets/chart.png",
        ];
        assert_eq!(unused_of(&files, &notes), ["assets/chart.png"]);
    }

    #[test]
    fn unread_note_is_counted() {
        let notes = [Path::new(ROOT).join("Gone.md")];
        let found = unused(
            vec![attachment("assets/a.png", 2_000_000)],
            &notes,
            &|_| None,
            &|| false,
        )
        .unwrap();
        assert_eq!(found.unread, 1);
        assert_eq!(found.files.len(), 1);
        assert_eq!(found.total, 2_000_000);
    }

    #[test]
    fn summary_counts_files_size_and_unread_notes() {
        let size = |b: u64| format!("{b} B");
        let mut found = Found::default();
        assert_eq!(summary(&found, 500, &size), "No unused attachments");
        found.files.push(attachment("assets/a.png", 1500));
        found.total = 1500;
        assert_eq!(summary(&found, 500, &size), "1 unused file (1500 B)");
        found.files.push(attachment("assets/b.png", 0));
        assert_eq!(summary(&found, 500, &size), "2 unused files (1500 B)");
        assert_eq!(
            summary(&found, 1, &size),
            "2 unused files (1500 B), first 1 listed"
        );
        found.unread = 2;
        assert_eq!(
            summary(&found, 500, &size),
            "2 unused files (1500 B) · 2 notes could not be read; files they use may be listed"
        );
    }

    /// Fresh empty folder under the system temp dir, removed on drop (also when a test fails).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mdmate-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn files(&self, paths: &[&str]) {
            for rel in paths {
                let p = self.0.join(rel);
                fs::create_dir_all(p.parent().unwrap()).unwrap();
                fs::write(p, "12345").unwrap();
            }
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn relative(found: &[Attachment], root: &Path) -> Vec<String> {
        let rel = |a: &Attachment| a.path.strip_prefix(root).unwrap().display().to_string();
        found.iter().map(rel).collect()
    }

    #[test]
    fn attachments_on_disk() {
        let t = TempDir::new("attachments");
        t.files(&[
            // In assets folders, nested too.
            "assets/a.png",
            "assets/2024/b.pdf",
            "Projects/Deep/assets/c.jpg",
            // Outside assets: ignored.
            "photo.png",
            "Projects/d.png",
            "assetsx/e.png",
            "my assets/f.png",
            // Hidden: ignored.
            "assets/.hidden.png",
            ".config/assets/g.png",
            "assets/.cache/h.png",
            // Notes and node_modules: ignored.
            "assets/note.md",
            "assets/NOTE.MD",
            "node_modules/assets/i.png",
        ]);
        std::os::unix::fs::symlink(t.0.join("photo.png"), t.0.join("assets/link.png")).unwrap();
        std::os::unix::fs::symlink(t.0.join("Projects"), t.0.join("assets/linked")).unwrap();

        let found = attachments(&t.0, &t.0, &|| false).unwrap();
        assert_eq!(
            relative(&found, &t.0),
            [
                "assets/2024/b.pdf",
                "assets/a.png",
                "Projects/Deep/assets/c.jpg"
            ]
        );
        assert!(found.iter().all(|a| a.size == 5));

        // A Find in “Name”… folder: only the files under it, paths still from the open folder.
        let scoped = attachments(&t.0.join("Projects"), &t.0, &|| false).unwrap();
        assert_eq!(relative(&scoped, &t.0), ["Projects/Deep/assets/c.jpg"]);
        let inside = attachments(&t.0.join("assets/2024"), &t.0, &|| false).unwrap();
        assert_eq!(relative(&inside, &t.0), ["assets/2024/b.pdf"]);
        assert_eq!(attachments(&t.0, &t.0, &|| true), None);
    }

    #[test]
    fn folder_scan_end_to_end() {
        let t = TempDir::new("attachments-scan");
        t.files(&[
            "assets/used.png",
            "assets/unused.png",
            "Notes/assets/nested.png",
        ]);
        let (home, note) = (t.0.join("Home.md"), t.0.join("Notes/Note.md"));
        fs::write(&home, "Nothing yet\n").unwrap();
        fs::write(&note, "Nothing\n").unwrap();
        // The open note's editor text counts, not the file on disk.
        let open = (home.clone(), "![](assets/used.png)\n".to_string());
        let found = find(&t.0, &t.0, &[home, note], Some(&open), &|| false).unwrap();
        assert_eq!(
            relative(&found.files, &t.0),
            ["assets/unused.png", "Notes/assets/nested.png"]
        );
        assert_eq!((found.total, found.unread), (10, 0));
    }
}
