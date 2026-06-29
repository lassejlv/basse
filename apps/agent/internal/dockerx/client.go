// Package dockerx is a tiny client for the Docker Engine API over the mounted
// unix socket. It uses only the standard library: the Engine API is plain HTTP,
// so we dial the socket directly instead of pulling in the heavy docker/docker
// SDK. The agent therefore has zero external dependencies.
package dockerx

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"time"
)

// Client talks to the Docker daemon over its unix socket.
type Client struct {
	http *http.Client
}

// New returns a Client that dials the given unix socket path.
func New(socketPath string) *Client {
	return &Client{
		http: &http.Client{
			Timeout: 10 * time.Second,
			Transport: &http.Transport{
				DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
					var d net.Dialer
					return d.DialContext(ctx, "unix", socketPath)
				},
			},
		},
	}
}

// Version mirrors the subset of GET /version we surface.
type Version struct {
	Version    string `json:"Version"`
	APIVersion string `json:"ApiVersion"`
	Os         string `json:"Os"`
	Arch       string `json:"Arch"`
	KernelVer  string `json:"KernelVersion"`
}

// Info mirrors the subset of GET /info we surface.
type Info struct {
	ID                string `json:"ID"`
	Name              string `json:"Name"`
	ServerVersion     string `json:"ServerVersion"`
	OperatingSystem   string `json:"OperatingSystem"`
	OSType            string `json:"OSType"`
	Architecture      string `json:"Architecture"`
	KernelVersion     string `json:"KernelVersion"`
	NCPU              int    `json:"NCPU"`
	MemTotal          int64  `json:"MemTotal"`
	Containers        int    `json:"Containers"`
	ContainersRunning int    `json:"ContainersRunning"`
	Images            int    `json:"Images"`
}

// Ping verifies the daemon is reachable. Used by the readiness probe.
func (c *Client) Ping(ctx context.Context) error {
	return c.get(ctx, "/_ping", nil)
}

// Version returns the daemon version information.
func (c *Client) Version(ctx context.Context) (Version, error) {
	var out Version
	if err := c.get(ctx, "/version", &out); err != nil {
		return Version{}, err
	}
	return out, nil
}

// Info returns the daemon's system-wide information.
func (c *Client) Info(ctx context.Context) (Info, error) {
	var out Info
	if err := c.get(ctx, "/info", &out); err != nil {
		return Info{}, err
	}
	return out, nil
}

// get issues a GET against the daemon. The host portion is a placeholder; the
// transport always dials the configured unix socket regardless of host.
func (c *Client) get(ctx context.Context, path string, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://docker"+path, nil)
	if err != nil {
		return err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return fmt.Errorf("docker %s: status %d: %s", path, resp.StatusCode, body)
	}

	if out == nil {
		_, _ = io.Copy(io.Discard, resp.Body)
		return nil
	}

	return json.NewDecoder(resp.Body).Decode(out)
}
