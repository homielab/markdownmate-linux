# Changelog

Release notes for MarkdownMate on Ubuntu. The Debian package installs this file as
the upstream changelog; keep `packaging/com.homielab.MarkdownMate.metainfo.xml`
(`<release>` entries, read by App Center) in step with it.

## Unreleased

- New Note and Today's Note in the dock icon's right-click menu and the app grid, so a thought is captured without switching to the app first. Also from a terminal: `markdownmate --new` and `markdownmate --today` reach the running window or start one. Same as the macOS Dock menu.
- Find Broken Links… in the main menu (also a Broken Links toggle in Find in Folder): lists every `[[wiki link]]` that reaches no note or alias and every relative link or image whose file is missing, across the folder's notes, so links broken by a rename or move outside the app can be fixed. Each row opens the note at that line. URLs, `mailto:`, absolute paths, anchors, code and front matter are skipped. Find in Folder… is in the main menu now too. Same as the macOS app.
- Find in “Folder”… in the sidebar's right-click menu on a folder: Find in Folder searches only the notes in that folder and its subfolders, and Replace All changes only those. A chip under the search box shows the folder; its ✕ searches everything again. Shift+Ctrl+F still searches the whole folder. Same as the macOS app.
- Front-matter aliases: list a note's other names under `aliases:` (or `alias:`) in its YAML front matter and `[[Alias]]` links reach it — in the preview, Ctrl+click, the hover tooltip, Backlinks, Unlinked Mentions and Export Folder as HTML. `[[` completion offers them as "Alias → Note". A note actually named like the alias still wins. Same as the macOS app.
- New Note from Selection… in the main menu (Shift+Ctrl+Alt+N): moves the selected text into a new note, named from its first line or heading, and leaves a `[[wiki link]]` to it in its place, so a long note splits into linked notes in one step. You stay in the note with the link selected; Ctrl+Z brings the text back, Ctrl+click on the link opens the new note. Same as the macOS app.
- Previous Daily Note / Next Daily Note in the main menu (Ctrl+Alt+[ / Ctrl+Alt+]): from a daily note, jump to the nearest earlier or later one that exists (never creates one); from any other note, Previous opens the most recent. Same as the macOS app.
- Export Folder as HTML… in the main menu: writes every note of the open folder as a standalone HTML page into an empty folder you pick, with wiki links and links to other notes pointing at their pages, the images they show and every `assets` folder copied along, and an `index.html` listing the notes as a tree by folder. Shows the progress and can be cancelled. Same as the macOS app.
- Hover a `[[wiki link]]` in the editor to see the linked note's first lines in a tooltip, without leaving the note you are writing: its name and up to 12 lines as plain Markdown (front matter left out), starting at the heading for `[[Note#Heading]]`. Large notes are read only as far as shown. Same as the macOS app.
- Tags in a note's YAML front matter count as tags: `tags: [a, b/c]`, a list of `- a` lines, or `tags: a, b` (also `tag:`, with or without `#`). They show in the sidebar's Tags list, `#` completion offers them, and Rename Tag rewrites them along with inline `#tags`. Notes moved from Obsidian or Jekyll keep their tags. Same as the macOS app.

## 0.8.0

- Preferences ▸ About: the app version (click to copy), What's New, Send Feedback (opens your mail app with the version and system details filled in, or copies the address), Share MarkdownMate, Donate, More Apps from HomieLab and the website. What's New is also in the main menu and the About dialog; after an update a small dot shows there is something new until you open it. It never pops up on its own.
- Find in Folder: a Regex toggle next to Match Case and Whole Word searches with a regular expression (Rust regex syntax, line by line); an invalid pattern shows why under the search box. Replace All then fills in groups: `$1`…`$9` or `${1}`, `$$` for a literal `$`. Same as the macOS app.
- Favorites: right-click a note or folder ▸ Add to Favorites to pin it in a Favorites section at the top of the sidebar, one click away; Open Quickly lists them first. Each folder keeps its own, remembered across restarts, and they follow renames and moves made in the app. Same as the macOS app.
- Duplicate in the sidebar's right-click menu: copies a note or a folder with its contents next to itself as "Name copy.md" ("Name copy 2.md" when taken) and opens a note copy. Same as the macOS app.
- Move to Folder… in the sidebar's right-click menu: pick the folder from a list of the open folder's folders to move a note or folder without dragging, also from the keyboard. It refuses a name that is already taken and offers to update links, like a drag, and the sidebar then shows the moved entry. Same as the macOS app.
- Back and Forward (Alt+Left / Alt+Right, or the main menu): return to the note you came from after following a link, a backlink or Open Quickly, then forward again, like a browser. Each window has its own history of up to 50 notes; notes deleted since are skipped. Same as the macOS app.
- Highlights: write `==text==` and it shows with a soft yellow background in the preview (light and dark), print, HTML / PDF export and Copy as HTML; Copy as Rich Text and Export as RTF give it a yellow background, and the editor shows it on a light background with dimmed `==`. Format ▸ Highlight (Shift+Ctrl+H, or `==` on the format bar) adds or removes it around the selection or the word at the cursor. Same as the macOS app.
- Alerts keep their colour when copied or exported: Copy as Rich Text pastes each kind (Note, Tip, Important, Warning, Caution) with its coloured border and title instead of grey, and Export as RTF shows a bold title in the kind's colour above the indented text.
- New from Template in the main menu: lists the notes of a `Templates` folder in the open folder and creates an Untitled note from the one you pick, filling `{{date}}` with today's date and `{{title}}` with the note's name, then opens it with the cursor at the end. The folder is set in Preferences ▸ Notes ▸ Templates Folder. Same as the macOS app.
- Find in Folder: Match Case and Whole Word toggles under the search box (both off by default, remembered across restarts). Replace All follows them.
- Unlinked Mentions: each entry has a Link button that turns that mention into a wiki link to the current note, `[[Name]]`, or `[[Name|mention]]` when the wording differs from the name. Ctrl+Z undoes it in the open note; a note changed since the list was made is left alone, with a message. Same as the macOS app.
- Table of contents: a line with only `[TOC]` shows the note's headings as nested links in the preview, print and HTML / PDF exports, and updates as you edit a heading. Same as the macOS app.
- Find in Folder can replace: type the new text and click Replace All to replace every listed match in all result notes, after confirming how many matches and notes change. Ctrl+Z undoes it in the open note; notes changed on disk since the search are left alone and named.
- Rename Tag… in the right-click menu of the sidebar's Tags list: renames `#old` to `#new` in every note of the folder (nested tags such as `#old/project` too), after showing how many notes change. Ctrl+Z undoes it in the open note.
- GitHub-style alerts: a quote starting with a `[!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]` or `[!CAUTION]` line shows as a coloured callout in the preview, print and HTML / PDF / rich-text exports, instead of a plain quote with a literal `[!NOTE]`.
- The package now installs a spell-check dictionary (US English, or any Hunspell one already there), so underlines show on a clean install.
- Focus Mode: dimmed paragraphs keep their colours (headings, links, code), faded, instead of turning one flat grey.
- Copy Wiki Link in a note's right-click menu in the sidebar: puts `[[Name]]` on the clipboard, or `[[Folder/Name]]` when another note in the folder has the same name.
- Spell check in the editor (Preferences ▸ Editor ▸ Check Spelling, on by default): misspelled words are underlined as you type, in your system's language, and a right-click offers corrections, Add to Dictionary and Ignore. Inline code, ``` code blocks and link URLs are skipped. Notes over 1 MB are not checked (the status bar says so); right-click ▸ Check Spelling checks one anyway.
- Today's Note (Ctrl+D) starts from a Template.md in the daily notes folder when present, replacing {{date}} and {{title}} with the date.

## 0.7.0

- A new app icon: the same M↓ mark as on the Mac, now with bolder, solid black strokes.
- Preferences ▸ Appearance ▸ Line Breaks: choose *Every New Line* to show each newline in a paragraph as a line break (no two spaces or `\` needed) in the preview, print and HTML / PDF / rich-text exports, as in the macOS app. *Markdown Standard* stays the default.

## 0.6.0

- App Center shows a new screenshot: math and diagrams in the preview.
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
