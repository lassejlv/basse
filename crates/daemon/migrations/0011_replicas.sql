-- Replicas: a service runs `replicas` identical containers behind the proxy. `scaling` holds
-- the load-balancing policy, rolling-deploy surge, the shared-volume opt-in and autoscaling as
-- a `Scaling`; NULL is the default.
ALTER TABLE services
    ADD COLUMN replicas INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN scaling JSONB;

-- The container serving each replica slot. A row has its own server so replicas can later be
-- placed on several servers; for now they all run on the service's server.
CREATE TABLE replicas (
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    server_id TEXT NOT NULL,
    deployment_id TEXT NOT NULL,
    container_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (service_id, idx)
);

-- The live container of every single-container service becomes its replica 0. Compose groups
-- keep their containers to themselves.
INSERT INTO replicas (service_id, idx, server_id, deployment_id, container_id, created_at)
SELECT DISTINCT ON (d.service_id) d.service_id, 0, d.server_id, d.id, d.container_id,
       COALESCE(d.finished_at, d.created_at)
FROM deployments d
JOIN services s ON s.id = d.service_id
WHERE d.status = 'live'
  AND d.container_id IS NOT NULL
  AND NOT (s.source->>'type' = 'git' AND s.source->'build'->>'method' = 'compose')
ORDER BY d.service_id, d.number DESC;
