use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::{ProxyServerCore, WorkerSocketApp};

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
