# Build/test environment for MarkdownMate (Ubuntu 24.04: GTK 4.14, libadwaita 1.5). Used by ./dev.sh.
# git/debhelper: PPA source package (packaging/build-source.sh).
# librsvg2-bin/python3-pil: store images (packaging/make-store-assets.py).
# xvfb/scrot/xdotool/xclip (clipboard)/dbus-x11/librsvg2-common (SVG icons): headless GUI smoke test (scripts/smoke.sh).
FROM ubuntu:24.04

RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates curl build-essential pkg-config git debhelper \
        libgtk-4-dev libadwaita-1-dev libgtksourceview-5-dev libwebkitgtk-6.0-dev \
        xvfb scrot xdotool xclip dbus-x11 librsvg2-common fonts-ubuntu \
        librsvg2-bin python3-pil \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
        --component clippy,rustfmt

WORKDIR /work

# Lets `./dev.sh clean` find this repo's old, untagged images (left behind by Dockerfile changes).
LABEL com.homielab.markdownmate.dev=1
