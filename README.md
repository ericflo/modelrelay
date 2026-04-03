# llm-worker-proxy

`llm-worker-proxy` is a central HTTP LLM proxy with authenticated remote workers over WebSockets.

The target shape is operationally simple: clients talk to one stable OpenAI-style or Anthropic-style endpoint, workers connect in from heterogeneous GPU boxes, and the server handles routing, queueing, streaming, cancellation, and worker churn in one place.

This repository now has the first live central-server slice: a composable Axum router that serves OpenAI-compatible `GET /v1/models` from the in-memory worker registry alongside the existing worker WebSocket transport. The immediate job remains tests-first expansion of the client-facing HTTP surface until chat, responses, messages, streaming, and cancellation are all backed by the same live router.

## Current Scope

- Katamari behavior contract captured from commit `ab5e90f6a2ff05a063663ce478146bf0b6829429`
- In-memory `proxy-server` core for worker registry, queueing, routing, cancellation, and graceful drain behavior
- Live Axum transport slices for worker WebSocket registration and OpenAI-compatible `GET /v1/models`
- Contract test crate plus a shared `worker-protocol` crate defining the bridge message schema

## Documents

- [Behavior contract](docs/behavior-contract.md)
- [Architecture sketch](docs/architecture.md)

## Validation

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p proxy-server --test http_router
cargo test --workspace
```
