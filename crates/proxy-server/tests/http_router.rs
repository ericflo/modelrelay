use std::{net::SocketAddr, sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use proxy_server::{
    MODELS_PATH, ProxyServerApp, ProxyServerCore, WORKER_CONNECT_PATH, WorkerSocketProviderConfig,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::Mutex, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use worker_protocol::{RegisterMessage, ServerToWorkerMessage, WorkerToServerMessage};

type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn spawn_server() -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let app = ProxyServerApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"))
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve composite proxy app");
    });

    (addr, core)
}

fn models_url(addr: SocketAddr) -> String {
    format!("http://{addr}{MODELS_PATH}")
}

fn worker_connect_request(addr: SocketAddr, secret: &str) -> http::Request<()> {
    let mut request = format!("ws://{addr}{WORKER_CONNECT_PATH}?provider=openai")
        .into_client_request()
        .expect("build websocket request");
    request.headers_mut().insert(
        "x-worker-secret",
        secret.parse().expect("parse worker secret header"),
    );
    request
}

async fn fetch_models(addr: SocketAddr) -> Value {
    reqwest::get(models_url(addr))
        .await
        .expect("send models request")
        .error_for_status()
        .expect("models request should succeed")
        .json()
        .await
        .expect("deserialize models response")
}

fn models_json(models: &[&str]) -> Value {
    json!({
        "object": "list",
        "data": models
            .iter()
            .map(|model| json!({
                "id": model,
                "object": "model",
                "owned_by": "worker-proxy"
            }))
            .collect::<Vec<_>>()
    })
}

fn register_message(worker_name: &str, models: &[&str]) -> WorkerToServerMessage {
    WorkerToServerMessage::Register(RegisterMessage {
        worker_name: worker_name.to_string(),
        models: models.iter().map(|model| (*model).to_string()).collect(),
        max_concurrent: 1,
        protocol_version: None,
        current_load: Some(0),
    })
}

async fn next_non_heartbeat_text_message(socket: &mut TestSocket, context: &str) -> String {
    loop {
        let message = timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap_or_else(|_| panic!("receive {context} before timeout"))
            .expect("socket message")
            .expect("websocket message");
        let Message::Text(payload) = message else {
            panic!("expected text {context}");
        };

        if !matches!(
            serde_json::from_str::<ServerToWorkerMessage>(&payload),
            Ok(ServerToWorkerMessage::Ping(_))
        ) {
            return payload.to_string();
        }
    }
}

async fn register_core_worker(
    core: &Arc<Mutex<ProxyServerCore>>,
    worker_name: &str,
    models: &[&str],
) -> String {
    let mut core = core.lock().await;
    core.register_worker(
        "openai",
        RegisterMessage {
            worker_name: worker_name.to_string(),
            models: models.iter().map(|model| (*model).to_string()).collect(),
            max_concurrent: 1,
            protocol_version: None,
            current_load: Some(0),
        },
    )
    .worker_id
}

#[tokio::test]
async fn models_endpoint_returns_openai_catalog_shape_for_live_registry_entries() {
    let (addr, core) = spawn_server().await;

    register_core_worker(
        &core,
        "alpha",
        &["llama-3.1-70b", "mistral-large", "llama-3.1-70b"],
    )
    .await;
    register_core_worker(&core, "beta", &["mistral-large", "gpt-oss-120b"]).await;

    assert_eq!(
        fetch_models(addr).await,
        models_json(&["llama-3.1-70b", "mistral-large", "gpt-oss-120b"])
    );
}

#[tokio::test]
async fn models_endpoint_tracks_updates_and_disconnects_without_stale_entries() {
    let (addr, core) = spawn_server().await;
    let first_worker_id = register_core_worker(&core, "alpha", &["llama-3.1-70b"]).await;
    let second_worker_id = register_core_worker(&core, "beta", &["mistral-large"]).await;

    assert_eq!(
        fetch_models(addr).await,
        models_json(&["llama-3.1-70b", "mistral-large"])
    );

    {
        let mut core = core.lock().await;
        assert!(
            core.update_worker_models(
                &first_worker_id,
                worker_protocol::ModelsUpdateMessage {
                    models: vec!["llama-3.1-70b".to_string(), "gpt-oss-120b".to_string()],
                    current_load: 0,
                },
            )
            .is_empty()
        );
        assert!(
            core.update_worker_models(
                &second_worker_id,
                worker_protocol::ModelsUpdateMessage {
                    models: vec!["gpt-oss-120b".to_string()],
                    current_load: 0,
                },
            )
            .is_empty()
        );
    }

    assert_eq!(
        fetch_models(addr).await,
        models_json(&["llama-3.1-70b", "gpt-oss-120b"])
    );

    {
        let mut core = core.lock().await;
        assert!(core.disconnect_worker(&first_worker_id).is_some());
    }

    assert_eq!(fetch_models(addr).await, models_json(&["gpt-oss-120b"]));
}

#[tokio::test]
async fn worker_connect_route_stays_live_when_mounted_with_models_router() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect to worker websocket");

    socket
        .send(Message::Text(
            serde_json::to_string(&register_message("mounted-worker", &["llama-3.1-70b"]))
                .expect("serialize register message")
                .into(),
        ))
        .await
        .expect("send worker register");

    let ack = serde_json::from_str::<ServerToWorkerMessage>(
        &next_non_heartbeat_text_message(&mut socket, "register ack").await,
    )
    .expect("deserialize register ack");
    assert!(matches!(ack, ServerToWorkerMessage::RegisterAck(_)));

    {
        let core = core.lock().await;
        assert_eq!(
            core.provider_models("openai"),
            vec!["llama-3.1-70b".to_string()]
        );
    }

    assert_eq!(fetch_models(addr).await, models_json(&["llama-3.1-70b"]));
}
