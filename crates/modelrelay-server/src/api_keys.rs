use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// The provided API key did not use the native `mr_live_` format.
    BadKey,
    /// An internal / database error.
    Internal(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadId => write!(f, "invalid key id"),
            Self::BadKey => write!(f, "invalid API key"),
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
    last_used_at: AtomicU64,
}

impl StoredApiKey {
    fn snapshot_metadata(&self) -> ApiKeyMetadata {
        let mut meta = self.metadata.clone();
        let last = self.last_used_at.load(Ordering::Relaxed);
        meta.last_used_at = (last > 0).then_some(last);
        meta
    }
}

#[derive(Debug, Default)]
struct InMemoryInner {
    keys: HashMap<String, StoredApiKey>,
    hash_to_id: HashMap<[u8; 32], String>,
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
            last_used_at: AtomicU64::new(0),
        };

        let mut store = self.inner.write().await;
        store.hash_to_id.insert(hash, id.clone());
        store.keys.insert(id, stored);

        Ok((metadata, raw_secret))
    }

    async fn validate_key(&self, raw_key: &str) -> Result<Option<String>, StoreError> {
        let hash = sha256_hash(raw_key.as_bytes());
        let store = self.inner.read().await;

        let Some(id) = store.hash_to_id.get(&hash) else {
            return Ok(None);
        };
        let Some(stored) = store.keys.get(id) else {
            return Ok(None);
        };
        if stored.metadata.revoked {
            return Ok(None);
        }
        if subtle::ConstantTimeEq::ct_eq(&stored.hash[..], &hash[..]).into() {
            stored.last_used_at.store(now_epoch(), Ordering::Relaxed);
            return Ok(Some(stored.metadata.id.clone()));
        }
        Ok(None)
    }

    async fn revoke_key(&self, id: &str) -> Result<bool, StoreError> {
        let mut store = self.inner.write().await;
        let hash_to_remove = if let Some(stored) = store.keys.get_mut(id) {
            if stored.metadata.revoked {
                return Ok(false);
            }
            stored.metadata.revoked = true;
            Some(stored.hash)
        } else {
            None
        };
        if let Some(hash) = hash_to_remove {
            store.hash_to_id.remove(&hash);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn list_keys(&self) -> Result<Vec<ApiKeyMetadata>, StoreError> {
        let store = self.inner.read().await;
        Ok(store
            .keys
            .values()
            .map(StoredApiKey::snapshot_metadata)
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Postgres implementation
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
mod postgres_store {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use tokio::sync::RwLock;

    use super::{
        API_KEY_PREFIX, ApiKeyMetadata, ApiKeyStore, StoreError, generate_api_key, sha256_hash,
        valid_api_key,
    };

    const VALIDATION_CACHE_TTL: Duration = Duration::from_secs(15);
    const VALIDATION_CACHE_MAX_ENTRIES: usize = 10_000;
    const LAST_USED_UPDATE_DEBOUNCE_SECS: i64 = 300;
    /// Fixed advisory transaction lock key for serializing bootstrap key imports
    /// without taking a table-level lock that blocks concurrent `validate_key` calls.
    const BOOTSTRAP_IMPORT_ADVISORY_LOCK_ID: i64 = 0x4D52_4B45_595F_494D;

    #[derive(Clone)]
    struct CachedValidation {
        key_id: String,
        expires_at: Instant,
    }

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
        validation_cache: Arc<RwLock<HashMap<[u8; 32], CachedValidation>>>,
    }

    impl PostgresApiKeyStore {
        #[must_use]
        pub fn new(pool: sqlx::PgPool) -> Self {
            Self {
                pool,
                validation_cache: Arc::new(RwLock::new(HashMap::new())),
            }
        }

        /// Ensure a pre-existing raw key is represented in the persistent
        /// store without ever retaining the raw value. This is intended for
        /// recovery and migration, where clients already hold a key that must
        /// remain valid after rebuilding the server database.
        ///
        /// The operation is idempotent. A previously revoked matching key is
        /// never resurrected.
        ///
        /// # Errors
        ///
        /// Returns [`StoreError::BadKey`] when `raw_key` is malformed and a
        /// storage error when the import cannot be read or committed.
        pub async fn ensure_imported_key(
            &self,
            name: &str,
            raw_key: &str,
        ) -> Result<ApiKeyMetadata, StoreError> {
            if !valid_api_key(raw_key) {
                return Err(StoreError::BadKey);
            }

            let hash = sha256_hash(raw_key.as_bytes());
            let prefix: String = raw_key.chars().take(8 + API_KEY_PREFIX.len()).collect();
            let mut transaction = self
                .pool
                .begin()
                .await
                .map_err(|error| StoreError::Internal(error.to_string()))?;

            // Serialize bootstrap imports with an advisory transaction lock so
            // concurrent starting replicas cannot duplicate rows while live
            // validation queries on other replicas remain completely unblocked.
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(BOOTSTRAP_IMPORT_ADVISORY_LOCK_ID)
                .execute(&mut *transaction)
                .await
                .map_err(|error| StoreError::Internal(error.to_string()))?;

            let existing = sqlx::query_as::<_, KeyRow>(
                "SELECT id, name, prefix, created_at, last_used_at, revoked_at \
                 FROM server_api_keys WHERE hash = $1 \
                 ORDER BY (revoked_at IS NULL) DESC, created_at DESC LIMIT 1",
            )
            .bind(&hash[..])
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| StoreError::Internal(error.to_string()))?;

            if let Some(row) = existing {
                if row.revoked_at.is_some() {
                    return Err(StoreError::Internal(
                        "bootstrap API key was previously revoked".to_string(),
                    ));
                }
                transaction
                    .commit()
                    .await
                    .map_err(|error| StoreError::Internal(error.to_string()))?;
                return Ok(ApiKeyMetadata::from(row));
            }

            let row = sqlx::query_as::<_, KeyRow>(
                "INSERT INTO server_api_keys (id, name, prefix, hash) \
                 VALUES ($1, $2, $3, $4) \
                 RETURNING id, name, prefix, created_at, last_used_at, revoked_at",
            )
            .bind(uuid::Uuid::new_v4())
            .bind(name)
            .bind(prefix)
            .bind(&hash[..])
            .fetch_one(&mut *transaction)
            .await
            .map_err(|error| StoreError::Internal(error.to_string()))?;

            transaction
                .commit()
                .await
                .map_err(|error| StoreError::Internal(error.to_string()))?;
            Ok(ApiKeyMetadata::from(row))
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
            let now = Instant::now();

            {
                let cache = self.validation_cache.read().await;
                if let Some(entry) = cache.get(&hash)
                    && entry.expires_at > now
                {
                    return Ok(Some(entry.key_id.clone()));
                }
            }

            // Read-only lookup avoids serializing concurrent inference requests
            // behind a Postgres row-level UPDATE lock on the same key row.
            let row: Option<(uuid::Uuid, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
                "SELECT id, last_used_at FROM server_api_keys \
                 WHERE hash = $1 AND revoked_at IS NULL \
                 LIMIT 1",
            )
            .bind(&hash[..])
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::Internal(e.to_string()))?;

            let Some((id, last_used_at)) = row else {
                return Ok(None);
            };
            let key_id = id.to_string();

            {
                let mut cache = self.validation_cache.write().await;
                if cache.len() >= VALIDATION_CACHE_MAX_ENTRIES {
                    cache.retain(|_, entry| entry.expires_at > now);
                    if cache.len() >= VALIDATION_CACHE_MAX_ENTRIES {
                        cache.clear();
                    }
                }
                cache.insert(
                    hash,
                    CachedValidation {
                        key_id: key_id.clone(),
                        expires_at: now + VALIDATION_CACHE_TTL,
                    },
                );
            }

            // Debounce last_used_at writes asynchronously so high-concurrency
            // traffic does not create row-lock contention or WAL bloat.
            let should_bump = last_used_at.is_none_or(|ts| {
                chrono::Utc::now().signed_duration_since(ts).num_seconds()
                    >= LAST_USED_UPDATE_DEBOUNCE_SECS
            });
            if should_bump {
                let pool = self.pool.clone();
                tokio::spawn(async move {
                    let _ = sqlx::query(
                        "UPDATE server_api_keys SET last_used_at = now() \
                         WHERE id = $1 AND revoked_at IS NULL \
                           AND (last_used_at IS NULL OR last_used_at < now() - interval '5 minutes')",
                    )
                    .bind(id)
                    .execute(&pool)
                    .await;
                });
            }

            Ok(Some(key_id))
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

            if result.rows_affected() > 0 {
                let mut cache = self.validation_cache.write().await;
                cache.retain(|_, entry| entry.key_id != id);
                Ok(true)
            } else {
                Ok(false)
            }
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

fn valid_api_key(raw_key: &str) -> bool {
    raw_key.strip_prefix(API_KEY_PREFIX).is_some_and(|suffix| {
        suffix.len() == API_KEY_RANDOM_LEN
            && suffix.is_ascii()
            && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
    })
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{API_KEY_PREFIX, API_KEY_RANDOM_LEN, generate_api_key, valid_api_key};

    #[test]
    fn generated_keys_are_importable() {
        assert!(valid_api_key(&generate_api_key()));
    }

    #[test]
    fn imported_keys_require_the_exact_native_format() {
        assert!(valid_api_key(&format!(
            "{API_KEY_PREFIX}{}",
            "a".repeat(API_KEY_RANDOM_LEN)
        )));
        assert!(!valid_api_key("not-a-modelrelay-key"));
        assert!(!valid_api_key(&format!(
            "{API_KEY_PREFIX}{}",
            "a".repeat(API_KEY_RANDOM_LEN - 1)
        )));
        assert!(!valid_api_key(&format!(
            "{API_KEY_PREFIX}{}!",
            "a".repeat(API_KEY_RANDOM_LEN - 1)
        )));
    }
}
