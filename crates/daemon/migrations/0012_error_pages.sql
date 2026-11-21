-- Whether the proxy serves branded error pages for a service, and custom pages that replace
-- the built-in ones: per service, or the instance default where service_id is NULL. `html` is
-- NULL while a repository page has not been read yet or could not be read.
ALTER TABLE services ADD COLUMN branded_error_pages BOOLEAN NOT NULL DEFAULT TRUE;

CREATE TABLE error_pages (
    service_id TEXT REFERENCES services(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    source JSONB NOT NULL,
    html TEXT,
    warnings JSONB NOT NULL DEFAULT '[]',
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE UNIQUE INDEX error_pages_owner_kind ON error_pages ((COALESCE(service_id, '')), kind);
