//! Window title text. Pure Rust (no GTK) so it is testable headless with `cargo test`.

pub const APP_NAME: &str = "MarkdownMate";

/// "MarkdownMate" with no note open, "<note> — MarkdownMate" otherwise.
pub fn window_title(note: Option<&str>) -> String {
    match note {
        Some(name) if !name.is_empty() => format!("{name} — {APP_NAME}"),
        _ => APP_NAME.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_without_note_is_app_name() {
        assert_eq!(window_title(None), "MarkdownMate");
        assert_eq!(window_title(Some("")), "MarkdownMate");
    }

    #[test]
    fn title_with_note_shows_note_first() {
        assert_eq!(window_title(Some("Todo.md")), "Todo.md — MarkdownMate");
    }
}
