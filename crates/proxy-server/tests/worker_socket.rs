use std::{net::SocketAddr, sync::Arc};

use futures_util::{SinkExt, StreamExt};
use proxy_server::{ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig};
use tokio::{net::TcpListener, sync::Mutex, time::timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use worker_protocol::{RegisterMessage, ServerToWorkerMessage, WorkerToServerMessage};

async fn spawn_server() -> (SocketAddr, Arc<Mutex<ProxyServerCore>>) {
    let core = Arc::new(Mutex::new(ProxyServerCore::new()));
    let app = WorkerSocketApp::new(core.clone())
        .with_provider("openai", WorkerSocketProviderConfig::enabled("top-secret"))
        .router();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener local addr");

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve worker socket app");
    });

    (addr, core)
}

fn worker_connect_request(addr: SocketAddr, secret: &str) -> http::Request<()> {
    let mut request = format!("ws://{addr}/v1/worker/connect?provider=openai")
        .into_client_request()
        .expect("build websocket request");
    request.headers_mut().insert(
        "x-worker-secret",
        secret.parse().expect("parse worker secret header"),
    );
    request
}

#[tokio::test]
async fn authenticated_worker_can_register_and_receive_register_ack() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "top-secret"))
        .await
        .expect("connect websocket");

    let register = WorkerToServerMessage::Register(RegisterMessage {
        worker_name: "gpu-box-a".to_string(),
        models: vec!["llama-3.1-70b".to_string(), " mistral-large ".to_string()],
        max_concurrent: 2,
        protocol_version: Some("2026-04-bridge-v1".to_string()),
        current_load: Some(0),
    });
    let register_payload = serde_json::to_string(&register).expect("serialize register");

    socket
        .send(Message::Text(register_payload.into()))
        .await
        .expect("send register");

    let ack_message = timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("receive register_ack before timeout")
        .expect("socket message")
        .expect("websocket message");
    let Message::Text(ack_payload) = ack_message else {
        panic!("expected text register_ack");
    };

    let ack = serde_json::from_str::<ServerToWorkerMessage>(&ack_payload)
        .expect("deserialize register ack");
    assert_eq!(
        ack,
        ServerToWorkerMessage::RegisterAck(worker_protocol::RegisterAck {
            worker_id: "worker-1".to_string(),
            models: vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()],
            warnings: Vec::new(),
            protocol_version: Some("2026-04-bridge-v1".to_string()),
        })
    );

    let core = core.lock().await;
    assert_eq!(
        core.provider_models("openai"),
        vec!["llama-3.1-70b".to_string(), "mistral-large".to_string()]
    );
}

#[tokio::test]
async fn rejected_auth_connection_is_closed_with_policy_violation() {
    let (addr, core) = spawn_server().await;
    let (mut socket, _) = connect_async(worker_connect_request(addr, "wrong-secret"))
        .await
        .expect("connect websocket");

    let close_message = timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("receive auth close before timeout")
        .expect("socket message")
        .expect("websocket message");
    let Message::Close(Some(close_frame)) = close_message else {
        panic!("expected auth rejection close frame");
    };

    assert_eq!(u16::from(close_frame.code), 1008);
    assert_eq!(close_frame.reason, "worker authentication failed");

    let core = core.lock().await;
    assert!(core.provider_models("openai").is_empty());
}
