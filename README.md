[![CI](https://github.com/ericflo/modelrelay/actions/workflows/ci.yml/badge.svg)](https://github.com/ericflo/modelrelay/actions/workflows/ci.yml)
[![Latest Release](https://img.shields.io/github/v/release/ericflo/modelrelay)](https://github.com/ericflo/modelrelay/releases/latest)
[![Coverage](https://codecov.io/gh/ericflo/modelrelay/branch/main/graph/badge.svg)](https://codecov.io/gh/ericflo/modelrelay)
[![crates.io](https://img.shields.io/badge/crates.io-coming%20soon-orange)](https://crates.io/crates/modelrelay-protocol)
[![Minimum Rust Version](https://img.shields.io/badge/rustc-1.94+-orange.svg)](rust-toolchain.toml)
[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

# ModelRelay

**Stop configuring clients for every GPU box. Workers connect out; requests route in.**

You have GPU boxes running `llama-server` (or Ollama, or vLLM, or anything OpenAI-compatible). Today you either expose each one directly — port forwarding, DNS, firewall rules — or you stick a load balancer in front that doesn't understand LLM streaming or cancellation.

ModelRelay flips the model: a central proxy receives standard inference requests while worker daemons on your GPU boxes connect *out* to it over WebSocket. The proxy handles queueing, routing, streaming pass-through, and cancellation propagation. Clients see one stable endpoint and never need to know about your hardware.

```
  Clients (curl, Claude Code, LiteLLM, Open WebUI, ...)
         │
         │  POST /v1/chat/completions
         │  POST /v1/messages
         ▼
  ┌──────────────────────┐
  │   modelrelay-server  │◄─── workers connect out (WebSocket)
  │   (one stable        │     no inbound ports needed on GPU boxes
  │    endpoint)         │
  └──────────────────────┘
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
docker pull ghcr.io/ericflo/modelrelay/modelrelay-server:latest
docker pull ghcr.io/ericflo/modelrelay/modelrelay-worker:latest

# Run the proxy
docker run -p 8080:8080 \
  -e WORKER_SECRET=mysecret \
  -e LISTEN_ADDR=0.0.0.0:8080 \
  ghcr.io/ericflo/modelrelay/modelrelay-server:latest

# Run a worker (on a GPU box)
docker run \
  -e PROXY_URL=http://<proxy-host>:8080 \
  -e WORKER_SECRET=mysecret \
  -e BACKEND_URL=http://host.docker.internal:8000 \
  -e MODELS=llama3.2:3b \
  ghcr.io/ericflo/modelrelay/modelrelay-worker:latest
```

For pinned versions, replace `:latest` with a release tag (e.g. `:0.1.0`).

### With Docker Compose (easiest for local dev)

```bash
git clone https://github.com/ericflo/modelrelay.git
cd modelrelay

# Start the proxy + one worker (assumes llama-server on host port 8081)
docker compose up
```

The proxy is now listening on `http://localhost:8080`. The worker connects to it automatically and forwards requests to your backend.

### Pre-built binaries

Download the latest release for your platform from the [Releases page](https://github.com/ericflo/modelrelay/releases):

| Platform | modelrelay-server | modelrelay-worker |
|----------|-------------------|-------------------|
| Linux x86_64 | `modelrelay-server-linux-amd64` | `modelrelay-worker-linux-amd64` |
| Linux arm64 | `modelrelay-server-linux-arm64` | `modelrelay-worker-linux-arm64` |
| macOS Intel | `modelrelay-server-darwin-amd64` | `modelrelay-worker-darwin-amd64` |
| macOS Apple Silicon | `modelrelay-server-darwin-arm64` | `modelrelay-worker-darwin-arm64` |
| Windows x86_64 | `modelrelay-server-windows-amd64.exe` | `modelrelay-worker-windows-amd64.exe` |
| Windows arm64 | `modelrelay-server-windows-arm64.exe` | `modelrelay-worker-windows-arm64.exe` |

### From crates.io

> **Note:** The crates are not yet published to crates.io. Use [pre-built binaries](#pre-built-binaries) or [Docker](#docker-recommended) in the meantime. See [CONTRIBUTING.md](CONTRIBUTING.md#ci-secrets) for how to configure the `CRATES_IO_TOKEN` secret for publishing.

```bash
cargo install modelrelay-server modelrelay-worker
```

### Build from source

```bash
cargo build --release
# Binaries: target/release/modelrelay-server  target/release/modelrelay-worker
```

**Start the proxy:**

```bash
./target/release/modelrelay-server \
  --listen 0.0.0.0:8080 \
  --worker-secret mysecret
```

**Start a worker** (on a GPU box with `llama-server` or any OpenAI-compatible backend running):

```bash
./target/release/modelrelay-worker \
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

## Connecting your tools

Once the proxy is running, point your existing tools at it — no special client needed.

**curl** — see [Try it](#try-it) above.

**Claude Code / Claude Desktop** — set the base URL to your proxy:

```bash
export ANTHROPIC_BASE_URL=http://localhost:8080
claude    # requests now route through ModelRelay
```

**LiteLLM** — add a model entry in your `config.yaml`:

```yaml
model_list:
  - model_name: llama3.2:3b
    litellm_params:
      model: openai/llama3.2:3b
      api_base: http://localhost:8080/v1
```

**Open WebUI** — point the OpenAI-compatible backend at the proxy:

```bash
export OPENAI_API_BASE_URL=http://localhost:8080/v1
```

Any tool that speaks OpenAI or Anthropic API formats works — just change the base URL.

## Features

- **Cross-platform** — pre-built binaries for Linux, macOS, and Windows (x86_64 + arm64)
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

### modelrelay-server

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--listen` | `LISTEN_ADDR` | `127.0.0.1:8080` | Address to listen on |
| `--worker-secret` | `WORKER_SECRET` | *(required)* | Secret workers must present to authenticate |
| `--provider` | `PROVIDER_NAME` | `local` | Provider name used for worker routing and request dispatch |
| `--max-queue-len` | `MAX_QUEUE_LEN` | `100` | Maximum number of queued requests (0 = unlimited) |
| `--queue-timeout` | `QUEUE_TIMEOUT_SECS` | `30` | Seconds before a queued request times out (0 = no timeout) |
| `--request-timeout` | `REQUEST_TIMEOUT_SECS` | `300` | Seconds before an in-flight HTTP request times out (0 = no timeout) |
| `--log-level` | `LOG_LEVEL` | `info` | Log level filter (e.g. `info`, `debug`, or `modelrelay_server=debug`). Overridden by `RUST_LOG` if set. |

### modelrelay-worker

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `--proxy-url` | `PROXY_URL` | `http://127.0.0.1:8080` | Base URL of the proxy server |
| `--worker-secret` | `WORKER_SECRET` | *(required)* | Secret used to authenticate with the proxy |
| `--backend-url` | `BACKEND_URL` | `http://127.0.0.1:8000` | Base URL of the local model backend |
| `--models` | `MODELS` | `default` | Comma-separated list of model names this worker supports |
| `--provider` | `PROVIDER_NAME` | `local` | Provider name to register with on the proxy |
| `--worker-name` | `WORKER_NAME` | `worker` | Human-readable name for this worker instance |
| `--max-concurrency` | `MAX_CONCURRENCY` | `1` | Maximum number of concurrent requests this worker will handle |
| `--log-level` | `LOG_LEVEL` | `info` | Log level filter (e.g. `info`, `debug`, or `modelrelay_worker=debug`). Overridden by `RUST_LOG` if set. |

All flags can be passed as CLI arguments or set via the corresponding environment variable.

## Production deployment

### Docker Compose (multi-worker)

The included [`docker-compose.yml`](docker-compose.yml) runs the proxy with two workers, health checks, restart policies, memory limits, and log rotation:

```bash
cp .env.example .env   # edit WORKER_SECRET and backend URLs
docker compose up -d
```

Add more workers by duplicating a worker service block and adjusting `MODELS`, `BACKEND_URL`, and `WORKER_NAME`.

### Systemd (bare metal / VM)

Service files live in [`extras/`](extras/):

```bash
# Install binaries (from a release archive or cargo build --release)
sudo install -m 755 modelrelay-server modelrelay-worker /usr/local/bin/

# Create a service user
sudo useradd --system --no-create-home modelrelay
sudo mkdir -p /var/lib/modelrelay /etc/modelrelay

# Proxy
sudo cp extras/modelrelay-server.service /etc/systemd/system/
sudo cp extras/proxy.env.example /etc/modelrelay/proxy.env
sudo vim /etc/modelrelay/proxy.env   # set WORKER_SECRET
sudo systemctl enable --now modelrelay-server

# Workers — the template unit lets you run multiple instances:
sudo cp extras/modelrelay-worker@.service /etc/systemd/system/
sudo cp extras/worker.env.example /etc/modelrelay/worker-gpu0.env
sudo vim /etc/modelrelay/worker-gpu0.env   # set PROXY_URL, BACKEND_URL, MODELS
sudo systemctl enable --now modelrelay-worker@gpu0
```

See [`extras/`](extras/) for the full service files and annotated env examples.

### TLS

The proxy and workers communicate over plain HTTP/WebSocket by default. For production, terminate TLS at a reverse proxy like nginx. An annotated configuration is provided at [`examples/tls-nginx.conf`](examples/tls-nginx.conf) — it handles HTTPS for client requests and `wss://` WebSocket upgrades for workers, with streaming-friendly settings (buffering disabled, long timeouts).

### Load Testing

A ready-made load test script lives at [`extras/load-test.sh`](extras/load-test.sh). It uses `hey` if installed, falls back to `wrk`, and finally to parallel `curl` loops:

```bash
./extras/load-test.sh -n 200 -c 20 -m llama3-8b
```

## Documents

- [Behavior contract](docs/behavior-contract.md) — the full specification of proxy, queue, streaming, and cancellation semantics
- [Architecture sketch](docs/architecture.md) — how the pieces fit together internally
- [Protocol walkthrough](docs/protocol-walkthrough.md) — ASCII wire traces for every message flow
- [Operational runbook](docs/runbook.md) — health checks, draining, scaling, troubleshooting

## Validation

The behavior matrix is exercised at three layers: black-box contract harnesses in `modelrelay-contract-tests`, live HTTP integration tests in `modelrelay-server`, and end-to-end live backend tests in `modelrelay-worker`.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Contributing

Bug reports, feature requests, and PRs are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md) for code style, test expectations,
branch naming, and CI secrets.

To report a security vulnerability, follow the process in
[SECURITY.md](SECURITY.md).

## License

MIT
