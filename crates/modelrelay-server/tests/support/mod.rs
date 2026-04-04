use std::sync::Arc;

use futures_util::StreamExt;
use modelrelay_server::{ProxyServerCore, RequestState};
use tokio::{io::AsyncReadExt, net::TcpStream, sync::Mutex, time::timeout};
use tokio_tungstenite::tungstenite::Message;
use modelrelay_protocol::{CancelMessage, CancelReason, ServerToWorkerMessage};

pub type TestSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn next_server_message(socket: &mut TestSocket, context: &str) -> ServerToWorkerMessage {
    loop {
        let message = timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .unwrap_or_else(|_| panic!("receive {context} before timeout"))
            .expect("socket message")
            .expect("websocket message");
        let Message::Text(payload) = message else {
            panic!("expected text {context}");
        };

        let server_message =
            serde_json::from_str(&payload).unwrap_or_else(|_| panic!("deserialize {context}"));
        if matches!(server_message, ServerToWorkerMessage::Ping(_)) {
            continue;
        }

        return server_message;
    }
}

pub async fn next_close_frame(
    socket: &mut TestSocket,
    context: &str,
) -> tokio_tungstenite::tungstenite::protocol::CloseFrame {
    let message = timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .unwrap_or_else(|_| panic!("receive {context} before timeout"))
        .expect("socket message")
        .expect("websocket message");
    let Message::Close(Some(close_frame)) = message else {
        panic!("expected close frame for {context}");
    };

    close_frame.clone()
}

pub async fn read_until_contains(stream: &mut TcpStream, needle: &str) -> String {
    let mut response = Vec::new();

    loop {
        if String::from_utf8_lossy(&response).contains(needle) {
            return String::from_utf8(response).expect("http response is utf8");
        }

        let mut chunk = [0_u8; 1024];
        let read = timeout(std::time::Duration::from_secs(2), stream.read(&mut chunk))
            .await
            .expect("read response chunk before timeout")
            .expect("read response chunk");
        assert!(read > 0, "response closed before expected bytes arrived");
        response.extend_from_slice(&chunk[..read]);
    }
}

pub async fn read_http_response(stream: &mut TcpStream) -> String {
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read http response");
    String::from_utf8(response).expect("http response is utf8")
}

pub async fn begin_graceful_shutdown(core: &Arc<Mutex<ProxyServerCore>>) {
    let mut core = core.lock().await;
    let signals = core.begin_graceful_shutdown(
        Some("proxy server shutting down"),
        std::time::Duration::from_secs(1),
    );
    assert_eq!(signals.len(), 1);
    assert!(core.worker_is_draining("worker-1"));
}

pub async fn assert_graceful_shutdown_signal(socket: &mut TestSocket) {
    assert_eq!(
        next_server_message(socket, "graceful shutdown").await,
        ServerToWorkerMessage::GracefulShutdown(modelrelay_protocol::GracefulShutdownMessage {
            reason: Some("proxy server shutting down".to_string()),
            drain_timeout_secs: Some(1),
        })
    );
}

pub async fn assert_draining_worker_stays_idle(socket: &mut TestSocket) {
    for _ in 0..2 {
        let Ok(Some(message)) = timeout(std::time::Duration::from_millis(60), socket.next()).await
        else {
            continue;
        };
        let message = message.expect("websocket message while draining");
        let Message::Text(payload) = message else {
            panic!("expected only text heartbeat messages before drain completion");
        };
        match serde_json::from_str::<ServerToWorkerMessage>(&payload)
            .expect("deserialize server message while draining")
        {
            ServerToWorkerMessage::Ping(_) => {}
            ServerToWorkerMessage::Request(_) => {
                panic!("queued work should not dispatch to a draining worker");
            }
            _ => panic!("unexpected server message while worker is draining"),
        }
    }
}

pub async fn assert_post_drain_close(socket: &mut TestSocket) {
    let close_message = timeout(std::time::Duration::from_secs(2), socket.next())
        .await
        .expect("receive close frame before timeout")
        .expect("socket message")
        .expect("websocket message");
    let Message::Close(Some(close_frame)) = close_message else {
        panic!("expected post-drain close frame");
    };
    assert_eq!(u16::from(close_frame.code), 1000);
    assert_eq!(close_frame.reason, "graceful shutdown complete");
}

pub async fn assert_worker_socket_closes(socket: &mut TestSocket) {
    loop {
        let message = timeout(std::time::Duration::from_secs(2), socket.next())
            .await
            .expect("receive worker close frame before timeout")
            .expect("socket message");

        // On Windows, the OS may deliver a ConnectionAborted (error 10053) or
        // ConnectionReset instead of a clean WebSocket close frame when the
        // server tears down the connection. Treat these IO errors as equivalent
        // to a successful close.
        let message = match message {
            Ok(msg) => msg,
            Err(tokio_tungstenite::tungstenite::Error::Io(ref e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                return;
            }
            Err(e) => panic!("unexpected websocket error while waiting for close: {e}"),
        };

        match message {
            Message::Close(Some(close_frame)) => {
                assert_eq!(u16::from(close_frame.code), 1000);
                return;
            }
            Message::Close(None) => {
                return;
            }
            Message::Text(payload) => match serde_json::from_str::<ServerToWorkerMessage>(&payload)
                .expect("deserialize server message while waiting for close")
            {
                ServerToWorkerMessage::Ping(_) => {}
                other => panic!("unexpected server message while waiting for close: {other:?}"),
            },
            other => panic!("unexpected websocket message while waiting for close: {other:?}"),
        }
    }
}

pub async fn assert_worker_emits_single_timeout_cancel_and_stays_idle(
    socket: &mut TestSocket,
    request_id: &str,
) {
    assert_eq!(
        next_server_message(socket, "worker timeout cancel").await,
        ServerToWorkerMessage::Cancel(CancelMessage {
            request_id: request_id.to_string(),
            reason: CancelReason::Timeout,
        })
    );

    for _ in 0..2 {
        let Ok(Some(message)) = timeout(std::time::Duration::from_millis(60), socket.next()).await
        else {
            continue;
        };
        let message = message.expect("websocket message while waiting for timed-out request");
        let Message::Text(payload) = message else {
            panic!("expected only text heartbeat messages while timed-out request is uncleared");
        };
        match serde_json::from_str::<ServerToWorkerMessage>(&payload)
            .expect("deserialize server message while waiting for timed-out request")
        {
            ServerToWorkerMessage::Ping(_) => {}
            ServerToWorkerMessage::Cancel(_) => {
                panic!("timed-out request should emit exactly one worker cancel");
            }
            ServerToWorkerMessage::Request(_) => {
                panic!("queued follow-up should remain queued until timed-out request clears");
            }
            _ => panic!("unexpected server message while timed-out request is uncleared"),
        }
    }
}

pub async fn wait_for_request_state(
    core: &Arc<Mutex<ProxyServerCore>>,
    request_id: &str,
    expected: RequestState,
) {
    timeout(std::time::Duration::from_secs(2), async {
        loop {
            {
                let core = core.lock().await;
                if core.request_state(request_id) == Some(expected.clone()) {
                    return;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("request {request_id} did not reach expected state"));
}
