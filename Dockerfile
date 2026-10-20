# Zetlyn in a container: the program, and Chromium for pages their scripts make.
#
#   docker build -t zetlyn .
#   docker run -p 127.0.0.1:4848:4848 -v zetlyn:/data zetlyn
#
# The image starts one account nobody signs in to (`--local`): for yourself, on your machine.
# For an instance others sign in to, see "Run it for others" in README.md.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked && cp target/release/zetlyn /zetlyn

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates chromium tini \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /data zetlyn \
 && mkdir /data && chown zetlyn /data
COPY --from=build /zetlyn /usr/local/bin/zetlyn
COPY LICENSE NOTICE THIRD_PARTY_LICENSES /usr/share/doc/zetlyn/
LABEL org.opencontainers.image.title="Zetlyn" \
      org.opencontainers.image.description="Watch a place on a web page, or a whole page, and be told when it changes; an API and an MCP server for LLMs." \
      org.opencontainers.image.url="https://zetlyn.com" \
      org.opencontainers.image.source="https://github.com/zetlynhq/zetlyn" \
      org.opencontainers.image.licenses="MIT"
# The container is Chrome's sandbox here; Chrome cannot make its own inside Docker's.
ENV ZETLYN_CHROME=/usr/bin/chromium ZETLYN_CHROME_SANDBOX=off
USER zetlyn
VOLUME /data
EXPOSE 4848
ENTRYPOINT ["tini", "--", "zetlyn"]
CMD ["serve", "--local", "--bind", "0.0.0.0:4848", "--data", "/data"]
