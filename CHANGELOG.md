# Changelog

Release notes for MarkdownMate on Ubuntu. The Debian package installs this file as
the upstream changelog. At release, `scripts/release-notes.py <version>` copies a version's
lines into the App Center notes (`packaging/com.homielab.MarkdownMate.metainfo.xml`), What's New
(`resources/whats-new.toml`) and `debian/changelog`.

## [Unreleased]

- MarkdownMate for Ubuntu is now open source under the MIT license.

## [0.15.0] - 2026-11-24

- Click a folder in the sidebar to see what's inside: subfolders first, then notes with their dates. Click to dive in, Alt+Left to come back.

## [0.14.0] - 2026-11-17

- Move a whole section up or down: right-click its heading in the outline ▸ Move Section Up / Down.

## [0.13.0] - 2026-11-13

- Open notes as tabs of one window, or in separate windows (Preferences ▸ General ▸ Open files in).

## [0.12.0] - 2026-11-11

- Style the preview, print and HTML / PDF exports with your own CSS file: Preferences ▸ Custom CSS.
- Open several notes at once from Files or the command line, each in its own window; New Window in the main menu (Shift+Ctrl+N); every open window comes back at the next launch.

## [0.11.0] - 2026-11-05

- New to MarkdownMate? Create Notes Folder… sets up a folder with a welcome note, a short tour and four templates.
- Format ▸ Callout inserts a callout of any kind, with no syntax to remember.
- Copy Link to Block copies a link to the paragraph, list item or table at the cursor.
- Hover a footnote in the preview to read it without jumping to the end of the note.
- Screen readers name every button and list row, and Ctrl+Tab moves from the editor to the preview.
- MarkdownMate opens faster, even in a folder with thousands of notes.

## [0.10.0] - 2026-11-02

- Open Quickly (Ctrl+P) now finds headings in every note and notes by their aliases, so you can jump straight to a section.
- Select several notes and folders in the sidebar, then move, trash, favorite or duplicate them all at once.
- Export as Word saves a note as a .docx that opens in Word and LibreOffice Writer.
- Callouts work like in Obsidian: more kinds, custom titles, and ones you can fold open or closed.
- Daily notes follow your own naming, and Daily Note for Date lets you pick a day on a calendar.
- Search in Find in Folder with tag: and path: to narrow down notes, or leave some out.
- Rename a heading in the Outline and every link to it follows.
- Links to a heading or block that no longer exists show up in Find Broken Links.
- Obsidian block references and note embeds now show correctly in the preview, print and exports.
- Front matter shows as a neat properties table in the preview.
- Hover a link in the preview to peek at the note it points to.
- Ctrl+click a tag to jump to it in the sidebar.
- Pasted images can be saved as PNG or JPEG and shrunk, and you choose the Attachments Folder.
- Large folders open faster, so no more staring at the sidebar.
- Preview polish: folded callouts stay as you left them, and tags are clickable only when a folder is open.

## [0.9.0] - 2026-11-01

- Find Open Tasks, Find Broken Links and Find Unused Attachments help you tidy up your folder.
- Export Folder as HTML turns all your notes into web pages with an index.
- Hover a link in the editor to peek at the note it points to.
- Notes can have other names (aliases) in their front matter, and links using them work.
- Tags in front matter now count as tags.
- New Note from Selection moves selected text into its own note and leaves a link behind.
- Previous and Next Daily Note jump between your daily notes.
- Drag a note from the sidebar into the editor to insert a link to it.
- Dates and times in templates can use your own format, and templates can place the cursor.
- Create Starter Templates adds four ready-made templates when you have none.
- Brackets, backticks and quotes close themselves as you type; turn it off in Preferences if you prefer. No more lonely brackets.
- Smart punctuation (off by default) shows curly quotes and proper dashes in the preview and exports.
- Find in Folder can search just one folder from its right-click menu.
- New Note and Today's Note in the dock icon menu, and from the terminal.
- MarkdownMate is now available for Ubuntu 26.04 LTS and 26.10 too.

## [0.8.0] - 2026-11-01

- Spell check in the editor, with corrections, Add to Dictionary and Ignore in the right-click menu.
- Find in Folder can replace text, with options for Match Case, Whole Word and Regex.
- Favorites pin notes and folders at the top of the sidebar.
- Back and Forward return you to the note you came from, like a browser.
- Highlight text with ==text==, for the lines you will want again.
- GitHub-style alerts show as coloured callouts in the preview and exports.
- New from Template creates a note from your templates, and Today's Note can start from a template.
- Table of contents: put [TOC] on its own line to list the note's headings.
- Move to Folder and Duplicate in the sidebar's right-click menu.
- Rename Tag changes a tag in every note at once.
- Unlinked Mentions have a Link button that turns a mention into a link.
- Copy Wiki Link in the sidebar's right-click menu.
- Preferences has an About section with What's New, Send Feedback and more.
- Alerts keep their colours when copied or exported, and Focus Mode keeps text colours.

## [0.7.0] - 2026-10-31

- A new app icon, the same as on the Mac.
- Choose how line breaks show in the preview and exports.

## [0.6.0] - 2026-10-26

- Focus Mode dims everything except the paragraph you are writing.
- Sort the sidebar by Name, Date Modified or Date Created.
- Set Line Spacing and Text Width in Preferences.
- Option to load images from the web in the preview, off by default.
- Backlinks and the sidebar appear faster in large folders.
- The app uses less memory.
- Fixed a crash on Intel graphics on Ubuntu 24.04 with Wayland.

## [0.5.0] - 2026-10-25

- Today's Note (Ctrl+D) opens or creates today's daily note.
- Math formulas show in the preview, print and exports, like on the Mac.
- Copy as Rich Text, Export as Plain Text and Export as RTF.
- Paste spreadsheet cells as a table.
- Click a link to a note that doesn't exist yet to create it.
- Preferences ▸ Default App makes MarkdownMate your Markdown app.
- Suggestions for code block languages and headings in links, and TypeScript code is coloured.
- Backlinks and the preview are much faster in large folders and notes.

## [0.4.0] - 2026-10-24

- Mermaid diagrams show in the preview, print and exports.
- Find in Preview with Ctrl+F.
- A format bar with buttons for headings, bold, links, lists, tables and more.
- Table commands: add or delete rows and columns, and align columns.
- Renaming or moving a note or folder offers to update your links.
- Unlinked Mentions under Backlinks lists notes that name this note without linking to it.
- Toggle Task, Footnote, Move Line and Duplicate Line commands.
- Drop files or folders into a note to insert links to them.
- The preview of large notes with code is much faster.

## [0.3.0] - 2026-10-22

- Backlinks to the current note.
- Recent notes in Open Quickly.
- Links that jump to a heading in the preview.
- Insert a table with Shift+Ctrl+T.
- Table editing inside quotes and lists, and monospace table rows.
- Show in Files and Copy Path in the sidebar.
- Cleaner pasted web text.
- A smoother preview with large notes.

## [0.2.0] - 2026-10-22

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

## [0.1.1] - 2026-10-21

- First release in the PPA, with app store metadata and screenshots.

## [0.1.0] - 2026-10-21

- First release, available from a PPA with updates through Software Updater.
