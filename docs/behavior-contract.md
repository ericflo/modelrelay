# Behavior Contract

This document captures the externally observable contract to preserve when rewriting Katamari's worker-proxy system in Rust.

Source of truth:

- Repository: `ericflo/katamari`
- Commit: `ab5e90f6a2ff05a063663ce478146bf0b6829429`
- Primary files inspected:
  - `platform/aiproxyd/internal/worker/messages.go`
  - `platform/aiproxyd/internal/worker/connection.go`
  - `platform/aiproxyd/internal/worker/manager.go`
  - `platform/aiproxyd/internal/worker/queue.go`
  - `platform/aiproxyd/internal/worker/integration_test.go`
  - `platform/aiproxyd/internal/proxy/handler.go`
  - `libs/aiproxy-worker-protocol/protocol.go`

## Core Contract

- Worker auth and registration:
  Workers connect to `/v1/worker/connect?provider=<name>` over WebSocket and authenticate with a provider-specific worker secret. The preferred transport is `X-Worker-Secret`; query-string secret fallback exists only for backward compatibility. Secret comparison is constant-time. Unknown providers are rejected, disabled providers are rejected, and repeated failed auth attempts are rate-limited by client IP.

- Capability advertisement:
  After connect, the worker sends a `register` message containing `worker_name`, `models`, `max_concurrent`, and `protocol_version`. The server may sanitize or truncate these values and must send `register_ack` with the accepted worker ID, accepted model list, and warnings. Legacy workers omitting `protocol_version` are tolerated in Katamari unless explicitly rejected by config; mismatched protocol versions are closed with a protocol error.
  The first Rust characterization harness makes that sanitization concrete by requiring the acked model list to trim whitespace, drop empty entries, de-duplicate exact duplicates while preserving first-seen order, and cap the accepted list at a provider-defined limit with warnings surfaced in `register_ack`.

- Model advertisement and worker selection:
  Workers advertise exact model names, and the server routes only to workers that explicitly support the requested model. Katamari keeps an O(1) model-membership set per worker. Selection is "lowest load with round-robin tie breaking" among workers that support the model and can atomically reserve capacity.

- Queueing when no worker is immediately available:
  If no eligible worker can accept the request, the request is queued per virtual provider. The queue is bounded and FIFO among requests compatible with a worker's model list. Requests remain keyed by original queue time so requeue does not grant infinite timeout extensions.

- Request dispatch over WebSocket:
  Requests are forwarded to workers as `request` messages with `request_id`, `model`, raw JSON body string, selected headers, target endpoint path, and `is_streaming`. The central proxy accepts ordinary provider-style HTTP requests and delegates only the worker-backed providers through this path.

- Non-streaming response pass-through:
  Workers reply with `response_complete` containing the final HTTP status, response headers, full body, and token counts. The proxy must forward status, headers, and body faithfully, including upstream 4xx and 5xx responses, rather than collapsing them into generic proxy errors.

- Streaming chunk ordering and termination semantics:
  Streaming responses are forwarded as `response_chunk` messages containing already-formatted SSE data and finish with `response_complete`. Chunks must preserve order. The HTTP side must flush promptly, retain streaming semantics, and treat completion metadata as the source of final status and token accounting. Katamari enforces a cumulative streaming size ceiling and emits an SSE error before terminating an oversized stream.

- Client cancellation propagation end to end:
  Client disconnect or request timeout must cancel the HTTP request context, remove queued work if still queued, or send a best-effort `cancel` message for active worker requests. Late chunks that arrive after cancellation are intentionally dropped. The worker protocol has explicit cancel reasons, including client disconnect and timeout.

- Worker disconnect during active request:
  On worker disconnect, active requests are examined one by one. If the request context is still alive, Katamari requeues it onto the provider queue without resetting its lifetime. If the request context is already canceled or timed out, the request fails immediately to the waiting client path instead. Requeue is capped at `MaxRequeueCount = 3`; after that the request fails with a service-unavailable style error instead of looping forever.

- Timeout behavior:
  Every provider has a request timeout used both for queue wait and overall request lifetime. Queue timeout produces a worker-unavailable style response. Streaming and non-streaming requests share the parent HTTP context, so client disconnect and timeout terminate the same request object. WebSocket heartbeats use ping every 15 seconds and a 45-second pong window.

- Queue-full, no-workers, and provider-disabled error surfaces:
  Katamari distinguishes bounded queue exhaustion, no worker capacity, disabled providers, deleted providers, timeout, and requeue exhaustion through dedicated error values. The public-facing HTTP layer currently sanitizes some internal errors into stable client messages such as "Service temporarily at capacity" and "Provider is currently disabled".

- Heartbeat, load reporting, and stale-worker cleanup:
  The server sends JSON `ping`; workers reply with JSON `pong` carrying current load. This heartbeat updates `last_heartbeat` and live load accounting. Workers may also send `models_update` when their local model catalog changes. Stale worker DB records are cleaned periodically, and failed auth rate-limit entries expire automatically.

- Graceful shutdown and drain semantics:
  The server can send `graceful_shutdown` to tell workers to stop accepting new work, finish current requests, and disconnect before a timeout. Provider deletion drains queued requests with an explicit provider-deleted error and closes connected workers.

- OpenAI-style and Anthropic-style compatibility:
  The central server is meant to accept ordinary client traffic, not a custom client. Katamari parses model and stream flags from OpenAI-style request bodies, provides a special `/v1/models` compatibility endpoint, and preserves SSE behavior expected by OpenAI-compatible tooling. The extracted Rust project should also preserve Anthropic-style compatibility at the central HTTP boundary even if the internal worker protocol stays provider-neutral.

## Wire Messages To Preserve

- Server to worker:
  `ping`, `request`, `register_ack`, `cancel`, `graceful_shutdown`, `models_refresh`

- Worker to server:
  `pong`, `register`, `models_update`, `response_chunk`, `response_complete`, `error`

## Invariants Worth Preserving

- A worker never silently gains capability beyond the sanitized models acknowledged by the server.
- Queueing is bounded per provider and does not grow without limit.
- Requeue is intentional and finite.
- HTTP error bodies from the worker backend are preserved where safe instead of flattened away.
- Streaming remains SSE-shaped end to end.
- Worker churn or late chunks must not leave requests hanging forever.

## First Characterization Tests To Write Next

Ordered by leverage:

1. Dynamic model catalog updates and `/v1/models` coherence:
   Worker `models_update` messages should immediately change routing eligibility and the public compatibility model catalog without requiring reconnects or serving stale `/v1/models` results.
