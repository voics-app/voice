# syntax=docker/dockerfile:1.7
# Build context: prober/. Building this image does not enable probe enforcement.
FROM alpine:3.22 AS chef-bin
ARG CHEF_VERSION=0.1.78
ADD --checksum=sha256:70ef940ef90d04d122f0176fdb8d6c39069191b484a1eaa29b327370c2e1c3c0 https://github.com/LukeMathWalker/cargo-chef/releases/download/v${CHEF_VERSION}/cargo-chef-x86_64-unknown-linux-gnu.tar.xz /tmp/cargo-chef.tar.xz
RUN tar -xJf /tmp/cargo-chef.tar.xz -C /tmp && mv /tmp/cargo-chef-*/cargo-chef /cargo-chef

FROM rust:1.99-slim-bookworm AS chef
RUN apt-get update && apt-get install -y --no-install-recommends build-essential cmake && rm -rf /var/lib/apt/lists/*
COPY --from=chef-bin /cargo-chef /usr/local/cargo/bin/cargo-chef
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY crates ./crates
COPY bin ./bin
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git cargo chef cook --release --locked -p voice-prober-bin --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY crates ./crates
COPY bin ./bin
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/usr/local/cargo/git cargo build --release --locked -p voice-prober-bin && install -D target/release/voice-prober /out/voice-prober

FROM gcr.io/distroless/cc-debian12:nonroot
ARG VERSION=0.1.0
ARG REVISION=unknown
LABEL org.opencontainers.image.title="voice-prober" \
      org.opencontainers.image.description="Voics independent ICE/DTLS/RTP health observer" \
      org.opencontainers.image.source="https://github.com/voics-app/voice" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.licenses="UNLICENSED"
COPY --from=build /out/voice-prober /usr/local/bin/voice-prober
USER nonroot:nonroot
EXPOSE 4103/tcp
ENV VOICE_PROBE_HTTP_BIND=0.0.0.0:4103
ENTRYPOINT ["/usr/local/bin/voice-prober"]
CMD ["run"]
