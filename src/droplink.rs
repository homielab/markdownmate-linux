//! What a file dropped into the editor becomes (images aside, which `images.rs` copies).

use std::path::{Path, PathBuf};

use crate::{complete, notes, relink};

/// The link for `path` dropped into the note in folder `note_dir`: a note (`.md`/`.markdown`,
/// any case) inside the open folder `root` becomes `[[Name]]`, or `[[Folder/Name]]` when another
/// of `notes` has its name (as Copy Wiki Link); anything else (outside `root`, other files,
/// folders) a relative link (`relink::dropped_links`).
pub fn dropped_link(
    path: &Path,
    is_dir: bool,
    root: Option<&Path>,
    notes: &[PathBuf],
    note_dir: Option<&Path>,
) -> String {
    match root {
        Some(root) if !is_dir && notes::is_note(path) && path.starts_with(root) => {
            format!("[[{}]]", complete::link_name(path, notes, root))
        }
        _ => relink::dropped_links(&[(path.to_path_buf(), is_dir)], note_dir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> Vec<PathBuf> {
        ["/n/A.md", "/n/x/Dup.md", "/n/y/Dup.md", "/n/Up.MD"]
            .iter()
            .map(PathBuf::from)
            .collect()
    }

    fn link(path: &str, is_dir: bool) -> String {
        dropped_link(
            Path::new(path),
            is_dir,
            Some(Path::new("/n")),
            &notes(),
            Some(Path::new("/n")),
        )
    }

    #[test]
    fn note_inside_is_a_wiki_link() {
        assert_eq!(link("/n/A.md", false), "[[A]]");
    }

    #[test]
    fn duplicate_name_gets_its_folder() {
        assert_eq!(link("/n/x/Dup.md", false), "[[x/Dup]]");
    }

    #[test]
    fn upper_case_extension() {
        assert_eq!(link("/n/Up.MD", false), "[[Up]]");
    }

    #[test]
    fn outside_folder_and_other_files_stay_relative() {
        assert_eq!(link("/other/B.md", false), "[B](../other/B.md)");
        assert_eq!(link("/n/doc.pdf", false), "[doc](doc.pdf)");
        assert!(!link("/n/x", true).starts_with("[["));
        let none = dropped_link(Path::new("/n/A.md"), false, None, &notes(), None);
        assert!(!none.starts_with("[["));
    }

    #[test]
    fn several_sidebar_notes_one_wiki_link_per_line() {
        let lines: Vec<String> = ["/n/A.md", "/n/x/Dup.md", "/n/Up.MD"]
            .iter()
            .map(|p| link(p, false))
            .collect();
        assert_eq!(lines.join("\n"), "[[A]]\n[[x/Dup]]\n[[Up]]");
    }

    #[test]
    fn mixed_files_one_per_line() {
        let lines: Vec<String> = [
            ("/n/A.md", false),
            ("/n/doc.pdf", false),
            ("/n/y/Dup.md", false),
        ]
        .iter()
        .map(|(p, d)| link(p, *d))
        .collect();
        assert_eq!(lines.join("\n"), "[[A]]\n[doc](doc.pdf)\n[[y/Dup]]");
    }
}
