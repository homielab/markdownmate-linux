//! Status bar text: word / character counts and the "Ln X, Col Y · …" line. Pure Rust, tested
//! with `cargo test`.

/// Number of words and characters in `text`. A word is a run of letters/digits (Unicode, so
/// Vietnamese with accents counts as it reads); Markdown markers (`#`, `*`, `-`, `>`, `|`,
/// backticks, `1.` of an ordered list…) are not words. Characters are Unicode scalar values,
/// newlines included.
pub fn count(text: &str) -> (usize, usize) {
    let words = text.lines().map(|l| line_words(strip_ordered(l))).sum();
    (words, text.chars().count())
}

/// Drops a leading ordered-list marker (`12. ` / `3) `) so its number is not a word.
fn strip_ordered(line: &str) -> &str {
    let rest = line.trim_start();
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    match &rest.as_bytes()[digits..] {
        [b'.' | b')', b' ' | b'\t', ..] | [b'.' | b')'] if digits > 0 => &rest[digits + 1..],
        _ => line,
    }
}

fn line_words(line: &str) -> usize {
    let mut words = 0;
    let mut in_word = false;
    for c in line.chars() {
        // A combining mark continues a word (decomposed/NFD "ê" = "e" + U+0302); one alone
        // does not start one.
        let part = c.is_alphanumeric() || (in_word && is_combining(c));
        if part && !in_word {
            words += 1;
        }
        in_word = part;
    }
    words
}

/// shortcut: only the common combining-mark blocks (Latin/Vietnamese diacritics), not all of
/// Unicode category Mn; upgrade with a Unicode table crate if other scripts need it.
pub(crate) fn is_combining(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}' | '\u{1AB0}'..='\u{1AFF}' | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}' | '\u{FE20}'..='\u{FE2F}')
}

/// "Ln 3, Col 7 · 120 words · 640 characters" (1-based line and column).
pub fn status_text(line: usize, col: usize, (words, chars): (usize, usize)) -> String {
    let s = |n: usize| if n == 1 { "" } else { "s" };
    format!(
        "Ln {line}, Col {col} · {words} word{} · {chars} character{}",
        s(words),
        s(chars)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        assert_eq!(count(""), (0, 0));
        assert_eq!(count("  \n\n"), (0, 4));
    }

    #[test]
    fn vietnamese() {
        assert_eq!(count("Tiếng Việt có dấu"), (4, 17));
        // NFD: "Việt" written as "Vie" + U+0323 U+0302 + "t" is still one word.
        assert_eq!(count("Vie\u{0323}\u{0302}t Nam"), (2, 10));
    }

    #[test]
    fn punctuation() {
        assert_eq!(count("Xin chào, thế giới! (ok)... 3.14"), (7, 32));
        assert_eq!(count("a—b"), (2, 3));
    }

    #[test]
    fn markdown_markers() {
        assert_eq!(count("# Tiêu đề\n- [ ] việc\n> **đậm** *nghiêng*").0, 5);
        assert_eq!(count("1. một\n  2) hai\n10.5 điểm").0, 5);
        assert_eq!(count("`code` | [link](url) ---").0, 3);
    }

    #[test]
    fn status_line() {
        assert_eq!(
            status_text(3, 7, (120, 640)),
            "Ln 3, Col 7 · 120 words · 640 characters"
        );
        assert_eq!(
            status_text(1, 1, (1, 1)),
            "Ln 1, Col 1 · 1 word · 1 character"
        );
    }
}
