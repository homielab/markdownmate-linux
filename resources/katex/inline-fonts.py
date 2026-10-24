#!/usr/bin/env python3
"""Writes KaTeX's katex.min.css with its fonts embedded, so the app needs no font files.

Usage: inline-fonts.py <katex package>/dist > resources/katex/katex.min.css

Each @font-face of the upstream file lists the woff2, woff and ttf files of a font; this keeps
only the woff2 one, as a data: URL. Nothing else in the file changes.
"""
import base64
import pathlib
import re
import sys

dist = pathlib.Path(sys.argv[1])
css = (dist / "katex.min.css").read_text(encoding="utf-8")


def embed(match):
    font = (dist / "fonts" / (match.group(1) + ".woff2")).read_bytes()
    data = base64.b64encode(font).decode("ascii")
    return f'src:url(data:font/woff2;base64,{data}) format("woff2")'


out, count = re.subn(
    r'src:url\(fonts/([A-Za-z0-9_-]+)\.woff2\) format\("woff2"\),'
    r'url\(fonts/\1\.woff\) format\("woff"\),'
    r'url\(fonts/\1\.ttf\) format\("truetype"\)',
    embed,
    css,
)
if count == 0 or "url(fonts/" in out:
    sys.exit("inline-fonts.py: unexpected @font-face rules, check the upstream file")
sys.stdout.write(out)
