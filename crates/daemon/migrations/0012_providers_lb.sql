-- Provider connections (Hetzner Cloud, Cloudflare) and the managed load balancers the daemon
-- keeps in front of services. The API token is a secret; `state` is what the reconciler last
-- applied and created at the provider, as a `lb::LbStored`.

CREATE TABLE providers (
    id TEXT PRIMARY KEY,
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    token_secret_id TEXT NOT NULL REFERENCES secrets(id),
    status TEXT NOT NULL,
    problem TEXT,
    account TEXT,
    load_balancing BOOLEAN NOT NULL DEFAULT FALSE,
    last_checked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE service_lbs (
    service_id TEXT PRIMARY KEY REFERENCES services(id) ON DELETE CASCADE,
    provider_id TEXT NOT NULL REFERENCES providers(id),
    config JSONB NOT NULL,
    state JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
