# Store & PPA copy

Chữ và ảnh dùng khi phát hành: trang Launchpad PPA, website, GitHub, mạng xã hội. Phần chữ công khai viết bằng tiếng Anh; ghi chú cho Boss bằng tiếng Việt.
Giữ đồng bộ với `packaging/com.homielab.MarkdownMate.metainfo.xml` (App Center / GNOME Software đọc file đó, không đọc file này). Chỉ ghi tính năng đã có: có công thức toán `$…$` (hiển thị bằng MathML qua `math-core`, từ 0.2.0); có sơ đồ Mermaid (```` ```mermaid ````, bản sau 0.3.0); chưa có Flatpak, sync.

## Ảnh

`./dev.sh packaging/make-store-assets.py` → `dist/store/` (không commit, tạo lại khi đổi icon/ảnh chụp). Script in kích thước + dung lượng và fail nếu sai kích thước hoặc vượt giới hạn Launchpad.

| File | Kích thước | Dùng ở đâu |
|---|---|---|
| `icon-512.png`, `icon-256.png` | 512×512, 256×256 | Website, chỗ cần icon PNG (nguồn: `packaging/com.homielab.MarkdownMate.svg`) |
| `social-1280x640.png` | 1280×640 | GitHub → Settings → Social preview; ảnh share link (Open Graph) của website |
| `banner-1218x406.png` | 1218×406 | Banner website / bài đăng |
| `launchpad-icon-14.png` | 14×14, ≤ 5 KB | Launchpad branding "Icon" |
| `launchpad-logo-64.png` | 64×64, ≤ 50 KB | Launchpad branding "Logo" |
| `launchpad-mugshot-192.png` | 192×192, ≤ 100 KB | Launchpad branding "Brand" (mugshot) |

Kích thước Launchpad lấy từ mã nguồn Launchpad (`lib/lp/registry/interfaces/person.py`, form `+branding`): icon đúng 14×14 ≤ 5 KB, logo đúng 64×64 ≤ 50 KB, brand đúng 192×192 ≤ 100 KB.
**Lưu ý:** PPA không có ảnh riêng — branding thuộc về chủ PPA `~homielab` (người dùng, đã có logo + mugshot HomieLab) và hiện trên mọi PPA của HomieLab, kể cả clean-my-ubuntu. Đề nghị giữ ảnh HomieLab; ảnh `launchpad-*` chỉ dùng nếu sau này tạo team riêng cho MarkdownMate.

Ảnh chụp màn hình: `docs/screenshots/1-editor.png` … `7-themes.png` (`./dev.sh packaging/make-screenshots.sh`).

## Launchpad PPA (`ppa:homielab/markdownmate`)

Trang PPA clean-my-ubuntu chỉ có một dòng mô tả ("Free up disk space and keep Ubuntu tidy"); PPA này hiện chưa có mô tả. Mô tả Launchpad là chữ thường (link tự nhận, không Markdown) — dán nguyên khối dưới vào "Change details" của PPA.

**Display name:** `MarkdownMate`

**Description:**

```
Write notes as plain Markdown files in your own folders.

MarkdownMate is a native, local-first Markdown notes app for Ubuntu (GTK 4 + libadwaita). Open any folder and your notes are just .md files in it: no account, no cloud, no telemetry. Everything is saved automatically and never overwrites changes made by another app.

Features:
- Folder sidebar that follows changes on disk
- Edit, Preview and Side by side modes with synced scrolling and highlighted code
- Math formulas and Mermaid diagrams in the preview, fully offline
- Open Quickly by name and Find in Folder across all notes
- Wiki links, tags, outline and autocompletion
- Smart lists, to-do checkboxes, paste or drop images
- Default, Paper, Graphite and Solarized themes in light and dark
- Export as HTML, Copy as HTML and Print to PDF

Install (Ubuntu 24.04 LTS "noble"):
  sudo add-apt-repository ppa:homielab/markdownmate
  sudo apt update
  sudo apt install markdownmate

Updates arrive through Software Updater.

Website: https://markdownmate.homielab.com/
Support the project: https://ko-fi.com/homielab
```

## Chữ cho store / listing

- **Name:** MarkdownMate
- **Tagline** (49 ký tự): `Markdown notes as plain files in your own folders`
- **Short description** (≤ 160): `A native, local-first Markdown notes app for Ubuntu. Your notes stay plain .md files in your own folders: no account, no cloud, no telemetry.` (141 ký tự)
- **Summary trong metainfo:** `Write notes as plain Markdown files in your own folders`
- **Long description:** dùng đúng phần `<description>` của metainfo (đoạn giới thiệu + danh sách "Features"), sửa ở metainfo trước rồi chép sang đây.
- **Keywords:** markdown, notes, editor, wiki, writing, local-first, plain text, GNOME, GTK, Ubuntu
- **Category:** Office / Text Editor
- **Licence:** All rights reserved (proprietary, free to download)
- **Links:** website https://markdownmate.homielab.com/ · donate https://ko-fi.com/homielab · contact contact@homielab.com

## Website / README

**Blurb:**

> MarkdownMate is a native Markdown notes app for Ubuntu. Your notes stay plain `.md` files in folders you choose, so you can back them up, sync them or open them with any other tool. No account, no cloud, no telemetry — just fast, focused writing that feels at home on GNOME.

**Mục "Download for Ubuntu" trên markdownmate.homielab.com** (gợi ý, đặt cạnh nút tải bản macOS):

```
## MarkdownMate for Ubuntu

Native GNOME app for Ubuntu 24.04 LTS. Install it from our PPA and get updates through Software Updater:

    sudo add-apt-repository ppa:homielab/markdownmate
    sudo apt update
    sudo apt install markdownmate

Then open MarkdownMate from the app grid, or run `markdownmate ~/Notes`.

[Screenshots: /ubuntu/screenshots/2-side-by-side.png, 1-editor.png, 6-dark.png]
Launchpad page: https://launchpad.net/~homielab/+archive/ubuntu/markdownmate
```

## Bài đăng ra mắt 0.1.0

**X (≤ 280):**

```
MarkdownMate is now on Ubuntu

Native Markdown notes app (GTK 4 + libadwaita). Your notes stay plain .md files in your own folders — no account, no cloud.

sudo add-apt-repository ppa:homielab/markdownmate
sudo apt install markdownmate

https://markdownmate.homielab.com/
```

**Reddit r/Ubuntu** — tiêu đề: `I made MarkdownMate, a native Markdown notes app for Ubuntu that keeps notes as plain files`

```
Hi r/Ubuntu! MarkdownMate started as a macOS app and now has a native Linux version built with GTK 4 and libadwaita, so it follows the system style and dark mode.

The idea: your notes are just .md files in folders you choose. No account, no cloud, no telemetry — sync or back them up with whatever you already use.

What it does today:
- Folder sidebar that follows changes on disk
- Edit / Preview / Side by side with synced scrolling and highlighted code
- Open Quickly (Ctrl+P) and Find in Folder (Shift+Ctrl+F)
- Wiki links, tags, outline, autocompletion
- Smart lists, to-dos, paste images straight into a note
- Themes (Default, Paper, Graphite, Solarized), light and dark
- Export as HTML, Copy as HTML, Print to PDF

Install on Ubuntu 24.04:

    sudo add-apt-repository ppa:homielab/markdownmate
    sudo apt update
    sudo apt install markdownmate

Website: https://markdownmate.homielab.com/
This is 0.1.0, so feedback is very welcome.
```

**Mastodon (≤ 500):**

```
MarkdownMate 0.1.0 for Ubuntu is out!

A native GNOME Markdown notes app (GTK 4 + libadwaita): your notes stay plain .md files in your own folders. Side by side preview, Open Quickly, wiki links, tags, themes, Export to HTML/PDF. No account, no cloud, no telemetry.

sudo add-apt-repository ppa:homielab/markdownmate
sudo apt install markdownmate

https://markdownmate.homielab.com/

#Ubuntu #GNOME #Markdown #Linux
```

## Checklist cho Boss khi release

- [ ] Upload `docs/screenshots/*.png` lên https://markdownmate.homielab.com/ubuntu/screenshots/ (metainfo trỏ tới đây; thiếu thì App Center không có ảnh).
- [ ] Launchpad → https://launchpad.net/~homielab/+archive/ubuntu/markdownmate/+edit: Display name + Description ở trên.
- [ ] (Tuỳ chọn) Launchpad branding — xem lưu ý ở mục Ảnh; mặc định giữ ảnh HomieLab.
- [ ] GitHub → Settings → Social preview: `dist/store/social-1280x640.png` (repo đang private nên chỉ có tác dụng khi public; dùng thêm làm `og:image` cho website).
- [ ] Website: thêm mục "Download for Ubuntu" + icon `icon-256.png`/`icon-512.png`.
- [ ] Đăng bài X / Reddit / Mastodon sau khi Launchpad build xong và `apt install markdownmate` chạy được trên máy Ubuntu 24.04 sạch.
