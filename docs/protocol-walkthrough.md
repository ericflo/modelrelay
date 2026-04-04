# Protocol Walkthrough — Wire Traces

This document shows the actual message flow between components for each
major scenario.  Message types reference the structs in the
`modelrelay-protocol` crate (`ServerToWorkerMessage` / `WorkerToServerMessage`).

---

## 1. Worker Registration and Heartbeat

```
  Worker                         Proxy Server
    │                                │
    │──── WebSocket UPGRADE ────────►│  GET /v1/worker/connect
    │◄─── 101 Switching Protocols ──│
    │                                │
    │  WorkerToServerMessage::Register
    │  {                             │
    │    "type": "register",         │
    │    "worker_name": "gpu-box-1", │
    │    "models": ["llama3-8b"],    │
    │    "max_concurrent": 4,        │
    │    "protocol_version": "1",    │
    │    "current_load": 0           │
    │  }                             │
    │──────────────────────────────►│  Proxy validates worker_secret
    │                                │  (passed as query param or header
    │                                │   during WebSocket upgrade)
    │                                │
    │  ServerToWorkerMessage::RegisterAck
    │  {                             │
    │    "type": "register_ack",     │
    │    "worker_id": "w-a1b2c3",   │
    │    "models": ["llama3-8b"],    │
    │    "protocol_version": "1"     │
    │  }                             │
    │◄──────────────────────────────│
    │                                │
    │         ┌──── heartbeat loop (HEARTBEAT_INTERVAL) ────┐
    │         │                      │                       │
    │  ServerToWorkerMessage::Ping   │                       │
    │  { "type": "ping",            │                       │
    │    "timestamp_unix_ms": ... }  │                       │
    │◄──────────────────────────────│                       │
    │                                │                       │
    │  WorkerToServerMessage::Pong   │                       │
    │  { "type": "pong",            │                       │
    │    "current_load": 1,          │                       │
    │    "timestamp_unix_ms": ... }  │                       │
    │──────────────────────────────►│  Proxy updates load   │
    │         └─────────────────────────────────────────────┘
```

If the worker misses heartbeats, the proxy closes the WebSocket with
reason `"worker heartbeat timed out"` and requeues any in-flight requests
(up to `MAX_REQUEUE_COUNT = 3` retries).

---

## 2. Normal Non-Streaming Request

```
  Client                 Proxy Server                Worker              Backend
    │                        │                         │                    │
    │  POST /v1/chat/completions                       │                    │
    │  {"model":"llama3-8b", │                         │                    │
    │   "stream": false,     │                         │                    │
    │   "messages":[...]}    │                         │                    │
    │───────────────────────►│                         │                    │
    │                        │                         │                    │
    │                        │  Queue lookup:           │                    │
    │                        │  provider="local",       │                    │
    │                        │  model="llama3-8b"       │                    │
    │                        │  → worker "gpu-box-1"    │                    │
    │                        │  has capacity             │                    │
    │                        │                         │                    │
    │                        │  ServerToWorkerMessage::Request              │
    │                        │  { "type": "request",   │                    │
    │                        │    "request_id": "r-001",│                   │
    │                        │    "model": "llama3-8b", │                   │
    │                        │    "endpoint_path":      │                    │
    │                        │      "/v1/chat/completions",                 │
    │                        │    "is_streaming": false, │                   │
    │                        │    "body": "{...}",      │                   │
    │                        │    "headers": {...} }    │                    │
    │                        │────────────────────────►│                    │
    │                        │                         │                    │
    │                        │                         │  POST /v1/chat/completions
    │                        │                         │───────────────────►│
    │                        │                         │                    │
    │                        │                         │◄───────────────────│
    │                        │                         │  200 OK + JSON body│
    │                        │                         │                    │
    │                        │  WorkerToServerMessage::ResponseComplete     │
    │                        │  { "type": "response_complete",             │
    │                        │    "request_id": "r-001",│                   │
    │                        │    "status_code": 200,   │                   │
    │                        │    "headers": {"content-type":              │
    │                        │      "application/json"},│                   │
    │                        │    "body": "{...}",      │                   │
    │                        │    "token_counts": {     │                   │
    │                        │      "prompt_tokens": 42,│                   │
    │                        │      "completion_tokens": 128,              │
    │                        │      "total_tokens": 170 │                   │
    │                        │    }                     │                   │
    │                        │  }                       │                   │
    │                        │◄────────────────────────│                    │
    │                        │                         │                    │
    │◄───────────────────────│  200 OK                  │                    │
    │  {"choices":[...]}     │  (body forwarded)        │                    │
```

---

## 3. Streaming Request (SSE)

```
  Client                 Proxy Server                Worker              Backend
    │                        │                         │                    │
    │  POST /v1/chat/completions                       │                    │
    │  {"stream": true, ...} │                         │                    │
    │───────────────────────►│                         │                    │
    │                        │                         │                    │
    │                        │  Request dispatched      │                    │
    │                        │  (is_streaming: true)    │                    │
    │                        │────────────────────────►│                    │
    │                        │                         │  POST (stream=true)│
    │                        │                         │───────────────────►│
    │                        │                         │                    │
    │                        │                         │  SSE: data: {...}  │
    │                        │                         │◄───────────────────│
    │                        │  WorkerToServerMessage::ResponseChunk       │
    │                        │  { "type": "response_chunk",               │
    │                        │    "request_id": "r-002",│                   │
    │                        │    "chunk": "data: {\"choices\":[...]}\n\n" │
    │                        │  }                       │                   │
    │                        │◄────────────────────────│                    │
    │  SSE: data: {...}      │                         │                    │
    │◄───────────────────────│                         │                    │
    │                        │                         │                    │
    │  ...more chunks...     │  ...more ResponseChunk..│  ...more SSE...   │
    │                        │                         │                    │
    │                        │                         │  SSE: data: [DONE] │
    │                        │                         │◄───────────────────│
    │                        │  ResponseChunk (final)   │                   │
    │                        │◄────────────────────────│                    │
    │  SSE: data: [DONE]     │                         │                    │
    │◄───────────────────────│                         │                    │
    │                        │                         │                    │
    │                        │  WorkerToServerMessage::ResponseComplete     │
    │                        │  { "type": "response_complete",             │
    │                        │    "request_id": "r-002",│                   │
    │                        │    "status_code": 200,   │                   │
    │                        │    "token_counts": {...}  │                  │
    │                        │  }                       │                   │
    │                        │◄────────────────────────│                    │
```

Chunks are forwarded byte-for-byte without re-parsing.  The proxy writes
each `ResponseChunk.chunk` directly to the HTTP response body, preserving
SSE framing intact.

---

## 4. Client Cancellation Propagation

```
  Client                 Proxy Server                Worker              Backend
    │                        │                         │                    │
    │  POST /v1/chat/completions (streaming)            │                    │
    │───────────────────────►│                         │                    │
    │                        │  → dispatched to worker  │                    │
    │                        │────────────────────────►│                    │
    │                        │                         │───────────────────►│
    │  (receiving chunks...) │                         │                    │
    │◄───────────────────────│                         │                    │
    │                        │                         │                    │
    │  CLIENT DISCONNECTS    │                         │                    │
    │──── TCP RST / close ──►│                         │                    │
    │                        │                         │                    │
    │                        │  Proxy detects drop      │                    │
    │                        │                         │                    │
    │                        │  ServerToWorkerMessage::Cancel               │
    │                        │  { "type": "cancel",     │                   │
    │                        │    "request_id": "r-002",│                   │
    │                        │    "reason":              │                   │
    │                        │      "client_disconnect"  │                  │
    │                        │  }                       │                   │
    │                        │────────────────────────►│                    │
    │                        │                         │                    │
    │                        │                         │  Worker aborts      │
    │                        │                         │  backend request    │
    │                        │                         │───── abort ────────►│
    │                        │                         │                    │
```

Cancel reasons (from `CancelReason` enum):
- `client_disconnect` — HTTP client dropped the connection
- `timeout` — request exceeded `REQUEST_TIMEOUT_SECS`
- `graceful_shutdown` — server is shutting down
- `worker_disconnect` — worker WebSocket closed unexpectedly
- `requeue_exhausted` — max requeue attempts (`MAX_REQUEUE_COUNT = 3`) exceeded
- `server_shutdown` — server process is terminating

---

## 5. Worker Disconnect Mid-Request (Requeue Path)

```
  Client                 Proxy Server                Worker              Backend
    │                        │                         │                    │
    │  POST /v1/chat/completions                       │                    │
    │───────────────────────►│                         │                    │
    │                        │  → dispatched to worker  │                    │
    │                        │────────────────────────►│                    │
    │                        │                         │                    │
    │                        │      WORKER CRASHES      │                    │
    │                        │      (WebSocket closes)  │                    │
    │                        │◄─── close frame / EOF ──│                    │
    │                        │                         ×                    │
    │                        │                         │                    │
    │                        │  requeue_count < MAX_REQUEUE_COUNT (3)?      │
    │                        │  YES → put request back in queue             │
    │                        │                         │                    │
    │                        │  ...time passes, another worker available... │
    │                        │                         │                    │
    │                        │             Worker-2    │                    │
    │                        │  ServerToWorkerMessage::Request              │
    │                        │────────────────────────►│  Worker-2          │
    │                        │                         │───────────────────►│
    │                        │                         │◄───────────────────│
    │                        │  ResponseComplete        │                   │
    │                        │◄────────────────────────│                    │
    │◄───────────────────────│  200 OK                  │                    │
    │                        │                         │                    │

  If requeue_count >= MAX_REQUEUE_COUNT (3):
    │                        │                         │
    │◄───────────────────────│  503 Service Unavailable │
    │  {"error": "requeue    │  Cancel with reason:     │
    │   attempts exhausted"} │  "requeue_exhausted"     │
```

---

## 6. Queue-Full Error

```
  Client                 Proxy Server
    │                        │
    │  POST /v1/chat/completions
    │───────────────────────►│
    │                        │
    │                        │  Queue length >= max_queue_len
    │                        │  (configured via MAX_QUEUE_LEN,
    │                        │   default: 100)
    │                        │
    │◄───────────────────────│  429 Too Many Requests
    │  {"error":             │
    │   "queue full"}        │
```

---

## 7. No Workers Available

```
  Client                 Proxy Server
    │                        │
    │  POST /v1/chat/completions
    │  {"model": "llama3-8b"}│
    │───────────────────────►│
    │                        │
    │                        │  No provider registered
    │                        │  for model "llama3-8b",
    │                        │  or no workers connected
    │                        │
    │                        │  If a provider exists but
    │                        │  no workers: request is queued
    │                        │  (will timeout after
    │                        │   QUEUE_TIMEOUT_SECS = 30)
    │                        │
    │                        │  If no provider matches at all:
    │◄───────────────────────│  404 Not Found
    │  {"error":             │
    │   "no provider for     │
    │    model llama3-8b"}   │
```

---

## 8. Graceful Shutdown / Worker Drain

```
  Proxy Server                Worker
    │                           │
    │  (admin triggers drain    │
    │   or server shutting down)│
    │                           │
    │  ServerToWorkerMessage::GracefulShutdown
    │  { "type": "graceful_shutdown",
    │    "reason": "maintenance",
    │    "drain_timeout_secs": 30
    │  }
    │──────────────────────────►│
    │                           │
    │  Worker marked is_draining│
    │  No new requests sent     │
    │                           │
    │  Worker finishes in-flight│
    │  requests normally...     │
    │                           │
    │  ResponseComplete(s)      │
    │◄──────────────────────────│
    │                           │
    │  disconnect_drained_worker_if_idle():
    │  all in-flight done?      │
    │  YES → close WebSocket    │
    │──── close frame ─────────►│
    │                           ×
```

---

## 9. Dynamic Model Update

```
  Worker                     Proxy Server
    │                           │
    │  (new model loaded or     │
    │   model removed locally)  │
    │                           │
    │  WorkerToServerMessage::ModelsUpdate
    │  { "type": "models_update",
    │    "models": ["llama3-8b",
    │               "codellama-13b"],
    │    "current_load": 1
    │  }
    │──────────────────────────►│
    │                           │  Proxy updates worker's
    │                           │  model list and routing
    │                           │
    │  (or server requests it)  │
    │                           │
    │  ServerToWorkerMessage::ModelsRefresh
    │  { "type": "models_refresh",
    │    "reason": "periodic"
    │  }
    │◄──────────────────────────│
    │                           │
    │  ModelsUpdate (response)  │
    │──────────────────────────►│
```

---

## Message Type Summary

### Server → Worker (`ServerToWorkerMessage`)

| Type | Struct | Purpose |
|------|--------|---------|
| `register_ack` | `RegisterAck` | Confirm registration, assign worker ID |
| `request` | `RequestMessage` | Dispatch an inference request |
| `cancel` | `CancelMessage` | Cancel an in-flight request |
| `ping` | `PingMessage` | Heartbeat probe |
| `graceful_shutdown` | `GracefulShutdownMessage` | Begin drain sequence |
| `models_refresh` | `ModelsRefreshMessage` | Ask worker to re-report models |

### Worker → Server (`WorkerToServerMessage`)

| Type | Struct | Purpose |
|------|--------|---------|
| `register` | `RegisterMessage` | Announce name, models, capacity |
| `models_update` | `ModelsUpdateMessage` | Update model list / load |
| `response_chunk` | `ResponseChunkMessage` | Forward a streaming chunk |
| `response_complete` | `ResponseCompleteMessage` | Signal request completion |
| `pong` | `PongMessage` | Heartbeat reply with load |
| `error` | `ErrorMessage` | Report a request-level error |
