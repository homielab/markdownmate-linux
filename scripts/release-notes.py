#!/usr/bin/env python3
"""Copy a version's CHANGELOG.md lines into the other release-note files.

Usage: scripts/release-notes.py <X.Y.Z> [YYYY-MM-DD]   (run from the repo root)

- CHANGELOG.md: with no `## [X.Y.Z]` section yet, the `## [Unreleased]` lines move under a
  new `## [X.Y.Z] - <date>` (default today); an empty `## [Unreleased]` stays on top.
- packaging/com.homielab.MarkdownMate.metainfo.xml: a <release> with every line (App Center).
- resources/whats-new.toml: the first 5 lines (most important first), show = true for X.Y.0.
- debian/changelog: `<version>-0ppa1` for noble, "New upstream release."

A file that already has the version is left alone, so the script can be run again.
"""

import datetime
import email.utils
import json
import re
import sys
from pathlib import Path
from xml.sax.saxutils import escape

CHANGELOG = Path("CHANGELOG.md")
METAINFO = Path("packaging/com.homielab.MarkdownMate.metainfo.xml")
WHATS_NEW = Path("resources/whats-new.toml")
DEBIAN = Path("debian/changelog")
WHATS_NEW_NOTES = 5


def plain(line: str) -> str:
    """A changelog line without Markdown code marks."""
    return line.replace("`", "")


def section_lines(text: str, version: str) -> list[str] | None:
    """The `- ` lines of `## [version]`, or None when there is no such section."""
    match = re.search(rf"^## \[{re.escape(version)}\][^\n]*\n(.*?)(?=^## |\Z)", text, re.M | re.S)
    if not match:
        return None
    return [m[1].strip() for m in re.finditer(r"^[-*] (.+)$", match[1], re.M)]


def release_changelog(version: str, date: str) -> list[str]:
    text = CHANGELOG.read_text()
    lines = section_lines(text, version)
    if lines is None:
        match = re.search(r"^## \[Unreleased\][^\n]*\n(.*?)(?=^## |\Z)", text, re.M | re.S)
        if not match or not match[1].strip():
            sys.exit(f"CHANGELOG.md: no [{version}] section and nothing under [Unreleased]")
        body = match[1].strip("\n")
        text = (
            text[: match.start()]
            + f"## [Unreleased]\n\n## [{version}] - {date}\n\n{body}\n\n"
            + text[match.end() :]
        )
        CHANGELOG.write_text(text)
        lines = section_lines(text, version)
        print(f"CHANGELOG.md: [Unreleased] -> [{version}] - {date}")
    if not lines:
        sys.exit(f"CHANGELOG.md: [{version}] has no lines")
    return [plain(line) for line in lines]


def add_metainfo(version: str, date: str, lines: list[str]) -> None:
    text = METAINFO.read_text()
    if f'<release version="{version}"' in text:
        print(f"{METAINFO}: {version} already there")
        return
    items = "".join(f"          <li>{escape(line)}</li>\n" for line in lines)
    entry = (
        f'    <release version="{version}" date="{date}">\n'
        "      <description>\n"
        "        <ul>\n"
        f"{items}"
        "        </ul>\n"
        "      </description>\n"
        "    </release>\n"
    )
    METAINFO.write_text(text.replace("  <releases>\n", "  <releases>\n" + entry, 1))
    print(f"{METAINFO}: added {version}")


def add_whats_new(version: str, lines: list[str]) -> None:
    text = WHATS_NEW.read_text()
    if re.search(rf'^version = "{re.escape(version)}"$', text, re.M):
        print(f"{WHATS_NEW}: {version} already there")
        return
    show = "true" if version.endswith(".0") else "false"
    notes = "".join(
        f"    {json.dumps(line, ensure_ascii=False)},\n" for line in lines[:WHATS_NEW_NOTES]
    )
    entry = f'[[release]]\nversion = "{version}"\nshow = {show}\nnotes = [\n{notes}]\n\n'
    first = re.search(r"^\[\[release\]\]", text, re.M)
    text = text + "\n" + entry if not first else text[: first.start()] + entry + text[first.start() :]
    WHATS_NEW.write_text(text)
    print(f"{WHATS_NEW}: added {version}")


def add_debian(version: str) -> None:
    text = DEBIAN.read_text()
    deb_version = f"{version}-0ppa1"
    if text.startswith(f"markdownmate ({version}-"):
        print(f"{DEBIAN}: {version} already there")
        return
    stamp = email.utils.format_datetime(datetime.datetime.now().astimezone())
    entry = (
        f"markdownmate ({deb_version}) noble; urgency=medium\n\n"
        "  * New upstream release.\n\n"
        f" -- HomieLab <contact@homielab.com>  {stamp}\n\n"
    )
    DEBIAN.write_text(entry + text)
    print(f"{DEBIAN}: added {deb_version}")


def main() -> None:
    if len(sys.argv) not in (2, 3) or not re.fullmatch(r"\d+\.\d+\.\d+", sys.argv[1]):
        sys.exit(__doc__)
    version = sys.argv[1]
    date = sys.argv[2] if len(sys.argv) == 3 else datetime.date.today().isoformat()
    lines = release_changelog(version, date)
    add_metainfo(version, date, lines)
    add_whats_new(version, lines)
    add_debian(version)


if __name__ == "__main__":
    main()
