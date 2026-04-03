# llm-worker-proxy

`llm-worker-proxy` is a central HTTP LLM proxy with authenticated remote workers over WebSockets.

The target shape is operationally simple: clients talk to one stable OpenAI-style or Anthropic-style endpoint, workers connect in from heterogeneous GPU boxes, and the server handles routing, queueing, streaming, cancellation, and worker churn in one place.

This repository now has an initial live central-server surface: the worker WebSocket bridge plus a client-facing OpenAI-compatible `GET /v1/models` endpoint backed by the in-memory worker registry. The immediate job remains tests-first expansion of the HTTP compatibility boundary and worker daemon behavior.

## Current Scope

- Katamari behavior contract captured from commit `ab5e90f6a2ff05a063663ce478146bf0b6829429`
- Minimal Rust workspace and CI for tests-first development
- Contract test crate plus a shared `worker-protocol` crate defining the bridge message schema
- Live `proxy-server` router slice exposing `/v1/worker/connect` and OpenAI-compatible `GET /v1/models`
- Black-box HTTP coverage that boots the router and verifies live model catalog updates and stale-entry removal

## Documents

- [Behavior contract](docs/behavior-contract.md)
- [Architecture sketch](docs/architecture.md)

## Validation

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```
