//! One-shot tool to reprovision active API keys from the cloud database
//! (`api_keys` table) into the server database (`server_api_keys` table).
//!
//! After the DB split (PR #220), the server runs against its own fresh Postgres
//! database with an empty `server_api_keys` table. Existing keys in the cloud DB
//! will not validate until their SHA-256 hashes are inserted into the server DB.
//!
//! The server admin API (`POST /admin/keys`) generates new keys — it cannot import
//! existing ones. This tool therefore connects directly to both databases and
//! inserts the hashed keys.
//!
//! # Environment variables
//!
//! - `CLOUD_DATABASE_URL` — Postgres connection string for the cloud database
//!   (containing `api_keys`).
//! - `SERVER_DATABASE_URL` — Postgres connection string for the server database
//!   (containing `server_api_keys`).
//!
//! # Usage
//!
//! ```sh
//! CLOUD_DATABASE_URL="postgres://..." SERVER_DATABASE_URL="postgres://..." \
//!   reprovision_server_keys [--continue-on-error]
//! ```

use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Prefix length stored in `server_api_keys.prefix`: the `mr_live_` tag (8 chars)
/// plus 8 random chars = 16.
const PREFIX_LEN: usize = 16;

#[derive(Debug, sqlx::FromRow)]
struct CloudKey {
    id: uuid::Uuid,
    key_id: String,
    raw_key: String,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn sha256_hash(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

async fn connect(env_var: &str) -> Result<PgPool, String> {
    let url = std::env::var(env_var)
        .map_err(|_| format!("{env_var} is not set"))?;

    PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .map_err(|e| format!("failed to connect via {env_var}: {e}"))
}

#[tokio::main]
async fn main() {
    let continue_on_error = std::env::args().any(|a| a == "--continue-on-error");

    eprintln!("reprovision-server-keys: connecting to databases...");

    let cloud_pool = match connect("CLOUD_DATABASE_URL").await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };

    let server_pool = match connect("SERVER_DATABASE_URL").await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("FATAL: {e}");
            std::process::exit(1);
        }
    };

    // Read active keys from the cloud database.
    let keys: Vec<CloudKey> = match sqlx::query_as::<_, CloudKey>(
        "SELECT id, key_id, raw_key, name, created_at \
         FROM api_keys \
         WHERE revoked_at IS NULL \
         ORDER BY created_at",
    )
    .fetch_all(&cloud_pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("FATAL: failed to read cloud api_keys: {e}");
            std::process::exit(1);
        }
    };

    let scanned = keys.len();
    eprintln!("reprovision-server-keys: found {scanned} active key(s) in cloud DB");

    let mut provisioned: usize = 0;
    let mut skipped: usize = 0;
    let mut failed: usize = 0;

    for key in &keys {
        let hash = sha256_hash(key.raw_key.as_bytes());
        let prefix: String = key.raw_key.chars().take(PREFIX_LEN).collect();

        // INSERT with ON CONFLICT on the unique hash to make this idempotent.
        // The server table has an index on `hash WHERE revoked_at IS NULL`, but no
        // UNIQUE constraint on hash itself. We use a CTE with an existence check
        // instead of ON CONFLICT.
        let result = sqlx::query(
            "INSERT INTO server_api_keys (id, name, prefix, hash, created_at) \
             SELECT $1, $2, $3, $4, $5 \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM server_api_keys WHERE hash = $4 \
             )",
        )
        .bind(key.id)
        .bind(&key.name)
        .bind(&prefix)
        .bind(&hash)
        .bind(key.created_at)
        .execute(&server_pool)
        .await;

        match result {
            Ok(r) if r.rows_affected() == 1 => {
                eprintln!("  + provisioned: {} (prefix: {})", key.key_id, prefix);
                provisioned += 1;
            }
            Ok(_) => {
                // rows_affected == 0 means the WHERE NOT EXISTS blocked insert (already present)
                eprintln!("  ~ skipped (already present): {}", key.key_id);
                skipped += 1;
            }
            Err(e) => {
                eprintln!("  ! FAILED: {} — {e}", key.key_id);
                failed += 1;
                if !continue_on_error {
                    eprintln!("FATAL: aborting (use --continue-on-error to keep going)");
                    break;
                }
            }
        }
    }

    eprintln!();
    eprintln!("=== Summary ===");
    eprintln!("  Scanned:      {scanned}");
    eprintln!("  Provisioned:  {provisioned}");
    eprintln!("  Skipped:      {skipped}");
    eprintln!("  Failed:       {failed}");

    if failed > 0 {
        std::process::exit(1);
    }
}
