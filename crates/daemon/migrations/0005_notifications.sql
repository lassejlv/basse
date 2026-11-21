-- Notification channels, rules and the delivery log, plus small state the monitors keep.

-- `config` is the redacted config for listings; `sealed` is the full config sealed with the
-- master key, bound to the channel id.
CREATE TABLE notify_channels (
    id TEXT PRIMARY KEY,
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    enabled BOOLEAN NOT NULL,
    config JSONB NOT NULL,
    sealed BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    last_test JSONB
);

-- `scope_id` is a project, environment or service id; NULL for the whole instance.
CREATE TABLE notify_rules (
    id TEXT PRIMARY KEY,
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    channel_id TEXT NOT NULL REFERENCES notify_channels(id) ON DELETE CASCADE,
    scope_type TEXT NOT NULL,
    scope_id TEXT,
    preset TEXT,
    events JSONB NOT NULL,
    enabled BOOLEAN NOT NULL,
    dedupe_secs BIGINT NOT NULL,
    max_per_hour BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX notify_rules_scope ON notify_rules (scope_id);

CREATE TABLE notify_deliveries (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    at TIMESTAMPTZ NOT NULL,
    channel_id TEXT NOT NULL REFERENCES notify_channels(id) ON DELETE CASCADE,
    rule_id TEXT REFERENCES notify_rules(id) ON DELETE SET NULL,
    event TEXT,
    subject TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    attempts INTEGER NOT NULL,
    error TEXT,
    latency_ms BIGINT
);

-- Small facts that must survive restarts, e.g. the version the daemon last ran as.
CREATE TABLE daemon_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- What the TLS monitor last saw on each HTTPS domain.
CREATE TABLE tls_certificates (
    host TEXT PRIMARY KEY,
    serial TEXT NOT NULL,
    not_before TIMESTAMPTZ NOT NULL,
    not_after TIMESTAMPTZ NOT NULL,
    expiry_warned BOOLEAN NOT NULL DEFAULT FALSE,
    invalid_reported BOOLEAN NOT NULL DEFAULT FALSE,
    checked_at TIMESTAMPTZ NOT NULL
);
