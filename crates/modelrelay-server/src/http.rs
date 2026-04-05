use std::sync::Arc;
use std::time::Instant;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path, State},
    http::{
        HeaderMap as AxumHeaderMap, HeaderName, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST},
    },
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use futures_util::stream;
use modelrelay_protocol::{HeaderMap, ResponseCompleteMessage};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    AdminWorkerInfo, CancelReason, HttpResponseEvent, PendingHttpResponse,
    PendingStreamingHttpResponse, ProxyServerCore, RequestFailureReason, WorkerSocketApp,
    api_keys::{ApiKeyMetadata, ApiKeyStore},
};

const OPENAI_MODELS_PROVIDER: &str = "openai";
const MAX_STREAM_RESPONSE_BYTES: usize = 64 * 1024;
const OVERSIZED_STREAM_ERROR_SSE: &str = "event: error\ndata: {\"error\":{\"type\":\"stream_error\",\"message\":\"stream exceeded size limit\"}}\n\n";

#[derive(Clone)]
pub struct ProxyHttpApp {
    core: Arc<Mutex<ProxyServerCore>>,
    models_provider: String,
    provider_enabled: bool,
    worker_socket_app: Option<WorkerSocketApp>,
    admin_token: Option<String>,
    require_api_keys: bool,
    api_key_store: Arc<dyn ApiKeyStore>,
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

    pub fn router(self) -> Router {
        let router = Router::new()
            .route("/health", get(health_handler))
            .route("/v1/models", get(models_handler))
            .route("/v1/chat/completions", post(chat_completions_handler))
            .route("/v1/messages", post(messages_handler))
            .route("/v1/responses", post(responses_handler))
            .route("/admin/workers", get(admin_workers_handler))
            .route("/admin/stats", get(admin_stats_handler))
            .route(
                "/admin/keys",
                get(admin_keys_handler).post(admin_keys_create_handler),
            )
            .route("/admin/keys/{id}", delete(admin_keys_revoke_handler))
            .with_state(HttpState {
                core: self.core,
                models_provider: self.models_provider,
                provider_enabled: self.provider_enabled,
                started_at: Instant::now(),
                admin_token: self.admin_token,
                require_api_keys: self.require_api_keys,
                api_key_store: self.api_key_store,
            });

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
}

#[derive(Debug, Serialize, PartialEq)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    workers_connected: usize,
    queue_depth: usize,
    uptime_secs: f64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ModelsResponse {
    object: &'static str,
    data: Vec<ModelObject>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ModelObject {
    id: String,
    object: &'static str,
    owned_by: &'static str,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionsRequest {
    model: String,
    #[serde(default)]
    stream: bool,
}

async fn health_handler(State(state): State<HttpState>) -> Json<HealthResponse> {
    let (workers_connected, queue_depth) = {
        let core = state.core.lock().await;
        (core.connected_worker_count(), core.total_queue_depth())
    };

    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        workers_connected,
        queue_depth,
        uptime_secs: state.started_at.elapsed().as_secs_f64(),
    })
}

async fn models_handler(State(state): State<HttpState>) -> Json<ModelsResponse> {
    let models = {
        let core = state.core.lock().await;
        core.provider_models(&state.models_provider)
    };

    Json(ModelsResponse {
        object: "list",
        data: models
            .into_iter()
            .map(|id| ModelObject {
                id,
                object: "model",
                owned_by: "worker-proxy",
            })
            .collect(),
    })
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

#[derive(Serialize)]
struct AdminKeysResponse {
    keys: Vec<ApiKeyMetadata>,
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

#[derive(Debug, Deserialize)]
struct CreateKeyRequest {
    name: String,
}

#[derive(Serialize)]
struct CreateKeyResponse {
    #[serde(flatten)]
    metadata: ApiKeyMetadata,
    secret: String,
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

async fn check_client_api_key(state: &HttpState, headers: &AxumHeaderMap) -> Result<(), Response> {
    if !state.require_api_keys {
        return Ok(());
    }

    let Some(auth_header) = headers.get("authorization") else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(
                serde_json::json!({"error": {"type": "auth_error", "message": "API key required"}}),
            ),
        )
            .into_response());
    };

    let Ok(auth_str) = auth_header.to_str() else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": {"type": "auth_error", "message": "Invalid authorization header"}})),
        )
            .into_response());
    };

    let token = auth_str.strip_prefix("Bearer ").unwrap_or(auth_str);

    match state.api_key_store.validate_key(token).await {
        Ok(Some(_)) => Ok(()),
        Ok(None) => Err((
            StatusCode::UNAUTHORIZED,
            Json(
                serde_json::json!({"error": {"type": "auth_error", "message": "Invalid API key"}}),
            ),
        )
            .into_response()),
        Err(e) => {
            tracing::error!("validate_key failed: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

async fn chat_completions_handler(
    State(state): State<HttpState>,
    headers: AxumHeaderMap,
    body: String,
) -> Response {
    worker_backed_http_handler(state, headers, body, "/v1/chat/completions").await
}

async fn messages_handler(
    State(state): State<HttpState>,
    headers: AxumHeaderMap,
    body: String,
) -> Response {
    worker_backed_http_handler(state, headers, body, "/v1/messages").await
}

async fn responses_handler(
    State(state): State<HttpState>,
    headers: AxumHeaderMap,
    body: String,
) -> Response {
    worker_backed_http_handler(state, headers, body, "/v1/responses").await
}

async fn worker_backed_http_handler(
    state: HttpState,
    headers: AxumHeaderMap,
    body: String,
    endpoint_path: &'static str,
) -> Response {
    if let Err(response) = check_client_api_key(&state, &headers).await {
        return response;
    }

    let Ok(request) = serde_json::from_str::<ChatCompletionsRequest>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    if !state.provider_enabled {
        return availability_failure_response(RequestFailureReason::ProviderDisabled);
    }

    if request.stream {
        return stream_worker_backed_http_response(
            state,
            headers,
            request.model,
            body,
            endpoint_path,
        )
        .await;
    }

    let pending = {
        let mut core = state.core.lock().await;
        match core.submit_http_response_request(
            &state.models_provider,
            request.model,
            endpoint_path,
            body,
            forwarded_request_headers(&headers),
        ) {
            Ok(pending) => pending,
            Err(failure) => return availability_failure_response(failure.reason),
        }
    };

    let mut cancellation_guard = HttpRequestCancellationGuard::new(state.core.clone(), &pending);
    match pending.response_rx.await {
        Ok(Ok(response)) => {
            cancellation_guard.disarm();
            response_complete_to_http_response(response)
        }
        Ok(Err(reason)) => {
            cancellation_guard.disarm();
            availability_failure_response(reason)
        }
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn stream_worker_backed_http_response(
    state: HttpState,
    headers: AxumHeaderMap,
    model: String,
    body: String,
    endpoint_path: &'static str,
) -> Response {
    if !state.provider_enabled {
        return availability_failure_response(RequestFailureReason::ProviderDisabled);
    }

    let mut pending = {
        let mut core = state.core.lock().await;
        match core.submit_http_streaming_request(
            &state.models_provider,
            model,
            endpoint_path,
            body,
            forwarded_request_headers(&headers),
        ) {
            Ok(pending) => pending,
            Err(failure) => return availability_failure_response(failure.reason),
        }
    };

    let cancellation_guard =
        HttpRequestCancellationGuard::new_streaming(state.core.clone(), &pending);
    match pending.event_rx.recv().await {
        Some(HttpResponseEvent::Chunk(first_chunk)) => {
            streaming_http_response(first_chunk, pending, cancellation_guard)
        }
        Some(HttpResponseEvent::Complete(response)) => {
            let mut guard = cancellation_guard;
            guard.disarm();
            response_complete_to_http_response(response)
        }
        Some(HttpResponseEvent::Failure(reason)) => availability_failure_response(reason),
        None => StatusCode::BAD_GATEWAY.into_response(),
    }
}

fn forwarded_request_headers(headers: &AxumHeaderMap) -> HeaderMap {
    headers
        .iter()
        .filter(|(name, _)| *name != CONTENT_LENGTH && *name != CONNECTION && *name != HOST)
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

fn streaming_http_response(
    first_chunk: String,
    pending: PendingStreamingHttpResponse,
    cancellation_guard: HttpRequestCancellationGuard,
) -> Response {
    let stream = stream::unfold(
        (
            Some(first_chunk),
            pending.event_rx,
            cancellation_guard,
            0_usize,
            false,
        ),
        |(next_chunk, mut event_rx, mut cancellation_guard, streamed_bytes, terminated)| async move {
            if terminated {
                return None;
            }

            if let Some(chunk) = next_chunk {
                let next_total = streamed_bytes.saturating_add(chunk.len());
                if next_total > MAX_STREAM_RESPONSE_BYTES {
                    cancellation_guard.disarm();
                    return Some((
                        Ok::<_, std::convert::Infallible>(Bytes::from_static(
                            OVERSIZED_STREAM_ERROR_SSE.as_bytes(),
                        )),
                        (None, event_rx, cancellation_guard, streamed_bytes, true),
                    ));
                }

                return Some((
                    Ok::<_, std::convert::Infallible>(Bytes::from(chunk)),
                    (None, event_rx, cancellation_guard, next_total, false),
                ));
            }

            match event_rx.recv().await {
                Some(HttpResponseEvent::Chunk(chunk)) => {
                    let next_total = streamed_bytes.saturating_add(chunk.len());
                    if next_total > MAX_STREAM_RESPONSE_BYTES {
                        cancellation_guard.disarm();
                        Some((
                            Ok(Bytes::from_static(OVERSIZED_STREAM_ERROR_SSE.as_bytes())),
                            (None, event_rx, cancellation_guard, streamed_bytes, true),
                        ))
                    } else {
                        Some((
                            Ok(Bytes::from(chunk)),
                            (None, event_rx, cancellation_guard, next_total, false),
                        ))
                    }
                }
                Some(HttpResponseEvent::Complete(_) | HttpResponseEvent::Failure(_)) => {
                    cancellation_guard.disarm();
                    None
                }
                None => None,
            }
        },
    );

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

    for (name, value) in response.headers {
        let Ok(header_name) = HeaderName::try_from(name) else {
            continue;
        };
        let Ok(header_value) = HeaderValue::from_str(&value) else {
            continue;
        };
        http_response
            .headers_mut()
            .insert(header_name, header_value);
    }

    http_response
}

fn availability_failure_response(reason: RequestFailureReason) -> Response {
    let message = match reason {
        RequestFailureReason::QueueTimedOut => "Request timed out waiting for worker",
        RequestFailureReason::QueueFull => "Service temporarily at capacity, please retry",
        RequestFailureReason::NoWorkersAvailable => "No workers available to handle request",
        RequestFailureReason::ProviderDisabled => "Provider is currently disabled",
        RequestFailureReason::MaxRequeuesExceeded => {
            "Request could not be processed after multiple attempts"
        }
        RequestFailureReason::ProviderDeleted => "Internal server error processing request",
        RequestFailureReason::GracefulShutdownTimedOut => {
            "Service temporarily unavailable while workers shut down"
        }
        RequestFailureReason::RequestAlreadyCanceled => "Request was canceled",
    };

    (
        StatusCode::SERVICE_UNAVAILABLE,
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
