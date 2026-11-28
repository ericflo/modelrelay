CREATE TABLE IF NOT EXISTS server_routing_configs (
    owner_scope TEXT PRIMARY KEY,
    config_json JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
