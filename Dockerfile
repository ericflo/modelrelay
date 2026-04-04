# syntax=docker/dockerfile:1

# Stage 1: Build both binaries
FROM rust:1.94.1-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin modelrelay-server --bin modelrelay-worker

# Stage 2: Minimal runtime image
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/modelrelay-server /usr/local/bin/modelrelay-server
COPY --from=builder /build/target/release/modelrelay-worker /usr/local/bin/modelrelay-worker
EXPOSE 8080
CMD ["modelrelay-server"]
