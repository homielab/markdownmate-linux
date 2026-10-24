#!/usr/bin/env python3
"""Render store/branding images into dist/store/ (uploaded by hand, so not committed).
Run: ./dev.sh packaging/make-store-assets.py [out_dir]. Sizes and where each goes: docs/STORE.md.

Needs: rsvg-convert (librsvg2-bin), Pillow (python3-pil), fonts-ubuntu.
"""
import io, os, subprocess, sys

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SVG = f"{ROOT}/packaging/com.homielab.MarkdownMate.svg"
OUT = sys.argv[1] if len(sys.argv) > 1 else f"{ROOT}/dist/store"
SHOT = f"{ROOT}/docs/screenshots/2-side-by-side.png"
NAME = "MarkdownMate"
TAGLINE = "Markdown notes as plain files in your own folders"


def font(pattern, size):
    path = subprocess.run(["fc-match", "-f", "%{file}", pattern], capture_output=True, text=True, check=True).stdout
    f = ImageFont.truetype(path, size)
    if ":bold" in pattern:
        try:  # fonts-ubuntu in 24.04 is one variable font: pick the weight ourselves
            f.set_variation_by_name("Bold")
        except OSError:
            pass  # static font file: fc-match already returned the bold one
    return f


def icon(size):
    png = subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size), SVG], capture_output=True, check=True).stdout
    return Image.open(io.BytesIO(png)).convert("RGBA")


def gradient(w, h, top=(74, 69, 84), bottom=(36, 33, 42)):
    # Dark graphite, close to the metainfo dark branding colour #3d3846.
    img = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(img)
    for y in range(h):
        t = y / max(h - 1, 1)
        d.line([(0, y), (w, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(top, bottom)))
    return img.convert("RGBA")


def save(img, name, size):
    path = f"{OUT}/{name}"
    img.save(path)
    assert img.size == size, (name, img.size)
    print(f"wrote {os.path.relpath(path, ROOT)}  {img.width}x{img.height}  {os.path.getsize(path) // 1024 or 1} KB")


def tile(size, bg=(61, 56, 70)):
    """Square on a solid background: Launchpad shows these without transparency."""
    img = Image.new("RGBA", (size, size), bg + (255,))
    ic = icon(size)
    img.alpha_composite(ic)
    return img.convert("RGB")


def screenshot(img, w, h, sw):
    shot = Image.open(SHOT).convert("RGBA")
    shot = shot.resize((sw, int(shot.height * sw / shot.width)), Image.LANCZOS)
    mask = Image.new("L", shot.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, *shot.size), 12, fill=255)
    sx, sy = w - sw - 56, (h - shot.height) // 2
    shadow = Image.new("RGBA", img.size, (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((sx, sy + 12, sx + sw, sy + shot.height + 12), 12, fill=(0, 0, 0, 120))
    img.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(14)))
    img.paste(shot, (sx, sy), mask)


def banner(w, h):
    """Wide banner: icon on the left, name and tagline beside it."""
    img = gradient(w, h)
    d = ImageDraw.Draw(img)
    ic = icon(int(h * .52))
    x = int(h * .16)
    img.alpha_composite(ic, (x, (h - ic.height) // 2))
    tx = x + ic.width + int(h * .09)
    d.text((tx, h * .28), NAME, font=font("Ubuntu:bold", int(h * .14)), fill="white")
    sub = font("Ubuntu", int(h * .065))
    d.text((tx, h * .50), TAGLINE + ".", font=sub, fill=(240, 238, 244))
    d.text((tx, h * .62), "Local-first · no account, no cloud · for Ubuntu", font=sub, fill=(200, 196, 210))
    return img.convert("RGB")


def social(w, h):
    """Link preview: text column on the left, a real screenshot on the right."""
    img = gradient(w, h)
    d = ImageDraw.Draw(img)
    img.alpha_composite(icon(150), (56, 70))
    d.text((64, 250), NAME, font=font("Ubuntu:bold", 64), fill="white")
    tag = font("Ubuntu", 30)
    d.text((64, 342), "Markdown notes as plain", font=tag, fill=(240, 238, 244))
    d.text((64, 382), "files in your own folders.", font=tag, fill=(240, 238, 244))
    d.text((64, 470), "Native GNOME app · local-first", font=font("Ubuntu", 22), fill=(200, 196, 210))
    d.text((64, 505), "markdownmate.homielab.com", font=font("Ubuntu:bold", 22), fill="white")
    screenshot(img, w, h, int(w * .47))
    return img.convert("RGB")


os.makedirs(OUT, exist_ok=True)
save(icon(512), "icon-512.png", (512, 512))
save(icon(256), "icon-256.png", (256, 256))
save(banner(1218, 406), "banner-1218x406.png", (1218, 406))
save(social(1280, 640), "social-1280x640.png", (1280, 640))
# Launchpad person/team branding (+branding): icon 14x14 ≤5 KB, logo 64x64 ≤50 KB, brand/mugshot 192x192 ≤100 KB.
save(tile(14), "launchpad-icon-14.png", (14, 14))
save(tile(64), "launchpad-logo-64.png", (64, 64))
save(tile(192), "launchpad-mugshot-192.png", (192, 192))
for name, kb in [("launchpad-icon-14.png", 5), ("launchpad-logo-64.png", 50), ("launchpad-mugshot-192.png", 100)]:
    assert os.path.getsize(f"{OUT}/{name}") <= kb * 1024, f"{name} over Launchpad's {kb} KB limit"
