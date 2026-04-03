# Architecture

This is the current Rust split for the extracted standalone project. It stays narrower and cleaner than the Katamari package layout while preserving the public worker-proxy behavior.

## Workspace Shape

- `crates/proxy-contract-tests`
  Black-box behavior tests and focused harnesses for registration, queueing, response streaming, cancellation, requeue, heartbeat, and graceful shutdown semantics.

- `crates/worker-protocol`
  Shared Rust protocol types for the WebSocket bridge: registration, dispatch, streaming chunks, cancellation, heartbeats, and operational control messages.

- `crates/proxy-server`
  Central HTTP proxy. Owns the client-facing OpenAI and Anthropic compatibility layers, worker auth, provider config, worker registry, queueing, routing, cancellation, and graceful drain.

- `crates/worker-daemon`
  Remote worker process. Authenticates to the server, advertises models and capacity, forwards requests to a local backend such as `llama-server`, streams chunks back, refreshes advertised models, reports live load in heartbeats, and honors cancellation plus graceful shutdown.

## Design Constraints

- The HTTP boundary should look normal to clients; the worker protocol can stay private and purpose-built.
- Queueing belongs at the central server, not at each worker.
- Streaming and cancellation are first-class concerns, not add-ons.
- The Rust rewrite should preserve behavior, not Go package boundaries.
- The implementation should optimize for testability and explicit state transitions over abstraction depth.

## Current Status

- The workspace has moved past bootstrap: the in-memory proxy core, real HTTP boundary, real worker WebSocket transport, and worker daemon all exist on `main`.
- The shipped test surface already covers OpenAI chat/completions and responses flows, Anthropic messages flows, queueing and timeout behavior, streaming pass-through, cancellation propagation, worker disconnect and requeue behavior, heartbeat/load reporting, model refresh, auth cooldown recovery, and graceful shutdown/drain.
- The main remaining work is depth and hardening around the existing architecture, not proving that the transport split is viable.

## Questions Deferred On Purpose

- Final public crate naming beyond the current workspace split.
- Persistence model for provider and worker metadata.
- Metrics, tracing, and admin API details.
- Packaging and release workflow.

Those matter later, but they should follow green contract tests rather than precede them.
