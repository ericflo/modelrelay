# ModelRelay Documentation

**Stop configuring clients for every GPU box. Workers connect out; requests route in.**

ModelRelay is a central HTTP LLM proxy that routes inference requests to authenticated remote workers over WebSocket. Workers on your GPU boxes connect *out* to the proxy — no inbound ports needed. The proxy handles queueing, routing, streaming pass-through, and cancellation propagation. Clients see one stable endpoint.

## Quick Links

- **[Architecture](architecture.md)** — System design, component overview, and data flow
- **[Protocol Walkthrough](protocol-walkthrough.md)** — Wire-level protocol details with examples
- **[Behavior Contract](behavior-contract.md)** — The exact behavioral guarantees the system provides
- **[Operational Runbook](runbook.md)** — Deployment, configuration, monitoring, and troubleshooting

## Getting Started

See the [main README](https://github.com/ericflo/modelrelay) for installation, quickstart, and configuration reference.
