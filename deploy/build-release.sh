#!/bin/sh
# Build the binaries a release carries, and pack them with their checksums in dist/release.
#
#   sh deploy/build-release.sh
#
# Linux is built on Ubuntu 20.04, against glibc 2.31, so it starts on every distribution of the
# last five years; a binary linked on a newer one does not start on an older one. The Macs are
# built here, for both of their processors.
#
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/dist/release/linux"

CONTEXT=$(mktemp -d)
trap 'rm -rf "$CONTEXT"' EXIT
tar -c -C "$ROOT" --exclude target --exclude dist --exclude .git . | tar -x -C "$CONTEXT"

cat > "$CONTEXT/Dockerfile" <<'DOCKER'
FROM ubuntu:20.04 AS builder
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential clang curl ca-certificates pkg-config \
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target-release \
    CARGO_TARGET_DIR=/src/target-release cargo build --release -j 2 && mkdir -p /out && cp /src/target-release/release/zetlyn /out/
FROM scratch
COPY --from=builder /out/ /
DOCKER

mkdir -p "$OUT"
DOCKER_BUILDKIT=1 docker build --platform linux/amd64 --memory 3g \
	--output "type=local,dest=$OUT" "$CONTEXT"

file "$OUT/zetlyn"

# The Macs, here, and everything packed as the install script fetches it.
cd "$ROOT"
cargo build --release -q --target aarch64-apple-darwin
cargo build --release -q --target x86_64-apple-darwin
R="$ROOT/dist/release"
pack() {
	d=$(mktemp -d); cp "$2" "$d/zetlyn"; cp LICENSE "$d/" 2>/dev/null || true
	tar -czf "$R/zetlyn-$1.tar.gz" -C "$d" .; rm -rf "$d"
}
pack linux-x86_64 "$OUT/zetlyn"
pack macos-arm64 target/aarch64-apple-darwin/release/zetlyn
pack macos-x86_64 target/x86_64-apple-darwin/release/zetlyn
( cd "$R" && shasum -a 256 zetlyn-*.tar.gz > SHA256SUMS && cat SHA256SUMS )
