//! Front-matter aliases: other names a note goes by, so `[[Alias]]` links reach it. Pure
//! logic, no GTK.
//!
//! The YAML front matter's top-level `aliases:` (or `alias:`) key, in the forms front-matter
//! tags take except splitting at spaces: a flow list `[Meeting notes, MN]`, a block list (`- MN`
//! lines) or a string split at commas; quotes dropped. A `[[link]]` that names no note
//! resolves to the note with that alias (any case, NFC or NFD; `#heading` and `|text` as for
//! names): `render::resolve_wiki_with` and `render::WikiIndex::with_aliases`.

use std::collections::HashMap;
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::{lists, render, tags};

const KEYS: &[&str] = &["aliases:", "alias:"];

/// Front matter longer than this (in lines) is not read for aliases.
const MAX_LINES: usize = 1000;

/// The aliases of the note `text`, as written (unquoted), each once, in order.
pub fn in_text(text: &str) -> Vec<String> {
    let lines = lists::front_matter_lines(text);
    let rows: Vec<&str> = text.lines().take(lines).collect();
    let mut out: Vec<String> = Vec::new();
    for (row, range) in tags::front_matter_list(text, lines, KEYS, false) {
        let alias = rows[row][range].to_string();
        if key(&alias).is_some() && !out.contains(&alias) {
            out.push(alias);
        }
    }
    out
}

/// `alias` (or a wiki link's target) as aliases are matched: the target `resolve_wiki` reads
/// (no `#heading`, no outer `/`, no `.md`), lower case, NFC. None when nothing is left.
pub fn key(alias: &str) -> Option<String> {
    render::wiki_key(alias).map(|k| k.nfc().collect())
}

/// The aliases of the note at `path`, reading only its front matter. None for a note
/// without front matter or that can't be read as UTF-8.
fn read(path: &Path) -> Vec<String> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut reader = BufReader::new(file);
    let mut text = String::new();
    for n in 0..MAX_LINES {
        let at = text.len();
        match reader.read_line(&mut text) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let line = text[at..].trim_end();
        if n == 0 && line != "---" {
            return Vec::new();
        }
        if n > 0 && line == "---" {
            return in_text(&text);
        }
    }
    Vec::new()
}

/// The aliases of the notes of a folder, by note and by key.
#[derive(Debug, Clone, Default)]
pub struct Aliases {
    by_note: HashMap<PathBuf, Vec<String>>,
    by_key: HashMap<String, Vec<PathBuf>>,
    fingerprint: u64,
}

impl Aliases {
    /// From each note's aliases (as `in_text` gives them).
    pub fn new(mut by_note: HashMap<PathBuf, Vec<String>>) -> Self {
        by_note.retain(|_, aliases| !aliases.is_empty());
        let mut sorted: Vec<(&PathBuf, &Vec<String>)> = by_note.iter().collect();
        sorted.sort();
        let mut hasher = DefaultHasher::new();
        sorted.hash(&mut hasher);
        let mut by_key: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for (note, aliases) in sorted {
            for k in aliases.iter().filter_map(|a| key(a)) {
                let notes = by_key.entry(k).or_default();
                if !notes.contains(note) {
                    notes.push(note.clone());
                }
            }
        }
        Aliases {
            by_note,
            by_key,
            fingerprint: hasher.finish(),
        }
    }

    /// The aliases of `note`, as written.
    pub fn of(&self, note: &Path) -> &[String] {
        self.by_note.get(note).map_or(&[], Vec::as_slice)
    }

    /// The notes with the alias a wiki link `target` names (`key`), in path order.
    pub fn notes(&self, target: &str) -> &[PathBuf] {
        key(target)
            .and_then(|k| self.by_key.get(&k))
            .map_or(&[], Vec::as_slice)
    }

    /// Every (note, alias as written), in path then alias order.
    pub fn all(&self) -> Vec<(&PathBuf, &String)> {
        let mut out: Vec<(&PathBuf, &String)> = self
            .by_note
            .iter()
            .flat_map(|(note, aliases)| aliases.iter().map(move |a| (note, a)))
            .collect();
        out.sort();
        out
    }

    /// Changes whenever any note's aliases do (to tell whether resolved links are stale).
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

impl PartialEq for Aliases {
    fn eq(&self, other: &Self) -> bool {
        self.by_note == other.by_note
    }
}

/// The aliases of `notes` (front matter only; unreadable notes have none).
pub fn index(notes: &[PathBuf]) -> Aliases {
    Aliases::new(
        notes
            .iter()
            .map(|n| (n.clone(), read(n)))
            .filter(|(_, a)| !a.is_empty())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_list() {
        assert_eq!(
            in_text("---\naliases: [Meeting notes, \"MN\", 'Réunion']\n---\nBody"),
            ["Meeting notes", "MN", "Réunion"]
        );
        // Over several lines up to `]`; unclosed in the front matter: none.
        assert_eq!(
            in_text("---\naliases: [A b,\n  C] # x\n---\n"),
            ["A b", "C"]
        );
        assert!(in_text("---\naliases: [A, B\n---\n").is_empty());
    }

    #[test]
    fn block_list() {
        let md = "---\ntitle: X\naliases:\n  - MN\n\n  # comment\n  - \"Meeting notes\" # cut\n- Third\nother: 1\n  - not\n---\n";
        assert_eq!(in_text(md), ["MN", "Meeting notes", "Third"]);
    }

    #[test]
    fn string_split_only_at_commas() {
        assert_eq!(
            in_text("---\nalias: Meeting notes, MN\n---\n"),
            ["Meeting notes", "MN"]
        );
        assert_eq!(
            in_text("---\nalias: \"Meeting notes\"\n---\n"),
            ["Meeting notes"]
        );
        assert_eq!(in_text("---\naliases: C# basics\n---\n"), ["C# basics"]);
        assert_eq!(in_text("---\naliases: One # comment\n---\n"), ["One"]);
    }

    #[test]
    fn only_top_level_keys_in_front_matter() {
        assert!(in_text("aliases: [A]\n").is_empty());
        assert!(in_text("# T\n---\naliases: [A]\n---\n").is_empty());
        assert!(in_text("---\n  aliases: [A]\nAliases: [B]\naliasesx: [C]\n---\n").is_empty());
        assert!(in_text("---\naliases: []\naliases: \"\"\n---\n").is_empty());
        // Each alias once; CRLF.
        assert_eq!(in_text("---\r\naliases: [A, A, B]\r\n---\r\n"), ["A", "B"]);
    }

    #[test]
    fn keys_ignore_case_heading_and_normal_form() {
        assert_eq!(key("Meeting Notes"), Some("meeting notes".into()));
        assert_eq!(key("MN#Agenda"), key("mn"));
        assert_eq!(key("Cafe\u{301}"), key("Café"));
        assert_eq!(key("#x"), None);
    }

    #[test]
    fn index_reads_front_matter_only() {
        let dir = std::env::temp_dir().join(format!("mm-aliases-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        struct Clean(PathBuf);
        impl Drop for Clean {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _clean = Clean(dir.clone());
        let a = dir.join("A.md");
        let b = dir.join("B.md");
        let c = dir.join("C.md");
        std::fs::write(&a, "---\naliases: [MN, Meeting notes]\n---\n# A\n").unwrap();
        std::fs::write(&b, "# B\n---\naliases: [X]\n---\n").unwrap();
        std::fs::write(&c, "---\nalias: mn\n---\n").unwrap();
        let index = index(&[a.clone(), b.clone(), c.clone(), dir.join("gone.md")]);
        assert_eq!(index.of(&a), ["MN", "Meeting notes"]);
        assert!(index.of(&b).is_empty());
        assert_eq!(index.notes("Mn#Agenda"), [a.clone(), c.clone()]);
        assert_eq!(index.notes("meeting NOTES"), std::slice::from_ref(&a));
        assert!(index.notes("x").is_empty());
        assert_eq!(index.all().len(), 3);
        let again = super::index(&[a, b, c]);
        assert!(again == index && again.fingerprint() == index.fingerprint());
        assert_ne!(Aliases::default().fingerprint(), index.fingerprint());
    }
}
