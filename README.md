# llm-worker-proxy

`llm-worker-proxy` is a central HTTP LLM proxy with authenticated remote workers over WebSockets.

One stable endpoint for clients. Any number of GPU boxes running `worker-daemon`, each pointing at a local model server (e.g. `llama-server`). Workers connect in over WebSocket, register their models and capacity, and the proxy routes queued requests to them — handling streaming, cancellation, requeue on worker disconnect, and graceful drain in one place.

This is operationally simple: you don't need to expose GPU boxes to the internet, update client configs when workers change, or run a service mesh. Workers reach out to the proxy; clients never know they exist.

## Quickstart

### Build

```bash
cargo build --release
# Binaries: target/release/proxy-server  target/release/worker-daemon
```

### Start the proxy server

```bash
./target/release/proxy-server \
  --listen 0.0.0.0:8080 \
  --worker-secret mysecret
```

The proxy accepts OpenAI-compatible and Anthropic-compatible requests on `--listen` and waits for workers to connect.

### Start a worker daemon

On a GPU box with `llama-server` (or any OpenAI-compatible backend) already running:

```bash
./target/release/worker-daemon \
  --proxy-url http://<proxy-host>:8080 \
  --worker-secret mysecret \
  --backend-url http://127.0.0.1:8000 \
  --models llama3.2:3b,llama3.2:1b
```

The daemon connects to the proxy over WebSocket, registers the listed models, and begins accepting forwarded requests.

### Send a test request

```bash
curl http://<proxy-host>:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "llama3.2:3b",
    "messages": [{"role": "user", "content": "Hello!"}],
    "stream": false
  }'
```

Streaming works too — set `"stream": true` and the proxy passes through SSE chunks from the backend.

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

## Features

- OpenAI-compatible endpoints: `GET /v1/models`, `POST /v1/chat/completions`, `POST /v1/responses`
- Anthropic-compatible endpoint: `POST /v1/messages`
- Authenticated worker registration over WebSocket
- Request queueing with configurable depth and timeout
- Streaming pass-through (SSE)
- Cancellation propagation to backends
- Automatic requeue on worker disconnect
- Heartbeat and load tracking
- Model catalog refresh without worker reconnect
- Auth cooldown recovery
- Graceful worker drain while replacement workers pick up queued work

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
