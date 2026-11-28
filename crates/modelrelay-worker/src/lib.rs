use std::{
    collections::{HashMap, HashSet},
    error::Error,
    io,
    sync::Arc,
    time::SystemTime,
};

use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use modelrelay_protocol::{
    ErrorMessage, ModelMetadataMap, ModelsUpdateMessage, PongMessage, RegisterMessage,
    RequestMessage, ResponseChunkMessage, ResponseCompleteMessage, ServerToWorkerMessage,
    TokenCounts, WorkerToServerMessage, parse_openai_models_response,
};
use reqwest::header::{
    ACCEPT_ENCODING, CONNECTION, CONTENT_LENGTH, HOST, HeaderMap as ReqwestHeaderMap, HeaderName,
    PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use serde::{Deserialize, Serialize};
use tokio::{
    select,
    sync::{Notify, mpsc, watch},
    task::JoinHandle,
    time::{Duration, Instant, sleep, timeout},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Bytes, Message,
        client::IntoClientRequest,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};

/// How long the worker waits without receiving any message from the server
/// before assuming the connection is dead and reconnecting.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// How often the worker sends a protocol-level WebSocket Ping frame to the
/// server to detect silently-half-open TCP connections through middleboxes.
const CLIENT_PING_INTERVAL: Duration = Duration::from_secs(15);

/// Maximum time to establish the WebSocket handshake to the proxy server.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Maximum time to wait for `RegisterAck` after sending `Register`.
const REGISTER_ACK_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum time allowed for a single WebSocket write before assuming the socket
/// is wedged and reconnecting.
const WS_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum time to wait for a clean WebSocket close frame on local shutdown.
const WS_CLOSE_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum time to drain in-flight requests after receiving a local shutdown signal.
const LOCAL_SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum time to wait for the next chunk from the local backend during streaming.
const STREAM_CHUNK_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Maximum time to wait for backend `/v1/models` discovery.
const MODEL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Capacity of the bounded internal event channel between request tasks and the
/// WebSocket writer loop.
const EVENT_CHANNEL_CAPACITY: usize = 128;

/// WebSocket policy violation close code returned by the proxy on auth failure.
const CLOSE_CODE_POLICY_VIOLATION: u16 = 1008;

type BoxError = Box<dyn Error + Send + Sync>;
type WebSocketStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WebSocketWrite = SplitSink<WebSocketStream, Message>;

/// Live connection phase reported by [`WorkerDaemon`] over a `watch` channel.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
    AuthFailed,
}

/// Snapshot of live [`WorkerDaemon`] telemetry for desktop/CLI UIs.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerStatusSnapshot {
    pub state: WorkerConnectionState,
    pub active_requests: u32,
    pub models: Vec<String>,
    pub last_error: Option<String>,
    pub worker_id: Option<String>,
}

enum DaemonEvent {
    Outbound(WorkerToServerMessage),
    RequestCompleted(ResponseCompleteMessage),
    RequestFailed {
        request_id: String,
        error: String,
    },
    ModelsDiscovered {
        models: Vec<String>,
        metadata: ModelMetadataMap,
        force_send: bool,
    },
}

enum SessionOutcome {
    /// Proxy initiated graceful shutdown (e.g. rolling deploy); reconnect after a short delay.
    ProxyGracefulShutdown,
    /// Local shutdown signal fired; exit cleanly without reconnecting.
    LocalShutdown,
    /// Server rejected authentication (WS close 1008); surface error and back off.
    AuthRejected { reason: String, registered: bool },
    /// Unexpected disconnect or timeout; reconnect with backoff.
    Disconnected {
        reason: Option<String>,
        registered: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerDaemonConfig {
    pub proxy_base_url: String,
    pub provider: String,
    pub worker_secret: String,
    pub worker_name: String,
    pub models: Vec<String>,
    pub max_concurrent: u32,
    pub backend_base_url: String,
    /// API endpoint path prefixes this worker supports (e.g. `["/v1/chat", "/v1/messages"]`).
    /// An empty list means the worker accepts any endpoint (legacy behavior).
    pub endpoint_prefixes: Vec<String>,
}

impl WorkerDaemonConfig {
    /// If the configured models list contains `"*"`, queries the backend's
    /// `/v1/models` endpoint and replaces the wildcard with the actual model
    /// IDs reported by the backend. Falls back to the original list when the
    /// backend does not support the endpoint or returns an error.
    pub async fn resolve_wildcard_models(&mut self) {
        if !self.models.iter().any(|m| m.trim() == "*") {
            return;
        }

        let client = build_http_client();
        match discover_backend_models(&client, self).await {
            Ok((discovered, _)) if !discovered.is_empty() => {
                tracing::info!(
                    count = discovered.len(),
                    models = %discovered.join(", "),
                    "discovered models from backend /v1/models"
                );
                self.models = discovered;
            }
            Ok(_) => {
                tracing::warn!("backend /v1/models returned no models, keeping wildcard");
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "failed to query backend /v1/models, keeping wildcard"
                );
            }
        }
    }

    #[must_use]
    pub fn websocket_url(&self) -> String {
        let base = self.proxy_base_url.trim().trim_end_matches('/');
        let scheme = if let Some(stripped) = base.strip_prefix("http://") {
            format!("ws://{stripped}")
        } else if let Some(stripped) = base.strip_prefix("https://") {
            format!("wss://{stripped}")
        } else {
            base.to_string()
        };

        let encoded_provider = percent_encode_query_component(self.provider.trim());
        format!("{scheme}/v1/worker/connect?provider={encoded_provider}")
    }

    #[must_use]
    pub fn backend_url(&self, endpoint_path: &str) -> String {
        let base = self.backend_base_url.trim().trim_end_matches('/');
        // Avoid doubling `/v1` when backend_base_url already ends with `/v1`
        // (e.g. `http://localhost:11434/v1` + `/v1/chat/completions`), and route
        // backend-native root metadata endpoints (`/props` for llama.cpp,
        // `/api/*` for LM Studio & Ollama) against the server root.
        if let Some(root) = base.strip_suffix("/v1") {
            if endpoint_path == "/v1" || endpoint_path.starts_with("/v1/") {
                return format!("{base}{}", &endpoint_path[3..]);
            }
            if endpoint_path == "/props"
                || endpoint_path.starts_with("/props?")
                || endpoint_path == "/models"
                || endpoint_path.starts_with("/api/")
            {
                return format!("{root}{endpoint_path}");
            }
        }
        format!("{base}{endpoint_path}")
    }
}

/// Build the shared `reqwest::Client` with connection timeouts and TCP keepalive.
fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .tcp_keepalive(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// RFC 3986 percent-encoding for URL query parameter values.
fn percent_encode_query_component(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.bytes() {
        let is_unreserved =
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if is_unreserved {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Incremental UTF-8 decoder that buffers up to 3 incomplete trailing bytes
/// across network chunk boundaries so multi-byte characters (em-dash, CJK,
/// emoji) split across TCP segments never abort the stream.
struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    fn new() -> Self {
        Self {
            pending: Vec::with_capacity(8),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> String {
        if bytes.is_empty() && self.pending.is_empty() {
            return String::new();
        }
        self.pending.extend_from_slice(bytes);
        let mut out = String::with_capacity(self.pending.len());

        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    out.push_str(valid);
                    self.pending.clear();
                    break;
                }
                Err(err) => {
                    let valid_up_to = err.valid_up_to();
                    if valid_up_to > 0 {
                        // SAFETY: `valid_up_to` is guaranteed to be valid UTF-8.
                        if let Ok(prefix) = std::str::from_utf8(&self.pending[..valid_up_to]) {
                            out.push_str(prefix);
                        }
                    }
                    if let Some(invalid_len) = err.error_len() {
                        // Truly invalid UTF-8 bytes — replace with U+FFFD and continue.
                        out.push('\u{FFFD}');
                        self.pending.drain(..valid_up_to + invalid_len);
                    } else {
                        // Incomplete multi-byte sequence at the end of the chunk;
                        // retain trailing bytes until the next chunk arrives.
                        if valid_up_to > 0 {
                            self.pending.drain(..valid_up_to);
                        }
                        break;
                    }
                }
            }
        }

        out
    }

    fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let remainder = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        Some(remainder)
    }
}

pub struct WorkerDaemon {
    config: WorkerDaemonConfig,
    client: reqwest::Client,
    status_tx: Option<watch::Sender<WorkerStatusSnapshot>>,
    shutdown_rx: Option<watch::Receiver<bool>>,
    models_poll_interval: Option<Duration>,
    wildcard_discovery: bool,
    models_refresh_notify: Option<Arc<Notify>>,
}

impl WorkerDaemon {
    #[must_use]
    pub fn new(config: WorkerDaemonConfig) -> Self {
        let wildcard_discovery = config.models.iter().any(|m| m.trim() == "*");
        Self {
            config,
            client: build_http_client(),
            status_tx: None,
            shutdown_rx: None,
            models_poll_interval: None,
            wildcard_discovery,
            models_refresh_notify: None,
        }
    }

    /// Attach a `watch::Sender` to receive live connection, load, model, and error updates.
    #[must_use]
    pub fn with_status_sender(mut self, tx: watch::Sender<WorkerStatusSnapshot>) -> Self {
        self.status_tx = Some(tx);
        self
    }

    /// Attach a `watch::Receiver<bool>` that triggers graceful drain and exit when `true`.
    #[must_use]
    pub fn with_shutdown_rx(mut self, rx: watch::Receiver<bool>) -> Self {
        self.shutdown_rx = Some(rx);
        self
    }

    /// Periodically re-query backend `/v1/models` when wildcard discovery is enabled.
    #[must_use]
    pub fn with_models_poll_interval(mut self, interval: Option<Duration>) -> Self {
        self.models_poll_interval = interval;
        self
    }

    /// Explicitly control whether wildcard backend `/v1/models` discovery is active
    /// (useful when `resolve_wildcard_models` was already called before `WorkerDaemon::new`).
    #[must_use]
    pub fn with_wildcard_discovery(mut self, enabled: bool) -> Self {
        self.wildcard_discovery = enabled;
        self
    }

    /// Attach a notifier that triggers an immediate backend `/v1/models` refresh on demand.
    #[must_use]
    pub fn with_models_refresh_notify(mut self, notify: Arc<Notify>) -> Self {
        self.models_refresh_notify = Some(notify);
        self
    }

    fn update_status(&self, f: impl FnOnce(&mut WorkerStatusSnapshot)) {
        if let Some(ref tx) = self.status_tx {
            tx.send_modify(f);
        }
    }

    fn is_local_shutdown(&self) -> bool {
        self.shutdown_rx.as_ref().is_some_and(|rx| *rx.borrow())
    }

    fn registration_message(
        &self,
        models: &[String],
        model_metadata: &ModelMetadataMap,
    ) -> WorkerToServerMessage {
        WorkerToServerMessage::Register(RegisterMessage {
            worker_name: self.config.worker_name.trim().to_string(),
            models: models.to_vec(),
            max_concurrent: self.config.max_concurrent.max(1),
            protocol_version: Some("2026-04-bridge-v1".to_string()),
            current_load: Some(0),
            endpoint_prefixes: self.config.endpoint_prefixes.clone(),
            model_metadata: model_metadata.clone(),
        })
    }

    /// Runs one connection session.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot construct the request, connect,
    /// complete registration, or write to the WebSocket.
    #[allow(clippy::too_many_lines)]
    async fn run_session(&self) -> Result<SessionOutcome, BoxError> {
        if self.is_local_shutdown() {
            return Ok(SessionOutcome::LocalShutdown);
        }

        // If wildcard discovery is active, refresh both the model list and per-model
        // metadata from the local backend before registering (falls back to configured
        // models on error). Explicit-model workers skip startup probing so they connect
        // immediately without sending unsolicited requests to the backend.
        let mut current_models = self.config.models.clone();
        let mut current_metadata = ModelMetadataMap::new();
        if self.wildcard_discovery
            && let Ok((discovered, metadata)) =
                discover_backend_models(&self.client, &self.config).await
            && !discovered.is_empty()
        {
            current_models = discovered;
            current_metadata = metadata;
        }

        self.update_status(|s| {
            s.state = WorkerConnectionState::Connecting;
            s.active_requests = 0;
            s.models.clone_from(&current_models);
        });

        let mut request = self.config.websocket_url().into_client_request()?;
        request
            .headers_mut()
            .insert("x-worker-secret", self.config.worker_secret.trim().parse()?);

        let (socket, _) = timeout(WS_CONNECT_TIMEOUT, connect_async(request))
            .await
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "WebSocket connect timed out after {}s",
                        WS_CONNECT_TIMEOUT.as_secs()
                    ),
                )
            })??;

        let (mut socket_write, mut socket_read) = socket.split();

        send_worker_message(
            &mut socket_write,
            &self.registration_message(&current_models, &current_metadata),
        )
        .await?;

        // Wait for RegisterAck before considering the connection healthy or resetting backoff.
        let ack_deadline = Instant::now() + REGISTER_ACK_TIMEOUT;
        let mut buffered_server_messages: Vec<ServerToWorkerMessage> = Vec::new();
        let ack = loop {
            let remaining = ack_deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for RegisterAck from proxy",
                )));
            }
            let maybe_msg = timeout(remaining, socket_read.next()).await.map_err(|_| {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for RegisterAck from proxy",
                )
            })?;
            let Some(msg_result) = maybe_msg else {
                return Ok(SessionOutcome::Disconnected {
                    reason: Some("proxy closed connection before RegisterAck".to_string()),
                    registered: false,
                });
            };
            match msg_result? {
                Message::Text(payload) => {
                    let server_msg = serde_json::from_str::<ServerToWorkerMessage>(&payload)?;
                    if let ServerToWorkerMessage::RegisterAck(ack) = server_msg {
                        break ack;
                    }
                    buffered_server_messages.push(server_msg);
                }
                Message::Close(frame) => {
                    return Ok(classify_close_frame(frame.as_ref(), false));
                }
                Message::Ping(payload) => {
                    send_ws_frame(&mut socket_write, Message::Pong(payload)).await?;
                }
                Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        };

        if !ack.warnings.is_empty() {
            tracing::warn!(
                worker_id = %ack.worker_id,
                warnings = ?ack.warnings,
                "proxy reported registration warnings"
            );
        }
        if !ack.models.is_empty() {
            current_models.clone_from(&ack.models);
        }

        self.update_status(|s| {
            s.state = WorkerConnectionState::Connected;
            s.active_requests = 0;
            s.models.clone_from(&current_models);
            s.worker_id = Some(ack.worker_id.clone());
            s.last_error = None;
        });

        tracing::info!(
            url = %self.config.websocket_url(),
            worker_id = %ack.worker_id,
            models = %current_models.join(", "),
            "connected to proxy and received RegisterAck"
        );

        let (event_tx, mut event_rx) = mpsc::channel::<DaemonEvent>(EVENT_CHANNEL_CAPACITY);
        let mut active_requests = HashMap::<String, JoinHandle<()>>::new();
        let mut shutting_down = false;
        let mut local_shutdown = false;
        let mut local_shutdown_deadline: Option<Instant> = None;
        let mut last_server_activity = Instant::now();
        let mut shutdown_rx = self.shutdown_rx.clone();

        for msg in buffered_server_messages {
            if !self
                .handle_server_message(
                    &mut socket_write,
                    &event_tx,
                    &mut active_requests,
                    &mut shutting_down,
                    msg,
                )
                .await?
            {
                return Ok(SessionOutcome::ProxyGracefulShutdown);
            }
        }

        let mut ping_ticker = tokio::time::interval(CLIENT_PING_INTERVAL);
        ping_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping_ticker.tick().await;

        let mut models_poll_ticker = self
            .models_poll_interval
            .filter(|_| self.wildcard_discovery)
            .map(|dur| {
                let mut ticker = tokio::time::interval(dur.max(Duration::from_secs(5)));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                ticker
            });
        if let Some(ref mut ticker) = models_poll_ticker {
            ticker.tick().await;
        }

        let mut outcome = SessionOutcome::Disconnected {
            reason: None,
            registered: true,
        };

        loop {
            if local_shutdown && active_requests.is_empty() {
                outcome = SessionOutcome::LocalShutdown;
                break;
            }

            let idle_deadline = sleep(IDLE_TIMEOUT.saturating_sub(last_server_activity.elapsed()));

            select! {
                () = wait_for_local_shutdown(&mut shutdown_rx), if !local_shutdown => {
                    tracing::info!(
                        in_flight = active_requests.len(),
                        "local shutdown signal received; draining in-flight requests"
                    );
                    local_shutdown = true;
                    shutting_down = true;
                    local_shutdown_deadline = Some(Instant::now() + LOCAL_SHUTDOWN_DRAIN_TIMEOUT);
                    if active_requests.is_empty() {
                        outcome = SessionOutcome::LocalShutdown;
                        break;
                    }
                }
                () = sleep_until_optional(local_shutdown_deadline), if local_shutdown => {
                    tracing::warn!(
                        remaining = active_requests.len(),
                        "local shutdown drain timeout reached; aborting remaining requests"
                    );
                    outcome = SessionOutcome::LocalShutdown;
                    break;
                }
                maybe_message = socket_read.next() => {
                    let Some(message) = maybe_message else {
                        outcome = if shutting_down {
                            SessionOutcome::ProxyGracefulShutdown
                        } else {
                            SessionOutcome::Disconnected {
                                reason: Some("proxy closed websocket stream".to_string()),
                                registered: true,
                            }
                        };
                        break;
                    };
                    match message? {
                        Message::Text(payload) => {
                            last_server_activity = Instant::now();
                            let server_message = serde_json::from_str::<ServerToWorkerMessage>(&payload)?;
                            if !self.handle_server_message(
                                &mut socket_write,
                                &event_tx,
                                &mut active_requests,
                                &mut shutting_down,
                                server_message,
                            ).await? {
                                outcome = SessionOutcome::ProxyGracefulShutdown;
                                break;
                            }
                        }
                        Message::Close(frame) => {
                            if shutting_down {
                                outcome = SessionOutcome::ProxyGracefulShutdown;
                            } else {
                                outcome = classify_close_frame(frame.as_ref(), true);
                            }
                            break;
                        }
                        Message::Ping(payload) => {
                            last_server_activity = Instant::now();
                            send_ws_frame(&mut socket_write, Message::Pong(payload)).await?;
                        }
                        Message::Binary(_) | Message::Frame(_) | Message::Pong(_) => {
                            last_server_activity = Instant::now();
                        }
                    }
                }
                maybe_event = event_rx.recv() => {
                    let Some(event) = maybe_event else {
                        break;
                    };
                    match event {
                        DaemonEvent::Outbound(message) => {
                            send_worker_message(&mut socket_write, &message).await?;
                        }
                        DaemonEvent::RequestCompleted(response) => {
                            // Remove BEFORE sending ResponseComplete so any concurrent Ping
                            // immediately sees the decremented load.
                            active_requests.remove(&response.request_id);
                            let count = u32::try_from(active_requests.len()).unwrap_or(u32::MAX);
                            self.update_status(|s| s.active_requests = count);
                            send_worker_message(
                                &mut socket_write,
                                &WorkerToServerMessage::ResponseComplete(response),
                            )
                            .await?;
                        }
                        DaemonEvent::RequestFailed { request_id, error } => {
                            // Isolate per-request backend errors: report Error for this
                            // request only without aborting siblings or dropping the WS.
                            active_requests.remove(&request_id);
                            let count = u32::try_from(active_requests.len()).unwrap_or(u32::MAX);
                            self.update_status(|s| s.active_requests = count);
                            tracing::warn!(
                                request_id = %request_id,
                                error = %error,
                                "backend request failed; reporting request error to proxy"
                            );
                            send_worker_message(
                                &mut socket_write,
                                &WorkerToServerMessage::Error(ErrorMessage {
                                    code: "backend_error".to_string(),
                                    message: error,
                                    request_id: Some(request_id),
                                }),
                            )
                            .await?;
                        }
                        DaemonEvent::ModelsDiscovered {
                            models,
                            metadata,
                            force_send,
                        } => {
                            if force_send
                                || models != current_models
                                || metadata != current_metadata
                            {
                                current_models.clone_from(&models);
                                current_metadata.clone_from(&metadata);
                                self.update_status(|s| s.models.clone_from(&models));
                                let current_load = u32::try_from(active_requests.len()).unwrap_or(u32::MAX);
                                send_worker_message(
                                    &mut socket_write,
                                    &WorkerToServerMessage::ModelsUpdate(ModelsUpdateMessage {
                                        models,
                                        current_load,
                                        model_metadata: metadata,
                                    }),
                                )
                                .await?;
                            }
                        }
                    }
                }
                () = idle_deadline => {
                    tracing::warn!(
                        timeout_secs = IDLE_TIMEOUT.as_secs(),
                        "no messages received from server within idle timeout, assuming dead connection"
                    );
                    outcome = SessionOutcome::Disconnected {
                        reason: Some(format!(
                            "no messages from proxy for {}s",
                            IDLE_TIMEOUT.as_secs()
                        )),
                        registered: true,
                    };
                    break;
                }
                _ = ping_ticker.tick() => {
                    if let Err(e) = send_ws_frame(&mut socket_write, Message::Ping(Bytes::new())).await {
                        tracing::warn!(
                            error = %e,
                            "client-initiated ping failed; connection is dead, reconnecting"
                        );
                        outcome = SessionOutcome::Disconnected {
                            reason: Some(format!("keepalive ping failed: {e}")),
                            registered: true,
                        };
                        break;
                    }
                    tracing::trace!("sent client-initiated ws ping");
                }
                () = tick_optional(&mut models_poll_ticker), if models_poll_ticker.is_some() => {
                    spawn_models_discovery(
                        self.client.clone(),
                        self.config.clone(),
                        event_tx.clone(),
                        false,
                    );
                }
                () = notified_optional(self.models_refresh_notify.as_ref()), if self.models_refresh_notify.is_some() => {
                    spawn_models_discovery(
                        self.client.clone(),
                        self.config.clone(),
                        event_tx.clone(),
                        true,
                    );
                }
            }
        }

        for (_, handle) in active_requests {
            handle.abort();
        }
        self.update_status(|s| s.active_requests = 0);

        if matches!(outcome, SessionOutcome::LocalShutdown) {
            let _ = timeout(
                WS_CLOSE_TIMEOUT,
                socket_write.send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: "worker shutting down".into(),
                }))),
            )
            .await;
            self.update_status(|s| {
                s.state = WorkerConnectionState::Disconnected;
                s.last_error = None;
            });
        }

        Ok(outcome)
    }

    /// Runs the worker loop until the proxy closes the connection or the task is cancelled.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot connect, authenticate, serialize protocol
    /// messages, or proxy a backend response.
    pub async fn run(self) -> Result<(), BoxError> {
        self.run_session().await.map(|_| ())
    }

    /// Runs the worker loop, reconnecting after unexpected disconnections.
    ///
    /// Reuses a single HTTP connection pool across reconnects, resets backoff once
    /// a session completes registration, and exits cleanly when a configured
    /// shutdown signal fires.
    ///
    /// # Errors
    ///
    /// Only returns an error if the daemon cannot be configured. Transient network
    /// errors are retried with exponential backoff until local shutdown.
    #[allow(clippy::too_many_lines)]
    pub async fn run_with_reconnect(self) -> Result<(), BoxError> {
        let mut backoff_ms: u64 = 1_000;
        let mut attempt: u32 = 0;

        loop {
            if self.is_local_shutdown() {
                self.update_status(|s| {
                    s.state = WorkerConnectionState::Disconnected;
                    s.active_requests = 0;
                    s.last_error = None;
                });
                return Ok(());
            }

            let session_start = Instant::now();
            match self.run_session().await {
                Ok(SessionOutcome::LocalShutdown) => {
                    return Ok(());
                }
                Ok(SessionOutcome::ProxyGracefulShutdown) => {
                    attempt += 1;
                    let delay_ms = 2_000 + subsecond_jitter_ms();
                    tracing::info!(
                        attempt,
                        delay_ms,
                        session_duration_secs = session_start.elapsed().as_secs(),
                        "server sent graceful shutdown, reconnecting to new instance"
                    );
                    self.update_status(|s| {
                        s.state = WorkerConnectionState::Reconnecting;
                        s.last_error = Some("Server restarting — reconnecting…".to_string());
                    });
                    backoff_ms = 1_000;
                    if sleep_or_shutdown(Duration::from_millis(delay_ms), self.shutdown_rx.as_ref())
                        .await
                    {
                        return Ok(());
                    }
                }
                Ok(SessionOutcome::AuthRejected { reason, registered }) => {
                    attempt += 1;
                    if registered {
                        backoff_ms = 1_000;
                    }
                    let delay_ms = backoff_ms + subsecond_jitter_ms();
                    tracing::error!(
                        attempt,
                        delay_ms,
                        reason = %reason,
                        "proxy rejected worker authentication (close 1008); check WORKER_SECRET / API key"
                    );
                    self.update_status(|s| {
                        s.state = WorkerConnectionState::AuthFailed;
                        s.last_error = Some(format!("Authentication failed: {reason}"));
                    });
                    if sleep_or_shutdown(Duration::from_millis(delay_ms), self.shutdown_rx.as_ref())
                        .await
                    {
                        return Ok(());
                    }
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
                Ok(SessionOutcome::Disconnected { reason, registered }) => {
                    attempt += 1;
                    if registered {
                        backoff_ms = 1_000;
                    }
                    let delay_ms = backoff_ms + subsecond_jitter_ms();
                    let msg =
                        reason.unwrap_or_else(|| "connection closed unexpectedly".to_string());
                    tracing::warn!(
                        attempt,
                        delay_ms,
                        session_duration_secs = session_start.elapsed().as_secs(),
                        reason = %msg,
                        "connection closed unexpectedly, reconnecting"
                    );
                    self.update_status(|s| {
                        s.state = WorkerConnectionState::Reconnecting;
                        s.last_error = Some(msg);
                    });
                    if sleep_or_shutdown(Duration::from_millis(delay_ms), self.shutdown_rx.as_ref())
                        .await
                    {
                        return Ok(());
                    }
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
                Err(e) => {
                    attempt += 1;
                    let delay_ms = backoff_ms + subsecond_jitter_ms();
                    let err_str = e.to_string();
                    tracing::warn!(
                        error = %err_str,
                        attempt,
                        delay_ms,
                        session_duration_secs = session_start.elapsed().as_secs(),
                        "connection error, reconnecting"
                    );
                    self.update_status(|s| {
                        s.state = WorkerConnectionState::Reconnecting;
                        s.last_error = Some(err_str);
                    });
                    if sleep_or_shutdown(Duration::from_millis(delay_ms), self.shutdown_rx.as_ref())
                        .await
                    {
                        return Ok(());
                    }
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
            }
        }
    }

    async fn handle_server_message(
        &self,
        socket_write: &mut WebSocketWrite,
        event_tx: &mpsc::Sender<DaemonEvent>,
        active_requests: &mut HashMap<String, JoinHandle<()>>,
        shutting_down: &mut bool,
        message: ServerToWorkerMessage,
    ) -> Result<bool, BoxError> {
        match message {
            ServerToWorkerMessage::Request(request) => {
                if *shutting_down {
                    let request_id = request.request_id;
                    send_worker_message(
                        socket_write,
                        &WorkerToServerMessage::Error(ErrorMessage {
                            code: "worker_draining".to_string(),
                            message: "worker is shutting down and cannot accept new requests"
                                .to_string(),
                            request_id: Some(request_id),
                        }),
                    )
                    .await?;
                    return Ok(true);
                }
                let request_id = request.request_id.clone();
                let finished_request_id = request_id.clone();
                let client = self.client.clone();
                let config = self.config.clone();
                let event_tx = event_tx.clone();
                let handle = tokio::spawn(async move {
                    match forward_request(client, config, request, &event_tx).await {
                        Ok(response) => {
                            let _ = event_tx.send(DaemonEvent::RequestCompleted(response)).await;
                        }
                        Err(error) => {
                            let _ = event_tx
                                .send(DaemonEvent::RequestFailed {
                                    request_id: finished_request_id,
                                    error: error.to_string(),
                                })
                                .await;
                        }
                    }
                });
                active_requests.insert(request_id, handle);
                let count = u32::try_from(active_requests.len()).unwrap_or(u32::MAX);
                self.update_status(|s| s.active_requests = count);
                Ok(true)
            }
            ServerToWorkerMessage::Ping(ping) => {
                let current_load = u32::try_from(active_requests.len()).unwrap_or(u32::MAX);
                send_worker_message(
                    socket_write,
                    &WorkerToServerMessage::Pong(PongMessage {
                        current_load,
                        timestamp_unix_ms: ping.timestamp_unix_ms,
                    }),
                )
                .await?;
                Ok(true)
            }
            ServerToWorkerMessage::GracefulShutdown(gs) => {
                tracing::info!(
                    reason = ?gs.reason,
                    drain_timeout_secs = ?gs.drain_timeout_secs,
                    in_flight = active_requests.len(),
                    "received GracefulShutdown from proxy"
                );
                *shutting_down = true;
                Ok(true)
            }
            ServerToWorkerMessage::ModelsRefresh(_) => {
                spawn_models_discovery(
                    self.client.clone(),
                    self.config.clone(),
                    event_tx.clone(),
                    true,
                );
                Ok(true)
            }
            ServerToWorkerMessage::Cancel(cancel) => {
                tracing::debug!(
                    request_id = %cancel.request_id,
                    reason = ?cancel.reason,
                    "cancelling in-flight request"
                );
                if let Some(handle) = active_requests.remove(&cancel.request_id) {
                    handle.abort();
                    let count = u32::try_from(active_requests.len()).unwrap_or(u32::MAX);
                    self.update_status(|s| s.active_requests = count);
                }
                // Acknowledge cancellation with ResponseComplete (status 499) so
                // the proxy immediately frees this worker's concurrency slot.
                send_worker_message(
                    socket_write,
                    &WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
                        request_id: cancel.request_id,
                        status_code: 499,
                        headers: modelrelay_protocol::HeaderMap::new(),
                        body: None,
                        token_counts: None,
                    }),
                )
                .await?;
                Ok(true)
            }
            ServerToWorkerMessage::RegisterAck(_) => Ok(true),
        }
    }
}

fn classify_close_frame(frame: Option<&CloseFrame>, registered: bool) -> SessionOutcome {
    let Some(frame) = frame else {
        return SessionOutcome::Disconnected {
            reason: Some("proxy closed connection without close frame".to_string()),
            registered,
        };
    };
    let code: u16 = frame.code.into();
    let reason = frame.reason.to_string();
    tracing::warn!(code, reason = %reason, "proxy closed websocket");
    if code == CLOSE_CODE_POLICY_VIOLATION {
        SessionOutcome::AuthRejected {
            reason: if reason.is_empty() {
                "policy violation (1008)".to_string()
            } else {
                reason
            },
            registered,
        }
    } else {
        SessionOutcome::Disconnected {
            reason: Some(if reason.is_empty() {
                format!("proxy closed connection (code {code})")
            } else {
                format!("proxy closed connection ({code}: {reason})")
            }),
            registered,
        }
    }
}

fn spawn_models_discovery(
    client: reqwest::Client,
    config: WorkerDaemonConfig,
    event_tx: mpsc::Sender<DaemonEvent>,
    force_send: bool,
) {
    tokio::spawn(async move {
        match discover_backend_models(&client, &config).await {
            Ok((models, metadata)) if !models.is_empty() => {
                let _ = event_tx
                    .send(DaemonEvent::ModelsDiscovered {
                        models,
                        metadata,
                        force_send,
                    })
                    .await;
            }
            Ok(_) => {
                tracing::debug!("backend /v1/models returned empty list during refresh");
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "backend /v1/models discovery failed during refresh; keeping existing models"
                );
            }
        }
    });
}

async fn sleep_until_optional(deadline: Option<Instant>) {
    match deadline {
        Some(instant) => tokio::time::sleep_until(instant).await,
        None => std::future::pending().await,
    }
}

async fn tick_optional(ticker: &mut Option<tokio::time::Interval>) {
    match ticker {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending().await,
    }
}

async fn notified_optional(notify: Option<&Arc<Notify>>) {
    match notify {
        Some(n) => n.notified().await,
        None => std::future::pending().await,
    }
}

async fn wait_for_local_shutdown(rx: &mut Option<watch::Receiver<bool>>) {
    let Some(receiver) = rx else {
        std::future::pending::<()>().await;
        return;
    };
    if *receiver.borrow() {
        return;
    }
    while receiver.changed().await.is_ok() {
        if *receiver.borrow() {
            return;
        }
    }
    std::future::pending::<()>().await;
}

/// Sleeps for `duration`, returning `true` early if local shutdown fires.
async fn sleep_or_shutdown(
    duration: Duration,
    shutdown_rx: Option<&watch::Receiver<bool>>,
) -> bool {
    let mut rx = shutdown_rx.cloned();
    select! {
        () = sleep(duration) => false,
        () = wait_for_local_shutdown(&mut rx) => true,
    }
}

async fn send_ws_frame(socket: &mut WebSocketWrite, frame: Message) -> Result<(), BoxError> {
    timeout(WS_WRITE_TIMEOUT, socket.send(frame))
        .await
        .map_err(|_| -> BoxError {
            Box::new(io::Error::new(
                io::ErrorKind::TimedOut,
                "WebSocket write timed out",
            ))
        })?
        .map_err(Into::into)
}

async fn send_worker_message(
    socket: &mut WebSocketWrite,
    message: &WorkerToServerMessage,
) -> Result<(), BoxError> {
    let payload = serde_json::to_string(message)?;
    send_ws_frame(socket, Message::Text(payload.into())).await
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

#[allow(clippy::too_many_lines)]
async fn forward_request(
    client: reqwest::Client,
    config: WorkerDaemonConfig,
    request: RequestMessage,
    event_tx: &mpsc::Sender<DaemonEvent>,
) -> Result<ResponseCompleteMessage, BoxError> {
    let RequestMessage {
        request_id,
        endpoint_path,
        method,
        is_streaming,
        body,
        headers,
        ..
    } = request;

    let req_connection_tokens: HashSet<String> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| {
            value
                .split(',')
                .map(|token| token.trim().to_ascii_lowercase())
                .filter(|token| !token.is_empty())
        })
        .collect();

    let mut backend_headers = ReqwestHeaderMap::new();
    for (name, value) in headers {
        let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        if header_name == CONTENT_LENGTH
            || header_name == HOST
            || header_name == ACCEPT_ENCODING
            || is_hop_by_hop_header(&header_name, &req_connection_tokens)
            || is_client_hop_header(&header_name)
        {
            continue;
        }
        let Ok(header_value) = reqwest::header::HeaderValue::from_str(&value) else {
            continue;
        };
        backend_headers.insert(header_name, header_value);
    }

    let url = config.backend_url(&endpoint_path);
    let reqwest_method = match method.to_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "PUT" => reqwest::Method::PUT,
        "DELETE" => reqwest::Method::DELETE,
        "PATCH" => reqwest::Method::PATCH,
        "HEAD" => reqwest::Method::HEAD,
        "OPTIONS" => reqwest::Method::OPTIONS,
        _ => reqwest::Method::POST,
    };

    let mut req_builder = client
        .request(reqwest_method, &url)
        .headers(backend_headers);
    if !body.is_empty() {
        req_builder = req_builder.body(body);
    }
    let response = req_builder.send().await?;
    let status = response.status();
    let status_code = status.as_u16();

    let resp_connection_tokens: HashSet<String> = response
        .headers()
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|raw| {
            raw.split(',')
                .map(|token| token.trim().to_ascii_lowercase())
                .filter(|token| !token.is_empty())
        })
        .collect();

    let response_headers = response
        .headers()
        .iter()
        .filter(|(name, _)| {
            *name != CONTENT_LENGTH
                && *name != HOST
                && !is_hop_by_hop_header(name, &resp_connection_tokens)
        })
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect();

    // Only stream SSE chunks on 2xx responses; for non-2xx errors (e.g. 400/404/429/500),
    // buffer the full error payload and return a unary ResponseComplete so the proxy
    // relays the real HTTP status code and JSON error body instead of HTTP 200 SSE.
    if is_streaming && status.is_success() {
        let mut response = response;
        let mut decoder = Utf8StreamDecoder::new();
        let mut usage_tracker = SseTokenUsageTracker::default();
        loop {
            let next_chunk = timeout(STREAM_CHUNK_READ_TIMEOUT, response.chunk())
                .await
                .map_err(|_| -> BoxError {
                    Box::new(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "backend stream chunk timed out",
                    ))
                })??;
            let Some(chunk) = next_chunk else {
                break;
            };
            let text_chunk = decoder.push(&chunk);
            if !text_chunk.is_empty() {
                usage_tracker.feed(&text_chunk);
                event_tx
                    .send(DaemonEvent::Outbound(WorkerToServerMessage::ResponseChunk(
                        ResponseChunkMessage {
                            request_id: request_id.clone(),
                            chunk: text_chunk,
                        },
                    )))
                    .await
                    .map_err(|error| -> BoxError {
                        Box::new(io::Error::other(error.to_string()))
                    })?;
            }
        }
        if let Some(tail) = decoder.finish() {
            usage_tracker.feed(&tail);
            event_tx
                .send(DaemonEvent::Outbound(WorkerToServerMessage::ResponseChunk(
                    ResponseChunkMessage {
                        request_id: request_id.clone(),
                        chunk: tail,
                    },
                )))
                .await
                .map_err(|error| -> BoxError { Box::new(io::Error::other(error.to_string())) })?;
        }

        return Ok(ResponseCompleteMessage {
            request_id,
            status_code,
            headers: response_headers,
            body: Some(String::new()),
            token_counts: usage_tracker.finish(),
        });
    }

    let body = response.text().await?;
    let token_counts = if status.is_success() {
        extract_token_counts_from_body(&body)
    } else {
        None
    };

    Ok(ResponseCompleteMessage {
        request_id,
        status_code,
        headers: response_headers,
        body: Some(body),
        token_counts,
    })
}

const MAX_SSE_USAGE_LINE_BYTES: usize = 64 * 1024;

/// Lightweight SSE line tracker that extracts backend token usage and KV-cache
/// hit counts (`OpenAI`, `Anthropic`, `OpenAI` Responses, and `llama.cpp`) without
/// delaying or modifying forwarded stream chunks.
#[derive(Default)]
struct SseTokenUsageTracker {
    line_buf: String,
    skip_until_newline: bool,
    counts: Option<TokenCounts>,
}

impl SseTokenUsageTracker {
    fn feed(&mut self, chunk: &str) {
        for part in chunk.split_inclusive('\n') {
            if self.skip_until_newline {
                if part.ends_with('\n') {
                    self.skip_until_newline = false;
                }
                continue;
            }
            if part.ends_with('\n') {
                if self.line_buf.is_empty() {
                    self.inspect_line(part);
                } else if self.line_buf.len() + part.len() <= MAX_SSE_USAGE_LINE_BYTES {
                    self.line_buf.push_str(part);
                    let line = std::mem::take(&mut self.line_buf);
                    self.inspect_line(&line);
                } else {
                    self.line_buf.clear();
                }
            } else if self.line_buf.len() + part.len() <= MAX_SSE_USAGE_LINE_BYTES {
                self.line_buf.push_str(part);
            } else {
                self.line_buf.clear();
                self.skip_until_newline = true;
            }
        }
    }

    fn inspect_line(&mut self, raw_line: &str) {
        let trimmed = raw_line.trim();
        let Some(data) = trimmed.strip_prefix("data:").map(str::trim) else {
            return;
        };
        if data.is_empty() || data == "[DONE]" || !maybe_has_usage_fields(data) {
            return;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data)
            && let Some(extracted) = extract_token_counts_from_json(&value)
        {
            self.counts = Some(merge_token_counts(self.counts.take(), extracted));
        }
    }

    fn finish(mut self) -> Option<TokenCounts> {
        if !self.line_buf.is_empty() {
            let tail = std::mem::take(&mut self.line_buf);
            self.inspect_line(&tail);
        }
        self.counts
    }
}

fn maybe_has_usage_fields(payload: &str) -> bool {
    payload.contains("\"usage\"")
        || payload.contains("\"timings\"")
        || payload.contains("\"cached_tokens\"")
        || payload.contains("\"cache_read_input_tokens\"")
}

fn extract_token_counts_from_body(body: &str) -> Option<TokenCounts> {
    if body.is_empty() || !maybe_has_usage_fields(body) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    extract_token_counts_from_json(&value)
}

fn json_u32(val: &serde_json::Value, key: &str) -> Option<u32> {
    val.get(key)
        .and_then(serde_json::Value::as_u64)
        .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
}

fn extract_token_counts_from_json(root: &serde_json::Value) -> Option<TokenCounts> {
    let usage = root
        .get("usage")
        .filter(|v| v.is_object())
        .or_else(|| {
            root.get("response")
                .and_then(|r| r.get("usage"))
                .filter(|v| v.is_object())
        })
        .or_else(|| {
            root.get("message")
                .and_then(|m| m.get("usage"))
                .filter(|v| v.is_object())
        });

    let mut prompt_tokens: Option<u32> = None;
    let mut completion_tokens: Option<u32> = None;
    let mut total_tokens: Option<u32> = None;
    let mut cached_prompt_tokens: Option<u32> = None;

    if let Some(u) = usage {
        if let Some(pt) = json_u32(u, "prompt_tokens") {
            prompt_tokens = Some(pt);
        }
        if let Some(ct) = json_u32(u, "completion_tokens").or_else(|| json_u32(u, "output_tokens"))
        {
            completion_tokens = Some(ct);
        }
        if let Some(tt) = json_u32(u, "total_tokens") {
            total_tokens = Some(tt);
        }

        // OpenAI Chat Completions & Responses API cached token counts.
        cached_prompt_tokens = u
            .get("prompt_tokens_details")
            .and_then(|d| json_u32(d, "cached_tokens"))
            .or_else(|| {
                u.get("input_tokens_details")
                    .and_then(|d| json_u32(d, "cached_tokens"))
            })
            .or_else(|| json_u32(u, "cached_tokens"));

        // Anthropic Messages API & OpenAI Responses input_tokens handling.
        let cache_read = json_u32(u, "cache_read_input_tokens");
        let cache_creation = json_u32(u, "cache_creation_input_tokens");
        if cache_read.is_some() {
            cached_prompt_tokens = cache_read;
        }
        if prompt_tokens.is_none()
            && let Some(input) = json_u32(u, "input_tokens")
        {
            // Anthropic separates `input_tokens` (uncached) from `cache_read_input_tokens`
            // and `cache_creation_input_tokens`, whereas OpenAI Responses includes cached
            // tokens inside `input_tokens`. Sum Anthropic's cache fields when present.
            let total_input = if cache_read.is_some() || cache_creation.is_some() {
                input
                    .saturating_add(cache_read.unwrap_or(0))
                    .saturating_add(cache_creation.unwrap_or(0))
            } else {
                input
            };
            prompt_tokens = Some(total_input);
        }
    }

    // `llama.cpp` / `llama-server` native timings (`cache_n` cached + `prompt_n` newly evaluated).
    if let Some(timings) = root.get("timings").filter(|v| v.is_object()) {
        let cache_n = json_u32(timings, "cache_n");
        let prompt_n = json_u32(timings, "prompt_n");
        if cached_prompt_tokens.is_none() && cache_n.is_some() {
            cached_prompt_tokens = cache_n;
        }
        if prompt_tokens.is_none() && (cache_n.is_some() || prompt_n.is_some()) {
            prompt_tokens = Some(cache_n.unwrap_or(0).saturating_add(prompt_n.unwrap_or(0)));
        }
    }

    if total_tokens.is_none()
        && let (Some(p), Some(c)) = (prompt_tokens, completion_tokens)
    {
        total_tokens = Some(p.saturating_add(c));
    }

    if prompt_tokens.is_none()
        && completion_tokens.is_none()
        && total_tokens.is_none()
        && cached_prompt_tokens.is_none()
    {
        return None;
    }

    Some(TokenCounts {
        prompt_tokens,
        completion_tokens,
        total_tokens,
        cached_prompt_tokens,
    })
}

fn merge_token_counts(existing: Option<TokenCounts>, incoming: TokenCounts) -> TokenCounts {
    let Some(prev) = existing else {
        return incoming;
    };
    let prompt_tokens = incoming.prompt_tokens.or(prev.prompt_tokens);
    let completion_tokens = incoming.completion_tokens.or(prev.completion_tokens);
    let cached_prompt_tokens = incoming.cached_prompt_tokens.or(prev.cached_prompt_tokens);
    let total_tokens = incoming.total_tokens.or(prev.total_tokens).or_else(|| {
        match (prompt_tokens, completion_tokens) {
            (Some(p), Some(c)) => Some(p.saturating_add(c)),
            _ => None,
        }
    });
    TokenCounts {
        prompt_tokens,
        completion_tokens,
        total_tokens,
        cached_prompt_tokens,
    }
}

/// Returns a pseudo-random jitter value in the range 0–499 ms derived from the
/// subsecond component of the system clock.
fn subsecond_jitter_ms() -> u64 {
    u64::from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.subsec_millis())
            .unwrap_or(0)
            % 500,
    )
}

async fn discover_backend_models(
    client: &reqwest::Client,
    config: &WorkerDaemonConfig,
) -> Result<(Vec<String>, ModelMetadataMap), BoxError> {
    let mut last_error: Option<BoxError> = None;
    for path in ["/v1/models", "/models"] {
        let response = match client
            .get(config.backend_url(path))
            .timeout(MODEL_DISCOVERY_TIMEOUT)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(err) => {
                last_error = Some(Box::new(err));
                break;
            }
        };
        let response = match response.error_for_status() {
            Ok(resp) => resp,
            Err(err) => {
                last_error = Some(Box::new(err));
                continue;
            }
        };
        let body = match response.json::<serde_json::Value>().await {
            Ok(body) => body,
            Err(err) => {
                last_error = Some(Box::new(err));
                continue;
            }
        };
        if let Some((models, metadata)) = parse_openai_models_response(&body) {
            return Ok((models, metadata));
        }
        last_error = Some(Box::new(io::Error::other(format!(
            "backend {path} response missing data array"
        ))));
    }

    Err(last_error.unwrap_or_else(|| {
        Box::new(io::Error::other(
            "backend /v1/models and /models discovery failed",
        ))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(proxy_base_url: &str, backend_base_url: &str) -> WorkerDaemonConfig {
        WorkerDaemonConfig {
            proxy_base_url: proxy_base_url.to_string(),
            provider: "test-provider".to_string(),
            worker_secret: "secret".to_string(),
            worker_name: "test-worker".to_string(),
            models: vec!["model-a".to_string()],
            max_concurrent: 4,
            backend_base_url: backend_base_url.to_string(),
            endpoint_prefixes: vec![],
        }
    }

    #[test]
    fn websocket_url_converts_http_to_ws() {
        let cfg = test_config("http://proxy.local:8080", "http://backend:11434");
        assert_eq!(
            cfg.websocket_url(),
            "ws://proxy.local:8080/v1/worker/connect?provider=test-provider"
        );
    }

    #[test]
    fn websocket_url_converts_https_to_wss() {
        let cfg = test_config("https://proxy.example.com", "http://backend:11434");
        assert_eq!(
            cfg.websocket_url(),
            "wss://proxy.example.com/v1/worker/connect?provider=test-provider"
        );
    }

    #[test]
    fn websocket_url_strips_trailing_slash() {
        let cfg = test_config("http://proxy.local:8080/", "http://backend:11434");
        assert_eq!(
            cfg.websocket_url(),
            "ws://proxy.local:8080/v1/worker/connect?provider=test-provider"
        );
    }

    #[test]
    fn websocket_url_strips_multiple_trailing_slashes() {
        let cfg = test_config("https://proxy.local///", "http://backend:11434");
        assert_eq!(
            cfg.websocket_url(),
            "wss://proxy.local/v1/worker/connect?provider=test-provider"
        );
    }

    #[test]
    fn websocket_url_passthrough_when_no_http_prefix() {
        let cfg = test_config("ws://already-ws.local", "http://backend:11434");
        assert_eq!(
            cfg.websocket_url(),
            "ws://already-ws.local/v1/worker/connect?provider=test-provider"
        );
    }

    #[test]
    fn websocket_url_percent_encodes_provider_and_trims() {
        let mut cfg = test_config("  https://proxy.local/  ", "http://backend:11434");
        cfg.provider = " my provider&x=1 ".to_string();
        assert_eq!(
            cfg.websocket_url(),
            "wss://proxy.local/v1/worker/connect?provider=my%20provider%26x%3D1"
        );
    }

    #[test]
    fn backend_url_joins_path() {
        let cfg = test_config("http://proxy.local", "http://localhost:11434");
        assert_eq!(
            cfg.backend_url("/v1/chat/completions"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    #[test]
    fn backend_url_strips_trailing_slash_on_base() {
        let cfg = test_config("http://proxy.local", "http://localhost:11434/");
        assert_eq!(
            cfg.backend_url("/v1/models"),
            "http://localhost:11434/v1/models"
        );
    }

    #[test]
    fn backend_url_deduplicates_v1_prefix() {
        let cfg = test_config("http://proxy.local", "http://localhost:11434/v1");
        assert_eq!(
            cfg.backend_url("/v1/chat/completions"),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            cfg.backend_url("/v1/models"),
            "http://localhost:11434/v1/models"
        );
        // Root metadata endpoints for llama.cpp (/props), LM Studio (/api/v1/models, /api/v0/models),
        // and Ollama (/api/show) resolve against the backend root even when backend_url ends in /v1.
        assert_eq!(
            cfg.backend_url("/props?model=foo&autoload=false"),
            "http://localhost:11434/props?model=foo&autoload=false"
        );
        assert_eq!(
            cfg.backend_url("/api/v1/models"),
            "http://localhost:11434/api/v1/models"
        );
        assert_eq!(
            cfg.backend_url("/api/v0/models"),
            "http://localhost:11434/api/v0/models"
        );
        assert_eq!(
            cfg.backend_url("/api/show"),
            "http://localhost:11434/api/show"
        );
    }

    #[test]
    fn backend_url_with_subpath_base() {
        let cfg = test_config("http://proxy.local", "http://localhost:11434/api/v2/");
        assert_eq!(
            cfg.backend_url("/chat"),
            "http://localhost:11434/api/v2/chat"
        );
    }

    #[test]
    fn backend_url_empty_endpoint_path() {
        let cfg = test_config("http://proxy.local", "http://localhost:11434");
        assert_eq!(cfg.backend_url(""), "http://localhost:11434");
    }

    #[test]
    fn utf8_stream_decoder_reassembles_split_multibyte_chars() {
        let mut decoder = Utf8StreamDecoder::new();
        let text = "Hello — 世界 🚀!";
        let bytes = text.as_bytes();
        let mut assembled = String::new();
        // Feed 1 byte at a time — worst-case TCP fragmentation!
        for b in bytes {
            assembled.push_str(&decoder.push(&[*b]));
        }
        if let Some(tail) = decoder.finish() {
            assembled.push_str(&tail);
        }
        assert_eq!(assembled, text);
    }

    #[test]
    fn subsecond_jitter_ms_within_range() {
        for _ in 0..50 {
            let jitter = subsecond_jitter_ms();
            assert!(jitter < 500, "jitter {jitter} must be < 500");
        }
    }

    #[test]
    fn config_debug_and_clone() {
        let cfg = test_config("http://proxy.local", "http://backend:11434");
        let cloned = cfg.clone();
        assert_eq!(cfg, cloned);
        let debug = format!("{cfg:?}");
        assert!(debug.contains("proxy_base_url"));
    }

    #[tokio::test]
    async fn resolve_wildcard_models_skips_when_no_wildcard() {
        let mut cfg = test_config("http://proxy.local", "http://backend:11434");
        cfg.models = vec!["model-a".to_string(), "model-b".to_string()];
        cfg.resolve_wildcard_models().await;
        assert_eq!(cfg.models, vec!["model-a", "model-b"]);
    }

    #[tokio::test]
    async fn resolve_wildcard_models_falls_back_on_unreachable_backend() {
        let mut cfg = test_config("http://proxy.local", "http://127.0.0.1:1");
        cfg.models = vec!["*".to_string()];
        cfg.resolve_wildcard_models().await;
        assert_eq!(cfg.models, vec!["*"]);
    }

    #[test]
    fn extracts_openai_chat_cached_token_counts() {
        let body = r#"{
            "id": "chatcmpl-1",
            "usage": {
                "prompt_tokens": 2048,
                "completion_tokens": 128,
                "total_tokens": 2176,
                "prompt_tokens_details": { "cached_tokens": 1800 }
            }
        }"#;
        assert_eq!(
            extract_token_counts_from_body(body),
            Some(TokenCounts {
                prompt_tokens: Some(2048),
                completion_tokens: Some(128),
                total_tokens: Some(2176),
                cached_prompt_tokens: Some(1800),
            })
        );
    }

    #[test]
    fn extracts_anthropic_unary_and_streaming_cache_read_tokens() {
        let unary = r#"{
            "id": "msg_1",
            "usage": {
                "input_tokens": 200,
                "cache_creation_input_tokens": 100,
                "cache_read_input_tokens": 1700,
                "output_tokens": 55
            }
        }"#;
        assert_eq!(
            extract_token_counts_from_body(unary),
            Some(TokenCounts {
                prompt_tokens: Some(2000),
                completion_tokens: Some(55),
                total_tokens: Some(2055),
                cached_prompt_tokens: Some(1700),
            })
        );

        let mut tracker = SseTokenUsageTracker::default();
        tracker.feed(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":50,\"cache_read_in",
        );
        tracker.feed(
            "put_tokens\":950}}}\n\nevent: content_block_delta\ndata: {\"delta\":{\"text\":\"hi\"}}\n\n",
        );
        tracker.feed(
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":42}}\n\n",
        );
        assert_eq!(
            tracker.finish(),
            Some(TokenCounts {
                prompt_tokens: Some(1000),
                completion_tokens: Some(42),
                total_tokens: Some(1042),
                cached_prompt_tokens: Some(950),
            })
        );
    }

    #[test]
    fn extracts_openai_responses_sse_and_llama_cpp_timings() {
        let mut tracker = SseTokenUsageTracker::default();
        tracker.feed("data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n");
        tracker.feed(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"usage\":{\"input_tokens\":4000,\"output_tokens\":200,\"total_tokens\":4200,\"input_tokens_details\":{\"cached_tokens\":3500}}}}\n\ndata: [DONE]\n\n",
        );
        assert_eq!(
            tracker.finish(),
            Some(TokenCounts {
                prompt_tokens: Some(4000),
                completion_tokens: Some(200),
                total_tokens: Some(4200),
                cached_prompt_tokens: Some(3500),
            })
        );

        let llama_body = r#"{"choices":[],"timings":{"cache_n":1536,"prompt_n":256}}"#;
        assert_eq!(
            extract_token_counts_from_body(llama_body),
            Some(TokenCounts {
                prompt_tokens: Some(1792),
                completion_tokens: None,
                total_tokens: None,
                cached_prompt_tokens: Some(1536),
            })
        );
    }
}
