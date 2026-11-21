-- Volumes the daemon keeps for compose services: named volumes and bind mounts of the
-- repository, as a `ComposeVolume`.
ALTER TABLE volumes ADD COLUMN compose JSONB;
