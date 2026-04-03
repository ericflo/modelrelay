use std::{collections::HashMap, error::Error, io, time::SystemTime};

use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use reqwest::header::{CONNECTION, CONTENT_LENGTH, HOST, HeaderMap as ReqwestHeaderMap};
use tokio::{
    select,
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, sleep},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use worker_protocol::{
    ModelsUpdateMessage, PongMessage, RegisterMessage, RequestMessage, ResponseChunkMessage,
    ResponseCompleteMessage, ServerToWorkerMessage, WorkerToServerMessage,
};

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

        loop {
            select! {
                maybe_message = socket_read.next() => {
                    let Some(message) = maybe_message else {
                        break;
                    };
                    match message? {
                        Message::Text(payload) => {
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
                            socket_write.send(Message::Pong(payload)).await?;
                        }
                        Message::Binary(_) | Message::Frame(_) | Message::Pong(_) => {}
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
            match daemon.run_session().await {
                Ok(true) => {
                    eprintln!("worker-daemon: graceful shutdown received, exiting");
                    return Ok(());
                }
                Ok(false) => {
                    attempt += 1;
                    let jitter_ms = subsecond_jitter_ms();
                    eprintln!(
                        "worker-daemon: connection closed unexpectedly \
                         (attempt {attempt}), reconnecting in {}ms",
                        backoff_ms + jitter_ms
                    );
                    sleep(Duration::from_millis(backoff_ms + jitter_ms)).await;
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
                Err(e) => {
                    attempt += 1;
                    let jitter_ms = subsecond_jitter_ms();
                    eprintln!(
                        "worker-daemon: error: {e} \
                         (attempt {attempt}), reconnecting in {}ms",
                        backoff_ms + jitter_ms
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
                let models = refresh_models(self.client.clone(), &self.config).await?;
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

async fn refresh_models(
    client: reqwest::Client,
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
