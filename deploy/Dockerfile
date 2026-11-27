# Build with the repository root as the Docker context:
# docker build -f deploy/Dockerfile .
FROM oven/bun:1.4.4 AS web-build
WORKDIR /workspace

COPY package.json bun.lock ./
COPY apps/web/package.json apps/web/package.json
COPY apps/mobile/package.json apps/mobile/package.json
RUN bun install --filter @wolui/web --frozen-lockfile

COPY apps/web apps/web
RUN bun run --filter @wolui/web build

FROM rust:1-bookworm AS server-build
WORKDIR /workspace
COPY . .
RUN cargo build --locked --release --package wol-server

FROM debian:bookworm-slim AS runtime
ENV WOLUI_ROLE=site \
    WOLUI_LISTEN=0.0.0.0:3000 \
    WOLUI_DATA_DIR=/data \
    WOLUI_ACCESS_TOKEN_FILE=/run/secrets/wolui_access_token \
    WOLUI_WEB_DIR=/opt/wolui/web

RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 wolui \
    && useradd --uid 10001 --gid wolui --no-create-home --home-dir /nonexistent \
        --shell /usr/sbin/nologin wolui \
    && install -d -o 10001 -g 10001 /data

COPY --from=server-build --chown=0:0 /workspace/target/release/wol-server /usr/local/bin/wol-server
COPY --from=web-build --chown=0:0 /workspace/apps/web/dist /opt/wolui/web

USER 10001:10001
WORKDIR /data
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:3000/health/ready || exit 1
ENTRYPOINT ["/usr/local/bin/wol-server"]
