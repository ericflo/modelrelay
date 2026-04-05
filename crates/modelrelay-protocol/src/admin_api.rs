//! HTTP wire types for the server's `/admin/*` API, consumed by `modelrelay-cloud`.
//!
//! These types are the **single source of truth** for the admin API contract. Any rename,
//! addition, or removal must be made here — both server and cloud pick it up via their
//! shared dependency. Direct `serde_json::Value` parsing in cloud handlers is prohibited;
//! use these structs instead.

use serde::{Deserialize, Serialize};

/// Request body for `POST /admin/keys`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateKeyRequest {
    /// Human-readable label for the key (e.g. `"user-alice@example.com"`).
    pub name: String,
}

/// Metadata about an API key. Returned by `GET /admin/keys` and (flattened) by
/// `POST /admin/keys`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeyMetadata {
    /// Unique key identifier (UUID).
    pub id: String,
    /// Human-readable name assigned at creation time.
    pub name: String,
    /// Non-secret prefix of the raw key (e.g. `"mr_live_abcd1234"`).
    pub prefix: String,
    /// Unix epoch seconds when the key was created.
    pub created_at: u64,
    /// Unix epoch seconds of the most recent successful validation, if any.
    pub last_used_at: Option<u64>,
    /// Whether the key has been revoked.
    pub revoked: bool,
}

/// Response body for `POST /admin/keys`. Includes the raw secret exactly once — the
/// server never returns it again.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateKeyResponse {
    /// All non-secret metadata for the key.
    #[serde(flatten)]
    pub metadata: ApiKeyMetadata,
    /// The raw API key, shown exactly once at creation time. Callers must store this —
    /// the server does not retain a recoverable copy.
    pub secret: String,
}

/// Response body for `GET /admin/keys`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminKeysResponse {
    /// List of all key metadata (secrets are never included).
    pub keys: Vec<ApiKeyMetadata>,
}
