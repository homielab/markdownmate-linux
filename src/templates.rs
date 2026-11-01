//! New from Template: the `.md` files of the templates folder (`Prefs::templates_folder`,
//! relative to the open folder like the daily notes folder, `Templates` by default) and the
//! placeholder fill shared with Today's Note (`{{date}}` → YYYY-MM-DD, `{{title}}` → the note's
//! name). Pure Rust (no GTK) so it is testable headless.

use std::fs;
use std::path::{Path, PathBuf};

/// The templates folder a new state file starts with.
pub const DEFAULT_FOLDER: &str = "Templates";

/// The four ready-made starter templates bundled with the app.
pub const STARTER_TEMPLATES: [(&str, &str); 4] = [
    (
        "Meeting Notes.md",
        include_str!("../resources/templates/Meeting Notes.md"),
    ),
    (
        "Project.md",
        include_str!("../resources/templates/Project.md"),
    ),
    (
        "Book Notes.md",
        include_str!("../resources/templates/Book Notes.md"),
    ),
    (
        "Weekly Review.md",
        include_str!("../resources/templates/Weekly Review.md"),
    ),
];

/// Creates `dir` if missing and writes the starter templates that do not already exist.
/// Existing files of the same name are left alone.
pub fn create_starters(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    for (name, content) in STARTER_TEMPLATES {
        let file = dir.join(name);
        if !file.exists() {
            fs::write(file, content)?;
        }
    }
    Ok(())
}

/// One template: its file and the title shown in the menu (the file name without `.md`).
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub title: String,
    pub path: PathBuf,
}

/// The templates in `dir`: its own `.md` files (extension in any case; not `.markdown`,
/// subfolders or hidden files), sorted like the sidebar's names (`notes::compare_names`).
/// A missing or unreadable folder has none.
pub fn list(dir: &Path) -> Vec<Template> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<Template> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let title = name.get(..name.len().checked_sub(3)?)?;
            let is_md = name[title.len()..].eq_ignore_ascii_case(".md");
            if !is_md || title.is_empty() || name.starts_with('.') {
                return None;
            }
            let path = entry.path();
            path.is_file().then(|| Template {
                title: title.to_string(),
                path,
            })
        })
        .collect();
    found.sort_by(|a, b| crate::notes::compare_names(&a.title, &b.title));
    found
}

/// `template` with every `{{date}}` replaced by `date` and every `{{title}}` by `title`
/// (exact case; other `{{…}}` stay as they are).
pub fn fill(template: &str, date: &str, title: &str) -> String {
    template
        .replace("{{date}}", date)
        .replace("{{title}}", title)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(tag: &str) -> TempDir {
        let path = std::env::temp_dir().join(format!("mm-templates-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    fn titles(dir: &Path) -> Vec<String> {
        list(dir).into_iter().map(|t| t.title).collect()
    }

    #[test]
    fn lists_md_files_in_name_order() {
        let t = temp_dir("list");
        for name in [
            "Meeting.md",
            "note 10.md",
            "note 2.md",
            "École.MD",
            "apple.md",
            "Draft.markdown",
            "readme.txt",
            ".hidden.md",
            ".md",
        ] {
            fs::write(t.0.join(name), "x").unwrap();
        }
        fs::create_dir_all(t.0.join("Folder.md")).unwrap();
        fs::create_dir_all(t.0.join("Sub")).unwrap();
        fs::write(t.0.join("Sub/Nested.md"), "x").unwrap();
        assert_eq!(
            titles(&t.0),
            ["apple", "École", "Meeting", "note 2", "note 10"]
        );
        let meeting = list(&t.0).into_iter().find(|t| t.title == "Meeting");
        assert_eq!(meeting.unwrap().path, t.0.join("Meeting.md"));
    }

    #[test]
    fn missing_or_empty_folder_has_none() {
        let t = temp_dir("empty");
        assert!(list(&t.0).is_empty());
        assert!(list(&t.0.join("missing")).is_empty());
        fs::write(t.0.join("file.md"), "x").unwrap();
        assert!(list(&t.0.join("file.md")).is_empty());
    }

    #[test]
    fn templates_folder_follows_the_daily_folder_rule() {
        let root = Path::new("/notes");
        let folder = |s| crate::daily::folder(root, s);
        assert_eq!(folder(DEFAULT_FOLDER), Path::new("/notes/Templates"));
        assert_eq!(folder(" ../Shared/ ./Tpl "), Path::new("/notes/Shared/Tpl"));
        assert_eq!(folder(""), root);
    }

    #[test]
    fn fills_date_and_title() {
        assert_eq!(
            fill(
                "# {{title}}\n\nDate: {{date}} ({{date}})\n{{Title}} {{other}}\n",
                "2026-11-01",
                "Untitled 2"
            ),
            "# Untitled 2\n\nDate: 2026-11-01 (2026-11-01)\n{{Title}} {{other}}\n"
        );
        assert_eq!(
            fill("No placeholders\n", "2026-11-01", "x"),
            "No placeholders\n"
        );
        assert_eq!(fill("", "2026-11-01", "x"), "");
    }

    #[test]
    fn creates_starter_templates_and_leaves_existing_untouched() {
        let t = temp_dir("starters");
        let target = t.0.join("Templates");
        fs::create_dir_all(&target).unwrap();
        let existing = target.join("Meeting Notes.md");
        fs::write(&existing, "custom notes\n").unwrap();

        create_starters(&target).unwrap();

        assert_eq!(fs::read_to_string(&existing).unwrap(), "custom notes\n");
        for (name, content) in STARTER_TEMPLATES {
            let path = target.join(name);
            assert!(path.is_file(), "missing {name}");
            if name != "Meeting Notes.md" {
                assert_eq!(
                    fs::read_to_string(&path).unwrap(),
                    content,
                    "content mismatch in {name}"
                );
            }
        }
        assert_eq!(
            titles(&target),
            ["Book Notes", "Meeting Notes", "Project", "Weekly Review"]
        );
    }

    #[test]
    fn creates_starter_templates_when_folder_missing() {
        let t = temp_dir("missing-folder");
        let target = t.0.join("Templates");
        assert!(!target.exists());

        create_starters(&target).unwrap();
        assert!(target.is_dir());

        for (name, content) in STARTER_TEMPLATES {
            let path = target.join(name);
            assert!(path.is_file());
            assert_eq!(fs::read_to_string(&path).unwrap(), content);
        }
        assert_eq!(
            titles(&target),
            ["Book Notes", "Meeting Notes", "Project", "Weekly Review"]
        );
    }

    #[test]
    fn starter_template_resources_match_contract() {
        for (name, content) in STARTER_TEMPLATES {
            assert!(
                content.starts_with("# {{title}}\n\n"),
                "{name} should start with title heading"
            );
            assert!(
                content.contains("{{date}}"),
                "{name} should contain date placeholder"
            );
            assert!(content.ends_with('\n'), "{name} should end with a newline");
            assert!(
                !content.contains('\r'),
                "{name} should not contain carriage returns"
            );
        }
    }
}
