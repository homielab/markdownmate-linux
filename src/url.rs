//! `markdownmate://` links (README "URL Scheme"): parsing and the safety checks. Any web page
//! can open such a link, so every parameter is hostile until checked here.

use std::path::{Component, Path, PathBuf};

use crate::{naming, notes, render};

/// Largest `text` accepted (bytes).
pub const MAX_TEXT: usize = 1 << 20;

#[derive(Debug, PartialEq)]
pub enum Action {
    Open {
        path: String,
    },
    /// `name` is a checked file name ending in `.md`/`.markdown`.
    Create {
        name: String,
        text: String,
    },
    Append {
        path: String,
        text: String,
    },
}

/// Parses `markdownmate://<action>?<params>`; parameters are percent-encoded, `+` is a space.
pub fn parse(uri: &str) -> Result<Action, &'static str> {
    let bad = "Invalid MarkdownMate link";
    let rest = uri
        .get(..15)
        .filter(|s| s.eq_ignore_ascii_case("markdownmate://"))
        .map(|_| &uri[15..])
        .ok_or(bad)?;
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut params = Vec::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        params.push((decode(k).ok_or(bad)?, decode(v).ok_or(bad)?));
    }
    let get = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let text = get("text").unwrap_or_default();
    if text.len() > MAX_TEXT || text.contains('\0') {
        return Err("The link's text is too long or invalid");
    }
    let path = || {
        get("path")
            .filter(|p| {
                !p.is_empty()
                    && !p.chars().any(char::is_control)
                    && !Path::new(p).components().any(|c| c == Component::ParentDir)
            })
            .ok_or("The link's path is missing or invalid")
    };
    match action.trim_end_matches('/') {
        "open" => Ok(Action::Open { path: path()? }),
        "append" if text.is_empty() => Err("The link has no text to append"),
        "append" => Ok(Action::Append {
            path: path()?,
            text,
        }),
        "create" => {
            let title = get("title").unwrap_or_default();
            let name = if title.trim().is_empty() {
                crate::UNTITLED.to_string()
            } else if title.chars().any(char::is_control) {
                return Err("The link's title is invalid");
            } else {
                naming::note_name(&title)?
            };
            Ok(Action::Create { name, text })
        }
        _ => Err(bad),
    }
}

/// Percent-decodes `s` (`+` is a space); None on a bad escape or invalid UTF-8.
fn decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        out.push(match b {
            b'+' => b' ',
            b'%' => {
                let hex = [bytes.next()?, bytes.next()?];
                u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?
            }
            b => b,
        });
    }
    String::from_utf8(out).ok()
}

/// The note a link's `path` names (relative to `root`, the open folder, or absolute): an
/// existing Markdown file inside `root` or a bookmarked folder once symlinks are resolved.
/// Returns that folder and the note.
pub fn resolve(
    path: &str,
    root: Option<&Path>,
    bookmarks: &[PathBuf],
) -> Result<(PathBuf, PathBuf), &'static str> {
    let p = Path::new(path);
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        root.ok_or("Open a folder first")?.join(p)
    };
    for dir in root
        .into_iter()
        .chain(bookmarks.iter().map(PathBuf::as_path))
    {
        if let Some(note) = render::note_in_folder(&full, dir) {
            return if notes::is_note(&note) {
                Ok((dir.to_path_buf(), note))
            } else {
                Err("Links can only open Markdown notes")
            };
        }
    }
    Err("Note not found in the open or a bookmarked folder")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn actions() {
        assert_eq!(
            parse("markdownmate://open?path=Projects/Plan.md"),
            Ok(Action::Open {
                path: "Projects/Plan.md".into()
            })
        );
        assert_eq!(
            parse("MarkdownMate://create?title=Idea+one&text=Call%20Anna%0Anow"),
            Ok(Action::Create {
                name: "Idea one.md".into(),
                text: "Call Anna\nnow".into()
            })
        );
        assert_eq!(
            parse("markdownmate://create/?text=Hi"),
            Ok(Action::Create {
                name: crate::UNTITLED.into(),
                text: "Hi".into()
            })
        );
        assert_eq!(
            parse("markdownmate://append?path=Inbox.md&text=Buy+milk+%2B+%C4%91%C6%B0%E1%BB%9Dng"),
            Ok(Action::Append {
                path: "Inbox.md".into(),
                text: "Buy milk + đường".into()
            })
        );
    }

    #[test]
    fn rejects_bad_links() {
        for uri in [
            "markdownmate://delete?path=a.md",
            "file:///a.md",
            "markdownmate://open",
            "markdownmate://open?path=",
            "markdownmate://open?path=../x.md",
            "markdownmate://open?path=a/../../x.md",
            "markdownmate://open?path=a%00.md",
            "markdownmate://open?path=a%0A.md",
            "markdownmate://open?path=%ZZ",
            "markdownmate://open?path=%FF.md",
            "markdownmate://append?path=a.md",
            "markdownmate://append?path=a.md&text=a%00b",
            "markdownmate://create?title=a/b",
            "markdownmate://create?title=..",
            "markdownmate://create?title=a%0Ab",
            "markdown",
        ] {
            assert!(parse(uri).is_err(), "{uri}");
        }
        let long = format!("markdownmate://create?text={}", "a".repeat(MAX_TEXT + 1));
        assert!(parse(&long).is_err());
    }

    #[test]
    fn resolve_stays_inside_allowed_folders() {
        let tmp = std::env::temp_dir().join(format!("mm-url-{}", std::process::id()));
        let root = tmp.join("root");
        let marked = tmp.join("marked");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(&marked).unwrap();
        fs::write(root.join("sub/a.md"), "").unwrap();
        fs::write(root.join("b.txt"), "").unwrap();
        fs::write(marked.join("m.markdown"), "").unwrap();
        fs::write(tmp.join("out.md"), "").unwrap();
        std::os::unix::fs::symlink(tmp.join("out.md"), root.join("link.md")).unwrap();
        std::os::unix::fs::symlink(root.join("b.txt"), root.join("txt.md")).unwrap();
        let bm = [marked.clone()];
        let r = Some(root.as_path());

        assert_eq!(
            resolve("sub/a.md", r, &bm),
            Ok((root.clone(), root.join("sub/a.md")))
        );
        let abs = marked.join("m.markdown");
        assert_eq!(
            resolve(abs.to_str().unwrap(), r, &bm),
            Ok((marked.clone(), abs.clone()))
        );
        assert!(resolve(abs.to_str().unwrap(), r, &[]).is_err());
        assert!(resolve("sub/a.md", None, &bm).is_err());
        // Not existing (append never creates), not Markdown, symlinks out or to a non-note.
        for p in ["new.md", "b.txt", "link.md", "txt.md", "sub"] {
            assert!(resolve(p, r, &bm).is_err(), "{p}");
        }
        assert!(resolve(tmp.join("out.md").to_str().unwrap(), r, &bm).is_err());
        // create never overwrites: a taken name is numbered.
        let Ok(Action::Create { name, .. }) = parse("markdownmate://create?title=a") else {
            panic!()
        };
        fs::write(root.join("a.md"), "keep").unwrap();
        assert_eq!(
            naming::create_note(&root, &name).unwrap(),
            root.join("a 2.md")
        );
        assert_eq!(fs::read_to_string(root.join("a.md")).unwrap(), "keep");
        fs::remove_dir_all(&tmp).unwrap();
    }
}
