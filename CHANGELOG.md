# Changelog

Release notes for MarkdownMate on Ubuntu. The Debian package installs this file as
the upstream changelog; keep `packaging/com.homielab.MarkdownMate.metainfo.xml`
(`<release>` entries, read by App Center) in step with it.

## Unreleased

- Find in Preview: Ctrl+F in Preview (or in Side by side with the preview focused) opens a find bar above the rendered note that highlights every match, counts them and steps through them with Enter / Shift+Enter or Ctrl+G / Shift+Ctrl+G; Esc closes it. Ctrl+F no longer switches Preview to the editor (Ctrl+H still does).
- Drop files or folders from Files or the sidebar into a note to insert relative links (`[name](path)`, one per line, percent-encoded) at the drop point; images are still copied to `assets`.
- Format bar (Shift+Ctrl+M, or main menu ▸ Format ▸ Format Bar): buttons above the editor for Heading (cycles `#` levels), Bold, Italic, Strikethrough, Inline Code, Link, Bulleted / Numbered / Task List, Quote, Code Block and Insert Table, plus the table commands while the cursor is in a table. Each is one undo step; the setting is remembered.
- Renaming or moving a note or folder in the sidebar asks “Update N links in M notes?” and, on Update, fixes the wiki links, relative links and images that pointed to the old place, and a moved note's own relative links; Don’t Update is the default. The open note's change is one undo step.
- Unlinked Mentions: a collapsed section under Backlinks lists the notes that name the current note without linking to it (whole word, any case, not in code or links); click one to open it at that line.
- Table commands: add a row below or above (Alt+Ctrl+Enter, Shift+Alt+Ctrl+Enter), delete a row (Alt+Ctrl+Backspace), add or delete a column (Shift+Alt+Ctrl+Backspace), align a column left / center / right, and Format Table; in the main menu's new Table submenu and on a table bar shown under the editor while the cursor is in a table. Each is one undo step and keeps the cursor in its cell.
- Editor line commands: Toggle Task (Ctrl+Enter), Footnote, Move Line Up / Down (Alt+Ctrl+Up / Down, or Alt+Up / Down) and Duplicate Line (Shift+Ctrl+D), in the new Edit and Format sections of the main menu; each is one undo step.
- Mermaid diagrams: a ```` ```mermaid ```` block shows as a diagram in Preview and Side by side (light and dark), in Print, Export as PDF and Export as HTML; a broken diagram shows its code with an error line. Copy as HTML keeps diagrams as code.
- Fixed: the PPA source package builds again; it ships this changelog from the repository root.
- The README and store notes now match what the app has: the right clone URL, `.deb` / PPA packaging only, and math formulas.

## 0.3.0

- Backlinks to the current note.
- Recent notes in Open Quickly.
- Links that jump to a heading in the preview.
- Insert a table with Shift+Ctrl+T.
- Table editing inside quotes and lists, and monospace table rows.
- Show in Files and Copy Path in the sidebar.
- Cleaner pasted web text.
- A smoother preview with large notes.

## 0.2.0

- Math formulas in the preview.
- Pasting rich text as Markdown.
- Table editing with Tab and Enter.
- Find and replace.
- Typewriter scrolling.
- Image previews on hover.
- Drag and drop in the sidebar.
- A Light/Dark style setting.
- A faster Side by side view.
- Many fixes.

## 0.1.1

- First release in the PPA, with app store metadata and screenshots.

## 0.1.0

- First release, available from a PPA with updates through Software Updater.
