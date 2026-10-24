//! Go to Line (Ctrl+L): parses `line` or `line:column` (1-based). Pure logic.

/// `"12"` → `(12, None)`, `" 12 : 5 "` → `(12, Some(5))`. Zero, negative, non-numbers or
/// empty → `None`.
pub fn parse(input: &str) -> Option<(usize, Option<usize>)> {
    let num = |s: &str| s.trim().parse::<usize>().ok().filter(|&n| n > 0);
    match input.split_once(':') {
        Some((line, col)) => Some((num(line)?, Some(num(col)?))),
        None => Some((num(input)?, None)),
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn valid() {
        assert_eq!(parse("12"), Some((12, None)));
        assert_eq!(parse("12:5"), Some((12, Some(5))));
        assert_eq!(parse("  12 : 5  "), Some((12, Some(5))));
        assert_eq!(parse(" 7\t"), Some((7, None)));
    }

    #[test]
    fn invalid() {
        for s in [
            "", "   ", "0", "0:3", "3:0", "-1", "3:-2", "+-1", "abc", "12a", "12:", ":5", "1:2:3",
            "1.5",
        ] {
            assert_eq!(parse(s), None, "{s:?}");
        }
    }
}
