use std::error::Error;

use futures_util::{SinkExt, StreamExt};
use reqwest::header::{CONNECTION, CONTENT_LENGTH, HOST, HeaderMap as ReqwestHeaderMap};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use worker_protocol::{
    ModelsUpdateMessage, PongMessage, RegisterMessage, RequestMessage, ResponseChunkMessage,
    ResponseCompleteMessage, ServerToWorkerMessage, WorkerToServerMessage,
};

type BoxError = Box<dyn Error + Send + Sync>;

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

    /// Runs the worker loop until the proxy closes the connection or the task is cancelled.
    ///
    /// # Errors
    ///
    /// Returns an error when the daemon cannot connect, authenticate, serialize protocol
    /// messages, or proxy a backend response.
    pub async fn run(self) -> Result<(), BoxError> {
        let mut request = self.config.websocket_url().into_client_request()?;
        request
            .headers_mut()
            .insert("x-worker-secret", self.config.worker_secret.parse()?);

        let (mut socket, _) = connect_async(request).await?;
        send_worker_message(
            &mut socket,
            &WorkerToServerMessage::Register(RegisterMessage {
                worker_name: self.config.worker_name.clone(),
                models: self.config.models.clone(),
                max_concurrent: self.config.max_concurrent,
                protocol_version: Some("2026-04-bridge-v1".to_string()),
                current_load: Some(0),
            }),
        )
        .await?;

        while let Some(message) = socket.next().await {
            match message? {
                Message::Text(payload) => {
                    let server_message = serde_json::from_str::<ServerToWorkerMessage>(&payload)?;
                    if !self
                        .handle_server_message(&mut socket, server_message)
                        .await?
                    {
                        break;
                    }
                }
                Message::Close(_) => break,
                Message::Ping(payload) => {
                    socket.send(Message::Pong(payload)).await?;
                }
                Message::Binary(_) | Message::Frame(_) | Message::Pong(_) => {}
            }
        }

        Ok(())
    }

    async fn handle_server_message(
        &self,
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        message: ServerToWorkerMessage,
    ) -> Result<bool, BoxError> {
        match message {
            ServerToWorkerMessage::Request(request) => {
                let response = self.forward_request(socket, request).await?;
                send_worker_message(socket, &WorkerToServerMessage::ResponseComplete(response))
                    .await?;
                Ok(true)
            }
            ServerToWorkerMessage::Ping(ping) => {
                send_worker_message(
                    socket,
                    &WorkerToServerMessage::Pong(PongMessage {
                        current_load: 0,
                        timestamp_unix_ms: ping.timestamp_unix_ms,
                    }),
                )
                .await?;
                Ok(true)
            }
            ServerToWorkerMessage::GracefulShutdown(_) => Ok(false),
            ServerToWorkerMessage::ModelsRefresh(_) => {
                send_worker_message(
                    socket,
                    &WorkerToServerMessage::ModelsUpdate(ModelsUpdateMessage {
                        models: self.config.models.clone(),
                        current_load: 0,
                    }),
                )
                .await?;
                Ok(true)
            }
            ServerToWorkerMessage::RegisterAck(_) | ServerToWorkerMessage::Cancel(_) => Ok(true),
        }
    }

    async fn forward_request(
        &self,
        socket: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        request: RequestMessage,
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

        let response = self
            .client
            .post(self.config.backend_url(&endpoint_path))
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
                send_worker_message(
                    socket,
                    &WorkerToServerMessage::ResponseChunk(ResponseChunkMessage {
                        request_id: request_id.clone(),
                        chunk,
                    }),
                )
                .await?;
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
}

async fn send_worker_message(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    message: &WorkerToServerMessage,
) -> Result<(), BoxError> {
    let payload = serde_json::to_string(message)?;
    socket.send(Message::Text(payload.into())).await?;
    Ok(())
}
