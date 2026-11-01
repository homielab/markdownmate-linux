#!/usr/bin/env bash
# Install dist/*.deb in a clean ubuntu:24.04 container, validate the .desktop file and start the
# installed app on Xvfb with a note given on the command line. Run on the host (needs Docker):
#   scripts/check-deb.sh              # the host's architecture (arm64 on Apple Silicon)
#   ARCH=amd64 scripts/check-deb.sh   # emulated amd64 container
#   DIST=dist/ppa-deb scripts/check-deb.sh   # another folder (scripts/check-source.sh)
#   SERIES=resolute scripts/check-deb.sh     # in ubuntu:resolute instead
set -euo pipefail
cd "$(dirname "$0")/.."
# Always explicit: the local ubuntu:<series> tag may hold the other architecture after an amd64 run.
PLATFORM="--platform=linux/${ARCH:-$(docker version -f '{{.Server.Arch}}')}"

docker run --rm -i $PLATFORM -v "$PWD/${DIST:-dist}":/dist:ro "ubuntu:${SERIES:-24.04}" bash -s <<'IN'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
deb=$(ls /dist/*_"$(dpkg --print-architecture)".deb)
echo "checking $deb"
apt-get update -qq
apt-get install -y -qq --no-install-recommends "$deb" xvfb xauth desktop-file-utils apparmor > /dev/null
# AppArmor profile (WebKit's sandbox on Ubuntu 24.04, #82): installed, a conffile, and it parses.
apparmor_parser -Q -K /etc/apparmor.d/markdownmate
dpkg-query -W -f='${Conffiles}' markdownmate | grep -q /etc/apparmor.d/markdownmate
echo "apparmor profile: ok"
desktop-file-validate /usr/share/applications/com.homielab.MarkdownMate.desktop
echo "desktop-file-validate: ok"
# AppStream metainfo: App Center / GNOME Software chỉ hiện mô tả, ảnh khi có file này.
ls /usr/share/metainfo/com.homielab.MarkdownMate.metainfo.xml > /dev/null

mkdir -p /tmp/notes && echo "# Hello" > /tmp/notes/note.md
export XDG_CONFIG_HOME=/tmp/config
# Theme dùng scheme riêng của app: phải tìm được trong gói đã cài.
ls /usr/share/markdownmate/styles/markdownmate-paper.xml > /dev/null
mkdir -p /tmp/config/markdownmate && echo "theme=paper" > /tmp/config/markdownmate/state
# WebKit's sandbox cannot run inside a container (same as scripts/smoke.sh).
export WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
status=0
# xvfb-run gives the command its own stderr: redirect inside it, or app.log stays empty.
xvfb-run -a timeout 8 sh -c 'exec markdownmate /tmp/notes/note.md 2> /tmp/app.log' || status=$?
cat /tmp/app.log
[ "$status" = 124 ] || { echo "app exited early ($status)"; exit 1; }
! grep -qE "panicked|CRITICAL|size_allocate|style scheme" /tmp/app.log || { echo "app printed a panic/CRITICAL/size_allocate/missing scheme warning"; exit 1; }
# The note given on the command line was opened inside its folder.
grep -q "notes/note.md" /tmp/config/markdownmate/state || { echo "note not opened"; cat /tmp/config/markdownmate/state; exit 1; }
echo "installed app started and opened the note: ok"
# Without the override WebKit's sandbox cannot start here (no user namespaces in the container,
# like Ubuntu 24.04 without the profile): the app must stay up, preview off (#82).
printf 'folder=/tmp/notes\nnote=/tmp/notes/note.md\nmode=split\n' > /tmp/config/markdownmate/state
status=0
xvfb-run -a timeout 8 env -u WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS sh -c 'exec markdownmate 2> /tmp/app.log' || status=$?
[ "$status" = 124 ] || { cat /tmp/app.log; echo "app exited early without the sandbox ($status)"; exit 1; }
grep -q "preview disabled" /tmp/app.log || { cat /tmp/app.log; echo "sandbox probe did not run"; exit 1; }
echo "app without WebKit's sandbox stays up: ok"
IN
