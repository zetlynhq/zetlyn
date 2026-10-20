#!/bin/sh
# The Linux binary, built in a container on the distribution it runs on (Ubuntu 24.04), so it
# links against the glibc that is there. (As the old Zetlyn's build-linux.sh.)
#
#   sh deploy/build-linux.sh      → dist/amd64/zetlyn
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT="$ROOT/dist/amd64"
CONTEXT=$(mktemp -d)
trap 'rm -rf "$CONTEXT"' EXIT
tar -c -C "$ROOT" --exclude target --exclude dist --exclude data --exclude secrets --exclude .git . | tar -x -C "$CONTEXT"

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
    cargo build --release --locked -j 2 && mkdir -p /out && cp target/release/zetlyn /out/
FROM scratch
COPY --from=builder /out/ /
DOCKER

mkdir -p "$OUT"
DOCKER_BUILDKIT=1 docker build --platform linux/amd64 --memory 3g --output "type=local,dest=$OUT" "$CONTEXT"
file "$OUT/zetlyn"
