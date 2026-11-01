//! New from Template: the `.md` files of the templates folder (`Prefs::templates_folder`,
//! relative to the open folder like the daily notes folder, `Templates` by default) and the
//! placeholder fill shared with Today's Note (`{{date}}`, `{{date:FORMAT}}`, `{{time}}`,
//! `{{time:FORMAT}}`, `{{title}}`, `{{cursor}}`). Pure Rust (no GTK) so it is testable headless.

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

/// The marker removed from a template's text; the cursor goes where the first one was.
pub const CURSOR_MARKER: &str = "{{cursor}}";

/// A filled-in template: its text and where the cursor goes (a character offset into `text`,
/// as `gtk::TextBuffer::iter_at_offset` takes).
#[derive(Debug, Clone, PartialEq)]
pub struct Filled {
    pub text: String,
    pub cursor: usize,
}

/// Local date and time components used for template formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Components {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub weekday: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Components {
    /// Date as `YYYY-MM-DD`.
    pub fn date_string(&self) -> String {
        crate::daily::date_string(self.year, self.month, self.day)
    }
}

#[cfg(test)]
impl Components {
    /// Constructs components and calculates the Gregorian weekday (1 = Monday .. 7 = Sunday).
    pub fn from_ymd_hms(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> Self {
        let weekday = weekday_of(year, month, day);
        Self {
            year,
            month,
            day,
            weekday,
            hour,
            minute,
            second,
        }
    }
}

/// Calculates the Gregorian weekday: 1 = Monday .. 7 = Sunday (GLib convention).
#[cfg(test)]
pub fn weekday_of(year: i32, month: u32, day: u32) -> u32 {
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = year;
    if month < 3 {
        y -= 1;
    }
    let m_idx = (month.clamp(1, 12) - 1) as usize;
    let w = (y + y / 4 - y / 100 + y / 400 + t[m_idx] + day as i32).rem_euclid(7);
    if w == 0 { 7 } else { w as u32 }
}

/// `HH:MM`, 24-hour and zero-padded.
#[allow(dead_code)]
pub fn time_string(hour: u32, minute: u32) -> String {
    format!("{hour:02}:{minute:02}")
}

/// Formats `components` according to `pattern` using moment-style tokens.
///
/// Tokens (longest match first):
/// - `YYYY`: 4-digit year (e.g. 2026)
/// - `YY`: 2-digit year (e.g. 26)
/// - `MMMM`: Full month name (e.g. November)
/// - `MMM`: Short month name (e.g. Nov)
/// - `MM`: 2-digit month (01..12)
/// - `M`: Month without leading zero (1..12)
/// - `DD`: 2-digit day of month (01..31)
/// - `D`: Day of month without leading zero (1..31)
/// - `dddd`: Full weekday name (e.g. Saturday)
/// - `ddd`: Short weekday name (e.g. Sat)
/// - `HH`: 2-digit 24-hour hour (00..23)
/// - `H`: 24-hour hour without leading zero (0..23)
/// - `hh`: 2-digit 12-hour hour (01..12; 12 at noon and midnight)
/// - `h`: 12-hour hour without leading zero (1..12)
/// - `mm`: 2-digit minute (00..59)
/// - `ss`: 2-digit second (00..59)
/// - `A`: AM or PM
/// - `a`: am or pm
///
/// Text in square brackets `[…]` is preserved literally with the brackets removed.
/// Any other character is treated as a literal.
pub fn format(pattern: &str, components: &Components) -> String {
    let mut result = String::with_capacity(pattern.len() * 2);
    let mut rest = pattern;

    while !rest.is_empty() {
        if rest.starts_with('[') {
            if let Some(close) = rest[1..].find(']') {
                result.push_str(&rest[1..=close]);
                rest = &rest[close + 2..];
            } else {
                result.push_str(&rest[1..]);
                break;
            }
            continue;
        }

        // Longest match first: 4, 3, 2, 1
        if let Some(tail) = rest.strip_prefix("YYYY") {
            result.push_str(&format!("{:04}", components.year));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("MMMM") {
            result.push_str(month_name(components.month));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("dddd") {
            result.push_str(weekday_name(components.weekday));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("MMM") {
            result.push_str(month_abbr(components.month));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("ddd") {
            result.push_str(weekday_abbr(components.weekday));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("YY") {
            result.push_str(&format!("{:02}", components.year.rem_euclid(100)));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("MM") {
            result.push_str(&format!("{:02}", components.month));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("DD") {
            result.push_str(&format!("{:02}", components.day));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("HH") {
            result.push_str(&format!("{:02}", components.hour));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("hh") {
            let h12 = match components.hour % 12 {
                0 => 12,
                h => h,
            };
            result.push_str(&format!("{h12:02}"));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("mm") {
            result.push_str(&format!("{:02}", components.minute));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix("ss") {
            result.push_str(&format!("{:02}", components.second));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix('M') {
            result.push_str(&format!("{}", components.month));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix('D') {
            result.push_str(&format!("{}", components.day));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix('H') {
            result.push_str(&format!("{}", components.hour));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix('h') {
            let h12 = match components.hour % 12 {
                0 => 12,
                h => h,
            };
            result.push_str(&format!("{h12}"));
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix('A') {
            result.push_str(if components.hour < 12 { "AM" } else { "PM" });
            rest = tail;
        } else if let Some(tail) = rest.strip_prefix('a') {
            result.push_str(if components.hour < 12 { "am" } else { "pm" });
            rest = tail;
        } else {
            let c = rest.chars().next().unwrap();
            result.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }

    result
}

fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "",
    }
}

fn month_abbr(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "",
    }
}

fn weekday_name(weekday: u32) -> &'static str {
    match weekday {
        1 => "Monday",
        2 => "Tuesday",
        3 => "Wednesday",
        4 => "Thursday",
        5 => "Friday",
        6 => "Saturday",
        0 | 7 => "Sunday",
        _ => "",
    }
}

fn weekday_abbr(weekday: u32) -> &'static str {
    match weekday {
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        6 => "Sat",
        0 | 7 => "Sun",
        _ => "",
    }
}

fn replace_placeholders(part: &str, components: &Components, title: &str) -> String {
    let mut result = String::with_capacity(part.len());
    let mut rest = part;

    while let Some(start) = rest.find("{{") {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];

        if let Some(tail) = after_open.strip_prefix("title}}") {
            result.push_str(title);
            rest = tail;
        } else if after_open.starts_with("date") || after_open.starts_with("time") {
            let is_date = after_open.starts_with("date");
            let after_tag = &after_open[4..];
            if let Some(end) = after_tag.find("}}") {
                let inside = &after_tag[..end];
                if inside.is_empty() {
                    let default_fmt = if is_date { "YYYY-MM-DD" } else { "HH:mm" };
                    result.push_str(&format(default_fmt, components));
                    rest = &after_tag[end + 2..];
                    continue;
                } else if let Some(format_str) = inside.strip_prefix(':') {
                    let fmt = if format_str.is_empty() {
                        if is_date { "YYYY-MM-DD" } else { "HH:mm" }
                    } else {
                        format_str
                    };
                    result.push_str(&format(fmt, components));
                    rest = &after_tag[end + 2..];
                    continue;
                }
            }
            result.push_str("{{");
            rest = after_open;
        } else {
            result.push_str("{{");
            rest = after_open;
        }
    }
    result.push_str(rest);
    result
}

/// `template` with placeholders replaced:
/// - `{{date}}` / `{{date:}}` → `YYYY-MM-DD`
/// - `{{date:FORMAT}}` → formatted date/time according to `FORMAT`
/// - `{{time}}` / `{{time:}}` → `HH:MM` (24-hour)
/// - `{{time:FORMAT}}` → formatted date/time according to `FORMAT`
/// - `{{title}}` → `title`
/// - `{{cursor}}` → removed; the cursor goes where the first one was, else at the end
///
/// Exact case (`{{ date:… }}` with spaces and other `{{…}}` stay as they are).
pub fn fill(template: &str, components: &Components, title: &str) -> Filled {
    let parts: Vec<String> = template
        .split(CURSOR_MARKER)
        .map(|part| replace_placeholders(part, components, title))
        .collect();
    let text = parts.concat();
    let cursor = if parts.len() > 1 {
        parts[0].chars().count()
    } else {
        text.chars().count()
    };
    Filled { text, cursor }
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

    fn sample_components() -> Components {
        Components::from_ymd_hms(2026, 11, 1, 9, 5, 7)
    }

    fn text(template: &str, title: &str) -> String {
        fill(template, &sample_components(), title).text
    }

    #[test]
    fn fills_date_and_title() {
        assert_eq!(
            text(
                "# {{title}}\n\nDate: {{date}} ({{date}})\n{{Title}} {{other}}\n",
                "Untitled 2"
            ),
            "# Untitled 2\n\nDate: 2026-11-01 (2026-11-01)\n{{Title}} {{other}}\n"
        );
        assert_eq!(text("No placeholders\n", "x"), "No placeholders\n");
        assert_eq!(text("", "x"), "");
    }

    #[test]
    fn time_is_24_hour_and_zero_padded() {
        assert_eq!(time_string(9, 5), "09:05");
        assert_eq!(time_string(0, 0), "00:00");
        assert_eq!(time_string(23, 59), "23:59");
        assert_eq!(
            text("{{time}} at {{time}}, {{Time}} {{ time }}", "x"),
            "09:05 at 09:05, {{Time}} {{ time }}"
        );
    }

    #[test]
    fn contract_examples_and_formatting() {
        let c = sample_components();
        assert_eq!(
            fill("{{date:dddd, MMMM D, YYYY}}", &c, "x").text,
            "Sunday, November 1, 2026"
        );
        assert_eq!(fill("{{time:h:mm A}}", &c, "x").text, "9:05 AM");
        assert_eq!(fill("{{date:[Week] YY}}", &c, "x").text, "Week 26");
        assert_eq!(fill("{{date:}}", &c, "x").text, "2026-11-01");
        assert_eq!(fill("{{time:}}", &c, "x").text, "09:05");
        assert_eq!(fill("{{ date:YYYY }}", &c, "x").text, "{{ date:YYYY }}");
        // Date tag with time format and time tag with date format share token set
        assert_eq!(fill("{{date:HH:mm}}", &c, "x").text, "09:05");
        assert_eq!(fill("{{time:YYYY-MM-DD}}", &c, "x").text, "2026-11-01");
        // Stops at the first }}
        assert_eq!(fill("{{date:YYYY}}tail}}", &c, "x").text, "2026tail}}");
    }

    #[test]
    fn every_token_of_the_contract() {
        let c = sample_components();
        assert_eq!(format("YYYY", &c), "2026");
        assert_eq!(format("YY", &c), "26");
        assert_eq!(format("MMMM", &c), "November");
        assert_eq!(format("MMM", &c), "Nov");
        assert_eq!(format("MM", &c), "11");
        assert_eq!(format("M", &c), "11");
        assert_eq!(format("DD", &c), "01");
        assert_eq!(format("D", &c), "1");
        assert_eq!(format("dddd", &c), "Sunday");
        assert_eq!(format("ddd", &c), "Sun");
        assert_eq!(format("HH", &c), "09");
        assert_eq!(format("H", &c), "9");
        assert_eq!(format("hh", &c), "09");
        assert_eq!(format("h", &c), "9");
        assert_eq!(format("mm", &c), "05");
        assert_eq!(format("ss", &c), "07");
        assert_eq!(format("A", &c), "AM");
        assert_eq!(format("a", &c), "am");

        // Unpadded month and day when single digit
        let march7 = Components {
            year: 2026,
            month: 3,
            day: 7,
            weekday: 6,
            hour: 4,
            minute: 8,
            second: 9,
        };
        assert_eq!(format("YYYY-M-D H:mm:ss", &march7), "2026-3-7 4:08:09");
        // Single m and s are literal because only mm and ss are tokens
        assert_eq!(format("m:s", &march7), "m:s");
        assert_eq!(
            format("YYYY-MM-DD HH:mm:ss", &march7),
            "2026-03-07 04:08:09"
        );
        assert_eq!(format("MMMM (MMM)", &march7), "March (Mar)");

        // All 12 months
        let month_names = [
            ("January", "Jan"),
            ("February", "Feb"),
            ("March", "Mar"),
            ("April", "Apr"),
            ("May", "May"),
            ("June", "Jun"),
            ("July", "Jul"),
            ("August", "Aug"),
            ("September", "Sep"),
            ("October", "Oct"),
            ("November", "Nov"),
            ("December", "Dec"),
        ];
        for (i, (full, abbr)) in month_names.iter().enumerate() {
            let mc = Components {
                year: 2026,
                month: (i + 1) as u32,
                day: 1,
                weekday: 1,
                hour: 0,
                minute: 0,
                second: 0,
            };
            assert_eq!(format("MMMM", &mc), *full);
            assert_eq!(format("MMM", &mc), *abbr);
        }

        // All 7 weekdays
        let weekday_tests = [
            (1, "Monday", "Mon"),
            (2, "Tuesday", "Tue"),
            (3, "Wednesday", "Wed"),
            (4, "Thursday", "Thu"),
            (5, "Friday", "Fri"),
            (6, "Saturday", "Sat"),
            (7, "Sunday", "Sun"),
            (0, "Sunday", "Sun"),
        ];
        for (w, full, abbr) in weekday_tests {
            let wc = Components {
                year: 2026,
                month: 1,
                day: 1,
                weekday: w,
                hour: 0,
                minute: 0,
                second: 0,
            };
            assert_eq!(format("dddd", &wc), full);
            assert_eq!(format("ddd", &wc), abbr);
        }
    }

    #[test]
    fn twelve_hour_midnight_and_noon() {
        let midnight = Components {
            year: 2026,
            month: 11,
            day: 1,
            weekday: 7,
            hour: 0,
            minute: 0,
            second: 0,
        };
        assert_eq!(format("HH H hh h A a", &midnight), "00 0 12 12 AM am");

        let noon = Components {
            year: 2026,
            month: 11,
            day: 1,
            weekday: 7,
            hour: 12,
            minute: 0,
            second: 0,
        };
        assert_eq!(format("HH H hh h A a", &noon), "12 12 12 12 PM pm");

        let morning1 = Components {
            year: 2026,
            month: 11,
            day: 1,
            weekday: 7,
            hour: 1,
            minute: 15,
            second: 0,
        };
        assert_eq!(format("hh:mm A", &morning1), "01:15 AM");
        assert_eq!(format("h:mm A", &morning1), "1:15 AM");

        let afternoon1 = Components {
            year: 2026,
            month: 11,
            day: 1,
            weekday: 7,
            hour: 13,
            minute: 15,
            second: 0,
        };
        assert_eq!(format("hh:mm A", &afternoon1), "01:15 PM");
        assert_eq!(format("h:mm a", &afternoon1), "1:15 pm");
    }

    #[test]
    fn square_brackets_and_literals() {
        let c = sample_components();
        assert_eq!(format("[Week] YY", &c), "Week 26");
        assert_eq!(format("[Today is] dddd", &c), "Today is Sunday");
        assert_eq!(format("[M] M", &c), "M 11");
        assert_eq!(format("[]", &c), "");
        assert_eq!(format("[Escaped: YYYY-MM-DD]", &c), "Escaped: YYYY-MM-DD");
        // Unknown letters stay literal
        assert_eq!(format("YYYY-MM-DDThh:mm:ss", &c), "2026-11-01T09:05:07");
        assert_eq!(format("Unknown X", &c), "Unknown X");
        assert_eq!(format("", &c), "");
        // Non-ASCII literal characters
        assert_eq!(format("YYYY年MM月DD日", &c), "2026年11月01日");
    }

    #[test]
    fn weekday_of_calculation() {
        assert_eq!(weekday_of(2026, 11, 1), 7); // Sunday
        assert_eq!(weekday_of(2026, 10, 31), 6); // Saturday
        assert_eq!(weekday_of(2026, 11, 2), 1); // Monday
        assert_eq!(weekday_of(2028, 2, 29), 2); // Tuesday (leap year)

        let from_ymd = Components::from_ymd_hms(2026, 10, 31, 9, 5, 0);
        assert_eq!(from_ymd.weekday, 6);
        assert_eq!(format("dddd", &from_ymd), "Saturday");
    }

    #[test]
    fn cursor_without_marker_is_at_the_end() {
        let c = sample_components();
        let filled = fill("# {{title}}\n\n", &c, "Café");
        assert_eq!(filled.text, "# Café\n\n");
        assert_eq!(filled.cursor, filled.text.chars().count());
        assert_eq!(fill("", &c, "x").cursor, 0);
    }

    #[test]
    fn cursor_at_the_one_marker() {
        let c = sample_components();
        let filled = fill("# Notes\n\n{{cursor}}\n\n## End\n", &c, "x");
        assert_eq!(filled.text, "# Notes\n\n\n\n## End\n");
        assert_eq!(filled.cursor, 9);
        let filled = fill("{{cursor}}tail", &c, "x");
        assert_eq!((filled.text.as_str(), filled.cursor), ("tail", 0));
        let filled = fill("head{{cursor}}", &c, "x");
        assert_eq!((filled.text.as_str(), filled.cursor), ("head", 4));
    }

    #[test]
    fn cursor_at_the_first_of_several_markers() {
        let c = sample_components();
        let filled = fill("a{{cursor}}b{{cursor}}c{{cursor}}", &c, "x");
        assert_eq!(filled.text, "abc");
        assert_eq!(filled.cursor, 1);
        let filled = fill("{{ cursor }} {{Cursor}} x", &c, "x");
        assert_eq!(filled.text, "{{ cursor }} {{Cursor}} x");
        assert_eq!(filled.cursor, filled.text.chars().count());
    }

    #[test]
    fn cursor_counts_filled_placeholders_in_characters() {
        let c = Components {
            year: 2026,
            month: 11,
            day: 1,
            weekday: 7,
            hour: 14,
            minute: 30,
            second: 0,
        };
        let filled = fill(
            "# {{title}}\n{{date:dddd, MMMM D, YYYY}} {{time:h:mm A}}\n\n{{cursor}}\n\nEnd {{date}}\n",
            &c,
            "Café ☕",
        );
        assert_eq!(
            filled.text,
            "# Café ☕\nSunday, November 1, 2026 2:30 PM\n\n\n\nEnd 2026-11-01\n"
        );
        let before = "# Café ☕\nSunday, November 1, 2026 2:30 PM\n\n";
        assert_eq!(filled.cursor, before.chars().count());
        assert_ne!(filled.cursor, before.len());
    }

    #[test]
    fn a_title_with_the_marker_stays_text() {
        let c = sample_components();
        let filled = fill("# {{title}}\n{{cursor}}", &c, "a {{cursor}} b");
        assert_eq!(filled.text, "# a {{cursor}} b\n");
        assert_eq!(filled.cursor, filled.text.chars().count());
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
