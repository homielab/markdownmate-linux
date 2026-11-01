#!/usr/bin/env bash
# Build the PPA source package (dist/ppa/, from packaging/build-source.sh) the way Launchpad does:
# clean ubuntu:24.04, build-deps from the archive (rustc-1.91), then the build itself with the
# network OFF, as a non-root user with no writable HOME. Runs lintian (fails on E:), then installs
# the resulting .deb in another clean container and starts the app (scripts/check-deb.sh).
# Run on the host (needs Docker):
#   ./dev.sh packaging/build-source.sh -us -uc && scripts/check-source.sh
#   ARCH=amd64 scripts/check-source.sh   # emulated amd64 container (slow)
#   ./dev.sh env SERIES=resolute packaging/build-source.sh -us -uc && SERIES=resolute scripts/check-source.sh
# SERIES=<codename> checks that series' package (*~<series>1.dsc) in a clean ubuntu:<codename>.
# SRC_DIR=<dir> reads the source package from there instead of dist/ppa (build-source.sh's OUT).
# DEB_OUT=<dir> puts the built .deb there instead of dist/ppa-deb (emptied first).
set -euo pipefail
cd "$(dirname "$0")/.."
ARCH=${ARCH:-$(docker version -f '{{.Server.Arch}}')}
# Always explicit: the local ubuntu:<series> tag may hold the other architecture.
PLATFORM="--platform=linux/$ARCH"
IMAGE=markdownmate-srccheck-$ARCH
OUT=${DEB_OUT:-dist/ppa-deb}
SRC_DIR=${SRC_DIR:-dist/ppa}
SERIES=${SERIES:-}
BASE=ubuntu:${SERIES:-24.04}
if [ -n "$SERIES" ]; then DSC=$(ls "$SRC_DIR"/*~"${SERIES}"1.dsc); else DSC=$(ls "$SRC_DIR"/*.dsc); fi
[ "$(echo "$DSC" | wc -l | tr -d " ")" = 1 ] || { echo "more than one .dsc in $SRC_DIR/: set SERIES or move the old ones out" >&2; exit 1; }
DSC=/src/$(basename "$DSC")

# The build-deps image (~2 GB) only lives for this run: removed on exit, success or failure.
cleanup() {
    docker rm -fv "$IMAGE" > /dev/null 2>&1 || true
    docker image rm -f "$IMAGE" > /dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

# 1. With network: build-deps + lintian, saved as an image.
cleanup
docker run -i $PLATFORM --name "$IMAGE" -e DSC="$DSC" -v "$PWD/$SRC_DIR":/src:ro "$BASE" bash -s <<'IN'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends dpkg-dev lintian > /dev/null
apt-get build-dep -y -qq "$DSC" > /dev/null
useradd -m builder
IN
docker commit "$IMAGE" "$IMAGE" > /dev/null
docker rm -fv "$IMAGE" > /dev/null

# 2. Network off: lintian, unpack, build, copy the .deb out.
rm -rf "$OUT" && mkdir -p "$OUT"
docker run --rm -i $PLATFORM --network none -e DSC="$DSC" -v "$PWD/$SRC_DIR":/src:ro -v "$PWD/$OUT":/out \
    "$IMAGE" bash -s <<'IN'
set -euo pipefail
su builder -s /bin/sh -c "lintian --info ${DSC%.dsc}_source.changes" > /tmp/lintian.txt 2>&1 || true
echo "lintian:"; sed 's/^/  /' /tmp/lintian.txt
! grep -q '^E:' /tmp/lintian.txt || { echo "lintian reported errors"; exit 1; }
su builder -s /bin/bash -c '
set -euo pipefail
export HOME=/nonexistent
cd /tmp && dpkg-source -x '"$DSC"' mm > /dev/null
cd mm && dpkg-buildpackage -b -us -uc > /tmp/build.log 2>&1 || { tail -40 /tmp/build.log; exit 1; }
grep -E "^test result:" /tmp/build.log
'
cp /tmp/*.deb /out/
ls /out
IN
cleanup

# 3. Install it in a clean container and start the app.
DIST=$OUT SERIES=$SERIES scripts/check-deb.sh
