//! Create Notes Folder… (the empty window's button, main menu ▸ New Notes Folder…): a new
//! folder with "Welcome to MarkdownMate", "Getting Around" and the starter templates, so a new
//! user has notes to read and links, backlinks, tags and templates to try. The notes' text is
//! bundled (`resources/welcome/`) and written byte for byte; the macOS app ships the same files.
//! Pure Rust (no GTK) so it is testable headless.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::templates;

/// The name the save dialog suggests.
pub const SUGGESTED_NAME: &str = "Notes";

/// The note opened once the folder is made.
pub const WELCOME_NOTE: &str = "Welcome to MarkdownMate.md";

/// The notes written at the top of the folder, in this order.
pub const NOTES: [(&str, &str); 2] = [
    (
        WELCOME_NOTE,
        include_str!("../resources/welcome/Welcome to MarkdownMate.md"),
    ),
    (
        "Getting Around.md",
        include_str!("../resources/welcome/Getting Around.md"),
    ),
];

/// The files a new notes folder holds, relative to it: the notes, then the starter templates
/// in the templates folder (`templates_setting`, resolved as Preferences ▸ Templates Folder is).
/// What `create` writes; the tests check it against the disk.
#[cfg(test)]
pub fn files(templates_setting: &str) -> Vec<PathBuf> {
    let templates = crate::daily::folder(Path::new(""), templates_setting);
    NOTES
        .iter()
        .map(|(name, _)| PathBuf::from(name))
        .chain(
            templates::STARTER_TEMPLATES
                .iter()
                .map(|(name, _)| templates.join(name)),
        )
        .collect()
}

/// Whether a notes folder can be made at `path`: nothing is there yet, or an empty folder
/// (hidden files such as `.directory` don't count). A file, a folder holding anything else, or
/// one that cannot be read is refused, so no one's files get mixed with the new notes.
pub fn can_create(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return true;
    };
    if !meta.is_dir() {
        return false;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries
        .flatten()
        .all(|e| e.file_name().to_string_lossy().starts_with('.'))
}

/// Makes `dir` (with any missing parents) and writes the notes and the starter templates into
/// it (templates folder from `templates_setting`), never overwriting a file. Returns the
/// welcome note.
pub fn create(dir: &Path, templates_setting: &str) -> std::io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    for (name, text) in NOTES {
        fs::File::create_new(dir.join(name))?.write_all(text.as_bytes())?;
    }
    templates::create_starters(&crate::daily::folder(dir, templates_setting))?;
    Ok(dir.join(WELCOME_NOTE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aliases::Aliases;
    use crate::backlinks;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(tag: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!("mm-welcome-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    #[test]
    fn the_file_set_is_the_two_notes_and_the_starter_templates() {
        let names: Vec<String> = files(templates::DEFAULT_FOLDER)
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            [
                "Welcome to MarkdownMate.md",
                "Getting Around.md",
                "Templates/Meeting Notes.md",
                "Templates/Project.md",
                "Templates/Book Notes.md",
                "Templates/Weekly Review.md",
            ]
        );
        assert_eq!(
            files("Notes/ Tpl /")[2],
            Path::new("Notes/Tpl/Meeting Notes.md")
        );
        assert_eq!(files("")[2], Path::new("Meeting Notes.md"));
    }

    #[test]
    fn only_a_new_or_empty_folder_is_accepted() {
        let t = temp_dir("rule");
        assert!(can_create(&t.0.join("missing")));
        assert!(can_create(&t.0.join("missing/deeper")));
        assert!(can_create(&t.0), "an empty folder");
        fs::write(t.0.join(".directory"), "").unwrap();
        assert!(can_create(&t.0), "hidden files don't count");
        fs::write(t.0.join("note.md"), "").unwrap();
        assert!(!can_create(&t.0), "a folder with a note");
        assert!(!can_create(&t.0.join("note.md")), "a file");
        let sub = t.0.join("sub");
        fs::create_dir(&sub).unwrap();
        fs::create_dir(sub.join("child")).unwrap();
        assert!(!can_create(&sub), "a folder with a folder");
    }

    #[test]
    fn writes_each_file_as_its_resource() {
        let t = temp_dir("create");
        let dir = t.0.join("Notes");
        let welcome = create(&dir, templates::DEFAULT_FOLDER).unwrap();
        assert_eq!(welcome, dir.join("Welcome to MarkdownMate.md"));
        let expected = NOTES.iter().chain(templates::STARTER_TEMPLATES.iter());
        for (rel, (_, text)) in files(templates::DEFAULT_FOLDER).iter().zip(expected) {
            assert_eq!(fs::read(dir.join(rel)).unwrap(), text.as_bytes(), "{rel:?}");
        }
        let mut top: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        top.sort();
        assert_eq!(
            top,
            [
                "Getting Around.md",
                "Templates",
                "Welcome to MarkdownMate.md"
            ]
        );
    }

    #[test]
    fn never_overwrites_a_note() {
        let t = temp_dir("existing");
        fs::write(t.0.join("Getting Around.md"), "mine\n").unwrap();
        assert!(create(&t.0, templates::DEFAULT_FOLDER).is_err());
        assert_eq!(
            fs::read_to_string(t.0.join("Getting Around.md")).unwrap(),
            "mine\n"
        );
    }

    #[test]
    fn the_two_notes_link_to_each_other_once() {
        let t = temp_dir("links");
        let welcome = create(&t.0, templates::DEFAULT_FOLDER).unwrap();
        let around = t.0.join("Getting Around.md");
        let notes: Vec<PathBuf> = files(templates::DEFAULT_FOLDER)
            .iter()
            .map(|rel| t.0.join(rel))
            .collect();
        for (target, from) in [(&welcome, &around), (&around, &welcome)] {
            let found = backlinks::find(notes.clone(), &Aliases::default(), &t.0, target);
            assert_eq!(found.len(), 1, "backlinks of {target:?}");
            assert_eq!(&found[0].path, from);
        }
    }

    #[test]
    fn the_notes_have_what_they_teach() {
        let (_, welcome) = NOTES[0];
        for part in [
            "**bold**",
            "- [x] ",
            "- [ ] ",
            "#welcome",
            "[[Getting Around]]",
            "> [!tip]",
            "⌘D on a Mac, Ctrl+D on Ubuntu",
        ] {
            assert!(welcome.contains(part), "welcome note lacks {part}");
        }
        assert!(
            NOTES[1]
                .1
                .ends_with("Back to [[Welcome to MarkdownMate]]\n")
        );
    }
}
