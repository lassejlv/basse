-- Encrypted archives of the daemon itself (store dump, config, master key, ssh).

CREATE TABLE instance_backups (
    id TEXT PRIMARY KEY,
    number BIGINT NOT NULL UNIQUE,
    status TEXT NOT NULL,
    trigger TEXT NOT NULL,
    file_name TEXT NOT NULL,
    size_bytes BIGINT NOT NULL DEFAULT 0,
    storage TEXT NOT NULL,
    storage_id TEXT REFERENCES storage_connections(id),
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ
);

-- One row. `passphrase` is sealed with the master key so scheduled runs can encrypt.
CREATE TABLE instance_backup_schedule (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    enabled BOOLEAN NOT NULL,
    hour_utc INTEGER NOT NULL,
    keep BIGINT NOT NULL,
    storage_id TEXT REFERENCES storage_connections(id),
    passphrase BYTEA,
    last_run_at TIMESTAMPTZ
);
