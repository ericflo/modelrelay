use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::{HeaderName, HeaderValue, Method};
use clap::{CommandFactory, Parser};
use clap_complete::{Shell, generate};
use modelrelay_server::{
    ApiKeyStore, InMemoryApiKeyStore, ProviderQueuePolicy, ProxyHttpApp, ProxyServerCore,
    WorkerSocketApp, WorkerSocketProviderConfig,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};
use tracing_subscriber::EnvFilter;

const CANCELLED_REQUEST_REAP_GRACE: Duration = Duration::from_secs(5);
const WORKER_AUTH_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(15);
const GRACEFUL_SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

/// Remote LLM worker proxy server.
///
/// Accepts provider-compatible inference requests and routes them to remote
/// workers over WebSocket, where those workers speak to local model servers.
#[derive(Parser, Debug)]
#[command(name = "modelrelay-server", version)]
#[allow(clippy::struct_excessive_bools)]
struct Args {
    /// Address to listen on
    #[arg(long, env = "LISTEN_ADDR", default_value = "127.0.0.1:8080")]
    listen: String,

    /// Provider name used for worker routing and request dispatch
    #[arg(long, env = "PROVIDER_NAME", default_value = "local")]
    provider: String,

    /// Secret that workers must present to authenticate.
    /// Optional when `DATABASE_URL` is set (workers can authenticate with API keys).
    #[arg(long, env = "WORKER_SECRET")]
    worker_secret: Option<String>,

    /// Maximum number of requests that may be queued (0 = unlimited)
    #[arg(long, env = "MAX_QUEUE_LEN", default_value = "100")]
    max_queue_len: usize,

    /// Seconds before a queued request times out (0 = no timeout)
    #[arg(long, env = "QUEUE_TIMEOUT_SECS", default_value = "30")]
    queue_timeout: u64,

    /// Seconds before an in-flight HTTP request (or streaming chunk gap) times out (0 = no timeout)
    #[arg(long, env = "REQUEST_TIMEOUT_SECS", default_value = "300")]
    request_timeout: u64,

    /// Maximum HTTP request body size in MiB
    #[arg(long, env = "MAX_BODY_MB", default_value = "64")]
    max_body_mb: usize,

    /// Log level filter (e.g. info, debug, warn, error, or a directive like
    /// `modelrelay_server=debug`). Overridden by `RUST_LOG` if set.
    #[arg(long, env = "LOG_LEVEL", default_value = "info")]
    log_level: String,

    /// Admin API token for /admin/* endpoints (if unset, admin endpoints return 403)
    #[arg(long, env = "MODELRELAY_ADMIN_TOKEN")]
    admin_token: Option<String>,

    /// Require client API keys for /v1/* endpoints (default: false, all clients accepted)
    #[arg(long, env = "MODELRELAY_REQUIRE_API_KEYS", default_value = "false")]
    require_api_keys: bool,

    /// Comma-separated allowed CORS origins (e.g. `https://modelrelay.io,http://localhost:3000`)
    /// or `*` for wildcard. When unset, `/v1/*` and `/health` allow browser inference requests
    /// while `/admin/*` does not emit permissive cross-origin headers.
    #[arg(long, env = "CORS_ALLOWED_ORIGINS")]
    cors_allowed_origins: Option<String>,

    /// Trust `X-Forwarded-For` / `Forwarded` headers for worker auth rate limiting.
    /// Only enable behind a trusted reverse proxy.
    #[arg(long, env = "TRUST_PROXY_HEADERS", default_value = "false")]
    trust_proxy_headers: bool,

    /// `PostgreSQL` connection URL for persistent API key storage.
    /// If unset, keys are stored in memory (single-pod only, lost on restart).
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// Existing native API key to hash-import into a rebuilt `PostgreSQL` store.
    /// The raw key is never persisted. A matching revoked key fails startup.
    #[arg(long, env = "MODELRELAY_BOOTSTRAP_API_KEY")]
    bootstrap_api_key: Option<String>,

    /// Probe the local server's `/health` endpoint and exit 0 if healthy (1 otherwise).
    /// Useful for container `HEALTHCHECK` probes in scratch/distroless images without curl.
    #[arg(long)]
    healthcheck: bool,

    /// Generate shell completion script for the given shell and exit
    #[arg(long, value_name = "SHELL", hide = true)]
    completions: Option<Shell>,
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() {
    let args = Args::parse();

    if args.healthcheck {
        let exit_code = run_healthcheck(&args.listen).await;
        std::process::exit(exit_code);
    }

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

    if args.worker_secret.is_none() && args.database_url.is_none() {
        tracing::warn!(
            "Neither WORKER_SECRET nor DATABASE_URL is configured; no workers will be able to \
             authenticate unless API keys are created dynamically via /admin/keys"
        );
    }

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

    let default_policy = ProviderQueuePolicy {
        max_queue_len,
        queue_timeout_ticks,
    };

    {
        let mut guard = core.lock().await;
        guard.set_default_queue_policy(default_policy);
        guard.configure_provider_queue(&args.provider, default_policy);
    }

    let api_key_store = build_api_key_store(
        args.database_url.as_deref(),
        args.bootstrap_api_key.as_deref(),
    )
    .await;

    let mut worker_socket_app = WorkerSocketApp::new(Arc::clone(&core))
        .with_api_key_store(Arc::clone(&api_key_store))
        .with_auth_rate_limit_cooldown(WORKER_AUTH_RATE_LIMIT_COOLDOWN)
        .with_trust_proxy_headers(args.trust_proxy_headers);

    if let Some(ref secret) = args.worker_secret {
        worker_socket_app = worker_socket_app
            .with_provider(&args.provider, WorkerSocketProviderConfig::enabled(secret));
    }

    let (public_cors, admin_cors) = build_cors_layers(args.cors_allowed_origins.as_deref());
    let request_timeout =
        (args.request_timeout > 0).then(|| Duration::from_secs(args.request_timeout));
    let max_body_bytes = args.max_body_mb.max(1).saturating_mul(1024 * 1024);

    let http_app = ProxyHttpApp::new(Arc::clone(&core))
        .with_models_provider(&args.provider)
        .with_worker_socket_app(worker_socket_app)
        .with_admin_token(args.admin_token)
        .with_require_api_keys(args.require_api_keys)
        .with_api_key_store(api_key_store)
        .with_request_timeout(request_timeout)
        .with_max_body_bytes(max_body_bytes)
        .with_public_cors(public_cors)
        .with_admin_cors(admin_cors);

    let router = http_app.router();

    // Always run maintenance loop to expire queued timeouts, reap unacknowledged
    // cancelled requests, and expire graceful shutdown deadlines.
    spawn_maintenance_loop(Arc::clone(&core));

    let listener = tokio::net::TcpListener::bind(&args.listen)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind to {}: {e}", args.listen));

    let queue_timeout_display = display_timeout(args.queue_timeout);
    let request_timeout_display = display_timeout(args.request_timeout);
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
        max_body_mb = args.max_body_mb,
        trust_proxy_headers = args.trust_proxy_headers,
        "modelrelay-server starting"
    );

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(Arc::clone(&core)))
    .await
    .expect("server error");
}

fn build_cors_layers(configured_origins: Option<&str>) -> (Option<CorsLayer>, Option<CorsLayer>) {
    let public_methods = AllowMethods::list([Method::GET, Method::POST, Method::OPTIONS]);
    let public_headers = AllowHeaders::list([
        HeaderName::from_static("authorization"),
        HeaderName::from_static("content-type"),
        HeaderName::from_static("x-api-key"),
        HeaderName::from_static("x-model"),
        HeaderName::from_static("x-provider"),
        HeaderName::from_static("anthropic-version"),
    ]);

    match configured_origins.map(str::trim) {
        Some("" | "none") => (None, None),
        Some("*") => {
            let public = CorsLayer::new()
                .allow_origin(AllowOrigin::any())
                .allow_methods(public_methods)
                .allow_headers(public_headers);
            let admin = CorsLayer::new()
                .allow_origin(AllowOrigin::any())
                .allow_methods(AllowMethods::list([
                    Method::GET,
                    Method::POST,
                    Method::DELETE,
                    Method::OPTIONS,
                ]))
                .allow_headers(AllowHeaders::list([
                    HeaderName::from_static("authorization"),
                    HeaderName::from_static("content-type"),
                ]));
            (Some(public), Some(admin))
        }
        Some(list) => {
            let origins: Vec<HeaderValue> = list
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter_map(|origin| match HeaderValue::from_str(origin) {
                    Ok(value) => Some(value),
                    Err(error) => {
                        tracing::warn!(origin = %origin, error = %error, "ignoring invalid CORS origin");
                        None
                    }
                })
                .collect();
            if origins.is_empty() {
                (None, None)
            } else {
                let public = CorsLayer::new()
                    .allow_origin(AllowOrigin::list(origins.clone()))
                    .allow_methods(public_methods)
                    .allow_headers(public_headers);
                let admin = CorsLayer::new()
                    .allow_origin(AllowOrigin::list(origins))
                    .allow_methods(AllowMethods::list([
                        Method::GET,
                        Method::POST,
                        Method::DELETE,
                        Method::OPTIONS,
                    ]))
                    .allow_headers(AllowHeaders::list([
                        HeaderName::from_static("authorization"),
                        HeaderName::from_static("content-type"),
                    ]));
                (Some(public), Some(admin))
            }
        }
        None => {
            // Default: allow browser inference requests on /v1/* and /health,
            // but keep /admin/* same-origin unless CORS_ALLOWED_ORIGINS is explicitly set.
            let public = CorsLayer::new()
                .allow_origin(AllowOrigin::any())
                .allow_methods(public_methods)
                .allow_headers(public_headers);
            (Some(public), None)
        }
    }
}

async fn run_healthcheck(listen: &str) -> i32 {
    let target = listen
        .replace("0.0.0.0:", "127.0.0.1:")
        .replace("[::]:", "[::1]:");
    let probe = async {
        let mut stream = tokio::net::TcpStream::connect(&target).await.ok()?;
        let request = "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut buf = [0_u8; 256];
        let n = stream.read(&mut buf).await.ok()?;
        let head = std::str::from_utf8(&buf[..n]).ok()?;
        let status_line = head.lines().next()?;
        if status_line.contains(" 200 ") {
            Some(())
        } else {
            None
        }
    };

    match tokio::time::timeout(Duration::from_secs(5), probe).await {
        Ok(Some(())) => 0,
        _ => 1,
    }
}

fn display_timeout(seconds: u64) -> String {
    if seconds == 0 {
        "none".to_string()
    } else {
        format!("{seconds}s")
    }
}

async fn build_api_key_store(
    database_url: Option<&str>,
    bootstrap_api_key: Option<&str>,
) -> Arc<dyn ApiKeyStore> {
    match database_url {
        #[cfg(feature = "postgres")]
        Some(url) => {
            let pool = connect_postgres_with_retry(url).await;
            sqlx::migrate!("./migrations")
                .run(&pool)
                .await
                .expect("failed to run server migrations");
            let store = modelrelay_server::PostgresApiKeyStore::new(pool);
            if let Some(raw_key) = bootstrap_api_key {
                store
                    .ensure_imported_key("recovered-bootstrap-key", raw_key)
                    .await
                    .expect("failed to import MODELRELAY_BOOTSTRAP_API_KEY");
                tracing::info!("recovery API key is present in the persistent store");
            }
            tracing::info!("api key store: postgres-backed");
            Arc::new(store)
        }
        #[cfg(not(feature = "postgres"))]
        Some(_) => {
            panic!("DATABASE_URL set but server was compiled without the `postgres` feature");
        }
        None => {
            assert!(
                bootstrap_api_key.is_none(),
                "MODELRELAY_BOOTSTRAP_API_KEY requires DATABASE_URL"
            );
            tracing::warn!(
                "DATABASE_URL not set \u{2014} using in-memory API key store \
                 (single-pod only, keys lost on restart)"
            );
            Arc::new(InMemoryApiKeyStore::new())
        }
    }
}

#[cfg(feature = "postgres")]
async fn connect_postgres_with_retry(url: &str) -> sqlx::PgPool {
    let mut attempt = 0_u32;
    let max_attempts = 5_u32;
    loop {
        attempt += 1;
        match sqlx::PgPool::connect(url).await {
            Ok(pool) => return pool,
            Err(error) if attempt < max_attempts => {
                let delay_secs = 1_u64 << (attempt - 1);
                tracing::warn!(
                    attempt,
                    max_attempts,
                    delay_secs,
                    error = %error,
                    "failed to connect to DATABASE_URL; retrying"
                );
                tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            }
            Err(error) => {
                panic!("failed to connect to DATABASE_URL after {max_attempts} attempts: {error}");
            }
        }
    }
}

/// Spawn a background maintenance task to expire queued requests, reap
/// unacknowledged cancelled requests, and expire graceful shutdown deadlines.
fn spawn_maintenance_loop(core: Arc<Mutex<ProxyServerCore>>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let now = Instant::now();
            let (failures, reaped) = {
                let mut guard = core.lock().await;
                let failures = guard.expire_queue_timeouts(now);
                let reaped = guard.expire_cancelled_requests(now, CANCELLED_REQUEST_REAP_GRACE);
                let _ = guard.expire_graceful_shutdown(now);
                (failures, reaped)
            };
            for failure in &failures {
                tracing::debug!(
                    request_id = %failure.request_id,
                    "queued request timed out"
                );
            }
            for request_id in &reaped {
                tracing::warn!(
                    request_id = %request_id,
                    "reaped unacknowledged cancelled request"
                );
            }
        }
    });
}

async fn shutdown_signal(core: Arc<Mutex<ProxyServerCore>>) {
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

    tracing::info!("shutdown signal received; draining connected workers");
    {
        let mut guard = core.lock().await;
        let _ = guard.begin_graceful_shutdown(
            Some("server shutting down"),
            GRACEFUL_SHUTDOWN_DRAIN_TIMEOUT,
        );
    }

    let deadline = Instant::now() + GRACEFUL_SHUTDOWN_DRAIN_TIMEOUT + Duration::from_secs(1);
    while Instant::now() < deadline {
        let remaining = {
            let mut guard = core.lock().await;
            let _ = guard.expire_graceful_shutdown(Instant::now());
            guard.connected_worker_count()
        };
        if remaining == 0 {
            tracing::info!("all workers drained cleanly");
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    tracing::warn!("graceful shutdown drain deadline reached; exiting");
}
