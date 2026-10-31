//! Preferences ▸ About (version, What's New, Send Feedback, Share, Donate, More Apps, website)
//! and What's New: notes bundled from `resources/whats-new.toml`, a badge on the main menu
//! button, its What's New item and the Preferences row while a flagged release is unseen.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::OnceLock;

use adw::prelude::*;
use gtk::gio;
use gtk::glib::{self, clone};

use crate::editor::Editor;
use crate::{APP_ID, state, state_file};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
const FEEDBACK_EMAIL: &str = "markdownmate@homielab.com";
const SHARE_TEXT: &str =
    "MarkdownMate: write notes as plain Markdown files. https://markdownmate.homielab.com/ubuntu/";
pub(crate) const DONATE_URL: &str = "https://ko-fi.com/homielab";
const MORE_APPS_URL: &str = "https://homielab.com";
const WEBSITE_URL: &str = "https://markdownmate.homielab.com/";
const CHANGELOG_URL: &str = "https://markdownmate.homielab.com/#ubuntu";
/// The last version without What's New: its state file has no `last_seen_whats_new`.
const BEFORE_WHATS_NEW: &str = "0.7.0";
const WHATS_NEW: &str = include_str!("../resources/whats-new.toml");
const WHATS_NEW_ACTION: &str = "win.whats-new";

type Version = (u32, u32, u32);

/// `X.Y.Z` → numbers; anything else is None.
fn parse_version(text: &str) -> Option<Version> {
    let mut parts = text.trim().split('.').map(|p| p.parse::<u32>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Release {
    pub(crate) version: String,
    number: Version,
    /// Badges the menu once after an update (X.Y.0 feature releases).
    show: bool,
    pub(crate) notes: Vec<String>,
}

/// The releases of a whats-new.toml text, newest first; an entry without a valid version or
/// notes is skipped.
fn parse_releases(text: &str) -> Vec<Release> {
    let Ok(table) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let entries = table.get("release").and_then(|r| r.as_array());
    let mut releases: Vec<Release> = entries
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.as_table()?;
            let version = entry.get("version")?.as_str()?.to_string();
            let number = parse_version(&version)?;
            let show = entry.get("show").and_then(|s| s.as_bool()) == Some(true);
            let notes: Vec<String> = entry
                .get("notes")?
                .as_array()?
                .iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect();
            (!notes.is_empty()).then_some(Release {
                version,
                number,
                show,
                notes,
            })
        })
        .collect();
    releases.sort_by_key(|r| std::cmp::Reverse(r.number));
    releases
}

fn releases() -> &'static [Release] {
    static RELEASES: OnceLock<Vec<Release>> = OnceLock::new();
    RELEASES.get_or_init(|| parse_releases(WHATS_NEW))
}

/// Flagged releases after `last_seen` up to `current`, newest first.
fn unseen<'a>(releases: &'a [Release], last_seen: &str, current: &str) -> Vec<&'a Release> {
    let (Some(seen), Some(now)) = (parse_version(last_seen), parse_version(current)) else {
        return Vec::new();
    };
    releases
        .iter()
        .filter(|r| r.show && r.number > seen && r.number <= now)
        .collect()
}

/// What the What's New dialog shows: every unseen flagged release, else the newest flagged
/// one up to `current`.
fn to_show<'a>(releases: &'a [Release], last_seen: &str, current: &str) -> Vec<&'a Release> {
    let unseen = unseen(releases, last_seen, current);
    if !unseen.is_empty() {
        return unseen;
    }
    let now = parse_version(current);
    releases
        .iter()
        .filter(|r| r.show && Some(r.number) <= now)
        .take(1)
        .collect()
}

/// The newest release up to `current`, flagged or not (the About dialog's release notes).
fn latest<'a>(releases: &'a [Release], current: &str) -> Option<&'a Release> {
    let now = parse_version(current)?;
    releases.iter().find(|r| r.number <= now)
}

/// The version whose news counts as seen when the state file names none: a fresh install
/// (no state file) starts at this version; a state file written before What's New existed,
/// at the version before it.
fn first_seen(state_existed: bool) -> &'static str {
    if state_existed {
        BEFORE_WHATS_NEW
    } else {
        VERSION
    }
}

/// Shows (true) or hides one badge; returns false once its widget is gone.
type Badge = Box<dyn Fn(bool) -> bool>;

thread_local! {
    /// A flagged release is unseen: the badges show.
    static PENDING: Cell<bool> = const { Cell::new(false) };
    /// Every badge of every window; one whose widget is gone is dropped.
    static BADGES: RefCell<Vec<Badge>> = RefCell::default();
}

/// At each window's start: saves the first seen version if the state file has none, then
/// turns the badges on while a flagged release is unseen. Nothing pops up.
pub(crate) fn start() {
    let path = state_file();
    let existed = path.exists();
    let mut saved = state::load(&path);
    let seen = match saved.last_seen_whats_new.clone() {
        Some(seen) => seen,
        None => {
            let seen = first_seen(existed).to_string();
            saved.last_seen_whats_new = Some(seen.clone());
            if let Err(e) = state::store(&path, &saved) {
                eprintln!("markdownmate: cannot save session state: {e}");
            }
            seen
        }
    };
    PENDING.set(!unseen(releases(), &seen, VERSION).is_empty());
}

/// Saves this version as seen and hides the badges in every window.
fn mark_seen() {
    let path = state_file();
    let mut saved = state::load(&path);
    if saved.last_seen_whats_new.as_deref() != Some(VERSION) {
        saved.last_seen_whats_new = Some(VERSION.to_string());
        if let Err(e) = state::store(&path, &saved) {
            eprintln!("markdownmate: cannot save session state: {e}");
        }
    }
    PENDING.set(false);
    BADGES.with_borrow_mut(|badges| badges.retain(|badge| badge(false)));
}

fn on_badge(badge: impl Fn(bool) -> bool + 'static) {
    if badge(PENDING.get()) {
        BADGES.with_borrow_mut(|badges| badges.push(Box::new(badge)));
    }
}

/// A small accent dot (class `badge-dot`, `main.rs`), hidden until a badge shows it.
fn badge_dot(halign: gtk::Align, valign: gtk::Align) -> gtk::Box {
    let dot = gtk::Box::builder()
        .css_classes(["badge-dot"])
        .halign(halign)
        .valign(valign)
        .visible(false)
        .build();
    let weak = dot.downgrade();
    on_badge(move |on| weak.upgrade().map(|d| d.set_visible(on)).is_some());
    dot
}

/// The main menu button's icon, with the What's New dot at its top right.
pub(crate) fn badge_menu_button(button: &gtk::MenuButton) {
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&gtk::Image::from_icon_name("open-menu-symbolic")));
    overlay.add_overlay(&badge_dot(gtk::Align::End, gtk::Align::Start));
    button.set_child(Some(&overlay));
    button.add_css_class("image-button");
}

fn menu_label(pending: bool) -> &'static str {
    if pending {
        "What's New •"
    } else {
        "What's New"
    }
}

/// The main menu's What's New item (in `section`) gets a dot after its label while pending.
pub(crate) fn badge_menu(section: &gio::Menu) {
    let weak = section.downgrade();
    on_badge(move |on| {
        let Some(menu) = weak.upgrade() else {
            return false;
        };
        let action = |i| {
            menu.item_attribute_value(i, "action", Some(glib::VariantTy::STRING))
                .and_then(|v| v.get::<String>())
        };
        if let Some(i) =
            (0..menu.n_items()).find(|&i| action(i).as_deref() == Some(WHATS_NEW_ACTION))
        {
            menu.remove(i);
            menu.insert(i, Some(menu_label(on)), Some(WHATS_NEW_ACTION));
        }
        true
    });
}

/// What's New (main menu, Preferences ▸ About): the notes of every flagged release since the
/// one last seen, or of the newest one, and a link to the full changelog. Opening it marks
/// this version as seen, which clears the badges. Esc closes it.
pub(crate) fn whats_new_dialog(editor: &Rc<Editor>) {
    let last_seen = state::load(&state_file())
        .last_seen_whats_new
        .unwrap_or_else(|| VERSION.to_string());
    let shown = to_show(releases(), &last_seen, VERSION);
    mark_seen();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(18)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    for release in shown {
        let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
        section.append(
            &gtk::Label::builder()
                .label(format!("Version {}", release.version))
                .css_classes(["title-4"])
                .xalign(0.0)
                .build(),
        );
        for note in &release.notes {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            line.append(
                &gtk::Label::builder()
                    .label("•")
                    .valign(gtk::Align::Start)
                    .build(),
            );
            line.append(
                &gtk::Label::builder()
                    .label(note.as_str())
                    .wrap(true)
                    .xalign(0.0)
                    .hexpand(true)
                    .build(),
            );
            section.append(&line);
        }
        content.append(&section);
    }
    let toasts = adw::ToastOverlay::new();
    let changelog = gtk::Button::builder()
        .label("Full Changelog")
        .halign(gtk::Align::Start)
        .css_classes(["pill"])
        .build();
    changelog.connect_clicked(clone!(
        #[weak]
        toasts,
        move |button| open_uri(button, CHANGELOG_URL, CHANGELOG_URL, move |m| {
            toasts.add_toast(adw::Toast::new(m))
        })
    ));
    content.append(&changelog);

    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(480)
        .child(&content)
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scrolled));
    toasts.set_child(Some(&view));
    adw::Dialog::builder()
        .title("What's New")
        .content_width(480)
        .child(&toasts)
        .build()
        .present(Some(&editor.window));
}

fn copy_text(widget: &impl IsA<gtk::Widget>, text: &str) {
    widget.clipboard().set_text(text);
}

/// Opens `uri` in the system browser or mail app. If none can, `copy` goes to the clipboard
/// and `toast` says so; a chooser the user dismisses is no failure.
fn open_uri(widget: &impl IsA<gtk::Widget>, uri: &str, copy: &str, toast: impl Fn(&str) + 'static) {
    let window = widget.root().and_downcast::<gtk::Window>();
    let widget = widget.clone().upcast::<gtk::Widget>();
    let copy = copy.to_string();
    gtk::UriLauncher::new(uri).launch(window.as_ref(), gio::Cancellable::NONE, move |result| {
        if let Err(e) = result
            && !e.matches(gtk::DialogError::Dismissed)
            && !e.matches(gio::IOErrorEnum::Cancelled)
        {
            copy_text(&widget, &copy);
            toast(&format!(
                "No app could open it: {copy} copied to the clipboard"
            ));
        }
    });
}

/// Percent-encodes everything but RFC 3986's unreserved characters (for a mailto query).
fn encode(text: &str) -> String {
    let mut out = String::new();
    for &b in text.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out += &format!("%{b:02X}");
        }
    }
    out
}

/// PRETTY_NAME of an os-release file, unquoted.
fn pretty_name(os_release: &str) -> Option<String> {
    let value = os_release
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))?
        .trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value);
    (!value.is_empty()).then(|| value.to_string())
}

/// The Send Feedback link: subject with the version, body with room to write above the
/// app version, OS and kernel.
fn mailto(version: &str, os: &str, kernel: &str) -> String {
    let subject = format!("[MarkdownMate {version}] Feedback");
    let body = format!("\r\n\r\n---\r\nApp version: {version}\r\nOS: {os}\r\nKernel: {kernel}\r\n");
    format!(
        "mailto:{FEEDBACK_EMAIL}?subject={}&body={}",
        encode(&subject),
        encode(&body)
    )
}

/// `mailto` for this machine (`/etc/os-release`, `/proc/sys/kernel/osrelease`).
pub(crate) fn feedback_uri() -> String {
    let os = std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|t| pretty_name(&t))
        .unwrap_or_else(|| "Linux".to_string());
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|k| k.trim().to_string())
        .unwrap_or_default();
    mailto(VERSION, &os, &kernel)
}

/// The About dialog's release notes (AppStream markup) and their version.
pub(crate) fn release_notes() -> Option<(String, String)> {
    let release = latest(releases(), VERSION)?;
    let items: String = release
        .notes
        .iter()
        .map(|n| format!("<li>{}</li>", glib::markup_escape_text(n)))
        .collect();
    Some((release.version.clone(), format!("<ul>{items}</ul>")))
}

/// An activatable row with a suffix icon.
fn link_row(title: &str, subtitle: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .activatable(true)
        .build();
    row.add_suffix(&gtk::Image::from_icon_name(icon));
    row
}

/// Preferences ▸ About: icon, name and version (click copies it), then What's New, Send
/// Feedback, Share MarkdownMate, Donate, More Apps from HomieLab and the MarkdownMate website.
/// Toasts show in `dialog`.
pub(crate) fn about_page(
    editor: &Rc<Editor>,
    dialog: &adw::PreferencesDialog,
) -> adw::PreferencesPage {
    let toast = {
        let dialog = dialog.downgrade();
        move |msg: &str| {
            if let Some(d) = dialog.upgrade() {
                d.add_toast(adw::Toast::new(msg));
            }
        }
    };

    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    header.append(
        &gtk::Image::builder()
            .icon_name(APP_ID)
            .pixel_size(96)
            .css_classes(["icon-dropshadow"])
            .margin_bottom(6)
            .build(),
    );
    header.append(
        &gtk::Label::builder()
            .label("MarkdownMate")
            .css_classes(["title-1"])
            .build(),
    );
    let version = gtk::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .label(format!("Version {VERSION}"))
                .icon_name("edit-copy-symbolic")
                .build(),
        )
        .tooltip_text("Copy Version")
        .halign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();
    version.connect_clicked(clone!(
        #[strong]
        toast,
        move |b| {
            copy_text(b, VERSION);
            toast("Version copied");
        }
    ));
    header.append(&version);
    let top = adw::PreferencesGroup::new();
    top.add(&header);

    let whats_new = adw::ActionRow::builder()
        .title("What's New")
        .subtitle("The highlights of this version")
        .activatable(true)
        .build();
    whats_new.add_suffix(&badge_dot(gtk::Align::Center, gtk::Align::Center));
    whats_new.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    whats_new.connect_activated(clone!(
        #[strong]
        editor,
        move |_| whats_new_dialog(&editor)
    ));

    let external = "adw-external-link-symbolic";
    let feedback = link_row("Send Feedback", FEEDBACK_EMAIL, external);
    feedback.connect_activated(clone!(
        #[strong]
        toast,
        move |row| open_uri(row, &feedback_uri(), FEEDBACK_EMAIL, toast.clone())
    ));
    let share = link_row(
        "Share MarkdownMate",
        "Copy a link to MarkdownMate to send to a friend",
        "edit-copy-symbolic",
    );
    share.connect_activated(clone!(
        #[strong]
        toast,
        move |row| {
            copy_text(row, SHARE_TEXT);
            toast("Link to MarkdownMate copied");
        }
    ));
    let links = [
        ("Donate", "Support MarkdownMate on Ko-fi", DONATE_URL),
        ("More Apps from HomieLab", "homielab.com", MORE_APPS_URL),
        (
            "MarkdownMate Website",
            "markdownmate.homielab.com",
            WEBSITE_URL,
        ),
    ]
    .map(|(title, subtitle, url)| {
        let row = link_row(title, subtitle, external);
        row.connect_activated(clone!(
            #[strong]
            toast,
            move |row| open_uri(row, url, url, toast.clone())
        ));
        row
    });

    let rows = adw::PreferencesGroup::new();
    rows.add(&whats_new);
    rows.add(&feedback);
    rows.add(&share);
    for row in &links {
        rows.add(row);
    }
    let page = adw::PreferencesPage::builder()
        .title("About")
        .icon_name("help-about-symbolic")
        .build();
    page.add(&top);
    page.add(&rows);
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = r#"
        [[release]]
        version = "1.1.0"
        show = true
        notes = ["Eleven"]

        [[release]]
        version = "1.0.1"
        notes = ["A fix"]

        [[release]]
        version = "1.2.0"
        show = true
        notes = ["Twelve a", "Twelve b"]

        [[release]]
        version = "1.0.0"
        show = true
        notes = ["Ten"]

        [[release]]
        version = "bad"
        show = true
        notes = ["Skipped"]
    "#;

    fn versions(list: &[&Release]) -> Vec<String> {
        list.iter().map(|r| r.version.clone()).collect()
    }

    #[test]
    fn parses_newest_first_and_skips_bad_entries() {
        let all = parse_releases(DATA);
        let got: Vec<&str> = all.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(got, ["1.2.0", "1.1.0", "1.0.1", "1.0.0"]);
        assert_eq!(all[0].notes, ["Twelve a", "Twelve b"]);
        assert!(!all[2].show);
        assert!(parse_releases("not = [toml").is_empty());
    }

    #[test]
    fn parse_version_needs_three_numbers() {
        assert_eq!(parse_version("0.8.0"), Some((0, 8, 0)));
        assert_eq!(parse_version("10.2.33"), Some((10, 2, 33)));
        assert_eq!(parse_version("0.8"), None);
        assert_eq!(parse_version("0.8.0.1"), None);
        assert_eq!(parse_version("0.8.x"), None);
    }

    #[test]
    fn fresh_install_shows_nothing() {
        let all = parse_releases(DATA);
        // No state file: this version counts as seen.
        assert!(unseen(releases(), first_seen(false), VERSION).is_empty());
        assert!(unseen(&all, "1.2.0", "1.2.0").is_empty());
        // A state file from before What's New: the releases since then are new.
        assert_eq!(first_seen(true), BEFORE_WHATS_NEW);
    }

    #[test]
    fn skipped_versions_merge_newest_first() {
        let all = parse_releases(DATA);
        assert_eq!(
            versions(&unseen(&all, "1.0.0", "1.2.0")),
            ["1.2.0", "1.1.0"]
        );
        assert_eq!(
            versions(&to_show(&all, "1.0.0", "1.2.0")),
            ["1.2.0", "1.1.0"]
        );
        // Nothing newer than the running version.
        assert_eq!(versions(&unseen(&all, "1.0.0", "1.1.0")), ["1.1.0"]);
    }

    #[test]
    fn patch_release_is_not_flagged() {
        let all = parse_releases(DATA);
        assert!(unseen(&all, "1.0.0", "1.0.1").is_empty());
        // Opened by hand: the newest flagged release up to this version.
        assert_eq!(versions(&to_show(&all, "1.0.1", "1.0.1")), ["1.0.0"]);
        // The About dialog shows the patch's own notes.
        assert_eq!(latest(&all, "1.0.1").unwrap().version, "1.0.1");
    }

    #[test]
    fn seen_or_unreadable_versions_show_no_badge() {
        let all = parse_releases(DATA);
        assert!(unseen(&all, "1.2.0", "1.2.0").is_empty());
        assert!(unseen(&all, "2.0.0", "1.2.0").is_empty());
        assert!(unseen(&all, "garbage", "1.2.0").is_empty());
    }

    #[test]
    fn bundled_notes_cover_this_feature_release() {
        let all = releases();
        assert!(!all.is_empty());
        for release in all {
            assert!(
                (1..=5).contains(&release.notes.len()),
                "{}",
                release.version
            );
        }
        let now = parse_version(VERSION).unwrap();
        if now.2 == 0 {
            assert!(
                all.iter().any(|r| r.version == VERSION && r.show),
                "resources/whats-new.toml needs a flagged {VERSION} entry"
            );
        }
        assert!(latest(all, VERSION).is_some());
    }

    #[test]
    fn mailto_is_encoded() {
        let uri = mailto("0.8.0", "Ubuntu 24.04.3 LTS", "6.8.0-45-generic");
        assert_eq!(
            uri,
            "mailto:markdownmate@homielab.com?subject=%5BMarkdownMate%200.8.0%5D%20Feedback\
             &body=%0D%0A%0D%0A---%0D%0AApp%20version%3A%200.8.0%0D%0AOS%3A%20Ubuntu%2024.04.3%20LTS\
             %0D%0AKernel%3A%206.8.0-45-generic%0D%0A"
        );
        assert_eq!(encode("a&b=c?d#é"), "a%26b%3Dc%3Fd%23%C3%A9");
    }

    #[test]
    fn pretty_name_unquotes() {
        let text = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\nID=ubuntu\n";
        assert_eq!(pretty_name(text).as_deref(), Some("Ubuntu 24.04.3 LTS"));
        assert_eq!(pretty_name("PRETTY_NAME=Debian").as_deref(), Some("Debian"));
        assert_eq!(
            pretty_name("PRETTY_NAME='Fedora 41'").as_deref(),
            Some("Fedora 41")
        );
        assert_eq!(pretty_name("NAME=x\n"), None);
    }
}
