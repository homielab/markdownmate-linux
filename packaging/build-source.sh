#!/usr/bin/env bash
#
# Build the source package for the Launchpad PPA in dist/ppa/. Run in the build container:
#
#   ./dev.sh packaging/build-source.sh -us -uc    # unsigned (the container has no GPG key)
#   packaging/build-source.sh                     # signed, on a Debian/Ubuntu host with the key
#   ./dev.sh env SERIES=resolute packaging/build-source.sh -us -uc   # for one Ubuntu series
#
# SERIES=<codename> builds the upload for that series: the top debian/changelog entry becomes
# <version>~<series>1 for <series> (only in the package, the repo's file is untouched), so each
# series gets its own version and newer series sort higher. Series after
# noble build with their default rustc/cargo (>= 1.91; noble's default is older, so noble keeps
# rustc-1.91). Every series shares one .orig.tar.xz: a second run must produce the same bytes.
#
# The upstream tarball holds every file git knows about (tracked, plus new files that
# aren't ignored, CHANGELOG.md included) except debian/, plus vendor/: every crate from
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
SERIES=${SERIES:-}
[[ $SERIES =~ ^[a-z]*$ ]] || { echo "SERIES must be an Ubuntu codename such as noble, not '$SERIES'." >&2; exit 1; }

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
git ls-files -co --exclude-standard -z -- . ':!debian' |
  (cd "$ROOT" && tar --null -T - -cf -) | tar -xf - -C "$SRC"
cargo vendor --locked --manifest-path "$SRC/Cargo.toml" "$SRC/vendor" > /dev/null
STAMP=$(git log -1 --format=%ct) # reproducible, and not "ancient" to lintian
tar -C "$WORK" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$STAMP" \
  -cJf "$WORK/markdownmate_$VERSION.orig.tar.xz" "markdownmate-$VERSION"

# packaging
cp -a "$ROOT/debian" "$SRC/debian"
if [[ -n $SERIES ]]; then
  DEB_VERSION=$DEB_VERSION~${SERIES}1
  sed -i "1s/^markdownmate ([^)]*) [a-z-]*;/markdownmate ($DEB_VERSION) $SERIES;/" "$SRC/debian/changelog"
  [[ $(dpkg-parsechangelog -l "$SRC/debian/changelog" -S Version) == "$DEB_VERSION" ]]
  if [[ $SERIES != noble ]]; then
    sed -i 's/\brustc-1\.91\b/rustc (>= 1.91)/; s/\bcargo-1\.91\b/cargo (>= 1.91)/' "$SRC/debian/control"
  fi
fi
(cd "$SRC" && dpkg-buildpackage -S -d -sa "$@")

mkdir -p "$OUT"
ORIG=markdownmate_$VERSION.orig.tar.xz
if [[ -e $OUT/$ORIG ]] && ! cmp -s "$WORK/$ORIG" "$OUT/$ORIG"; then
  echo "$OUT/$ORIG is from another build: Launchpad needs one tarball per version. Remove it and run again." >&2
  exit 1
fi
cp "$WORK/$ORIG" "$WORK"/markdownmate_"$DEB_VERSION"* "$OUT/"
echo "Source package in $OUT:"
ls -1sh "$OUT" | sed 's/^/  /'
