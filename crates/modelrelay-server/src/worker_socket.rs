use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration as StdDuration, Instant},
};

use axum::{
    Router,
    extract::{
        ConnectInfo, FromRequestParts, Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::request::Parts,
    routing::get,
};
use modelrelay_protocol::{
    CancelMessage, CancelReason, GracefulShutdownMessage, ModelsUpdateMessage, PingMessage,
    PongMessage, ResponseChunkMessage, ServerToWorkerMessage, WorkerToServerMessage,
};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tokio::sync::{Mutex, Notify};
use tokio::time::{Duration, timeout};

use crate::{
    ApiKeyStore, GracefulShutdownDisconnectReason, GracefulShutdownSignal, HttpResponseEvent,
    ProxyServerCore, WorkerCancelSignal, WorkerModelsRefreshSignal,
};

const WORKER_SECRET_HEADER: &str = "x-worker-secret";
const WORKER_PROTOCOL_VERSION: &str = "2026-04-bridge-v1";
const CLOSE_REASON_AUTH_FAILED: &str = "worker authentication failed";
const CLOSE_REASON_PROTOCOL_ERROR: &str = "worker registration protocol error";
const CLOSE_REASON_AUTH_RATE_LIMITED: &str = "worker authentication temporarily rate limited";
const CLOSE_REASON_GRACEFUL_SHUTDOWN_COMPLETE: &str = "graceful shutdown complete";
const CLOSE_REASON_GRACEFUL_SHUTDOWN_TIMED_OUT: &str = "graceful shutdown timed out";
const CLOSE_REASON_STALE_HEARTBEAT: &str = "worker heartbeat timed out";
const CLOSE_CODE_POLICY_VIOLATION: u16 = 1008;
const CLOSE_CODE_PROTOCOL_ERROR: u16 = 1002;
const CLOSE_CODE_NORMAL: u16 = 1000;
const AUTH_FAILURE_THRESHOLD: u32 = 3;
const DEFAULT_AUTH_RATE_LIMIT_COOLDOWN: StdDuration = StdDuration::from_millis(250);
const MAX_FAILED_AUTH_CLIENTS: usize = 10_000;
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const DEFAULT_HEARTBEAT_PONG_TIMEOUT: Duration = Duration::from_secs(10);
const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_SEND_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
const STREAM_CHUNK_SEND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerSocketProviderConfig {
    pub worker_secret: String,
    pub enabled: bool,
}

impl WorkerSocketProviderConfig {
    #[must_use]
    pub fn enabled(worker_secret: impl Into<String>) -> Self {
        Self {
            worker_secret: worker_secret.into(),
            enabled: true,
        }
    }

    #[must_use]
    pub fn disabled(worker_secret: impl Into<String>) -> Self {
        Self {
            worker_secret: worker_secret.into(),
            enabled: false,
        }
    }
}

#[derive(Clone)]
pub struct WorkerSocketApp {
    state: WorkerSocketState,
}

impl WorkerSocketApp {
    pub const ROUTE_PATH: &str = "/v1/worker/connect";

    #[must_use]
    pub fn new(core: Arc<Mutex<ProxyServerCore>>) -> Self {
        Self {
            state: WorkerSocketState {
                core,
                providers: HashMap::new(),
                api_key_store: None,
                heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
                heartbeat_pong_timeout: DEFAULT_HEARTBEAT_PONG_TIMEOUT,
                auth_rate_limit_cooldown: DEFAULT_AUTH_RATE_LIMIT_COOLDOWN,
                trust_proxy_headers: false,
                failed_auth_by_client: Arc::new(Mutex::new(HashMap::new())),
            },
        }
    }

    /// Override heartbeat timing (for tests). Production uses 30s interval / 10s pong timeout.
    #[must_use]
    pub fn with_heartbeat(mut self, interval: Duration, pong_timeout: Duration) -> Self {
        self.state.heartbeat_interval = interval;
        self.state.heartbeat_pong_timeout = pong_timeout;
        self
    }

    /// Override the cooldown duration applied after repeated failed authentication attempts.
    #[must_use]
    pub fn with_auth_rate_limit_cooldown(mut self, cooldown: StdDuration) -> Self {
        self.state.auth_rate_limit_cooldown = cooldown;
        self
    }

    /// Trust `Forwarded` / `X-Forwarded-For` headers when identifying worker peers for
    /// auth rate limiting. Keep `false` unless running behind a trusted reverse proxy.
    #[must_use]
    pub fn with_trust_proxy_headers(mut self, trust: bool) -> Self {
        self.state.trust_proxy_headers = trust;
        self
    }

    #[must_use]
    pub fn with_provider(
        mut self,
        provider: impl Into<String>,
        config: WorkerSocketProviderConfig,
    ) -> Self {
        self.state.providers.insert(provider.into(), config);
        self
    }

    /// Enable API-key-based worker authentication.
    ///
    /// When set, workers can authenticate with any valid (non-revoked) API key
    /// as their `worker_secret`. The worker joins the provider specified in the
    /// connection query string and is scoped to that API key's owner identity.
    #[must_use]
    pub fn with_api_key_store(mut self, store: Arc<dyn ApiKeyStore>) -> Self {
        self.state.api_key_store = Some(store);
        self
    }

    pub fn router(self) -> Router {
        Router::new()
            .route(Self::ROUTE_PATH, get(worker_connect_handler))
            .with_state(self.state)
    }
}

#[derive(Clone)]
struct WorkerSocketState {
    core: Arc<Mutex<ProxyServerCore>>,
    providers: HashMap<String, WorkerSocketProviderConfig>,
    api_key_store: Option<Arc<dyn ApiKeyStore>>,
    heartbeat_interval: Duration,
    heartbeat_pong_timeout: Duration,
    auth_rate_limit_cooldown: StdDuration,
    trust_proxy_headers: bool,
    failed_auth_by_client: Arc<Mutex<HashMap<String, FailedAuthState>>>,
}

#[derive(Debug, Deserialize)]
struct WorkerConnectQuery {
    provider: Option<String>,
    #[serde(default, rename = "worker_secret")]
    worker_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthOutcome {
    Authenticated {
        provider: String,
        owner_key_id: Option<String>,
    },
    Rejected {
        reason: &'static str,
    },
}

#[derive(Debug, Clone)]
struct FailedAuthState {
    failures: u32,
    last_failed_at: Instant,
    blocked_until: Option<Instant>,
}

/// Optional peer `SocketAddr` extractor that never fails when the server is mounted
/// without `into_make_service_with_connect_info` (e.g., in integration tests).
struct MaybeConnectInfo(Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for MaybeConnectInfo {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let addr = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0);
        Ok(Self(addr))
    }
}

fn resolve_client_identity(
    state: &WorkerSocketState,
    peer_addr: Option<SocketAddr>,
    headers: &axum::http::HeaderMap,
) -> String {
    if state.trust_proxy_headers
        && let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                headers
                    .get(axum::http::header::FORWARDED)
                    .and_then(|value| value.to_str().ok())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
    {
        return forwarded.to_string();
    }

    if let Some(addr) = peer_addr {
        return addr.ip().to_string();
    }

    "unknown-client".to_string()
}

async fn worker_connect_handler(
    ws: WebSocketUpgrade,
    State(state): State<WorkerSocketState>,
    MaybeConnectInfo(peer_addr): MaybeConnectInfo,
    Query(query): Query<WorkerConnectQuery>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let client_identity = resolve_client_identity(&state, peer_addr, &headers);

    match authenticate_connection(&state, &client_identity, &headers, &query).await {
        AuthOutcome::Authenticated {
            provider,
            owner_key_id,
        } => ws.on_upgrade(move |socket| async move {
            handle_authenticated_socket(socket, state, provider, owner_key_id).await;
        }),
        AuthOutcome::Rejected { reason } => {
            tracing::warn!(
                client = %client_identity,
                reason = %reason,
                "rejecting worker websocket authentication"
            );
            ws.on_upgrade(move |socket| async move {
                close_socket(socket, CLOSE_CODE_POLICY_VIOLATION, reason).await;
            })
        }
    }
}

async fn authenticate_connection(
    state: &WorkerSocketState,
    client_identity: &str,
    headers: &axum::http::HeaderMap,
    query: &WorkerConnectQuery,
) -> AuthOutcome {
    if client_is_rate_limited(state, client_identity).await {
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_RATE_LIMITED,
        };
    }

    let presented_secret = headers
        .get(WORKER_SECRET_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .or_else(|| query.worker_secret.clone());

    let Some(presented_secret) = presented_secret else {
        record_failed_auth(state, client_identity.to_string()).await;
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_FAILED,
        };
    };

    let provider = query.provider.as_deref().unwrap_or("local");

    // 1. Try static provider match (self-hosted / shared fleet mode)
    if let Some(config) = state.providers.get(provider)
        && config.enabled
    {
        let secret_matches: bool = presented_secret
            .as_bytes()
            .ct_eq(config.worker_secret.as_bytes())
            .into();
        if secret_matches {
            clear_failed_auth(state, client_identity).await;
            return AuthOutcome::Authenticated {
                provider: provider.to_string(),
                owner_key_id: None,
            };
        }
    }

    // 2. Try API key store (per-user auth for hosted multi-tenant mode).
    // The validated key_id scopes this worker to the presenting tenant.
    if let Some(ref store) = state.api_key_store {
        match store.validate_key(&presented_secret).await {
            Ok(Some(key_id)) => {
                clear_failed_auth(state, client_identity).await;
                return AuthOutcome::Authenticated {
                    provider: provider.to_string(),
                    owner_key_id: Some(key_id),
                };
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(error = %error, "worker API key validation store error");
            }
        }
    }

    record_failed_auth(state, client_identity.to_string()).await;
    AuthOutcome::Rejected {
        reason: CLOSE_REASON_AUTH_FAILED,
    }
}

async fn client_is_rate_limited(state: &WorkerSocketState, client_identity: &str) -> bool {
    let now = Instant::now();
    let cooldown = state.auth_rate_limit_cooldown;
    let mut failed_auth_by_client = state.failed_auth_by_client.lock().await;

    match failed_auth_by_client.get(client_identity) {
        Some(entry)
            if entry
                .blocked_until
                .is_some_and(|blocked_until| blocked_until > now) =>
        {
            true
        }
        Some(_) => {
            if failed_auth_by_client
                .get(client_identity)
                .is_some_and(|entry| now.duration_since(entry.last_failed_at) > cooldown)
            {
                failed_auth_by_client.remove(client_identity);
            }
            false
        }
        None => false,
    }
}

async fn record_failed_auth(state: &WorkerSocketState, client_identity: String) {
    let now = Instant::now();
    let cooldown = state.auth_rate_limit_cooldown;
    let mut failed_auth_by_client = state.failed_auth_by_client.lock().await;

    if failed_auth_by_client.len() >= MAX_FAILED_AUTH_CLIENTS {
        failed_auth_by_client.retain(|_, entry| {
            entry.blocked_until.is_some_and(|until| until > now)
                || now.duration_since(entry.last_failed_at) <= cooldown
        });
        if failed_auth_by_client.len() >= MAX_FAILED_AUTH_CLIENTS {
            failed_auth_by_client.clear();
        }
    }

    let entry = failed_auth_by_client
        .entry(client_identity)
        .or_insert(FailedAuthState {
            failures: 0,
            last_failed_at: now,
            blocked_until: None,
        });

    if entry
        .blocked_until
        .is_some_and(|blocked_until| blocked_until <= now)
        || now.duration_since(entry.last_failed_at) > cooldown
    {
        *entry = FailedAuthState {
            failures: 0,
            last_failed_at: now,
            blocked_until: None,
        };
    }

    entry.failures += 1;
    entry.last_failed_at = now;
    if entry.failures >= AUTH_FAILURE_THRESHOLD {
        entry.blocked_until = Some(now + cooldown);
    }
}

async fn clear_failed_auth(state: &WorkerSocketState, client_identity: &str) {
    let mut failed_auth_by_client = state.failed_auth_by_client.lock().await;
    failed_auth_by_client.remove(client_identity);
}

#[allow(clippy::too_many_lines)]
async fn handle_authenticated_socket(
    mut socket: WebSocket,
    state: WorkerSocketState,
    provider: String,
    owner_key_id: Option<String>,
) {
    let Ok((worker_id, payload, notify)) =
        register_authenticated_worker(&mut socket, &state, provider, owner_key_id).await
    else {
        tracing::warn!("worker registration failed or timed out");
        close_socket(
            socket,
            CLOSE_CODE_PROTOCOL_ERROR,
            CLOSE_REASON_PROTOCOL_ERROR,
        )
        .await;
        return;
    };

    if send_ws_text(&mut socket, payload).await.is_err() {
        tracing::warn!(worker_id = %worker_id, "failed to send RegisterAck to worker");
        disconnect_worker(&state, &worker_id).await;
        return;
    }

    // Independent application-level JSON Ping schedule. Inbound transport-level
    // WS Ping/Pong frames must NOT delay or satisfy this schedule so half-open
    // or event-loop-wedged workers are reliably detected and reaped.
    let mut heartbeat_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + state.heartbeat_interval,
        state.heartbeat_interval,
    );
    heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut awaiting_pong_since: Option<tokio::time::Instant> = None;

    loop {
        if flush_all_pending(&mut socket, &state, &worker_id)
            .await
            .is_err()
        {
            tracing::warn!(worker_id = %worker_id, "worker websocket write failed; disconnecting");
            disconnect_worker(&state, &worker_id).await;
            return;
        }

        if let Some((code, reason)) = graceful_shutdown_close_action(&state, &worker_id).await {
            tracing::info!(
                worker_id = %worker_id,
                reason = %reason,
                "closing worker websocket after graceful shutdown"
            );
            close_socket(socket, code, reason).await;
            return;
        }

        let pong_deadline =
            awaiting_pong_since.map(|sent_at| sent_at + state.heartbeat_pong_timeout);
        let drain_deadline = worker_drain_deadline(&state, &worker_id).await;
        let next_deadline = match (pong_deadline, drain_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        tokio::select! {
            () = notify.notified() => {
                // New work, cancel, models refresh, or graceful shutdown queued.
                // Loop back immediately to flush pending messages.
            }
            _ = heartbeat_tick.tick() => {
                if awaiting_pong_since.is_none() {
                    if send_heartbeat_ping(&mut socket).await.is_err() {
                        tracing::warn!(worker_id = %worker_id, "failed to send heartbeat ping");
                        disconnect_worker(&state, &worker_id).await;
                        return;
                    }
                    awaiting_pong_since = Some(tokio::time::Instant::now());
                }
            }
            () = sleep_until_optional(next_deadline) => {
                if awaiting_pong_since
                    .is_some_and(|sent_at| sent_at.elapsed() >= state.heartbeat_pong_timeout)
                {
                    tracing::warn!(
                        worker_id = %worker_id,
                        pong_timeout_ms = state.heartbeat_pong_timeout.as_millis(),
                        "worker heartbeat pong timed out; disconnecting"
                    );
                    disconnect_worker(&state, &worker_id).await;
                    close_socket(socket, CLOSE_CODE_NORMAL, CLOSE_REASON_STALE_HEARTBEAT).await;
                    return;
                }
                // Otherwise a graceful shutdown deadline elapsed; top of loop handles it.
            }
            maybe_msg = socket.recv() => {
                match maybe_msg {
                    Some(Ok(Message::Text(payload))) => {
                        match handle_worker_message(&state, &worker_id, &payload).await {
                            WorkerMessageOutcome::Ok { is_pong } => {
                                if is_pong {
                                    awaiting_pong_since = None;
                                }
                            }
                            WorkerMessageOutcome::ProtocolError => {
                                tracing::warn!(
                                    worker_id = %worker_id,
                                    "worker sent invalid protocol message; disconnecting"
                                );
                                disconnect_worker(&state, &worker_id).await;
                                close_socket(
                                    socket,
                                    CLOSE_CODE_PROTOCOL_ERROR,
                                    CLOSE_REASON_PROTOCOL_ERROR,
                                )
                                .await;
                                return;
                            }
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        tracing::info!(
                            worker_id = %worker_id,
                            close_frame = ?frame,
                            "worker closed websocket"
                        );
                        disconnect_worker(&state, &worker_id).await;
                        return;
                    }
                    Some(Err(error)) => {
                        tracing::warn!(
                            worker_id = %worker_id,
                            error = %error,
                            "worker websocket read error"
                        );
                        disconnect_worker(&state, &worker_id).await;
                        return;
                    }
                    None => {
                        tracing::info!(worker_id = %worker_id, "worker websocket stream ended");
                        disconnect_worker(&state, &worker_id).await;
                        return;
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                        // Transport keepalive frames do not satisfy JSON heartbeat.
                    }
                    Some(Ok(Message::Binary(_))) => {
                        tracing::warn!(
                            worker_id = %worker_id,
                            "worker sent unexpected binary frame; disconnecting"
                        );
                        disconnect_worker(&state, &worker_id).await;
                        close_socket(
                            socket,
                            CLOSE_CODE_PROTOCOL_ERROR,
                            CLOSE_REASON_PROTOCOL_ERROR,
                        )
                        .await;
                        return;
                    }
                }
            }
        }
    }
}

async fn sleep_until_optional(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(instant) => tokio::time::sleep_until(instant).await,
        None => std::future::pending().await,
    }
}

async fn worker_drain_deadline(
    state: &WorkerSocketState,
    worker_id: &str,
) -> Option<tokio::time::Instant> {
    let deadline = {
        let core = state.core.lock().await;
        core.worker_graceful_shutdown_deadline(worker_id)
    }?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    Some(tokio::time::Instant::now() + remaining)
}

async fn flush_all_pending(
    socket: &mut WebSocket,
    state: &WorkerSocketState,
    worker_id: &str,
) -> Result<(), ()> {
    flush_pending_requests(socket, state, worker_id).await?;
    flush_pending_graceful_shutdowns(socket, state, worker_id).await?;
    flush_pending_models_refreshes(socket, state, worker_id).await?;
    Ok(())
}

async fn register_authenticated_worker(
    socket: &mut WebSocket,
    state: &WorkerSocketState,
    provider: String,
    owner_key_id: Option<String>,
) -> Result<(String, String, Arc<Notify>), ()> {
    let deadline = tokio::time::Instant::now() + REGISTRATION_TIMEOUT;
    let payload = loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(());
        }
        match timeout(remaining, socket.recv()).await {
            Ok(Some(Ok(Message::Text(payload)))) => break payload,
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {}
            _ => return Err(()),
        }
    };

    let Ok(WorkerToServerMessage::Register(register)) = serde_json::from_str(&payload) else {
        return Err(());
    };
    if let Some(protocol_version) = register.protocol_version.as_deref()
        && protocol_version != WORKER_PROTOCOL_VERSION
    {
        return Err(());
    }

    let notify = Arc::new(Notify::new());
    let registered = {
        let mut core = state.core.lock().await;
        let registered = core.register_worker_for_owner(provider, register, owner_key_id);
        core.register_worker_notifier(&registered.worker_id, Arc::clone(&notify));
        registered
    };
    let worker_id = registered.worker_id;
    let payload = serde_json::to_string(&ServerToWorkerMessage::RegisterAck(registered.ack))
        .map_err(|_| ())?;

    Ok((worker_id, payload, notify))
}

async fn disconnect_worker(state: &WorkerSocketState, worker_id: &str) {
    let mut core = state.core.lock().await;
    let _ = core.disconnect_worker(worker_id);
}

enum WorkerMessageOutcome {
    Ok { is_pong: bool },
    ProtocolError,
}

async fn handle_worker_message(
    state: &WorkerSocketState,
    worker_id: &str,
    payload: &str,
) -> WorkerMessageOutcome {
    match serde_json::from_str(payload) {
        Ok(WorkerToServerMessage::ModelsUpdate(ModelsUpdateMessage {
            models,
            current_load,
            model_metadata,
        })) => {
            let mut core = state.core.lock().await;
            if !core.has_worker(worker_id) {
                return WorkerMessageOutcome::ProtocolError;
            }
            let _ = core.update_worker_models(
                worker_id,
                ModelsUpdateMessage {
                    models,
                    current_load,
                    model_metadata,
                },
            );
            WorkerMessageOutcome::Ok { is_pong: false }
        }
        Ok(WorkerToServerMessage::Pong(PongMessage {
            current_load,
            timestamp_unix_ms,
        })) => {
            let mut core = state.core.lock().await;
            if !core.has_worker(worker_id) {
                return WorkerMessageOutcome::ProtocolError;
            }
            let _ = core.record_worker_pong(
                worker_id,
                &PongMessage {
                    current_load,
                    timestamp_unix_ms,
                },
            );
            WorkerMessageOutcome::Ok { is_pong: true }
        }
        Ok(WorkerToServerMessage::ResponseChunk(ResponseChunkMessage { request_id, chunk })) => {
            let sender = {
                let mut core = state.core.lock().await;
                let Some(sender) = core.prepare_stream_chunk_sender(worker_id, &request_id) else {
                    return WorkerMessageOutcome::ProtocolError;
                };
                sender
            };
            if let Some(tx) = sender {
                if let Ok(Ok(()) | Err(_)) = timeout(
                    STREAM_CHUNK_SEND_TIMEOUT,
                    tx.send(HttpResponseEvent::Chunk(chunk)),
                )
                .await
                {
                    // Delivered, or HTTP client already disconnected (drop guard cancels).
                } else {
                    tracing::warn!(
                        worker_id = %worker_id,
                        request_id = %request_id,
                        "streaming HTTP client stalled reading chunks; cancelling request"
                    );
                    let mut core = state.core.lock().await;
                    let _ =
                        core.cancel_request(&request_id, crate::CancelReason::ClientDisconnected);
                }
            }
            WorkerMessageOutcome::Ok { is_pong: false }
        }
        Ok(WorkerToServerMessage::ResponseComplete(response)) => {
            let mut core = state.core.lock().await;
            if core.complete_http_response(worker_id, response) {
                WorkerMessageOutcome::Ok { is_pong: false }
            } else {
                WorkerMessageOutcome::ProtocolError
            }
        }
        Ok(WorkerToServerMessage::Error(error_msg)) => {
            let mut core = state.core.lock().await;
            if core.handle_worker_error(
                worker_id,
                error_msg.request_id.as_deref(),
                &error_msg.code,
                &error_msg.message,
            ) {
                WorkerMessageOutcome::Ok { is_pong: false }
            } else {
                WorkerMessageOutcome::ProtocolError
            }
        }
        _ => WorkerMessageOutcome::ProtocolError,
    }
}

async fn flush_pending_requests(
    socket: &mut WebSocket,
    state: &WorkerSocketState,
    worker_id: &str,
) -> Result<(), ()> {
    flush_pending_cancels(socket, state, worker_id).await?;

    let requests = {
        let mut core = state.core.lock().await;
        core.take_pending_worker_requests(worker_id)
    };

    for request in requests {
        let payload =
            serde_json::to_string(&ServerToWorkerMessage::Request(request)).map_err(|_| ())?;
        send_ws_text(socket, payload).await?;
    }

    Ok(())
}

async fn flush_pending_graceful_shutdowns(
    socket: &mut WebSocket,
    state: &WorkerSocketState,
    worker_id: &str,
) -> Result<(), ()> {
    let shutdowns = {
        let mut core = state.core.lock().await;
        core.take_pending_worker_graceful_shutdown_signals(worker_id)
    };

    for shutdown in shutdowns {
        let payload =
            serde_json::to_string(&graceful_shutdown_message(shutdown)).map_err(|_| ())?;
        send_ws_text(socket, payload).await?;
    }

    Ok(())
}

async fn flush_pending_models_refreshes(
    socket: &mut WebSocket,
    state: &WorkerSocketState,
    worker_id: &str,
) -> Result<(), ()> {
    let refreshes = {
        let mut core = state.core.lock().await;
        core.take_pending_worker_models_refresh_signals(worker_id)
    };

    for refresh in refreshes {
        let payload = serde_json::to_string(&models_refresh_message(refresh)).map_err(|_| ())?;
        send_ws_text(socket, payload).await?;
    }

    Ok(())
}

async fn flush_pending_cancels(
    socket: &mut WebSocket,
    state: &WorkerSocketState,
    worker_id: &str,
) -> Result<(), ()> {
    let cancels = {
        let mut core = state.core.lock().await;
        core.take_pending_worker_cancel_signals(worker_id)
    };

    for cancel in cancels {
        let payload = serde_json::to_string(&cancel_message(cancel)).map_err(|_| ())?;
        send_ws_text(socket, payload).await?;
    }

    Ok(())
}

fn cancel_message(cancel: WorkerCancelSignal) -> ServerToWorkerMessage {
    ServerToWorkerMessage::Cancel(CancelMessage {
        request_id: cancel.request_id,
        reason: match cancel.reason {
            crate::CancelReason::ClientDisconnected => CancelReason::ClientDisconnect,
            crate::CancelReason::RequestTimedOut => CancelReason::Timeout,
        },
    })
}

fn graceful_shutdown_message(shutdown: GracefulShutdownSignal) -> ServerToWorkerMessage {
    ServerToWorkerMessage::GracefulShutdown(GracefulShutdownMessage {
        reason: shutdown.reason,
        drain_timeout_secs: Some(shutdown.drain_timeout.as_secs().max(1)),
    })
}

fn models_refresh_message(refresh: WorkerModelsRefreshSignal) -> ServerToWorkerMessage {
    ServerToWorkerMessage::ModelsRefresh(modelrelay_protocol::ModelsRefreshMessage {
        reason: refresh.reason,
    })
}

async fn graceful_shutdown_close_action(
    state: &WorkerSocketState,
    worker_id: &str,
) -> Option<(u16, &'static str)> {
    let mut core = state.core.lock().await;
    let outcome = core.expire_graceful_shutdown(Instant::now());
    if outcome
        .disconnected_worker_ids
        .iter()
        .any(|disconnected_worker_id| disconnected_worker_id == worker_id)
    {
        let _ = core.take_graceful_shutdown_disconnect_reason(worker_id);
        return Some((CLOSE_CODE_NORMAL, CLOSE_REASON_GRACEFUL_SHUTDOWN_TIMED_OUT));
    }

    if core.disconnect_drained_worker_if_idle(worker_id) {
        let _ = core.take_graceful_shutdown_disconnect_reason(worker_id);
        return Some((CLOSE_CODE_NORMAL, CLOSE_REASON_GRACEFUL_SHUTDOWN_COMPLETE));
    }

    match core.take_graceful_shutdown_disconnect_reason(worker_id) {
        Some(GracefulShutdownDisconnectReason::Completed) => {
            Some((CLOSE_CODE_NORMAL, CLOSE_REASON_GRACEFUL_SHUTDOWN_COMPLETE))
        }
        Some(GracefulShutdownDisconnectReason::TimedOut) => {
            Some((CLOSE_CODE_NORMAL, CLOSE_REASON_GRACEFUL_SHUTDOWN_TIMED_OUT))
        }
        None => None,
    }
}

async fn send_heartbeat_ping(socket: &mut WebSocket) -> Result<(), ()> {
    let payload = serde_json::to_string(&ServerToWorkerMessage::Ping(PingMessage {
        timestamp_unix_ms: None,
    }))
    .map_err(|_| ())?;
    send_ws_text(socket, payload).await
}

async fn send_ws_text(socket: &mut WebSocket, payload: String) -> Result<(), ()> {
    match timeout(
        SOCKET_SEND_TIMEOUT,
        socket.send(Message::Text(payload.into())),
    )
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) | Err(_) => Err(()),
    }
}

async fn close_socket(mut socket: WebSocket, code: u16, reason: &'static str) {
    let _ = timeout(
        SOCKET_CLOSE_TIMEOUT,
        socket.send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        }))),
    )
    .await;
}
