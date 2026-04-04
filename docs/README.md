# ModelRelay

**Stop configuring clients for every GPU box. Workers connect out; requests route in.**

You have GPU boxes running `llama-server` (or Ollama, or vLLM, or anything OpenAI-compatible). Today you either expose each one directly — port forwarding, DNS, firewall rules — or you stick a load balancer in front that doesn't understand LLM streaming or cancellation.

ModelRelay flips the model: a central proxy receives standard inference requests while worker daemons on your GPU boxes connect *out* to it over WebSocket. The proxy handles queueing, routing, streaming pass-through, and cancellation propagation. Clients see one stable endpoint and never need to know about your hardware.

```
  Clients (curl, Claude Code, LiteLLM, Open WebUI, ...)
         |
         |  POST /v1/chat/completions
         |  POST /v1/messages
         v
  +----------------------+
  |   modelrelay-server  |<--- workers connect out (WebSocket)
  |   (one stable        |     no inbound ports needed on GPU boxes
  |    endpoint)         |
  +----------------------+
         |  routes request to best available worker
         v
  +--------+  +--------+  +--------+
  |worker-1|  |worker-2|  |worker-3|
  | llama  |  | ollama |  | vllm   |  <- your GPU boxes,
  | server |  |        |  |        |    anywhere on any network
  +--------+  +--------+  +--------+
```

## Who is this for?

- **Home GPU users** running local models who want a single API endpoint across multiple machines
- **Teams with on-prem hardware** that need to pool GPU capacity without a service mesh
- **Researchers** juggling models across heterogeneous boxes who are tired of updating client configs

## Features

- **OpenAI + Anthropic compatible** — `POST /v1/chat/completions`, `POST /v1/responses`, `POST /v1/messages`, `GET /v1/models`
- **No inbound ports on GPU boxes** — workers connect out to the proxy over WebSocket
- **Request queueing** — configurable depth and timeout when all workers are busy
- **Streaming pass-through** — SSE chunks forwarded with preserved ordering and termination
- **End-to-end cancellation** — client disconnect propagates through the proxy to the worker to the backend
- **Automatic requeue** — if a worker dies mid-request, the request is requeued to another worker
- **Heartbeat and load tracking** — stale workers are cleaned up; workers report current load
- **Graceful drain** — workers can shut down while replacement workers pick up queued work
- **Cross-platform** — pre-built binaries for Linux, macOS, and Windows (x86_64 + arm64)

## Quick Start

The fastest way to get running is with Docker:

```bash
# 1. Run the proxy
docker run -p 8080:8080 \
  -e WORKER_SECRET=mysecret \
  -e LISTEN_ADDR=0.0.0.0:8080 \
  ghcr.io/ericflo/modelrelay/modelrelay-server:latest

# 2. Run a worker (on a GPU box with llama-server or similar)
docker run \
  -e PROXY_URL=http://<proxy-host>:8080 \
  -e WORKER_SECRET=mysecret \
  -e BACKEND_URL=http://host.docker.internal:8000 \
  -e MODELS=llama3.2:3b \
  ghcr.io/ericflo/modelrelay/modelrelay-worker:latest

# 3. Send a request
curl http://localhost:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "llama3.2:3b",
    "messages": [{"role": "user", "content": "Hello!"}]
  }'
```

For more installation options (pre-built binaries, Docker Compose, building from source, systemd, Windows services), see the [GitHub README](https://github.com/ericflo/modelrelay).

## Documentation

- **[Architecture](architecture.md)** — System design, component overview, and data flow
- **[Protocol Walkthrough](protocol-walkthrough.md)** — Wire-level protocol details with examples
- **[Behavior Contract](behavior-contract.md)** — The exact behavioral guarantees the system provides
- **[Operational Runbook](runbook.md)** — Deployment, configuration, monitoring, and troubleshooting

## Source & Contributing

ModelRelay is MIT-licensed and developed at [github.com/ericflo/modelrelay](https://github.com/ericflo/modelrelay). Bug reports, feature requests, and PRs are welcome — see [CONTRIBUTING.md](https://github.com/ericflo/modelrelay/blob/main/CONTRIBUTING.md) for details.
