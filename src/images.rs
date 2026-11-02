//! Pasted / dropped images: stored in the attachments folder (`Prefs::attachments_folder`,
//! default `assets/` next to the note; `/name` = at the open folder's root), linked relative
//! to the note. Pasted image data can be converted to PNG / JPEG and scaled down
//! (`PastedFormat`, `MAX_WIDTHS`, `convert`).
//! No display needed (gdk-pixbuf only), so it is testable headless.

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

/// Preferences ▸ Notes ▸ Pasted Images: the format image data pasted from the clipboard is
/// saved in. Keep = as before (PNG, the format GTK gives the clipboard image in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PastedFormat {
    #[default]
    Keep,
    Png,
    Jpeg,
}

impl PastedFormat {
    pub const ALL: [PastedFormat; 3] = [PastedFormat::Keep, PastedFormat::Png, PastedFormat::Jpeg];

    pub fn name(self) -> &'static str {
        match self {
            PastedFormat::Keep => "keep",
            PastedFormat::Png => "png",
            PastedFormat::Jpeg => "jpeg",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            PastedFormat::Keep => "Keep Format",
            PastedFormat::Png => "PNG",
            PastedFormat::Jpeg => "JPEG",
        }
    }

    pub fn from_name(name: &str) -> Option<PastedFormat> {
        PastedFormat::ALL.into_iter().find(|f| f.name() == name)
    }

    /// The extension of the file written.
    pub fn extension(self) -> &'static str {
        match self {
            PastedFormat::Keep | PastedFormat::Png => "png",
            PastedFormat::Jpeg => "jpg",
        }
    }
}

/// Preferences ▸ Notes ▸ Scale Down To: the widest a pasted image is saved, in pixels; 0 = Off.
pub const MAX_WIDTHS: [u32; 4] = [0, 1200, 1600, 2000];

/// JPEG quality of converted pasted images (gdk-pixbuf's 0–100 scale).
const JPEG_QUALITY: &str = "85";

/// `pasted_image_max_width=` of the state file: one of `MAX_WIDTHS`.
pub fn parse_max_width(v: &str) -> Option<u32> {
    let w = v.trim().parse().ok()?;
    MAX_WIDTHS.contains(&w).then_some(w)
}

/// Whether the pasted image settings leave pasted images as before (Keep, Off).
pub fn changes_nothing(format: PastedFormat, max_width: u32) -> bool {
    format == PastedFormat::Keep && max_width == 0
}

/// The size a `w`×`h` image is saved at with `max_width` (0 = Off): shrunk to `max_width`
/// wide with its aspect ratio (height rounded, at least 1), never enlarged.
pub fn target_size(w: u32, h: u32, max_width: u32) -> (u32, u32) {
    if max_width == 0 || w <= max_width {
        return (w, h);
    }
    let h = (u64::from(h) * u64::from(max_width) + u64::from(w) / 2) / u64::from(w);
    (max_width, (h as u32).max(1))
}

/// The extension a clipboard image of type `mime` is stored with as it is, never converted
/// or scaled: GIF (it may be animated) and SVG (a drawing, not pixels). None: other types.
pub fn passthrough(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next().unwrap_or("").trim();
    if mime.eq_ignore_ascii_case("image/gif") {
        Some("gif")
    } else if mime.eq_ignore_ascii_case("image/svg+xml") || mime.eq_ignore_ascii_case("image/svg") {
        Some("svg")
    } else {
        None
    }
}

/// Pasted image data (`png`: PNG bytes, as GTK gives the clipboard image) in `format`, at most
/// `max_width` wide (`target_size`, bilinear): the bytes and the extension to write. JPEG is
/// written at quality 85 with transparency flattened on white. PNG that needs no scaling is
/// returned as it is, without decoding.
pub fn convert(
    png: Vec<u8>,
    format: PastedFormat,
    max_width: u32,
) -> Result<(Vec<u8>, &'static str), gtk::glib::Error> {
    use gtk::gdk_pixbuf::{InterpType, PixbufLoader, prelude::*};
    let ext = format.extension();
    if format != PastedFormat::Jpeg && max_width == 0 {
        return Ok((png, ext));
    }
    let failed = || gtk::glib::Error::new(gtk::glib::FileError::Failed, "cannot convert the image");
    let loader = PixbufLoader::with_type("png")?;
    loader.write(&png)?;
    loader.close()?;
    let pixbuf = loader.pixbuf().ok_or_else(failed)?;
    let (w, h) = (pixbuf.width() as u32, pixbuf.height() as u32);
    let (tw, th) = target_size(w, h, max_width);
    let scaled = (tw, th) != (w, h);
    if format != PastedFormat::Jpeg && !scaled {
        return Ok((png, ext));
    }
    let (tw, th) = (tw as i32, th as i32);
    let out = if format == PastedFormat::Jpeg && pixbuf.has_alpha() {
        // A new pixbuf without alpha, blended over white.
        pixbuf.composite_color_simple(tw, th, InterpType::Bilinear, 255, 8, 0xFFFFFF, 0xFFFFFF)
    } else if scaled {
        pixbuf.scale_simple(tw, th, InterpType::Bilinear)
    } else {
        Some(pixbuf)
    }
    .ok_or_else(failed)?;
    let bytes = match format {
        PastedFormat::Jpeg => out.save_to_bufferv("jpeg", &[("quality", JPEG_QUALITY)])?,
        _ => out.save_to_bufferv("png", &[])?,
    };
    Ok((bytes, ext))
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
    fn pasted_images_shrink_only_and_keep_the_aspect_ratio() {
        assert_eq!(target_size(3000, 2000, 1600), (1600, 1067));
        assert_eq!(target_size(800, 600, 1600), (800, 600));
        assert_eq!(target_size(1600, 900, 1600), (1600, 900));
        assert_eq!(target_size(3000, 2000, 0), (3000, 2000));
        assert_eq!(target_size(4000, 1, 1200), (1200, 1));
        assert_eq!(target_size(2560, 1600, 2000), (2000, 1250));
    }

    #[test]
    fn pasted_image_settings() {
        let ext: Vec<_> = PastedFormat::ALL.map(PastedFormat::extension).into();
        assert_eq!(ext, ["png", "png", "jpg"]);
        for f in PastedFormat::ALL {
            assert_eq!(PastedFormat::from_name(f.name()), Some(f));
        }
        assert_eq!(PastedFormat::from_name("webp"), None);
        assert_eq!(PastedFormat::default(), PastedFormat::Keep);
        assert_eq!(parse_max_width("1600"), Some(1600));
        assert_eq!(parse_max_width("0"), Some(0));
        assert_eq!(parse_max_width("1000"), None);
        assert_eq!(parse_max_width("x"), None);
        assert!(changes_nothing(PastedFormat::Keep, 0));
        assert!(!changes_nothing(PastedFormat::Png, 0));
        assert!(!changes_nothing(PastedFormat::Keep, 1200));
    }

    #[test]
    fn gif_and_svg_pass_through() {
        assert_eq!(passthrough("image/gif"), Some("gif"));
        assert_eq!(passthrough("IMAGE/GIF"), Some("gif"));
        assert_eq!(passthrough("image/svg+xml"), Some("svg"));
        assert_eq!(passthrough("image/svg+xml;charset=utf-8"), Some("svg"));
        for other in [
            "image/png",
            "image/jpeg",
            "image/webp",
            "image/bmp",
            "text/plain",
        ] {
            assert_eq!(passthrough(other), None, "{other}");
        }
    }

    fn png(w: i32, h: i32, alpha: bool, rgba: u32) -> Vec<u8> {
        let p =
            gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, alpha, 8, w, h).unwrap();
        p.fill(rgba);
        p.save_to_bufferv("png", &[]).unwrap()
    }

    fn decode(bytes: &[u8]) -> gtk::gdk_pixbuf::Pixbuf {
        use gtk::gdk_pixbuf::prelude::*;
        let loader = gtk::gdk_pixbuf::PixbufLoader::new();
        loader.write(bytes).unwrap();
        loader.close().unwrap();
        loader.pixbuf().unwrap()
    }

    #[test]
    fn pasted_images_are_converted_and_scaled() {
        let big = png(3000, 2000, false, 0x3366_99FF);
        // Keep / PNG without scaling: the bytes as they are.
        assert_eq!(
            convert(big.clone(), PastedFormat::Keep, 0).unwrap(),
            (big.clone(), "png")
        );
        assert_eq!(
            convert(big.clone(), PastedFormat::Png, 0).unwrap(),
            (big.clone(), "png")
        );
        let small = png(800, 600, true, 0x0000_0000);
        assert_eq!(
            convert(small.clone(), PastedFormat::Png, 1600).unwrap().0,
            small
        );

        let (bytes, ext) = convert(big.clone(), PastedFormat::Jpeg, 1600).unwrap();
        assert_eq!(ext, "jpg");
        assert!(bytes.starts_with(&[0xFF, 0xD8]), "JPEG data");
        let p = decode(&bytes);
        assert_eq!((p.width(), p.height()), (1600, 1067));

        let (bytes, ext) = convert(big, PastedFormat::Keep, 2000).unwrap();
        assert_eq!(ext, "png");
        let p = decode(&bytes);
        assert_eq!((p.width(), p.height()), (2000, 1333));

        // Transparent → white in JPEG, not enlarged.
        let (bytes, _) = convert(small, PastedFormat::Jpeg, 1600).unwrap();
        let p = decode(&bytes);
        assert_eq!((p.width(), p.height(), p.has_alpha()), (800, 600, false));
        let px = p.read_pixel_bytes();
        assert!(px[..3].iter().all(|&c| c > 245), "white: {:?}", &px[..3]);
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
