package handlers

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/lassejlv/basse/apps/agent/internal/config"
	"github.com/lassejlv/basse/apps/agent/internal/dockerx"
	"github.com/lassejlv/basse/apps/agent/internal/httpx"
)

// Apps deploys and manages user application containers.
type Apps struct {
	Docker *dockerx.Client
	Cfg    config.Config
}

func containerName(appID string) string {
	return "basse-app-" + appID
}

type deployRequest struct {
	AppID     string            `json:"appId"`
	Image     string            `json:"image"`
	Port      int               `json:"port"`
	Env       map[string]string `json:"env"`
	PullImage bool              `json:"pullImage"`
	Volumes   []struct {
		HostPath      string `json:"hostPath"`
		ContainerPath string `json:"containerPath"`
		ReadOnly      bool   `json:"readOnly"`
	} `json:"volumes"`
	Registry struct {
		Host  string `json:"host"`
		User  string `json:"user"`
		Token string `json:"token"`
	} `json:"registry,omitempty"`
}

type deployResponse struct {
	ContainerID string `json:"containerId"`
	Name        string `json:"name"`
	Upstream    string `json:"upstream"`
	Running     bool   `json:"running"`
}

// Deploy pulls the app image (private Depot registry) and (re)runs the container
// on the shared 'basse' network so Caddy can reverse-proxy to it. Bearer-guarded.
func (a Apps) Deploy(w http.ResponseWriter, r *http.Request) {
	var req deployRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		httpx.Error(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.AppID == "" || req.Image == "" || req.Port < 1 || req.Port > 65535 {
		httpx.Error(w, http.StatusBadRequest, "appId, image and a valid port are required")
		return
	}

	ctx := r.Context()
	name := containerName(req.AppID)

	if err := a.Docker.EnsureNetwork(ctx, a.Cfg.ProxyNetwork); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "ensure network: "+err.Error())
		return
	}

	if req.Registry.Host != "" {
		if err := a.Docker.PullImageAuth(ctx, req.Image, dockerx.RegistryAuth{
			Username:      req.Registry.User,
			Password:      req.Registry.Token,
			ServerAddress: req.Registry.Host,
		}); err != nil {
			httpx.Error(w, http.StatusBadGateway, "pull image: "+err.Error())
			return
		}
	} else if req.PullImage {
		if err := a.Docker.PullImage(ctx, req.Image); err != nil {
			httpx.Error(w, http.StatusBadGateway, "pull image: "+err.Error())
			return
		}
	}

	if err := a.Docker.RemoveContainer(ctx, name); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "remove old container: "+err.Error())
		return
	}

	// Deterministic env ordering keeps container specs stable across deploys.
	env := make([]string, 0, len(req.Env))
	for k, v := range req.Env {
		env = append(env, k+"="+v)
	}
	sort.Strings(env)

	port := fmt.Sprintf("%d/tcp", req.Port)
	binds := make([]string, 0, len(req.Volumes))
	for _, volume := range req.Volumes {
		if volume.HostPath == "" || volume.ContainerPath == "" {
			httpx.Error(w, http.StatusBadRequest, "volume hostPath and containerPath are required")
			return
		}
		bind := volume.HostPath + ":" + volume.ContainerPath
		if volume.ReadOnly {
			bind += ":ro"
		}
		binds = append(binds, bind)
	}
	spec := dockerx.ContainerSpec{
		Image:  req.Image,
		Env:    env,
		Labels: map[string]string{"basse.managed": "true", "basse.app": req.AppID},
		ExposedPorts: map[string]struct{}{
			port: {},
		},
		HostConfig: dockerx.HostConfig{
			// No PortBindings — traffic arrives via Caddy on the basse network.
			NetworkMode:   a.Cfg.ProxyNetwork,
			RestartPolicy: dockerx.RestartPolicy{Name: "unless-stopped"},
			Binds:         binds,
		},
	}

	id, err := a.Docker.CreateContainer(ctx, name, spec)
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, "create container: "+err.Error())
		return
	}
	if err := a.Docker.StartContainer(ctx, id); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "start container: "+err.Error())
		return
	}

	state, _ := a.Docker.InspectContainer(ctx, name)
	httpx.JSON(w, http.StatusOK, deployResponse{
		ContainerID: id,
		Name:        name,
		Upstream:    fmt.Sprintf("%s:%d", name, req.Port),
		Running:     state.Running,
	})
}

// Status reports whether the app container exists and is running. Bearer-guarded.
func (a Apps) Status(w http.ResponseWriter, r *http.Request) {
	state, err := a.Docker.InspectContainer(r.Context(), containerName(r.PathValue("appId")))
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]bool{"exists": state.Exists, "running": state.Running})
}

func (a Apps) Metrics(w http.ResponseWriter, r *http.Request) {
	metrics, err := a.Docker.ContainerStats(r.Context(), containerName(r.PathValue("appId")))
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	httpx.JSON(w, http.StatusOK, metrics)
}

func (a Apps) Logs(w http.ResponseWriter, r *http.Request) {
	tail := 200
	if raw := r.URL.Query().Get("tail"); raw != "" {
		if parsed, err := strconv.Atoi(raw); err == nil {
			tail = parsed
		}
	}
	logs, err := a.Docker.ContainerLogs(r.Context(), containerName(r.PathValue("appId")), tail)
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]string{"logs": logs})
}

func (a Apps) Exec(w http.ResponseWriter, r *http.Request) {
	var req struct {
		Command string `json:"command"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		httpx.Error(w, http.StatusBadRequest, "invalid request body")
		return
	}
	command := strings.TrimSpace(req.Command)
	if command == "" {
		httpx.Error(w, http.StatusBadRequest, "command is required")
		return
	}
	if len(command) > 500 {
		httpx.Error(w, http.StatusBadRequest, "command is too long")
		return
	}

	ctx, cancel := context.WithTimeout(r.Context(), 20*time.Second)
	defer cancel()
	result, err := a.Docker.ExecContainer(ctx, containerName(r.PathValue("appId")), command)
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	httpx.JSON(w, http.StatusOK, result)
}

// Remove tears down the app container. Bearer-guarded.
func (a Apps) Remove(w http.ResponseWriter, r *http.Request) {
	if err := a.Docker.RemoveContainer(r.Context(), containerName(r.PathValue("appId"))); err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]bool{"ok": true})
}
