//! Auto-pairing brackets, backticks and quotes while typing.
//!
//! Pure rules for:
//! - Openers `(`, `[`, `{`, ``` ` ```, `"`: insert the pair and place the cursor
//!   between them when the next character is end of text/line, whitespace, or one
//!   of `)]}` ``` ` ``` `"`. Before anything else, only the opener is inserted.
//! - `"` and ``` ` ``` pair only when the previous character is not a letter, digit
//!   or the same character (so `it"` and the third backtick of a fence stay plain).
//! - Closers `)`, `]`, `}`, ``` ` ```, `"`: when the next character is that same
//!   character, step over it without inserting anything.
//! - Backspace: when the cursor is between an adjacent opener and its matching closer
//!   (`()`, `[]`, `{}`, ```` `` ````, `""`), deletes both.

/// The typing event passed to [`action`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    Char(char),
    Backspace,
}

impl From<char> for Typing {
    fn from(c: char) -> Self {
        Typing::Char(c)
    }
}

/// The action to perform in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Insert the opener and closer pair, placing the cursor between them.
    InsertPair(char, char),
    /// Insert the character as plain text.
    InsertChar(char),
    /// Step the cursor forward over the closing character.
    Skip,
    /// Delete both the opener before the cursor and the closer after the cursor.
    DeletePair,
    /// Standard single-character backspace.
    DeleteChar,
}

const PAIRS: [(char, char); 5] = [('(', ')'), ('[', ']'), ('{', '}'), ('`', '`'), ('"', '"')];

fn matching_closer(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '`' => Some('`'),
        '"' => Some('"'),
        _ => None,
    }
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '`' | '"')
}

/// Decides what action the editor should take when `typing` occurs with `text_before`
/// before the cursor and `text_after` after the cursor.
pub fn action(typing: Typing, text_before: &str, text_after: &str) -> Action {
    match typing {
        Typing::Backspace => {
            let prev = text_before.chars().next_back();
            let next = text_after.chars().next();
            if let (Some(p), Some(n)) = (prev, next)
                && PAIRS.contains(&(p, n))
            {
                return Action::DeletePair;
            }
            Action::DeleteChar
        }
        Typing::Char(c) => {
            let next = text_after.chars().next();

            // Closer rule: typing a closer when the next character is that same character
            // steps over it, nothing inserted.
            if is_closer(c) && next == Some(c) {
                return Action::Skip;
            }

            // Opener rule:
            if let Some(closer) = matching_closer(c) {
                // Next character must be end of text / line, whitespace, or one of `)]}` ` "
                let next_ok = match next {
                    None => true,
                    Some(n) => n.is_whitespace() || is_closer(n),
                };

                // Quotes and backticks pair only when previous character is not a letter,
                // digit, or the same character.
                let prev_ok = if matches!(c, '`' | '"') {
                    match text_before.chars().next_back() {
                        None => true,
                        Some(p) => !p.is_alphanumeric() && p != c,
                    }
                } else {
                    true
                };

                if next_ok && prev_ok {
                    return Action::InsertPair(c, closer);
                }
            }

            Action::InsertChar(c)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openers_before_space_end_letter_closer() {
        let openers = [('(', ')'), ('[', ']'), ('{', '}'), ('`', '`'), ('"', '"')];

        for (open, close) in openers {
            // Before end of text / line
            assert_eq!(
                action(Typing::Char(open), "", ""),
                Action::InsertPair(open, close),
                "opener '{open}' before end of text"
            );
            assert_eq!(
                action(Typing::Char(open), "", "\n"),
                Action::InsertPair(open, close),
                "opener '{open}' before newline"
            );

            // Before space
            assert_eq!(
                action(Typing::Char(open), "", " "),
                Action::InsertPair(open, close),
                "opener '{open}' before space"
            );
            assert_eq!(
                action(Typing::Char(open), "word ", "   "),
                Action::InsertPair(open, close),
                "opener '{open}' before spaces"
            );

            // Before letter -> stays single char
            assert_eq!(
                action(Typing::Char(open), "", "a"),
                Action::InsertChar(open),
                "opener '{open}' before letter 'a'"
            );
            assert_eq!(
                action(Typing::Char(open), "", "xyz"),
                Action::InsertChar(open),
                "opener '{open}' before 'xyz'"
            );

            // Before closer -> pairs (e.g. nested brackets or quotes before closing paren)
            // Note: for ` and ", if the next char is the SAME closer, typing it triggers Skip.
            // But before a DIFFERENT closer (e.g. before ')'), it inserts a pair.
            assert_eq!(
                action(Typing::Char(open), "", ")"),
                if open == '`' || open == '"' {
                    Action::InsertPair(open, close)
                } else if open == '(' {
                    Action::InsertPair('(', ')')
                } else {
                    Action::InsertPair(open, close)
                },
                "opener '{open}' before ')'"
            );
        }
    }

    #[test]
    fn quote_and_backtick_exceptions() {
        // Quote after letter stays single
        assert_eq!(action(Typing::Char('"'), "it", ""), Action::InsertChar('"'));
        assert_eq!(
            action(Typing::Char('"'), "word", ""),
            Action::InsertChar('"')
        );

        // Quote after digit stays single
        assert_eq!(action(Typing::Char('"'), "5", ""), Action::InsertChar('"'));

        // Quote after quote stays single
        assert_eq!(action(Typing::Char('"'), "\"", ""), Action::InsertChar('"'));

        // Backtick after letter or digit stays single
        assert_eq!(action(Typing::Char('`'), "fn", ""), Action::InsertChar('`'));
        assert_eq!(
            action(Typing::Char('`'), "123", ""),
            Action::InsertChar('`')
        );

        // Backtick after backtick stays single
        assert_eq!(action(Typing::Char('`'), "`", ""), Action::InsertChar('`'));
    }

    #[test]
    fn three_backticks_sequence() {
        // 1st backtick on empty line: pairs -> `|`
        assert_eq!(
            action(Typing::Char('`'), "", ""),
            Action::InsertPair('`', '`')
        );

        // 2nd backtick with cursor inside `|`: next is `, so it skips -> ``|
        assert_eq!(action(Typing::Char('`'), "`", "`"), Action::Skip);

        // 3rd backtick at ``|: previous is `, so it stays plain -> ```|
        assert_eq!(action(Typing::Char('`'), "``", ""), Action::InsertChar('`'));
    }

    #[test]
    fn skip_over_closers() {
        assert_eq!(action(Typing::Char(')'), "(", ")"), Action::Skip);
        assert_eq!(action(Typing::Char(']'), "[", "]"), Action::Skip);
        assert_eq!(action(Typing::Char('}'), "{", "}"), Action::Skip);
        assert_eq!(action(Typing::Char('`'), "`", "`"), Action::Skip);
        assert_eq!(action(Typing::Char('"'), "\"", "\""), Action::Skip);

        // Typing a closer when next char does not match does NOT skip
        assert_eq!(action(Typing::Char(')'), "(", "]"), Action::InsertChar(')'));
        assert_eq!(action(Typing::Char(']'), "[", ")"), Action::InsertChar(']'));
        assert_eq!(action(Typing::Char(')'), "(", ""), Action::InsertChar(')'));
    }

    #[test]
    fn backspace_pair_deletions() {
        assert_eq!(action(Typing::Backspace, "(", ")"), Action::DeletePair);
        assert_eq!(action(Typing::Backspace, "[", "]"), Action::DeletePair);
        assert_eq!(action(Typing::Backspace, "{", "}"), Action::DeletePair);
        assert_eq!(action(Typing::Backspace, "`", "`"), Action::DeletePair);
        assert_eq!(action(Typing::Backspace, "\"", "\""), Action::DeletePair);

        // Mismatched or non-pairs
        assert_eq!(action(Typing::Backspace, "(", "]"), Action::DeleteChar);
        assert_eq!(action(Typing::Backspace, "a", "b"), Action::DeleteChar);
        assert_eq!(action(Typing::Backspace, "", ""), Action::DeleteChar);
        assert_eq!(action(Typing::Backspace, "(", ""), Action::DeleteChar);
        assert_eq!(action(Typing::Backspace, "", ")"), Action::DeleteChar);
        assert_eq!(
            action(Typing::Backspace, "foo(", ")bar"),
            Action::DeletePair
        );
    }

    #[test]
    fn wiki_link_and_completion() {
        // Typing [ gives [|]
        assert_eq!(
            action(Typing::Char('['), "", ""),
            Action::InsertPair('[', ']')
        );

        // Typing [ inside [|] gives [[|]]
        assert_eq!(
            action(Typing::Char('['), "[", "]"),
            Action::InsertPair('[', ']')
        );

        // Check complete::insertion result with the resulting "]]"
        let insertion = crate::complete::insertion("My Note", "]]");
        assert_eq!(insertion, "My Note");
    }

    #[test]
    fn normal_characters_inserted() {
        assert_eq!(action(Typing::Char('a'), "", ""), Action::InsertChar('a'));
        assert_eq!(
            action(Typing::Char('1'), "abc", "def"),
            Action::InsertChar('1')
        );
        assert_eq!(action(Typing::Char('?'), "", ""), Action::InsertChar('?'));
    }
}
