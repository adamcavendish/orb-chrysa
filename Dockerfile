FROM rust:1.92-slim AS builder

WORKDIR /app
ENV CI=true
ENV VP_HOME=/opt/vite-plus
ENV VP_NODE_MANAGER=yes
ENV VP_NODE_VERSION=24.15.0
ENV NPM_CONFIG_REGISTRY=https://registry.npmmirror.com
ENV npm_config_registry=https://registry.npmmirror.com
ENV PATH="${VP_HOME}/bin:${PATH}"

RUN apt-get update \
    && apt-get install -y pkg-config libssl-dev ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && echo "registry=https://registry.npmmirror.com" > /app/.npmrc \
    && curl -fsSL https://vite.plus | bash \
    && vp --version \
    && node --version

COPY Cargo.toml Cargo.lock ./
COPY docs/assets/brand docs/assets/brand
COPY crates/ crates/
RUN cp /app/.npmrc crates/orb-chrysa-server/dashboard/.npmrc

RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cd crates/orb-chrysa-server/dashboard \
    && vp install \
    && vp build \
    && cd /app \
    && cargo build --release -p orb-chrysa-server \
    && cp /app/target/release/orb-chrysa-server /tmp/orb-chrysa-server

FROM debian:trixie-slim

RUN apt-get update && apt-get install -y ca-certificates curl && rm -rf /var/lib/apt/lists/*

COPY --from=builder /tmp/orb-chrysa-server /usr/local/bin/orb-chrysa-server

ENTRYPOINT ["orb-chrysa-server"]
