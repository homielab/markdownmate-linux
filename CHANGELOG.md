# Changelog

Release notes for MarkdownMate on Ubuntu. The Debian package installs this file as
the upstream changelog; keep `packaging/com.homielab.MarkdownMate.metainfo.xml`
(`<release>` entries, read by App Center) in step with it.

## Unreleased

- Faster preview while typing in large notes: only the blocks you changed are rendered and replaced on the page, instead of the whole note, so formulas, diagrams and the scroll position stay put. Notes with footnotes or reference links, and edits to front matter, still refresh the whole page.
- Copy as Rich Text (main menu): copies the selection or the whole note so it pastes formatted into LibreOffice, Thunderbird and web mail (headings, bold, lists, links, tables, the note's images) and as plain text elsewhere. Copy as HTML is in the main menu too.
- Export as Plain Text…: the main menu saves the note as a `.txt` without Markdown syntax (bullets as `• `, links as `text (url)`, code kept, formulas as TeX, tables as tab-separated rows, front matter left out).
- Math typeset by KaTeX: `$…$`, `$$…$$` and ```` ```math ```` formulas (fractions, matrices, most LaTeX macros) look as in the macOS app in Preview and Side by side (light and dark), Print, Export as PDF and Export as HTML; a broken formula shows its source in red with the error on hover. KaTeX is bundled and loaded only for notes with math. Copy as HTML keeps MathML.
- Paste spreadsheet cells as a table: tab-separated text with two or more rows and the same two or more columns on every row pastes as an aligned Markdown table, the first row as the header. Shift+Ctrl+V still pastes plain text.
- Code block languages: typing ```` ``` ```` (or `~~~`) and a letter at the start of a line pops up the languages the preview colours (`js`, `py`, `sh`, `yml`, …, plus `math` and `mermaid`).
- Heading completion: typing `#` after `[[Note` pops up that note's headings, and `[[#` this note's.

## 0.4.0

- Faster preview of large notes with code: code blocks are highlighted on all CPU cores, and while you type only the code blocks you changed are highlighted again (in a 1 MB test note, the first preview is about 4× faster and an update after a keystroke about 9× faster).
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
