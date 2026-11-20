# One image: the service and the commands it runs (see docs/design/architecture.md).

FROM node:26-trixie-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web ./
RUN npm run build

FROM rust:1.99-slim-trixie AS build
RUN apt-get update \
    && apt-get install --yes --no-install-recommends cmake g++ make
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# The binary embeds the UI; a build without it would still compile, so check it is there.
COPY --from=web /src/web/dist web/dist
RUN test -f web/dist/index.html
RUN cargo build --release --locked --package dalsegno

FROM debian:trixie-slim AS tools
ARG YTDLP_VERSION=2026.10.26
ARG DENO_VERSION=2.9.8
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl unzip
RUN curl --fail --location --silent --show-error --output /usr/local/bin/yt-dlp \
        "https://github.com/yt-dlp/yt-dlp/releases/download/${YTDLP_VERSION}/yt-dlp_linux" \
    && chmod +x /usr/local/bin/yt-dlp
RUN curl --fail --location --silent --show-error --output /tmp/deno.zip \
        "https://github.com/denoland/deno/releases/download/v${DENO_VERSION}/deno-x86_64-unknown-linux-gnu.zip" \
    && unzip -d /usr/local/bin /tmp/deno.zip

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists
COPY --from=tools /usr/local/bin/yt-dlp /usr/local/bin/deno /usr/local/bin/
COPY --from=build /src/target/release/dalsegno /usr/local/bin/
# The service runs as the library owner, who has no home here, and yt-dlp wants a cache.
ENV HOME=/data DALSEGNO_DATA=/data DALSEGNO_BIND=0.0.0.0:8737
EXPOSE 8737
# The service stops cleanly on an interrupt.
STOPSIGNAL SIGINT
CMD ["dalsegno", "serve"]
