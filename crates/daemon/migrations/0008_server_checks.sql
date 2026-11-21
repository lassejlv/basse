-- The preflight checklist and the problem found by a server's last check.
ALTER TABLE servers
    ADD COLUMN checks JSONB NOT NULL DEFAULT '[]',
    ADD COLUMN problem JSONB;
