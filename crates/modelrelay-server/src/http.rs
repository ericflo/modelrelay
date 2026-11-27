use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{
        HeaderMap as AxumHeaderMap, HeaderName, HeaderValue, Method, StatusCode,
        header::{
            ACCEPT_ENCODING, AUTHORIZATION, CACHE_CONTROL, CONNECTION, CONTENT_LENGTH,
            CONTENT_TYPE, HOST, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER,
            TRANSFER_ENCODING, UPGRADE,
        },
    },
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use futures_util::{future, stream};
use modelrelay_protocol::admin_api::{AdminKeysResponse, CreateKeyRequest, CreateKeyResponse};
use modelrelay_protocol::{
    HeaderMap, ProviderRouting, ResponseCompleteMessage, parse_openai_models_response,
};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

use crate::{
    AdminWorkerInfo, CancelReason, HttpResponseEvent, PendingHttpResponse,
    PendingStreamingHttpResponse, ProxyServerCore, RequestFailureReason, WorkerSocketApp,
    api_keys::ApiKeyStore,
};

const OPENAI_MODELS_PROVIDER: &str = "openai";
const LEGACY_MODELS_PROBE_TTL: Duration = Duration::from_secs(30);
const LEGACY_MODELS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const METADATA_PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024 * 1024; // 64 MiB (supports vision/multimodal payloads)
const MAX_STREAM_RESPONSE_BYTES: usize = 10 * 1024 * 1024 * 1024; // 10 GB
const OVERSIZED_STREAM_ERROR_SSE: &str = "event: error\ndata: {\"error\":{\"type\":\"stream_error\",\"message\":\"stream exceeded size limit\"}}\n\n";
const STREAM_TIMEOUT_ERROR_SSE: &str = "event: error\ndata: {\"error\":{\"type\":\"timeout\",\"message\":\"stream timed out waiting for worker\"}}\n\n";
const STREAM_DISCONNECTED_ERROR_SSE: &str = "event: error\ndata: {\"error\":{\"type\":\"stream_error\",\"message\":\"worker disconnected mid-stream\"}}\n\n";

#[derive(Clone)]
pub struct ProxyHttpApp {
    core: Arc<Mutex<ProxyServerCore>>,
    models_provider: String,
    provider_enabled: bool,
    worker_socket_app: Option<WorkerSocketApp>,
    admin_token: Option<String>,
    require_api_keys: bool,
    api_key_store: Arc<dyn ApiKeyStore>,
    request_timeout: Option<Duration>,
    max_body_bytes: usize,
    public_cors: Option<CorsLayer>,
    admin_cors: Option<CorsLayer>,
    legacy_models_probe: bool,
}

impl ProxyHttpApp {
    #[must_use]
    pub fn new(core: Arc<Mutex<ProxyServerCore>>) -> Self {
        Self {
            core,
            models_provider: OPENAI_MODELS_PROVIDER.to_string(),
            provider_enabled: true,
            worker_socket_app: None,
            admin_token: None,
            require_api_keys: false,
            api_key_store: Arc::new(crate::api_keys::InMemoryApiKeyStore::new()),
            request_timeout: None,
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            public_cors: None,
            admin_cors: None,
            legacy_models_probe: false,
        }
    }

    #[must_use]
    pub fn with_admin_token(mut self, token: Option<String>) -> Self {
        self.admin_token = token;
        self
    }

    #[must_use]
    pub fn with_models_provider(mut self, provider: impl Into<String>) -> Self {
        self.models_provider = provider.into();
        self
    }

    #[must_use]
    pub fn with_worker_socket_app(mut self, worker_socket_app: WorkerSocketApp) -> Self {
        self.worker_socket_app = Some(worker_socket_app);
        self
    }

    #[must_use]
    pub fn with_provider_enabled(mut self, enabled: bool) -> Self {
        self.provider_enabled = enabled;
        self
    }

    #[must_use]
    pub fn with_require_api_keys(mut self, require: bool) -> Self {
        self.require_api_keys = require;
        self
    }

    #[must_use]
    pub fn with_api_key_store(mut self, store: Arc<dyn ApiKeyStore>) -> Self {
        self.api_key_store = store;
        self
    }

    /// Set the request timeout for unary responses and per-chunk idle timeout for
    /// streaming responses. Applied inside the HTTP handlers rather than an outer
    /// middleware so WebSocket upgrades and healthy long-running streams are never
    /// prematurely killed.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Override the maximum accepted HTTP request body size (default: 64 MiB).
    #[must_use]
    pub fn with_max_body_bytes(mut self, max_bytes: usize) -> Self {
        self.max_body_bytes = max_bytes;
        self
    }

    /// Configure CORS for public inference & health routes (`/v1/*`, `/health`, fallback).
    #[must_use]
    pub fn with_public_cors(mut self, cors: Option<CorsLayer>) -> Self {
        self.public_cors = cors;
        self
    }

    /// Configure CORS specifically for `/admin/*` routes. When `None`, `/admin/*`
    /// does not emit permissive cross-origin headers.
    #[must_use]
    pub fn with_admin_cors(mut self, cors: Option<CorsLayer>) -> Self {
        self.admin_cors = cors;
        self
    }

    /// Enable lightweight cached `GET /v1/models` probing against connected legacy
    /// workers that registered without native `model_metadata`, so their backend's
    /// context window and reasoning effort fields pass through `/v1/models` without
    /// requiring an immediate worker upgrade.
    #[must_use]
    pub fn with_legacy_models_probe(mut self, enabled: bool) -> Self {
        self.legacy_models_probe = enabled;
        self
    }

    pub fn router(self) -> Router {
        let state = HttpState {
            core: self.core,
            models_provider: self.models_provider,
            provider_enabled: self.provider_enabled,
            started_at: Instant::now(),
            admin_token: self.admin_token,
            require_api_keys: self.require_api_keys,
            api_key_store: self.api_key_store,
            request_timeout: self.request_timeout,
            legacy_models_probe: self.legacy_models_probe,
        };

        let mut admin_router = Router::new()
            .route("/admin/workers", get(admin_workers_handler))
            .route("/admin/stats", get(admin_stats_handler))
            .route(
                "/admin/keys",
                get(admin_keys_handler).post(admin_keys_create_handler),
            )
            .route("/admin/keys/{id}", delete(admin_keys_revoke_handler))
            .with_state(state.clone());

        if let Some(admin_cors) = self.admin_cors {
            admin_router = admin_router.layer(admin_cors);
        }

        let mut public_router = Router::new()
            .route("/health", get(health_handler))
            .route("/v1/models", get(models_handler))
            .route("/models", get(models_handler))
            .fallback(wildcard_handler)
            .layer(DefaultBodyLimit::max(self.max_body_bytes))
            .with_state(state);

        if let Some(public_cors) = self.public_cors {
            public_router = public_router.layer(public_cors);
        }

        let router = public_router.merge(admin_router);

        match self.worker_socket_app {
            Some(worker_socket_app) => router.merge(worker_socket_app.router()),
            None => router,
        }
    }
}

#[derive(Clone)]
struct HttpState {
    core: Arc<Mutex<ProxyServerCore>>,
    models_provider: String,
    provider_enabled: bool,
    started_at: Instant,
    admin_token: Option<String>,
    require_api_keys: bool,
    api_key_store: Arc<dyn ApiKeyStore>,
    request_timeout: Option<Duration>,
    legacy_models_probe: bool,
}

#[derive(Debug, Serialize, PartialEq)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    workers_connected: usize,
    queue_depth: usize,
    uptime_secs: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    workers: Vec<WorkerHealthInfo>,
    models_provider: String,
}

#[derive(Debug, Serialize, PartialEq)]
struct WorkerHealthInfo {
    id: String,
    provider: String,
    models: Vec<String>,
    in_flight: usize,
    max_concurrent: usize,
    is_draining: bool,
}

#[derive(Debug, Serialize, PartialEq)]
struct ModelsResponse {
    object: &'static str,
    data: Vec<serde_json::Map<String, Value>>,
}

async fn health_handler(State(state): State<HttpState>) -> Json<HealthResponse> {
    let (workers_connected, queue_depth, workers) = {
        let core = state.core.lock().await;
        // In multi-tenant hosted mode (`require_api_keys = true`), avoid leaking
        // per-worker provider/model topology on the unauthenticated /health endpoint.
        let worker_infos: Vec<WorkerHealthInfo> = if state.require_api_keys {
            Vec::new()
        } else {
            core.worker_debug_info()
                .into_iter()
                .map(
                    |(id, provider, models, in_flight, max_concurrent, is_draining)| {
                        WorkerHealthInfo {
                            id,
                            provider,
                            models,
                            in_flight,
                            max_concurrent,
                            is_draining,
                        }
                    },
                )
                .collect()
        };
        (
            core.connected_worker_count(),
            core.total_queue_depth(),
            worker_infos,
        )
    };

    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        workers_connected,
        queue_depth,
        uptime_secs: state.started_at.elapsed().as_secs_f64(),
        workers,
        models_provider: state.models_provider.clone(),
    })
}

async fn models_handler(State(state): State<HttpState>, headers: AxumHeaderMap) -> Response {
    let owner_key_id = match check_client_api_key(&state, &headers).await {
        Ok(owner_key_id) => owner_key_id,
        Err(response) => return response,
    };

    if state.legacy_models_probe {
        probe_legacy_workers_metadata(&state, owner_key_id.as_deref()).await;
    }

    let data = {
        let core = state.core.lock().await;
        core.provider_model_entries_for_owner(&state.models_provider, owner_key_id.as_deref())
    };

    Json(ModelsResponse {
        object: "list",
        data,
    })
    .into_response()
}

enum LegacyProbeOutcome {
    Metadata(modelrelay_protocol::ModelMetadataMap),
    /// The worker replied cleanly, but this path returned non-2xx or unparseable JSON
    /// (so falling back to an alternate path such as `/models` is worthwhile).
    TryFallback,
    /// Timeout or transport failure — do not burn another timeout on fallback paths.
    Stop,
}

async fn run_single_legacy_models_probe(
    core_ref: &Arc<Mutex<ProxyServerCore>>,
    worker_id: &str,
    endpoint_path: &str,
) -> LegacyProbeOutcome {
    let pending = {
        let mut core = core_ref.lock().await;
        core.submit_worker_models_probe_path(worker_id, endpoint_path)
    };
    let Some(pending) = pending else {
        return LegacyProbeOutcome::TryFallback;
    };

    let request_id = pending.request_id.clone();
    let mut guard = HttpRequestCancellationGuard::new(Arc::clone(core_ref), &pending);
    let result = tokio::time::timeout(LEGACY_MODELS_PROBE_TIMEOUT, pending.response_rx).await;
    match result {
        Ok(Ok(Ok(response))) => {
            guard.disarm();
            if (200..300).contains(&response.status_code) {
                if let Some(body) = response.body.as_deref()
                    && let Ok(parsed) = serde_json::from_str::<Value>(body)
                    && let Some((_, metadata)) = parse_openai_models_response(&parsed)
                    && !metadata.is_empty()
                {
                    return LegacyProbeOutcome::Metadata(metadata);
                }
                tracing::warn!(
                    worker_id = %worker_id,
                    endpoint_path = %endpoint_path,
                    status = response.status_code,
                    "legacy worker models probe returned empty or unparseable body"
                );
                LegacyProbeOutcome::TryFallback
            } else {
                tracing::warn!(
                    worker_id = %worker_id,
                    endpoint_path = %endpoint_path,
                    status = response.status_code,
                    "legacy worker models probe returned non-2xx status"
                );
                LegacyProbeOutcome::TryFallback
            }
        }
        Ok(Ok(Err(reason))) => {
            guard.disarm();
            tracing::warn!(
                worker_id = %worker_id,
                endpoint_path = %endpoint_path,
                ?reason,
                "legacy worker models probe failed"
            );
            LegacyProbeOutcome::Stop
        }
        Ok(Err(_)) => {
            guard.disarm();
            LegacyProbeOutcome::Stop
        }
        Err(_) => {
            guard.disarm();
            tracing::warn!(
                worker_id = %worker_id,
                endpoint_path = %endpoint_path,
                request_id = %request_id,
                "legacy worker models probe timed out"
            );
            let mut core = core_ref.lock().await;
            let _ = core.cancel_request(&request_id, CancelReason::RequestTimedOut);
            LegacyProbeOutcome::Stop
        }
    }
}

async fn probe_legacy_workers_metadata(state: &HttpState, owner_key_id: Option<&str>) {
    let targets = {
        let core = state.core.lock().await;
        core.legacy_metadata_probe_targets(
            &state.models_provider,
            owner_key_id,
            Instant::now(),
            LEGACY_MODELS_PROBE_TTL,
        )
    };

    if targets.is_empty() {
        return;
    }

    let core_ref = &state.core;
    let probe_futs = targets.into_iter().map(|worker_id| async move {
        match run_single_legacy_models_probe(core_ref, &worker_id, "/v1/models").await {
            LegacyProbeOutcome::Metadata(metadata) => Some((worker_id, metadata)),
            LegacyProbeOutcome::TryFallback => {
                match run_single_legacy_models_probe(core_ref, &worker_id, "/models").await {
                    LegacyProbeOutcome::Metadata(metadata) => Some((worker_id, metadata)),
                    LegacyProbeOutcome::TryFallback | LegacyProbeOutcome::Stop => None,
                }
            }
            LegacyProbeOutcome::Stop => None,
        }
    });

    let results = future::join_all(probe_futs).await;
    let updates: Vec<_> = results.into_iter().flatten().collect();
    if !updates.is_empty() {
        let mut core = state.core.lock().await;
        for (worker_id, metadata) in updates {
            core.apply_probed_worker_metadata(&worker_id, metadata);
        }
    }
}

fn check_admin_auth(state: &HttpState, headers: &AxumHeaderMap) -> Result<(), StatusCode> {
    let Some(expected) = &state.admin_token else {
        return Err(StatusCode::FORBIDDEN);
    };

    let Some(auth_header) = headers.get("authorization") else {
        return Err(StatusCode::FORBIDDEN);
    };

    let Ok(auth_str) = auth_header.to_str() else {
        return Err(StatusCode::FORBIDDEN);
    };

    let token = auth_str.strip_prefix("Bearer ").unwrap_or(auth_str);
    if subtle::ConstantTimeEq::ct_eq(token.as_bytes(), expected.as_bytes()).into() {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

#[derive(Serialize)]
struct AdminWorkersResponse {
    workers: Vec<AdminWorkerInfo>,
}

async fn admin_workers_handler(State(state): State<HttpState>, headers: AxumHeaderMap) -> Response {
    if let Err(status) = check_admin_auth(&state, &headers) {
        return status.into_response();
    }

    let workers = state.core.lock().await.admin_workers_snapshot();
    Json(AdminWorkersResponse { workers }).into_response()
}

#[derive(Serialize)]
struct AdminStatsResponse {
    queue_depth: std::collections::HashMap<String, usize>,
    active_workers: usize,
}

async fn admin_stats_handler(State(state): State<HttpState>, headers: AxumHeaderMap) -> Response {
    if let Err(status) = check_admin_auth(&state, &headers) {
        return status.into_response();
    }

    let core = state.core.lock().await;
    let queue_depth = core.admin_queue_depth();
    let active_workers = core.connected_worker_count();
    Json(AdminStatsResponse {
        queue_depth,
        active_workers,
    })
    .into_response()
}

async fn admin_keys_handler(State(state): State<HttpState>, headers: AxumHeaderMap) -> Response {
    if let Err(status) = check_admin_auth(&state, &headers) {
        return status.into_response();
    }

    match state.api_key_store.list_keys().await {
        Ok(keys) => Json(AdminKeysResponse { keys }).into_response(),
        Err(e) => {
            tracing::error!("list_keys failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn admin_keys_create_handler(
    State(state): State<HttpState>,
    headers: AxumHeaderMap,
    Json(body): Json<CreateKeyRequest>,
) -> Response {
    if let Err(status) = check_admin_auth(&state, &headers) {
        return status.into_response();
    }

    match state.api_key_store.create_key(body.name).await {
        Ok((metadata, secret)) => (
            StatusCode::CREATED,
            Json(CreateKeyResponse { metadata, secret }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("create_key failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn admin_keys_revoke_handler(
    State(state): State<HttpState>,
    headers: AxumHeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(status) = check_admin_auth(&state, &headers) {
        return status.into_response();
    }

    match state.api_key_store.revoke_key(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!("revoke_key failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Extract client API key from `Authorization: Bearer <key>` or `x-api-key: <key>`
/// (supporting Anthropic SDKs and `OpenAI` SDKs equally).
fn extract_client_api_key(headers: &AxumHeaderMap) -> Result<Option<&str>, &'static str> {
    if let Some(auth_header) = headers.get(AUTHORIZATION) {
        let Ok(auth_str) = auth_header.to_str() else {
            return Err("Invalid authorization header");
        };
        let token = auth_str
            .strip_prefix("Bearer ")
            .or_else(|| auth_str.strip_prefix("bearer "))
            .unwrap_or(auth_str)
            .trim();
        if !token.is_empty() {
            return Ok(Some(token));
        }
    }

    if let Some(api_key_header) = headers.get("x-api-key") {
        let Ok(key_str) = api_key_header.to_str() else {
            return Err("Invalid x-api-key header");
        };
        let token = key_str.trim();
        if !token.is_empty() {
            return Ok(Some(token));
        }
    }

    Ok(None)
}

async fn check_client_api_key(
    state: &HttpState,
    headers: &AxumHeaderMap,
) -> Result<Option<String>, Response> {
    if !state.require_api_keys {
        return Ok(None);
    }

    let token = match extract_client_api_key(headers) {
        Ok(Some(token)) => token,
        Ok(None) => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": {
                        "type": "auth_error",
                        "message": "API key required"
                    }
                })),
            )
                .into_response());
        }
        Err(message) => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": {
                        "type": "auth_error",
                        "message": message
                    }
                })),
            )
                .into_response());
        }
    };

    match state.api_key_store.validate_key(token).await {
        Ok(Some(key_id)) => Ok(Some(key_id)),
        Ok(None) => Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "type": "auth_error",
                    "message": "Invalid API key"
                }
            })),
        )
            .into_response()),
        Err(e) => {
            tracing::error!("validate_key failed: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

/// Resiliently parse `model`, `stream`, and `provider` from a JSON request body.
/// Tolerates `"stream": null` and non-object `"provider"` values (e.g., string
/// shorthands) without failing the entire parse.
fn parse_proxy_body_metadata(body: &str) -> (Option<String>, bool, Option<ProviderRouting>) {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) else {
        return (None, false, None);
    };

    let model = map
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let is_streaming = map.get("stream").and_then(Value::as_bool).unwrap_or(false);

    let provider_routing = match map.get("provider") {
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

    (model, is_streaming, provider_routing)
}

fn extract_query_param(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        if k != key {
            return None;
        }
        let decoded = percent_decode_query_value(v);
        let trimmed = decoded.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn percent_decode_query_value(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                    out.push((hi << 4) | lo);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Catch-all handler that proxies any path/method to a worker.
///
/// Extracts the model from the JSON body's `model` field, or from the
/// `X-Model` header. Provider routing can come from the body's `provider`
/// field or the `X-Provider` header (as a provider name filter).
#[allow(clippy::too_many_lines)]
async fn wildcard_handler(
    State(state): State<HttpState>,
    method: Method,
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
    headers: AxumHeaderMap,
    body: String,
) -> Response {
    let owner_key_id = match check_client_api_key(&state, &headers).await {
        Ok(owner_key_id) => owner_key_id,
        Err(response) => return response,
    };

    if !state.provider_enabled {
        return availability_failure_response(RequestFailureReason::ProviderDisabled);
    }

    // Preserve query parameters (e.g. Azure OpenAI `?api-version=...`).
    let endpoint_path = uri
        .path_and_query()
        .map_or_else(|| uri.path().to_string(), ToString::to_string);
    let method_str = method.as_str().to_string();

    let (model, is_streaming, provider_routing) = parse_proxy_body_metadata(&body);

    // Fall back to X-Model header or `?model=<id>` query parameter (e.g. llama.cpp `/props?model=...`).
    let model = model
        .or_else(|| {
            headers
                .get("x-model")
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
        .or_else(|| extract_query_param(uri.query(), "model"));

    let Some(model) = model else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "type": "invalid_request",
                    "message": "model is required (in JSON body or X-Model header)"
                }
            })),
        )
            .into_response();
    };

    // Allow X-Provider header as a shorthand for provider.only routing.
    let provider_routing = provider_routing.or_else(|| {
        headers
            .get("x-provider")
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|provider_name| ProviderRouting {
                only: Some(vec![provider_name.to_string()]),
                ..Default::default()
            })
    });

    if is_streaming {
        return stream_worker_backed_http_response(
            state,
            headers,
            owner_key_id,
            model,
            body,
            endpoint_path,
            method_str,
            provider_routing,
        )
        .await;
    }

    let pending = {
        let mut core = state.core.lock().await;
        match core.submit_http_response_request_for_owner(
            &state.models_provider,
            model,
            &endpoint_path,
            &method_str,
            body,
            forwarded_request_headers(&headers, state.require_api_keys),
            provider_routing,
            owner_key_id,
        ) {
            Ok(pending) => pending,
            Err(failure) => return availability_failure_response(failure.reason),
        }
    };

    let request_id = pending.request_id.clone();
    let mut cancellation_guard = HttpRequestCancellationGuard::new(state.core.clone(), &pending);
    let effective_timeout = if is_metadata_probe_path(&endpoint_path) {
        Some(
            state
                .request_timeout
                .map_or(METADATA_PROBE_TIMEOUT, |limit| {
                    limit.min(METADATA_PROBE_TIMEOUT)
                }),
        )
    } else {
        state.request_timeout
    };
    let wait_result = match effective_timeout {
        Some(limit) => tokio::time::timeout(limit, pending.response_rx).await.ok(),
        None => Some(pending.response_rx.await),
    };

    match wait_result {
        Some(Ok(Ok(response))) => {
            cancellation_guard.disarm();
            response_complete_to_http_response(response)
        }
        Some(Ok(Err(reason))) => {
            cancellation_guard.disarm();
            availability_failure_response(reason)
        }
        Some(Err(_)) => {
            cancellation_guard.disarm();
            bad_gateway_json_response("Worker response channel closed unexpectedly")
        }
        None => {
            cancellation_guard.disarm();
            let mut core = state.core.lock().await;
            let _ = core.cancel_request(&request_id, CancelReason::RequestTimedOut);
            tracing::warn!(request_id = %request_id, "unary HTTP request timed out");
            gateway_timeout_json_response("Request timed out waiting for worker response")
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_worker_backed_http_response(
    state: HttpState,
    headers: AxumHeaderMap,
    owner_key_id: Option<String>,
    model: String,
    body: String,
    endpoint_path: String,
    method: String,
    provider_routing: Option<ProviderRouting>,
) -> Response {
    if !state.provider_enabled {
        return availability_failure_response(RequestFailureReason::ProviderDisabled);
    }

    let mut pending = {
        let mut core = state.core.lock().await;
        match core.submit_http_streaming_request_for_owner(
            &state.models_provider,
            model,
            &endpoint_path,
            &method,
            body,
            forwarded_request_headers(&headers, state.require_api_keys),
            provider_routing,
            owner_key_id,
        ) {
            Ok(pending) => pending,
            Err(failure) => return availability_failure_response(failure.reason),
        }
    };

    let request_id = pending.request_id.clone();
    let mut cancellation_guard =
        HttpRequestCancellationGuard::new_streaming(state.core.clone(), &pending);
    let first_event = match state.request_timeout {
        Some(limit) => tokio::time::timeout(limit, pending.event_rx.recv())
            .await
            .ok(),
        None => Some(pending.event_rx.recv().await),
    };

    match first_event {
        Some(Some(HttpResponseEvent::Chunk(first_chunk))) => streaming_http_response(
            first_chunk,
            pending,
            cancellation_guard,
            state.core,
            state.request_timeout,
        ),
        Some(Some(HttpResponseEvent::Complete(response))) => {
            cancellation_guard.disarm();
            response_complete_to_http_response(response)
        }
        Some(Some(HttpResponseEvent::Failure(reason))) => {
            cancellation_guard.disarm();
            availability_failure_response(reason)
        }
        Some(Some(HttpResponseEvent::WorkerError(message))) => {
            cancellation_guard.disarm();
            bad_gateway_json_response(&message)
        }
        Some(None) => {
            cancellation_guard.disarm();
            bad_gateway_json_response("Worker response stream closed before sending any data")
        }
        None => {
            cancellation_guard.disarm();
            let mut core = state.core.lock().await;
            let _ = core.cancel_request(&request_id, CancelReason::RequestTimedOut);
            tracing::warn!(request_id = %request_id, "streaming request timed out before first chunk");
            gateway_timeout_json_response("Request timed out waiting for first streaming chunk")
        }
    }
}

fn is_hop_by_hop_header(name: &HeaderName, connection_tokens: &HashSet<String>) -> bool {
    *name == CONNECTION
        || *name == TRANSFER_ENCODING
        || *name == UPGRADE
        || *name == TE
        || *name == TRAILER
        || *name == PROXY_AUTHENTICATE
        || *name == PROXY_AUTHORIZATION
        || name.as_str().eq_ignore_ascii_case("keep-alive")
        || name.as_str().eq_ignore_ascii_case("trailers")
        || connection_tokens.contains(&name.as_str().to_ascii_lowercase())
}

fn parse_connection_header_tokens(headers: &AxumHeaderMap) -> HashSet<String> {
    let mut tokens = HashSet::new();
    for value in headers.get_all(CONNECTION) {
        if let Ok(raw) = value.to_str() {
            for token in raw.split(',') {
                let trimmed = token.trim().to_ascii_lowercase();
                if !trimmed.is_empty() {
                    tokens.insert(trimmed);
                }
            }
        }
    }
    tokens
}

/// Headers that belong strictly to the client↔proxy HTTP hop (browser CORS /
/// fetch metadata, cookies, and ingress forwarding headers) and must never be
/// relayed to local inference backends behind a worker. Forwarding `Origin` or
/// `Referer` causes localhost backends with DNS-rebinding / CORS guards (such
/// as Ollama, llama-server, or loom) to reject already-authenticated browser
/// requests with `403 Forbidden`.
fn is_client_hop_header(name: &HeaderName) -> bool {
    let lower = name.as_str();
    lower.eq_ignore_ascii_case("origin")
        || lower.eq_ignore_ascii_case("referer")
        || lower.eq_ignore_ascii_case("cookie")
        || lower.eq_ignore_ascii_case("forwarded")
        || lower.eq_ignore_ascii_case("x-real-ip")
        || lower.eq_ignore_ascii_case("true-client-ip")
        || lower.eq_ignore_ascii_case("cdn-loop")
        || lower.starts_with("sec-fetch-")
        || lower.starts_with("sec-ch-")
        || lower.starts_with("x-forwarded-")
        || lower.starts_with("cf-")
}

fn forwarded_request_headers(headers: &AxumHeaderMap, strip_client_auth: bool) -> HeaderMap {
    let connection_tokens = parse_connection_header_tokens(headers);

    headers
        .iter()
        .filter(|(name, _)| {
            *name != CONTENT_LENGTH
                && *name != HOST
                && *name != ACCEPT_ENCODING
                && !is_hop_by_hop_header(name, &connection_tokens)
                && !is_client_hop_header(name)
                && (!strip_client_auth
                    || (*name != AUTHORIZATION && !name.as_str().eq_ignore_ascii_case("x-api-key")))
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

fn format_sse_error(error_type: &str, message: &str) -> Bytes {
    let payload = serde_json::json!({
        "error": {
            "type": error_type,
            "message": message,
        }
    });
    Bytes::from(format!("event: error\ndata: {payload}\n\n"))
}

struct StreamState {
    next_chunk: Option<String>,
    event_rx: tokio::sync::mpsc::Receiver<HttpResponseEvent>,
    cancellation_guard: HttpRequestCancellationGuard,
    core: Arc<Mutex<ProxyServerCore>>,
    request_id: String,
    stream_idle_timeout: Option<Duration>,
    streamed_bytes: usize,
    terminated: bool,
}

#[allow(clippy::too_many_lines)]
fn streaming_http_response(
    first_chunk: String,
    pending: PendingStreamingHttpResponse,
    cancellation_guard: HttpRequestCancellationGuard,
    core: Arc<Mutex<ProxyServerCore>>,
    stream_idle_timeout: Option<Duration>,
) -> Response {
    let state = StreamState {
        next_chunk: Some(first_chunk),
        event_rx: pending.event_rx,
        cancellation_guard,
        core,
        request_id: pending.request_id,
        stream_idle_timeout,
        streamed_bytes: 0,
        terminated: false,
    };

    let stream = stream::unfold(state, |mut st| async move {
        if st.terminated {
            return None;
        }

        if let Some(chunk) = st.next_chunk.take() {
            let next_total = st.streamed_bytes.saturating_add(chunk.len());
            if next_total > MAX_STREAM_RESPONSE_BYTES {
                // Cancel worker immediately so it aborts upstream generation.
                st.cancellation_guard
                    .cancel_now(CancelReason::ClientDisconnected);
                st.terminated = true;
                return Some((
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(
                        OVERSIZED_STREAM_ERROR_SSE.as_bytes(),
                    )),
                    st,
                ));
            }
            st.streamed_bytes = next_total;
            return Some((Ok::<_, std::convert::Infallible>(Bytes::from(chunk)), st));
        }

        let next_event = match st.stream_idle_timeout {
            Some(limit) => tokio::time::timeout(limit, st.event_rx.recv()).await.ok(),
            None => Some(st.event_rx.recv().await),
        };

        match next_event {
            Some(Some(HttpResponseEvent::Chunk(chunk))) => {
                let next_total = st.streamed_bytes.saturating_add(chunk.len());
                if next_total > MAX_STREAM_RESPONSE_BYTES {
                    st.cancellation_guard
                        .cancel_now(CancelReason::ClientDisconnected);
                    st.terminated = true;
                    Some((
                        Ok(Bytes::from_static(OVERSIZED_STREAM_ERROR_SSE.as_bytes())),
                        st,
                    ))
                } else {
                    st.streamed_bytes = next_total;
                    Some((Ok(Bytes::from(chunk)), st))
                }
            }
            Some(Some(HttpResponseEvent::Complete(_)) | None) => {
                st.cancellation_guard.disarm();
                None
            }
            Some(Some(HttpResponseEvent::Failure(reason))) => {
                st.cancellation_guard.disarm();
                st.terminated = true;
                let sse = match reason {
                    RequestFailureReason::WorkerDisconnectedMidStream => {
                        Bytes::from_static(STREAM_DISCONNECTED_ERROR_SSE.as_bytes())
                    }
                    other => format_sse_error("stream_error", failure_reason_message(other)),
                };
                Some((Ok(sse), st))
            }
            Some(Some(HttpResponseEvent::WorkerError(message))) => {
                st.cancellation_guard.disarm();
                st.terminated = true;
                Some((Ok(format_sse_error("backend_error", &message)), st))
            }
            None => {
                st.cancellation_guard.disarm();
                {
                    let mut core = st.core.lock().await;
                    let _ = core.cancel_request(&st.request_id, CancelReason::RequestTimedOut);
                }
                tracing::warn!(
                    request_id = %st.request_id,
                    "streaming HTTP response timed out waiting for next chunk"
                );
                st.terminated = true;
                Some((
                    Ok(Bytes::from_static(STREAM_TIMEOUT_ERROR_SSE.as_bytes())),
                    st,
                ))
            }
        }
    });

    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

fn response_complete_to_http_response(response: ResponseCompleteMessage) -> Response {
    let mut http_response = Response::new(response.body.unwrap_or_default().into());
    *http_response.status_mut() =
        StatusCode::from_u16(response.status_code).unwrap_or(StatusCode::BAD_GATEWAY);

    let connection_tokens: HashSet<String> = response
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| {
            value
                .split(',')
                .map(|token| token.trim().to_ascii_lowercase())
                .filter(|token| !token.is_empty())
        })
        .collect();

    for (name, value) in response.headers {
        let Ok(header_name) = HeaderName::try_from(name) else {
            continue;
        };
        if header_name == CONTENT_LENGTH
            || is_hop_by_hop_header(&header_name, &connection_tokens)
            || header_name.as_str().starts_with("access-control-")
        {
            continue;
        }
        let Ok(header_value) = HeaderValue::from_str(&value) else {
            continue;
        };
        http_response
            .headers_mut()
            .insert(header_name, header_value);
    }

    http_response
}

fn bad_gateway_json_response(message: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(serde_json::json!({
            "error": {
                "type": "backend_error",
                "message": message,
            }
        })),
    )
        .into_response()
}

fn gateway_timeout_json_response(message: &str) -> Response {
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(serde_json::json!({
            "error": {
                "type": "timeout",
                "message": message,
            }
        })),
    )
        .into_response()
}

fn is_metadata_probe_path(endpoint_path: &str) -> bool {
    let path = endpoint_path
        .split_once('?')
        .map_or(endpoint_path, |(path, _)| path);
    matches!(
        path,
        "/props" | "/api/v1/models" | "/api/v0/models" | "/api/show"
    )
}

const fn failure_reason_message(reason: RequestFailureReason) -> &'static str {
    match reason {
        RequestFailureReason::QueueTimedOut => "Request timed out waiting for worker",
        RequestFailureReason::QueueFull => "Service temporarily at capacity, please retry",
        RequestFailureReason::NoWorkersAvailable => "No workers available to handle request",
        RequestFailureReason::EndpointNotSupported => {
            "Connected worker does not support this endpoint path"
        }
        RequestFailureReason::ProviderDisabled => "Provider is currently disabled",
        RequestFailureReason::MaxRequeuesExceeded => {
            "Request could not be processed after multiple attempts"
        }
        RequestFailureReason::ProviderDeleted => "Internal server error processing request",
        RequestFailureReason::GracefulShutdownTimedOut => {
            "Service temporarily unavailable while workers shut down"
        }
        RequestFailureReason::RequestAlreadyCanceled => "Request was canceled",
        RequestFailureReason::WorkerDisconnectedMidStream => "Worker disconnected mid-stream",
    }
}

fn availability_failure_response(reason: RequestFailureReason) -> Response {
    let status = match reason {
        RequestFailureReason::WorkerDisconnectedMidStream => StatusCode::BAD_GATEWAY,
        RequestFailureReason::EndpointNotSupported => StatusCode::NOT_FOUND,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    let message = failure_reason_message(reason);

    (
        status,
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                HeaderName::from_static("x-content-type-options"),
                HeaderValue::from_static("nosniff"),
            ),
        ],
        format!("{message}\n"),
    )
        .into_response()
}

struct HttpRequestCancellationGuard {
    core: Arc<Mutex<ProxyServerCore>>,
    request_id: String,
    armed: bool,
}

impl HttpRequestCancellationGuard {
    fn new(core: Arc<Mutex<ProxyServerCore>>, pending: &PendingHttpResponse) -> Self {
        Self {
            core,
            request_id: pending.request_id.clone(),
            armed: true,
        }
    }

    fn new_streaming(
        core: Arc<Mutex<ProxyServerCore>>,
        pending: &PendingStreamingHttpResponse,
    ) -> Self {
        Self {
            core,
            request_id: pending.request_id.clone(),
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn cancel_now(&mut self, reason: CancelReason) {
        if !self.armed {
            return;
        }
        self.armed = false;
        let core = self.core.clone();
        let request_id = self.request_id.clone();
        tokio::spawn(async move {
            let mut core = core.lock().await;
            let _ = core.cancel_request(&request_id, reason);
        });
    }
}

impl Drop for HttpRequestCancellationGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }

        let core = self.core.clone();
        let request_id = self.request_id.clone();
        tokio::spawn(async move {
            let mut core = core.lock().await;
            let _ = core.cancel_request(&request_id, CancelReason::ClientDisconnected);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_request_headers_strips_browser_origin_and_proxy_headers() {
        let mut raw = AxumHeaderMap::new();
        raw.insert(
            "origin",
            HeaderValue::from_static("https://chat.neuralcolumn.com"),
        );
        raw.insert(
            "referer",
            HeaderValue::from_static("https://chat.neuralcolumn.com/"),
        );
        raw.insert("cookie", HeaderValue::from_static("sid=secret"));
        raw.insert("sec-fetch-site", HeaderValue::from_static("cross-site"));
        raw.insert("sec-fetch-mode", HeaderValue::from_static("cors"));
        raw.insert(
            "sec-ch-ua",
            HeaderValue::from_static("\"Chromium\";v=\"130\""),
        );
        raw.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.10"));
        raw.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        raw.insert("forwarded", HeaderValue::from_static("for=203.0.113.10"));
        raw.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.10"));
        raw.insert(
            "authorization",
            HeaderValue::from_static("Bearer mr_live_secret"),
        );
        raw.insert("content-type", HeaderValue::from_static("application/json"));
        raw.insert("accept", HeaderValue::from_static("text/event-stream"));
        raw.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));

        let forwarded = forwarded_request_headers(&raw, true);

        for stripped in [
            "origin",
            "referer",
            "cookie",
            "sec-fetch-site",
            "sec-fetch-mode",
            "sec-ch-ua",
            "x-forwarded-for",
            "x-forwarded-proto",
            "forwarded",
            "cf-connecting-ip",
            "authorization",
        ] {
            assert!(
                !forwarded.contains_key(stripped),
                "expected {stripped} to be stripped from forwarded headers"
            );
        }
        assert_eq!(
            forwarded.get("content-type").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            forwarded.get("accept").map(String::as_str),
            Some("text/event-stream")
        );
        assert_eq!(
            forwarded.get("anthropic-version").map(String::as_str),
            Some("2023-06-01")
        );
    }

    #[test]
    fn response_complete_strips_backend_cors_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type".to_string(), "application/json".to_string());
        headers.insert(
            "access-control-allow-origin".to_string(),
            "http://localhost:3000".to_string(),
        );
        let resp = response_complete_to_http_response(ResponseCompleteMessage {
            request_id: "req-1".to_string(),
            status_code: 200,
            headers,
            body: Some("{}".to_string()),
            token_counts: None,
        });
        assert!(!resp.headers().contains_key("access-control-allow-origin"));
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
    }
}
