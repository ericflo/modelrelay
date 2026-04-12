use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use rand::RngExt;
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

pub use modelrelay_protocol::admin_api::ApiKeyMetadata;

pub const API_KEY_PREFIX: &str = "mr_live_";
pub const API_KEY_RANDOM_LEN: usize = 32;

/// Errors that an [`ApiKeyStore`] implementation may return.
#[derive(Debug)]
pub enum StoreError {
    /// The provided id was not a valid format (e.g. not a UUID).
    BadId,
    /// An internal / database error.
    Internal(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId => write!(f, "invalid key id"),
            Self::Internal(msg) => write!(f, "store error: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Abstraction over API key storage so the HTTP layer is backend-agnostic.
#[async_trait::async_trait]
pub trait ApiKeyStore: Send + Sync + 'static {
    /// Create a new API key. Returns (metadata, `raw_secret`). The raw secret is
    /// returned exactly once and never stored.
    async fn create_key(&self, name: String) -> Result<(ApiKeyMetadata, String), StoreError>;

    /// Validate a raw API key. Returns the key id if valid and not revoked.
    async fn validate_key(&self, raw_key: &str) -> Result<Option<String>, StoreError>;

    /// Revoke a key by id. Returns true if the key existed and was revoked.
    async fn revoke_key(&self, id: &str) -> Result<bool, StoreError>;

    /// List all key metadata (never includes secrets or hashes).
    async fn list_keys(&self) -> Result<Vec<ApiKeyMetadata>, StoreError>;
}

// ---------------------------------------------------------------------------
// In-memory implementation (for tests and single-pod fallback)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct StoredApiKey {
    metadata: ApiKeyMetadata,
    hash: [u8; 32],
}

#[derive(Debug, Default)]
struct InMemoryInner {
    keys: HashMap<String, StoredApiKey>,
}

/// In-memory API key store backed by an `Arc<RwLock<HashMap>>`.
///
/// Keys live only in process memory and are lost on restart. Use only for
/// tests or single-pod deployments where persistence is not required.
#[derive(Debug, Default, Clone)]
pub struct InMemoryApiKeyStore {
    inner: Arc<RwLock<InMemoryInner>>,
}

impl InMemoryApiKeyStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl ApiKeyStore for InMemoryApiKeyStore {
    async fn create_key(&self, name: String) -> Result<(ApiKeyMetadata, String), StoreError> {
        let raw_secret = generate_api_key();
        let hash = sha256_hash(raw_secret.as_bytes());
        let id = uuid::Uuid::new_v4().to_string();
        let prefix = raw_secret
            .chars()
            .take(8 + API_KEY_PREFIX.len())
            .collect::<String>();
        let now = now_epoch();

        let metadata = ApiKeyMetadata {
            id: id.clone(),
            name,
            prefix,
            created_at: now,
            last_used_at: None,
            revoked: false,
        };

        let stored = StoredApiKey {
            metadata: metadata.clone(),
            hash,
        };

        self.inner.write().await.keys.insert(id, stored);

        Ok((metadata, raw_secret))
    }

    async fn validate_key(&self, raw_key: &str) -> Result<Option<String>, StoreError> {
        let hash = sha256_hash(raw_key.as_bytes());
        let mut store = self.inner.write().await;

        for stored in store.keys.values_mut() {
            if stored.metadata.revoked {
                continue;
            }
            if subtle::ConstantTimeEq::ct_eq(&stored.hash[..], &hash[..]).into() {
                stored.metadata.last_used_at = Some(now_epoch());
                return Ok(Some(stored.metadata.id.clone()));
            }
        }
        Ok(None)
    }

    async fn revoke_key(&self, id: &str) -> Result<bool, StoreError> {
        let mut store = self.inner.write().await;
        if let Some(stored) = store.keys.get_mut(id) {
            stored.metadata.revoked = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn list_keys(&self) -> Result<Vec<ApiKeyMetadata>, StoreError> {
        let store = self.inner.read().await;
        Ok(store.keys.values().map(|s| s.metadata.clone()).collect())
    }
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
mod postgres_store {
    use super::{
        API_KEY_PREFIX, ApiKeyMetadata, ApiKeyStore, StoreError, generate_api_key, sha256_hash,
    };

    /// Row shape for `SELECT ... FROM server_api_keys`.
    #[derive(sqlx::FromRow)]
    struct KeyRow {
        id: uuid::Uuid,
        name: String,
        prefix: String,
        created_at: chrono::DateTime<chrono::Utc>,
        last_used_at: Option<chrono::DateTime<chrono::Utc>>,
        revoked_at: Option<chrono::DateTime<chrono::Utc>>,
    }

    impl From<KeyRow> for ApiKeyMetadata {
        fn from(row: KeyRow) -> Self {
            Self {
                id: row.id.to_string(),
                name: row.name,
                prefix: row.prefix,
                created_at: u64::try_from(row.created_at.timestamp()).unwrap_or(0),
                last_used_at: row
                    .last_used_at
                    .map(|t| u64::try_from(t.timestamp()).unwrap_or(0)),
                revoked: row.revoked_at.is_some(),
            }
        }
    }

    /// Postgres-backed API key store for multi-replica correctness.
    pub struct PostgresApiKeyStore {
        pool: sqlx::PgPool,
    }

    impl PostgresApiKeyStore {
        #[must_use]
        pub fn new(pool: sqlx::PgPool) -> Self {
            Self { pool }
        }
    }

    #[async_trait::async_trait]
    impl ApiKeyStore for PostgresApiKeyStore {
        async fn create_key(&self, name: String) -> Result<(ApiKeyMetadata, String), StoreError> {
            let raw_secret = generate_api_key();
            let hash = sha256_hash(raw_secret.as_bytes());
            let id = uuid::Uuid::new_v4();
            let prefix: String = raw_secret.chars().take(8 + API_KEY_PREFIX.len()).collect();

            let row = sqlx::query_as::<_, KeyRow>(
                "INSERT INTO server_api_keys (id, name, prefix, hash) \
                 VALUES ($1, $2, $3, $4) \
                 RETURNING id, name, prefix, created_at, last_used_at, revoked_at",
            )
            .bind(id)
            .bind(&name)
            .bind(&prefix)
            .bind(&hash[..])
            .fetch_one(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;

            Ok((ApiKeyMetadata::from(row), raw_secret))
        }

        async fn validate_key(&self, raw_key: &str) -> Result<Option<String>, StoreError> {
            let hash = sha256_hash(raw_key.as_bytes());

            // Single atomic UPDATE+RETURNING: looks up by full hash (indexed),
            // restricted to non-revoked keys, and bumps last_used_at in one
            // roundtrip. No SELECT-then-UPDATE race across replicas.
            //
            // Timing-attack note: the WHERE clause uses full-hash equality via
            // the btree index. Postgres does not expose timing differences that
            // an attacker could measure over a network — the query plan is a
            // single index scan regardless of whether zero or one row matches.
            let row: Option<(uuid::Uuid,)> = sqlx::query_as(
                "UPDATE server_api_keys SET last_used_at = now() \
                 WHERE hash = $1 AND revoked_at IS NULL \
                 RETURNING id",
            )
            .bind(&hash[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;

            Ok(row.map(|(id,)| id.to_string()))
        }

        async fn revoke_key(&self, id: &str) -> Result<bool, StoreError> {
            let uuid: uuid::Uuid = id.parse().map_err(|_| StoreError::BadId)?;

            let result = sqlx::query(
                "UPDATE server_api_keys SET revoked_at = now() \
                 WHERE id = $1 AND revoked_at IS NULL",
            )
            .bind(uuid)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;

            Ok(result.rows_affected() > 0)
        }

        async fn list_keys(&self) -> Result<Vec<ApiKeyMetadata>, StoreError> {
            let rows = sqlx::query_as::<_, KeyRow>(
                "SELECT id, name, prefix, created_at, last_used_at, revoked_at \
                 FROM server_api_keys ORDER BY created_at DESC",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;

            Ok(rows.into_iter().map(Into::into).collect())
        }
    }
}

#[cfg(feature = "postgres")]
pub use postgres_store::PostgresApiKeyStore;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

#[must_use]
pub fn generate_api_key() -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::rng();
    let random_part: String = (0..API_KEY_RANDOM_LEN)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();
    format!("{API_KEY_PREFIX}{random_part}")
}

#[must_use]
pub fn sha256_hash(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
