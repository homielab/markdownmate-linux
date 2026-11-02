//! The daily note name (Preferences ▸ Notes ▸ Daily Note Name): a pattern with the date tokens
//! of the templates' `{{date:FORMAT}}` (`YYYY YY MMMM MMM MM M DD D dddd ddd`, `[…]` kept as
//! text) and `/` for subfolders of the daily notes folder, `YYYY-MM-DD` by default. `format`
//! turns a date into the note's path relative to the daily notes folder (without `.md`);
//! `parse` reads such a path back, so every daily-note command (Today's Note, Previous / Next,
//! Daily Note for Date…) finds notes kept as `2026/11/2026-11-02.md` or
//! `2026-11-02 Monday.md`. Pure Rust (no GTK) so it is testable headless.

use regex::Regex;

use crate::templates::{self, Components};

/// The pattern a new state file starts with: `2026-11-02.md` directly in the folder.
pub const DEFAULT: &str = "YYYY-MM-DD";

const NEEDS_DATE: &str =
    "Needs a year (YYYY or YY), a month (MMMM, MMM, MM or M) and a day (DD or D)";

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Literal,
    Year4,
    Year2,
    MonthName,
    MonthAbbr,
    Month2,
    Month1,
    Day2,
    Day1,
    Weekday,
    WeekdayAbbr,
}

/// Tokens in the order `templates::format` tries them (longest first); None = a time token,
/// which has no place in a daily note's name.
const TOKENS: [(&str, Option<Kind>); 18] = [
    ("YYYY", Some(Kind::Year4)),
    ("MMMM", Some(Kind::MonthName)),
    ("dddd", Some(Kind::Weekday)),
    ("MMM", Some(Kind::MonthAbbr)),
    ("ddd", Some(Kind::WeekdayAbbr)),
    ("YY", Some(Kind::Year2)),
    ("MM", Some(Kind::Month2)),
    ("DD", Some(Kind::Day2)),
    ("HH", None),
    ("hh", None),
    ("mm", None),
    ("ss", None),
    ("M", Some(Kind::Month1)),
    ("D", Some(Kind::Day1)),
    ("H", None),
    ("h", None),
    ("A", None),
    ("a", None),
];

#[derive(Debug)]
struct Token {
    kind: Kind,
    /// The token as written (`MM`), or the literal text.
    text: String,
}

/// A valid daily note name pattern, compiled for `parse`.
#[derive(Debug)]
pub struct Pattern {
    text: String,
    tokens: Vec<Token>,
    regex: Regex,
    depth: usize,
}

impl Pattern {
    /// Checks `setting` (trimmed): it needs a year, a month and a day token, no other letters
    /// outside `[…]`, no leading `/`, no empty, `.` or `..` part and no NUL. Err = the problem,
    /// as Preferences shows it under the field.
    pub fn new(setting: &str) -> Result<Self, String> {
        let text = setting.trim();
        if text.contains('\0') {
            return Err("Contains a character file names can't hold".into());
        }
        if text.starts_with('/') {
            return Err("Can't start with /: the name is inside the daily notes folder".into());
        }
        let tokens = tokenize(text)?;
        let has = |kinds: &[Kind]| tokens.iter().any(|t| kinds.contains(&t.kind));
        if !has(&[Kind::Year4, Kind::Year2])
            || !has(&[Kind::MonthName, Kind::MonthAbbr, Kind::Month2, Kind::Month1])
            || !has(&[Kind::Day2, Kind::Day1])
        {
            return Err(NEEDS_DATE.into());
        }
        let sample = templates::format(text, &Components::from_ymd(2026, 11, 2));
        let parts: Vec<&str> = sample.split('/').collect();
        if parts.iter().any(|p| p.is_empty()) {
            return Err("Has an empty folder name (// or a / at the end)".into());
        }
        if parts.iter().any(|p| *p == "." || *p == "..") {
            return Err("Can't use . or .. as a folder or file name".into());
        }
        let regex = Regex::new(&regex_source(&tokens)).map_err(|e| e.to_string())?;
        Ok(Self {
            text: text.to_string(),
            tokens,
            regex,
            depth: parts.len(),
        })
    }

    /// The pattern of the `daily_name` setting; an invalid one (edited by hand) = the default.
    pub fn from_setting(setting: &str) -> Self {
        Self::new(setting)
            .or_else(|_| Self::new(DEFAULT))
            .expect("the default pattern is valid")
    }

    /// The pattern as stored (trimmed).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// How many path parts a name has (1 = directly in the daily notes folder).
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// The daily note of `date` relative to the daily notes folder, without `.md`, `/`
    /// between folders (the templates' formatter; the weekday comes from `date`).
    pub fn format(&self, date: &Components) -> String {
        templates::format(&self.text, date)
    }

    /// The date of `relative` (a path relative to the daily notes folder, `/` between parts,
    /// `.md` in any case) when it is a daily note of this pattern: a real date whose name
    /// `format` gives back, ignoring case and the weekday's name (a wrong weekday still
    /// counts). Anything else (stray notes, `2026-11-2.md` for `YYYY-MM-DD`) = None.
    pub fn parse(&self, relative: &str) -> Option<(i32, u32, u32)> {
        let cut = relative.len().checked_sub(3)?;
        if !relative.get(cut..)?.eq_ignore_ascii_case(".md") {
            return None;
        }
        let caps = self.regex.captures(&relative[..cut])?;
        let (mut year, mut month, mut day) = (None, None, None);
        let fields = self.tokens.iter().filter(|t| t.kind != Kind::Literal);
        for (i, token) in fields.clone().enumerate() {
            let s = caps.get(i + 1)?.as_str();
            let (slot, value) = match token.kind {
                Kind::Year4 => (&mut year, s.parse().ok()?),
                Kind::Year2 => (&mut year, 2000 + s.parse::<i32>().ok()?),
                Kind::MonthName => (&mut month, month_number(s, templates::month_name)?),
                Kind::MonthAbbr => (&mut month, month_number(s, templates::month_abbr)?),
                Kind::Month2 | Kind::Month1 => (&mut month, s.parse().ok()?),
                Kind::Day2 | Kind::Day1 => (&mut day, s.parse().ok()?),
                _ => continue,
            };
            match slot {
                Some(v) if *v != value => return None,
                _ => *slot = Some(value),
            }
        }
        let (year, month, day) = (year?, u32::try_from(month?).ok()?, day?);
        let day = u32::try_from(day).ok()?;
        if !(1..=last_day(year, month)?).contains(&day) {
            return None;
        }
        let date = Components::from_ymd(year, month, day);
        for (i, token) in fields.enumerate() {
            if matches!(token.kind, Kind::Weekday | Kind::WeekdayAbbr) {
                continue;
            }
            let written = caps.get(i + 1)?.as_str();
            if !written.eq_ignore_ascii_case(&templates::format(&token.text, &date)) {
                return None;
            }
        }
        Some((year, month, day))
    }
}

/// Splits `pattern` like `templates::format` reads it; a time token or a letter that is no
/// token outside `[…]` is an error (it would not give the same name every time, or is a
/// moment token like `WW` the formatter does not know).
fn tokenize(pattern: &str) -> Result<Vec<Token>, String> {
    let mut tokens: Vec<Token> = Vec::new();
    let literal = |tokens: &mut Vec<Token>, s: &str| match tokens.last_mut() {
        Some(t) if t.kind == Kind::Literal => t.text.push_str(s),
        _ => tokens.push(Token {
            kind: Kind::Literal,
            text: s.to_string(),
        }),
    };
    let not_a_date = |t: &str| format!("“{t}” is not a date token: put other text in [brackets]");
    let mut rest = pattern;
    while !rest.is_empty() {
        if let Some(inside) = rest.strip_prefix('[') {
            match inside.find(']') {
                Some(close) => {
                    literal(&mut tokens, &inside[..close]);
                    rest = &inside[close + 1..];
                }
                None => {
                    literal(&mut tokens, inside);
                    rest = "";
                }
            }
            continue;
        }
        if let Some((text, kind)) = TOKENS.iter().find(|(t, _)| rest.starts_with(t)) {
            let kind = kind.ok_or_else(|| not_a_date(text))?;
            tokens.push(Token {
                kind,
                text: (*text).to_string(),
            });
            rest = &rest[text.len()..];
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        if c.is_ascii_alphabetic() {
            return Err(not_a_date(&c.to_string()));
        }
        literal(&mut tokens, &rest[..c.len_utf8()]);
        rest = &rest[c.len_utf8()..];
    }
    Ok(tokens)
}

/// The whole-name, case-insensitive regex of `tokens`: one group per date token.
fn regex_source(tokens: &[Token]) -> String {
    let names = |f: fn(u32) -> &'static str| {
        // 1…12: months; weekdays give "" after 7.
        let all: Vec<&str> = (1..=12).map(f).filter(|n| !n.is_empty()).collect();
        format!("({})", all.join("|"))
    };
    let mut source = String::from("(?i)^");
    for token in tokens {
        match token.kind {
            Kind::Literal => source.push_str(&regex::escape(&token.text)),
            Kind::Year4 => source.push_str("([0-9]{4})"),
            Kind::Year2 | Kind::Month2 | Kind::Day2 => source.push_str("([0-9]{2})"),
            Kind::Month1 | Kind::Day1 => source.push_str("([0-9]{1,2})"),
            Kind::MonthName => source.push_str(&names(templates::month_name)),
            Kind::MonthAbbr => source.push_str(&names(templates::month_abbr)),
            Kind::Weekday => source.push_str(&names(templates::weekday_name)),
            Kind::WeekdayAbbr => source.push_str(&names(templates::weekday_abbr)),
        }
    }
    source.push('$');
    source
}

fn month_number(name: &str, names: fn(u32) -> &'static str) -> Option<i32> {
    (1..=12)
        .find(|&m| names(m).eq_ignore_ascii_case(name))
        .map(|m| m as i32)
}

/// The number of days in `year`-`month` (Gregorian); None for a month out of 1…12.
fn last_day(year: i32, month: u32) -> Option<u32> {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 if leap => Some(29),
        2 => Some(28),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(p: &str) -> Pattern {
        Pattern::new(p).unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> Components {
        Components::from_ymd(y, m, d)
    }

    #[test]
    fn formats_and_parses_back() {
        for (p, name) in [
            ("YYYY-MM-DD", "2026-11-02"),
            ("YYYY/MM/YYYY-MM-DD", "2026/11/2026-11-02"),
            ("YYYY/MMMM/DD", "2026/November/02"),
            ("YYYY-MM-DD dddd", "2026-11-02 Monday"),
            ("[Journal] YYYY-MM-DD", "Journal 2026-11-02"),
            ("YY.M.D ddd", "26.11.2 Mon"),
            ("MMM D, YYYY", "Nov 2, 2026"),
        ] {
            let p = pattern(p);
            assert_eq!(p.format(&date(2026, 11, 2)), name, "{}", p.text());
            assert_eq!(
                p.parse(&format!("{name}.md")),
                Some((2026, 11, 2)),
                "{}",
                p.text()
            );
        }
        assert_eq!(pattern("YYYY/MM/YYYY-MM-DD").depth(), 3);
        assert_eq!(pattern(" YYYY-MM-DD ").text(), "YYYY-MM-DD");
        assert_eq!(pattern(DEFAULT).depth(), 1);
    }

    #[test]
    fn parse_ignores_case_and_the_weekday_name() {
        let p = pattern("[Journal] YYYY/MMMM/DD dddd");
        assert_eq!(
            p.parse("journal 2026/NOVEMBER/02 monday.MD"),
            Some((2026, 11, 2))
        );
        // 2026-11-03 is a Tuesday: the date counts, the name is not checked.
        assert_eq!(
            p.parse("Journal 2026/November/03 Monday.md"),
            Some((2026, 11, 3))
        );
        assert_eq!(p.parse("Journal 2026/November/03 Someday.md"), None);
    }

    #[test]
    fn stray_names_do_not_parse() {
        let p = pattern(DEFAULT);
        assert_eq!(p.parse("2026-11-01.md"), Some((2026, 11, 1)));
        assert_eq!(p.parse("2028-02-29.md"), Some((2028, 2, 29)));
        for bad in [
            "2026-02-29.md",
            "2026-13-01.md",
            "2026-11-1.md",
            "2026-11-01.txt",
            "2026-11-01.markdown",
            "x.md",
            "Template.md",
            "2026-11-01 b.md",
            "٢٠٢٦-11-01.md",
            "a/2026-11-01.md",
            ".md",
            "",
        ] {
            assert_eq!(p.parse(bad), None, "{bad}");
        }
        let nested = pattern("YYYY/MM/YYYY-MM-DD");
        assert_eq!(nested.parse("2026-11-02.md"), None);
        // Folders and name disagree.
        assert_eq!(nested.parse("2026/10/2026-11-02.md"), None);
        assert_eq!(nested.parse("2026/11/2026-11-02.md"), Some((2026, 11, 2)));
        // M / D must be written without a leading zero, as `format` writes them.
        let short = pattern("YYYY-M-D");
        assert_eq!(short.parse("2026-1-2.md"), Some((2026, 1, 2)));
        assert_eq!(short.parse("2026-01-02.md"), None);
        // YY = 2000…2099.
        assert_eq!(
            pattern("YY-MM-DD").parse("99-12-31.md"),
            Some((2099, 12, 31))
        );
    }

    #[test]
    fn invalid_patterns_are_refused() {
        for (p, why) in [
            ("", NEEDS_DATE),
            ("   ", NEEDS_DATE),
            ("YYYY-MM", NEEDS_DATE),
            (
                "YYYY/[Week] WW",
                "“W” is not a date token: put other text in [brackets]",
            ),
            (
                "YYYY-MM-DD WW",
                "“W” is not a date token: put other text in [brackets]",
            ),
            (
                "YYYY-MM-DD HH",
                "“HH” is not a date token: put other text in [brackets]",
            ),
            (
                "Daily YYYY-MM-DD",
                "“a” is not a date token: put other text in [brackets]",
            ),
            ("[MM-DD] YYYY", NEEDS_DATE),
            (
                "YYYY/../MM-DD",
                "Can't use . or .. as a folder or file name",
            ),
            (
                "YYYY/[..]/MM-DD",
                "Can't use . or .. as a folder or file name",
            ),
            (
                "YYYY//MM-DD",
                "Has an empty folder name (// or a / at the end)",
            ),
            (
                "YYYY-MM-DD/",
                "Has an empty folder name (// or a / at the end)",
            ),
            (
                "/YYYY-MM-DD",
                "Can't start with /: the name is inside the daily notes folder",
            ),
            ("YYYY-MM-DD\0", "Contains a character file names can't hold"),
        ] {
            assert_eq!(Pattern::new(p).unwrap_err(), why, "{p:?}");
        }
        assert_eq!(Pattern::from_setting("YYYY-MM").text(), DEFAULT);
        assert_eq!(Pattern::from_setting("YYYY/MM/DD").text(), "YYYY/MM/DD");
    }
}
