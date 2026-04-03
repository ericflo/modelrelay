# llm-worker-proxy

`llm-worker-proxy` is a central HTTP LLM proxy with authenticated remote workers over WebSockets.

The target shape is operationally simple: clients talk to one stable OpenAI-style or Anthropic-style endpoint, workers connect in from heterogeneous GPU boxes, and the server handles routing, queueing, streaming, cancellation, and worker churn in one place.

This repository already ships the core worker-backed proxy path end to end: authenticated worker WebSocket registration, OpenAI-compatible and Anthropic-compatible HTTP entry points, queueing and redispatch, streaming pass-through, cancellation, heartbeat/load tracking, model refresh, auth cooldown recovery, and graceful worker drain.

## Current Scope

- Katamari behavior contract captured from commit `ab5e90f6a2ff05a063663ce478146bf0b6829429`
- Rust workspace split across `proxy-server`, `worker-daemon`, `worker-protocol`, and black-box contract tests
- Live worker WebSocket bridge with authenticated registration, heartbeats, load reporting, model refresh, and graceful shutdown control messages
- Client-facing compatibility coverage for OpenAI `GET /v1/models`, `POST /v1/chat/completions`, `POST /v1/responses`, and Anthropic `POST /v1/messages`
- Queueing, queue timeout/queue-full handling, cancellation propagation, worker disconnect requeue, and requeue exhaustion behavior covered by contract tests plus live HTTP integration tests
- Worker-daemon live coverage for forwarding local backend traffic, preserving non-2xx upstream responses, SSE streaming, client-disconnect cancellation, model catalog refresh without reconnect, auth cooldown recovery, and graceful drain while replacement workers pick up queued work

## Documents

- [Behavior contract](docs/behavior-contract.md)
- [Architecture sketch](docs/architecture.md)

## Validation

The current behavior matrix is exercised at three layers: black-box contract harnesses in `proxy-contract-tests`, live HTTP integration tests in `proxy-server`, and end-to-end live backend tests in `worker-daemon`.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```
