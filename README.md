[![CI](https://github.com/ericflo/llm-worker-proxy/actions/workflows/ci.yml/badge.svg)](https://github.com/ericflo/llm-worker-proxy/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/ericflo/llm-worker-proxy/branch/main/graph/badge.svg)](https://codecov.io/gh/ericflo/llm-worker-proxy)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

# llm-worker-proxy

**Stop configuring clients for every GPU box. Workers connect out; requests route in.**

You have GPU boxes running `llama-server` (or Ollama, or vLLM, or anything OpenAI-compatible). Today you either expose each one directly — port forwarding, DNS, firewall rules — or you stick a load balancer in front that doesn't understand LLM streaming or cancellation.

`llm-worker-proxy` flips the model: a central proxy receives standard inference requests while worker daemons on your GPU boxes connect *out* to it over WebSocket. The proxy handles queueing, routing, streaming pass-through, and cancellation propagation. Clients see one stable endpoint and never need to know about your hardware.

```
  Clients (curl, Claude Code, LiteLLM, Open WebUI, ...)
         │
         │  POST /v1/chat/completions
         │  POST /v1/messages
         ▼
  ┌──────────────────┐
  │   proxy-server   │◄─── workers connect out (WebSocket)
  │   (one stable    │     no inbound ports needed on GPU boxes
  │    endpoint)     │
  └──────────────────┘
         │  routes request to best available worker
         ▼
  ┌────────┐  ┌────────┐  ┌────────┐
  │worker-1│  │worker-2│  │worker-3│
  │ llama  │  │ ollama │  │ vllm   │  ← your GPU boxes,
  │ server │  │        │  │        │    anywhere on any network
  └────────┘  └────────┘  └────────┘
```

## Who is this for?

- **Home GPU users** running local models who want a single API endpoint across multiple machines
- **Teams with on-prem hardware** that need to pool GPU capacity without a service mesh
- **Researchers** juggling models across heterogeneous boxes who are tired of updating client configs

## Why this instead of...

| Alternative | What's missing |
|---|---|
| **Pointing clients directly at llama-server** | No HA, no queue, clients must know about every box, no cancellation |
| **nginx / HAProxy** | Doesn't understand LLM streaming semantics, no queueing, no worker auth, no cancellation propagation |
| **LiteLLM / OpenRouter** | Cloud-first routing — not designed for your own private hardware calling home |

## Quickstart

### Docker (recommended)

Pre-built images are published to GitHub Container Registry on every release and main push.

```bash
# Pull the latest images
docker pull ghcr.io/ericflo/llm-worker-proxy/proxy-server:latest
docker pull ghcr.io/ericflo/llm-worker-proxy/worker-daemon:latest

# Run the proxy
docker run -p 8080:8080 \
  -e WORKER_SECRET=mysecret \
  -e LISTEN_ADDR=0.0.0.0:8080 \
  ghcr.io/ericflo/llm-worker-proxy/proxy-server:latest

# Run a worker (on a GPU box)
docker run \
  -e PROXY_URL=http://<proxy-host>:8080 \
  -e WORKER_SECRET=mysecret \
  -e BACKEND_URL=http://host.docker.internal:8000 \
  -e MODELS=llama3.2:3b \
  ghcr.io/ericflo/llm-worker-proxy/worker-daemon:latest
```

For pinned versions, replace `:latest` with a release tag (e.g. `:0.1.0`).

### With Docker Compose (easiest for local dev)

```bash
git clone https://github.com/ericflo/llm-worker-proxy.git
cd llm-worker-proxy

# Start the proxy + one worker (assumes llama-server on host port 8081)
docker compose up
```

The proxy is now listening on `http://localhost:8080`. The worker connects to it automatically and forwards requests to your backend.

### Build from source

```bash
cargo build --release
# Binaries: target/release/proxy-server  target/release/worker-daemon
```

**Start the proxy:**

```bash
./target/release/proxy-server \
  --listen 0.0.0.0:8080 \
  --worker-secret mysecret
```

**Start a worker** (on a GPU box with `llama-server` or any OpenAI-compatible backend running):

```bash
./target/release/worker-daemon \
  --proxy-url http://<proxy-host>:8080 \
  --worker-secret mysecret \
  --backend-url http://127.0.0.1:8000 \
  --models llama3.2:3b,llama3.2:1b
```

### Try it

```bash
# Non-streaming
curl http://localhost:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "llama3.2:3b",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": false
  }'

# Streaming (SSE chunks pass through from the backend)
curl http://localhost:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "llama3.2:3b",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": true
  }'
```

## Features

- **OpenAI + Anthropic compatible** — `POST /v1/chat/completions`, `POST /v1/responses`, `POST /v1/messages`, `GET /v1/models`
- **No inbound ports on GPU boxes** — workers connect out to the proxy over WebSocket
- **Request queueing** — configurable depth and timeout when all workers are busy
- **Streaming pass-through** — SSE chunks forwarded with preserved ordering and termination
- **End-to-end cancellation** — client disconnect propagates through the proxy to the worker to the backend
- **Automatic requeue** — if a worker dies mid-request, the request is requeued to another worker
- **Heartbeat and load tracking** — stale workers are cleaned up; workers report current load
- **Graceful drain** — workers can shut down while replacement workers pick up queued work
- **Model catalog refresh** — workers can update their model list without reconnecting
- **Auth cooldown recovery** — workers recover gracefully from authentication failures

## Configuration

### proxy-server

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--listen` | `LISTEN_ADDR` | `127.0.0.1:8080` | Address to listen on |
| `--worker-secret` | `WORKER_SECRET` | *(required)* | Secret workers must present to authenticate |
| `--provider` | `PROVIDER_NAME` | `local` | Provider name used for worker routing and request dispatch |
| `--max-queue-len` | `MAX_QUEUE_LEN` | `100` | Maximum number of queued requests (0 = unlimited) |
| `--queue-timeout` | `QUEUE_TIMEOUT_SECS` | `30` | Seconds before a queued request times out (0 = no timeout) |
| `--request-timeout` | `REQUEST_TIMEOUT_SECS` | `300` | Seconds before an in-flight HTTP request times out (0 = no timeout) |
| `--log-level` | `LOG_LEVEL` | `info` | Log level filter (e.g. `info`, `debug`, or `proxy_server=debug`). Overridden by `RUST_LOG` if set. |

### worker-daemon

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--proxy-url` | `PROXY_URL` | `http://127.0.0.1:8080` | Base URL of the proxy server |
| `--worker-secret` | `WORKER_SECRET` | *(required)* | Secret used to authenticate with the proxy |
| `--backend-url` | `BACKEND_URL` | `http://127.0.0.1:8000` | Base URL of the local model backend |
| `--models` | `MODELS` | `default` | Comma-separated list of model names this worker supports |
| `--provider` | `PROVIDER_NAME` | `local` | Provider name to register with on the proxy |
| `--worker-name` | `WORKER_NAME` | `worker` | Human-readable name for this worker instance |
| `--max-concurrency` | `MAX_CONCURRENCY` | `1` | Maximum number of concurrent requests this worker will handle |
| `--log-level` | `LOG_LEVEL` | `info` | Log level filter (e.g. `info`, `debug`, or `worker_daemon=debug`). Overridden by `RUST_LOG` if set. |

All flags can be passed as CLI arguments or set via the corresponding environment variable.

## Documents

- [Behavior contract](docs/behavior-contract.md) — the full specification of proxy, queue, streaming, and cancellation semantics
- [Architecture sketch](docs/architecture.md) — how the pieces fit together internally

## Validation

The behavior matrix is exercised at three layers: black-box contract harnesses in `proxy-contract-tests`, live HTTP integration tests in `proxy-server`, and end-to-end live backend tests in `worker-daemon`.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## License

MIT
