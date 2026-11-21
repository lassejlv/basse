-- Secrets, encrypted with the master key in <data dir>/master.key. `ciphertext` is
-- version || nonce || XChaCha20-Poly1305 output, with the id as associated data.

CREATE TABLE secrets (
    id TEXT PRIMARY KEY,
    seq BIGINT GENERATED ALWAYS AS IDENTITY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    public_part TEXT,
    ciphertext BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

-- Servers connect with this key instead of the daemon's own one when it is set.
ALTER TABLE servers ADD COLUMN ssh_key_secret_id TEXT REFERENCES secrets(id);
