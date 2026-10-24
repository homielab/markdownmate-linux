#!/usr/bin/env bash
# Run a command inside the Ubuntu 24.04 build container, e.g. ./dev.sh cargo test
# ARCH=amd64 ./dev.sh ... runs an amd64 container (emulated on Apple Silicon) with its own image/target cache.
set -euo pipefail
cd "$(dirname "$0")"
IMAGE=markdownmate-dev
TARGET=markdownmate-target
# Always explicit: the local ubuntu:24.04 tag may hold the other architecture after an ARCH=amd64 run.
NATIVE=$(docker version -f '{{.Server.Arch}}')
ARCH=${ARCH:-$NATIVE}
PLATFORM="--platform=linux/$ARCH"
if [ "$ARCH" != "$NATIVE" ]; then
    IMAGE+="-$ARCH"
    TARGET+="-$ARCH"
fi

# Cached by Docker: a no-op unless the Dockerfile changed. No build context needed.
docker build -q $PLATFORM -t "$IMAGE" - < Dockerfile > /dev/null

TTY=-i
[ -t 0 ] && [ -t 1 ] && TTY=-it

# Named volumes keep the cargo registry and build output between runs (fast rebuilds).
exec docker run --rm "$TTY" $PLATFORM \
    -v "$PWD":/work \
    -v markdownmate-cargo-registry:/usr/local/cargo/registry \
    -v "$TARGET":/target \
    -e CARGO_TARGET_DIR=/target \
    "$IMAGE" "${@:-bash}"
