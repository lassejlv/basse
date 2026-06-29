// Package handlers implements the agent's HTTP endpoints.
package handlers

import (
	"net/http"

	"github.com/lassejlv/basse/apps/agent/internal/dockerx"
	"github.com/lassejlv/basse/apps/agent/internal/httpx"
)

// Health holds dependencies for the liveness/readiness probes.
type Health struct {
	Docker *dockerx.Client
}

// Live is the liveness probe: 200 as long as the process is up. Unauthenticated.
func (h Health) Live(w http.ResponseWriter, _ *http.Request) {
	httpx.JSON(w, http.StatusOK, map[string]string{"status": "ok"})
}

// Ready is the readiness probe: 200 only if the Docker daemon is reachable,
// 503 otherwise. Lets the control plane confirm the agent can control Docker
// before flipping the server to active. Unauthenticated.
func (h Health) Ready(w http.ResponseWriter, r *http.Request) {
	if err := h.Docker.Ping(r.Context()); err != nil {
		httpx.Error(w, http.StatusServiceUnavailable, "docker daemon unreachable")
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]string{"status": "ready"})
}
