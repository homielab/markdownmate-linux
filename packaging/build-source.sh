#!/usr/bin/env bash
#
# Build the source package for the Launchpad PPA in dist/ppa/. Run in the build container:
#
#   ./dev.sh packaging/build-source.sh -us -uc    # unsigned (the container has no GPG key)
#   packaging/build-source.sh                     # signed, on a Debian/Ubuntu host with the key
#
# The upstream tarball holds every file git knows about (tracked, plus new files that
# aren't ignored) except debian/ and the agent-team files, plus vendor/: every crate from
# Cargo.lock (`cargo vendor`, needs network here), because Launchpad builds offline.
# vendor/ is never committed to git. Then sign (debsign, if built with -us -uc) and:
#   dput ppa:homielab/markdownmate dist/ppa/markdownmate_<version>_source.changes
#
set -euo pipefail

# Ubuntu's cargo-1.91 (CI runner, Launchpad) installs its cargo here; absent in ./dev.sh.
export PATH=/usr/lib/rust-1.91/bin:$PATH
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)
DEB_VERSION=$(dpkg-parsechangelog -l "$ROOT/debian/changelog" -S Version)
OUT=${OUT:-$ROOT/dist/ppa}

[[ ${DEB_VERSION%%-*} == "$VERSION" ]] || {
  echo "debian/changelog is at $DEB_VERSION but Cargo.toml says $VERSION." >&2
  echo "Add an entry first:  dch -v $VERSION-0ppa1 -D noble \"New upstream release.\"" >&2
  exit 1
}

WORK=$(mktemp -d -t mm-src.XXXXXX)
trap 'rm -rf "$WORK"' EXIT
SRC=$WORK/markdownmate-$VERSION
# The repo is mounted from the host with another owner inside ./dev.sh.
git() { command git -c safe.directory="$ROOT" -C "$ROOT" "$@"; }

# upstream tarball
mkdir "$SRC"
git ls-files -co --exclude-standard -z -- . ':!debian' ':!agent-team' ':!agent.sh' ':!CLAUDE.md' |
  (cd "$ROOT" && tar --null -T - -cf -) | tar -xf - -C "$SRC"
cp "$ROOT/agent-team/CHANGELOG.md" "$SRC/CHANGELOG.md"
cargo vendor --locked --manifest-path "$SRC/Cargo.toml" "$SRC/vendor" > /dev/null
STAMP=$(git log -1 --format=%ct) # reproducible, and not "ancient" to lintian
tar -C "$WORK" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$STAMP" \
  -cJf "$WORK/markdownmate_$VERSION.orig.tar.xz" "markdownmate-$VERSION"

# packaging
cp -a "$ROOT/debian" "$SRC/debian"
(cd "$SRC" && dpkg-buildpackage -S -d -sa "$@")

mkdir -p "$OUT"
cp "$WORK"/markdownmate_"$VERSION".orig.tar.xz "$WORK"/markdownmate_"$DEB_VERSION"* "$OUT/"
echo "Source package in $OUT:"
ls -1sh "$OUT" | sed 's/^/  /'
