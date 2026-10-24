#!/usr/bin/env bash
# Run a command inside the Ubuntu 24.04 build container, e.g. ./dev.sh cargo test
# ARCH=amd64 ./dev.sh ... runs an amd64 container (emulated on Apple Silicon) with its own image/target cache.
set -euo pipefail
cd "$(dirname "$0")"

# ./dev.sh clean: drop the build output (target volumes), the emulated amd64 image, leftover
# source-check images and old untagged dev images. Keeps markdownmate-dev and the cargo registry.
if [ "${1:-}" = clean ]; then
    for v in markdownmate-target markdownmate-target-amd64 markdownmate-target-arm64; do
        if docker volume inspect "$v" > /dev/null 2>&1; then
            docker volume rm "$v" > /dev/null && echo "removed volume $v"
        fi
    done
    for i in markdownmate-srccheck-amd64 markdownmate-srccheck-arm64; do
        docker rm -fv "$i" > /dev/null 2>&1 || true
    done
    for i in markdownmate-dev-amd64 markdownmate-dev-arm64 markdownmate-srccheck-amd64 markdownmate-srccheck-arm64; do
        if docker image inspect "$i" > /dev/null 2>&1; then
            docker image rm "$i" > /dev/null && echo "removed image $i"
        fi
    done
    docker image prune -f --filter label=com.homielab.markdownmate.dev=1 > /dev/null
    exit 0
fi

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
