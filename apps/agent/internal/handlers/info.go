package handlers

import (
	"net/http"

	"github.com/lassejlv/basse/apps/agent/internal/dockerx"
	"github.com/lassejlv/basse/apps/agent/internal/httpx"
)

// System holds dependencies for the authenticated info/version endpoints.
type System struct {
	Docker  *dockerx.Client
	Version string
}

// infoResponse is the host + Docker fact sheet returned by /v1/info.
type infoResponse struct {
	Agent  agentInfo       `json:"agent"`
	Docker dockerx.Info    `json:"docker"`
	Engine dockerx.Version `json:"engine"`
}

type agentInfo struct {
	Version string `json:"version"`
}

// Info returns host and Docker facts. 503 if the daemon is unreachable.
func (s System) Info(w http.ResponseWriter, r *http.Request) {
	ctx := r.Context()

	info, err := s.Docker.Info(ctx)
	if err != nil {
		httpx.Error(w, http.StatusServiceUnavailable, "docker daemon unreachable")
		return
	}

	version, err := s.Docker.Version(ctx)
	if err != nil {
		httpx.Error(w, http.StatusServiceUnavailable, "docker daemon unreachable")
		return
	}

	httpx.JSON(w, http.StatusOK, infoResponse{
		Agent:  agentInfo{Version: s.Version},
		Docker: info,
		Engine: version,
	})
}

// VersionInfo returns the agent build version, so the control plane can confirm
// an upgrade took effect after re-running the container.
func (s System) VersionInfo(w http.ResponseWriter, _ *http.Request) {
	httpx.JSON(w, http.StatusOK, agentInfo{Version: s.Version})
}
