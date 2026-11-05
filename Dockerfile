# syntax=docker/dockerfile:1@sha256:87999aa3d42bdc6bea60565083ee17e86d1f3339802f543c0d03998580f9cb89

FROM rust:1.94.1-slim@sha256:cf09adf8c3ebaba10779e5c23ff7fe4df4cccdab8a91f199b0c142c53fef3e1a AS build
WORKDIR /build

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

ADD --checksum=sha256:7fb9497f8594b389e5fce5ef9b92db08432996895b2e0c5a0167a69ed445c428 \
    https://github.com/rustsec/rustsec/releases/download/cargo-audit/v0.22.2/cargo-audit-x86_64-unknown-linux-musl-v0.22.2.tgz \
    /tmp/cargo-audit.tgz
ADD --checksum=sha256:9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f \
    https://github.com/EmbarkStudios/cargo-deny/releases/download/0.20.2/cargo-deny-0.20.2-x86_64-unknown-linux-musl.tar.gz \
    /tmp/cargo-deny.tgz
RUN mkdir -p /opt/modelrelay-ci \
    && tar -xzf /tmp/cargo-audit.tgz -C /opt/modelrelay-ci --strip-components=1 \
      cargo-audit-x86_64-unknown-linux-musl-v0.22.2/cargo-audit \
    && tar -xzf /tmp/cargo-deny.tgz -C /opt/modelrelay-ci --strip-components=1 \
      cargo-deny-0.20.2-x86_64-unknown-linux-musl/cargo-deny \
    && rm /tmp/cargo-audit.tgz /tmp/cargo-deny.tgz

COPY Cargo.toml Cargo.lock rust-toolchain.toml deny.toml ./
COPY .cargo/ .cargo/
COPY crates/ crates/
COPY docs/ docs/

# Quality checks and release compilation share one exact source snapshot and
# one node-local compiler cache. A failed check prevents /out from existing,
# so neither production target can be published without the complete gate.
RUN --mount=type=cache,id=modelrelay-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=modelrelay-cargo-target,target=/build/target,sharing=locked \
    CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 cargo fmt --check \
    && CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 \
      cargo clippy --locked --workspace --exclude modelrelay-desktop --all-targets --all-features -- -D warnings \
    && CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 \
      cargo test --locked --workspace --exclude modelrelay-desktop \
    && /opt/modelrelay-ci/cargo-audit audit \
    && /opt/modelrelay-ci/cargo-deny --all-features --locked check licenses bans sources \
    && cargo build --locked --release \
      -p modelrelay-server -p modelrelay-worker -p modelrelay-cloud \
    && install -d /out \
    && install -m 0755 target/release/modelrelay-server /out/modelrelay-server \
    && install -m 0755 target/release/modelrelay-worker /out/modelrelay-worker \
    && install -m 0755 target/release/modelrelay-cloud /out/modelrelay-cloud \
    && install -m 0755 target/release/reprovision-server-keys /out/reprovision-server-keys

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS cloud
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/modelrelay-cloud /usr/local/bin/modelrelay-cloud
COPY --from=build /out/reprovision-server-keys /usr/local/bin/reprovision-server-keys
COPY --from=build /build/crates/modelrelay-cloud/templates/ /app/templates/
EXPOSE 8000
CMD ["modelrelay-cloud"]

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc AS server
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/modelrelay-server /usr/local/bin/modelrelay-server
COPY --from=build /out/modelrelay-worker /usr/local/bin/modelrelay-worker
EXPOSE 8080
CMD ["modelrelay-server"]
