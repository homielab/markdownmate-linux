//! Outline ▸ Move Section Up / Down: a heading with everything under it (up to the next heading
//! of the same or a higher level) swaps places with the previous or next section of the same
//! level under the same parent. Pure logic, no GTK.

use crate::outline::{self, Heading};
use crate::relink::Edit;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    Up,
    Down,
}

/// The index of the heading the section at `index` swaps with, or `None` when it has none
/// (first / last under its parent, or the neighbour is of another level).
pub fn sibling(headings: &[Heading], index: usize, direction: Direction) -> Option<usize> {
    let level = headings.get(index)?.level;
    let found = match direction {
        Direction::Up => headings[..index].iter().rposition(|h| h.level <= level),
        Direction::Down => headings[index + 1..]
            .iter()
            .position(|h| h.level <= level)
            .map(|i| index + 1 + i),
    }?;
    (headings[found].level == level).then_some(found)
}

/// The edit that moves the section of the heading on 1-based `line`, and the 1-based line the
/// heading is on afterwards; `None` when it can't move. Both sections keep their bytes; a last
/// section without a final line break gets the note's line break before the other.
pub fn move_section(text: &str, line: usize, direction: Direction) -> Option<(Edit, usize)> {
    let headings = outline::headings(text);
    let index = headings.iter().position(|h| h.line == line)?;
    let other = sibling(&headings, index, direction)?;
    let (first, second) = match direction {
        Direction::Up => (other, index),
        Direction::Down => (index, other),
    };
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let start = |h: usize| starts.get(headings[h].line.checked_sub(1)?).copied();
    let level = headings[second].level;
    let end = match headings[second + 1..].iter().position(|h| h.level <= level) {
        Some(i) => start(second + 1 + i)?,
        None => text.len(),
    };
    let (a_start, b_start) = (start(first)?, start(second)?);
    if a_start >= b_start || b_start > end {
        return None;
    }
    let a = &text[a_start..b_start];
    let mut b = text[b_start..end].to_string();
    if !b.ends_with('\n') {
        b.push_str(if a.ends_with("\r\n") { "\r\n" } else { "\n" });
    }
    let heading_line = match direction {
        Direction::Up => headings[first].line,
        Direction::Down => headings[first].line + b.matches('\n').count(),
    };
    let edit = Edit {
        range: a_start..end,
        text: b + a,
    };
    Some((edit, heading_line))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relink;

    /// The text after moving the section on `line`, and the moved heading's line; `None` when it can't move.
    fn moved(text: &str, line: usize, direction: Direction) -> Option<(String, String)> {
        let (edit, at) = move_section(text, line, direction)?;
        let after = relink::apply(text, &[edit]);
        let heading = after.lines().nth(at - 1).unwrap_or_default().to_string();
        Some((after, heading))
    }

    const NESTED: &str = "Intro\n# A\na\n## A1\na1\n## A2\na2\n# B\nb\n";

    #[test]
    fn moves_with_subsections() {
        let up = moved(NESTED, 8, Direction::Up).unwrap();
        assert_eq!(up.0, "Intro\n# B\nb\n# A\na\n## A1\na1\n## A2\na2\n");
        assert_eq!(up.1, "# B");
        let down = moved(NESTED, 2, Direction::Down).unwrap();
        assert_eq!(down.0, up.0);
        assert_eq!(down.1, "# A");
    }

    #[test]
    fn subsections_stay_under_their_parent() {
        assert_eq!(
            moved(NESTED, 6, Direction::Up).unwrap().0,
            "Intro\n# A\na\n## A2\na2\n## A1\na1\n# B\nb\n"
        );
        assert_eq!(
            moved(NESTED, 4, Direction::Up),
            None,
            "first under its parent"
        );
        assert_eq!(
            moved(NESTED, 6, Direction::Down),
            None,
            "last under its parent"
        );
        assert_eq!(
            moved(NESTED, 2, Direction::Up),
            None,
            "text before never moves"
        );
        assert_eq!(moved(NESTED, 8, Direction::Down), None, "last");
    }

    #[test]
    fn neighbour_of_another_level_disables() {
        let text = "## A\na\n# B\nb\n";
        assert_eq!(moved(text, 3, Direction::Up), None);
        assert_eq!(moved(text, 1, Direction::Down), None);
        assert_eq!(
            sibling(&outline::headings(NESTED), 0, Direction::Down),
            Some(3)
        );
    }

    #[test]
    fn setext_headings() {
        assert_eq!(
            moved("A\n===\na\n\nB\n===\nb\n", 5, Direction::Up)
                .unwrap()
                .0,
            "B\n===\nb\nA\n===\na\n\n"
        );
    }

    #[test]
    fn code_blocks_and_front_matter_ignored() {
        let text = "---\ntitle: x\n---\n# A\n```\n# not\n```\n# B\nb\n";
        assert_eq!(
            moved(text, 8, Direction::Up).unwrap().0,
            "---\ntitle: x\n---\n# B\nb\n# A\n```\n# not\n```\n"
        );
        assert_eq!(moved(text, 6, Direction::Up), None);
    }

    #[test]
    fn last_section_without_newline_gets_one() {
        assert_eq!(
            moved("# A\na\n# B\nb", 3, Direction::Up).unwrap().0,
            "# B\nb\n# A\na\n"
        );
        assert_eq!(
            moved("# A\r\na\r\n# B\r\nb", 1, Direction::Down).unwrap().0,
            "# B\r\nb\r\n# A\r\na\r\n"
        );
    }

    #[test]
    fn crlf_kept() {
        let (text, heading) = moved("x\r\n# A\r\na\r\n# B\r\nb\r\n", 2, Direction::Down).unwrap();
        assert_eq!(text, "x\r\n# B\r\nb\r\n# A\r\na\r\n");
        assert_eq!(heading, "# A");
    }
}
