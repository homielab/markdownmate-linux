# Changelog

Release notes for MarkdownMate on Ubuntu. The Debian package installs this file as
the upstream changelog; keep `packaging/com.homielab.MarkdownMate.metainfo.xml`
(`<release>` entries, read by App Center) in step with it.

## Unreleased

- Preferences ▸ Privacy ▸ Load Images from the Web (off by default): turn it on to show `http(s)` images in Preview, Side by side, Print and Export as PDF / HTML, as in the macOS app; scripts and other web content stay blocked. Off, nothing is downloaded — now also while Export as HTML draws formulas or diagrams — and the preview says why a web image is missing.
- The sidebar appears with the window: the last folder is read while the window opens, so with 10,000 notes the sidebar is filled about 0.13 s after launch instead of 0.2–0.6 s, already in the first frame.
- Sort the sidebar by Name, Date Modified or Date Created (main menu ▸ Sort Sidebar By, or Sort By in the sidebar's right-click menu), as in the macOS app: folders first, dates newest first, remembered. A saved note moves to the top under Date Modified without closing folders. Names now sort naturally, so `Note 2` comes before `Note 10`.
- Focus Mode (Shift+Ctrl+J, or main menu ▸ Focus Mode), as in the macOS app: the editor dims every paragraph except the one you are writing in, in every theme, light or dark. Off by default and remembered; typing in a 5 MB note is as fast with it on.
- Fixed a crash on Intel graphics (Ubuntu 24.04, Wayland): the preview no longer hands its frames to GTK as GPU (DMA-BUF) buffers unless you set `WEBKIT_DISABLE_DMABUF_RENDERER` yourself; the README's troubleshooting section lists the settings to try if a crash remains.
- Typing or opening a note no longer logs "Trying to snapshot GtkGizmo … without a current allocation" (the editor's scrollbar).
- Less memory: the preview's web page is set up the first time you open Preview or Side by side instead of at launch, so a session spent in Edit starts no WebKit process (about 55 MB less); the hidden page used by Print, Export as PDF and Export as HTML is released once done instead of staying until the next print; WebKit keeps only small caches.
- Instant Backlinks in large folders: the links of every note are read once when the folder opens and kept up to date as notes are saved, changed on disk, renamed, moved, created or deleted, so with 10,000 notes the panel fills in a few milliseconds instead of about 0.3 s, and refreshes when another note changes on disk.
- Preferences ▸ Editor ▸ Line Spacing (1 to 2) and Text Width (Narrow, Medium, Wide or Full Width), as in the macOS app: on a wide screen the editor's text and the preview's page stay centred at a readable width (Medium by default; Full Width fills the window edge to edge in the editor and the preview, with the usual padding). Print and exports are unchanged.

## 0.5.0

- Preferences ▸ Default App: shows whether MarkdownMate is the default app for Markdown files and has a Make Default button, so `.md` files open here from Files without digging through Open With.
- TypeScript code blocks are coloured: ```` ```ts ````, ```` ```typescript ```` and ```` ```tsx ```` in Preview, Side by side, Print and exports; the code block language list offers `ts`, `tsx` and `typescript`.
- Faster Backlinks in large folders: with 10,000 notes the panel lists a note's backlinks in about 0.3 s instead of a minute and a half. Link updates after renaming or moving a note in the sidebar use the same faster lookup.
- Export as RTF… (main menu): saves the note as a `.rtf` file that opens formatted in LibreOffice Writer and other word processors — headings, emphasis, links, lists, quotes, code, tables; images as `alt (path)` text, formulas as TeX, front matter left out.
- Clicking a `[[wiki link]]` to a note that doesn't exist (Ctrl+click in the editor, click in the preview) creates it empty and opens it, as in the macOS app: `Name.md` at the open folder's top level (beside the note when no folder is open), `[[Folder/Name]]` only into an existing folder; `#heading` and `|text` are ignored. Names that can't be a file still show the message.
- New screenshots showing KaTeX math, Mermaid diagrams and the format bar.
- Today's Note (Ctrl+D, or the main menu): opens today's `YYYY-MM-DD.md` in the open folder's `Daily` folder, creating it as `# YYYY-MM-DD` with the cursor at the end when it doesn't exist yet. The folder is set in Preferences ▸ Notes (empty = the open folder itself).
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
