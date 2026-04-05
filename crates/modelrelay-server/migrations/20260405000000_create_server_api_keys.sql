CREATE TABLE IF NOT EXISTS server_api_keys (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    hash BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_server_api_keys_prefix
    ON server_api_keys(prefix) WHERE revoked_at IS NULL;

CREATE INDEX IF NOT EXISTS idx_server_api_keys_hash
    ON server_api_keys(hash) WHERE revoked_at IS NULL;
