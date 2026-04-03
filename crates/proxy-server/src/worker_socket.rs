use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration as StdDuration, Instant},
};

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    routing::get,
};
use serde::Deserialize;
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tokio::time::{Duration, timeout};
use worker_protocol::{
    CancelMessage, CancelReason, GracefulShutdownMessage, ModelsUpdateMessage, PingMessage,
    PongMessage, ResponseChunkMessage, ServerToWorkerMessage, WorkerToServerMessage,
};

use crate::{
    GracefulShutdownDisconnectReason, GracefulShutdownSignal, ProxyServerCore, WorkerCancelSignal,
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
const AUTH_RATE_LIMIT_COOLDOWN: StdDuration = StdDuration::from_millis(250);
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);
const HEARTBEAT_PONG_TIMEOUT: Duration = Duration::from_millis(300);

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
                failed_auth_by_client: Arc::new(Mutex::new(HashMap::new())),
            },
        }
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
    Authenticated { provider: String },
    Rejected { reason: &'static str },
}

#[derive(Debug, Clone)]
struct FailedAuthState {
    failures: u32,
    last_failed_at: Instant,
    blocked_until: Option<Instant>,
}

async fn worker_connect_handler(
    ws: WebSocketUpgrade,
    State(state): State<WorkerSocketState>,
    Query(query): Query<WorkerConnectQuery>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let client_identity = headers
        .get(axum::http::header::FORWARDED)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .or_else(|| {
            headers
                .get("x-forwarded-for")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "unknown-client".to_string());

    match authenticate_connection(&state, client_identity, &headers, &query).await {
        AuthOutcome::Authenticated { provider } => ws.on_upgrade(move |socket| async move {
            handle_authenticated_socket(socket, state, provider).await;
        }),
        AuthOutcome::Rejected { reason } => ws.on_upgrade(move |socket| async move {
            close_socket(socket, CLOSE_CODE_POLICY_VIOLATION, reason).await;
        }),
    }
}

async fn authenticate_connection(
    state: &WorkerSocketState,
    client_identity: String,
    headers: &axum::http::HeaderMap,
    query: &WorkerConnectQuery,
) -> AuthOutcome {
    if client_is_rate_limited(state, &client_identity).await {
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_RATE_LIMITED,
        };
    }

    let Some(provider) = query.provider.as_deref() else {
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_FAILED,
        };
    };

    let Some(config) = state.providers.get(provider) else {
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_FAILED,
        };
    };

    if !config.enabled {
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_FAILED,
        };
    }

    let presented_secret = headers
        .get(WORKER_SECRET_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
        .or_else(|| query.worker_secret.clone());

    let Some(presented_secret) = presented_secret else {
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_FAILED,
        };
    };

    let secret_matches: bool = presented_secret
        .as_bytes()
        .ct_eq(config.worker_secret.as_bytes())
        .into();
    if !secret_matches {
        record_failed_auth(state, client_identity).await;
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_FAILED,
        };
    }

    clear_failed_auth(state, &client_identity).await;

    AuthOutcome::Authenticated {
        provider: provider.to_string(),
    }
}

async fn client_is_rate_limited(state: &WorkerSocketState, client_identity: &str) -> bool {
    let now = Instant::now();
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
                .is_some_and(|entry| {
                    now.duration_since(entry.last_failed_at) > AUTH_RATE_LIMIT_COOLDOWN
                })
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
    let mut failed_auth_by_client = state.failed_auth_by_client.lock().await;
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
        || now.duration_since(entry.last_failed_at) > AUTH_RATE_LIMIT_COOLDOWN
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
        entry.blocked_until = Some(now + AUTH_RATE_LIMIT_COOLDOWN);
    }
}

async fn clear_failed_auth(state: &WorkerSocketState, client_identity: &str) {
    let mut failed_auth_by_client = state.failed_auth_by_client.lock().await;
    failed_auth_by_client.remove(client_identity);
}

async fn handle_authenticated_socket(
    mut socket: WebSocket,
    state: WorkerSocketState,
    provider: String,
) {
    let mut last_heartbeat_activity_at = tokio::time::Instant::now();
    let mut awaiting_pong_since: Option<tokio::time::Instant> = None;

    let Ok((worker_id, payload)) =
        register_authenticated_worker(&mut socket, &state, provider).await
    else {
        close_socket(
            socket,
            CLOSE_CODE_PROTOCOL_ERROR,
            CLOSE_REASON_PROTOCOL_ERROR,
        )
        .await;
        return;
    };

    if socket.send(Message::Text(payload.into())).await.is_err() {
        disconnect_worker(&state, &worker_id).await;
        return;
    }

    loop {
        if flush_pending_requests(&mut socket, &state, &worker_id)
            .await
            .is_err()
        {
            disconnect_worker(&state, &worker_id).await;
            return;
        }

        if flush_pending_graceful_shutdowns(&mut socket, &state, &worker_id)
            .await
            .is_err()
        {
            disconnect_worker(&state, &worker_id).await;
            return;
        }

        if let Some((code, reason)) = graceful_shutdown_close_action(&state, &worker_id).await {
            close_socket(socket, code, reason).await;
            return;
        }

        match timeout(Duration::from_millis(25), socket.recv()).await {
            Ok(Some(Ok(Message::Text(payload)))) => {
                last_heartbeat_activity_at = tokio::time::Instant::now();
                awaiting_pong_since = None;
                if !handle_worker_message(&state, &worker_id, &payload).await {
                    close_socket(
                        socket,
                        CLOSE_CODE_PROTOCOL_ERROR,
                        CLOSE_REASON_PROTOCOL_ERROR,
                    )
                    .await;
                    disconnect_worker(&state, &worker_id).await;
                    return;
                }
            }
            Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => {
                disconnect_worker(&state, &worker_id).await;
                return;
            }
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {
                last_heartbeat_activity_at = tokio::time::Instant::now();
                awaiting_pong_since = None;
            }
            Err(_) => {
                if awaiting_pong_since
                    .is_some_and(|sent_at| sent_at.elapsed() >= HEARTBEAT_PONG_TIMEOUT)
                {
                    disconnect_worker(&state, &worker_id).await;
                    close_socket(socket, CLOSE_CODE_NORMAL, CLOSE_REASON_STALE_HEARTBEAT).await;
                    return;
                }

                if awaiting_pong_since.is_none()
                    && last_heartbeat_activity_at.elapsed() >= HEARTBEAT_INTERVAL
                {
                    if send_heartbeat_ping(&mut socket).await.is_err() {
                        disconnect_worker(&state, &worker_id).await;
                        return;
                    }
                    awaiting_pong_since = Some(tokio::time::Instant::now());
                }
            }
            Ok(Some(Ok(Message::Binary(_)))) => {
                close_socket(
                    socket,
                    CLOSE_CODE_PROTOCOL_ERROR,
                    CLOSE_REASON_PROTOCOL_ERROR,
                )
                .await;
                disconnect_worker(&state, &worker_id).await;
                return;
            }
        }
    }
}

async fn register_authenticated_worker(
    socket: &mut WebSocket,
    state: &WorkerSocketState,
    provider: String,
) -> Result<(String, String), ()> {
    let Some(Ok(Message::Text(payload))) = socket.recv().await else {
        return Err(());
    };
    let Ok(WorkerToServerMessage::Register(register)) = serde_json::from_str(&payload) else {
        return Err(());
    };
    if let Some(protocol_version) = register.protocol_version.as_deref()
        && protocol_version != WORKER_PROTOCOL_VERSION
    {
        return Err(());
    }

    let registered = {
        let mut core = state.core.lock().await;
        core.register_worker(provider, register)
    };
    let worker_id = registered.worker_id;
    let payload = serde_json::to_string(&ServerToWorkerMessage::RegisterAck(registered.ack))
        .map_err(|_| ())?;

    Ok((worker_id, payload))
}

async fn disconnect_worker(state: &WorkerSocketState, worker_id: &str) {
    let mut core = state.core.lock().await;
    let _ = core.disconnect_worker(worker_id);
}

async fn handle_worker_message(state: &WorkerSocketState, worker_id: &str, payload: &str) -> bool {
    match serde_json::from_str(payload) {
        Ok(WorkerToServerMessage::ModelsUpdate(ModelsUpdateMessage {
            models,
            current_load,
        })) => {
            let mut core = state.core.lock().await;
            if !core.has_worker(worker_id) {
                return false;
            }
            let _ = core.update_worker_models(
                worker_id,
                ModelsUpdateMessage {
                    models,
                    current_load,
                },
            );
            true
        }
        Ok(WorkerToServerMessage::Pong(PongMessage {
            current_load,
            timestamp_unix_ms,
        })) => {
            let mut core = state.core.lock().await;
            if !core.has_worker(worker_id) {
                return false;
            }
            let _ = core.record_worker_pong(
                worker_id,
                &PongMessage {
                    current_load,
                    timestamp_unix_ms,
                },
            );
            true
        }
        Ok(WorkerToServerMessage::ResponseChunk(ResponseChunkMessage { request_id, chunk })) => {
            let mut core = state.core.lock().await;
            core.stream_http_response_chunk(worker_id, &request_id, chunk)
        }
        Ok(WorkerToServerMessage::ResponseComplete(response)) => {
            let request_id = response.request_id.clone();
            let mut core = state.core.lock().await;
            core.complete_http_response(worker_id, response)
                || core.finish_request(worker_id, &request_id).is_some()
                || core.request_state(&request_id).is_none()
        }
        _ => false,
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
        socket
            .send(Message::Text(payload.into()))
            .await
            .map_err(|_| ())?;
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
        socket
            .send(Message::Text(payload.into()))
            .await
            .map_err(|_| ())?;
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
        socket
            .send(Message::Text(payload.into()))
            .await
            .map_err(|_| ())?;
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

async fn graceful_shutdown_close_action(
    state: &WorkerSocketState,
    worker_id: &str,
) -> Option<(u16, &'static str)> {
    let mut core = state.core.lock().await;
    let outcome = core.expire_graceful_shutdown(std::time::Instant::now());
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
    socket
        .send(Message::Text(payload.into()))
        .await
        .map_err(|_| ())
}

async fn close_socket(mut socket: WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}
