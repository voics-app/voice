# syntax=docker/dockerfile:1
# Discordy SFU (Rust, str0m). Build context: the sfu/ directory.

FROM rust:1.99-slim-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential cmake \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bin ./bin
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p sfu \
 && install -D target/release/sfu /out/sfu

FROM gcr.io/distroless/cc-debian12:nonroot
ARG VERSION=0.1.0
ARG REVISION=unknown
LABEL org.opencontainers.image.title="voice-sfu" \
      org.opencontainers.image.description="Discordy voice SFU (str0m): UDP media mux and NDJSON control server" \
      org.opencontainers.image.source="https://github.com/discordy-voics/voice" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.licenses="UNLICENSED"
COPY --from=build /out/sfu /usr/local/bin/sfu
USER nonroot:nonroot
# Media (UDP), control (TCP, loopback by default) and operational HTTP.
EXPOSE 50000/udp 4101/tcp 4102/tcp
ENV SFU_UDP_BIND=0.0.0.0:50000 \
    SFU_CONTROL_BIND=127.0.0.1:4101 \
    SFU_HTTP_BIND=0.0.0.0:4102
ENTRYPOINT ["/usr/local/bin/sfu"]
