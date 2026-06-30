// Package caddyx drives Caddy's admin API over its unix socket. Stdlib-only,
// the same dial pattern as dockerx. The socket is on a volume shared only with
// the Caddy container — the admin API is never a TCP listener and never public.
package caddyx

import (
	"bytes"
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"time"
)

// Client talks to Caddy's admin API over a unix socket.
type Client struct {
	http *http.Client
}

// New returns a Client that dials the given admin unix socket path.
func New(socketPath string) *Client {
	return &Client{
		http: &http.Client{
			Timeout: 15 * time.Second,
			Transport: &http.Transport{
				DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
					var d net.Dialer
					return d.DialContext(ctx, "unix", socketPath)
				},
			},
		},
	}
}

// Load atomically replaces Caddy's entire config (admin POST /load). Caddy
// validates the payload and keeps the prior config on error, so a route set is
// never half-applied.
func (c *Client) Load(ctx context.Context, config []byte) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://caddy/load", bytes.NewReader(config))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return fmt.Errorf("caddy load: status %d: %s", resp.StatusCode, body)
	}
	return nil
}

// Ping returns nil if the admin API is reachable (GET /config/).
func (c *Client) Ping(ctx context.Context) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://caddy/config/", nil)
	if err != nil {
		return err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return fmt.Errorf("caddy ping: status %d", resp.StatusCode)
	}
	return nil
}
