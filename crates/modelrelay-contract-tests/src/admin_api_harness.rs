//! Contract test harness for the admin HTTP API wire types.
//!
//! These tests pin the exact JSON wire format produced by the shared admin API types
//! in `modelrelay-protocol`. They use pinned string literals (not round-trip checks) so
//! that any accidental `#[serde(rename)]` or field rename is caught immediately.

use modelrelay_protocol::admin_api::{
    AdminKeysResponse, ApiKeyMetadata, CreateKeyRequest, CreateKeyResponse,
};

/// Build a canonical `ApiKeyMetadata` value for use across tests.
fn sample_metadata() -> ApiKeyMetadata {
    ApiKeyMetadata {
        id: "550e8400-e29b-41d4-a716-446655440000".to_string(),
        name: "test-key".to_string(),
        prefix: "mr_live_abcd1234".to_string(),
        created_at: 1_700_000_000,
        last_used_at: Some(1_700_001_000),
        revoked: false,
    }
}

/// `CreateKeyRequest` must serialize to `{"name":"..."}`.
///
/// # Panics
///
/// Panics if the wire format does not match the pinned expectation.
#[must_use]
pub fn test_create_key_request_serialization() -> bool {
    let req = CreateKeyRequest {
        name: "my-key".to_string(),
    };
    let json = serde_json::to_string(&req).expect("serialize CreateKeyRequest");
    let expected = r#"{"name":"my-key"}"#;
    assert_eq!(json, expected, "CreateKeyRequest wire format changed");
    true
}

/// `CreateKeyResponse` must deserialize from a pinned JSON literal that matches the
/// exact field names the server emits.
///
/// # Panics
///
/// Panics if the pinned JSON cannot be deserialized or field values differ.
#[must_use]
pub fn test_create_key_response_deserialization() -> bool {
    let json = r#"{
        "id": "550e8400-e29b-41d4-a716-446655440000",
        "name": "test-key",
        "prefix": "mr_live_abcd1234",
        "created_at": 1700000000,
        "last_used_at": 1700001000,
        "revoked": false,
        "secret": "mr_live_abcdefghijklmnopqrstuvwxyz123456"
    }"#;

    let resp: CreateKeyResponse =
        serde_json::from_str(json).expect("deserialize CreateKeyResponse from pinned JSON");

    assert_eq!(resp.metadata.id, "550e8400-e29b-41d4-a716-446655440000");
    assert_eq!(resp.metadata.name, "test-key");
    assert_eq!(resp.metadata.prefix, "mr_live_abcd1234");
    assert_eq!(resp.metadata.created_at, 1_700_000_000);
    assert_eq!(resp.metadata.last_used_at, Some(1_700_001_000));
    assert!(!resp.metadata.revoked);
    assert_eq!(resp.secret, "mr_live_abcdefghijklmnopqrstuvwxyz123456");
    true
}

/// `AdminKeysResponse` must round-trip through serialize/deserialize with structural equality.
///
/// # Panics
///
/// Panics if the round-trip produces different values.
#[must_use]
pub fn test_admin_keys_response_roundtrip() -> bool {
    let original = AdminKeysResponse {
        keys: vec![
            sample_metadata(),
            ApiKeyMetadata {
                id: "660e8400-e29b-41d4-a716-446655440001".to_string(),
                name: "revoked-key".to_string(),
                prefix: "mr_live_wxyz5678".to_string(),
                created_at: 1_699_000_000,
                last_used_at: None,
                revoked: true,
            },
        ],
    };

    let json = serde_json::to_string(&original).expect("serialize AdminKeysResponse");
    let decoded: AdminKeysResponse =
        serde_json::from_str(&json).expect("deserialize AdminKeysResponse");

    assert_eq!(decoded.keys.len(), 2);
    assert_eq!(decoded.keys[0], original.keys[0]);
    assert_eq!(decoded.keys[1], original.keys[1]);
    true
}

/// The field names produced by `ApiKeyMetadata` serialized standalone must match the
/// field names produced when it is `#[serde(flatten)]`ed inside `CreateKeyResponse`.
/// This catches divergence caused by `rename_all` or per-field renames.
///
/// # Panics
///
/// Panics if standalone and flattened serialization produce different field names.
#[must_use]
pub fn test_metadata_flatten_ordering_is_stable() -> bool {
    let meta = sample_metadata();

    // Standalone serialization
    let standalone: serde_json::Value =
        serde_json::to_value(&meta).expect("serialize standalone metadata");

    // Flattened inside CreateKeyResponse
    let response = CreateKeyResponse {
        metadata: meta,
        secret: "mr_live_placeholder".to_string(),
    };
    let mut flattened: serde_json::Value =
        serde_json::to_value(&response).expect("serialize CreateKeyResponse");

    // Remove the `secret` field so we can compare just the metadata fields
    flattened
        .as_object_mut()
        .expect("response is an object")
        .remove("secret");

    // The remaining fields must be identical
    assert_eq!(
        standalone, flattened,
        "ApiKeyMetadata field names differ between standalone and flattened serialization"
    );
    true
}

/// Pinned JSON field-name check: serializing `CreateKeyResponse` must produce exactly
/// these top-level field names. This catches renames that `serde(rename_all)` would apply.
///
/// # Panics
///
/// Panics if expected field names are missing or forbidden camelCase names appear.
#[must_use]
pub fn test_create_key_response_field_names() -> bool {
    let resp = CreateKeyResponse {
        metadata: sample_metadata(),
        secret: "mr_live_placeholder".to_string(),
    };
    let json = serde_json::to_string(&resp).expect("serialize CreateKeyResponse");

    // These exact field names are part of the wire contract
    let expected_fields = [
        "\"id\":",
        "\"name\":",
        "\"prefix\":",
        "\"created_at\":",
        "\"last_used_at\":",
        "\"revoked\":",
        "\"secret\":",
    ];
    for field in &expected_fields {
        assert!(
            json.contains(field),
            "CreateKeyResponse JSON missing expected field {field}: {json}"
        );
    }

    // Must NOT contain camelCase variants
    let forbidden_fields = [
        "\"createdAt\":",
        "\"lastUsedAt\":",
        "\"keyId\":",
        "\"apiKey\":",
    ];
    for field in &forbidden_fields {
        assert!(
            !json.contains(field),
            "CreateKeyResponse JSON contains unexpected camelCase field {field}: {json}"
        );
    }
    true
}
