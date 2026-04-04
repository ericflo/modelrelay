# Architecture

This document describes the internal architecture of ModelRelay: how the components fit together, how data flows through the system, and why key design decisions were made. It is intended for contributors, operators, and anyone evaluating ModelRelay for their own infrastructure.

## Workspace Shape

- `crates/proxy-contract-tests`
  Black-box behavior tests and focused harnesses for registration, queueing, response streaming, cancellation, requeue, heartbeat, and graceful shutdown semantics.

- `crates/worker-protocol`
  Shared Rust protocol types for the WebSocket bridge: registration, dispatch, streaming chunks, cancellation, heartbeats, and operational control messages.

- `crates/modelrelay-server`
  Central HTTP proxy. Owns the client-facing OpenAI and Anthropic compatibility layers, worker auth, provider config, worker registry, queueing, routing, cancellation, and graceful drain.

- `crates/modelrelay-worker`
  Remote worker process. Authenticates to the server, advertises models and capacity, forwards requests to a local backend such as `llama-server`, streams chunks back, refreshes advertised models, reports live load in heartbeats, and honors cancellation plus graceful shutdown.

## Component Overview

```
                    ┌─────────────────────────────────────────────────────────┐
                    │                  modelrelay-server                      │
                    │                                                         │
  HTTP clients      │  ┌───────────┐    ┌──────────────┐    ┌─────────────┐  │  WebSocket
  ─────────────────►│  │ HTTP      │───►│ Queue        │───►│ Dispatcher  │  │◄──────────
  /v1/chat/         │  │ Router    │    │ Manager      │    │             │  │  workers
  completions,      │  │           │    │ (per-provider│    │ (load-aware │  │  connect in
  /v1/messages,     │  │ (axum     │    │  FIFO)       │    │  round-     │  │
  /v1/responses     │  │  routes)  │    │              │    │  robin)     │  │
                    │  └───────────┘    └──────────────┘    └──────┬──────┘  │
                    │        │                                     │         │
                    │        │          ┌──────────────┐           │         │
                    │        │          │ Worker       │◄──────────┘         │
                    │        │          │ Registry     │                     │
                    │        │          │ (auth, model │                     │
                    │        │          │  tracking,   │                     │
                    │        │          │  load, drain)│                     │
                    │        │          └──────────────┘                     │
                    │        │                                               │
                    │        ▼                                               │
                    │  ┌───────────┐    ┌──────────────┐                    │
                    │  │ Cancel    │    │ WebSocket    │                    │
                    │  │ Guard     │    │ Hub          │                    │
                    │  │ (RAII     │    │ (per-worker  │                    │
                    │  │  drop)    │    │  message     │                    │
                    │  │           │    │  routing)    │                    │
                    │  └───────────┘    └──────────────┘                    │
                    └─────────────────────────────────────────────────────────┘
```

**HTTP Router** (`http.rs`) — axum-based handler for four client-facing routes plus the worker WebSocket upgrade endpoint. Parses the model name and streaming flag from the request body, submits to the core, and bridges the response back as either a single body or an SSE stream.

**Queue Manager** (`lib.rs`) — per-provider FIFO queue with configurable max length and timeout. Requests land here when no worker with capacity is immediately available. The queue is drained oldest-first whenever a worker finishes a request or a new worker registers.

**Dispatcher** (`lib.rs`) — selects the best worker for a request. Filters by provider, model support, capacity, and drain state, then picks the lowest-load worker with round-robin tie-breaking via per-provider cursors.

**Worker Registry** (`lib.rs`) — tracks every connected worker's identity, supported models, max concurrency, reported load, in-flight request set, and drain state. Updated by registration, heartbeat pongs, model refreshes, and disconnect events.

**WebSocket Hub** (`worker_socket.rs`) — manages the authenticated WebSocket connection for each worker. Routes server-to-worker messages (request dispatch, cancel signals, pings, graceful shutdown, model refresh) and worker-to-server messages (response chunks, completions, pongs, model updates, errors).

**Cancel Guard** (`http.rs`) — an RAII `HttpRequestCancellationGuard` that fires if the HTTP response future is dropped (client disconnect or timeout). On drop, it broadcasts a cancel signal through the core to the assigned worker.

## Worker Daemon Internals

```
  ┌─────────────────────────────────────────────────────────┐
  │                  modelrelay-worker                       │
  │                                                          │
  │  ┌──────────────┐          ┌─────────────────────────┐  │
  │  │ Connection   │          │ Request Tasks           │  │
  │  │ Manager      │          │                         │  │
  │  │              │  spawn   │  ┌───────┐ ┌───────┐    │  │
  │  │ • connect    │─────────►│  │ Req 1 │ │ Req 2 │... │  │
  │  │ • register   │          │  │       │ │       │    │  │
  │  │ • reconnect  │◄─────────│  │ POST  │ │ POST  │    │  │
  │  │   (exp.      │  events  │  │ to    │ │ to    │    │  │
  │  │   backoff)   │          │  │ local │ │ local │    │  │
  │  └──────┬───────┘          │  └───┬───┘ └───┬───┘    │  │
  │         │                  └──────┼─────────┼────────┘  │
  │         │                         │         │           │
  │         ▼                         ▼         ▼           │
  │  ┌──────────────┐          ┌─────────────────────┐      │
  │  │ Socket Loop  │          │ Local Backend       │      │
  │  │ (select!)    │          │ (llama-server,      │      │
  │  │              │          │  Ollama, vLLM, etc) │      │
  │  │ • read msgs  │          └─────────────────────┘      │
  │  │ • send msgs  │                                       │
  │  │ • heartbeat  │                                       │
  │  └──────────────┘                                       │
  └─────────────────────────────────────────────────────────┘
```

The worker daemon runs a single `select!` loop that multiplexes:

1. **Inbound WebSocket messages** — dispatched to `handle_server_message()` which routes each message type: spawns a task for `Request`, responds to `Ping`, applies `Cancel` to active tasks, triggers model refresh, or begins graceful drain.

2. **Outbound events from request tasks** — each spawned request task communicates back through an mpsc channel. `ResponseChunk` events are forwarded immediately over the WebSocket. `RequestFinished` and `RequestFailed` events trigger cleanup.

3. **Reconnection with exponential backoff** — on unexpected disconnect, the outer `run_with_reconnect()` loop retries with 1–30 second backoff plus up to 500ms jitter. Only a `GracefulShutdown` message causes a clean exit.

## Data Flow: Non-Streaming Request

```
  Client                    Server                     Worker              Backend
    │                         │                          │                    │
    │  POST /v1/chat/         │                          │                    │
    │  completions            │                          │                    │
    │────────────────────────►│                          │                    │
    │                         │  find_eligible_worker()  │                    │
    │                         │  assign_to_worker()      │                    │
    │                         │                          │                    │
    │                         │  WS: Request{id,body}    │                    │
    │                         │─────────────────────────►│                    │
    │                         │                          │  POST /v1/chat/    │
    │                         │                          │  completions       │
    │                         │                          │───────────────────►│
    │                         │                          │                    │
    │                         │                          │◄───────────────────│
    │                         │                          │  200 {response}    │
    │                         │  WS: ResponseComplete    │                    │
    │                         │  {id, 200, body}         │                    │
    │                         │◄─────────────────────────│                    │
    │                         │                          │                    │
    │  200 {response}         │  finish_request()        │                    │
    │◄────────────────────────│  dispatch_next_compat()  │                    │
    │                         │                          │                    │
```

## Data Flow: Streaming Request

```
  Client                    Server                     Worker              Backend
    │                         │                          │                    │
    │  POST /v1/chat/         │                          │                    │
    │  completions            │                          │                    │
    │  stream: true           │                          │                    │
    │────────────────────────►│                          │                    │
    │                         │  WS: Request{id,body,    │                    │
    │                         │    is_streaming: true}    │                    │
    │                         │─────────────────────────►│                    │
    │                         │                          │  POST to backend   │
    │                         │                          │───────────────────►│
    │                         │                          │                    │
    │  SSE: data: chunk1      │  WS: ResponseChunk       │◄── chunk 1 ───────│
    │◄────────────────────────│◄─────────────────────────│                    │
    │  SSE: data: chunk2      │  WS: ResponseChunk       │◄── chunk 2 ───────│
    │◄────────────────────────│◄─────────────────────────│                    │
    │  SSE: data: chunk3      │  WS: ResponseChunk       │◄── chunk 3 ───────│
    │◄────────────────────────│◄─────────────────────────│                    │
    │                         │                          │                    │
    │  SSE: [DONE]            │  WS: ResponseComplete    │◄── end ───────────│
    │◄────────────────────────│◄─────────────────────────│                    │
    │                         │  finish_request()        │                    │
```

Streaming chunks flow through three hops with minimal buffering: the worker reads chunks from the backend HTTP response body as they arrive, wraps each in a `ResponseChunk` WebSocket message, the server receives it and pushes it into an mpsc channel, and the HTTP handler yields it as an SSE event to the client.

## Data Flow: Client Cancellation

```
  Client                    Server                     Worker              Backend
    │                         │                          │                    │
    │  POST /v1/chat/...      │  WS: Request             │  POST to backend   │
    │────────────────────────►│─────────────────────────►│───────────────────►│
    │                         │                          │                    │
    │  [client disconnects]   │                          │  ◄── streaming ──  │
    │─ ─ ─ ─ ─X               │                          │                    │
    │                         │  CancellationGuard drop  │                    │
    │                         │  cancel_request(id)      │                    │
    │                         │                          │                    │
    │                         │  WS: Cancel{id}          │                    │
    │                         │─────────────────────────►│                    │
    │                         │                          │  [abort request    │
    │                         │                          │   task]            │
    │                         │                          │───── abort ───────►│
```

The RAII `HttpRequestCancellationGuard` is the key mechanism. When the HTTP response future is dropped — either because the client disconnected or a server-side timeout fired — the guard's `Drop` implementation spawns an async task that calls `cancel_request()`. If the request is still queued, it is removed immediately. If it is in-flight with a worker, a `Cancel` message is sent over the WebSocket, and the worker aborts the corresponding request task.

## Worker Lifecycle State Machine

```
                    ┌────────────┐
                    │ Connecting │
                    │            │
                    │ WS handshake
                    │ + auth     │
                    └─────┬──────┘
                          │ RegisterAck
                          ▼
                    ┌────────────┐
              ┌────►│    Idle    │◄───────────────┐
              │     │            │                 │
              │     │ capacity > 0                 │
              │     │ no in-flight                 │
              │     └─────┬──────┘                 │
              │           │ Request dispatched     │ request finished
              │           ▼                        │ (and more capacity)
              │     ┌────────────┐                 │
              │     │    Busy    │─────────────────┘
              │     │            │
              │     │ in-flight  │
              │     │ requests > 0                 ┌────────────┐
              │     └─────┬──────┘                 │    Gone    │
              │           │ GracefulShutdown        │            │
              │           ▼                        │ disconnected
              │     ┌────────────┐                 │ or shutdown │
              │     │  Draining  │────────────────►│ complete   │
              │     │            │  all in-flight   └────────────┘
              │     │ no new     │  finished or          ▲
              │     │ requests   │  drain timeout        │
              │     └────────────┘                       │
              │                                          │
              └──────────────────────────────────────────┘
                        unexpected disconnect
                        (triggers reconnect in worker daemon)
```

**Connecting** — WebSocket handshake in progress. The worker sends `x-worker-secret` in the upgrade request for authentication and the provider name as a query parameter.

**Idle** — registered and waiting for work. The worker has capacity (reported load < max concurrency) and no in-flight requests. The server may dispatch requests to it.

**Busy** — processing one or more requests. The worker still accepts new requests up to its max concurrency. Each in-flight request is tracked independently.

**Draining** — the server sent `GracefulShutdown`. No new requests are dispatched. Existing in-flight requests are allowed to complete up to an optional drain timeout. Once all requests finish (or the timeout expires), the worker transitions to Gone.

**Gone** — the worker is removed from the registry. In-flight requests are requeued (up to 3 attempts per request) or failed if already cancelled. The worker daemon's reconnect loop may bring it back as a new Connecting session.

## Request Lifecycle State Machine

```
  ┌────────────┐
  │  Received  │
  │            │
  │ HTTP req   │
  │ parsed     │
  └─────┬──────┘
        │
        ├── eligible worker found ──────────┐
        │                                    │
        ▼                                    ▼
  ┌────────────┐                      ┌────────────────┐
  │   Queued   │                      │  Dispatched    │
  │            │──── worker becomes──►│                │
  │ in provider│     available        │ assigned to    │
  │ FIFO queue │                      │ worker, WS msg │
  └─────┬──┬───┘                      │ sent           │
        │  │                          └───────┬────────┘
        │  │                                  │
        │  │ queue timeout    ┌───────────────┤
        │  │ or queue full    │               │
        │  ▼                  │               │ is_streaming
  ┌────────────┐              │               ▼
  │   Failed   │              │         ┌────────────┐
  │            │◄─────────────┤         │ Streaming  │
  │ • QueueFull│  worker dies │         │            │
  │ • Timeout  │  (requeue    │         │ chunks     │
  │ • NoWorkers│   exhausted) │         │ forwarded  │
  │ • Cancelled│              │         │ via mpsc   │
  └────────────┘              │         └─────┬──────┘
        ▲                     │               │
        │                     │               │
        │   cancel signal     │               │
        │   (client disconnect│               │
        │    or timeout)      │               │
        │                     │               ▼
  ┌────────────┐              │         ┌────────────┐
  │ Cancelled  │◄─────────────┤         │    Done    │
  │            │              │         │            │
  │ cancel     │◄─────────────┘         │ Response   │
  │ propagated │                        │ Complete   │
  │ to worker  │                        │ sent to    │
  └────────────┘                        │ client     │
                                        └────────────┘
```

A request can be cancelled at any point: if still queued, it is removed from the queue immediately. If dispatched or streaming, a `Cancel` message is sent to the worker. After a request finishes (Done, Failed, or Cancelled), the dispatcher checks whether the now-free worker can pick up the next queued request for a compatible model.

## Protocol Messages

All messages are JSON over WebSocket. The protocol is defined in the `modelrelay-protocol` crate and uses serde's tagged enum representation (`"type": "message_type"`).

**Server → Worker:**

| Message | Purpose |
|---------|---------|
| `RegisterAck` | Confirms registration, assigns worker ID, echoes accepted models |
| `Request` | Dispatches an inference request (id, model, endpoint, body, headers, streaming flag) |
| `Cancel` | Cancels an in-flight request with a reason |
| `Ping` | Heartbeat probe with optional timestamp |
| `GracefulShutdown` | Initiates drain with optional reason and timeout |
| `ModelsRefresh` | Asks worker to re-query its backend for available models |

**Worker → Server:**

| Message | Purpose |
|---------|---------|
| `Register` | Initial registration with name, models, max concurrency, load |
| `ModelsUpdate` | Updated model list and current load (after refresh or change) |
| `ResponseChunk` | One chunk of a streaming response |
| `ResponseComplete` | Final response with status code, headers, and optional body |
| `Pong` | Heartbeat response echoing timestamp plus current load |
| `Error` | Error report, optionally scoped to a specific request |

## Key Design Decisions

### Why the queue lives at the center

Queueing at each worker would require clients to retry across workers or implement their own load balancing. Central queueing means one place manages fairness, timeout policy, and capacity-aware routing. When a worker finishes a request, the server immediately checks the queue for the next compatible request — no external coordination needed.

### Why WebSocket instead of gRPC

Workers connect *out* to the server. This is the fundamental topology: GPU boxes on home networks, behind NATs, with no inbound ports. WebSocket over HTTP works through every proxy and firewall. gRPC would add a proto compilation step, a heavier runtime dependency, and more complex connection management for no meaningful benefit in a system where the message vocabulary is small and the payload is mostly opaque passthrough.

### Why the protocol is flat JSON

The protocol has ~12 message types. Each is a small JSON object with a `"type"` tag. There is no binary framing, no schema negotiation, no version handshake beyond a simple `protocol_version` field. This makes debugging trivial (read the WebSocket frames), keeps the protocol crate minimal, and means any language can implement a worker in an afternoon. The heavy payload (inference request bodies and response chunks) is opaque text passed through without parsing.

### Why streaming is chunked SSE, not buffered

LLM inference can take seconds to minutes. Buffering the full response before sending it to the client would destroy the interactive experience. ModelRelay preserves streaming semantics end-to-end: the worker reads chunks from the backend as they arrive, wraps each in a `ResponseChunk` message, and the server yields each as an SSE event. The client sees tokens arrive in real time, identical to talking directly to the backend.

### Why cancellation is RAII-based

Client disconnects are the normal case, not an exception. When a user closes a tab or ctrl-C's a curl command, the HTTP response future is dropped. Rust's ownership model makes this the natural place to trigger cleanup: the `HttpRequestCancellationGuard` fires on drop, propagates the cancel through the server core to the worker, and the worker aborts the backend request. No polling, no timers, no forgotten cleanup paths.

### Why requeue has a cap of 3

When a worker dies mid-request, the server requeues the request to another worker. But if workers keep dying (bad model, OOM, hardware failure), infinite requeue would loop forever. Three attempts is enough to survive transient worker restarts without masking systemic failures.

## Capacity and Scaling

### What limits the server

The server is single-process, async (tokio). The practical limits are:

- **Connected workers**: bounded by memory for the worker registry and WebSocket connections. Thousands of workers are feasible.
- **Queue depth**: configurable per provider (`max_queue_len`). Memory cost is proportional to queued request bodies.
- **Concurrent in-flight requests**: bounded by the sum of all workers' `max_concurrent` values. Each in-flight request holds a small state record and channel handles.
- **Streaming throughput**: chunks flow through an mpsc channel per request. The server does minimal processing per chunk (no parsing, no transformation), so throughput scales with I/O.

### What limits a worker

Each worker is bounded by its local backend's capacity. The `max_concurrent` setting should match what the backend can handle (e.g., llama-server's `-np` parallel slots). The worker itself adds negligible overhead — it is a thin forwarding layer.

### What the queue cannot do

- **Priority**: the queue is FIFO per provider. There is no request priority mechanism.
- **Cross-provider routing**: a request targets one provider. There is no fallback to a different provider if the primary queue is full.
- **Persistence**: the queue is in-memory. If the server restarts, queued requests are lost. In-flight requests fail and clients retry.

### Scaling patterns

- **Vertical**: increase `max_concurrent` on workers with more GPU memory or faster hardware.
- **Horizontal**: add more workers. The server's round-robin dispatcher spreads load automatically.
- **Multi-server**: not built in. For HA, run multiple server instances behind a load balancer, but each server maintains its own worker pool and queue (no shared state). Workers can connect to multiple servers for redundancy.

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
