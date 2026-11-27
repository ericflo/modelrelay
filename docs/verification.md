# Verification

This page records what is claimed about the ModelRelay proxy–worker protocol, what has been checked, how, and what has not. Bounded model checking is evidence about a protocol model, not a proof of the Rust implementation or of every deployment.

## What is claimed

For one proxy coordinating a fleet of workers, under the assumptions below:

1. **Bounded capacity and leak-free slots.** No worker ever holds more in-flight requests than its `max_concurrency`. Every server-side in-flight slot on a connected worker corresponds to either a `Request` on the wire, an active backend task, a terminal reply (`ResponseComplete` or `Error`) on the wire, or an armed server cancel-reap timer—a cancelled request cannot permanently leak a concurrency slot.
2. **Stream integrity and error status fidelity.** Client-visible SSE chunks for a streaming request come from at most one worker: once any chunk is emitted, a worker disconnect terminates the stream with an error instead of requeueing to a second worker. A non-2xx streaming backend error returns a unary error status rather than committing HTTP 200 SSE.
3. **Ownership, fault isolation, and policy enforcement.** Only the owning worker may deliver chunks or complete an in-flight request; stray or stale frames from other workers are ignored. A backend failure on one request never aborts healthy sibling requests on the same worker. Tenant-scoped workers never execute requests belonging to another tenant. `ProviderRouting` (`only`, `ignore`, and `allow_fallbacks`) is respected on immediate, fallback, and queued dispatch.
4. **Work conservation, queue consistency, and accurate load.** No queued request remains waiting while a compatible, connected, non-draining worker has capacity. Every queued request resides in its own requested provider's queue even after cross-provider disconnect requeues. Workers remove finished requests from their active set before sending `ResponseComplete`, so heartbeat `Pong` frames never report stale inflated load for already-completed work.
5. **Lifecycle soundness and termination.** Workers transition to connected only after authentication succeeds and `RegisterAck` arrives. Transport-level WebSocket `Ping` frames never satisfy or reset application-level JSON heartbeat tracking, so wedged workers are detected and disconnected. Draining workers receive no new dispatches. Under weak fairness, every submitted request eventually reaches a terminal state, every wedged worker is reaped, and every draining worker exits.

## Assumptions

- **WebSocket connections** deliver frames FIFO per connection without silent reordering or corruption, and may close abruptly at any step. Frames in flight on a closed connection are lost.
- **Workers** may connect with valid or invalid credentials, pause arbitrarily, disconnect without warning, wedge their JSON processing loop while keeping transport-level WebSocket pings alive, or emit stale/stray completion frames on a live socket. Workers and proxy do not run corrupted code or forge secrets.
- **Local backends** may succeed, stream chunks, return non-2xx errors on streaming requests, or fail arbitrarily per request.
- **Clocks** are not assumed synchronized across proxy and workers. Timeouts (queue expiry, cancel-reap grace period, heartbeat pong timeout) are modeled as nondeterministic actions enabled whenever their precondition holds, so safety does not depend on concrete timeout durations and liveness assumes only that enabled timeouts eventually fire.

## The model

[`spec/ModelRelay.tla`](../spec/ModelRelay.tla) specifies the proxy–worker protocol (`ProxyServerCore`, `WorkerSocketApp`, `ProxyHttpApp`, and `WorkerDaemon`) with explicit per-worker FIFO WebSocket channels (`s2w`, `w2s`), provider queues, deterministic atomic queue draining (`DrainAll`), streaming chunks, non-2xx streaming errors, client cancellation and server reap, stray worker frames, multi-tenant ownership, `ProviderRouting`, JSON `Ping`/`Pong` heartbeats versus transport WebSocket pings, wedged workers, auth rejection (`Close 1008`), and graceful shutdown draining.

Fourteen safety invariants are checked in every reachable state:

| Invariant | Meaning |
| --- | --- |
| `CapacityRespected` | No worker ever has more in-flight requests than `MaxConcurrent`. |
| `NoOrphanedServerSlot` | Every in-flight slot on a connected worker is backed by a `Req` in `s2w`, an active task in `wActive`, a `Complete`/`Error` in `w2s`, or an armed cancel-reap timer. |
| `SingleStreamOrigin` | Client-visible chunks for a streaming request come from at most one worker (no Franken-streams). |
| `OwnerMatchesDelivery` | No non-owner worker delivers a chunk or steals completion of an active request. |
| `ErrorIsolation` | A backend error on one request never aborts healthy sibling requests on the same worker. |
| `LoadAccountingAccurate` | No `Pong` reports inflated active load counting a request whose `Complete` was already sent. |
| `WorkConserving` | No queued request sits waiting while a compatible, connected, non-draining worker has free capacity. |
| `QueueProviderConsistent` | Every queued request is in its own `rProvider` queue, and every queue entry is queued for that provider. |
| `TenantIsolated` | No in-flight request runs on a worker owned by a different tenant. |
| `RoutingRespected` | Every in-flight request satisfies its `ProviderRouting` (`only`, `ignore`, `allow_fallbacks`). |
| `WedgedWorkerDetected` | Transport-level WebSocket pings never clear JSON heartbeat tracking for a wedged worker. |
| `NoDispatchWhileDraining` | No request is dispatched to a worker marked draining. |
| `ConnectedMeansRegistered` | A worker enters `connected` only after authenticating and receiving `RegisterAck`. |
| `ErrorStatusPreserved` | A non-2xx streaming backend error never commits HTTP 200 SSE to the client. |

Three temporal liveness properties are checked under weak fairness (`FairSpec`) in `spec/Liveness.cfg`:

| Property | Meaning |
| --- | --- |
| `AllRequestsTerminate` | Every queued or in-flight request eventually reaches `completed`, `failed`, or `cancelled`. |
| `AllWedgedWorkersReaped` | Every connected wedged worker is eventually disconnected by heartbeat timeout. |
| `AllDrainingWorkersExit` | Every connected draining worker eventually disconnects once in-flight work finishes. |

Run the full default model check (all positive configurations, liveness, and all 13 negative controls):

```sh
scripts/check-model.sh        # ~2 minutes locally; capped at 2 GiB heap, 4 workers, and 10 minutes per configuration
```

The `model` stage in [`Dockerfile`](../Dockerfile) runs `scripts/check-model.sh` against pinned `tla2tools-1.7.4.jar` (SHA-256 verified) and gates both the `server` and `cloud` targets via the `verified` stage so Woodpecker CI cannot publish images if any positive configuration fails or any negative control stops being refuted.

**Results** (TLC 2.19, 2026-11-26), each an exhaustive search with zero violations:

| Configuration | Bounds & enabled features | States generated | Distinct states | Depth | Checked |
| --- | --- | --- | --- | --- | --- |
| `ModelRelay` | 2 workers, 1 tenant, 1 provider, `MaxConcurrent=1`, 2 requests, 1 requeue, queue 2; streaming, backend errors, stray frames | 10,422,451 | 1,799,985 | 31 | 14 safety invariants |
| `Concurrency` | 1 worker, `MaxConcurrent=2`, 2 requests, 1 requeue, queue 2; heartbeats, wedged workers, backend errors | 453,858 | 85,213 | 24 | 14 safety invariants |
| `Routing` | 2 workers across 2 providers, `MaxConcurrent=1`, 1 request, 1 requeue, queue 1; all `only`/`ignore`/`allow_fallbacks` combinations, cross-provider requeue | 22,965 | 6,085 | 13 | 14 safety invariants |
| `MultiTenant` | 2 tenant-scoped workers across 2 tenants, `MaxConcurrent=1`, 2 requests, 1 requeue, queue 2 | 41,085 | 9,155 | 21 | 14 safety invariants |
| `Lifecycle` | 1 worker, `MaxConcurrent=2`, 2 requests, 1 requeue, queue 1; auth failures, `RegisterAck`, heartbeats/wedged workers, graceful shutdown | 618,365 | 104,990 | 24 | 14 safety invariants |
| `Liveness` | 2 workers, `MaxConcurrent=1`, 1 request, 1 requeue, queue 1; heartbeats/wedged workers, graceful shutdown (`FairSpec`) | 192,278 | 21,341 | 23 | 14 safety invariants + 3 liveness properties |
| `DeepRouting` *(on-demand)* | 2 workers across 2 providers, `MaxConcurrent=1`, 2 requests, 1 requeue, queue 2; `only`/`ignore`/`allow_fallbacks`, cross-provider requeue | 16,295,085 | 3,526,013 | 21 | 14 safety invariants |
| `DeepLiveness` *(on-demand)* | 2 workers, `MaxConcurrent=1`, 1 request, 1 requeue, queue 1; streaming, heartbeats/wedged workers, graceful shutdown (`FairSpec`) | 730,115 | 79,717 | 25 | 14 safety invariants + 3 liveness properties |

Larger on-demand configurations (`DeepRouting`, `DeepLiveness`, `DeepLifecycle`, and `DeepModelRelay`) are skipped in the default CI run so slower build nodes stay well within the per-configuration 10-minute timeout, and can be run explicitly:

```sh
TLC_TIMEOUT=3600 scripts/check-model.sh DeepRouting
```

### Negative controls

Every protocol rule is controlled by a `BOOLEAN` constant in `spec/ModelRelay.tla`. In the real protocol all 13 rules are `TRUE`. Each `spec/Unsafe*.cfg` configuration flips exactly one rule to `FALSE` and must be refuted by TLC with an invariant counterexample (exit code 12); if any negative control passed, `scripts/check-model.sh` would fail because the model would no longer be testing that rule. All thirteen are refuted:

| Rule turned off | Configuration | Violated invariant |
| --- | --- | --- |
| Worker acknowledges `Cancel` with `499` and server arms cancel-reap timer (`AckCancelOrReap`) | `UnsafeNoCancelRelease` | `NoOrphanedServerSlot` |
| Never requeue a streaming request after chunk 1 is emitted (`NoRequeueAfterChunk`) | `UnsafeRequeueMidStream` | `SingleStreamOrigin` |
| Verify worker ownership before touching the client response channel (`VerifyOwnerBeforeComplete`) | `UnsafeUnverifiedComplete` | `OwnerMatchesDelivery` |
| Isolate per-request backend errors from sibling requests and the WebSocket session (`IsolateBackendErrors`) | `UnsafeErrorKillsWorker` | `ErrorIsolation` |
| Remove completed request from active set before sending `ResponseComplete` (`AtomicLoadBeforeComplete`) | `UnsafeStalePongLoad` | `LoadAccountingAccurate` |
| Drain compatible queued work on worker registration and disconnect (`DrainOnRegisterAndDisconnect`) | `UnsafeNoDrainOnJoinOrLeave` | `WorkConserving` |
| Requeue disconnected requests under `request.provider`, not `worker.provider` (`RequeueUnderRequestProvider`) | `UnsafeRequeueUnderWorkerProvider` | `QueueProviderConsistent` |
| Enforce tenant ownership scoping on worker dispatch (`EnforceTenantIsolation`) | `UnsafeCrossTenantDispatch` | `TenantIsolated` |
| Enforce `ProviderRouting` (`only`/`ignore`/`allow_fallbacks`) on queued and fallback dispatch (`EnforceRoutingOnQueuedDispatch`) | `UnsafeIgnoreRoutingOnQueue` | `RoutingRespected` |
| Keep JSON heartbeat tracking independent of transport-level WS `Ping` frames (`HeartbeatIndependentOfWsPing`) | `UnsafeWsPingStarvesHeartbeat` | `WedgedWorkerDetected` |
| Stop assigning new requests to workers marked draining (`StopAssigningWhileDraining`) | `UnsafeDispatchWhileDraining` | `NoDispatchWhileDraining` |
| Mark worker connected only after authentication succeeds and `RegisterAck` arrives (`RequireRegisterAckBeforeConnected`) | `UnsafeConnectedBeforeAck` | `ConnectedMeansRegistered` |
| Return unary non-2xx `ResponseComplete` for non-2xx streaming backend errors (`UnaryNon2xxForStreamingError`) | `UnsafeStreamNon2xxAs200` | `ErrorStatusPreserved` |

### What the model leaves out

- **Single-proxy topology.** `modelrelay-server` is a single-process proxy; multi-proxy coordination or distributed shared queues are not modeled.
- **Wire framing, parsing, and HTTP body streaming.** WebSocket messages are abstract typed records and streaming bodies are abstract chunks. JSON serialization, SSE line framing, UTF-8 handling, payload size limits, HTTP header forwarding, and TLS termination are outside the model.
- **Concrete timeouts and real clocks.** Queue timeouts, cancel-reap grace periods, heartbeat intervals, and reconnect backoff/jitter are modeled nondeterministically; TLC checks that the protocol does not rely on clock synchronization, not that particular millisecond or second settings prevent false timeouts on noisy networks.
- **Control plane, persistence, web UI, and desktop shell.** `modelrelay-cloud` (SQLite/PostgreSQL, sessions, billing, worker/API key provisioning), `modelrelay-web` templates, and `modelrelay-desktop` (Tauri tray/settings/auto-updater) are outside the TLA+ model, except for the `RegisterAck` authentication handshake boundary.
- **Model-name matching and tie-breaking heuristics.** Routing models provider names (`only`, `ignore`, `allow_fallbacks`), tenant ownership, and concurrency load, not per-model string matching or worker selection tie-breaking.
- **Small finite bounds.** The default configurations check 1–2 workers, 1–2 providers, 1–2 tenants, and 1–2 requests. Interactions that require 3+ workers or deeper queue chains are outside those bounds.
- **No mechanical refinement link to Rust.** Nothing formally proves that `crates/modelrelay-server` and `crates/modelrelay-worker` refine `spec/ModelRelay.tla`; confidence in the implementation rests on direct code correspondence and the tests below.

## Tests of the implementation

The Rust workspace is tested on every `Dockerfile` build (`cargo fmt --check`, `cargo clippy --locked --workspace --exclude modelrelay-desktop --all-targets --all-features -- -D warnings`, `cargo test --locked --workspace --exclude modelrelay-desktop`, plus `cargo-deny`), and `modelrelay-desktop` is tested locally alongside the workspace:

| Test suite | What it covers | Tests |
| --- | --- | --- |
| `crates/modelrelay-server/tests/adversarial_reliability.rs` | Targeted regression tests for cancellation slot release and cancel-reap, mid-stream disconnect without Franken-stream requeue, cross-provider disconnect requeue, register/disconnect queue draining, multi-tenant isolation, `ProviderRouting` queued enforcement, WS-ping heartbeat independence, non-2xx streaming status preservation, graceful drain rejection, and stale frame ownership checks. | 12 |
| `crates/modelrelay-server/tests/worker_socket.rs` | WebSocket registration, auth rejection (`Close 1008`), model catalog updates, heartbeat pings/timeouts, graceful shutdown draining, and disconnect cleanup. | 16 |
| `crates/modelrelay-server/tests/http_*.rs` | Live HTTP integration suites across `/v1/chat/completions`, `/v1/messages`, `/v1/responses`, `/v1/models`, `/health`, API key auth (`/admin/keys`), and live `WorkerDaemon` round-trips including streaming and cancellation. | 75 |
| `crates/modelrelay-contract-tests/` | Behavior-contract harnesses (`registration`, `dispatch`, `response`, `heartbeat`, `admin_api`) and live server contract tests checked against [`docs/behavior-contract.md`](behavior-contract.md). | 67 |
| `crates/modelrelay-worker/tests/live_backend.rs` | End-to-end worker daemon against live mock OpenAI/Anthropic backends: streaming, non-2xx errors, backend crashes/resets, cancellation propagation, reconnect backoff, and model polling. | 17 |
| `crates/modelrelay-cloud/tests/http_routes.rs` | Control-plane HTTP routes, authentication/sessions, CSRF protection, key management, and billing/admin guards. | 35 |
| Unit tests (`modelrelay-server`, `modelrelay-worker`, `modelrelay-protocol`, `modelrelay-cloud`, `modelrelay-web`, `modelrelay-desktop`) | Core state-machine transitions, load accounting, key hashing, protocol serialization, template rendering, and desktop tray/reconnect state transitions. | 86 |
