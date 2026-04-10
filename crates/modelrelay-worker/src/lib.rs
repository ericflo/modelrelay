use std::{collections::HashMap, error::Error, io, time::SystemTime};

use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use modelrelay_protocol::{
    ModelsUpdateMessage, PongMessage, RegisterMessage, RequestMessage, ResponseChunkMessage,
    ResponseCompleteMessage, ServerToWorkerMessage, WorkerToServerMessage,
};
use reqwest::header::{CONNECTION, CONTENT_LENGTH, HOST, HeaderMap as ReqwestHeaderMap};
use tokio::{
    select,
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, Instant, sleep},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

/// How long the worker waits without receiving any message from the server
/// (including pings) before assuming the connection is dead and reconnecting.
/// The server sends application-level pings every 30 s, so 90 s (3× that)
/// gives plenty of margin for transient delays while still catching truly
/// dead connections within a couple of minutes.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

type BoxError = Box<dyn Error + Send + Sync>;
type WebSocketStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WebSocketWrite = SplitSink<WebSocketStream, Message>;

enum DaemonEvent {
    Outbound(WorkerToServerMessage),
    RequestFinished(String),
    RequestFailed { request_id: String, error: String },
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
}

impl WorkerDaemonConfig {
    /// If the configured models list contains `"*"`, queries the backend's
    /// `/v1/models` endpoint and replaces the wildcard with the actual model
    /// IDs reported by the backend.  Falls back to the original list when the
    /// backend does not support the endpoint or returns an error.
    pub async fn resolve_wildcard_models(&mut self) {
        if !self.models.iter().any(|m| m.trim() == "*") {
            return;
        }

        let client = reqwest::Client::new();
        match discover_backend_models(&client, self).await {
            Ok(discovered) if !discovered.is_empty() => {
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
        let base = self.proxy_base_url.trim_end_matches('/');
        let scheme = if let Some(stripped) = base.strip_prefix("http://") {
            format!("ws://{stripped}")
        } else if let Some(stripped) = base.strip_prefix("https://") {
            format!("wss://{stripped}")
        } else {
            base.to_string()
        };

        format!(
            "{scheme}/v1/worker/connect?provider={}",
            self.provider.as_str()
        )
    }

    #[must_use]
    pub fn backend_url(&self, endpoint_path: &str) -> String {
        format!(
            "{}{}",
            self.backend_base_url.trim_end_matches('/'),
            endpoint_path
        )
    }
}

pub struct WorkerDaemon {
    config: WorkerDaemonConfig,
    client: reqwest::Client,
}

impl WorkerDaemon {
    #[must_use]
    pub fn new(config: WorkerDaemonConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }

    /// Runs one connection session.
    ///
    /// Returns `Ok(true)` when the proxy sent a `GracefulShutdown` before closing
    /// (the caller should not reconnect), and `Ok(false)` when the connection was
    /// lost unexpectedly (the caller should reconnect).
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot connect, authenticate, serialize protocol
    /// messages, or proxy a backend response.
    async fn run_session(&self) -> Result<bool, BoxError> {
        let mut request = self.config.websocket_url().into_client_request()?;
        request
            .headers_mut()
            .insert("x-worker-secret", self.config.worker_secret.parse()?);

        let (socket, _) = connect_async(request).await?;
        let (mut socket_write, mut socket_read) = socket.split();
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let mut active_requests = HashMap::<String, JoinHandle<()>>::new();
        let mut shutting_down = false;
        let mut last_server_activity = Instant::now();

        send_worker_message(
            &mut socket_write,
            &WorkerToServerMessage::Register(RegisterMessage {
                worker_name: self.config.worker_name.clone(),
                models: self.config.models.clone(),
                max_concurrent: self.config.max_concurrent,
                protocol_version: Some("2026-04-bridge-v1".to_string()),
                current_load: Some(0),
            }),
        )
        .await?;

        tracing::info!(
            url = %self.config.websocket_url(),
            "connected to proxy, registered worker"
        );

        loop {
            let idle_deadline = sleep(IDLE_TIMEOUT.saturating_sub(last_server_activity.elapsed()));

            select! {
                maybe_message = socket_read.next() => {
                    let Some(message) = maybe_message else {
                        break;
                    };
                    match message? {
                        Message::Text(payload) => {
                            last_server_activity = Instant::now();
                            let server_message = serde_json::from_str::<ServerToWorkerMessage>(&payload)?;
                            if !self.handle_server_message(
                                &event_tx,
                                &mut active_requests,
                                &mut shutting_down,
                                server_message,
                            ).await? {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        Message::Ping(payload) => {
                            last_server_activity = Instant::now();
                            socket_write.send(Message::Pong(payload)).await?;
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
                        DaemonEvent::RequestFinished(request_id) => {
                            active_requests.remove(&request_id);
                        }
                        DaemonEvent::RequestFailed { request_id, error } => {
                            active_requests.remove(&request_id);
                            for (_, handle) in active_requests.drain() {
                                handle.abort();
                            }
                            return Err(Box::new(io::Error::other(error)));
                        }
                    }
                }
                () = idle_deadline => {
                    tracing::warn!(
                        timeout_secs = IDLE_TIMEOUT.as_secs(),
                        "no messages received from server within idle timeout, assuming dead connection"
                    );
                    break;
                }
            }
        }

        for (_, handle) in active_requests {
            handle.abort();
        }

        Ok(shutting_down)
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
    /// Exits cleanly on a graceful proxy shutdown or task cancellation. On connection
    /// errors or unexpected drops, waits with exponential backoff (1 s base, 30 s cap,
    /// up to 500 ms of jitter) before reconnecting. Each attempt is logged to stderr.
    ///
    /// # Errors
    ///
    /// Only returns an error if the underlying machinery cannot be set up at all (e.g.
    /// invalid config that cannot form a URL or header value). Transient network errors
    /// are retried indefinitely.
    pub async fn run_with_reconnect(self) -> Result<(), BoxError> {
        let config = self.config;
        let mut backoff_ms: u64 = 1_000;
        let mut attempt: u32 = 0;

        loop {
            let daemon = Self::new(config.clone());
            let session_start = Instant::now();
            match daemon.run_session().await {
                Ok(true) => {
                    // Server sent a graceful shutdown — it's going away (e.g. rolling
                    // deploy), not telling us to die. Reconnect to the replacement pod
                    // after a short delay.
                    attempt += 1;
                    let jitter_ms = subsecond_jitter_ms();
                    let delay_ms = 2_000 + jitter_ms;
                    tracing::info!(
                        attempt,
                        delay_ms,
                        session_duration_secs = session_start.elapsed().as_secs(),
                        "server sent graceful shutdown, reconnecting to new instance"
                    );
                    sleep(Duration::from_millis(delay_ms)).await;
                    backoff_ms = 1_000; // reset backoff — this is expected
                }
                Ok(false) => {
                    attempt += 1;
                    let session_secs = session_start.elapsed().as_secs();
                    // If the session lasted longer than the idle timeout, it was a
                    // genuine connection that eventually dropped — reset backoff so
                    // we reconnect quickly rather than waiting up to 30 s.
                    if session_secs > IDLE_TIMEOUT.as_secs() {
                        backoff_ms = 1_000;
                    }
                    let jitter_ms = subsecond_jitter_ms();
                    tracing::warn!(
                        attempt,
                        delay_ms = backoff_ms + jitter_ms,
                        session_duration_secs = session_secs,
                        "connection closed unexpectedly, reconnecting"
                    );
                    sleep(Duration::from_millis(backoff_ms + jitter_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
                Err(e) => {
                    attempt += 1;
                    let session_secs = session_start.elapsed().as_secs();
                    if session_secs > IDLE_TIMEOUT.as_secs() {
                        backoff_ms = 1_000;
                    }
                    let jitter_ms = subsecond_jitter_ms();
                    tracing::warn!(
                        error = %e,
                        attempt,
                        delay_ms = backoff_ms + jitter_ms,
                        session_duration_secs = session_secs,
                        "connection error, reconnecting"
                    );
                    sleep(Duration::from_millis(backoff_ms + jitter_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
            }
        }
    }

    async fn handle_server_message(
        &self,
        event_tx: &mpsc::UnboundedSender<DaemonEvent>,
        active_requests: &mut HashMap<String, JoinHandle<()>>,
        shutting_down: &mut bool,
        message: ServerToWorkerMessage,
    ) -> Result<bool, BoxError> {
        match message {
            ServerToWorkerMessage::Request(request) => {
                if *shutting_down {
                    return Err(Box::new(io::Error::other(
                        "received new request while draining worker daemon",
                    )));
                }
                let request_id = request.request_id.clone();
                let finished_request_id = request_id.clone();
                let client = self.client.clone();
                let config = self.config.clone();
                let event_tx = event_tx.clone();
                let handle = tokio::spawn(async move {
                    match forward_request(client, config, request, &event_tx).await {
                        Ok(response) => {
                            let _ = event_tx.send(DaemonEvent::Outbound(
                                WorkerToServerMessage::ResponseComplete(response),
                            ));
                            let _ =
                                event_tx.send(DaemonEvent::RequestFinished(finished_request_id));
                        }
                        Err(error) => {
                            let _ = event_tx.send(DaemonEvent::RequestFailed {
                                request_id: finished_request_id,
                                error: error.to_string(),
                            });
                        }
                    }
                });
                active_requests.insert(request_id, handle);
                Ok(true)
            }
            ServerToWorkerMessage::Ping(ping) => {
                let current_load = u32::try_from(active_requests.len()).unwrap_or(u32::MAX);
                event_tx
                    .send(DaemonEvent::Outbound(WorkerToServerMessage::Pong(
                        PongMessage {
                            current_load,
                            timestamp_unix_ms: ping.timestamp_unix_ms,
                        },
                    )))
                    .map_err(|error| -> BoxError {
                        Box::new(io::Error::other(error.to_string()))
                    })?;
                Ok(true)
            }
            ServerToWorkerMessage::GracefulShutdown(_) => {
                *shutting_down = true;
                Ok(true)
            }
            ServerToWorkerMessage::ModelsRefresh(_) => {
                let models = discover_backend_models(&self.client, &self.config).await?;
                event_tx
                    .send(DaemonEvent::Outbound(WorkerToServerMessage::ModelsUpdate(
                        ModelsUpdateMessage {
                            models,
                            current_load: u32::try_from(active_requests.len()).unwrap_or(u32::MAX),
                        },
                    )))
                    .map_err(|error| -> BoxError {
                        Box::new(io::Error::other(error.to_string()))
                    })?;
                Ok(true)
            }
            ServerToWorkerMessage::Cancel(cancel) => {
                if let Some(handle) = active_requests.remove(&cancel.request_id) {
                    handle.abort();
                }
                Ok(true)
            }
            ServerToWorkerMessage::RegisterAck(_) => Ok(true),
        }
    }
}

async fn send_worker_message(
    socket: &mut WebSocketWrite,
    message: &WorkerToServerMessage,
) -> Result<(), BoxError> {
    let payload = serde_json::to_string(message)?;
    socket.send(Message::Text(payload.into())).await?;
    Ok(())
}

async fn forward_request(
    client: reqwest::Client,
    config: WorkerDaemonConfig,
    request: RequestMessage,
    event_tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<ResponseCompleteMessage, BoxError> {
    let RequestMessage {
        request_id,
        endpoint_path,
        is_streaming,
        body,
        headers,
        ..
    } = request;

    let mut backend_headers = ReqwestHeaderMap::new();
    for (name, value) in headers {
        let Ok(header_name) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        if header_name == CONTENT_LENGTH || header_name == CONNECTION || header_name == HOST {
            continue;
        }
        let Ok(header_value) = reqwest::header::HeaderValue::from_str(&value) else {
            continue;
        };
        backend_headers.insert(header_name, header_value);
    }

    let response = client
        .post(config.backend_url(&endpoint_path))
        .headers(backend_headers)
        .body(body)
        .send()
        .await?;
    let status_code = response.status().as_u16();
    let response_headers = response
        .headers()
        .iter()
        .filter(|(name, _)| *name != CONTENT_LENGTH && *name != CONNECTION && *name != HOST)
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect();
    if is_streaming {
        let mut response = response;
        while let Some(chunk) = response.chunk().await? {
            let chunk = String::from_utf8(chunk.to_vec())?;
            event_tx
                .send(DaemonEvent::Outbound(WorkerToServerMessage::ResponseChunk(
                    ResponseChunkMessage {
                        request_id: request_id.clone(),
                        chunk,
                    },
                )))
                .map_err(|error| -> BoxError { Box::new(io::Error::other(error.to_string())) })?;
        }

        return Ok(ResponseCompleteMessage {
            request_id,
            status_code,
            headers: response_headers,
            body: Some(String::new()),
            token_counts: None,
        });
    }

    let body = response.text().await?;

    Ok(ResponseCompleteMessage {
        request_id,
        status_code,
        headers: response_headers,
        body: Some(body),
        token_counts: None,
    })
}

/// Returns a pseudo-random jitter value in the range 0–499 ms derived from the
/// subsecond component of the system clock.  Not cryptographically strong, but
/// sufficient for backoff spread across independent processes.
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
) -> Result<Vec<String>, BoxError> {
    let response = client.get(config.backend_url("/v1/models")).send().await?;
    let response = response.error_for_status()?;
    let body = response.json::<serde_json::Value>().await?;
    let Some(data) = body.get("data").and_then(serde_json::Value::as_array) else {
        return Err(Box::new(io::Error::other(
            "backend /v1/models response missing data array",
        )));
    };

    Ok(data
        .iter()
        .filter_map(|entry| entry.get("id").and_then(serde_json::Value::as_str))
        .map(ToOwned::to_owned)
        .collect())
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
    fn subsecond_jitter_ms_within_range() {
        // Run multiple times to increase confidence (the function is time-derived)
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
        // Ensure Debug is implemented (compilation check + format)
        let debug = format!("{cfg:?}");
        assert!(debug.contains("proxy_base_url"));
    }

    #[tokio::test]
    async fn resolve_wildcard_models_skips_when_no_wildcard() {
        let mut cfg = test_config("http://proxy.local", "http://backend:11434");
        cfg.models = vec!["model-a".to_string(), "model-b".to_string()];
        cfg.resolve_wildcard_models().await;
        // Models should be unchanged when there's no wildcard
        assert_eq!(cfg.models, vec!["model-a", "model-b"]);
    }

    #[tokio::test]
    async fn resolve_wildcard_models_falls_back_on_unreachable_backend() {
        let mut cfg = test_config("http://proxy.local", "http://127.0.0.1:1");
        cfg.models = vec!["*".to_string()];
        cfg.resolve_wildcard_models().await;
        // Should keep wildcard when backend is unreachable
        assert_eq!(cfg.models, vec!["*"]);
    }
}
