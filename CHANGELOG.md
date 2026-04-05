# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1] - 2026-04-05

### Added

- Full setup wizard polish: 8-step flow with platform-aware hardware prereqs, multi-backend support (LM Studio, Ollama, llama.cpp, vLLM), backend-specific model loading instructions, env var alternative for config, troubleshooting hints with skip-detection on step 6, and persistence walkthrough (systemd/launchd/Windows Service) as step 8 (#235)
- Live worker status card on cloud admin dashboard with auto-polling and "Connect another machine" link
- "Make it persistent" section in README linking to systemd/launchd/Windows Service setup
- Postgres-backed `ApiKeyStore` for multi-replica correctness — atomic `UPDATE...RETURNING`, trait-based store with in-memory and Postgres implementations (#215)
- First-class admin users in modelrelay-cloud with proper `api_keys` association (#212)
- Shared admin API types in `modelrelay-protocol` crate with contract tests to prevent server/cloud drift (#216)
- Setup wizard integrated into cloud dashboard onboarding flow (#217)
- HTTP integration tests for cloud route handlers (#219)
- Integration tests for cloud checkout routes (#226)
- One-shot reprovision tool for syncing cloud API keys into server database (#222)
- `--config` flag support for worker binary, with corrected wizard instructions (#224)
- Kubernetes deployment manifests for modelrelay-server at api.modelrelay.io (#209, #210)
- Relay stats card on subscriber dashboard (#227)
- "How It Works" section with code example on cloud landing page (#228)
- SEO meta tags and favicon on cloud landing page (#229)
- Responsive CSS breakpoints for mobile on cloud landing and pricing pages (#230)
- Responsive CSS breakpoints for `page_shell` auth pages (#231)
- Custom branded 404 page for cloud (#232)
- CSRF protection on all cloud POST forms (#233)

### Changed

- README: pre-built binaries are now the recommended (first-listed) installation method; Docker moved to second position
- Setup wizard progress bar labels: "Platform | Backend | Model | Download | Configure | Connect | Test | Persist"
- "Add another machine" flow skips to step 4 (Download) instead of resetting to step 1
- Cloud checkout success template uses `include_str!` instead of runtime file read (#213)

### Fixed

- Deploy: reference `modelrelay-cloud` secret in Postgres deployment manifest
- Deploy: add `WORKER_SECRET` env var to server deployment (#211)
- Deploy: give modelrelay-server its own Postgres database instead of sharing (#220)
- Cloud: parse `secret` field from server admin API response correctly
- Cloud: add login/signup nav links to pricing page (#214)
- Web: show login/signup nav links for non-authenticated `page_shell` users (#221)

## [0.2.0] - 2026-04-05

### Added

- `GET /health` endpoint returning JSON with version, status, connected worker count, queue depth, and uptime
- Admin monitoring API: `GET /admin/workers`, `GET /admin/stats`, `GET /admin/keys` with `MODELRELAY_ADMIN_TOKEN` bearer auth
- Optional client API key authentication via `MODELRELAY_REQUIRE_API_KEYS` env var with admin `POST`/`DELETE` endpoints for key create/revoke
- `modelrelay-web` crate: Axum-based admin web service with embedded static assets, served under a configurable `/admin/` prefix
- Live admin dashboard with real-time worker status, request metrics, queue depth, and API key management panels
- Worker onboarding setup wizard: always-accessible step-by-step guide for connecting new machines (platform detection, LM Studio install, model config, worker download, live connection status, test inference)
- `modelrelay-cloud` crate: commercial features separated from OSS core (Stripe billing, user auth, API key provisioning, landing page)
- User authentication: sign-up, login, and logout flows in modelrelay-cloud
- Stripe integration: checkout skeleton, webhook handler, billing portal, and subscription status dashboard
- API key provisioning via admin API in modelrelay-cloud
- PostgreSQL and session support for modelrelay-cloud
- Login/signup/pricing navigation links on the cloud landing page
- Hosted-version blurb in README and mdBook docs intro page linking to modelrelay.io
- Admin API, API key auth, and web dashboard documentation in operational runbook
- Unit tests for cloud webhook, auth, and dashboard modules

### Changed

- OSS admin routes from `modelrelay-web` are now mounted in `modelrelay-cloud` under `/admin`
- Shared `page_shell` HTML template refactored for reuse across OSS and cloud web crates

### Fixed

- Added missing `tokio/signal` feature flag for graceful shutdown in `modelrelay-web`

## [0.1.6] - 2026-04-04

### Added

- TLS setup guide (`docs/tls.md`) explaining nginx termination for HTTPS clients and WSS workers
- Unit tests for `WorkerDaemonConfig` URL helper methods in modelrelay-worker

## [0.1.5] - 2026-04-04

### Changed

- CI now respects `rust-toolchain.toml` for MSRV verification — replaced `dtolnay/rust-toolchain@stable` with `rustup show` so the test matrix runs against the pinned 1.94.1 toolchain rather than latest stable

## [0.1.4] - 2026-04-04

### Added

- mdBook documentation site auto-deployed to GitHub Pages on every push to main
- GitHub Pages badge in README linking to https://ericflo.github.io/modelrelay/
- GitHub Pages enabled at https://ericflo.github.io/modelrelay/

## [0.1.3] - 2026-04-04

### Added

- Shell completion generation via `--completions <SHELL>` flag on both `modelrelay-server` and `modelrelay-worker` (bash, zsh, fish, PowerShell, elvish)
- Windows Service setup scripts: `extras/install-windows-service.ps1` and `extras/install-windows-service-worker.ps1`
- Timeout and queue-full wire traces in `docs/protocol-walkthrough.md`

### Changed

- README documents shell completion installation for all supported shells
- README documents Windows Service installation with `sc.exe`

### Fixed

- Bumped docker/metadata-action, docker/build-push-action, actions/download-artifact to latest major versions (dependabot)

## [0.1.2] - 2026-04-04

### Added

- CODE_OF_CONDUCT.md (Contributor Covenant)
- MSRV badge (rustc 1.94+) in README
- Rustdoc coverage on `modelrelay-protocol` public API
- Expanded architecture.md with component diagrams and state machines
- Wire trace examples, TLS nginx config, load test script, and operational runbook
- Documentation of required CI secrets for crates.io publishing

### Changed

- Cleaned up `docs/behavior-contract.md` to remove private provenance references; renamed "First Characterization Tests To Write Next" section to "Extension Points"
- Pointed homepage to GitHub until modelrelay.io is live

### Fixed

- Bumped crate versions to 0.1.2 to match release tag
- Migrated cargo audit CI step from deprecated `rustsec/audit-check` to direct `cargo audit`
- Fixed Node.js 20 deprecation warnings in release workflow; added graceful crates.io skip
- Corrected crates.io badge format in README
- Fixed stale binary names (`proxy-server`/`worker-daemon`) in docs, examples, and CONTRIBUTING.md
- Removed internal agent branch prefix from CONTRIBUTING.md
- Used compare URL for CHANGELOG v0.1.1 link

## [0.1.1] - 2026-04-04

### Added

- Windows x86_64 and arm64 release binaries
- crates.io publish step in release workflow
- Social preview image in extras/

### Changed

- Binary names now use `modelrelay-server` and `modelrelay-worker` prefix (previously `proxy-server` and `worker-daemon`)
- Repo renamed from llm-worker-proxy to modelrelay on GitHub

## [0.1.0] - 2026-04-04

### Added

- Central proxy server (`modelrelay-server`) that accepts OpenAI-compatible and Anthropic-compatible HTTP requests and routes them to remote workers over WebSocket.
- Worker daemon (`modelrelay-worker`) that connects to the proxy, registers supported models, and forwards inference requests to a local backend (e.g. llama-server).
- WebSocket-based worker protocol (`modelrelay-protocol`) with authentication, model advertisement, request dispatch, and response streaming.
- Request queueing at the proxy when no worker is immediately available, with configurable queue size and timeout.
- Streaming response pass-through preserving SSE chunk ordering and termination semantics.
- End-to-end cancellation propagation from HTTP client disconnect through WebSocket to worker backend.
- Worker heartbeat, stale-worker cleanup, and graceful shutdown/drain.
- OpenAI-compatible `/v1/chat/completions` endpoint with streaming support.
- Anthropic-compatible `/v1/messages` endpoint with streaming support.
- Comprehensive contract test suite (`modelrelay-contract-tests`) covering registration, queueing, streaming, cancellation, disconnect, timeout, and error surfaces.
- Docker support with multi-stage builds for proxy and worker images.
- GHCR container image publishing via CI.
- Cross-platform release binaries (Linux x86_64/aarch64, macOS x86_64/aarch64) via GitHub Actions.
- CI pipeline with formatting, linting, and test checks.

[Unreleased]: https://github.com/ericflo/modelrelay/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/ericflo/modelrelay/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/ericflo/modelrelay/compare/v0.1.6...v0.2.0
[0.1.6]: https://github.com/ericflo/modelrelay/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/ericflo/modelrelay/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/ericflo/modelrelay/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/ericflo/modelrelay/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/ericflo/modelrelay/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/ericflo/modelrelay/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/ericflo/modelrelay/releases/tag/v0.1.0
