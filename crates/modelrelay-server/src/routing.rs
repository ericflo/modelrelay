//! Smart multi-worker routing engine: tenant-isolated bounded LRU prefix trie,
//! session & Responses-API affinity tables, per-worker EWMA latency/health
//! tracking, hardware-aware filters (context window & loaded-model state), and
//! four routing strategies (`PrefixAware` default with `SpillOnBusy`,
//! `PriorityFailover`, `LeastLoaded`, and `RoundRobin`).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::prefix::{PrefixCheckpoint, RequestRoutingHints};

/// Default minimum matched prefix size (in canonical bytes, ~128 tokens)
/// required before treating a worker as prefix-warm.
pub const DEFAULT_MIN_PREFIX_BYTES: usize = 512;

/// Default retention TTL for prefix trie entries and session affinity mappings.
pub const DEFAULT_PREFIX_TTL: Duration = Duration::from_secs(15 * 60);

/// Maximum number of prefix trie nodes kept globally across all tenants/models.
const MAX_GLOBAL_PREFIX_NODES: usize = 16_384;

/// Maximum number of prefix trie nodes kept for a single `(owner, model)` partition.
const MAX_TENANT_MODEL_PREFIX_NODES: usize = 4_096;

/// Maximum entries in explicit session and Responses-API affinity maps.
const MAX_AFFINITY_ENTRIES: usize = 8_192;

/// Duration over which a worker's recent backend error streak soft-penalizes it.
const ERROR_PENALTY_WINDOW: Duration = Duration::from_secs(30);

/// EWMA smoothing factor for TTFT observations (`0.25` balances responsiveness and stability).
const TTFT_EWMA_ALPHA: f64 = 0.25;

/// Default estimated cold prefill throughput in canonical bytes/ms (~4000 tok/s ≈ 16 bytes/ms)
/// used by `CostBalanced` spillover when no calibrated rate is available yet.
const DEFAULT_PREFILL_BYTES_PER_MS: f64 = 16.0;

/// Concurrency contention penalty per unit load ratio (in ms) for `CostBalanced` mode.
const COST_BALANCED_LOAD_PENALTY_MS: f64 = 2_500.0;

/// Maximum number of compatible queued requests inspected for prefix-aware dequeue.
pub const PREFIX_QUEUE_LOOKAHEAD: usize = 8;

/// Maximum wait time before an oldest queued request bypasses prefix reordering
/// to guarantee zero starvation.
pub const QUEUE_ANTI_STARVATION_CAP: Duration = Duration::from_secs(5);

/// High-level routing strategy applied when multiple workers can serve a request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
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

impl RoutingStrategy {
    /// Parse a per-request `ProviderRouting.sort` string override.
    #[must_use]
    pub fn from_sort_override(sort: &str) -> Option<Self> {
        let normalized = sort.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "prefix" | "prefix_aware" => Some(Self::PrefixAware),
            "priority" | "priority_failover" | "failover" => Some(Self::PriorityFailover),
            "load" | "least_loaded" | "latency" => Some(Self::LeastLoaded),
            "round_robin" | "round-robin" => Some(Self::RoundRobin),
            _ => None,
        }
    }
}

/// Controls how `PrefixAware` behaves when the warm worker is currently busy
/// (`selection_load >= 1`) and another eligible worker is idle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpilloverMode {
    /// Immediately farm concurrent requests to an idle peer (recommended default
    /// for local GPUs running `llama-server`, Ollama, or LM Studio).
    #[default]
    SpillOnBusy,
    /// Compare estimated uncached prefill time on an idle worker against
    /// concurrency contention on the busy warm worker (good for vLLM/SGLang).
    CostBalanced,
    /// Keep routing to the warm worker as long as it has any free slot
    /// (`selection_load < max_concurrent`), spilling only when completely full.
    StrictSticky,
}

/// Resolved routing policy for a single request or model group.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RoutingPolicy {
    pub strategy: RoutingStrategy,
    pub spillover_mode: SpilloverMode,
    pub min_prefix_bytes: usize,
    pub prefix_ttl: Duration,
    pub prefix_aware_queue: bool,
    pub respect_context_window: bool,
    pub prefer_loaded_models: bool,
    pub allow_client_overrides: bool,
    /// Ordered worker names or provider names for `PriorityFailover`.
    pub worker_priority: Vec<String>,
    /// In `PriorityFailover`, keep a conversation on an overflow worker once it
    /// has built a warm prefix there and the overflow worker is idle.
    pub sticky_after_overflow: bool,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            strategy: RoutingStrategy::PrefixAware,
            spillover_mode: SpilloverMode::SpillOnBusy,
            min_prefix_bytes: DEFAULT_MIN_PREFIX_BYTES,
            prefix_ttl: DEFAULT_PREFIX_TTL,
            prefix_aware_queue: true,
            respect_context_window: true,
            prefer_loaded_models: true,
            allow_client_overrides: true,
            worker_priority: Vec::new(),
            sticky_after_overflow: true,
        }
    }
}

/// Why the router selected a particular worker for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    SingleCandidate,
    PrefixHit,
    SessionHit,
    ConcurrentSpillover,
    WarmBusySticky,
    ColdSpread,
    PriorityPrimary,
    PriorityOverflow,
    LeastLoaded,
    RoundRobin,
    QueuedPrefixMatch,
    QueuedFifo,
}

/// Outcome returned by the smart router for observability and testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub worker_id: String,
    pub reason: RouteReason,
    pub matched_prefix_bytes: usize,
    pub effective_matched_bytes: usize,
}

/// Model residency state advertised by router-mode backends (llama.cpp router, LM Studio).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelLoadedState {
    /// Explicitly reported as unloaded / sleeping / downloading on this worker.
    Unloaded = 0,
    /// Backend does not report residency state (dedicated single-model server).
    Unknown = 1,
    /// Explicitly reported as currently loaded in VRAM on this worker.
    Loaded = 2,
}

/// Snapshot of an eligible worker passed into the candidate selector.
#[derive(Debug, Clone)]
pub struct CandidateWorker {
    pub worker_id: String,
    pub worker_name: String,
    pub provider: String,
    pub worker_order_index: usize,
    pub selection_load: usize,
    pub max_concurrent: usize,
    pub context_window: Option<usize>,
    pub loaded_state: ModelLoadedState,
}

impl CandidateWorker {
    #[allow(clippy::cast_precision_loss)]
    fn load_ratio(&self) -> f64 {
        self.selection_load as f64 / self.max_concurrent.max(1) as f64
    }

    fn is_idle(&self) -> bool {
        self.selection_load == 0
    }
}

/// Extract a model's advertised context window from backend `/v1/models` metadata.
#[must_use]
pub fn extract_model_context_window(meta: &serde_json::Map<String, Value>) -> Option<usize> {
    let positive = |v: Option<&Value>| -> Option<usize> {
        v.and_then(Value::as_u64)
            .filter(|&n| n > 0)
            .and_then(|n| usize::try_from(n).ok())
    };

    positive(meta.get("loaded_context_length"))
        .or_else(|| positive(meta.get("meta").and_then(|m| m.get("n_ctx"))))
        .or_else(|| positive(meta.get("context_window")))
        .or_else(|| positive(meta.get("contextWindow")))
        .or_else(|| positive(meta.get("context_length")))
        .or_else(|| positive(meta.get("max_model_len")))
        .or_else(|| positive(meta.get("max_context_length")))
        .or_else(|| positive(meta.get("n_ctx")))
        .or_else(|| positive(meta.get("limits").and_then(|l| l.get("context_length"))))
        .or_else(|| positive(meta.get("limits").and_then(|l| l.get("context_window"))))
        .or_else(|| {
            positive(
                meta.get("top_provider")
                    .and_then(|tp| tp.get("context_length")),
            )
        })
}

/// Determine whether a worker's `/v1/models` metadata indicates the model is
/// currently loaded in VRAM, unloaded/sleeping, or unknown (dedicated backend).
#[must_use]
pub fn extract_model_loaded_state(meta: &serde_json::Map<String, Value>) -> ModelLoadedState {
    if let Some(status_val) = meta
        .get("status")
        .and_then(|s| s.get("value"))
        .and_then(Value::as_str)
    {
        return match status_val.to_ascii_lowercase().as_str() {
            "loaded" => ModelLoadedState::Loaded,
            "unloaded" | "sleeping" | "loading" | "downloading" => ModelLoadedState::Unloaded,
            _ => ModelLoadedState::Unknown,
        };
    }

    if let Some(state_str) = meta.get("state").and_then(Value::as_str) {
        return match state_str.to_ascii_lowercase().as_str() {
            "loaded" => ModelLoadedState::Loaded,
            "not-loaded" | "unloaded" => ModelLoadedState::Unloaded,
            _ => ModelLoadedState::Unknown,
        };
    }

    if let Some(Value::Array(instances)) = meta.get("loaded_instances") {
        return if instances.is_empty() {
            ModelLoadedState::Unloaded
        } else {
            ModelLoadedState::Loaded
        };
    }

    ModelLoadedState::Unknown
}

/// Snapshot of routing telemetry counters for admin/dashboard observability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RoutingTelemetrySnapshot {
    pub total_routed: u64,
    pub prefix_hits: u64,
    pub session_hits: u64,
    pub concurrent_spillovers: u64,
    pub warm_busy_stickies: u64,
    pub cold_spreads: u64,
    pub priority_primaries: u64,
    pub priority_overflows: u64,
    pub least_loaded_routes: u64,
    pub round_robin_routes: u64,
    pub queued_prefix_matches: u64,
    pub queued_fifo_dispatches: u64,
    pub context_guard_skips: u64,
    pub total_matched_prefix_bytes: u64,
    pub tracked_prefix_nodes: usize,
}

/// Snapshot of per-`(worker, model)` speed and health metrics.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkerModelMetricsSnapshot {
    pub ewma_ttft_ms: Option<f64>,
    pub consecutive_errors: u32,
    pub total_dispatches: u64,
    pub eviction_epoch: u64,
}

/// Per-`(worker_id, model)` runtime telemetry used for speed tie-breaking,
/// error soft-penalties, LRU cold spreading, and KV-cache eviction decay.
#[derive(Debug, Clone, Default)]
struct WorkerModelMetrics {
    ewma_ttft_ms: Option<f64>,
    consecutive_errors: u32,
    last_error_at: Option<Instant>,
    last_dispatched_seq: u64,
    total_dispatches: u64,
    eviction_epoch: u64,
}

impl WorkerModelMetrics {
    fn is_penalized(&self, now: Instant) -> bool {
        self.consecutive_errors > 0
            && self
                .last_error_at
                .is_some_and(|at| now.saturating_duration_since(at) < ERROR_PENALTY_WINDOW)
    }

    fn record_ttft(&mut self, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.0;
        self.ewma_ttft_ms = Some(match self.ewma_ttft_ms {
            Some(prev) => prev * (1.0 - TTFT_EWMA_ALPHA) + ms * TTFT_EWMA_ALPHA,
            None => ms,
        });
        self.consecutive_errors = 0;
        self.last_error_at = None;
    }

    fn record_success(&mut self) {
        self.consecutive_errors = 0;
        self.last_error_at = None;
    }

    fn record_error(&mut self, now: Instant) {
        self.consecutive_errors = self.consecutive_errors.saturating_add(1);
        self.last_error_at = Some(now);
    }

    fn snapshot(&self) -> WorkerModelMetricsSnapshot {
        WorkerModelMetricsSnapshot {
            ewma_ttft_ms: self.ewma_ttft_ms,
            consecutive_errors: self.consecutive_errors,
            total_dispatches: self.total_dispatches,
            eviction_epoch: self.eviction_epoch,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TenantModelKey {
    owner_key_id: Option<String>,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WorkerModelKey {
    worker_id: String,
    model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SessionAffinityKey {
    owner_key_id: Option<String>,
    model: String,
    session_key: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ResponseAffinityKey {
    owner_key_id: Option<String>,
    response_id: String,
}

#[derive(Debug, Clone)]
struct AffinityTarget {
    worker_id: String,
    last_used_at: Instant,
    last_used_seq: u64,
}

#[derive(Debug, Clone, Copy)]
struct WorkerPrefixStamp {
    last_used_at: Instant,
    last_used_seq: u64,
    eviction_epoch: u64,
}

#[derive(Debug, Clone)]
struct TrieNode {
    parent: Option<u64>,
    edge_hash: u64,
    cumulative_bytes: usize,
    children: HashMap<u64, u64>,
    workers: HashMap<String, WorkerPrefixStamp>,
    last_touched_seq: u64,
}

#[derive(Debug, Clone)]
struct TenantModelTrie {
    next_node_id: u64,
    nodes: HashMap<u64, TrieNode>,
}

impl Default for TenantModelTrie {
    fn default() -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(
            0,
            TrieNode {
                parent: None,
                edge_hash: 0,
                cumulative_bytes: 0,
                children: HashMap::new(),
                workers: HashMap::new(),
                last_touched_seq: 0,
            },
        );
        Self {
            next_node_id: 1,
            nodes,
        }
    }
}

impl TenantModelTrie {
    fn non_root_len(&self) -> usize {
        self.nodes.len().saturating_sub(1)
    }

    /// Evict up to `count` least-recently-touched leaf nodes while preserving
    /// prefix closure (`if a node exists, all of its ancestors exist`).
    fn evict_lru_leaves(&mut self, count: usize) -> usize {
        let mut evicted = 0;
        while evicted < count {
            let mut leaves: Vec<(u64, u64)> = self
                .nodes
                .iter()
                .filter(|&(&id, node)| id != 0 && node.children.is_empty())
                .map(|(&id, node)| (node.last_touched_seq, id))
                .collect();
            if leaves.is_empty() {
                break;
            }
            leaves.sort_unstable_by_key(|&(seq, _)| seq);
            let to_remove = (count - evicted).min(leaves.len());
            for &(_, leaf_id) in &leaves[..to_remove] {
                if let Some(removed) = self.nodes.remove(&leaf_id) {
                    if let Some(parent_id) = removed.parent
                        && let Some(parent) = self.nodes.get_mut(&parent_id)
                    {
                        parent.children.remove(&removed.edge_hash);
                    }
                    evicted += 1;
                }
            }
        }
        evicted
    }

    fn sweep_expired(
        &mut self,
        now: Instant,
        ttl: Duration,
        disconnected_workers: &HashSet<String>,
    ) -> usize {
        for node in self.nodes.values_mut() {
            node.workers.retain(|worker_id, stamp| {
                !disconnected_workers.contains(worker_id)
                    && now.saturating_duration_since(stamp.last_used_at) <= ttl
            });
        }

        let mut removed_count = 0;
        loop {
            let empty_leaves: Vec<u64> = self
                .nodes
                .iter()
                .filter(|&(&id, node)| {
                    id != 0 && node.children.is_empty() && node.workers.is_empty()
                })
                .map(|(&id, _)| id)
                .collect();
            if empty_leaves.is_empty() {
                break;
            }
            for leaf_id in empty_leaves {
                if let Some(removed) = self.nodes.remove(&leaf_id) {
                    if let Some(parent_id) = removed.parent
                        && let Some(parent) = self.nodes.get_mut(&parent_id)
                    {
                        parent.children.remove(&removed.edge_hash);
                    }
                    removed_count += 1;
                }
            }
        }
        removed_count
    }
}

/// Result of matching a request's prefix checkpoints against one worker.
#[derive(Debug, Clone, Copy, Default)]
pub struct PrefixMatch {
    /// Raw longest matched prefix length in canonical bytes.
    pub raw_matched_bytes: usize,
    /// Eviction-decayed effective matched prefix bytes.
    pub effective_matched_bytes: usize,
    /// Sequence number when this worker last saw this prefix.
    pub last_used_seq: u64,
}

/// Central in-memory routing state owned by `ProxyServerCore`.
#[derive(Debug, Default)]
pub struct RoutingState {
    dispatch_seq: u64,
    tries: HashMap<TenantModelKey, TenantModelTrie>,
    total_prefix_nodes: usize,
    disconnected_workers: HashSet<String>,
    session_affinity: HashMap<SessionAffinityKey, AffinityTarget>,
    response_affinity: HashMap<ResponseAffinityKey, AffinityTarget>,
    worker_metrics: HashMap<WorkerModelKey, WorkerModelMetrics>,
    telemetry: RoutingTelemetrySnapshot,
    last_decision: Option<RouteDecision>,
}

impl RoutingState {
    /// Walk the `(owner_key_id, model)` prefix trie once and compute the deepest
    /// matched prefix for every candidate worker.
    #[must_use]
    pub fn match_prefix_for_candidates(
        &self,
        owner_key_id: Option<&str>,
        model: &str,
        checkpoints: &[PrefixCheckpoint],
        candidates: &[CandidateWorker],
        now: Instant,
        ttl: Duration,
    ) -> HashMap<String, PrefixMatch> {
        let mut results = HashMap::new();
        if checkpoints.is_empty() || candidates.is_empty() {
            return results;
        }

        let key = TenantModelKey {
            owner_key_id: owner_key_id.map(ToOwned::to_owned),
            model: model.to_string(),
        };
        let Some(trie) = self.tries.get(&key) else {
            return results;
        };

        let candidate_caps: HashMap<&str, usize> = candidates
            .iter()
            .map(|c| (c.worker_id.as_str(), c.max_concurrent.max(1)))
            .collect();

        let mut node_id = 0_u64;
        for cp in checkpoints {
            let Some(node) = trie.nodes.get(&node_id) else {
                break;
            };
            let Some(&next_id) = node.children.get(&cp.hash) else {
                break;
            };
            node_id = next_id;
            let Some(matched_node) = trie.nodes.get(&node_id) else {
                break;
            };

            for (worker_id, stamp) in &matched_node.workers {
                let Some(&max_concurrent) = candidate_caps.get(worker_id.as_str()) else {
                    continue;
                };
                if self.disconnected_workers.contains(worker_id)
                    || now.saturating_duration_since(stamp.last_used_at) > ttl
                {
                    continue;
                }
                let current_epoch = self
                    .worker_metrics
                    .get(&WorkerModelKey {
                        worker_id: worker_id.clone(),
                        model: model.to_string(),
                    })
                    .map_or(0, |m| m.eviction_epoch);
                let effective = apply_eviction_decay(
                    matched_node.cumulative_bytes,
                    current_epoch.saturating_sub(stamp.eviction_epoch),
                    max_concurrent,
                );
                results.insert(
                    worker_id.clone(),
                    PrefixMatch {
                        raw_matched_bytes: matched_node.cumulative_bytes,
                        effective_matched_bytes: effective,
                        last_used_seq: stamp.last_used_seq,
                    },
                );
            }
        }

        results
    }

    /// Compute the effective prefix match for a single `(worker_id, max_concurrent)`
    /// against a queued request (used by bounded prefix-aware queue draining).
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn match_prefix_for_single_worker(
        &self,
        owner_key_id: Option<&str>,
        model: &str,
        hints: &RequestRoutingHints,
        worker_id: &str,
        max_concurrent: usize,
        now: Instant,
        ttl: Duration,
    ) -> usize {
        if self.disconnected_workers.contains(worker_id) {
            return 0;
        }

        let mut best_effective = 0_usize;

        let key = TenantModelKey {
            owner_key_id: owner_key_id.map(ToOwned::to_owned),
            model: model.to_string(),
        };
        if let Some(trie) = self.tries.get(&key) {
            let current_epoch = self
                .worker_metrics
                .get(&WorkerModelKey {
                    worker_id: worker_id.to_string(),
                    model: model.to_string(),
                })
                .map_or(0, |m| m.eviction_epoch);
            let mut node_id = 0_u64;
            for cp in &hints.prefix_checkpoints {
                let Some(node) = trie.nodes.get(&node_id) else {
                    break;
                };
                let Some(&next_id) = node.children.get(&cp.hash) else {
                    break;
                };
                node_id = next_id;
                let Some(matched_node) = trie.nodes.get(&node_id) else {
                    break;
                };
                if let Some(stamp) = matched_node.workers.get(worker_id)
                    && now.saturating_duration_since(stamp.last_used_at) <= ttl
                {
                    let effective = apply_eviction_decay(
                        matched_node.cumulative_bytes,
                        current_epoch.saturating_sub(stamp.eviction_epoch),
                        max_concurrent.max(1),
                    );
                    best_effective = best_effective.max(effective);
                }
            }
        }

        if best_effective == 0
            && self
                .lookup_explicit_affinity(owner_key_id, model, hints, now, ttl)
                .is_some_and(|w| w == worker_id)
        {
            best_effective = DEFAULT_MIN_PREFIX_BYTES;
        }

        best_effective
    }

    /// Look up an explicit affinity target from `previous_response_id` (first)
    /// or `session_key` (second).
    #[must_use]
    pub fn lookup_explicit_affinity(
        &self,
        owner_key_id: Option<&str>,
        model: &str,
        hints: &RequestRoutingHints,
        now: Instant,
        ttl: Duration,
    ) -> Option<&str> {
        if let Some(ref prev_resp_id) = hints.previous_response_id {
            let key = ResponseAffinityKey {
                owner_key_id: owner_key_id.map(ToOwned::to_owned),
                response_id: prev_resp_id.clone(),
            };
            if let Some(target) = self.response_affinity.get(&key)
                && !self.disconnected_workers.contains(&target.worker_id)
                && now.saturating_duration_since(target.last_used_at) <= ttl
            {
                return Some(target.worker_id.as_str());
            }
        }

        if let Some(session_key) = hints.session_key {
            let key = SessionAffinityKey {
                owner_key_id: owner_key_id.map(ToOwned::to_owned),
                model: model.to_string(),
                session_key,
            };
            if let Some(target) = self.session_affinity.get(&key)
                && !self.disconnected_workers.contains(&target.worker_id)
                && now.saturating_duration_since(target.last_used_at) <= ttl
            {
                return Some(target.worker_id.as_str());
            }
        }

        None
    }

    /// Select the best candidate worker according to `policy` and `hints`.
    ///
    /// `round_robin_pick` is invoked only when `RoundRobin` or tie-breaking cursor
    /// advancement is requested so `ProxyServerCore`'s cursor map remains the single
    /// source of truth for round-robin state.
    #[allow(clippy::too_many_arguments)]
    pub fn select_worker<F>(
        &mut self,
        owner_key_id: Option<&str>,
        model: &str,
        candidates: &[CandidateWorker],
        hints: &RequestRoutingHints,
        policy: &RoutingPolicy,
        now: Instant,
        round_robin_pick: F,
    ) -> Option<RouteDecision>
    where
        F: FnOnce(&[&CandidateWorker]) -> Option<String>,
    {
        if candidates.is_empty() {
            return None;
        }

        // Context-window guard: when prompt estimate exceeds a worker's advertised
        // context window (95% headroom) and at least one larger-context candidate exists,
        // avoid routing to the undersized worker.
        let filtered_refs: Vec<&CandidateWorker> =
            if policy.respect_context_window && hints.estimated_prompt_tokens() > 0 {
                let needed = hints.estimated_prompt_tokens();
                let fitting: Vec<&CandidateWorker> = candidates
                    .iter()
                    .filter(|c| c.context_window.is_none_or(|ctx| needed * 100 <= ctx * 95))
                    .collect();
                if !fitting.is_empty() && fitting.len() < candidates.len() {
                    self.telemetry.context_guard_skips =
                        self.telemetry.context_guard_skips.saturating_add(1);
                    fitting
                } else {
                    candidates.iter().collect()
                }
            } else {
                candidates.iter().collect()
            };

        let filtered: Vec<CandidateWorker> = filtered_refs.into_iter().cloned().collect();

        if filtered.len() == 1 {
            let only = &filtered[0];
            let matches = self.match_prefix_for_candidates(
                owner_key_id,
                model,
                &hints.prefix_checkpoints,
                &filtered,
                now,
                policy.prefix_ttl,
            );
            let m = matches.get(&only.worker_id).copied().unwrap_or_default();
            return Some(RouteDecision {
                worker_id: only.worker_id.clone(),
                reason: RouteReason::SingleCandidate,
                matched_prefix_bytes: m.raw_matched_bytes,
                effective_matched_bytes: m.effective_matched_bytes,
            });
        }

        match policy.strategy {
            RoutingStrategy::RoundRobin => {
                let refs: Vec<&CandidateWorker> = filtered.iter().collect();
                let worker_id = round_robin_pick(&refs)?;
                Some(RouteDecision {
                    worker_id,
                    reason: RouteReason::RoundRobin,
                    matched_prefix_bytes: 0,
                    effective_matched_bytes: 0,
                })
            }
            RoutingStrategy::LeastLoaded => {
                Some(self.select_least_loaded(model, &filtered, policy, now, round_robin_pick))
            }
            RoutingStrategy::PriorityFailover => Some(self.select_priority_failover(
                owner_key_id,
                model,
                &filtered,
                hints,
                policy,
                now,
            )),
            RoutingStrategy::PrefixAware => {
                Some(self.select_prefix_aware(owner_key_id, model, &filtered, hints, policy, now))
            }
        }
    }

    fn select_least_loaded<F>(
        &self,
        model: &str,
        candidates: &[CandidateWorker],
        policy: &RoutingPolicy,
        now: Instant,
        round_robin_pick: F,
    ) -> RouteDecision
    where
        F: FnOnce(&[&CandidateWorker]) -> Option<String>,
    {
        let mut best_load = f64::INFINITY;
        for c in candidates {
            let lr = c.load_ratio();
            if lr < best_load {
                best_load = lr;
            }
        }
        let lowest: Vec<&CandidateWorker> = candidates
            .iter()
            .filter(|c| (c.load_ratio() - best_load).abs() < 1e-9)
            .collect();

        // Narrow by health and loaded state if they distinguish candidates.
        let healthy: Vec<&CandidateWorker> = lowest
            .iter()
            .copied()
            .filter(|c| !self.worker_is_penalized(&c.worker_id, model, now))
            .collect();
        let pool = if healthy.is_empty() { lowest } else { healthy };

        let pool = if policy.prefer_loaded_models {
            let max_loaded = pool
                .iter()
                .map(|c| c.loaded_state)
                .max()
                .unwrap_or(ModelLoadedState::Unknown);
            pool.into_iter()
                .filter(|c| c.loaded_state == max_loaded)
                .collect()
        } else {
            pool
        };

        let worker_id = if pool.len() == 1 {
            pool[0].worker_id.clone()
        } else {
            // Prefer fastest EWMA TTFT if all tied workers have speed telemetry, else round-robin.
            let all_have_ttft = pool
                .iter()
                .all(|c| self.worker_ewma_ttft(&c.worker_id, model).is_some());
            if all_have_ttft {
                pool.iter()
                    .min_by(|a, b| {
                        let ta = self
                            .worker_ewma_ttft(&a.worker_id, model)
                            .unwrap_or(f64::INFINITY);
                        let tb = self
                            .worker_ewma_ttft(&b.worker_id, model)
                            .unwrap_or(f64::INFINITY);
                        ta.total_cmp(&tb)
                            .then_with(|| a.worker_order_index.cmp(&b.worker_order_index))
                    })
                    .map_or_else(|| pool[0].worker_id.clone(), |c| c.worker_id.clone())
            } else {
                round_robin_pick(&pool).unwrap_or_else(|| pool[0].worker_id.clone())
            }
        };

        RouteDecision {
            worker_id,
            reason: RouteReason::LeastLoaded,
            matched_prefix_bytes: 0,
            effective_matched_bytes: 0,
        }
    }

    fn select_priority_failover(
        &self,
        owner_key_id: Option<&str>,
        model: &str,
        candidates: &[CandidateWorker],
        hints: &RequestRoutingHints,
        policy: &RoutingPolicy,
        now: Instant,
    ) -> RouteDecision {
        let matches = self.match_prefix_for_candidates(
            owner_key_id,
            model,
            &hints.prefix_checkpoints,
            candidates,
            now,
            policy.prefix_ttl,
        );
        let session_worker =
            self.lookup_explicit_affinity(owner_key_id, model, hints, now, policy.prefix_ttl);

        // Optional sticky-after-overflow: if a conversation already spilled to a
        // lower-tier worker that is idle and holds a strong warm prefix while no
        // higher-tier idle worker holds it, stay warm on the overflow worker.
        if policy.sticky_after_overflow {
            let best_idle_warm = candidates
                .iter()
                .filter(|c| c.is_idle() && !self.worker_is_penalized(&c.worker_id, model, now))
                .filter_map(|c| {
                    let m = matches.get(&c.worker_id)?;
                    (m.effective_matched_bytes >= policy.min_prefix_bytes).then_some((c, *m))
                })
                .max_by(|(ca, ma), (cb, mb)| {
                    ma.effective_matched_bytes
                        .cmp(&mb.effective_matched_bytes)
                        .then_with(|| {
                            worker_priority_rank(cb, &policy.worker_priority)
                                .cmp(&worker_priority_rank(ca, &policy.worker_priority))
                        })
                });
            if let Some((chosen, m)) = best_idle_warm {
                let rank = worker_priority_rank(chosen, &policy.worker_priority);
                return RouteDecision {
                    worker_id: chosen.worker_id.clone(),
                    reason: if rank == 0 {
                        RouteReason::PriorityPrimary
                    } else {
                        RouteReason::PrefixHit
                    },
                    matched_prefix_bytes: m.raw_matched_bytes,
                    effective_matched_bytes: m.effective_matched_bytes,
                };
            }
        }

        // Partition candidates by priority tier.
        let mut by_tier: Vec<(usize, Vec<CandidateWorker>)> = Vec::new();
        for c in candidates {
            let rank = worker_priority_rank(c, &policy.worker_priority);
            if let Some((_, group)) = by_tier.iter_mut().find(|(r, _)| *r == rank) {
                group.push(c.clone());
            } else {
                by_tier.push((rank, vec![c.clone()]));
            }
        }
        by_tier.sort_by_key(|(rank, _)| *rank);

        let highest_tier_rank = by_tier[0].0;

        // First look for the highest-priority tier with an IDLE healthy worker
        // (unless SpilloverMode is StrictSticky, where we fill Tier 1 before overflowing).
        let chosen_tier = if policy.spillover_mode == SpilloverMode::StrictSticky {
            by_tier
                .iter()
                .find(|(_, grp)| {
                    grp.iter()
                        .any(|c| !self.worker_is_penalized(&c.worker_id, model, now))
                })
                .unwrap_or(&by_tier[0])
        } else {
            by_tier
                .iter()
                .find(|(_, grp)| {
                    grp.iter()
                        .any(|c| c.is_idle() && !self.worker_is_penalized(&c.worker_id, model, now))
                })
                .or_else(|| {
                    by_tier.iter().find(|(_, grp)| {
                        grp.iter()
                            .any(|c| !self.worker_is_penalized(&c.worker_id, model, now))
                    })
                })
                .unwrap_or(&by_tier[0])
        };

        let (tier_rank, tier_candidates) = chosen_tier;
        let mut inner = self.select_prefix_aware_with_matches(
            model,
            tier_candidates,
            hints,
            policy,
            now,
            &matches,
            session_worker,
        );
        inner.reason = if *tier_rank == highest_tier_rank {
            RouteReason::PriorityPrimary
        } else {
            RouteReason::PriorityOverflow
        };
        inner
    }

    fn select_prefix_aware(
        &self,
        owner_key_id: Option<&str>,
        model: &str,
        candidates: &[CandidateWorker],
        hints: &RequestRoutingHints,
        policy: &RoutingPolicy,
        now: Instant,
    ) -> RouteDecision {
        let matches = self.match_prefix_for_candidates(
            owner_key_id,
            model,
            &hints.prefix_checkpoints,
            candidates,
            now,
            policy.prefix_ttl,
        );
        let session_worker =
            self.lookup_explicit_affinity(owner_key_id, model, hints, now, policy.prefix_ttl);

        self.select_prefix_aware_with_matches(
            model,
            candidates,
            hints,
            policy,
            now,
            &matches,
            session_worker,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn select_prefix_aware_with_matches(
        &self,
        model: &str,
        candidates: &[CandidateWorker],
        hints: &RequestRoutingHints,
        policy: &RoutingPolicy,
        now: Instant,
        matches: &HashMap<String, PrefixMatch>,
        session_worker: Option<&str>,
    ) -> RouteDecision {
        let idle: Vec<&CandidateWorker> = candidates.iter().filter(|c| c.is_idle()).collect();
        let busy: Vec<&CandidateWorker> = candidates.iter().filter(|c| !c.is_idle()).collect();

        // Case A: Warm prefix (or session) match on at least one IDLE worker -> STICK!
        let best_idle_prefix = idle
            .iter()
            .copied()
            .filter_map(|c| {
                let m = matches.get(&c.worker_id)?;
                (m.effective_matched_bytes >= policy.min_prefix_bytes).then_some((c, *m))
            })
            .max_by(|(ca, ma), (cb, mb)| {
                let health_a = !self.worker_is_penalized(&ca.worker_id, model, now);
                let health_b = !self.worker_is_penalized(&cb.worker_id, model, now);
                health_a
                    .cmp(&health_b)
                    .then_with(|| ma.effective_matched_bytes.cmp(&mb.effective_matched_bytes))
                    .then_with(|| {
                        if policy.prefer_loaded_models {
                            ca.loaded_state.cmp(&cb.loaded_state)
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
                    .then_with(|| ma.last_used_seq.cmp(&mb.last_used_seq))
                    .then_with(|| cb.worker_order_index.cmp(&ca.worker_order_index))
            });

        if let Some((chosen, m)) = best_idle_prefix {
            return RouteDecision {
                worker_id: chosen.worker_id.clone(),
                reason: RouteReason::PrefixHit,
                matched_prefix_bytes: m.raw_matched_bytes,
                effective_matched_bytes: m.effective_matched_bytes,
            };
        }

        if let Some(sess_w) = session_worker
            && let Some(chosen) = idle.iter().copied().find(|c| {
                c.worker_id == sess_w && !self.worker_is_penalized(&c.worker_id, model, now)
            })
        {
            let m = matches.get(&chosen.worker_id).copied().unwrap_or_default();
            return RouteDecision {
                worker_id: chosen.worker_id.clone(),
                reason: RouteReason::SessionHit,
                matched_prefix_bytes: m.raw_matched_bytes,
                effective_matched_bytes: m.effective_matched_bytes,
            };
        }

        // Case B: Warm match ONLY on a BUSY worker (concurrent same-prefix request!).
        let best_busy_warm = busy
            .iter()
            .copied()
            .filter_map(|c| {
                let m = matches.get(&c.worker_id).copied().unwrap_or_default();
                let is_session = session_worker.is_some_and(|sw| sw == c.worker_id);
                if m.effective_matched_bytes >= policy.min_prefix_bytes || is_session {
                    Some((c, m, is_session))
                } else {
                    None
                }
            })
            .max_by(|(ca, ma, sa), (cb, mb, sb)| {
                let health_a = !self.worker_is_penalized(&ca.worker_id, model, now);
                let health_b = !self.worker_is_penalized(&cb.worker_id, model, now);
                health_a
                    .cmp(&health_b)
                    .then_with(|| ma.effective_matched_bytes.cmp(&mb.effective_matched_bytes))
                    .then_with(|| sa.cmp(sb))
                    .then_with(|| cb.load_ratio().total_cmp(&ca.load_ratio()))
                    .then_with(|| cb.worker_order_index.cmp(&ca.worker_order_index))
            });

        if let Some((warm_busy, warm_match, _)) = best_busy_warm {
            if !idle.is_empty() {
                match policy.spillover_mode {
                    SpilloverMode::SpillOnBusy => {
                        let spill_target =
                            self.pick_best_spill_idle(&idle, matches, model, policy, now);
                        let m = matches
                            .get(&spill_target.worker_id)
                            .copied()
                            .unwrap_or_default();
                        return RouteDecision {
                            worker_id: spill_target.worker_id.clone(),
                            reason: RouteReason::ConcurrentSpillover,
                            matched_prefix_bytes: m.raw_matched_bytes,
                            effective_matched_bytes: m.effective_matched_bytes,
                        };
                    }
                    SpilloverMode::CostBalanced => {
                        let spill_target =
                            self.pick_best_spill_idle(&idle, matches, model, policy, now);
                        let spill_match = matches
                            .get(&spill_target.worker_id)
                            .copied()
                            .unwrap_or_default();
                        let busy_cost = estimate_cost_balanced_ms(
                            hints.total_prompt_bytes,
                            warm_match.effective_matched_bytes,
                            warm_busy.load_ratio(),
                            self.worker_is_penalized(&warm_busy.worker_id, model, now),
                        );
                        let idle_cost = estimate_cost_balanced_ms(
                            hints.total_prompt_bytes,
                            spill_match.effective_matched_bytes,
                            0.0,
                            self.worker_is_penalized(&spill_target.worker_id, model, now),
                        );
                        if busy_cost < idle_cost {
                            return RouteDecision {
                                worker_id: warm_busy.worker_id.clone(),
                                reason: RouteReason::WarmBusySticky,
                                matched_prefix_bytes: warm_match.raw_matched_bytes,
                                effective_matched_bytes: warm_match.effective_matched_bytes,
                            };
                        }
                        return RouteDecision {
                            worker_id: spill_target.worker_id.clone(),
                            reason: RouteReason::ConcurrentSpillover,
                            matched_prefix_bytes: spill_match.raw_matched_bytes,
                            effective_matched_bytes: spill_match.effective_matched_bytes,
                        };
                    }
                    SpilloverMode::StrictSticky => {
                        return RouteDecision {
                            worker_id: warm_busy.worker_id.clone(),
                            reason: RouteReason::WarmBusySticky,
                            matched_prefix_bytes: warm_match.raw_matched_bytes,
                            effective_matched_bytes: warm_match.effective_matched_bytes,
                        };
                    }
                }
            }

            // All eligible workers are busy: balance load ratio against prefix match ratio.
            let best = busy
                .iter()
                .copied()
                .min_by(|a, b| {
                    let score_a = self.busy_combined_score(
                        a,
                        matches.get(&a.worker_id).copied().unwrap_or_default(),
                        hints.total_prompt_bytes,
                        model,
                        now,
                    );
                    let score_b = self.busy_combined_score(
                        b,
                        matches.get(&b.worker_id).copied().unwrap_or_default(),
                        hints.total_prompt_bytes,
                        model,
                        now,
                    );
                    score_a
                        .total_cmp(&score_b)
                        .then_with(|| a.worker_order_index.cmp(&b.worker_order_index))
                })
                .unwrap_or(warm_busy);
            let m = matches.get(&best.worker_id).copied().unwrap_or_default();
            let reason = if best.worker_id == warm_busy.worker_id {
                RouteReason::WarmBusySticky
            } else {
                RouteReason::ConcurrentSpillover
            };
            return RouteDecision {
                worker_id: best.worker_id.clone(),
                reason,
                matched_prefix_bytes: m.raw_matched_bytes,
                effective_matched_bytes: m.effective_matched_bytes,
            };
        }

        // Case C: Cold / unrelated request -> cache-preserving LRU spread!
        // Prefer idle workers first; within the pool pick healthy -> loaded ->
        // lowest load_ratio -> oldest last_dispatched_seq (LRU avoids evicting a
        // hot session on another machine!) -> fastest EWMA TTFT -> worker_order.
        let pool = if idle.is_empty() { &busy } else { &idle };
        let chosen = self.pick_cold_spread(pool, model, policy, now);
        let m = matches.get(&chosen.worker_id).copied().unwrap_or_default();
        RouteDecision {
            worker_id: chosen.worker_id.clone(),
            reason: RouteReason::ColdSpread,
            matched_prefix_bytes: m.raw_matched_bytes,
            effective_matched_bytes: m.effective_matched_bytes,
        }
    }

    fn pick_best_spill_idle<'a>(
        &self,
        idle: &[&'a CandidateWorker],
        matches: &HashMap<String, PrefixMatch>,
        model: &str,
        policy: &RoutingPolicy,
        now: Instant,
    ) -> &'a CandidateWorker {
        idle.iter()
            .copied()
            .min_by(|a, b| {
                let pen_a = self.worker_is_penalized(&a.worker_id, model, now);
                let pen_b = self.worker_is_penalized(&b.worker_id, model, now);
                let match_a = matches
                    .get(&a.worker_id)
                    .map_or(0, |m| m.effective_matched_bytes);
                let match_b = matches
                    .get(&b.worker_id)
                    .map_or(0, |m| m.effective_matched_bytes);
                let seq_a = self.worker_last_dispatched_seq(&a.worker_id, model);
                let seq_b = self.worker_last_dispatched_seq(&b.worker_id, model);
                pen_a
                    .cmp(&pen_b)
                    .then_with(|| {
                        if policy.prefer_loaded_models {
                            b.loaded_state.cmp(&a.loaded_state)
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
                    // Prefer an idle peer with a partial prefix match (e.g. shared system prompt)
                    .then_with(|| match_b.cmp(&match_a))
                    // Otherwise prefer least-recently-used worker to avoid evicting another session
                    .then_with(|| seq_a.cmp(&seq_b))
                    .then_with(|| self.compare_ewma_ttft(&a.worker_id, &b.worker_id, model))
                    .then_with(|| a.worker_order_index.cmp(&b.worker_order_index))
            })
            .unwrap_or(idle[0])
    }

    fn pick_cold_spread<'a>(
        &self,
        pool: &[&'a CandidateWorker],
        model: &str,
        policy: &RoutingPolicy,
        now: Instant,
    ) -> &'a CandidateWorker {
        pool.iter()
            .copied()
            .min_by(|a, b| {
                let pen_a = self.worker_is_penalized(&a.worker_id, model, now);
                let pen_b = self.worker_is_penalized(&b.worker_id, model, now);
                let seq_a = self.worker_last_dispatched_seq(&a.worker_id, model);
                let seq_b = self.worker_last_dispatched_seq(&b.worker_id, model);
                pen_a
                    .cmp(&pen_b)
                    .then_with(|| {
                        if policy.prefer_loaded_models {
                            b.loaded_state.cmp(&a.loaded_state)
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
                    .then_with(|| a.load_ratio().total_cmp(&b.load_ratio()))
                    .then_with(|| seq_a.cmp(&seq_b))
                    .then_with(|| self.compare_ewma_ttft(&a.worker_id, &b.worker_id, model))
                    .then_with(|| a.worker_order_index.cmp(&b.worker_order_index))
            })
            .unwrap_or(pool[0])
    }

    #[allow(clippy::cast_precision_loss)]
    fn busy_combined_score(
        &self,
        worker: &CandidateWorker,
        prefix_match: PrefixMatch,
        total_prompt_bytes: usize,
        model: &str,
        now: Instant,
    ) -> f64 {
        let load = worker.load_ratio();
        let match_ratio = if total_prompt_bytes > 0 {
            (prefix_match.effective_matched_bytes as f64 / total_prompt_bytes as f64)
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let error_pen = if self.worker_is_penalized(&worker.worker_id, model, now) {
            2.0
        } else {
            0.0
        };
        let unloaded_pen = if worker.loaded_state == ModelLoadedState::Unloaded {
            0.5
        } else {
            0.0
        };
        load - (match_ratio * 0.6) + error_pen + unloaded_pen
    }

    /// Record a request dispatch to `worker_id`: updates prefix trie, eviction
    /// epoch (when an unrelated prompt lands on this worker), session affinity,
    /// `last_dispatched_seq`, and telemetry counters.
    #[allow(clippy::too_many_arguments)]
    pub fn record_dispatch(
        &mut self,
        owner_key_id: Option<&str>,
        model: &str,
        worker_id: &str,
        hints: &RequestRoutingHints,
        decision: RouteDecision,
        min_prefix_bytes: usize,
        now: Instant,
    ) {
        self.disconnected_workers.remove(worker_id);
        self.dispatch_seq = self.dispatch_seq.saturating_add(1);
        let seq = self.dispatch_seq;

        let wm_key = WorkerModelKey {
            worker_id: worker_id.to_string(),
            model: model.to_string(),
        };
        let current_epoch = {
            let metrics = self.worker_metrics.entry(wm_key).or_default();
            metrics.last_dispatched_seq = seq;
            metrics.total_dispatches = metrics.total_dispatches.saturating_add(1);
            // If a non-trivial prompt arrived with no meaningful overlap against
            // this worker's existing cache, bump its eviction epoch so prior
            // unrelated prefixes on this worker decay in confidence.
            if hints.total_prompt_bytes >= min_prefix_bytes
                && decision.matched_prefix_bytes < min_prefix_bytes
            {
                metrics.eviction_epoch = metrics.eviction_epoch.saturating_add(1);
            }
            metrics.eviction_epoch
        };

        if !hints.prefix_checkpoints.is_empty() {
            self.insert_prefix_checkpoints(
                owner_key_id,
                model,
                worker_id,
                &hints.prefix_checkpoints,
                now,
                seq,
                current_epoch,
            );
        }

        if let Some(session_key) = hints.session_key {
            if self.session_affinity.len() >= MAX_AFFINITY_ENTRIES {
                self.prune_oldest_affinity_entries();
            }
            self.session_affinity.insert(
                SessionAffinityKey {
                    owner_key_id: owner_key_id.map(ToOwned::to_owned),
                    model: model.to_string(),
                    session_key,
                },
                AffinityTarget {
                    worker_id: worker_id.to_string(),
                    last_used_at: now,
                    last_used_seq: seq,
                },
            );
        }

        // Update routing telemetry counters.
        self.telemetry.total_routed = self.telemetry.total_routed.saturating_add(1);
        self.telemetry.total_matched_prefix_bytes = self
            .telemetry
            .total_matched_prefix_bytes
            .saturating_add(decision.matched_prefix_bytes as u64);
        match decision.reason {
            RouteReason::PrefixHit => {
                self.telemetry.prefix_hits = self.telemetry.prefix_hits.saturating_add(1);
            }
            RouteReason::SessionHit => {
                self.telemetry.session_hits = self.telemetry.session_hits.saturating_add(1);
            }
            RouteReason::ConcurrentSpillover => {
                self.telemetry.concurrent_spillovers =
                    self.telemetry.concurrent_spillovers.saturating_add(1);
            }
            RouteReason::WarmBusySticky => {
                self.telemetry.warm_busy_stickies =
                    self.telemetry.warm_busy_stickies.saturating_add(1);
            }
            RouteReason::ColdSpread => {
                self.telemetry.cold_spreads = self.telemetry.cold_spreads.saturating_add(1);
            }
            RouteReason::PriorityPrimary => {
                self.telemetry.priority_primaries =
                    self.telemetry.priority_primaries.saturating_add(1);
            }
            RouteReason::PriorityOverflow => {
                self.telemetry.priority_overflows =
                    self.telemetry.priority_overflows.saturating_add(1);
            }
            RouteReason::LeastLoaded => {
                self.telemetry.least_loaded_routes =
                    self.telemetry.least_loaded_routes.saturating_add(1);
            }
            RouteReason::RoundRobin => {
                self.telemetry.round_robin_routes =
                    self.telemetry.round_robin_routes.saturating_add(1);
            }
            RouteReason::QueuedPrefixMatch => {
                self.telemetry.queued_prefix_matches =
                    self.telemetry.queued_prefix_matches.saturating_add(1);
            }
            RouteReason::QueuedFifo => {
                self.telemetry.queued_fifo_dispatches =
                    self.telemetry.queued_fifo_dispatches.saturating_add(1);
            }
            RouteReason::SingleCandidate => {}
        }
        self.telemetry.tracked_prefix_nodes = self.total_prefix_nodes;
        self.last_decision = Some(decision);
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_prefix_checkpoints(
        &mut self,
        owner_key_id: Option<&str>,
        model: &str,
        worker_id: &str,
        checkpoints: &[PrefixCheckpoint],
        now: Instant,
        seq: u64,
        eviction_epoch: u64,
    ) {
        let key = TenantModelKey {
            owner_key_id: owner_key_id.map(ToOwned::to_owned),
            model: model.to_string(),
        };

        let mut added_nodes = 0_usize;
        {
            let trie = self.tries.entry(key.clone()).or_default();
            let mut node_id = 0_u64;
            let stamp = WorkerPrefixStamp {
                last_used_at: now,
                last_used_seq: seq,
                eviction_epoch,
            };

            for cp in checkpoints {
                let next_id = if let Some(&existing_child) = trie
                    .nodes
                    .get(&node_id)
                    .and_then(|n| n.children.get(&cp.hash))
                {
                    existing_child
                } else {
                    let new_id = trie.next_node_id;
                    trie.next_node_id = trie.next_node_id.saturating_add(1);
                    if let Some(parent) = trie.nodes.get_mut(&node_id) {
                        parent.children.insert(cp.hash, new_id);
                        parent.last_touched_seq = seq;
                    }
                    trie.nodes.insert(
                        new_id,
                        TrieNode {
                            parent: Some(node_id),
                            edge_hash: cp.hash,
                            cumulative_bytes: cp.cumulative_bytes,
                            children: HashMap::new(),
                            workers: HashMap::new(),
                            last_touched_seq: seq,
                        },
                    );
                    added_nodes += 1;
                    new_id
                };

                if let Some(node) = trie.nodes.get_mut(&next_id) {
                    node.last_touched_seq = seq;
                    node.workers.insert(worker_id.to_string(), stamp);
                }
                node_id = next_id;
            }

            if trie.non_root_len() > MAX_TENANT_MODEL_PREFIX_NODES {
                let target = MAX_TENANT_MODEL_PREFIX_NODES * 9 / 10;
                let excess = trie.non_root_len().saturating_sub(target);
                let _ = trie.evict_lru_leaves(excess);
                self.total_prefix_nodes =
                    self.tries.values().map(TenantModelTrie::non_root_len).sum();
                added_nodes = 0;
            }
        }

        self.total_prefix_nodes = self.total_prefix_nodes.saturating_add(added_nodes);

        if self.total_prefix_nodes > MAX_GLOBAL_PREFIX_NODES {
            self.evict_global_prefix_nodes();
        }
    }

    fn evict_global_prefix_nodes(&mut self) {
        let target = MAX_GLOBAL_PREFIX_NODES * 9 / 10;
        let mut to_evict = self.total_prefix_nodes.saturating_sub(target);
        if to_evict == 0 {
            return;
        }
        // Evict proportionally from the largest tries first.
        let mut keys_by_size: Vec<(usize, TenantModelKey)> = self
            .tries
            .iter()
            .map(|(k, t)| (t.non_root_len(), k.clone()))
            .collect();
        keys_by_size.sort_unstable_by(|a, b| b.0.cmp(&a.0));

        for (_, key) in keys_by_size {
            if to_evict == 0 {
                break;
            }
            if let Some(trie) = self.tries.get_mut(&key) {
                let quota = (trie.non_root_len() / 4).max(1).min(to_evict);
                let evicted = trie.evict_lru_leaves(quota);
                to_evict = to_evict.saturating_sub(evicted);
            }
        }
        self.tries.retain(|_, t| t.non_root_len() > 0);
        self.total_prefix_nodes = self.tries.values().map(TenantModelTrie::non_root_len).sum();
    }

    fn prune_oldest_affinity_entries(&mut self) {
        let target = MAX_AFFINITY_ENTRIES * 3 / 4;
        if self.session_affinity.len() > target {
            let mut entries: Vec<(u64, SessionAffinityKey)> = self
                .session_affinity
                .iter()
                .map(|(k, v)| (v.last_used_seq, k.clone()))
                .collect();
            entries.sort_unstable_by_key(|&(seq, _)| seq);
            let remove_n = entries.len().saturating_sub(target);
            for (_, key) in entries.into_iter().take(remove_n) {
                self.session_affinity.remove(&key);
            }
        }
        if self.response_affinity.len() > target {
            let mut entries: Vec<(u64, ResponseAffinityKey)> = self
                .response_affinity
                .iter()
                .map(|(k, v)| (v.last_used_seq, k.clone()))
                .collect();
            entries.sort_unstable_by_key(|&(seq, _)| seq);
            let remove_n = entries.len().saturating_sub(target);
            for (_, key) in entries.into_iter().take(remove_n) {
                self.response_affinity.remove(&key);
            }
        }
    }

    /// Record an `OpenAI` Responses API `response.id -> worker_id` mapping so a
    /// subsequent `previous_response_id` call sticks to the same machine.
    pub fn record_response_affinity(
        &mut self,
        owner_key_id: Option<&str>,
        response_id: String,
        worker_id: &str,
        now: Instant,
    ) {
        if response_id.is_empty() || self.disconnected_workers.contains(worker_id) {
            return;
        }
        if self.response_affinity.len() >= MAX_AFFINITY_ENTRIES {
            self.prune_oldest_affinity_entries();
        }
        self.dispatch_seq = self.dispatch_seq.saturating_add(1);
        self.response_affinity.insert(
            ResponseAffinityKey {
                owner_key_id: owner_key_id.map(ToOwned::to_owned),
                response_id,
            },
            AffinityTarget {
                worker_id: worker_id.to_string(),
                last_used_at: now,
                last_used_seq: self.dispatch_seq,
            },
        );
    }

    /// Record a TTFT observation (first streaming chunk or unary 2xx completion).
    pub fn record_ttft(&mut self, worker_id: &str, model: &str, elapsed: Duration) {
        let key = WorkerModelKey {
            worker_id: worker_id.to_string(),
            model: model.to_string(),
        };
        self.worker_metrics
            .entry(key)
            .or_default()
            .record_ttft(elapsed);
    }

    /// Clear any active error streak on successful completion.
    pub fn record_success(&mut self, worker_id: &str, model: &str) {
        let key = WorkerModelKey {
            worker_id: worker_id.to_string(),
            model: model.to_string(),
        };
        if let Some(metrics) = self.worker_metrics.get_mut(&key) {
            metrics.record_success();
        }
    }

    /// Record a worker backend error or 5xx response to soft-deprioritize it temporarily.
    pub fn record_error(&mut self, worker_id: &str, model: &str, now: Instant) {
        let key = WorkerModelKey {
            worker_id: worker_id.to_string(),
            model: model.to_string(),
        };
        self.worker_metrics
            .entry(key)
            .or_default()
            .record_error(now);
    }

    /// O(1) disconnect invalidation: marks `worker_id` disconnected and drops its
    /// affinity and speed metrics; trie node cleanup runs in [`Self::sweep_expired`].
    pub fn on_worker_disconnected(&mut self, worker_id: &str) {
        self.disconnected_workers.insert(worker_id.to_string());
        self.worker_metrics.retain(|k, _| k.worker_id != worker_id);
        self.session_affinity
            .retain(|_, v| v.worker_id != worker_id);
        self.response_affinity
            .retain(|_, v| v.worker_id != worker_id);
    }

    /// Periodic maintenance sweep: removes expired prefix stamps, disconnected
    /// worker entries, empty leaf nodes, and expired affinity records.
    pub fn sweep_expired(&mut self, now: Instant, ttl: Duration) {
        for trie in self.tries.values_mut() {
            let _ = trie.sweep_expired(now, ttl, &self.disconnected_workers);
        }
        self.tries.retain(|_, t| t.non_root_len() > 0);
        self.total_prefix_nodes = self.tries.values().map(TenantModelTrie::non_root_len).sum();
        self.disconnected_workers.clear();

        self.session_affinity
            .retain(|_, v| now.saturating_duration_since(v.last_used_at) <= ttl);
        self.response_affinity
            .retain(|_, v| now.saturating_duration_since(v.last_used_at) <= ttl);
        self.telemetry.tracked_prefix_nodes = self.total_prefix_nodes;
    }

    #[must_use]
    pub fn telemetry_snapshot(&self) -> RoutingTelemetrySnapshot {
        let mut snap = self.telemetry.clone();
        snap.tracked_prefix_nodes = self.total_prefix_nodes;
        snap
    }

    #[must_use]
    pub fn worker_metrics_snapshot(
        &self,
        worker_id: &str,
        model: &str,
    ) -> Option<WorkerModelMetricsSnapshot> {
        self.worker_metrics
            .get(&WorkerModelKey {
                worker_id: worker_id.to_string(),
                model: model.to_string(),
            })
            .map(WorkerModelMetrics::snapshot)
    }

    #[must_use]
    pub fn last_decision(&self) -> Option<&RouteDecision> {
        self.last_decision.as_ref()
    }

    fn worker_is_penalized(&self, worker_id: &str, model: &str, now: Instant) -> bool {
        self.worker_metrics
            .get(&WorkerModelKey {
                worker_id: worker_id.to_string(),
                model: model.to_string(),
            })
            .is_some_and(|m| m.is_penalized(now))
    }

    fn worker_ewma_ttft(&self, worker_id: &str, model: &str) -> Option<f64> {
        self.worker_metrics
            .get(&WorkerModelKey {
                worker_id: worker_id.to_string(),
                model: model.to_string(),
            })
            .and_then(|m| m.ewma_ttft_ms)
    }

    fn worker_last_dispatched_seq(&self, worker_id: &str, model: &str) -> u64 {
        self.worker_metrics
            .get(&WorkerModelKey {
                worker_id: worker_id.to_string(),
                model: model.to_string(),
            })
            .map_or(0, |m| m.last_dispatched_seq)
    }

    fn compare_ewma_ttft(&self, worker_a: &str, worker_b: &str, model: &str) -> std::cmp::Ordering {
        match (
            self.worker_ewma_ttft(worker_a, model),
            self.worker_ewma_ttft(worker_b, model),
        ) {
            (Some(a), Some(b)) => a.total_cmp(&b),
            _ => std::cmp::Ordering::Equal,
        }
    }
}

/// Apply eviction-epoch confidence decay to a matched byte length.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn apply_eviction_decay(raw_bytes: usize, evictions: u64, max_concurrent: usize) -> usize {
    if evictions == 0 {
        return raw_bytes;
    }
    let decay_per_eviction = 2.0 / (max_concurrent.max(1) as f64);
    let factor = 1.0 / (1.0 + (evictions as f64) * decay_per_eviction);
    ((raw_bytes as f64) * factor).round() as usize
}

#[allow(clippy::cast_precision_loss)]
fn estimate_cost_balanced_ms(
    total_prompt_bytes: usize,
    effective_matched_bytes: usize,
    load_ratio: f64,
    penalized: bool,
) -> f64 {
    let uncached = total_prompt_bytes.saturating_sub(effective_matched_bytes) as f64;
    let prefill_ms = uncached / DEFAULT_PREFILL_BYTES_PER_MS;
    let load_ms = load_ratio * COST_BALANCED_LOAD_PENALTY_MS;
    let error_ms = if penalized { 10_000.0 } else { 0.0 };
    prefill_ms + load_ms + error_ms
}

fn worker_priority_rank(worker: &CandidateWorker, priority_list: &[String]) -> usize {
    priority_list
        .iter()
        .position(|entry| entry == &worker.worker_name || entry == &worker.provider)
        .unwrap_or(priority_list.len())
}

/// Extract the `OpenAI` Responses API `"id"` string from a unary response body
/// (e.g. `{"id":"resp_abc123",...}`) or a streaming SSE chunk (`"response":{"id":"resp_..."}`).
#[must_use]
pub fn extract_responses_api_id(payload: &str) -> Option<String> {
    if !payload.contains("\"id\"") {
        return None;
    }
    // Fast path for unary JSON response bodies.
    if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(payload) {
        if let Some(id) = obj.get("id").and_then(Value::as_str)
            && !id.trim().is_empty()
        {
            return Some(id.trim().to_string());
        }
        if let Some(id) = obj
            .get("response")
            .and_then(|r| r.get("id"))
            .and_then(Value::as_str)
            && !id.trim().is_empty()
        {
            return Some(id.trim().to_string());
        }
    }
    // Streaming SSE lines: look for `data: {...}` carrying `"response":{"id":...}`.
    for line in payload.lines() {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        if data.is_empty() || data == "[DONE]" || !data.contains("\"id\"") {
            continue;
        }
        if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(data) {
            if let Some(id) = obj
                .get("response")
                .and_then(|r| r.get("id"))
                .and_then(Value::as_str)
                && !id.trim().is_empty()
            {
                return Some(id.trim().to_string());
            }
            if let Some(id) = obj.get("id").and_then(Value::as_str)
                && id.starts_with("resp_")
            {
                return Some(id.to_string());
            }
        }
    }
    None
}
