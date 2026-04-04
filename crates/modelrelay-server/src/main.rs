use std::sync::Arc;
use std::time::Duration;

use axum::{middleware, response::IntoResponse};
use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use modelrelay_server::{
    ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore, WorkerSocketApp, WorkerSocketProviderConfig,
};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

/// Remote LLM worker proxy server.
///
/// Accepts provider-compatible inference requests and routes them to remote
/// workers over WebSocket, where those workers speak to local model servers.
#[derive(Parser, Debug)]
#[command(name = "modelrelay-server", version)]
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

    /// Seconds before a queued request times out (0 = no timeout)
    #[arg(long, env = "QUEUE_TIMEOUT_SECS", default_value = "30")]
    queue_timeout: u64,

    /// Seconds before an in-flight HTTP request times out (0 = no timeout)
    #[arg(long, env = "REQUEST_TIMEOUT_SECS", default_value = "300")]
    request_timeout: u64,

    /// Log level filter (e.g. info, debug, warn, error, or a directive like
    /// `modelrelay_server=debug`). Overridden by `RUST_LOG` if set.
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// Generate shell completion script for the given shell and exit
    #[arg(long, value_name = "SHELL", hide = true)]
    completions: Option<Shell>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    if let Some(shell) = args.completions {
        generate(
            shell,
            &mut Args::command(),
            "modelrelay-server",
            &mut std::io::stdout(),
        );
        return;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level)),
        )
        .init();

    let core = Arc::new(Mutex::new(ProxyServerCore::new()));

    let max_queue_len = if args.max_queue_len == 0 {
        usize::MAX
    } else {
        args.max_queue_len
    };

    let queue_timeout_ticks = if args.queue_timeout == 0 {
        None
    } else {
        Some(args.queue_timeout)
    };

    {
        let mut guard = core.lock().await;
        guard.configure_provider_queue(
            &args.provider,
            ProviderQueuePolicy {
                max_queue_len,
                queue_timeout_ticks,
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

    let mut router = http_app.router();

    if args.request_timeout > 0 {
        let timeout = Duration::from_secs(args.request_timeout);
        router = router.layer(middleware::from_fn(
            move |req: axum::extract::Request, next: middleware::Next| async move {
                match tokio::time::timeout(timeout, next.run(req)).await {
                    Ok(response) => response,
                    Err(_) => axum::http::StatusCode::REQUEST_TIMEOUT.into_response(),
                }
            },
        ));
    }

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind to {}: {e}", args.listen));

    let queue_timeout_display = if args.queue_timeout == 0 {
        "none".to_string()
    } else {
        format!("{}s", args.queue_timeout)
    };
    let request_timeout_display = if args.request_timeout == 0 {
        "none".to_string()
    } else {
        format!("{}s", args.request_timeout)
    };
    let max_queue_display = if args.max_queue_len == 0 {
        "unlimited".to_string()
    } else {
        args.max_queue_len.to_string()
    };
    tracing::info!(
        listen = %args.listen,
        provider = %args.provider,
        queue_timeout = %queue_timeout_display,
        request_timeout = %request_timeout_display,
        max_queue_len = %max_queue_display,
        "modelrelay-server starting"
    );

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

    tracing::info!("shutting down gracefully");
}
