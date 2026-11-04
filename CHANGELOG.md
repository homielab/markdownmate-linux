# Changelog

Release notes for MarkdownMate on Ubuntu. The Debian package installs this file as
the upstream changelog; keep `packaging/com.homielab.MarkdownMate.metainfo.xml`
(`<release>` entries, read by App Center) in step with it.

## Unreleased

- Packaging: when Launchpad briefly refuses the upload for one Ubuntu version, the release is uploaded again automatically, so a new version reaches every supported Ubuntu release without waiting for a manual retry.
- Format ▸ Callout in the main menu inserts a callout of any kind — Note, Tip, Important, Warning, Caution and the Obsidian kinds — so you no longer need to remember the `> [!kind]` syntax. With lines selected, they become the callout's body. Same as the macOS app.
- Copy Link to Block in the editor's right-click menu and main menu ▸ Edit: copies `[[Note#^id]]` for the paragraph, list item, quote or table at the cursor, first adding a `^id` to it if it has none, so linking to a block no longer means inventing an id and typing it twice. Same as the macOS app.
- Hovering a footnote reference in the preview shows the footnote's text in a small box next to it, so reading a footnote no longer means jumping to the end of the note and back. A click still jumps to it. Same as the macOS app.

## 0.10.0

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
- Large folders open faster.
- Preview polish: folded callouts stay as you left them, and tags are clickable only when a folder is open.

## 0.9.0

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
- Brackets, backticks and quotes close themselves as you type; turn it off in Preferences if you prefer.
- Smart punctuation (off by default) shows curly quotes and proper dashes in the preview and exports.
- Find in Folder can search just one folder from its right-click menu.
- New Note and Today's Note in the dock icon menu, and from the terminal.
- MarkdownMate is now available for Ubuntu 26.04 LTS and 26.10 too.

## 0.8.0

- Spell check in the editor, with corrections, Add to Dictionary and Ignore in the right-click menu.
- Find in Folder can replace text, with options for Match Case, Whole Word and Regex.
- Favorites pin notes and folders at the top of the sidebar.
- Back and Forward return you to the note you came from, like a browser.
- Highlight text with ==text==.
- GitHub-style alerts show as coloured callouts in the preview and exports.
- New from Template creates a note from your templates, and Today's Note can start from a template.
- Table of contents: put [TOC] on its own line to list the note's headings.
- Move to Folder and Duplicate in the sidebar's right-click menu.
- Rename Tag changes a tag in every note at once.
- Unlinked Mentions have a Link button that turns a mention into a link.
- Copy Wiki Link in the sidebar's right-click menu.
- Preferences has an About section with What's New, Send Feedback and more.
- Alerts keep their colours when copied or exported, and Focus Mode keeps text colours.

## 0.7.0

- A new app icon, the same as on the Mac.
- Choose how line breaks show in the preview and exports.

## 0.6.0

- Focus Mode dims everything except the paragraph you are writing.
- Sort the sidebar by Name, Date Modified or Date Created.
- Set Line Spacing and Text Width in Preferences.
- Option to load images from the web in the preview, off by default.
- Backlinks and the sidebar appear faster in large folders.
- The app uses less memory.
- Fixed a crash on Intel graphics on Ubuntu 24.04 with Wayland.
