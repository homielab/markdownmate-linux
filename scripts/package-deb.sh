#!/usr/bin/env bash
# Build the release binary and pack dist/markdownmate_<version>_<arch>.deb.
# Run inside the build container: ./dev.sh scripts/package-deb.sh
# Arch = the container's (arm64 on Apple Silicon). amd64 from an arm64 Mac (emulated):
#   ARCH=amd64 ./dev.sh scripts/package-deb.sh
# --stage DIR: only install the built binary + data files under DIR (used by debian/rules,
# which builds and strips itself), so the manual .deb and the PPA package share one layout.
set -euo pipefail
cd "$(dirname "$0")/.."
bin="${CARGO_TARGET_DIR:-target}/release/markdownmate"
id=com.homielab.MarkdownMate

stage() {
    install -Dm755 "$bin" "$1/usr/bin/markdownmate"
    install -Dm644 "packaging/$id.desktop" "$1/usr/share/applications/$id.desktop"
    install -Dm644 "packaging/$id.metainfo.xml" "$1/usr/share/metainfo/$id.metainfo.xml"
    install -Dm644 "packaging/$id.svg" "$1/usr/share/icons/hicolor/scalable/apps/$id.svg"
    install -Dm644 "packaging/$id-symbolic.svg" "$1/usr/share/icons/hicolor/symbolic/apps/$id-symbolic.svg"
    # Scheme GtkSourceView riêng của app (theme Paper, Graphite); app tìm ở ../share/markdownmate/styles cạnh binary.
    install -Dm644 -t "$1/usr/share/markdownmate/styles" packaging/styles/*.xml
    # Cho sandbox của WebKit tạo user namespace trên Ubuntu 24.04 (#82); nạp bởi postinst.
    install -Dm644 packaging/markdownmate.apparmor "$1/etc/apparmor.d/markdownmate"
}
if [ "${1:-}" = --stage ]; then
    stage "${2:?usage: $0 --stage DIR}"
    exit
fi

cargo build --release
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(dpkg --print-architecture)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
root="$work/pkg"
stage "$root"
strip "$root/usr/bin/markdownmate"

# dpkg-shlibdeps needs a debian/control to run from; it prints the runtime library packages.
mkdir -p "$work/debian"
printf 'Source: markdownmate\n\nPackage: markdownmate\nArchitecture: any\n' > "$work/debian/control"
depends=$(cd "$work" && dpkg-shlibdeps --warnings=0 -O "pkg/usr/bin/markdownmate" | sed -n 's/^shlibs:Depends=//p')

mkdir -p "$root/DEBIAN"
install -m755 debian/markdownmate.postinst "$root/DEBIAN/postinst"
echo /etc/apparmor.d/markdownmate > "$root/DEBIAN/conffiles"
cat > "$root/DEBIAN/control" <<CONTROL
Package: markdownmate
Version: $version
Architecture: $arch
Maintainer: minhtc <minhtc.git@homielab.com>
Installed-Size: $(du -sk "$root/usr" | cut -f1)
Depends: $depends
Section: editors
Priority: optional
Homepage: https://github.com/homielab/markdownmate-ubuntu
Description: Local-first, native Markdown note-taking app
 MarkdownMate keeps notes as plain .md files in your own folders (GTK 4 + libadwaita).
CONTROL

mkdir -p dist
out="dist/markdownmate_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$root" "$out" > /dev/null
echo "$out"
