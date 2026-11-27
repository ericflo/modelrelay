# API Reference (`api.modelrelay.io`)

ModelRelay exposes a single OpenAI- and Anthropic-compatible HTTPS endpoint — **`https://api.modelrelay.io`** (or your self-hosted relay URL) — backed by your connected WebSocket workers.

## Authentication

Send your ModelRelay API key in either header format:

- `Authorization: Bearer <MODELRELAY_API_KEY>` (OpenAI Chat Completions, Responses, `/v1/models`)
- `x-api-key: <MODELRELAY_API_KEY>` (Anthropic Messages `/v1/messages`)

## Endpoints

| Method & Path | Protocol | Notes |
|---|---|---|
| `GET /v1/models` | OpenAI model list | Live models advertised by your connected workers |
| `POST /v1/chat/completions` | OpenAI Chat Completions | Unary JSON or unbuffered SSE streaming (`"stream": true`) |
| `POST /v1/responses` | OpenAI Responses API | Supports `input` and SSE event pass-through |
| `POST /v1/messages` | Anthropic Messages API | Send `x-api-key` and `anthropic-version: 2023-06-01` |
| `GET /health` | Relay health | Unauthenticated JSON status (`status`, `version`, `uptime_secs`) |

## `curl` Examples

### List live models

```bash
curl https://api.modelrelay.io/v1/models \
  -H "Authorization: Bearer $MODELRELAY_API_KEY"
```

### Chat Completions

```bash
curl https://api.modelrelay.io/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{
    "model": "qwen3.5-27b",
    "messages": [{"role": "user", "content": "Explain WebSockets in one sentence."}]
  }'
```

### Streaming Chat Completions (SSE)

Pass `-N` to disable curl buffering. Pressing `Ctrl+C` drops the HTTP connection and propagates a `Cancel` message over WebSocket to stop generation on the worker:

```bash
curl -N https://api.modelrelay.io/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{
    "model": "qwen3.5-27b",
    "stream": true,
    "messages": [{"role": "user", "content": "Write a haiku about local GPUs."}]
  }'
```

### OpenAI Responses API

```bash
curl https://api.modelrelay.io/v1/responses \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $MODELRELAY_API_KEY" \
  -d '{
    "model": "qwen3.5-27b",
    "input": "List three advantages of local inference."
  }'
```

### Anthropic Messages API

```bash
curl https://api.modelrelay.io/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: $MODELRELAY_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "qwen3.5-27b",
    "max_tokens": 256,
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

## Routing & Status Codes

- Model selection reads the JSON `"model"` field (or falls back to an `X-Model` header).
- Optional worker-group routing accepts `X-Provider: <name>` or `"provider": "<name>"`.
- `400` missing `model`, `401` invalid/missing API key, `502` local backend error, `503` no eligible worker or queue full, `504` queue/backend timeout.
