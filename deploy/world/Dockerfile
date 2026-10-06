# A world of its own, as a container: `zetlyn world serve` on port 2500, its workspace in /world.
# Caddy in front of it, for the domain and its certificate, is compose.yaml beside this.
#
#   docker build -f deploy/world/Dockerfile -t zetlyn-world .
#
# Built from this checkout, on the distribution it runs on.
FROM ubuntu:24.04 AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential clang curl ca-certificates pkg-config \
 && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -j 2 && cp target/release/zetlyn /zetlyn

FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --home-dir /world --shell /usr/sbin/nologin zetlyn \
 && mkdir -p /world && chown zetlyn:zetlyn /world
COPY --from=build /zetlyn /usr/local/bin/zetlyn
COPY --from=build /src/LICENSE /src/NOTICE /src/THIRD_PARTY_LICENSES /usr/share/doc/zetlyn/
USER zetlyn
ENV HOME=/world ZETLYN_HOME=/world/.zetlyn
VOLUME /world
EXPOSE 2500
# The workspace is made on the first start, from DOMAIN and OWNER, and left as it is after that.
CMD ["sh", "-c", "zetlyn world init /world --domain \"$DOMAIN\" --owner \"$OWNER\" && exec zetlyn world serve /world --addr 0.0.0.0:2500"]
