#!/bin/sh
# Build the Linux binary in a container and put it in dist/.
#
#   sh deploy/build-linux.sh
#
# Built on the distribution it runs on. A binary linked against a newer glibc than the machine it
# is copied to does not start, and the machine is Ubuntu 24.04.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/dist/amd64"

CONTEXT=$(mktemp -d)
trap 'rm -rf "$CONTEXT"' EXIT
tar -c -C "$ROOT" --exclude target --exclude dist --exclude .git . | tar -x -C "$CONTEXT"

cat > "$CONTEXT/Dockerfile" <<'DOCKER'
FROM ubuntu:24.04 AS builder
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential clang curl ca-certificates pkg-config \
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -j 2 && mkdir -p /out && cp target/release/zetlyn /out/
FROM scratch
COPY --from=builder /out/ /
DOCKER

mkdir -p "$OUT"
DOCKER_BUILDKIT=1 docker build --platform linux/amd64 --memory 3g \
	--output "type=local,dest=$OUT" "$CONTEXT"

file "$OUT/zetlyn"
