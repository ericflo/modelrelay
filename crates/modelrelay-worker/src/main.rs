use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use modelrelay_worker::{WorkerDaemon, WorkerDaemonConfig};
use serde::Deserialize;
use tracing_subscriber::EnvFilter;

/// Remote LLM worker daemon.
///
/// Connects to a central proxy server over WebSocket and forwards inference
/// requests to a local model backend (e.g. llama-server).
#[derive(Parser, Debug)]
#[command(name = "modelrelay-worker", version)]
struct Args {
    /// Path to a TOML configuration file.
    ///
    /// If provided, settings are loaded from the file first, then overridden
    /// by any explicit CLI flags or environment variables.
    #[arg(long, value_name = "FILE")]
    config: Option<String>,

    /// Base URL of the proxy server (e.g. `http://127.0.0.1:8080`)
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

    /// Base URL of the local model backend (e.g. `http://127.0.0.1:8000`)
    #[arg(long, env = "BACKEND_URL")]
    backend_url: Option<String>,

    /// Log level filter (e.g. info, debug, warn, error, or a directive like
    /// `modelrelay_worker=debug`). Overridden by `RUST_LOG` if set.
    #[arg(long, env = "LOG_LEVEL")]
    log_level: Option<String>,

    /// Generate shell completion script for the given shell and exit
    #[arg(long, value_name = "SHELL", hide = true)]
    completions: Option<Shell>,
}

/// Configuration loaded from a TOML file.
#[derive(Deserialize, Debug, Default)]
struct ConfigFile {
    proxy_url: Option<String>,
    provider: Option<String>,
    worker_secret: Option<String>,
    worker_name: Option<String>,
    models: Option<toml::Value>,
    max_concurrency: Option<u32>,
    backend_url: Option<String>,
    log_level: Option<String>,
}

/// Resolved configuration with all defaults applied.
struct ResolvedConfig {
    proxy_url: String,
    provider: String,
    worker_secret: String,
    worker_name: String,
    models: String,
    max_concurrency: u32,
    backend_url: String,
    log_level: String,
}

impl ResolvedConfig {
    fn resolve(args: Args) -> Self {
        let file_cfg = if let Some(ref path) = args.config {
            let contents = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("failed to read config file '{path}': {e}"));
            toml::from_str::<ConfigFile>(&contents)
                .unwrap_or_else(|e| panic!("failed to parse config file '{path}': {e}"))
        } else {
            ConfigFile::default()
        };

        // Convert TOML models value to comma-separated string
        let file_models = file_cfg.models.map(|v| match v {
            toml::Value::Array(arr) => arr
                .iter()
                .filter_map(|item| item.as_str().map(String::from))
                .collect::<Vec<_>>()
                .join(","),
            toml::Value::String(s) => s,
            other => other.to_string(),
        });

        // CLI/env > config file > default
        let proxy_url = args
            .proxy_url
            .or(file_cfg.proxy_url)
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());
        let provider = args
            .provider
            .or(file_cfg.provider)
            .unwrap_or_else(|| "local".to_string());
        let worker_secret = args.worker_secret.or(file_cfg.worker_secret).expect(
            "worker secret is required: pass --worker-secret, set WORKER_SECRET, or add \
             worker_secret to your config file",
        );
        let worker_name = args
            .worker_name
            .or(file_cfg.worker_name)
            .unwrap_or_else(|| "worker".to_string());
        let models = args
            .models
            .or(file_models)
            .unwrap_or_else(|| "default".to_string());
        let max_concurrency = args
            .max_concurrency
            .or(file_cfg.max_concurrency)
            .unwrap_or(1);
        let backend_url = args
            .backend_url
            .or(file_cfg.backend_url)
            .unwrap_or_else(|| "http://127.0.0.1:8000".to_string());
        let log_level = args
            .log_level
            .or(file_cfg.log_level)
            .unwrap_or_else(|| "info".to_string());

        Self {
            proxy_url,
            provider,
            worker_secret,
            worker_name,
            models,
            max_concurrency,
            backend_url,
            log_level,
        }
    }
}

#[tokio::main]
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

    let cfg = ResolvedConfig::resolve(args);

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.log_level)),
        )
        .init();

    let models: Vec<String> = cfg
        .models
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let config = WorkerDaemonConfig {
        proxy_base_url: cfg.proxy_url.clone(),
        provider: cfg.provider.clone(),
        worker_secret: cfg.worker_secret.clone(),
        worker_name: cfg.worker_name.clone(),
        models: models.clone(),
        max_concurrent: cfg.max_concurrency,
        backend_base_url: cfg.backend_url.clone(),
    };

    tracing::info!(
        proxy = %cfg.proxy_url,
        provider = %cfg.provider,
        name = %cfg.worker_name,
        models = %models.join(","),
        concurrency = cfg.max_concurrency,
        backend = %cfg.backend_url,
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
