# Architecture

This is the planned Rust split for the extracted standalone project. It is intentionally narrower and cleaner than the Katamari package layout while preserving the public behavior.

## Planned Workspace Shape

- `crates/proxy-contract-tests`
  Black-box behavior tests and test fixtures. This stays lightweight and grows first.

- `crates/worker-protocol`
  Shared Rust protocol types for the WebSocket bridge: registration, dispatch, streaming chunks, cancellation, heartbeats, and operational control messages.

- `crates/proxy-server`
  Central HTTP proxy. Responsible for client-facing compatibility layers, auth, provider config, worker registry, queueing, routing, cancellation, and graceful drain.

- `crates/worker-daemon`
  Remote worker process. Responsible for authenticating to the server, advertising models and capacity, forwarding requests to a local backend such as `llama-server`, streaming chunks back, and honoring cancellation.

## Design Constraints

- The HTTP boundary should look normal to clients; the worker protocol can stay private and purpose-built.
- Queueing belongs at the central server, not at each worker.
- Streaming and cancellation are first-class concerns, not add-ons.
- The Rust rewrite should preserve behavior, not Go package boundaries.
- Early implementation should optimize for testability and explicit state transitions over abstraction depth.

## Early Implementation Order

1. Lock the contract with black-box tests in `proxy-contract-tests`.
2. Introduce the shared worker protocol crate. This workspace now includes `crates/worker-protocol` for the bridge message schema.
3. Build an in-memory proxy-server core that can satisfy the contract tests without real sockets.
4. Add transport adapters for real WebSocket and HTTP boundaries.
5. Add the worker daemon and local-backend integration.

## Questions Deferred On Purpose

- Final public crate naming beyond this bootstrap.
- Persistence model for provider and worker metadata.
- Metrics, tracing, and admin API details.
- Packaging and release workflow.

Those matter later, but they should follow green contract tests rather than precede them.
