FROM rust:1.83-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p epec-legal-agent

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/epec-legal-agent /usr/local/bin/
ENTRYPOINT ["epec-legal-agent"]
