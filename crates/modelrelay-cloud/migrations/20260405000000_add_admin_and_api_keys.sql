-- Add admin flag to users
ALTER TABLE users ADD COLUMN IF NOT EXISTS is_admin BOOLEAN NOT NULL DEFAULT false;

-- Create proper api_keys association table
CREATE TABLE IF NOT EXISTS api_keys (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key_id TEXT NOT NULL UNIQUE,
    raw_key TEXT NOT NULL,
    name TEXT NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_api_keys_user_id ON api_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_active ON api_keys(user_id) WHERE revoked_at IS NULL;

-- Migrate any existing api_key data from users table into api_keys table
-- before dropping the column
INSERT INTO api_keys (user_id, key_id, raw_key, name)
SELECT u.id, COALESCE(s.api_key_id, 'migrated-' || u.id::text), u.api_key, 'migrated-key'
FROM users u
LEFT JOIN subscriptions s ON s.user_id = u.id
WHERE u.api_key IS NOT NULL;

-- Drop the old single-key column on users
ALTER TABLE users DROP COLUMN IF EXISTS api_key;
