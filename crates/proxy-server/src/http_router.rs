use std::sync::Arc;

use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::{ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig};

pub const MODELS_PATH: &str = "/v1/models";
const MODELS_OWNER: &str = "worker-proxy";
const DEFAULT_MODELS_PROVIDER: &str = "openai";

#[derive(Clone)]
pub struct ProxyServerApp {
    core: Arc<Mutex<ProxyServerCore>>,
    worker_socket_app: WorkerSocketApp,
    models_provider: String,
}

impl ProxyServerApp {
    #[must_use]
    pub fn new(core: Arc<Mutex<ProxyServerCore>>) -> Self {
        Self {
            core: core.clone(),
            worker_socket_app: WorkerSocketApp::new(core),
            models_provider: DEFAULT_MODELS_PROVIDER.to_string(),
        }
    }

    #[must_use]
    pub fn with_provider(
        mut self,
        provider: impl Into<String>,
        config: WorkerSocketProviderConfig,
    ) -> Self {
        self.worker_socket_app = self.worker_socket_app.with_provider(provider, config);
        self
    }

    #[must_use]
    pub fn with_models_provider(mut self, provider: impl Into<String>) -> Self {
        self.models_provider = provider.into();
        self
    }

    pub fn router(self) -> Router {
        models_router(self.core, self.models_provider).merge(self.worker_socket_app.router())
    }
}

#[derive(Clone)]
struct ModelsState {
    core: Arc<Mutex<ProxyServerCore>>,
    provider: String,
}

fn models_router(core: Arc<Mutex<ProxyServerCore>>, provider: String) -> Router {
    Router::new()
        .route(MODELS_PATH, get(models_handler))
        .with_state(ModelsState { core, provider })
}

async fn models_handler(State(state): State<ModelsState>) -> Json<ModelsResponse> {
    let models = {
        let core = state.core.lock().await;
        core.provider_models(&state.provider)
    };

    Json(ModelsResponse {
        object: "list",
        data: models
            .into_iter()
            .map(|id| ModelCard {
                id,
                object: "model",
                owned_by: MODELS_OWNER,
            })
            .collect(),
    })
}

#[derive(Debug, Serialize)]
struct ModelsResponse {
    object: &'static str,
    data: Vec<ModelCard>,
}

#[derive(Debug, Serialize)]
struct ModelCard {
    id: String,
    object: &'static str,
    owned_by: &'static str,
}
