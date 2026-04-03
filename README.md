# llm-worker-proxy

`llm-worker-proxy` is a central HTTP LLM proxy with authenticated remote workers over WebSockets.

The target shape is operationally simple: clients talk to one stable OpenAI-style or Anthropic-style endpoint, workers connect in from heterogeneous GPU boxes, and the server handles routing, queueing, streaming, cancellation, and worker churn in one place.

This repository is deliberately in the phase-0 bootstrap state. The immediate job is to lock down the Katamari worker-proxy behavior as a public contract and build the Rust test harness that future characterization tests will use.

## Current Scope

- Katamari behavior contract captured from commit `ab5e90f6a2ff05a063663ce478146bf0b6829429`
- Minimal Rust workspace and CI for tests-first development
- Contract test crate plus a shared `worker-protocol` crate defining the bridge message schema

## Documents

- [Behavior contract](docs/behavior-contract.md)
- [Architecture sketch](docs/architecture.md)

## Validation

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```
