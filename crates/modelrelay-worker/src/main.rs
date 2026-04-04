use clap::Parser;
use tracing_subscriber::EnvFilter;
use modelrelay_worker::{WorkerDaemon, WorkerDaemonConfig};

/// Remote LLM worker daemon.
///
/// Connects to a central proxy server over WebSocket and forwards inference
/// requests to a local model backend (e.g. llama-server).
#[derive(Parser, Debug)]
#[command(name = "modelrelay-worker", version)]
struct Args {
    /// Base URL of the proxy server (e.g. `http://127.0.0.1:8080`)
    #[arg(long, env = "PROXY_URL", default_value = "http://127.0.0.1:8080")]
    proxy_url: String,

    /// Provider name to register with on the proxy
    #[arg(long, env = "PROVIDER_NAME", default_value = "local")]
    provider: String,

    /// Secret used to authenticate with the proxy (required)
    #[arg(long, env = "WORKER_SECRET")]
    worker_secret: String,

    /// Human-readable name for this worker instance
    #[arg(long, env = "WORKER_NAME", default_value = "worker")]
    worker_name: String,

    /// Comma-separated list of model names this worker supports
    #[arg(long, env = "MODELS", default_value = "default")]
    models: String,

    /// Maximum number of concurrent requests this worker will handle
    #[arg(long, env = "MAX_CONCURRENCY", default_value = "1")]
    max_concurrency: u32,

    /// Base URL of the local model backend (e.g. `http://127.0.0.1:8000`)
    #[arg(long, env = "BACKEND_URL", default_value = "http://127.0.0.1:8000")]
    backend_url: String,

    /// Log level filter (e.g. info, debug, warn, error, or a directive like
    /// `modelrelay_worker=debug`). Overridden by `RUST_LOG` if set.
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&args.log_level)),
        )
        .init();

    let models: Vec<String> = args
        .models
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let config = WorkerDaemonConfig {
        proxy_base_url: args.proxy_url.clone(),
        provider: args.provider.clone(),
        worker_secret: args.worker_secret.clone(),
        worker_name: args.worker_name.clone(),
        models: models.clone(),
        max_concurrent: args.max_concurrency,
        backend_base_url: args.backend_url.clone(),
    };

    tracing::info!(
        proxy = %args.proxy_url,
        provider = %args.provider,
        name = %args.worker_name,
        models = %models.join(","),
        concurrency = args.max_concurrency,
        backend = %args.backend_url,
        "modelrelay-worker starting"
    );

    let daemon = WorkerDaemon::new(config);

    tokio::select! {
        result = daemon.run_with_reconnect() => {
            if let Err(e) = result {
                tracing::error!(error = %e, "modelrelay-worker error");
                std::process::exit(1);
            }
        }
        () = shutdown_signal() => {
            tracing::info!("modelrelay-worker shutting down gracefully");
        }
    }
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
