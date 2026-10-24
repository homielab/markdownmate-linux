//! Phím định dạng Ctrl+B / Ctrl+I / Ctrl+K, dán URL thành link, gõ ký tự bọc vùng chọn:
//! văn bản + vùng chọn → các thay đổi cần làm.
//! Logic thuần (không GTK); mọi vị trí tính bằng ký tự (char) như offset của GtkTextBuffer.

/// Hai thay đổi `(start, end, chèn)` trên văn bản cũ: dấu mở trước chữ và dấu đóng sau chữ
/// (`before.1 <= after.0`); chữ ở giữa giữ nguyên để vùng chọn GTK không bị xoá rỗng (rỗng
/// một lần là GTK mất vùng chọn PRIMARY và bỏ chọn sau đó). `select` tính trên văn bản mới.
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub before: (usize, usize, String),
    pub after: (usize, usize, String),
    pub select: (usize, usize),
}

#[derive(Clone, Copy)]
pub enum Style {
    Bold,
    Italic,
}

/// Bọc/bỏ bọc `**` (Bold) hoặc `*` (Italic). Số `*` quanh chữ là mức nhấn: 1 nghiêng, 2 đậm,
/// 3 cả hai; nên Ctrl+I trên `**đậm**` thành `***đậm***` chứ không bỏ một `*` của chữ đậm.
/// Không có vùng chọn: dùng từ tại con trỏ; không có từ: chèn `****` / `**`, con trỏ ở giữa.
pub fn emphasis(text: &str, sel: (usize, usize), style: Style) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let (mut s, mut e) = (sel.0.min(sel.1), sel.0.max(sel.1).min(chars.len()));
    s = s.min(e);
    if s == e {
        let word = |c: &char| c.is_alphanumeric() || *c == '_';
        while s > 0 && word(&chars[s - 1]) {
            s -= 1;
        }
        while e < chars.len() && word(&chars[e]) {
            e += 1;
        }
    }
    let stars = |it: &mut dyn Iterator<Item = &char>| it.take_while(|c| **c == '*').count();
    let inner = &chars[s..e];
    let inside = stars(&mut inner.iter())
        .min(stars(&mut inner.iter().rev()))
        .min(3);
    // Mức nhấn nằm trong vùng chọn (`**x**` được chọn cả dấu) hoặc ngay bên ngoài nó.
    let (start, end, level) = if inside > 0 && inside * 2 < inner.len() {
        (s, e, inside)
    } else {
        let level = stars(&mut chars[..s].iter().rev())
            .min(stars(&mut chars[e..].iter()))
            .min(3);
        (s - level, e + level, level)
    };
    let len = end - start - 2 * level;
    let has = match style {
        Style::Bold => level >= 2,
        Style::Italic => level % 2 == 1,
    };
    let step = match style {
        Style::Bold => 2,
        Style::Italic => 1,
    };
    let new = if has { level - step } else { level + step };
    let marks = "*".repeat(new);
    Edit {
        before: (start, start + level, marks.clone()),
        after: (end - level, end, marks),
        select: (start + new, start + new + len),
    }
}

/// Vùng chọn → `[text](url)` với `url` được chọn; không có vùng chọn → `[](url)`, con trỏ trong `[]`.
pub fn link(text: &str, sel: (usize, usize)) -> Edit {
    let e = sel.0.max(sel.1).min(text.chars().count());
    let s = sel.0.min(sel.1).min(e);
    let len = e - s;
    let select = if len == 0 {
        (s + 1, s + 1)
    } else {
        (s + len + 3, s + len + 6)
    };
    Edit {
        before: (s, s, "[".into()),
        after: (e, e, "](url)".into()),
        select,
    }
}

/// Văn bản clipboard là đúng một URL `http://`, `https://`, `mailto:` (bỏ khoảng trắng hai đầu,
/// bên trong không có khoảng trắng) → URL đó.
pub fn url(clip: &str) -> Option<&str> {
    let u = clip.trim();
    let scheme = ["http://", "https://", "mailto:"]
        .into_iter()
        .find(|p| u.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p)))?;
    (u.len() > scheme.len() && !u.contains(char::is_whitespace)).then_some(u)
}

/// Dán `url` lên vùng chọn một dòng → `[chữ](url)`, con trỏ sau link; không vùng chọn hoặc vùng
/// chọn nhiều dòng → `None` (dán thường).
pub fn paste_link(text: &str, sel: (usize, usize), url: &str) -> Option<Edit> {
    let chars: Vec<char> = text.chars().collect();
    let e = sel.0.max(sel.1).min(chars.len());
    let s = sel.0.min(sel.1).min(e);
    if s == e || chars[s..e].contains(&'\n') {
        return None;
    }
    let end = e + 4 + url.chars().count();
    Some(Edit {
        before: (s, s, "[".into()),
        after: (e, e, format!("]({url})")),
        select: (end, end),
    })
}

/// Gõ `c` khi có vùng chọn: `*` `_` `` ` `` `~` `"` bọc bằng chính nó, `[` → `[ ]`, `(` → `( )`;
/// vùng chọn vẫn là chữ bên trong. Ký tự khác hoặc không vùng chọn → `None`.
pub fn wrap(sel: (usize, usize), c: char) -> Option<Edit> {
    let close = match c {
        '*' | '_' | '`' | '~' | '"' => c,
        '[' => ']',
        '(' => ')',
        _ => return None,
    };
    let (s, e) = (sel.0.min(sel.1), sel.0.max(sel.1));
    (s < e).then(|| Edit {
        before: (s, s, c.into()),
        after: (e, e, close.into()),
        select: (s + 1, e + 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Áp `Edit` lên văn bản, trả văn bản mới + phần đang chọn.
    fn apply(
        text: &str,
        sel: (usize, usize),
        f: impl Fn(&str, (usize, usize)) -> Edit,
    ) -> (String, String) {
        let edit = f(text, sel);
        let chars: Vec<char> = text.chars().collect();
        let part = |a: usize, b: usize| chars[a..b].iter().collect::<String>();
        let (b, a) = (&edit.before, &edit.after);
        let new = part(0, b.0) + &b.2 + &part(b.1, a.0) + &a.2 + &part(a.1, chars.len());
        let picked = new
            .chars()
            .skip(edit.select.0)
            .take(edit.select.1 - edit.select.0)
            .collect();
        (new, picked)
    }

    fn bold(text: &str, sel: (usize, usize)) -> (String, String) {
        apply(text, sel, |t, s| emphasis(t, s, Style::Bold))
    }

    fn italic(text: &str, sel: (usize, usize)) -> (String, String) {
        apply(text, sel, |t, s| emphasis(t, s, Style::Italic))
    }

    #[test]
    fn wraps_selection() {
        assert_eq!(
            bold("a chữ đậm b", (2, 9)),
            ("a **chữ đậm** b".into(), "chữ đậm".into())
        );
        assert_eq!(
            italic("a chữ b", (2, 5)),
            ("a *chữ* b".into(), "chữ".into())
        );
    }

    #[test]
    fn unwraps_selection_inside_or_outside_markers() {
        // Chọn chữ bên trong dấu.
        assert_eq!(
            bold("a **chữ đậm** b", (4, 11)),
            ("a chữ đậm b".into(), "chữ đậm".into())
        );
        // Chọn cả dấu.
        assert_eq!(
            bold("a **chữ đậm** b", (2, 13)),
            ("a chữ đậm b".into(), "chữ đậm".into())
        );
        assert_eq!(italic("*ý*", (0, 3)), ("ý".into(), "ý".into()));
    }

    #[test]
    fn word_at_cursor() {
        assert_eq!(
            bold("xin chào bạn", (6, 6)),
            ("xin **chào** bạn".into(), "chào".into())
        );
        // Con trỏ cuối từ, và trong từ đã đậm → bỏ đậm.
        assert_eq!(
            bold("xin chào", (8, 8)),
            ("xin **chào**".into(), "chào".into())
        );
        assert_eq!(
            bold("a **đậm** b", (5, 5)),
            ("a đậm b".into(), "đậm".into())
        );
    }

    #[test]
    fn no_word_inserts_empty_markers() {
        assert_eq!(bold("a  b", (2, 2)), ("a **** b".into(), String::new()));
        assert_eq!(bold("a **** b", (4, 4)).0, "a  b");
        assert_eq!(italic("", (0, 0)), ("**".into(), String::new()));
        let e = emphasis("", (0, 0), Style::Bold);
        assert_eq!(e.select, (2, 2));
    }

    #[test]
    fn italic_and_bold_combine() {
        // Ctrl+I trên chữ đậm thêm nghiêng, không bỏ một `*` của chữ đậm.
        assert_eq!(
            italic("**đậm**", (2, 5)),
            ("***đậm***".into(), "đậm".into())
        );
        assert_eq!(
            italic("***đậm***", (3, 6)),
            ("**đậm**".into(), "đậm".into())
        );
        assert_eq!(bold("***đậm***", (3, 6)), ("*đậm*".into(), "đậm".into()));
        assert_eq!(
            bold("*nghiêng*", (1, 8)),
            ("***nghiêng***".into(), "nghiêng".into())
        );
        assert_eq!(
            italic("**đậm**", (0, 7)),
            ("***đậm***".into(), "đậm".into())
        );
    }

    #[test]
    fn links() {
        assert_eq!(
            apply("xem chữ này", (4, 7), link),
            ("xem [chữ](url) này".into(), "url".into())
        );
        assert_eq!(link("ab", (1, 1)).select, (2, 2));
        assert_eq!(apply("ab", (1, 1), link).0, "a[](url)b");
        // Vùng chọn ngược (con trỏ ở đầu) vẫn đúng.
        assert_eq!(apply("đây", (3, 0), link).0, "[đây](url)");
    }

    #[test]
    fn urls() {
        assert_eq!(
            url("  https://vi.wikipedia.org/wiki/Việt\n"),
            Some("https://vi.wikipedia.org/wiki/Việt")
        );
        assert_eq!(url("HTTP://x"), Some("HTTP://x"));
        assert_eq!(url("mailto:a@b.c"), Some("mailto:a@b.c"));
        for bad in [
            "https://",
            "ftp://x",
            "xem https://x",
            "https://x y",
            "https://x\nhttps://y",
            "",
            "đ",
        ] {
            assert_eq!(url(bad), None, "{bad}");
        }
    }

    #[test]
    fn paste_links() {
        let link = |t: &str, s| paste_link(t, s, "https://x");
        assert_eq!(
            apply("xem chữ này", (4, 7), |t, s| link(t, s).unwrap()),
            ("xem [chữ](https://x) này".into(), String::new())
        );
        assert_eq!(link("xem chữ này", (7, 4)).unwrap().select, (20, 20));
        assert_eq!(link("ab", (1, 1)), None);
        assert_eq!(link("dòng một\nhai", (2, 11)), None);
    }

    #[test]
    fn wraps() {
        let w = |t: &str, s, c| apply(t, s, |_, s| wrap(s, c).unwrap());
        assert_eq!(
            w("a chữ b", (2, 5), '['),
            ("a [chữ] b".into(), "chữ".into())
        );
        assert_eq!(
            w("a chữ b", (5, 2), '('),
            ("a (chữ) b".into(), "chữ".into())
        );
        assert_eq!(
            w("a chữ b", (2, 5), '"'),
            ("a \"chữ\" b".into(), "chữ".into())
        );
        // `*` hai lần → đậm, vùng chọn giữ nguyên chữ.
        let (once, _) = w("đậm", (0, 3), '*');
        assert_eq!(w(&once, (1, 4), '*'), ("**đậm**".into(), "đậm".into()));
        assert_eq!(w("một\nhai", (1, 6), '~').0, "m~ột\nha~i");
        assert_eq!(wrap((1, 1), '*'), None);
        assert_eq!(wrap((0, 2), 'a'), None);
    }
}
