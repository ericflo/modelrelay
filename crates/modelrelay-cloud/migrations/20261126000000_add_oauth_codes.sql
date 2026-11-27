-- Single-use OAuth 2.0 PKCE authorization codes for browser & CLI apps
-- (e.g. chat.neuralcolumn.com, OpenRouter-style PKCE key exchange).
-- Raw codes are never persisted; only their SHA-256 hex digest is stored.
CREATE TABLE IF NOT EXISTS oauth_authorization_codes (
    code_hash TEXT PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    api_key_row_id UUID NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    callback_url TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    code_challenge_method TEXT NOT NULL DEFAULT 'S256',
    key_label TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_oauth_codes_expires_at
    ON oauth_authorization_codes (expires_at);
CREATE INDEX IF NOT EXISTS idx_oauth_codes_user_id
    ON oauth_authorization_codes (user_id);
