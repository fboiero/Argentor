# Stage 1: Build the WASM echo-skill
FROM rust:slim-bookworm AS wasm-builder
RUN rustup target add wasm32-wasip1
WORKDIR /build
COPY skills/echo-skill/ skills/echo-skill/
RUN cargo build --target wasm32-wasip1 --release --manifest-path skills/echo-skill/Cargo.toml

# Stage 2: Build the main binary
FROM rust:slim-bookworm AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY wit/ wit/
COPY experiments/ experiments/
RUN cargo build --release --bin argentor && strip /build/target/release/argentor

# Stage 3: Minimal runtime image (~80MB)
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.title="Argentor" \
      org.opencontainers.image.description="Autonomous AI agent framework — secure, sandboxed, multi-agent orchestration" \
      org.opencontainers.image.url="https://github.com/fboiero/Agentor" \
      org.opencontainers.image.source="https://github.com/fboiero/Agentor" \
      org.opencontainers.image.licenses="AGPL-3.0-only" \
      org.opencontainers.image.vendor="Argentor"

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl3 wget && \
    rm -rf /var/lib/apt/lists/* && \
    useradd -r -s /usr/sbin/nologin -d /app argentor

WORKDIR /app

# Copy binary
COPY --from=builder /build/target/release/argentor /app/argentor

# Copy WASM skills
COPY --from=wasm-builder /build/skills/echo-skill/target/wasm32-wasip1/release/echo-skill.wasm /app/skills/echo-skill.wasm

# Copy default config
COPY argentor.toml /app/argentor.toml

# Create data directories
RUN mkdir -p /app/data/audit /app/data/sessions /app/data/transcripts && \
    chown -R argentor:argentor /app

USER argentor

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=10s --retries=3 --start-period=10s \
    CMD ["/app/argentor", "health"]

ENTRYPOINT ["/app/argentor"]
CMD ["serve"]
