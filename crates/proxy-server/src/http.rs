use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::{
        HeaderMap as AxumHeaderMap, HeaderName, HeaderValue, StatusCode,
        header::{CONNECTION, CONTENT_LENGTH, HOST},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use worker_protocol::{HeaderMap, ResponseCompleteMessage};

use crate::{CancelReason, PendingHttpResponse, ProxyServerCore, WorkerSocketApp};

const OPENAI_MODELS_PROVIDER: &str = "openai";

#[derive(Clone)]
pub struct ProxyHttpApp {
    core: Arc<Mutex<ProxyServerCore>>,
    models_provider: String,
    worker_socket_app: Option<WorkerSocketApp>,
}

impl ProxyHttpApp {
    #[must_use]
    pub fn new(core: Arc<Mutex<ProxyServerCore>>) -> Self {
        Self {
            core,
            models_provider: OPENAI_MODELS_PROVIDER.to_string(),
            worker_socket_app: None,
        }
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

    pub fn router(self) -> Router {
        let router = Router::new()
            .route("/v1/models", get(models_handler))
            .route("/v1/chat/completions", post(chat_completions_handler))
            .route("/v1/messages", post(messages_handler))
            .route("/v1/responses", post(responses_handler))
            .with_state(HttpState {
                core: self.core,
                models_provider: self.models_provider,
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
    let Ok(request) = serde_json::from_str::<ChatCompletionsRequest>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    if request.stream {
        return StatusCode::NOT_IMPLEMENTED.into_response();
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
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    };

    let mut cancellation_guard = HttpRequestCancellationGuard::new(state.core.clone(), &pending);
    match pending.response_rx.await {
        Ok(response) => {
            cancellation_guard.disarm();
            response_complete_to_http_response(response)
        }
        Err(_) => StatusCode::BAD_GATEWAY.into_response(),
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
