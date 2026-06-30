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
	AppID      string            `json:"appId"`
	Image      string            `json:"image"`
	Cmd        []string          `json:"cmd"`
	Port       int               `json:"port"`
	Env        map[string]string `json:"env"`
	PullImage  bool              `json:"pullImage"`
	PublicPort int               `json:"publicPort"`
	CPU        int               `json:"cpuLimitMillicores"`
	Memory     int64             `json:"memoryLimitBytes"`
	Volumes    []struct {
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

type importContainerRequest struct {
	AppID       string `json:"appId"`
	ContainerID string `json:"containerId"`
}

func isImportableContainer(container dockerx.ContainerSummary, caddyContainer string) bool {
	if container.ID == "" || container.Name == "" {
		return false
	}
	if container.Name == caddyContainer || container.Name == "basse-agent" {
		return false
	}
	if strings.HasPrefix(container.Name, "basse-app-") {
		return false
	}
	if container.Labels["basse.managed"] == "true" || container.Labels["basse.app"] != "" {
		return false
	}
	return true
}

func (a Apps) ImportableContainers(w http.ResponseWriter, r *http.Request) {
	containers, err := a.Docker.ListContainers(r.Context())
	if err != nil {
		httpx.Error(w, http.StatusBadGateway, "list containers: "+err.Error())
		return
	}

	out := make([]dockerx.ContainerSummary, 0, len(containers))
	for _, container := range containers {
		if isImportableContainer(container, a.Cfg.CaddyContainer) {
			out = append(out, container)
		}
	}
	httpx.JSON(w, http.StatusOK, map[string][]dockerx.ContainerSummary{"containers": out})
}

func (a Apps) ImportContainer(w http.ResponseWriter, r *http.Request) {
	var req importContainerRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		httpx.Error(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.AppID == "" || req.ContainerID == "" {
		httpx.Error(w, http.StatusBadRequest, "appId and containerId are required")
		return
	}

	ctx := r.Context()
	details, err := a.Docker.InspectContainerDetails(ctx, req.ContainerID)
	if err != nil {
		httpx.Error(w, http.StatusBadGateway, "inspect container: "+err.Error())
		return
	}
	if !isImportableContainer(details.ContainerSummary, a.Cfg.CaddyContainer) {
		httpx.Error(w, http.StatusBadRequest, "container is already managed by Basse")
		return
	}

	targetName := containerName(req.AppID)
	if err := a.Docker.EnsureNetwork(ctx, a.Cfg.ProxyNetwork); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "ensure network: "+err.Error())
		return
	}
	if err := a.Docker.RenameContainer(ctx, req.ContainerID, targetName); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "rename container: "+err.Error())
		return
	}
	if err := a.Docker.ConnectNetwork(ctx, a.Cfg.ProxyNetwork, targetName, []string{targetName}); err != nil {
		httpx.Error(w, http.StatusInternalServerError, "connect network: "+err.Error())
		return
	}

	imported, err := a.Docker.InspectContainerDetails(ctx, targetName)
	if err != nil {
		httpx.Error(w, http.StatusBadGateway, "inspect imported container: "+err.Error())
		return
	}
	httpx.JSON(w, http.StatusOK, imported)
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
	if req.PublicPort < 0 || req.PublicPort > 65535 {
		httpx.Error(w, http.StatusBadRequest, "publicPort must be a valid port")
		return
	}
	if req.CPU < 0 {
		httpx.Error(w, http.StatusBadRequest, "cpuLimitMillicores must be non-negative")
		return
	}
	if req.Memory < 0 {
		httpx.Error(w, http.StatusBadRequest, "memoryLimitBytes must be non-negative")
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
	portBindings := map[string][]dockerx.PortBinding(nil)
	if req.PublicPort > 0 {
		portBindings = map[string][]dockerx.PortBinding{
			port: []dockerx.PortBinding{{HostIP: "0.0.0.0", HostPort: strconv.Itoa(req.PublicPort)}},
		}
	}
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
		Cmd:    req.Cmd,
		Env:    env,
		Labels: map[string]string{"basse.managed": "true", "basse.app": req.AppID},
		ExposedPorts: map[string]struct{}{
			port: {},
		},
		HostConfig: dockerx.HostConfig{
			// Services usually route via Caddy on the basse network; databases can
			// opt into direct TCP exposure through PortBindings.
			Memory:        req.Memory,
			NanoCPUs:      int64(req.CPU) * 1_000_000,
			NetworkMode:   a.Cfg.ProxyNetwork,
			PortBindings:  portBindings,
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
