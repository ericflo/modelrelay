use std::path::PathBuf;
use std::time::Duration;

use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use modelrelay_worker::{WorkerDaemon, WorkerDaemonConfig};
use serde::Deserialize;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

/// Configuration file format for `--config config.toml`.
///
/// All fields are optional — CLI flags and env vars override any values
/// loaded from the file.
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    proxy_url: Option<String>,
    provider: Option<String>,
    worker_secret: Option<String>,
    worker_name: Option<String>,
    models: Option<Vec<String>>,
    max_concurrency: Option<u32>,
    backend_url: Option<String>,
    log_level: Option<String>,
    endpoint_prefixes: Option<Vec<String>>,
    models_poll_interval_secs: Option<u64>,
}

/// Remote LLM worker daemon.
///
/// Connects to a central proxy server over WebSocket and forwards inference
/// requests to a local model backend (e.g. llama-server).
///
/// Configuration can be provided via CLI flags, environment variables, or a
/// TOML config file (with `--config`). Precedence: CLI flags > env vars >
/// config file > defaults.
#[derive(Parser, Debug)]
#[command(name = "modelrelay-worker", version)]
struct Args {
    /// Path to a TOML configuration file
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Base URL of the proxy server (e.g. `https://api.modelrelay.io`)
    #[arg(long, env = "PROXY_URL")]
    proxy_url: Option<String>,

    /// Provider name to register with on the proxy
    #[arg(long, env = "PROVIDER_NAME")]
    provider: Option<String>,

    /// Secret used to authenticate with the proxy (required)
    #[arg(long, env = "WORKER_SECRET")]
    worker_secret: Option<String>,

    /// Human-readable name for this worker instance
    #[arg(long, env = "WORKER_NAME")]
    worker_name: Option<String>,

    /// Comma-separated list of model names this worker supports
    #[arg(long, env = "MODELS")]
    models: Option<String>,

    /// Maximum number of concurrent requests this worker will handle
    #[arg(long, env = "MAX_CONCURRENCY")]
    max_concurrency: Option<u32>,

    /// Base URL of the local model backend (e.g. `http://localhost:1234`)
    #[arg(long, env = "BACKEND_URL")]
    backend_url: Option<String>,

    /// Log level filter (e.g. info, debug, warn, error, or a directive like
    /// `modelrelay_worker=debug`). Overridden by `RUST_LOG` if set.
    #[arg(long, env = "LOG_LEVEL")]
    log_level: Option<String>,

    /// Comma-separated list of endpoint path prefixes this worker supports
    /// (e.g. `/v1/chat,/v1/messages`). Empty means accept any endpoint.
    #[arg(long, env = "ENDPOINT_PREFIXES")]
    endpoint_prefixes: Option<String>,

    /// Seconds between periodic `/v1/models` polls when `models` contains `*` (0 = disabled)
    #[arg(long, env = "MODELS_POLL_INTERVAL_SECS")]
    models_poll_interval: Option<u64>,

    /// Generate shell completion script for the given shell and exit
    #[arg(long, value_name = "SHELL", hide = true)]
    completions: Option<Shell>,
}

/// Load and parse a TOML config file, returning an error message on failure.
fn load_config_file(path: &PathBuf) -> Result<FileConfig, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    toml::from_str(&contents).map_err(|e| format!("failed to parse {}: {e}", path.display()))
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() {
    let args = Args::parse();

    if let Some(shell) = args.completions {
        generate(
            shell,
            &mut Args::command(),
            "modelrelay-worker",
            &mut std::io::stdout(),
        );
        return;
    }

    // Load config file if provided.
    let file_cfg = if let Some(ref path) = args.config {
        match load_config_file(path) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    } else {
        FileConfig::default()
    };

    // Resolve each field: CLI/env > config file > default.
    let proxy_url = args
        .proxy_url
        .or(file_cfg.proxy_url)
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
        .trim()
        .to_string();
    let provider = args
        .provider
        .or(file_cfg.provider)
        .unwrap_or_else(|| "local".to_string())
        .trim()
        .to_string();
    let worker_secret = args
        .worker_secret
        .or(file_cfg.worker_secret)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            eprintln!(
                "error: worker secret is required (--worker-secret, WORKER_SECRET env, or config file)"
            );
            std::process::exit(1);
        });
    let worker_name = args
        .worker_name
        .or(file_cfg.worker_name)
        .unwrap_or_else(|| "worker".to_string())
        .trim()
        .to_string();
    let models_str = args.models;
    let models: Vec<String> = if let Some(ref s) = models_str {
        s.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if let Some(ref v) = file_cfg.models {
        v.iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec!["default".to_string()]
    };
    let max_concurrency = args
        .max_concurrency
        .or(file_cfg.max_concurrency)
        .unwrap_or(1)
        .max(1);
    let backend_url = args
        .backend_url
        .or(file_cfg.backend_url)
        .unwrap_or_else(|| "http://127.0.0.1:8000".to_string())
        .trim()
        .to_string();
    let endpoint_prefixes: Vec<String> = if let Some(ref s) = args.endpoint_prefixes {
        s.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if let Some(ref v) = file_cfg.endpoint_prefixes {
        v.clone()
    } else {
        vec![]
    };
    let models_poll_interval_secs = args
        .models_poll_interval
        .or(file_cfg.models_poll_interval_secs)
        .unwrap_or(30);
    let log_level = args
        .log_level
        .or(file_cfg.log_level)
        .unwrap_or_else(|| "info".to_string());

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&log_level)),
        )
        .init();

    let wildcard_discovery = models.iter().any(|m| m.trim() == "*");
    let mut config = WorkerDaemonConfig {
        proxy_base_url: proxy_url.clone(),
        provider: provider.clone(),
        worker_secret: worker_secret.clone(),
        worker_name: worker_name.clone(),
        models: models.clone(),
        max_concurrent: max_concurrency,
        backend_base_url: backend_url.clone(),
        endpoint_prefixes: endpoint_prefixes.clone(),
    };

    // Perform initial wildcard discovery so startup logs reflect real models,
    // while retaining `wildcard_discovery = true` for reconnect & periodic re-polling.
    config.resolve_wildcard_models().await;

    tracing::info!(
        proxy = %proxy_url,
        provider = %provider,
        name = %worker_name,
        models = %config.models.join(","),
        concurrency = max_concurrency,
        backend = %backend_url,
        wildcard_discovery,
        models_poll_interval_secs,
        "modelrelay-worker starting"
    );

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    let poll_interval =
        (models_poll_interval_secs > 0).then(|| Duration::from_secs(models_poll_interval_secs));

    let daemon = WorkerDaemon::new(config)
        .with_wildcard_discovery(wildcard_discovery)
        .with_models_poll_interval(poll_interval)
        .with_shutdown_rx(shutdown_rx);

    if let Err(e) = daemon.run_with_reconnect().await {
        tracing::error!(error = %e, "modelrelay-worker error");
        std::process::exit(1);
    }

    tracing::info!("modelrelay-worker exited cleanly");
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

    tracing::info!("shutdown signal received");
}
