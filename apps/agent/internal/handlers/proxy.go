package handlers

import (
	"context"
	"encoding/json"
	"net/http"
	"os"
	"time"

	"github.com/lassejlv/basse/apps/agent/internal/caddyx"
	"github.com/lassejlv/basse/apps/agent/internal/config"
	"github.com/lassejlv/basse/apps/agent/internal/dockerx"
	"github.com/lassejlv/basse/apps/agent/internal/httpx"
)

// Proxy manages the Caddy reverse-proxy container and its routes.
type Proxy struct {
	Docker *dockerx.Client
	Caddy  *caddyx.Client
	Cfg    config.Config
}

type proxyStatus struct {
	Running        bool   `json:"running"`
	AdminReachable bool   `json:"adminReachable"`
	CaddyVersion   string `json:"caddyVersion,omitempty"`
}

// Ensure idempotently brings Caddy up: create the shared network, pull the
// image, write the boot config, (re)create and start the container, then wait
// for the admin API to answer. Bearer-guarded.
func (p Proxy) Ensure(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()
	cfg := p.Cfg

	if err := p.Docker.EnsureNetwork(ctx, cfg.ProxyNetwork); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "ensure network: "+err.Error())
		return
	}

	if err := p.Docker.PullImage(ctx, cfg.CaddyImage); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "pull image: "+err.Error())
		return
	}

	// Boot config: admin on the unix socket + empty HTTP server. Routes arrive
	// later via /v1/proxy/sync.
	initConfig, err := caddyx.BuildConfig(cfg.AdminSocketPath(), nil)
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, "build init config: "+err.Error())
		return
	}
	if err := os.MkdirAll(cfg.AdminDir, 0o755); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "prepare admin dir: "+err.Error())
		return
	}
	if err := os.WriteFile(cfg.InitConfigPath(), initConfig, 0o644); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "write init config: "+err.Error())
		return
	}

	// Recreate the container (volumes — certs in particular — survive rm).
	if err := p.Docker.RemoveContainer(ctx, cfg.CaddyContainer); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "remove old caddy: "+err.Error())
		return
	}

	spec := dockerx.ContainerSpec{
		Image: cfg.CaddyImage,
		Cmd:   []string{"caddy", "run", "--config", cfg.InitConfigPath()},
		ExposedPorts: map[string]struct{}{
			"80/tcp":  {},
			"443/tcp": {},
		},
		HostConfig: dockerx.HostConfig{
			Binds: []string{
				cfg.DataVolume + ":/data",
				cfg.AdminVolume + ":" + cfg.AdminDir,
			},
			PortBindings: map[string][]dockerx.PortBinding{
				"80/tcp":  {{HostPort: "80"}},
				"443/tcp": {{HostPort: "443"}},
			},
			NetworkMode:   cfg.ProxyNetwork,
			RestartPolicy: dockerx.RestartPolicy{Name: "unless-stopped"},
		},
	}

	id, err := p.Docker.CreateContainer(ctx, cfg.CaddyContainer, spec)
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, "create caddy: "+err.Error())
		return
	}
	if err := p.Docker.StartContainer(ctx, id); err != nil {
		// Most commonly a :80/:443 host-port conflict — surface it clearly.
		httpx.Error(w, http.StatusInternalServerError, "start caddy: "+err.Error())
		return
	}

	if err := p.waitForAdmin(ctx); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "caddy admin did not become reachable: "+err.Error())
		return
	}

	httpx.JSON(w, http.StatusOK, p.status(ctx))
}

// Status reports the proxy's running + admin-reachable state. Bearer-guarded.
func (p Proxy) Status(w http.ResponseWriter, r *http.Request) {
	httpx.JSON(w, http.StatusOK, p.status(r.Context()))
}

type syncRequest struct {
	Domains []struct {
		Host     string `json:"host"`
		Upstream string `json:"upstream"`
	} `json:"domains"`
}

// Sync applies the full desired domain set atomically (admin POST /load).
// Bearer-guarded.
func (p Proxy) Sync(w http.ResponseWriter, r *http.Request) {
	var body syncRequest
	if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
		httpx.Error(w, http.StatusBadRequest, "invalid request body")
		return
	}

	domains := make([]caddyx.DesiredDomain, 0, len(body.Domains))
	for _, d := range body.Domains {
		domains = append(domains, caddyx.DesiredDomain{Host: d.Host, Upstream: d.Upstream})
	}

	config, err := caddyx.BuildConfig(p.Cfg.AdminSocketPath(), domains)
	if err != nil {
		httpx.Error(w, http.StatusBadRequest, err.Error())
		return
	}

	if err := p.Caddy.Load(r.Context(), config); err != nil {
		httpx.Error(w, http.StatusBadGateway, "apply config: "+err.Error())
		return
	}

	httpx.JSON(w, http.StatusOK, map[string]any{"ok": true, "domains": len(domains)})
}

func (p Proxy) status(ctx context.Context) proxyStatus {
	state, err := p.Docker.InspectContainer(ctx, p.Cfg.CaddyContainer)
	running := err == nil && state.Running

	adminReachable := false
	if running {
		adminReachable = p.Caddy.Ping(ctx) == nil
	}

	status := proxyStatus{Running: running, AdminReachable: adminReachable}
	if running {
		status.CaddyVersion = p.Cfg.CaddyImage
	}
	return status
}

// waitForAdmin polls the admin socket until it answers or the deadline passes.
func (p Proxy) waitForAdmin(ctx context.Context) error {
	deadline := time.Now().Add(120 * time.Second)
	var lastErr error
	for time.Now().Before(deadline) {
		if err := ctx.Err(); err != nil {
			return err
		}
		if lastErr = p.Caddy.Ping(ctx); lastErr == nil {
			return nil
		}
		time.Sleep(time.Second)
	}
	return lastErr
}
