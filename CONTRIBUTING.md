# Contributing to llm-worker-proxy

Thanks for your interest in contributing. This guide covers what you need to get started.

## Building and testing

The project uses a pinned Rust toolchain defined in `rust-toolchain.toml`. Install it once:

```bash
rustup toolchain install 1.94.1 --component clippy rustfmt
```

Standard workflow:

```bash
# Check formatting
cargo +1.94.1 fmt --check

# Run lints (CI enforces zero warnings)
cargo +1.94.1 clippy --workspace --all-targets --all-features -- -D warnings

# Run all tests
cargo +1.94.1 test --workspace
```

You can also use the default toolchain if it matches the pinned version — `rustup show` will confirm.

To build release binaries:

```bash
cargo build --release
# target/release/proxy-server
# target/release/worker-daemon
```

## Rust toolchain

The toolchain is pinned to `1.94.1` via `rust-toolchain.toml`. CI runs `cargo fmt --check` and `cargo clippy ... -D warnings` against that exact version, so please run them locally before pushing to avoid avoidable CI failures.

## Branch naming

Use the `ce/` prefix for agent-generated branches. Human contributors can use any reasonable convention, e.g. `feat/`, `fix/`, `docs/`.

## PR expectations

- **One logical change per PR.** Large changes are harder to review and easier to revert; prefer small focused PRs.
- **Tests required for new behavior.** Protocol semantics, routing logic, cancellation paths, and error surfaces should each have a test. See `crates/proxy-contract-tests/` for integration test examples.
- **Formatting and lints must pass.** Run `cargo fmt` and `cargo clippy -- -D warnings` before pushing.
- **Keep the description brief and honest.** Say what changed and why.

## Code layout

```
crates/
  proxy-server/        Central HTTP server — request intake, queueing, routing
  worker-daemon/       Worker side — connects to proxy, forwards to local backend
  worker-protocol/     Shared WebSocket message types
  proxy-contract-tests/  Integration tests for the full server+worker stack
examples/              Shell scripts showing realistic startup sequences
```

## Running the integration tests

The contract tests spin up a real proxy server and worker in-process:

```bash
cargo +1.94.1 test --package proxy-contract-tests
```

Individual tests can be filtered with `-- <pattern>`:

```bash
cargo +1.94.1 test --package proxy-contract-tests -- streaming
```

## License

MIT. By contributing you agree your changes are licensed under the same terms.
