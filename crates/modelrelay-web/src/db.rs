use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Initialize the PostgreSQL connection pool from `DATABASE_URL`.
/// Returns `None` if `DATABASE_URL` is not set, allowing the app to run without a database.
pub async fn connect() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .inspect_err(|e| tracing::error!("failed to connect to database: {e}"))
        .ok()?;

    tracing::info!("connected to PostgreSQL");
    Some(pool)
}

/// Run embedded SQL migrations.
pub async fn run_migrations(pool: &PgPool) {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .inspect_err(|e| tracing::error!("migration error: {e}"))
        .ok();
}
