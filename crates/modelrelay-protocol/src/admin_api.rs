//! HTTP wire types for the server's `/admin/*` and `/v1/routing*` APIs, consumed by
//! `modelrelay-cloud` and the shared dashboard UI.
//!
//! These types are the **single source of truth** for the admin and routing API contract.
//! Any rename, addition, or removal must be made here — both server and cloud pick it up
//! via their shared dependency. Direct `serde_json::Value` parsing in cloud handlers is
//! prohibited; use these structs instead.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Request body for `POST /admin/keys`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateKeyRequest {
    /// Human-readable label for the key (e.g. `"user-alice@example.com"`).
    pub name: String,
}

/// Metadata about an API key. Returned by `GET /admin/keys` and (flattened) by
/// `POST /admin/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyMetadata {
    /// Unique key identifier (UUID).
    pub id: String,
    /// Human-readable name assigned at creation time.
    pub name: String,
    /// Non-secret prefix of the raw key (e.g. `"mr_live_abcd1234"`).
    pub prefix: String,
    /// Unix epoch seconds when the key was created.
    pub created_at: u64,
    /// Unix epoch seconds of the most recent successful validation, if any.
    pub last_used_at: Option<u64>,
    /// Whether the key has been revoked.
    pub revoked: bool,
}

/// Response body for `POST /admin/keys`. Includes the raw secret exactly once — the
/// server never returns it again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateKeyResponse {
    /// All non-secret metadata for the key.
    #[serde(flatten)]
    pub metadata: ApiKeyMetadata,
    /// The raw API key, shown exactly once at creation time. Callers must store this —
    /// the server does not retain a recoverable copy.
    pub secret: String,
}

/// Response body for `GET /admin/keys`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminKeysResponse {
    /// List of all key metadata (secrets are never included).
    pub keys: Vec<ApiKeyMetadata>,
}

// ---------------------------------------------------------------------------
// Smart Multi-Worker Routing Configuration & Telemetry Types
// ---------------------------------------------------------------------------

/// High-level routing strategy applied when multiple workers can serve a model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategyDto {
    /// Stick sequential turns to the warm prefix worker, spill concurrent bursts
    /// to idle peers, and spread unrelated new sessions across LRU workers.
    #[default]
    PrefixAware,
    /// Prefer ranked priority workers/providers first; overflow to secondary tiers
    /// when primary machines are busy or offline.
    PriorityFailover,
    /// Spread stateless requests by normalized capacity (`load / max_concurrent`)
    /// with fastest-TTFT tie-breaking.
    LeastLoaded,
    /// Deterministic round-robin across eligible workers.
    RoundRobin,
}

/// Controls how `prefix_aware` behaves when the warm worker is busy (`load >= 1`)
/// and another eligible worker is idle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpilloverModeDto {
    /// Immediately farm concurrent requests to an idle peer (default for local GPUs).
    #[default]
    SpillOnBusy,
    /// Compare estimated uncached prefill time against busy-worker load penalty.
    CostBalanced,
    /// Stay on the warm worker while `load < max_concurrent`, spilling only when full.
    StrictSticky,
}

fn default_min_prefix_bytes() -> usize {
    512
}

fn default_prefix_ttl_secs() -> u64 {
    900
}

fn default_true() -> bool {
    true
}

/// Serializable routing policy settings for the default group, an exact model
/// override, or a custom model group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct RoutingPolicyConfig {
    /// Primary worker selection strategy.
    #[serde(default)]
    pub strategy: RoutingStrategyDto,
    /// Concurrent spillover policy when the warm worker is busy.
    #[serde(default)]
    pub spillover_mode: SpilloverModeDto,
    /// Minimum matched prefix size in bytes (~128 tokens at 512 bytes) to treat
    /// a worker as prefix-warm.
    #[serde(default = "default_min_prefix_bytes")]
    pub min_prefix_bytes: usize,
    /// Prefix cache & session affinity retention window in seconds.
    #[serde(default = "default_prefix_ttl_secs")]
    pub prefix_ttl_secs: u64,
    /// Enable bounded prefix-aware queue draining (with anti-starvation cap).
    #[serde(default = "default_true")]
    pub prefix_aware_queue: bool,
    /// Avoid routing long prompts to workers with a smaller advertised context window.
    #[serde(default = "default_true")]
    pub respect_context_window: bool,
    /// Prefer workers where the model is already loaded in VRAM over unloaded/sleeping ones.
    #[serde(default = "default_true")]
    pub prefer_loaded_models: bool,
    /// Allow clients to override strategy per request via `provider.sort`.
    #[serde(default = "default_true")]
    pub allow_client_overrides: bool,
    /// Ordered worker names or provider names for `priority_failover`.
    #[serde(default)]
    pub worker_priority: Vec<String>,
    /// In `priority_failover`, keep an overflowed session sticky on its warm worker while idle.
    #[serde(default = "default_true")]
    pub sticky_after_overflow: bool,
}

impl Default for RoutingPolicyConfig {
    fn default() -> Self {
        Self {
            strategy: RoutingStrategyDto::PrefixAware,
            spillover_mode: SpilloverModeDto::SpillOnBusy,
            min_prefix_bytes: default_min_prefix_bytes(),
            prefix_ttl_secs: default_prefix_ttl_secs(),
            prefix_aware_queue: true,
            respect_context_window: true,
            prefer_loaded_models: true,
            allow_client_overrides: true,
            worker_priority: Vec::new(),
            sticky_after_overflow: true,
        }
    }
}

/// How a custom dashboard group ties models together (exact model IDs only, no globs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelGroupMode {
    /// Checked exact model IDs keep their own model names and share this routing policy.
    #[default]
    SharedPolicy,
    /// Exposes `pool_model_id` as one unified model across all checked `(worker, model)`
    /// targets, rewriting `"model"` in the request body to each worker's native model ID.
    UnifiedPool,
}

/// One exact `(optional worker_name, model_id)` member selected in the dashboard UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelGroupTarget {
    /// Optional worker name filter (`None` matches any worker advertising `model_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_name: Option<String>,
    /// Exact native model identifier advertised by the worker.
    pub model_id: String,
}

/// User-defined model group configured in the dashboard UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoutingGroupConfig {
    /// Stable identifier for this group (e.g. UUID or slug).
    pub id: String,
    /// Human-readable group name displayed in the dashboard.
    pub name: String,
    /// Whether this group shares routing policy across separate model IDs or pools
    /// differently-named worker models under one unified model ID.
    #[serde(default)]
    pub mode: ModelGroupMode,
    /// Unified model ID exposed in `/v1/models` when `mode == UnifiedPool`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_model_id: Option<String>,
    /// Exact `(worker_name, model_id)` members tied together in this group.
    #[serde(default)]
    pub targets: Vec<ModelGroupTarget>,
    /// Routing policy applied to requests matching this group.
    #[serde(default)]
    pub policy: RoutingPolicyConfig,
}

/// Complete routing configuration persisted for one owner scope (global self-hosted
/// scope or a specific API-key owner).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantRoutingConfig {
    /// Catch-all default policy for any exact model without an override or group.
    #[serde(default)]
    pub default_policy: RoutingPolicyConfig,
    /// Per-exact-model-ID policy overrides (keyed by exact model ID).
    #[serde(default)]
    pub model_overrides: BTreeMap<String, RoutingPolicyConfig>,
    /// User-defined groups tying exact models/workers together.
    #[serde(default)]
    pub groups: Vec<ModelRoutingGroupConfig>,
    /// Emit `X-ModelRelay-Worker`, `X-ModelRelay-Route-Reason`, and
    /// `X-ModelRelay-Prefix-Match-Bytes` headers on inference responses.
    #[serde(default)]
    pub expose_debug_headers: bool,
}

/// Live telemetry for one connected worker serving a particular model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveModelWorkerInfo {
    /// Ephemeral server-assigned worker connection ID.
    pub worker_id: String,
    /// Human-readable worker name (e.g. `"rtx-5090"`, `"mac-studio"`).
    pub worker_name: String,
    /// Worker provider namespace.
    pub provider: String,
    /// Native model ID this worker advertises (may differ from pool ID for unified pools).
    pub native_model_id: String,
    /// Number of active in-flight requests on this worker.
    pub in_flight: usize,
    /// Configured maximum concurrent requests on this worker.
    pub max_concurrent: usize,
    /// Whether this worker is draining for graceful shutdown.
    pub is_draining: bool,
    /// Residency state reported by backend metadata (`"loaded"`, `"unloaded"`, or `"unknown"`).
    pub loaded_state: String,
    /// Advertised context window in tokens, when reported by the backend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
    /// Exponentially weighted moving average time-to-first-token in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ewma_ttft_ms: Option<f64>,
    /// Recent consecutive backend error count.
    pub consecutive_errors: u32,
    /// Total requests dispatched to this `(worker, model)`.
    pub total_dispatches: u64,
    /// Verified backend-reported cached prompt tokens (Phase 4 telemetry).
    #[serde(default)]
    pub cached_prompt_tokens_total: u64,
    /// Total backend-reported prompt tokens (Phase 4 telemetry).
    #[serde(default)]
    pub prompt_tokens_total: u64,
}

/// Live dashboard view of an exact model ID or a unified pooled model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveModelGroupView {
    /// Exact model ID or unified pool model ID.
    pub model_id: String,
    /// `true` when this entry represents a user-defined `UnifiedPool` model.
    pub is_unified_pool: bool,
    /// Owning custom group ID, if governed by a user-defined group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// Owning custom group display name, if governed by a user-defined group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
    /// `true` when an explicit per-exact-model override exists for this model.
    pub has_custom_override: bool,
    /// Effective resolved routing policy currently active for this model.
    pub effective_policy: RoutingPolicyConfig,
    /// Connected workers currently capable of serving this model.
    pub workers: Vec<LiveModelWorkerInfo>,
}

/// Summary of a connected worker visible to the caller, including live routing metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingWorkerSummary {
    /// Ephemeral server-assigned worker ID.
    pub worker_id: String,
    /// Human-readable worker name.
    pub worker_name: String,
    /// Provider name.
    pub provider: String,
    /// Models advertised by this worker.
    pub models: Vec<String>,
    /// Current in-flight request count.
    pub in_flight_count: usize,
    /// Maximum concurrent slots.
    pub max_concurrent: usize,
    /// Whether the worker is draining.
    pub is_draining: bool,
    /// Average EWMA TTFT across this worker's models, when observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_ttft_ms: Option<f64>,
    /// Number of active prefix trie entries referencing this worker.
    pub warm_prefix_count: usize,
}

/// Aggregated routing and prefix-cache telemetry counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingTelemetryDto {
    /// Total inference requests routed through the smart router.
    pub total_routed: u64,
    /// Requests matched to an idle warm prefix worker.
    pub prefix_hits: u64,
    /// Requests matched via explicit session or `previous_response_id` affinity.
    pub session_hits: u64,
    /// Same-prefix concurrent requests spilled to an idle/less-loaded peer.
    pub concurrent_spillovers: u64,
    /// Concurrent requests kept on a busy warm worker (`cost_balanced` / `strict_sticky`).
    pub warm_busy_stickies: u64,
    /// Cold/unrelated requests spread to an LRU worker to preserve active caches.
    pub cold_spreads: u64,
    /// Requests routed to the primary tier under `priority_failover`.
    pub priority_primaries: u64,
    /// Requests overflowed to a secondary tier under `priority_failover`.
    pub priority_overflows: u64,
    /// Requests routed under `least_loaded`.
    pub least_loaded_routes: u64,
    /// Requests routed under `round_robin`.
    pub round_robin_routes: u64,
    /// Queued requests matched to a warm worker during bounded prefix-aware dequeue.
    pub queued_prefix_matches: u64,
    /// Queued requests dispatched strictly FIFO.
    pub queued_fifo_dispatches: u64,
    /// Times the context-window guard skipped an undersized worker.
    pub context_guard_skips: u64,
    /// Cumulative canonical prefix bytes matched across routed requests.
    pub total_matched_prefix_bytes: u64,
    /// Current number of live prefix trie nodes in memory.
    pub tracked_prefix_nodes: usize,
    /// Cumulative verified cached prompt tokens reported by backends (Phase 4).
    #[serde(default)]
    pub verified_cached_prompt_tokens: u64,
    /// Cumulative prompt tokens reported by backends (Phase 4).
    #[serde(default)]
    pub verified_prompt_tokens: u64,
}

/// Response body for `GET /v1/routing` and `GET /admin/routing`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingStateResponse {
    /// Persisted routing configuration for this scope.
    pub config: TenantRoutingConfig,
    /// Live exact models and unified pools currently visible to this scope.
    pub live_models: Vec<LiveModelGroupView>,
    /// Connected workers visible to this scope, with speed & cache metrics.
    pub workers: Vec<RoutingWorkerSummary>,
    /// Global/scope routing telemetry snapshot.
    pub telemetry: RoutingTelemetryDto,
}

/// Request body for `PUT /v1/routing`, `POST /v1/routing`, `PUT /admin/routing`,
/// and `POST /admin/routing`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRoutingConfigRequest {
    /// Replacement routing configuration for this scope.
    pub config: TenantRoutingConfig,
}
