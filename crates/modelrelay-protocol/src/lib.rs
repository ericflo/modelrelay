//! Wire protocol types for communication between the modelrelay proxy server and remote workers.
//!
//! This crate defines the JSON-serializable message types exchanged over WebSocket connections
//! between the central proxy server and worker daemons. Messages are tagged with a `"type"` field
//! for unambiguous deserialization.

#![deny(missing_docs)]

#[cfg(feature = "admin-api")]
pub mod admin_api;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A case-insensitive-style header map carried alongside requests and responses.
///
/// Keys are header names (lowercase by convention) and values are header values.
pub type HeaderMap = BTreeMap<String, String>;

/// Messages sent from the proxy server to a connected worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerToWorkerMessage {
    /// Acknowledges a worker's registration and confirms its assigned identity.
    RegisterAck(RegisterAck),
    /// Dispatches an inference request to the worker for processing.
    Request(RequestMessage),
    /// Instructs the worker to cancel an in-flight request.
    Cancel(CancelMessage),
    /// Server-initiated keepalive probe; the worker must reply with [`WorkerToServerMessage::Pong`].
    Ping(PingMessage),
    /// Asks the worker to stop accepting new requests and drain its in-flight work.
    GracefulShutdown(GracefulShutdownMessage),
    /// Asks the worker to re-advertise its current model list.
    ModelsRefresh(ModelsRefreshMessage),
}

/// Messages sent from a worker back to the proxy server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerToServerMessage {
    /// Initial registration announcing the worker's identity, models, and capacity.
    Register(RegisterMessage),
    /// Updates the server with the worker's current model list and load.
    ModelsUpdate(ModelsUpdateMessage),
    /// Delivers one chunk of a streaming response back to the waiting client.
    ResponseChunk(ResponseChunkMessage),
    /// Signals that the worker has finished processing a request.
    ResponseComplete(ResponseCompleteMessage),
    /// Reply to a server [`ServerToWorkerMessage::Ping`], carrying current load information.
    Pong(PongMessage),
    /// Reports an error encountered while processing a request or a general worker fault.
    Error(ErrorMessage),
}

/// Sent by a worker when it first connects to register its capabilities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterMessage {
    /// Human-readable name for this worker (e.g. hostname or GPU box label).
    pub worker_name: String,
    /// Model identifiers this worker can serve.
    pub models: Vec<String>,
    /// Maximum number of concurrent requests this worker can handle.
    pub max_concurrent: u32,
    /// Protocol version the worker speaks, for forward compatibility negotiation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    /// Number of requests the worker is currently processing at connect time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_load: Option<u32>,
}

/// Server's acknowledgment of a successful worker registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterAck {
    /// Unique identifier assigned to this worker by the server.
    pub worker_id: String,
    /// Confirmed set of models the server accepted from the worker's advertisement.
    pub models: Vec<String>,
    /// Non-fatal issues detected during registration (e.g. duplicate model names).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Protocol version the server will use for this connection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
}

/// An inference request dispatched from the server to a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMessage {
    /// Unique identifier for this request, used to correlate responses and cancellations.
    pub request_id: String,
    /// The model to use for this inference request.
    pub model: String,
    /// The HTTP path the worker should forward to its local backend (e.g. `/v1/chat/completions`).
    pub endpoint_path: String,
    /// Whether the client expects a streaming (SSE) response.
    pub is_streaming: bool,
    /// The raw JSON request body to forward to the local backend.
    pub body: String,
    /// HTTP headers to forward to the local backend.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: HeaderMap,
}

/// Instructs a worker to cancel an in-flight request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelMessage {
    /// The request to cancel.
    pub request_id: String,
    /// Why the request is being cancelled.
    pub reason: CancelReason,
}

/// The reason a request was cancelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    /// The original HTTP client disconnected before the response completed.
    ClientDisconnect,
    /// The request exceeded its allowed processing time.
    Timeout,
    /// The server is shutting down gracefully and draining in-flight work.
    GracefulShutdown,
    /// The assigned worker disconnected unexpectedly.
    WorkerDisconnect,
    /// The request was requeued too many times after worker failures.
    RequeueExhausted,
    /// The server is shutting down immediately.
    ServerShutdown,
    /// A cancellation reason not covered by the other variants.
    Other(String),
}

/// Server-initiated keepalive probe sent to workers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingMessage {
    /// Optional Unix timestamp in milliseconds for round-trip latency measurement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_unix_ms: Option<u64>,
}

/// Worker's reply to a [`PingMessage`], carrying current load information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PongMessage {
    /// Number of requests the worker is currently processing.
    pub current_load: u32,
    /// Echoed timestamp from the corresponding [`PingMessage`] for latency measurement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_unix_ms: Option<u64>,
}

/// Asks the worker to stop accepting new requests and finish in-flight work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GracefulShutdownMessage {
    /// Human-readable explanation for the shutdown (e.g. "maintenance").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// How many seconds the worker has to finish in-flight requests before being disconnected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drain_timeout_secs: Option<u64>,
}

/// Asks the worker to re-scan and re-advertise its available models.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelsRefreshMessage {
    /// Optional explanation for why a refresh was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Sent by a worker to update the server with its current model list and load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelsUpdateMessage {
    /// Current set of models the worker can serve.
    pub models: Vec<String>,
    /// Number of requests the worker is currently processing.
    pub current_load: u32,
}

/// One chunk of a streaming response being relayed back to the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseChunkMessage {
    /// The request this chunk belongs to.
    pub request_id: String,
    /// Raw SSE or body chunk data to forward to the waiting HTTP client.
    pub chunk: String,
}

/// Signals that the worker has finished processing a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseCompleteMessage {
    /// The request that completed.
    pub request_id: String,
    /// HTTP status code from the worker's local backend.
    pub status_code: u16,
    /// Response headers from the worker's local backend.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: HeaderMap,
    /// Full response body for non-streaming requests, or a final body segment for streaming ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Token usage statistics reported by the backend, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_counts: Option<TokenCounts>,
}

/// Token usage statistics for a completed inference request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    /// Number of tokens in the input prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    /// Number of tokens generated in the completion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    /// Total token count (prompt + completion).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

/// Reports an error from the worker, either for a specific request or a general fault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMessage {
    /// The request that caused the error, or `None` for general worker-level errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Machine-readable error code (e.g. `"upstream_error"`, `"model_not_found"`).
    pub code: String,
    /// Human-readable error description.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::{
        CancelMessage, CancelReason, ErrorMessage, GracefulShutdownMessage, HeaderMap,
        ModelsUpdateMessage, RegisterAck, RegisterMessage, RequestMessage, ResponseChunkMessage,
        ResponseCompleteMessage, ServerToWorkerMessage, TokenCounts, WorkerToServerMessage,
    };

    #[test]
    fn round_trips_register_ack_message() {
        let message = ServerToWorkerMessage::RegisterAck(RegisterAck {
            worker_id: "worker-123".to_string(),
            models: vec!["gpt-oss-120b".to_string(), "llama-4".to_string()],
            warnings: vec!["duplicate model dropped".to_string()],
            protocol_version: Some("2026-04-bridge-v1".to_string()),
        });

        let json = serde_json::to_string(&message).expect("serialize register ack");
        let decoded: ServerToWorkerMessage =
            serde_json::from_str(&json).expect("deserialize register ack");

        assert_eq!(decoded, message);
        assert!(json.contains("\"type\":\"register_ack\""));
    }

    #[test]
    fn round_trips_request_message_with_headers_and_raw_body() {
        let headers = HeaderMap::from([
            ("authorization".to_string(), "Bearer token".to_string()),
            ("x-trace-id".to_string(), "trace-123".to_string()),
        ]);
        let message = ServerToWorkerMessage::Request(RequestMessage {
            request_id: "req-42".to_string(),
            model: "gpt-oss-120b".to_string(),
            endpoint_path: "/v1/chat/completions".to_string(),
            is_streaming: true,
            body: r#"{"model":"gpt-oss-120b","stream":true}"#.to_string(),
            headers,
        });

        let json = serde_json::to_string(&message).expect("serialize request");
        let decoded: ServerToWorkerMessage =
            serde_json::from_str(&json).expect("deserialize request");

        assert_eq!(decoded, message);
        assert!(json.contains("\"endpoint_path\":\"/v1/chat/completions\""));
        assert!(json.contains("\"is_streaming\":true"));
    }

    #[test]
    fn round_trips_worker_messages_for_registration_and_updates() {
        let register = WorkerToServerMessage::Register(RegisterMessage {
            worker_name: "gpu-box-a".to_string(),
            models: vec!["gpt-oss-120b".to_string()],
            max_concurrent: 2,
            protocol_version: Some("2026-04-bridge-v1".to_string()),
            current_load: Some(1),
        });
        let update = WorkerToServerMessage::ModelsUpdate(ModelsUpdateMessage {
            models: vec!["gpt-oss-120b".to_string(), "qwen3".to_string()],
            current_load: 1,
        });

        let register_json = serde_json::to_string(&register).expect("serialize register");
        let update_json = serde_json::to_string(&update).expect("serialize models_update");

        let decoded_register: WorkerToServerMessage =
            serde_json::from_str(&register_json).expect("deserialize register");
        let decoded_update: WorkerToServerMessage =
            serde_json::from_str(&update_json).expect("deserialize models_update");

        assert_eq!(decoded_register, register);
        assert_eq!(decoded_update, update);
        assert!(register_json.contains("\"type\":\"register\""));
        assert!(update_json.contains("\"type\":\"models_update\""));
    }

    #[test]
    fn round_trips_response_messages() {
        let chunk = WorkerToServerMessage::ResponseChunk(ResponseChunkMessage {
            request_id: "req-42".to_string(),
            chunk: "data: {\"delta\":\"hello\"}\n\n".to_string(),
        });
        let complete = WorkerToServerMessage::ResponseComplete(ResponseCompleteMessage {
            request_id: "req-42".to_string(),
            status_code: 200,
            headers: HeaderMap::from([(
                "content-type".to_string(),
                "text/event-stream".to_string(),
            )]),
            body: Some("{\"done\":true}".to_string()),
            token_counts: Some(TokenCounts {
                prompt_tokens: Some(11),
                completion_tokens: Some(7),
                total_tokens: Some(18),
            }),
        });

        let chunk_json = serde_json::to_string(&chunk).expect("serialize response chunk");
        let complete_json = serde_json::to_string(&complete).expect("serialize response complete");

        let decoded_chunk: WorkerToServerMessage =
            serde_json::from_str(&chunk_json).expect("deserialize response chunk");
        let decoded_complete: WorkerToServerMessage =
            serde_json::from_str(&complete_json).expect("deserialize response complete");

        assert_eq!(decoded_chunk, chunk);
        assert_eq!(decoded_complete, complete);
        assert!(complete_json.contains("\"status_code\":200"));
    }

    #[test]
    fn round_trips_control_and_error_messages() {
        let cancel = ServerToWorkerMessage::Cancel(CancelMessage {
            request_id: "req-42".to_string(),
            reason: CancelReason::ClientDisconnect,
        });
        let shutdown = ServerToWorkerMessage::GracefulShutdown(GracefulShutdownMessage {
            reason: Some("maintenance".to_string()),
            drain_timeout_secs: Some(30),
        });
        let error = WorkerToServerMessage::Error(ErrorMessage {
            request_id: Some("req-42".to_string()),
            code: "upstream_error".to_string(),
            message: "worker backend disconnected".to_string(),
        });

        let cancel_json = serde_json::to_string(&cancel).expect("serialize cancel");
        let shutdown_json = serde_json::to_string(&shutdown).expect("serialize shutdown");
        let error_json = serde_json::to_string(&error).expect("serialize error");

        let decoded_cancel: ServerToWorkerMessage =
            serde_json::from_str(&cancel_json).expect("deserialize cancel");
        let decoded_shutdown: ServerToWorkerMessage =
            serde_json::from_str(&shutdown_json).expect("deserialize shutdown");
        let decoded_error: WorkerToServerMessage =
            serde_json::from_str(&error_json).expect("deserialize error");

        assert_eq!(decoded_cancel, cancel);
        assert_eq!(decoded_shutdown, shutdown);
        assert_eq!(decoded_error, error);
        assert!(cancel_json.contains("\"reason\":\"client_disconnect\""));
    }
}
