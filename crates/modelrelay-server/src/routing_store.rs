//! Persistent storage backends for per-owner [`TenantRoutingConfig`].
//!
//! Mirrors [`crate::api_keys`]: supports `PostgreSQL` when `DATABASE_URL` is set,
//! an atomic JSON file when `--routing-config` is passed in self-hosted mode,
//! or an in-memory fallback for tests and ephemeral deployments.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use modelrelay_protocol::admin_api::{ModelGroupMode, TenantRoutingConfig};
use tokio::sync::RwLock;

/// Maximum number of custom routing groups permitted per owner scope.
pub const MAX_ROUTING_GROUPS: usize = 128;

/// Maximum number of exact per-model overrides permitted per owner scope.
pub const MAX_MODEL_OVERRIDES: usize = 512;

/// Maximum number of targets inside a single custom group.
pub const MAX_TARGETS_PER_GROUP: usize = 64;

/// Maximum worker priority entries inside a single policy.
pub const MAX_PRIORITY_ENTRIES: usize = 64;

/// Errors returned by [`RoutingConfigStore`] or config validation.
#[derive(Debug)]
pub enum RoutingStoreError {
    /// Client-supplied configuration failed validation.
    InvalidConfig(String),
    /// Internal storage or I/O failure.
    Internal(String),
}

impl fmt::Display for RoutingStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "invalid routing config: {msg}"),
            Self::Internal(msg) => write!(f, "routing store error: {msg}"),
        }
    }
}

impl std::error::Error for RoutingStoreError {}

/// Validate and normalize a [`TenantRoutingConfig`] before persisting or applying it.
///
/// # Errors
///
/// Returns [`RoutingStoreError::InvalidConfig`] when limits, naming rules, or
/// exact-model invariants are violated.
pub fn validate_and_normalize_config(
    mut config: TenantRoutingConfig,
) -> Result<TenantRoutingConfig, RoutingStoreError> {
    validate_policy(&mut config.default_policy)?;

    if config.model_overrides.len() > MAX_MODEL_OVERRIDES {
        return Err(RoutingStoreError::InvalidConfig(format!(
            "too many model overrides (max {MAX_MODEL_OVERRIDES})"
        )));
    }
    let mut cleaned_overrides = std::collections::BTreeMap::new();
    for (raw_model, mut policy) in config.model_overrides {
        let model = raw_model.trim().to_string();
        if model.is_empty() || model == "*" {
            return Err(RoutingStoreError::InvalidConfig(
                "model override keys must be non-empty exact model IDs".to_string(),
            ));
        }
        validate_policy(&mut policy)?;
        cleaned_overrides.insert(model, policy);
    }
    config.model_overrides = cleaned_overrides;

    if config.groups.len() > MAX_ROUTING_GROUPS {
        return Err(RoutingStoreError::InvalidConfig(format!(
            "too many routing groups (max {MAX_ROUTING_GROUPS})"
        )));
    }

    let mut seen_group_ids = HashSet::new();
    let mut seen_pool_ids = HashSet::new();
    for group in &mut config.groups {
        group.id = group.id.trim().to_string();
        if group.id.is_empty() {
            group.id = uuid::Uuid::new_v4().to_string();
        }
        if !seen_group_ids.insert(group.id.clone()) {
            return Err(RoutingStoreError::InvalidConfig(format!(
                "duplicate routing group id '{}'",
                group.id
            )));
        }
        group.name = group.name.trim().to_string();
        if group.name.is_empty() {
            return Err(RoutingStoreError::InvalidConfig(
                "routing group name cannot be empty".to_string(),
            ));
        }
        if group.targets.is_empty() {
            return Err(RoutingStoreError::InvalidConfig(format!(
                "routing group '{}' must include at least one model target",
                group.name
            )));
        }
        if group.targets.len() > MAX_TARGETS_PER_GROUP {
            return Err(RoutingStoreError::InvalidConfig(format!(
                "routing group '{}' has too many targets (max {MAX_TARGETS_PER_GROUP})",
                group.name
            )));
        }
        for target in &mut group.targets {
            target.model_id = target.model_id.trim().to_string();
            if target.model_id.is_empty() || target.model_id == "*" {
                return Err(RoutingStoreError::InvalidConfig(format!(
                    "routing group '{}' targets must use exact non-empty model IDs",
                    group.name
                )));
            }
            target.worker_name = target
                .worker_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned);
        }

        if group.mode == ModelGroupMode::UnifiedPool {
            let pool_id = group
                .pool_model_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(&group.name)
                .to_string();
            if pool_id.is_empty() || pool_id == "*" {
                return Err(RoutingStoreError::InvalidConfig(format!(
                    "unified pool group '{}' must specify a valid pool_model_id",
                    group.name
                )));
            }
            if !seen_pool_ids.insert(pool_id.clone()) {
                return Err(RoutingStoreError::InvalidConfig(format!(
                    "duplicate unified pool model ID '{pool_id}'"
                )));
            }
            group.pool_model_id = Some(pool_id);
        } else {
            group.pool_model_id = None;
        }

        validate_policy(&mut group.policy)?;
    }

    Ok(config)
}

fn validate_policy(
    policy: &mut modelrelay_protocol::admin_api::RoutingPolicyConfig,
) -> Result<(), RoutingStoreError> {
    if policy.prefix_ttl_secs == 0 || policy.prefix_ttl_secs > 86_400 {
        return Err(RoutingStoreError::InvalidConfig(
            "prefix_ttl_secs must be between 1 and 86400".to_string(),
        ));
    }
    if policy.min_prefix_bytes > 1024 * 1024 {
        return Err(RoutingStoreError::InvalidConfig(
            "min_prefix_bytes cannot exceed 1048576".to_string(),
        ));
    }
    if policy.worker_priority.len() > MAX_PRIORITY_ENTRIES {
        return Err(RoutingStoreError::InvalidConfig(format!(
            "worker_priority cannot exceed {MAX_PRIORITY_ENTRIES} entries"
        )));
    }
    let mut cleaned = Vec::with_capacity(policy.worker_priority.len());
    let mut seen = HashSet::new();
    for entry in &policy.worker_priority {
        let trimmed = entry.trim();
        if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
            cleaned.push(trimmed.to_string());
        }
    }
    policy.worker_priority = cleaned;
    Ok(())
}

/// Backend-agnostic persistence for routing configurations.
#[async_trait::async_trait]
pub trait RoutingConfigStore: Send + Sync + 'static {
    /// Load every persisted scope's routing config on server startup.
    async fn load_all(
        &self,
    ) -> Result<HashMap<Option<String>, TenantRoutingConfig>, RoutingStoreError>;

    /// Persist the updated routing configuration for one owner scope (`None` = global).
    async fn save_config(
        &self,
        owner_key_id: Option<&str>,
        config: &TenantRoutingConfig,
    ) -> Result<(), RoutingStoreError>;
}

// ---------------------------------------------------------------------------
// In-memory store
// ---------------------------------------------------------------------------

/// Ephemeral in-memory routing config store (used in tests and zero-DB setups).
#[derive(Debug, Default, Clone)]
pub struct InMemoryRoutingConfigStore {
    inner: Arc<RwLock<HashMap<Option<String>, TenantRoutingConfig>>>,
}

impl InMemoryRoutingConfigStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait::async_trait]
impl RoutingConfigStore for InMemoryRoutingConfigStore {
    async fn load_all(
        &self,
    ) -> Result<HashMap<Option<String>, TenantRoutingConfig>, RoutingStoreError> {
        Ok(self.inner.read().await.clone())
    }

    async fn save_config(
        &self,
        owner_key_id: Option<&str>,
        config: &TenantRoutingConfig,
    ) -> Result<(), RoutingStoreError> {
        self.inner
            .write()
            .await
            .insert(owner_key_id.map(ToOwned::to_owned), config.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// JSON file store (for self-hosted persistence without Postgres)
// ---------------------------------------------------------------------------

/// Atomic JSON-file-backed routing config store for self-hosted deployments.
#[derive(Debug, Clone)]
pub struct FileRoutingConfigStore {
    path: PathBuf,
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

impl FileRoutingConfigStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

#[async_trait::async_trait]
impl RoutingConfigStore for FileRoutingConfigStore {
    async fn load_all(
        &self,
    ) -> Result<HashMap<Option<String>, TenantRoutingConfig>, RoutingStoreError> {
        match tokio::fs::read_to_string(&self.path).await {
            Ok(raw) => {
                if raw.trim().is_empty() {
                    return Ok(HashMap::new());
                }
                let raw_map: HashMap<String, TenantRoutingConfig> = serde_json::from_str(&raw)
                    .map_err(|e| RoutingStoreError::Internal(format!("parse routing file: {e}")))?;
                let mut out = HashMap::with_capacity(raw_map.len());
                for (scope, cfg) in raw_map {
                    let key = if scope.is_empty() { None } else { Some(scope) };
                    out.insert(key, cfg);
                }
                Ok(out)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(RoutingStoreError::Internal(format!(
                "read routing file: {e}"
            ))),
        }
    }

    async fn save_config(
        &self,
        owner_key_id: Option<&str>,
        config: &TenantRoutingConfig,
    ) -> Result<(), RoutingStoreError> {
        let _guard = self.write_lock.lock().await;
        let mut all = self.load_all().await?;
        all.insert(owner_key_id.map(ToOwned::to_owned), config.clone());

        let disk_map: std::collections::BTreeMap<String, TenantRoutingConfig> = all
            .into_iter()
            .map(|(k, v)| (k.unwrap_or_default(), v))
            .collect();
        let serialized = serde_json::to_string_pretty(&disk_map)
            .map_err(|e| RoutingStoreError::Internal(format!("serialize routing file: {e}")))?;

        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| RoutingStoreError::Internal(format!("create parent dir: {e}")))?;
        }
        let tmp_path = self.path.with_extension("json.tmp");
        tokio::fs::write(&tmp_path, serialized)
            .await
            .map_err(|e| RoutingStoreError::Internal(format!("write temp routing file: {e}")))?;
        tokio::fs::rename(&tmp_path, &self.path)
            .await
            .map_err(|e| RoutingStoreError::Internal(format!("rename routing file: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL store
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
mod postgres_routing_store {
    use std::collections::HashMap;

    use modelrelay_protocol::admin_api::TenantRoutingConfig;

    use super::{RoutingConfigStore, RoutingStoreError};

    /// PostgreSQL-backed routing configuration store.
    #[derive(Clone)]
    pub struct PostgresRoutingConfigStore {
        pool: sqlx::PgPool,
    }

    impl PostgresRoutingConfigStore {
        #[must_use]
        pub fn new(pool: sqlx::PgPool) -> Self {
            Self { pool }
        }
    }

    #[async_trait::async_trait]
    impl RoutingConfigStore for PostgresRoutingConfigStore {
        async fn load_all(
            &self,
        ) -> Result<HashMap<Option<String>, TenantRoutingConfig>, RoutingStoreError> {
            let rows: Vec<(String, serde_json::Value)> =
                sqlx::query_as("SELECT owner_scope, config_json FROM server_routing_configs")
                    .fetch_all(&self.pool)
                    .await
                    .map_err(|e| RoutingStoreError::Internal(e.to_string()))?;

            let mut out = HashMap::with_capacity(rows.len());
            for (scope, json_val) in rows {
                match serde_json::from_value::<TenantRoutingConfig>(json_val) {
                    Ok(cfg) => {
                        let key = if scope.is_empty() { None } else { Some(scope) };
                        out.insert(key, cfg);
                    }
                    Err(e) => {
                        tracing::warn!(
                            owner_scope = %scope,
                            error = %e,
                            "ignoring malformed persisted routing config row"
                        );
                    }
                }
            }
            Ok(out)
        }

        async fn save_config(
            &self,
            owner_key_id: Option<&str>,
            config: &TenantRoutingConfig,
        ) -> Result<(), RoutingStoreError> {
            let scope = owner_key_id.unwrap_or("");
            let json_val = serde_json::to_value(config)
                .map_err(|e| RoutingStoreError::Internal(e.to_string()))?;
            sqlx::query(
                "INSERT INTO server_routing_configs (owner_scope, config_json, updated_at) \
                 VALUES ($1, $2, now()) \
                 ON CONFLICT (owner_scope) \
                 DO UPDATE SET config_json = EXCLUDED.config_json, updated_at = now()",
            )
            .bind(scope)
            .bind(json_val)
            .execute(&self.pool)
            .await
            .map_err(|e| RoutingStoreError::Internal(e.to_string()))?;
            Ok(())
        }
    }
}

#[cfg(feature = "postgres")]
pub use postgres_routing_store::PostgresRoutingConfigStore;
