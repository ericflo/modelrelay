//! Pre-lock prompt canonicalization, hybrid boundary+block prefix fingerprinting,
//! and explicit session affinity extraction.
//!
//! All functions in this module are pure and run *before* acquiring the
//! `ProxyServerCore` mutex so JSON parsing and SHA-256 hashing never contend
//! with worker dispatch or WebSocket frame routing.

use axum::http::HeaderMap as AxumHeaderMap;
use modelrelay_protocol::{HeaderMap as ProtocolHeaderMap, ProviderRouting};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Interval in canonical bytes between internal checkpoints within a single
/// large message, system prompt, or tool block (~256 tokens).
pub const INTERNAL_CHECKPOINT_BYTES: usize = 1024;

/// Maximum canonical prompt bytes hashed per request (~256K tokens).
/// Bounds worst-case CPU work even on huge multimodal payloads.
pub const MAX_PREFIX_SCAN_BYTES: usize = 1024 * 1024;

/// Maximum number of checkpoints emitted per request.
pub const MAX_PREFIX_CHECKPOINTS: usize = 1024;

/// Threshold above which a single JSON string (e.g. an inline base64 image)
/// is summarized (head + length + tail) so one large media attachment does not
/// exhaust [`MAX_PREFIX_SCAN_BYTES`] and hide subsequent conversation turns.
const LARGE_STRING_SUMMARIZE_THRESHOLD: usize = 32 * 1024;
const LARGE_STRING_EDGE_BYTES: usize = 4 * 1024;

/// A cumulative prefix hash checkpoint emitted along the canonical prompt stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefixCheckpoint {
    /// First 64 bits of SHA-256 over the canonical prompt stream from byte 0
    /// up to `cumulative_bytes`.
    pub hash: u64,
    /// Cumulative canonical byte offset represented by this checkpoint.
    pub cumulative_bytes: usize,
}

/// Pre-extracted routing metadata and prompt prefix fingerprints for a request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestRoutingHints {
    /// Hashed explicit session/cache affinity key from headers (`x-session-affinity`,
    /// `session_id`, `x-session-id`) or JSON (`prompt_cache_key`).
    pub session_key: Option<u64>,
    /// `OpenAI` Responses API `previous_response_id`, used to look up which worker
    /// produced the preceding response.
    pub previous_response_id: Option<String>,
    /// Ordered cumulative prefix checkpoints from byte 0 upward.
    pub prefix_checkpoints: Vec<PrefixCheckpoint>,
    /// Total canonical prompt bytes observed (capped at [`MAX_PREFIX_SCAN_BYTES`]
    /// for hashing, though logical length of summarized media is included).
    pub total_prompt_bytes: usize,
}

impl RequestRoutingHints {
    /// Fast token-count estimate (`bytes / 4`, rounding up for non-zero inputs)
    /// used for context-window guards and cost estimation without loading a tokenizer.
    #[must_use]
    pub fn estimated_prompt_tokens(&self) -> usize {
        if self.total_prompt_bytes == 0 {
            0
        } else {
            self.total_prompt_bytes.div_ceil(4)
        }
    }

    /// Returns true if this request carries neither prefix checkpoints nor
    /// explicit session/response affinity hints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.prefix_checkpoints.is_empty()
            && self.session_key.is_none()
            && self.previous_response_id.is_none()
    }
}

/// Combined single-pass parse result for HTTP proxy requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedProxyRequest {
    pub model: Option<String>,
    pub is_streaming: bool,
    pub provider_routing: Option<ProviderRouting>,
    pub routing_hints: RequestRoutingHints,
}

/// Resiliently parse `model`, `stream`, `provider`, and smart-routing hints from
/// a raw HTTP request in a single JSON pass (outside the `ProxyServerCore` lock).
#[must_use]
pub fn parse_proxy_request(
    method: &str,
    endpoint_path: &str,
    headers: &AxumHeaderMap,
    body: &str,
) -> ParsedProxyRequest {
    let header_session_key = extract_session_key_from_axum_headers(headers);
    parse_proxy_request_inner(method, endpoint_path, header_session_key, body)
}

/// Extract routing hints from a protocol [`ProtocolHeaderMap`] and JSON body
/// (used when callers invoke `ProxyServerCore::submit_transport_request*` directly
/// without pre-computing hints in the HTTP layer).
#[must_use]
pub fn extract_routing_hints_from_transport(
    method: &str,
    endpoint_path: &str,
    headers: &ProtocolHeaderMap,
    body: &str,
) -> RequestRoutingHints {
    let header_session_key = extract_session_key_from_protocol_headers(headers);
    parse_proxy_request_inner(method, endpoint_path, header_session_key, body).routing_hints
}

fn parse_proxy_request_inner(
    method: &str,
    endpoint_path: &str,
    header_session_key: Option<u64>,
    body: &str,
) -> ParsedProxyRequest {
    let mut parsed = ParsedProxyRequest {
        routing_hints: RequestRoutingHints {
            session_key: header_session_key,
            ..Default::default()
        },
        ..Default::default()
    };

    if body.is_empty() {
        return parsed;
    }

    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) else {
        return parsed;
    };

    parsed.model = map
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    parsed.is_streaming = map.get("stream").and_then(Value::as_bool).unwrap_or(false);

    parsed.provider_routing = match map.get("provider") {
        Some(Value::Object(_)) => map
            .get("provider")
            .cloned()
            .and_then(|value| serde_json::from_value::<ProviderRouting>(value).ok()),
        Some(Value::String(name)) if !name.trim().is_empty() => Some(ProviderRouting {
            only: Some(vec![name.trim().to_string()]),
            ..Default::default()
        }),
        _ => None,
    };

    // Skip prefix extraction on GET probes and stateless embedding/rerank/audio endpoints.
    if !method.eq_ignore_ascii_case("POST") || is_stateless_endpoint(endpoint_path) {
        return parsed;
    }

    // Explicit JSON session/cache hints (body fields fill in when headers absent).
    if parsed.routing_hints.session_key.is_none()
        && let Some(cache_key) = map
            .get("prompt_cache_key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    {
        parsed.routing_hints.session_key = Some(hash_session_key("prompt_cache_key", cache_key));
    }

    if let Some(prev_id) = map
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        parsed.routing_hints.previous_response_id = Some(prev_id.to_string());
    }

    let (checkpoints, total_bytes) = build_prefix_checkpoints(&map);
    parsed.routing_hints.prefix_checkpoints = checkpoints;
    parsed.routing_hints.total_prompt_bytes = total_bytes;

    parsed
}

fn is_stateless_endpoint(endpoint_path: &str) -> bool {
    let path = endpoint_path
        .split_once('?')
        .map_or(endpoint_path, |(p, _)| p);
    path.contains("/embeddings")
        || path.contains("/rerank")
        || path.contains("/audio/")
        || path.contains("/images/")
        || path.contains("/moderations")
        || matches!(
            path,
            "/props" | "/models" | "/v1/models" | "/api/v1/models" | "/api/v0/models" | "/api/show"
        )
}

fn extract_session_key_from_axum_headers(headers: &AxumHeaderMap) -> Option<u64> {
    for name in ["x-session-affinity", "session_id", "x-session-id"] {
        if let Some(raw) = headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(hash_session_key("header", raw));
        }
    }
    None
}

fn extract_session_key_from_protocol_headers(headers: &ProtocolHeaderMap) -> Option<u64> {
    for name in ["x-session-affinity", "session_id", "x-session-id"] {
        if let Some(raw) = headers
            .get(name)
            .map(String::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(hash_session_key("header", raw));
        }
    }
    None
}

#[must_use]
pub fn hash_session_key(namespace: &str, value: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"mr-session-v1\x1f");
    hasher.update(namespace.as_bytes());
    hasher.update(b"\x1f");
    hasher.update(value.as_bytes());
    truncate_u64(&hasher.finalize())
}

fn truncate_u64(digest: &[u8]) -> u64 {
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes)
}

/// Streaming canonical prompt hasher that emits cumulative checkpoints both at
/// logical boundaries (tools, system, each message) and every
/// [`INTERNAL_CHECKPOINT_BYTES`] inside long units.
struct PrefixStreamer {
    hasher: Sha256,
    /// Bytes fed into the SHA-256 state (capped at `MAX_PREFIX_SCAN_BYTES`).
    hashed_bytes: usize,
    /// Logical prompt bytes observed (including full length of summarized blobs).
    logical_bytes: usize,
    /// Value of `hashed_bytes` at the previous emitted checkpoint.
    last_checkpoint_hashed_bytes: usize,
    /// Logical bytes at the previous emitted checkpoint.
    last_checkpoint_logical_bytes: usize,
    checkpoints: Vec<PrefixCheckpoint>,
}

impl PrefixStreamer {
    fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"mr-prefix-v1\x1d");
        Self {
            hasher,
            hashed_bytes: 0,
            logical_bytes: 0,
            last_checkpoint_hashed_bytes: 0,
            last_checkpoint_logical_bytes: 0,
            checkpoints: Vec::new(),
        }
    }

    fn budget_exhausted(&self) -> bool {
        self.hashed_bytes >= MAX_PREFIX_SCAN_BYTES
            || self.checkpoints.len() >= MAX_PREFIX_CHECKPOINTS
    }

    /// Feed structural/framing tag bytes (do not count toward logical prompt length).
    fn feed_tag(&mut self, tag: &[u8]) {
        if !self.budget_exhausted() {
            self.hasher.update(tag);
        }
    }

    /// Feed prompt content bytes, automatically emitting internal checkpoints
    /// every [`INTERNAL_CHECKPOINT_BYTES`].
    fn feed_content(&mut self, mut bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.logical_bytes = self.logical_bytes.saturating_add(bytes.len());

        while !bytes.is_empty() && !self.budget_exhausted() {
            let remaining_budget = MAX_PREFIX_SCAN_BYTES.saturating_sub(self.hashed_bytes);
            if remaining_budget == 0 {
                break;
            }
            let bytes_since_cp = self
                .hashed_bytes
                .saturating_sub(self.last_checkpoint_hashed_bytes);
            let until_next_cp = INTERNAL_CHECKPOINT_BYTES.saturating_sub(bytes_since_cp);
            let take = bytes.len().min(remaining_budget).min(until_next_cp.max(1));

            self.hasher.update(&bytes[..take]);
            self.hashed_bytes += take;
            bytes = &bytes[take..];

            if self
                .hashed_bytes
                .saturating_sub(self.last_checkpoint_hashed_bytes)
                >= INTERNAL_CHECKPOINT_BYTES
            {
                self.emit_checkpoint();
            }
        }
    }

    /// Feed a string value, summarizing very large payloads (such as inline
    /// base64 images) so later text messages remain fingerprinted.
    fn feed_string_value(&mut self, s: &str) {
        let bytes = s.as_bytes();
        if bytes.len() > LARGE_STRING_SUMMARIZE_THRESHOLD {
            self.feed_tag(b"blob:");
            self.feed_tag(&bytes.len().to_le_bytes());
            self.feed_content(&bytes[..LARGE_STRING_EDGE_BYTES]);
            self.feed_tag(b"..");
            self.feed_content(&bytes[bytes.len() - LARGE_STRING_EDGE_BYTES..]);
            // Count the skipped middle bytes toward logical size (capped for media)
            // without burning SHA-256 scan budget.
            let skipped = bytes.len().saturating_sub(LARGE_STRING_EDGE_BYTES * 2);
            self.logical_bytes = self.logical_bytes.saturating_add(skipped.min(8 * 1024));
        } else {
            self.feed_content(bytes);
        }
    }

    /// Emit a checkpoint at a logical boundary if new content has arrived since
    /// the last checkpoint.
    fn finish_boundary(&mut self) {
        if self.checkpoints.len() >= MAX_PREFIX_CHECKPOINTS {
            return;
        }
        if self.logical_bytes > self.last_checkpoint_logical_bytes {
            self.emit_checkpoint();
        }
    }

    fn emit_checkpoint(&mut self) {
        if self.checkpoints.len() >= MAX_PREFIX_CHECKPOINTS {
            return;
        }
        let digest = self.hasher.clone().finalize();
        self.checkpoints.push(PrefixCheckpoint {
            hash: truncate_u64(&digest),
            cumulative_bytes: self.logical_bytes,
        });
        self.last_checkpoint_hashed_bytes = self.hashed_bytes;
        self.last_checkpoint_logical_bytes = self.logical_bytes;
    }

    /// Feed any JSON value canonically (keys are already sorted in `serde_json::Map`).
    /// Strips transient `cache_control` keys (Anthropic moving breakpoints) so
    /// moving a breakpoint forward between turns never invalidates earlier hashes.
    fn feed_json(&mut self, value: &Value) {
        match value {
            Value::Null => self.feed_tag(b"N"),
            Value::Bool(true) => self.feed_tag(b"T"),
            Value::Bool(false) => self.feed_tag(b"F"),
            Value::Number(num) => {
                self.feed_tag(b"D");
                self.feed_content(num.to_string().as_bytes());
                self.feed_tag(b";");
            }
            Value::String(s) => {
                self.feed_tag(b"S");
                self.feed_string_value(s);
                self.feed_tag(b"\x1f");
            }
            Value::Array(items) => {
                self.feed_tag(b"[");
                for item in items {
                    self.feed_json(item);
                    self.feed_tag(b",");
                }
                self.feed_tag(b"]");
            }
            Value::Object(obj) => {
                self.feed_tag(b"{");
                for (k, v) in obj {
                    if k == "cache_control" {
                        continue;
                    }
                    self.feed_tag(b"K");
                    self.feed_content(k.as_bytes());
                    self.feed_tag(b"=");
                    self.feed_json(v);
                    self.feed_tag(b";");
                }
                self.feed_tag(b"}");
            }
        }
    }
}

/// Extract ordered canonical prefix checkpoints from an `OpenAI` Chat, Anthropic
/// Messages, `OpenAI` Responses, or legacy Completions JSON request object.
fn build_prefix_checkpoints(
    map: &serde_json::Map<String, Value>,
) -> (Vec<PrefixCheckpoint>, usize) {
    let has_prompt_surface = map.contains_key("messages")
        || map.contains_key("input")
        || map.contains_key("prompt")
        || map.contains_key("system")
        || map.contains_key("instructions");

    if !has_prompt_surface {
        return (Vec::new(), 0);
    }

    let mut streamer = PrefixStreamer::new();

    // 1. Template-affecting reasoning/thinking parameters (toggling these alters
    //    the rendered chat template from token 0).
    let mut has_template_knobs = false;
    for knob in [
        "reasoning_effort",
        "reasoning",
        "thinking",
        "enable_thinking",
        "chat_template_kwargs",
    ] {
        if let Some(val) = map.get(knob)
            && !val.is_null()
        {
            if !has_template_knobs {
                streamer.feed_tag(b"\x1dknobs:");
                has_template_knobs = true;
            }
            streamer.feed_tag(knob.as_bytes());
            streamer.feed_tag(b"=");
            streamer.feed_json(val);
            streamer.feed_tag(b";");
        }
    }

    // 2. Tool / function schemas (injected into the prompt prefix by chat templates).
    for tool_key in ["tools", "functions"] {
        if let Some(tools) = map.get(tool_key)
            && !tools.is_null()
        {
            streamer.feed_tag(b"\x1dtools:");
            streamer.feed_json(tools);
            streamer.finish_boundary();
        }
    }

    // 3. Top-level system / instructions (Anthropic `system`, Responses `instructions`).
    if let Some(system) = map.get("system")
        && !system.is_null()
    {
        streamer.feed_tag(b"\x1dsystem:");
        streamer.feed_json(system);
        streamer.finish_boundary();
    }
    if let Some(instructions) = map.get("instructions")
        && !instructions.is_null()
    {
        streamer.feed_tag(b"\x1dinstructions:");
        streamer.feed_json(instructions);
        streamer.finish_boundary();
    }

    // 4. Messages (OpenAI Chat Completions / Anthropic Messages).
    if let Some(Value::Array(messages)) = map.get("messages") {
        for message in messages {
            streamer.feed_tag(b"\x1dmsg:");
            streamer.feed_json(message);
            streamer.finish_boundary();
        }
    }

    // 5. Responses API `input` (string or item array).
    if let Some(input) = map.get("input")
        && !input.is_null()
    {
        match input {
            Value::Array(items) => {
                for item in items {
                    streamer.feed_tag(b"\x1dinput_item:");
                    streamer.feed_json(item);
                    streamer.finish_boundary();
                }
            }
            other => {
                streamer.feed_tag(b"\x1dinput:");
                streamer.feed_json(other);
                streamer.finish_boundary();
            }
        }
    }

    // 6. Legacy Completions `prompt`.
    if let Some(prompt) = map.get("prompt")
        && !prompt.is_null()
    {
        streamer.feed_tag(b"\x1dprompt:");
        streamer.feed_json(prompt);
        streamer.finish_boundary();
    }

    let total_bytes = streamer.logical_bytes;
    (streamer.checkpoints, total_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn multi_turn_chat_shares_exact_prefix_checkpoints() {
        let sys =
            "You are an expert Rust coding assistant with deep systems knowledge. ".repeat(15);
        let turn1 = serde_json::json!({
            "model": "qwen3-coder-30b",
            "temperature": 0.2,
            "stream": true,
            "tools": [{"type": "function", "function": {"name": "read_file", "parameters": {"type": "object"}}}],
            "messages": [
                {"role": "system", "content": sys},
                {"role": "user", "content": "Inspect lib.rs and find the dispatcher."}
            ]
        })
        .to_string();

        // Turn 2 changes sampling params (temperature, max_tokens) and appends assistant + user turns.
        let turn2 = serde_json::json!({
            "model": "qwen3-coder-30b",
            "temperature": 0.7,
            "max_tokens": 4096,
            "stream": false,
            "tools": [{"type": "function", "function": {"name": "read_file", "parameters": {"type": "object"}}}],
            "messages": [
                {"role": "system", "content": sys},
                {"role": "user", "content": "Inspect lib.rs and find the dispatcher."},
                {"role": "assistant", "content": "The dispatcher lives in pick_from_candidates."},
                {"role": "user", "content": "Now explain how prefix caching interacts with it."}
            ]
        })
        .to_string();

        let headers = AxumHeaderMap::new();
        let p1 = parse_proxy_request("POST", "/v1/chat/completions", &headers, &turn1);
        let p2 = parse_proxy_request("POST", "/v1/chat/completions", &headers, &turn2);

        assert!(!p1.routing_hints.prefix_checkpoints.is_empty());
        assert!(
            p2.routing_hints.prefix_checkpoints.len() > p1.routing_hints.prefix_checkpoints.len()
        );
        // Every single checkpoint from Turn 1 must appear identically at the front of Turn 2.
        assert_eq!(
            &p2.routing_hints.prefix_checkpoints[..p1.routing_hints.prefix_checkpoints.len()],
            p1.routing_hints.prefix_checkpoints.as_slice()
        );
    }

    #[test]
    fn anthropic_moving_cache_control_preserves_prefix_match() {
        let shared_sys = "Large system prompt for coding agent. ".repeat(30);
        let turn1 = serde_json::json!({
            "model": "claude-sonnet",
            "system": [{"type": "text", "text": shared_sys, "cache_control": {"type": "ephemeral"}}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Step 1 prompt", "cache_control": {"type": "ephemeral"}}]}
            ]
        })
        .to_string();

        // Turn 2 moves cache_control off message 0 onto message 2!
        let turn2 = serde_json::json!({
            "model": "claude-sonnet",
            "system": [{"type": "text", "text": shared_sys, "cache_control": {"type": "ephemeral"}}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Step 1 prompt"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Step 1 reply"}]},
                {"role": "user", "content": [{"type": "text", "text": "Step 2 prompt", "cache_control": {"type": "ephemeral"}}]}
            ]
        })
        .to_string();

        let headers = AxumHeaderMap::new();
        let p1 = parse_proxy_request("POST", "/v1/messages", &headers, &turn1);
        let p2 = parse_proxy_request("POST", "/v1/messages", &headers, &turn2);

        assert!(!p1.routing_hints.prefix_checkpoints.is_empty());
        assert_eq!(
            &p2.routing_hints.prefix_checkpoints[..p1.routing_hints.prefix_checkpoints.len()],
            p1.routing_hints.prefix_checkpoints.as_slice()
        );
    }

    #[test]
    fn single_message_long_document_shares_internal_block_checkpoints() {
        let doc = "The quick brown fox jumps over the lazy dog in a massive codebase. ".repeat(100);
        let req1 = serde_json::json!({
            "model": "qwen3",
            "messages": [{"role": "user", "content": format!("{doc}\nQuestion A: summarize?")}]
        })
        .to_string();
        let req2 = serde_json::json!({
            "model": "qwen3",
            "messages": [{"role": "user", "content": format!("{doc}\nQuestion B: list bugs?")}]
        })
        .to_string();

        let headers = AxumHeaderMap::new();
        let p1 = parse_proxy_request("POST", "/v1/chat/completions", &headers, &req1);
        let p2 = parse_proxy_request("POST", "/v1/chat/completions", &headers, &req2);

        let shared_count = p1
            .routing_hints
            .prefix_checkpoints
            .iter()
            .zip(p2.routing_hints.prefix_checkpoints.iter())
            .take_while(|(a, b)| a == b)
            .count();

        assert!(
            shared_count >= 5,
            "expected at least 5 shared 1KiB checkpoints across long document, got {shared_count}"
        );
        let matched_bytes = p1.routing_hints.prefix_checkpoints[shared_count - 1].cumulative_bytes;
        assert!(
            matched_bytes >= 5_000,
            "expected >= 5000 matched bytes, got {matched_bytes}"
        );
    }

    #[test]
    fn extracts_session_headers_and_responses_previous_id() {
        let mut headers = AxumHeaderMap::new();
        headers.insert("x-session-affinity", HeaderValue::from_static("sess-abc"));

        let body = serde_json::json!({
            "model": "gpt-oss-120b",
            "previous_response_id": "resp_12345",
            "input": "Continue from last turn"
        })
        .to_string();

        let parsed = parse_proxy_request("POST", "/v1/responses", &headers, &body);
        assert_eq!(parsed.model.as_deref(), Some("gpt-oss-120b"));
        assert!(parsed.routing_hints.session_key.is_some());
        assert_eq!(
            parsed.routing_hints.previous_response_id.as_deref(),
            Some("resp_12345")
        );
        assert!(!parsed.routing_hints.prefix_checkpoints.is_empty());
    }

    #[test]
    fn skips_prefix_hashing_for_embeddings_and_get_probes() {
        let headers = AxumHeaderMap::new();
        let body = serde_json::json!({
            "model": "nomic-embed-text",
            "input": "hello world".repeat(100)
        })
        .to_string();

        let embed = parse_proxy_request("POST", "/v1/embeddings", &headers, &body);
        assert!(embed.routing_hints.prefix_checkpoints.is_empty());

        let probe = parse_proxy_request("GET", "/props?model=qwen3", &headers, &body);
        assert!(probe.routing_hints.prefix_checkpoints.is_empty());
    }

    #[test]
    fn reasoning_toggle_separates_prefix_hashes() {
        let body_off = serde_json::json!({
            "model": "qwen3",
            "chat_template_kwargs": {"enable_thinking": false},
            "messages": [{"role": "user", "content": "A long shared prompt. ".repeat(50)}]
        })
        .to_string();
        let body_on = serde_json::json!({
            "model": "qwen3",
            "chat_template_kwargs": {"enable_thinking": true},
            "messages": [{"role": "user", "content": "A long shared prompt. ".repeat(50)}]
        })
        .to_string();

        let headers = AxumHeaderMap::new();
        let p_off = parse_proxy_request("POST", "/v1/chat/completions", &headers, &body_off);
        let p_on = parse_proxy_request("POST", "/v1/chat/completions", &headers, &body_on);

        assert_ne!(
            p_off.routing_hints.prefix_checkpoints[0],
            p_on.routing_hints.prefix_checkpoints[0]
        );
    }
}
