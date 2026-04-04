# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-04-04

### Added

- Central proxy server (`proxy-server`) that accepts OpenAI-compatible and Anthropic-compatible HTTP requests and routes them to remote workers over WebSocket.
- Worker daemon (`worker-daemon`) that connects to the proxy, registers supported models, and forwards inference requests to a local backend (e.g. llama-server).
- WebSocket-based worker protocol (`worker-protocol`) with authentication, model advertisement, request dispatch, and response streaming.
- Request queueing at the proxy when no worker is immediately available, with configurable queue size and timeout.
- Streaming response pass-through preserving SSE chunk ordering and termination semantics.
- End-to-end cancellation propagation from HTTP client disconnect through WebSocket to worker backend.
- Worker heartbeat, stale-worker cleanup, and graceful shutdown/drain.
- OpenAI-compatible `/v1/chat/completions` endpoint with streaming support.
- Anthropic-compatible `/v1/messages` endpoint with streaming support.
- Comprehensive contract test suite (`proxy-contract-tests`) covering registration, queueing, streaming, cancellation, disconnect, timeout, and error surfaces.
- Docker support with multi-stage builds for proxy and worker images.
- GHCR container image publishing via CI.
- Cross-platform release binaries (Linux x86_64/aarch64, macOS x86_64/aarch64) via GitHub Actions.
- CI pipeline with formatting, linting, and test checks.

[0.1.0]: https://github.com/ericflo/modelrelay/releases/tag/v0.1.0
