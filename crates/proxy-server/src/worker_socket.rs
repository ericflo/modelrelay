use std::{collections::HashMap, sync::Arc};

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
use worker_protocol::{ServerToWorkerMessage, WorkerToServerMessage};

use crate::ProxyServerCore;

const WORKER_SECRET_HEADER: &str = "x-worker-secret";
const CLOSE_REASON_AUTH_FAILED: &str = "worker authentication failed";
const CLOSE_REASON_PROTOCOL_ERROR: &str = "worker registration protocol error";
const CLOSE_CODE_POLICY_VIOLATION: u16 = 1008;
const CLOSE_CODE_PROTOCOL_ERROR: u16 = 1002;

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
    #[must_use]
    pub fn new(core: Arc<Mutex<ProxyServerCore>>) -> Self {
        Self {
            state: WorkerSocketState {
                core,
                providers: HashMap::new(),
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
            .route("/v1/worker/connect", get(worker_connect_handler))
            .with_state(self.state)
    }
}

#[derive(Clone)]
struct WorkerSocketState {
    core: Arc<Mutex<ProxyServerCore>>,
    providers: HashMap<String, WorkerSocketProviderConfig>,
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

async fn worker_connect_handler(
    ws: WebSocketUpgrade,
    State(state): State<WorkerSocketState>,
    Query(query): Query<WorkerConnectQuery>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    match authenticate_connection(&state, &headers, &query) {
        AuthOutcome::Authenticated { provider } => ws.on_upgrade(move |socket| async move {
            handle_authenticated_socket(socket, state, provider).await;
        }),
        AuthOutcome::Rejected { reason } => ws.on_upgrade(move |socket| async move {
            close_socket(socket, CLOSE_CODE_POLICY_VIOLATION, reason).await;
        }),
    }
}

fn authenticate_connection(
    state: &WorkerSocketState,
    headers: &axum::http::HeaderMap,
    query: &WorkerConnectQuery,
) -> AuthOutcome {
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
        return AuthOutcome::Rejected {
            reason: CLOSE_REASON_AUTH_FAILED,
        };
    }

    AuthOutcome::Authenticated {
        provider: provider.to_string(),
    }
}

async fn handle_authenticated_socket(
    mut socket: WebSocket,
    state: WorkerSocketState,
    provider: String,
) {
    let Some(Ok(Message::Text(payload))) = socket.recv().await else {
        close_socket(
            socket,
            CLOSE_CODE_PROTOCOL_ERROR,
            CLOSE_REASON_PROTOCOL_ERROR,
        )
        .await;
        return;
    };

    let Ok(WorkerToServerMessage::Register(register)) = serde_json::from_str(&payload) else {
        close_socket(
            socket,
            CLOSE_CODE_PROTOCOL_ERROR,
            CLOSE_REASON_PROTOCOL_ERROR,
        )
        .await;
        return;
    };

    let ack = {
        let mut core = state.core.lock().await;
        core.register_worker(provider, register).ack
    };
    let response = ServerToWorkerMessage::RegisterAck(ack);

    let Ok(payload) = serde_json::to_string(&response) else {
        close_socket(
            socket,
            CLOSE_CODE_PROTOCOL_ERROR,
            CLOSE_REASON_PROTOCOL_ERROR,
        )
        .await;
        return;
    };

    let _ = socket.send(Message::Text(payload.into())).await;
}

async fn close_socket(mut socket: WebSocket, code: u16, reason: &'static str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}
