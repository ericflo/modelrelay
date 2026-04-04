use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

mod db;
mod routes;
mod state;

use state::CloudState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Connect to PostgreSQL (optional — app works without it)
    let pool = db::connect().await;
    if let Some(ref p) = pool {
        db::run_migrations(p).await;
    } else {
        tracing::warn!("DATABASE_URL not set — running without database");
    }

    // Stripe key (optional — checkout returns friendly error without it)
    let stripe_key = std::env::var("STRIPE_SECRET_KEY").ok();
    if stripe_key.is_none() {
        tracing::warn!("STRIPE_SECRET_KEY not set — checkout disabled");
    }

    // Stripe webhook secret (optional — webhooks return 500 without it)
    let webhook_secret = std::env::var("STRIPE_WEBHOOK_SECRET").ok();
    if webhook_secret.is_none() {
        tracing::warn!("STRIPE_WEBHOOK_SECRET not set — webhook verification disabled");
    }

    // Admin API for key provisioning (optional — keys won't be provisioned without it)
    let admin_url = std::env::var("MODELRELAY_ADMIN_URL").ok();
    let admin_token = std::env::var("MODELRELAY_ADMIN_TOKEN").ok();
    if admin_url.is_none() || admin_token.is_none() {
        tracing::warn!(
            "MODELRELAY_ADMIN_URL or MODELRELAY_ADMIN_TOKEN not set — API key provisioning disabled"
        );
    }

    // Set up session layer if we have a DB
    let session_layer = if let Some(ref p) = pool {
        let session_store = tower_sessions_sqlx_store::PostgresStore::new(p.clone());
        session_store
            .migrate()
            .await
            .inspect_err(|e| tracing::error!("session migration error: {e}"))
            .ok();

        Some(
            tower_sessions::SessionManagerLayer::new(session_store)
                .with_secure(true)
                .with_http_only(true),
        )
    } else {
        None
    };

    let state = Arc::new(CloudState {
        db: pool,
        stripe_key,
        webhook_secret,
        admin_url,
        admin_token,
    });

    let mut app = Router::new()
        .merge(routes::router(state))
        .layer(TraceLayer::new_for_http());

    if let Some(layer) = session_layer {
        app = app.layer(layer);
    }

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8000);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("listening on {addr}");

    let listener = TcpListener::bind(addr).await.expect("failed to bind");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C handler");
    tracing::info!("shutdown signal received");
}
