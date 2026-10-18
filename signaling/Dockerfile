# syntax=docker/dockerfile:1
# Discordy voice signaling (Elixir release). Build context: the signaling/ directory.

FROM hexpm/elixir:1.20.4-erlang-29.1.2-alpine-3.24.2 AS build
RUN apk add --no-cache build-base git
ENV MIX_ENV=prod
WORKDIR /app
RUN mix local.hex --force && mix local.rebar --force
COPY mix.exs mix.lock ./
COPY config config
RUN mix deps.get --only prod && mix deps.compile
COPY lib lib
RUN mix release --path /out

FROM alpine:3.24.2
ARG VERSION=0.1.0
ARG REVISION=unknown
LABEL org.opencontainers.image.title="voice-signaling" \
      org.opencontainers.image.description="Discordy voice signaling: voice WebSocket, rooms, SFU control and etcd discovery" \
      org.opencontainers.image.source="https://github.com/discordy-voics/voice" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.licenses="UNLICENSED"
RUN apk add --no-cache libstdc++ libgcc ncurses-libs openssl \
 && addgroup -S -g 10001 voice \
 && adduser -S -u 10001 -G voice voice
COPY --from=build --chown=voice:voice /out /app
USER voice:voice
WORKDIR /app
ENV PORT=4100 \
    RELEASE_DISTRIBUTION=none \
    LANG=C.UTF-8
EXPOSE 4100/tcp
ENTRYPOINT ["/app/bin/discordy_voice"]
CMD ["start"]
