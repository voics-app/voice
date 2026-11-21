# syntax=docker/dockerfile:1.7
# Voics SFU (Rust, str0m). Build context: the sfu/ directory.

# Prebuilt, checksum-pinned cargo-chef (upstream release binary) instead of compiling it from source on every
# cold build; unpacked in alpine because the slim rust image has no xz.
FROM alpine:3.22 AS chef-bin
ARG CHEF_VERSION=0.1.78
ADD --checksum=sha256:70ef940ef90d04d122f0176fdb8d6c39069191b484a1eaa29b327370c2e1c3c0 https://github.com/LukeMathWalker/cargo-chef/releases/download/v${CHEF_VERSION}/cargo-chef-x86_64-unknown-linux-gnu.tar.xz /tmp/cargo-chef.tar.xz
RUN tar -xJf /tmp/cargo-chef.tar.xz -C /tmp && mv /tmp/cargo-chef-*/cargo-chef /cargo-chef

FROM rust:1.99-slim-bookworm AS chef
RUN apt-get update  && apt-get install -y --no-install-recommends build-essential cmake  && rm -rf /var/lib/apt/lists/*
COPY --from=chef-bin /cargo-chef /usr/local/cargo/bin/cargo-chef
WORKDIR /src

# cargo-chef builds the dependency layer once, from a recipe of the manifests only (also reused through the
# registry layer cache in CI, which does not export cache mounts).
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bin ./bin
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry     --mount=type=cache,target=/usr/local/cargo/git     cargo chef cook --release --locked -p sfu --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bin ./bin
RUN --mount=type=cache,target=/usr/local/cargo/registry     --mount=type=cache,target=/usr/local/cargo/git     cargo build --release --locked -p sfu  && install -D target/release/sfu /out/sfu

FROM gcr.io/distroless/cc-debian12:nonroot
ARG VERSION=0.1.0
ARG REVISION=unknown
LABEL org.opencontainers.image.title="voice-sfu" \
      org.opencontainers.image.description="Voics voice SFU (str0m): UDP media mux and NDJSON control server" \
      org.opencontainers.image.source="https://github.com/voics-app/voice" \
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
