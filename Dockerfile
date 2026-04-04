# syntax=docker/dockerfile:1

# Stage 1: Build both binaries
FROM rust:1.94.1-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin proxy-server --bin worker-daemon

# Stage 2: Minimal runtime image
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/proxy-server /usr/local/bin/proxy-server
COPY --from=builder /build/target/release/worker-daemon /usr/local/bin/worker-daemon
EXPOSE 8080
CMD ["proxy-server"]
