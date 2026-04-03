use std::sync::Arc;

use clap::Parser;
use proxy_server::{
    ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig,
};
use tokio::sync::Mutex;

/// Remote LLM worker proxy server.
///
/// Accepts provider-compatible inference requests and routes them to remote
/// workers over WebSocket, where those workers speak to local model servers.
#[derive(Parser, Debug)]
#[command(name = "proxy-server", version)]
struct Args {
    /// Address to listen on
    #[arg(long, env = "LISTEN_ADDR", default_value = "127.0.0.1:8080")]
    listen: String,

    /// Provider name used for worker routing and request dispatch
    #[arg(long, env = "PROVIDER_NAME", default_value = "local")]
    provider: String,

    /// Secret that workers must present to authenticate (required)
    #[arg(long, env = "WORKER_SECRET")]
    worker_secret: String,

    /// Maximum number of requests that may be queued (0 = unlimited)
    #[arg(long, env = "MAX_QUEUE_LEN", default_value = "100")]
    max_queue_len: usize,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));

    let max_queue_len = if args.max_queue_len == 0 {
        usize::MAX
    } else {
        args.max_queue_len
    };

    {
        let mut guard = core.lock().await;
        guard.configure_provider_queue(
            &args.provider,
            ProviderQueuePolicy {
                max_queue_len,
                queue_timeout_ticks: None,
            },
        );
    }

    let worker_socket_app = WorkerSocketApp::new(Arc::clone(&core)).with_provider(
        &args.provider,
        WorkerSocketProviderConfig::enabled(&args.worker_secret),
    );

    let http_app = ProxyHttpApp::new(Arc::clone(&core))
        .with_models_provider(&args.provider)
        .with_worker_socket_app(worker_socket_app);

    let router = http_app.router();

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind to {}: {e}", args.listen));

    eprintln!("proxy-server listening on {}", args.listen);

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = sigterm => {},
    }

    eprintln!("shutting down gracefully");
}
