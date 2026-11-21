-- Git sources services clone from: GitHub Apps and personal access tokens. Credentials live in
-- `secrets`; services point at a source through `source->>'source_id'`.

CREATE TABLE git_sources (
    id TEXT PRIMARY KEY,
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    html_url TEXT NOT NULL,
    api_url TEXT NOT NULL,
    -- Organization the manifest flow creates the app in; NULL for a personal account.
    organization TEXT,
    owner TEXT,
    owner_kind TEXT,
    app_id BIGINT,
    app_slug TEXT,
    installation_id BIGINT,
    status TEXT NOT NULL,
    status_detail TEXT,
    secret_id TEXT REFERENCES secrets(id),
    -- SHA-256 of the state the manifest flow's browser pages must present, and its expiry.
    setup_state_sha256 TEXT,
    setup_expires_at TIMESTAMPTZ,
    last_checked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL
);
