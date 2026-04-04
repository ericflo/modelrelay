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

The project is complete and ready for production use. The full behavior matrix is implemented and verified by an extensive automated test suite covering:

- OpenAI chat/completions and responses flows
- Anthropic messages flows
- Queueing and timeout behavior
- Streaming pass-through with preserved ordering and termination
- Client cancellation propagation through the WebSocket link
- Worker disconnect and automatic requeue
- Heartbeat and live-load reporting
- Model refresh and auth cooldown recovery
- Graceful shutdown and drain semantics

A multi-stage Dockerfile and docker-compose example are provided for quick setup without a Rust toolchain.
