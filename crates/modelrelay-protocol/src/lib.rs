use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type HeaderMap = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerToWorkerMessage {
    RegisterAck(RegisterAck),
    Request(RequestMessage),
    Cancel(CancelMessage),
    Ping(PingMessage),
    GracefulShutdown(GracefulShutdownMessage),
    ModelsRefresh(ModelsRefreshMessage),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerToServerMessage {
    Register(RegisterMessage),
    ModelsUpdate(ModelsUpdateMessage),
    ResponseChunk(ResponseChunkMessage),
    ResponseComplete(ResponseCompleteMessage),
    Pong(PongMessage),
    Error(ErrorMessage),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterMessage {
    pub worker_name: String,
    pub models: Vec<String>,
    pub max_concurrent: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_load: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterAck {
    pub worker_id: String,
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMessage {
    pub request_id: String,
    pub model: String,
    pub endpoint_path: String,
    pub is_streaming: bool,
    pub body: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: HeaderMap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelMessage {
    pub request_id: String,
    pub reason: CancelReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelReason {
    ClientDisconnect,
    Timeout,
    GracefulShutdown,
    WorkerDisconnect,
    RequeueExhausted,
    ServerShutdown,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PongMessage {
    pub current_load: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GracefulShutdownMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drain_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelsRefreshMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelsUpdateMessage {
    pub models: Vec<String>,
    pub current_load: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseChunkMessage {
    pub request_id: String,
    pub chunk: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseCompleteMessage {
    pub request_id: String,
    pub status_code: u16,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: HeaderMap,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_counts: Option<TokenCounts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCounts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub code: String,
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
