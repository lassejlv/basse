-- S3-compatible storage connections for backups.

CREATE TABLE storage_connections (
    id TEXT PRIMARY KEY,
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    name TEXT NOT NULL UNIQUE,
    endpoint TEXT NOT NULL,
    region TEXT NOT NULL,
    bucket TEXT NOT NULL,
    path_prefix TEXT NOT NULL DEFAULT '',
    path_style BOOLEAN NOT NULL,
    credentials_secret_id TEXT NOT NULL REFERENCES secrets(id),
    created_at TIMESTAMPTZ NOT NULL,
    last_test JSONB
);

ALTER TABLE backups ADD FOREIGN KEY (storage_id) REFERENCES storage_connections(id);
ALTER TABLE backup_schedules ADD COLUMN storage_id TEXT REFERENCES storage_connections(id);
