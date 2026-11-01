//! Pasted / dropped images: stored in the attachments folder (`Prefs::attachments_folder`,
//! default `assets/` next to the note; `/name` = at the open folder's root), linked relative
//! to the note.
//! Pure Rust (no GTK) so it is testable headless.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::{naming, render};

/// Default attachments folder, next to the note; always read as an attachments folder.
pub const ASSETS: &str = "assets";

/// Where a note's images go: the folder and the link prefix from the note's folder to it
/// (`assets/`, `../attachments/`, …).
#[derive(Debug, Clone, PartialEq)]
pub struct Dest {
    pub dir: PathBuf,
    pub prefix: String,
}

/// The attachments folder setting cleaned like the templates folder (`daily::folder` rule):
/// blank, `.` and `..` parts dropped, a leading `/` kept; nothing left = `assets`.
pub fn setting(raw: &str) -> String {
    let parts: Vec<&str> = raw
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .collect();
    if parts.is_empty() {
        return ASSETS.to_string();
    }
    let root = if raw.trim_start().starts_with('/') {
        "/"
    } else {
        ""
    };
    format!("{root}{}", parts.join("/"))
}

/// The attachments folder for `note`: `<note's folder>/<name>`, or `<root>/<name>` for `/name`
/// when a folder is open and the note is inside it (else next to the note).
pub fn dest(note: &Path, root: Option<&Path>, raw: &str) -> Dest {
    let name = setting(raw);
    let note_dir = note.parent().unwrap_or(Path::new("."));
    if let Some(rel) = name.strip_prefix('/')
        && let Some((root, up)) = root.and_then(|r| {
            note_dir
                .strip_prefix(r)
                .ok()
                .map(|d| (r, d.components().count()))
        })
    {
        return Dest {
            dir: root.join(rel),
            prefix: format!("{}{rel}/", "../".repeat(up)),
        };
    }
    let rel = name.trim_start_matches('/');
    Dest {
        dir: note_dir.join(rel),
        prefix: format!("{rel}/"),
    }
}

/// Whether a folder named `name` holds attachments: `assets` or the last part of the setting,
/// any case.
pub fn is_attachments_name(name: &std::ffi::OsStr, raw: &str) -> bool {
    let name = name.to_string_lossy();
    let configured = setting(raw);
    let last = configured.rsplit('/').next().unwrap_or(ASSETS);
    name.eq_ignore_ascii_case(ASSETS) || name.eq_ignore_ascii_case(last)
}

/// Whether `path` has an image extension the preview can show.
pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["png", "jpg", "jpeg", "gif", "webp", "svg"]
            .iter()
            .any(|x| e.eq_ignore_ascii_case(x))
    })
}

/// `path` as an image file inside `root` to open, or None. The image check is on the resolved
/// path, so a symlink `x.png` → `run.sh` is refused.
pub fn in_folder(path: &Path, root: &Path) -> Option<PathBuf> {
    render::note_in_folder(path, root).filter(|p| is_image(p))
}

/// `![](<prefix><name>)` (`Dest::prefix`), with the characters that would break the link or URL %-encoded
/// (space, `%`, brackets, `#`, `?`, control characters); other characters, accents included,
/// stay readable.
pub fn link(prefix: &str, name: &str) -> String {
    let mut out = String::from("![](");
    for c in prefix.chars().chain(name.chars()) {
        if c.is_control() || " %()<>[]#?\\".contains(c) {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out.push(')');
    out
}

/// Stores an image in `dest` as `<name>` (numbered `<stem> 2.<ext>`, … if taken,
/// never overwriting: the file is created with create_new first, then `fill`ed). `fill` gets
/// the new, empty file's path. Returns the Markdown link. On error no new file is left behind.
pub fn store(
    dest: &Dest,
    name: &str,
    fill: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<String> {
    let dir = &dest.dir;
    fs::create_dir_all(dir)?;
    let path = naming::create_note(dir, name)?;
    if let Err(e) = fill(&path) {
        let _ = fs::remove_file(&path);
        return Err(e);
    }
    Ok(link(&dest.prefix, &file_name(&path)))
}

/// Link for a dropped image file: copied into `dest` (keeping its name, numbered
/// if taken), or just linked if it already is in that folder.
pub fn store_file(dest: &Dest, src: &Path) -> io::Result<String> {
    let src_dir = src.parent().and_then(|d| d.canonicalize().ok());
    if src_dir.is_some() && src_dir == dest.dir.canonicalize().ok() {
        return Ok(link(&dest.prefix, &file_name(src)));
    }
    store(dest, &file_name(src), |dest| {
        fs::copy(src, dest).map(|_| ())
    })
}

/// `path` (an image, see `is_image`) as a `data:` URL with its MIME type, for Copy as Rich
/// Text: the pasted HTML carries the image itself. None if it cannot be read.
pub fn data_url(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    let bytes = fs::read(path).ok()?;
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

/// Standard base64 with `=` padding.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ABC[((n >> (18 - 6 * i)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mdmate-img-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn in_folder_checks_the_resolved_file() {
        let dir = TempDir::new("infolder");
        let root = &dir.0;
        fs::write(root.join("a.png"), b"x").unwrap();
        fs::write(root.join("run.sh"), b"x").unwrap();
        std::os::unix::fs::symlink(root.join("run.sh"), root.join("x.png")).unwrap();
        std::os::unix::fs::symlink(root.join("a.png"), root.join("b.png")).unwrap();
        assert_eq!(
            in_folder(&root.join("a.png"), root),
            Some(root.join("a.png"))
        );
        assert_eq!(
            in_folder(&root.join("b.png"), root),
            Some(root.join("a.png"))
        );
        assert_eq!(in_folder(&root.join("x.png"), root), None);
        assert_eq!(in_folder(&root.join("run.sh"), root), None);
        assert_eq!(in_folder(&root.join("nope.png"), root), None);
    }

    #[test]
    fn base64_pads() {
        for (raw, b64) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(raw.as_bytes()), b64, "{raw}");
        }
        assert_eq!(base64(&[0xff, 0xfe, 0x00]), "//4A");
    }

    #[test]
    fn data_urls_carry_the_mime_type() {
        let dir = TempDir::new("dataurl");
        fs::write(dir.0.join("a.PNG"), b"foo").unwrap();
        fs::write(dir.0.join("b.svg"), b"<svg/>").unwrap();
        fs::write(dir.0.join("c.txt"), b"foo").unwrap();
        assert_eq!(
            data_url(&dir.0.join("a.PNG")).as_deref(),
            Some("data:image/png;base64,Zm9v")
        );
        assert!(
            data_url(&dir.0.join("b.svg"))
                .unwrap()
                .starts_with("data:image/svg+xml;base64,")
        );
        assert_eq!(data_url(&dir.0.join("c.txt")), None);
        assert_eq!(data_url(&dir.0.join("gone.png")), None);
    }

    #[test]
    fn image_extensions() {
        for ok in [
            "a.png", "a.PNG", "b.jpg", "c.JPEG", "d.gif", "e.webp", "f.svg", "x y.Png",
        ] {
            assert!(is_image(Path::new(ok)), "{ok}");
        }
        for bad in ["a.md", "a.pdf", "png", ".png", "a.png.txt", "a"] {
            assert!(!is_image(Path::new(bad)), "{bad}");
        }
    }

    #[test]
    fn links_encode_spaces_and_breaking_characters() {
        assert_eq!(link("assets/", "image.png"), "![](assets/image.png)");
        assert_eq!(link("assets/", "image 2.png"), "![](assets/image%202.png)");
        assert_eq!(
            link("assets/", "a(1)%#?.png"),
            "![](assets/a%281%29%25%23%3F.png)"
        );
        assert_eq!(
            link("assets/", "ảnh chụp.png"),
            "![](assets/ảnh%20chụp.png)"
        );
    }

    #[test]
    fn stored_images_are_numbered_and_never_overwrite() {
        let t = TempDir::new("store");
        let note = t.0.join("Note.md");
        let png = |bytes: &'static [u8]| move |p: &Path| fs::write(p, bytes);
        assert_eq!(
            store(&dest(&note, None, ""), "image.png", png(b"1")).unwrap(),
            "![](assets/image.png)"
        );
        assert_eq!(
            store(&dest(&note, None, ""), "image.png", png(b"2")).unwrap(),
            "![](assets/image%202.png)"
        );
        let a = t.0.join("assets");
        assert_eq!(fs::read(a.join("image.png")).unwrap(), b"1");
        assert_eq!(fs::read(a.join("image 2.png")).unwrap(), b"2");

        // A failed write leaves nothing behind.
        let err = store(&dest(&note, None, ""), "image.png", |_| {
            Err(io::Error::other("full"))
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "full");
        assert!(!a.join("image 3.png").exists());
    }

    #[test]
    fn dropped_files_are_copied_or_linked_in_place() {
        let t = TempDir::new("drop");
        let note = t.0.join("sub").join("Note.md");
        fs::create_dir_all(t.0.join("sub/assets")).unwrap();
        fs::write(t.0.join("sub/assets/pic.png"), "old").unwrap();
        fs::write(t.0.join("pic.png"), "new").unwrap();

        // Same name already in assets/: copied as "pic 2.png", the old one kept.
        let l = store_file(&dest(&note, None, ""), &t.0.join("pic.png")).unwrap();
        assert_eq!(l, "![](assets/pic%202.png)");
        assert_eq!(fs::read(t.0.join("sub/assets/pic.png")).unwrap(), b"old");
        assert_eq!(fs::read(t.0.join("sub/assets/pic 2.png")).unwrap(), b"new");

        // Already in the note's assets/: linked, not copied.
        let l = store_file(&dest(&note, None, ""), &t.0.join("sub/assets/pic.png")).unwrap();
        assert_eq!(l, "![](assets/pic.png)");
        assert!(!t.0.join("sub/assets/pic 3.png").exists());

        // Copy failure (source gone): error, no empty file left.
        assert!(store_file(&dest(&note, None, ""), &t.0.join("gone.png")).is_err());
        assert!(!t.0.join("sub/assets/gone.png").exists());
    }

    #[test]
    fn attachments_folder_setting() {
        assert_eq!(setting(""), "assets");
        assert_eq!(setting("  / "), "assets");
        assert_eq!(setting(" Media "), "Media");
        assert_eq!(setting("/attachments"), "/attachments");
        assert_eq!(setting("../x/./y/.."), "x/y");
        assert_eq!(setting("/../attachments"), "/attachments");
    }

    #[test]
    fn attachments_folder_destination() {
        let root = Path::new("/v");
        let note = Path::new("/v/a/b/Note.md");
        assert_eq!(
            dest(note, Some(root), "/attachments"),
            Dest {
                dir: PathBuf::from("/v/attachments"),
                prefix: "../../attachments/".into()
            }
        );
        let top = dest(Path::new("/v/Note.md"), Some(root), "/attachments");
        assert_eq!(top.prefix, "attachments/");
        assert_eq!(
            dest(note, Some(root), "").dir,
            PathBuf::from("/v/a/b/assets")
        );
        assert_eq!(dest(note, Some(root), "img").prefix, "img/");
        // No open folder, or a note outside it: next to the note.
        assert_eq!(
            dest(note, None, "/attachments").dir,
            PathBuf::from("/v/a/b/attachments")
        );
        assert_eq!(
            dest(note, Some(Path::new("/w")), "/attachments").prefix,
            "attachments/"
        );
        assert_eq!(
            link(&dest(note, Some(root), "/My Files").prefix, "a.png"),
            "![](../../My%20Files/a.png)"
        );
    }

    #[test]
    fn attachments_folder_names() {
        use std::ffi::OsStr;
        assert!(is_attachments_name(OsStr::new("Assets"), "/attachments"));
        assert!(is_attachments_name(
            OsStr::new("Attachments"),
            "/attachments"
        ));
        assert!(is_attachments_name(OsStr::new("img"), "media/img"));
        assert!(!is_attachments_name(OsStr::new("notes"), "/attachments"));
    }
}
