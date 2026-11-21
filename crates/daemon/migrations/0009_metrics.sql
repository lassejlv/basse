-- Container metrics, sampled every 10 seconds. One row per service, container (the compose
-- service, '' for a single container), resolution and bucket. Resolution 10 holds the raw
-- samples; 60, 600 and 3600 are rollups that every sample is folded into as it arrives. Rows
-- keep sums rather than averages, so a rollup stays exact however many samples it got.
CREATE TABLE metrics (
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    container TEXT NOT NULL,
    resolution INTEGER NOT NULL,
    at TIMESTAMPTZ NOT NULL,
    samples INTEGER NOT NULL,
    cpu_sum DOUBLE PRECISION NOT NULL,
    cpu_max DOUBLE PRECISION NOT NULL,
    mem_sum DOUBLE PRECISION NOT NULL,
    mem_max BIGINT NOT NULL,
    mem_limit BIGINT NOT NULL,
    -- Samples with I/O rates: a container's first sample has nothing to compare with.
    rate_samples INTEGER NOT NULL,
    net_rx_sum DOUBLE PRECISION NOT NULL,
    net_tx_sum DOUBLE PRECISION NOT NULL,
    disk_read_sum DOUBLE PRECISION NOT NULL,
    disk_write_sum DOUBLE PRECISION NOT NULL,
    pids_max BIGINT NOT NULL,
    -- As of the newest sample.
    restarts BIGINT NOT NULL,
    uptime BIGINT NOT NULL,
    -- Requests through the proxy to the domains routed here, over http_secs seconds.
    http_secs DOUBLE PRECISION NOT NULL,
    s2xx BIGINT NOT NULL,
    s3xx BIGINT NOT NULL,
    s4xx BIGINT NOT NULL,
    s5xx BIGINT NOT NULL,
    -- Latency percentiles weighted by the requests they describe.
    latency_requests BIGINT NOT NULL,
    p50_wsum DOUBLE PRECISION NOT NULL,
    p95_wsum DOUBLE PRECISION NOT NULL,
    PRIMARY KEY (service_id, container, resolution, at)
);

CREATE INDEX metrics_expiry ON metrics (resolution, at);
