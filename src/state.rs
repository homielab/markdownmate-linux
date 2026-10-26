//! Session state remembered between runs: open folder, open note, view mode, bookmarked
//! folders (in order, one `bookmark=` line each), recent folders (newest first, one `recent=`
//! line each), editor preferences (`Prefs`), cursor position per note (`pos=dòng:cột:đường dẫn`,
//! mới nhất trước, tối đa POS_MAX) and expanded sidebar folders (`expanded=`).
//! Stored as `key=value` lines; paths are byte-escaped (`%XX`) so non-UTF-8 paths survive.
//! A missing, unreadable or malformed file is never an error: unknown lines are ignored.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::save;

/// What the window shows: editor, preview, or both side by side.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum Mode {
    #[default]
    Edit,
    Preview,
    Split,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Edit, Mode::Preview, Mode::Split];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Edit => "edit",
            Mode::Preview => "preview",
            Mode::Split => "split",
        }
    }

    pub fn from_name(name: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// Sáng/tối của app (Preferences ▸ Style): theo hệ thống, luôn sáng hoặc luôn tối.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum Style {
    #[default]
    System,
    Light,
    Dark,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::System, Style::Light, Style::Dark];

    pub fn name(self) -> &'static str {
        match self {
            Style::System => "system",
            Style::Light => "light",
            Style::Dark => "dark",
        }
    }

    pub fn from_name(name: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// Theme màu của editor + preview (Preferences); sáng/tối theo `Style`.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum Theme {
    #[default]
    Default,
    Paper,
    Graphite,
    Solarized,
}

impl Theme {
    pub const ALL: [Theme; 4] = [
        Theme::Default,
        Theme::Paper,
        Theme::Graphite,
        Theme::Solarized,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Theme::Default => "default",
            Theme::Paper => "paper",
            Theme::Graphite => "graphite",
            Theme::Solarized => "solarized",
        }
    }

    pub fn from_name(name: &str) -> Option<Theme> {
        Theme::ALL.into_iter().find(|t| t.name() == name)
    }

    /// Id scheme GtkSourceView: dùng scheme có sẵn nếu khớp, không thì scheme riêng của app
    /// (`packaging/styles/*.xml`).
    pub fn scheme(self, dark: bool) -> &'static str {
        match (self, dark) {
            (Theme::Default, false) => "Adwaita",
            (Theme::Default, true) => "Adwaita-dark",
            (Theme::Paper, false) => "markdownmate-paper",
            (Theme::Paper, true) => "markdownmate-paper-dark",
            (Theme::Graphite, false) => "markdownmate-graphite",
            (Theme::Graphite, true) => "markdownmate-graphite-dark",
            (Theme::Solarized, false) => "solarized-light",
            (Theme::Solarized, true) => "solarized-dark",
        }
    }
}

/// How wide the text runs (Preferences ▸ Text Width): the editor's text and the preview's
/// page are centred at most this wide. Full Width = the whole editor and the whole preview
/// (the padding stays); print and exports always keep the page's own reading width.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum TextWidth {
    Narrow,
    #[default]
    Medium,
    Wide,
    Full,
}

impl TextWidth {
    pub const ALL: [TextWidth; 4] = [
        TextWidth::Narrow,
        TextWidth::Medium,
        TextWidth::Wide,
        TextWidth::Full,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TextWidth::Narrow => "narrow",
            TextWidth::Medium => "medium",
            TextWidth::Wide => "wide",
            TextWidth::Full => "full",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            TextWidth::Narrow => "Narrow",
            TextWidth::Medium => "Medium",
            TextWidth::Wide => "Wide",
            TextWidth::Full => "Full Width",
        }
    }

    pub fn from_name(name: &str) -> Option<TextWidth> {
        TextWidth::ALL.into_iter().find(|w| w.name() == name)
    }

    /// The widest the text may be, in pixels (as the macOS app); None = no limit.
    pub fn pixels(self) -> Option<i32> {
        match self {
            TextWidth::Narrow => Some(680),
            TextWidth::Medium => Some(820),
            TextWidth::Wide => Some(1080),
            TextWidth::Full => None,
        }
    }
}

/// Line spacing choices in percent of the font's line height (Preferences ▸ Line Spacing).
pub const LINE_SPACINGS: [u32; 5] = [100, 115, 130, 150, 200];

/// `line_spacing=` of the state file: a multiple like `1.15`, one of `LINE_SPACINGS`.
fn parse_line_spacing(v: &str) -> Option<u32> {
    let percent = (v.trim().parse::<f64>().ok()? * 100.0).round();
    LINE_SPACINGS.into_iter().find(|s| f64::from(*s) == percent)
}

fn line_spacing_name(percent: u32) -> String {
    let s = format!("{}.{:02}", percent / 100, percent % 100);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Editor preferences (Ctrl+,). A missing or broken line keeps the default.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    /// Pango font description (`Family Style Size`); None = the system monospace font.
    pub font: Option<String>,
    pub line_numbers: bool,
    pub wrap: bool,
    /// Spaces Tab / Shift+Tab add or remove on list lines: 2 or 4.
    pub indent: usize,
    /// Zoom of editor + preview in percent (Ctrl+= / Ctrl+- / Ctrl+0), ZOOM_MIN..=ZOOM_MAX.
    pub zoom: u32,
    pub theme: Theme,
    pub style: Style,
    /// Side by side: the preview follows the editor's scroll position.
    pub sync_scroll: bool,
    /// Tags on (`tags::set_options`); colour codes like `#fff` are not tags.
    pub tags: bool,
    pub ignore_colours: bool,
    /// Keep the cursor's line in the middle of the editor.
    pub typewriter: bool,
    /// The format bar above the editor (Shift+Ctrl+M).
    pub format_bar: bool,
    /// Focus Mode: the editor dims every paragraph but the cursor's (Shift+Ctrl+J).
    pub focus_mode: bool,
    /// Today's Note's folder, relative to the open folder (`daily::folder`); empty = the open
    /// folder itself.
    pub daily_folder: String,
    /// Line height in percent of the font's (`LINE_SPACINGS`); 100 = the font's own.
    pub line_spacing: u32,
    pub text_width: TextWidth,
    /// How the sidebar is sorted (`notes::SortOrder`).
    pub sidebar_sort: crate::notes::SortOrder,
    /// Preview, Print and Export load `http(s)` images; off = no request leaves the machine.
    pub web_images: bool,
}

pub const ZOOM_MIN: u32 = 50;
pub const ZOOM_MAX: u32 = 300;
const ZOOM_STEP: i64 = 10;

/// The zoom after `steps` presses (+ in, - out, 0 = back to 100%), kept in range.
pub fn zoom_step(zoom: u32, steps: i32) -> u32 {
    if steps == 0 {
        return 100;
    }
    (i64::from(zoom) + i64::from(steps) * ZOOM_STEP).clamp(ZOOM_MIN.into(), ZOOM_MAX.into()) as u32
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            font: None,
            line_numbers: false,
            wrap: true,
            indent: 4,
            zoom: 100,
            theme: Theme::Default,
            style: Style::System,
            sync_scroll: true,
            tags: true,
            ignore_colours: true,
            typewriter: false,
            format_bar: false,
            focus_mode: false,
            daily_folder: crate::daily::DEFAULT_FOLDER.to_string(),
            line_spacing: 100,
            text_width: TextWidth::Medium,
            sidebar_sort: crate::notes::SortOrder::Name,
            web_images: false,
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct State {
    pub folder: Option<PathBuf>,
    pub note: Option<PathBuf>,
    pub mode: Mode,
    pub bookmarks: Vec<PathBuf>,
    pub recent: Vec<PathBuf>,
    pub prefs: Prefs,
    /// Vị trí con trỏ theo note (dòng, cột 1-based), mới nhất trước.
    pub positions: Vec<Position>,
    /// Thư mục đang mở rộng trong sidebar (đường dẫn tuyệt đối, trong `folder`).
    pub expanded: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub note: PathBuf,
    pub line: usize,
    pub column: usize,
}

impl State {
    pub fn parse(text: &str) -> State {
        let mut state = State::default();
        for line in text.lines() {
            match line.split_once('=') {
                Some(("folder", v)) => state.folder = unescape(v),
                Some(("note", v)) => state.note = unescape(v),
                Some(("mode", v)) => state.mode = Mode::from_name(v).unwrap_or_default(),
                Some(("bookmark", v)) => {
                    if let Some(p) = unescape(v) {
                        toggle_bookmark(&mut state.bookmarks, &p, true);
                    }
                }
                Some(("recent", v)) => {
                    if let Some(p) = unescape(v)
                        && !state.recent.contains(&p)
                        && state.recent.len() < RECENT_MAX
                    {
                        state.recent.push(p);
                    }
                }
                Some(("pos", v)) => {
                    if let Some(p) = parse_position(v)
                        && state.positions.len() < POS_MAX
                        && !state.positions.iter().any(|q| q.note == p.note)
                    {
                        state.positions.push(p);
                    }
                }
                Some(("expanded", v)) => {
                    if let Some(p) = unescape(v)
                        && !state.expanded.contains(&p)
                    {
                        state.expanded.push(p);
                    }
                }
                Some(("font", v)) if !v.trim().is_empty() => {
                    state.prefs.font = Some(v.trim().to_string())
                }
                Some(("line_numbers", v)) => {
                    state.prefs.line_numbers = v.parse().unwrap_or(state.prefs.line_numbers)
                }
                Some(("wrap", v)) => state.prefs.wrap = v.parse().unwrap_or(state.prefs.wrap),
                Some(("sync_scroll", v)) => {
                    state.prefs.sync_scroll = v.parse().unwrap_or(state.prefs.sync_scroll)
                }
                Some(("tags", v)) => state.prefs.tags = v.parse().unwrap_or(state.prefs.tags),
                Some(("ignore_colours", v)) => {
                    state.prefs.ignore_colours = v.parse().unwrap_or(state.prefs.ignore_colours)
                }
                Some(("typewriter", v)) => {
                    state.prefs.typewriter = v.parse().unwrap_or(state.prefs.typewriter)
                }
                Some(("format_bar", v)) => {
                    state.prefs.format_bar = v.parse().unwrap_or(state.prefs.format_bar)
                }
                Some(("focus_mode", v)) => {
                    state.prefs.focus_mode = v.parse().unwrap_or(state.prefs.focus_mode)
                }
                Some(("web_images", v)) => {
                    state.prefs.web_images = v.parse().unwrap_or(state.prefs.web_images)
                }
                // Kept as typed (an empty value is a choice: the open folder).
                Some(("daily_folder", v)) => state.prefs.daily_folder = v.to_string(),
                Some(("indent", v @ ("2" | "4"))) => state.prefs.indent = v.parse().unwrap_or(4),
                Some(("zoom", v)) => {
                    if let Ok(z) = v.parse::<u32>() {
                        state.prefs.zoom = z.clamp(ZOOM_MIN, ZOOM_MAX);
                    }
                }
                Some(("line_spacing", v)) => {
                    state.prefs.line_spacing = parse_line_spacing(v).unwrap_or(100)
                }
                Some(("text_width", v)) => {
                    state.prefs.text_width = TextWidth::from_name(v).unwrap_or_default()
                }
                Some(("sidebar_sort", v)) => {
                    state.prefs.sidebar_sort =
                        crate::notes::SortOrder::from_name(v).unwrap_or_default()
                }
                Some(("theme", v)) => state.prefs.theme = Theme::from_name(v).unwrap_or_default(),
                Some(("style", v)) => state.prefs.style = Style::from_name(v).unwrap_or_default(),
                // Written before side by side existed.
                Some(("preview", "true")) => state.mode = Mode::Preview,
                _ => {}
            }
        }
        state
    }

    pub fn serialize(&self) -> String {
        let mut out = String::new();
        for (key, path) in [("folder", &self.folder), ("note", &self.note)] {
            if let Some(p) = path {
                out += &format!("{key}={}\n", escape(p));
            }
        }
        out += &format!("mode={}\n", self.mode.name());
        for b in &self.bookmarks {
            out += &format!("bookmark={}\n", escape(b));
        }
        for r in &self.recent {
            out += &format!("recent={}\n", escape(r));
        }
        for p in &self.positions {
            out += &format!("pos={}:{}:{}\n", p.line, p.column, escape(&p.note));
        }
        for e in &self.expanded {
            out += &format!("expanded={}\n", escape(e));
        }
        let p = &self.prefs;
        if let Some(font) = &p.font {
            out += &format!("font={font}\n");
        }
        out += &format!(
            "line_numbers={}\nwrap={}\nindent={}\nzoom={}\ntheme={}\nstyle={}\nsync_scroll={}\ntags={}\nignore_colours={}\ntypewriter={}\nformat_bar={}\nfocus_mode={}\ndaily_folder={}\nline_spacing={}\ntext_width={}\nsidebar_sort={}\nweb_images={}\n",
            p.line_numbers,
            p.wrap,
            p.indent,
            p.zoom,
            p.theme.name(),
            p.style.name(),
            p.sync_scroll,
            p.tags,
            p.ignore_colours,
            p.typewriter,
            p.format_bar,
            p.focus_mode,
            p.daily_folder.replace('\n', " "),
            line_spacing_name(p.line_spacing),
            p.text_width.name(),
            p.sidebar_sort.name(),
            p.web_images
        );
        out
    }

    /// Drops what no longer exists: a folder that is not a directory, a note that is not a
    /// file inside the folder. Preview / side by side only make sense with a note.
    pub fn usable(self) -> State {
        let folder = self.folder.filter(|f| f.is_absolute() && f.is_dir());
        let note = match &folder {
            Some(f) => self.note.filter(|n| n.starts_with(f) && n.is_file()),
            None => None,
        };
        let expanded = match &folder {
            Some(f) => self
                .expanded
                .into_iter()
                .filter(|e| e.starts_with(f))
                .collect(),
            None => Vec::new(),
        };
        let mode = if note.is_some() {
            self.mode
        } else {
            Mode::Edit
        };
        // Bookmarks are kept even if gone now (an unmounted drive may come back); opening one
        // that is gone only shows a toast.
        State {
            folder,
            note,
            mode,
            bookmarks: self.bookmarks,
            recent: self.recent,
            prefs: self.prefs,
            // Note đã mất chỉ chiếm chỗ đến khi bị đẩy ra khỏi POS_MAX.
            positions: self.positions,
            expanded,
        }
    }
}

/// CSS giving `.mm-editor` the chosen font. `families` is Pango's comma-separated family
/// list; each name is quoted (`\\` and `"` escaped, control characters dropped) so a font
/// name can never break out of the rule. `size_pt` None keeps the theme's size; `zoom` (%)
/// scales it.
pub fn font_css(
    families: &str,
    size_pt: Option<f64>,
    weight: i32,
    italic: bool,
    zoom: u32,
) -> String {
    let mut list: Vec<String> = families
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(|f| {
            let mut q = String::from('"');
            for c in f.chars().filter(|c| !c.is_control()) {
                if c == '"' || c == '\\' {
                    q.push('\\');
                }
                q.push(c);
            }
            q + "\""
        })
        .collect();
    list.push("monospace".into());
    let mut css = format!(
        ".mm-editor {{ font-family: {}; font-weight: {}; font-style: {};",
        list.join(", "),
        weight.clamp(1, 1000),
        if italic { "italic" } else { "normal" }
    );
    css += &zoom_size(size_pt, zoom);
    css + " }"
}

/// The editor's line spacing as CSS (GTK's `line-height`, a multiple of the font's line
/// height): "" at 100% (the font's own spacing).
pub fn line_height_css(percent: u32) -> String {
    if percent == 100 {
        return String::new();
    }
    format!(
        ".mm-editor {{ line-height: {}; }}",
        line_spacing_name(percent)
    )
}

/// The side margins that centre text at most `max` wide in a view `width` wide, never less
/// than `min`.
pub fn centred_margin(width: i32, max: Option<i32>, min: i32) -> i32 {
    match max {
        Some(max) => ((width - max) / 2).max(min),
        None => min,
    }
}

/// Only the zoomed size, for the system monospace font (no font chosen): "" at 100%.
pub fn zoom_css(zoom: u32) -> String {
    match zoom_size(None, zoom) {
        s if s.is_empty() => s,
        s => format!(".mm-editor {{{s} }}"),
    }
}

fn zoom_size(size_pt: Option<f64>, zoom: u32) -> String {
    match size_pt.filter(|s| s.is_finite() && *s > 0.0) {
        Some(pt) => format!(" font-size: {}pt;", pt * f64::from(zoom) / 100.0),
        None if zoom != 100 => format!(" font-size: {zoom}%;"),
        None => String::new(),
    }
}

/// Adds `dir` at the end (`on`, if not already there) or removes it (`!on`).
pub fn toggle_bookmark(bookmarks: &mut Vec<PathBuf>, dir: &Path, on: bool) {
    let at = bookmarks.iter().position(|b| b == dir);
    match (at, on) {
        (None, true) => bookmarks.push(dir.to_path_buf()),
        (Some(i), false) => {
            bookmarks.remove(i);
        }
        _ => {}
    }
}

/// How many recently opened folders are remembered.
pub const RECENT_MAX: usize = 10;

/// Puts `dir` first in `recent` (moved there if already listed), keeping at most RECENT_MAX.
pub fn push_recent(recent: &mut Vec<PathBuf>, dir: &Path) {
    recent.retain(|r| r != dir);
    recent.insert(0, dir.to_path_buf());
    recent.truncate(RECENT_MAX);
}

/// How many notes' cursor positions are remembered.
pub const POS_MAX: usize = 100;

/// Puts `note`'s cursor position first (replacing an older one), keeping at most POS_MAX.
pub fn push_position(positions: &mut Vec<Position>, note: &Path, line: usize, column: usize) {
    positions.retain(|p| p.note != note);
    positions.insert(
        0,
        Position {
            note: note.to_path_buf(),
            line,
            column,
        },
    );
    positions.truncate(POS_MAX);
}

/// `dòng:cột:đường dẫn escape`; None nếu hỏng (số 0 / không phải số / đường dẫn hỏng).
fn parse_position(v: &str) -> Option<Position> {
    let (line, rest) = v.split_once(':')?;
    let (column, path) = rest.split_once(':')?;
    let (line, column) = (line.parse().ok()?, column.parse().ok()?);
    (line > 0 && column > 0).then_some(())?;
    Some(Position {
        note: unescape(path)?,
        line,
        column,
    })
}

/// Missing, unreadable or non-UTF-8 file = empty state.
pub fn load(path: &Path) -> State {
    fs::read_to_string(path)
        .map(|t| State::parse(&t))
        .unwrap_or_default()
}

pub fn store(path: &Path, state: &State) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    save::write_atomic(path, &state.serialize())
}

/// `%`, control and non-ASCII bytes become `%XX`; the result is plain ASCII on one line.
fn escape(path: &Path) -> String {
    let mut out = String::new();
    for &b in path.as_os_str().as_bytes() {
        if b == b'%' || !(0x20..0x7f).contains(&b) {
            out += &format!("%{b:02X}");
        } else {
            out.push(b as char);
        }
    }
    out
}

/// None if the value is empty or has a broken `%` escape.
pub fn unescape(s: &str) -> Option<PathBuf> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    (!out.is_empty()).then(|| PathBuf::from(OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mdmate-state-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn round_trips_including_non_utf8_paths() {
        let state = State {
            folder: Some(PathBuf::from("/home/u/Ghi chú 100%")),
            note: Some(PathBuf::from(OsString::from_vec(
                b"/home/u/\xff=\n.md".to_vec(),
            ))),
            mode: Mode::Preview,
            bookmarks: Vec::new(),
            recent: Vec::new(),
            prefs: Prefs::default(),
            positions: Vec::new(),
            expanded: Vec::new(),
        };
        let text = state.serialize();
        assert!(text.is_ascii());
        assert_eq!(text.lines().count(), 20);
        assert_eq!(State::parse(&text), state);
    }

    #[test]
    fn mode_round_trips_and_old_preview_key_is_read() {
        for mode in Mode::ALL {
            let state = State {
                mode,
                ..State::default()
            };
            assert_eq!(State::parse(&state.serialize()), state);
        }
        assert_eq!(State::parse("preview=true\n").mode, Mode::Preview);
        assert_eq!(State::parse("preview=false\n").mode, Mode::Edit);
        assert_eq!(State::parse("mode=bogus\n").mode, Mode::Edit);
    }

    #[test]
    fn bookmarks_keep_order_add_remove_and_old_files_have_none() {
        let odd = PathBuf::from(OsString::from_vec(b"/home/u/\xff=%\n".to_vec()));
        let (a, b) = (
            PathBuf::from("/home/u/Work"),
            PathBuf::from("/home/u/Cá nhân"),
        );
        let mut marks = Vec::new();
        for p in [&b, &a, &odd, &a] {
            toggle_bookmark(&mut marks, p, true);
        }
        assert_eq!(marks, [b.clone(), a.clone(), odd.clone()]);
        toggle_bookmark(&mut marks, &a, false);
        toggle_bookmark(&mut marks, Path::new("/nope"), false);
        assert_eq!(marks, [b.clone(), odd.clone()]);

        let state = State {
            folder: Some(a.clone()),
            bookmarks: marks.clone(),
            ..State::default()
        };
        let text = state.serialize();
        assert!(text.is_ascii());
        assert_eq!(State::parse(&text), state);
        // Duplicates and broken lines in the file are dropped; usable keeps bookmarks.
        let parsed = State::parse(&format!("{text}bookmark=%G1\nbookmark=\n{text}"));
        assert_eq!(parsed.bookmarks, marks);
        assert_eq!(State::parse(&text).usable().bookmarks, marks);
        // A state file written before bookmarks existed.
        let old = State::parse("folder=/home/u/Work\nnote=/home/u/Work/a.md\nmode=split\n");
        assert_eq!(old.folder, Some(a));
        assert_eq!(old.mode, Mode::Split);
        assert!(old.bookmarks.is_empty());
    }

    #[test]
    fn recent_newest_first_no_duplicates_at_most_ten_old_files_have_none() {
        let dirs: Vec<PathBuf> = (0..12).map(|i| PathBuf::from(format!("/n/{i}"))).collect();
        let mut recent = Vec::new();
        for d in &dirs {
            push_recent(&mut recent, d);
        }
        assert_eq!(recent.len(), RECENT_MAX);
        assert_eq!(recent[0], dirs[11]);
        assert_eq!(recent[9], dirs[2]);
        // Reopening one moves it to the front, not twice in the list.
        push_recent(&mut recent, &dirs[5]);
        assert_eq!(recent[0], dirs[5]);
        assert_eq!(recent.iter().filter(|r| **r == dirs[5]).count(), 1);
        assert_eq!(recent.len(), RECENT_MAX);

        let odd = PathBuf::from(OsString::from_vec(b"/home/u/\xff=%\n".to_vec()));
        push_recent(&mut recent, &odd);
        let state = State {
            recent: recent.clone(),
            ..State::default()
        };
        let text = state.serialize();
        assert!(text.is_ascii());
        assert_eq!(State::parse(&text), state);
        assert_eq!(State::parse(&text).usable().recent, recent);
        // Duplicates, broken lines and more than ten lines in the file are dropped.
        let parsed = State::parse(&format!("recent=%G1\nrecent=\n{text}{text}recent=/x\n"));
        assert_eq!(parsed.recent, recent);
        // A state file written before recent folders existed.
        assert!(
            State::parse("folder=/home/u/Work\nbookmark=/a\n")
                .recent
                .is_empty()
        );
    }

    #[test]
    fn positions_newest_first_capped_broken_skipped_old_files_have_none() {
        let mut pos = Vec::new();
        for i in 0..105 {
            push_position(&mut pos, Path::new(&format!("/n/{i}.md")), i + 1, 2);
        }
        assert_eq!(pos.len(), POS_MAX);
        assert_eq!(pos[0].note, PathBuf::from("/n/104.md"));
        push_position(&mut pos, Path::new("/n/50.md"), 7, 3);
        assert_eq!((pos[0].line, pos[0].column), (7, 3));
        assert_eq!(pos.iter().filter(|p| p.note.ends_with("50.md")).count(), 1);
        assert_eq!(pos.len(), POS_MAX);

        let odd = PathBuf::from(OsString::from_vec(b"/home/u/\xff:%=\n.md".to_vec()));
        push_position(&mut pos, &odd, 12, 1);
        let state = State {
            positions: pos.clone(),
            expanded: vec![PathBuf::from("/n/a"), odd.clone()],
            ..State::default()
        };
        let text = state.serialize();
        assert!(text.is_ascii());
        assert_eq!(State::parse(&text), state);
        // Hỏng / trùng / quá POS_MAX dòng: bỏ qua.
        let parsed = State::parse(&format!(
            "pos=x:1:/a\npos=0:1:/a\npos=1:1:\npos=1:/a\npos=1:1:%G1\n{text}{text}pos=1:1:/z\n"
        ));
        assert_eq!(parsed.positions, pos);
        assert_eq!(parsed.expanded, state.expanded);
        // File state cũ, chưa có khoá mới.
        let old = State::parse("folder=/n\nnote=/n/a.md\nmode=edit\n");
        assert!(old.positions.is_empty() && old.expanded.is_empty());
    }

    #[test]
    fn usable_keeps_expanded_inside_folder_only() {
        let dir = tmp("expanded");
        let state = State {
            folder: Some(dir.clone()),
            expanded: vec![
                dir.join("a"),
                PathBuf::from("/elsewhere/b"),
                dir.join("a/c"),
            ],
            ..State::default()
        };
        assert_eq!(state.usable().expanded, [dir.join("a"), dir.join("a/c")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefs_round_trip_default_on_old_or_broken_files() {
        let prefs = Prefs {
            font: Some("DejaVu Sans Mono Bold 13".into()),
            line_numbers: true,
            wrap: false,
            indent: 2,
            zoom: 100,
            theme: Theme::Graphite,
            style: Style::Dark,
            sync_scroll: false,
            tags: false,
            ignore_colours: false,
            typewriter: true,
            format_bar: true,
            focus_mode: true,
            daily_folder: " Journal/2026 ".into(),
            line_spacing: 115,
            text_width: TextWidth::Wide,
            sidebar_sort: crate::notes::SortOrder::Modified,
            web_images: true,
        };
        let state = State {
            prefs: prefs.clone(),
            ..State::default()
        };
        assert_eq!(State::parse(&state.serialize()).prefs, prefs);
        // Written before preferences existed.
        let old = State::parse("folder=/a\nmode=edit\n");
        assert_eq!(old.prefs, Prefs::default());
        assert!(old.prefs.wrap && old.prefs.indent == 4 && old.prefs.font.is_none());
        assert!(old.prefs.sync_scroll && old.prefs.tags && old.prefs.ignore_colours);
        assert!(!old.prefs.typewriter && !old.prefs.format_bar && !old.prefs.focus_mode);
        assert_eq!(old.prefs.daily_folder, "Daily");
        assert_eq!(old.prefs.line_spacing, 100);
        assert_eq!(old.prefs.text_width, TextWidth::Medium);
        assert_eq!(old.prefs.sidebar_sort, crate::notes::SortOrder::Name);
        assert!(!old.prefs.web_images);
        // An empty daily notes folder (the open folder) stays empty.
        assert_eq!(State::parse("daily_folder=\n").prefs.daily_folder, "");
        // Broken values keep the default.
        let bad = State::parse(
            "font= \nline_numbers=yes\nwrap=1\nindent=3\nindent=x\ntheme=neon\nstyle=dim\nsync_scroll=no\ntags=0\nignore_colours=off\ntypewriter=on\nformat_bar=yes\nfocus_mode=1\nline_spacing=1.2\nline_spacing=x\ntext_width=huge\nsidebar_sort=size\nweb_images=yes\n",
        );
        assert_eq!(bad.prefs, Prefs::default());
        for t in Theme::ALL {
            assert_eq!(Theme::from_name(t.name()), Some(t));
        }
        for s in Style::ALL {
            assert_eq!(Style::from_name(s.name()), Some(s));
        }
    }

    #[test]
    fn line_height_and_centred_margin() {
        assert_eq!(line_height_css(100), "");
        assert_eq!(line_height_css(130), ".mm-editor { line-height: 1.3; }");
        assert_eq!(line_height_css(200), ".mm-editor { line-height: 2; }");
        assert_eq!(centred_margin(1000, None, 12), 12);
        assert_eq!(centred_margin(1000, Some(680), 12), 160);
        assert_eq!(centred_margin(600, Some(680), 12), 12);
    }

    #[test]
    fn line_spacing_and_text_width_keys() {
        for percent in LINE_SPACINGS {
            let text = format!("line_spacing={}\n", line_spacing_name(percent));
            assert_eq!(State::parse(&text).prefs.line_spacing, percent);
        }
        assert_eq!(line_spacing_name(100), "1");
        assert_eq!(line_spacing_name(115), "1.15");
        assert_eq!(line_spacing_name(130), "1.3");
        assert_eq!(line_spacing_name(200), "2");
        assert_eq!(State::parse("line_spacing=1.50\n").prefs.line_spacing, 150);
        assert_eq!(State::parse("line_spacing=0\n").prefs.line_spacing, 100);
        for w in TextWidth::ALL {
            let state = State::parse(&format!("text_width={}\n", w.name()));
            assert_eq!(state.prefs.text_width, w);
        }
        assert_eq!(TextWidth::Full.pixels(), None);
        assert_eq!(TextWidth::Medium.pixels(), Some(820));
        let serialized = State::default().serialize();
        assert!(serialized.contains("\nline_spacing=1\ntext_width=medium\n"));
    }

    #[test]
    fn font_css_quotes_and_escapes_family_names() {
        assert_eq!(
            font_css("Fira Code", Some(12.0), 400, false, 100),
            ".mm-editor { font-family: \"Fira Code\", monospace; font-weight: 400; \
             font-style: normal; font-size: 12pt; }"
        );
        let evil = font_css("A\"; } * { color: red, B\\\n", None, 2000, true, 100);
        assert_eq!(
            evil,
            ".mm-editor { font-family: \"A\\\"; } * { color: red\", \"B\\\\\", monospace; \
             font-weight: 1000; font-style: italic; }"
        );
        assert!(font_css("", Some(f64::NAN), 400, false, 100).contains("font-family: monospace;"));
    }

    #[test]
    fn zoom_steps_clamps_resets_and_scales_font() {
        assert_eq!(zoom_step(100, 1), 110);
        assert_eq!(zoom_step(100, -1), 90);
        assert_eq!(zoom_step(250, 0), 100);
        assert_eq!(zoom_step(295, 1), ZOOM_MAX);
        assert_eq!(zoom_step(ZOOM_MAX, 1), ZOOM_MAX);
        assert_eq!(zoom_step(55, -1), ZOOM_MIN);
        assert_eq!(zoom_step(ZOOM_MIN, -1), ZOOM_MIN);
        assert_eq!(zoom_step(100, i32::MIN), ZOOM_MIN);
        assert!(font_css("Fira Code", Some(12.0), 400, false, 150).ends_with("font-size: 18pt; }"));
        assert!(font_css("Fira Code", None, 400, false, 120).ends_with("font-size: 120%; }"));
        assert_eq!(zoom_css(100), "");
        assert_eq!(zoom_css(80), ".mm-editor { font-size: 80%; }");
    }

    #[test]
    fn zoom_round_trips_and_old_or_broken_files_are_100() {
        let state = State {
            prefs: Prefs {
                zoom: 130,
                ..Prefs::default()
            },
            ..State::default()
        };
        assert!(state.serialize().contains("zoom=130\n"));
        assert_eq!(State::parse(&state.serialize()).prefs.zoom, 130);
        assert_eq!(State::parse("folder=/a\nindent=2\n").prefs.zoom, 100);
        assert_eq!(State::parse("zoom=x\nzoom=-5\n").prefs.zoom, 100);
        assert_eq!(State::parse("zoom=9999\n").prefs.zoom, ZOOM_MAX);
        assert_eq!(State::parse("zoom=1\n").prefs.zoom, ZOOM_MIN);
    }

    #[test]
    fn store_then_load_creates_config_dir() {
        let dir = tmp("store");
        let file = dir.join("markdownmate/state");
        let state = State {
            folder: Some(dir.clone()),
            note: None,
            mode: Mode::Edit,
            ..State::default()
        };
        store(&file, &state).unwrap();
        assert_eq!(load(&file), state);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_corrupt_file_gives_empty_state() {
        let dir = tmp("corrupt");
        assert_eq!(load(&dir.join("nope")), State::default());
        let bad = dir.join("bad");
        fs::write(&bad, b"\xff\xfe garbage").unwrap();
        assert_eq!(load(&bad), State::default());
        let partial = State::parse("junk\nfolder=%G1\nnote=%4\npreview=yes\nnote=/a.md\n");
        assert_eq!(partial.folder, None);
        assert_eq!(partial.note, Some(PathBuf::from("/a.md")));
        assert_eq!(partial.mode, Mode::Edit);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn usable_drops_paths_that_are_gone() {
        let dir = tmp("usable");
        let note = dir.join("a.md");
        fs::write(&note, "x").unwrap();
        let full = State {
            folder: Some(dir.clone()),
            note: Some(note.clone()),
            mode: Mode::Preview,
            ..State::default()
        };
        let kept = State {
            folder: Some(dir.clone()),
            note: Some(note.clone()),
            mode: Mode::Preview,
            ..State::default()
        };
        assert_eq!(full.usable(), kept);

        let note_gone = State {
            folder: Some(dir.clone()),
            note: Some(dir.join("gone.md")),
            mode: Mode::Preview,
            ..State::default()
        };
        assert_eq!(
            note_gone.usable(),
            State {
                folder: Some(dir.clone()),
                note: None,
                mode: Mode::Edit,
                ..State::default()
            }
        );

        let outside = State {
            folder: Some(dir.join("sub")),
            note: Some(note),
            mode: Mode::Edit,
            ..State::default()
        };
        assert_eq!(outside.usable(), State::default());
        let _ = fs::remove_dir_all(&dir);
    }
}
