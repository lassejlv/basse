package dockerx

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// Engine API write operations used to manage the Caddy proxy container. Still
// pure stdlib HTTP over the docker socket (no docker CLI in the agent image).

// PortBinding maps a container port to a host binding.
type PortBinding struct {
	HostIP   string `json:"HostIp,omitempty"`
	HostPort string `json:"HostPort"`
}

// RestartPolicy mirrors the Engine API restart policy.
type RestartPolicy struct {
	Name string `json:"Name"`
}

// HostConfig is the subset of the Engine API HostConfig we set.
type HostConfig struct {
	Binds         []string                 `json:"Binds,omitempty"`
	Memory        int64                    `json:"Memory,omitempty"`
	NanoCPUs      int64                    `json:"NanoCpus,omitempty"`
	PortBindings  map[string][]PortBinding `json:"PortBindings,omitempty"`
	NetworkMode   string                   `json:"NetworkMode,omitempty"`
	RestartPolicy RestartPolicy            `json:"RestartPolicy"`
}

// ContainerSpec is the create payload (subset).
type ContainerSpec struct {
	Image        string              `json:"Image"`
	Cmd          []string            `json:"Cmd,omitempty"`
	Env          []string            `json:"Env,omitempty"`
	Labels       map[string]string   `json:"Labels,omitempty"`
	ExposedPorts map[string]struct{} `json:"ExposedPorts,omitempty"`
	HostConfig   HostConfig          `json:"HostConfig"`
}

// RegistryAuth is the credential for pulling a private image.
type RegistryAuth struct {
	Username      string `json:"username"`
	Password      string `json:"password"`
	ServerAddress string `json:"serveraddress"`
}

// ContainerState is the subset of GET /containers/{id}/json we read. Running
// alone hides crash loops: the restart policy brings a crashing container back
// within seconds, so Restarting and RestartCount are what expose the crashes.
type ContainerState struct {
	Exists       bool
	Running      bool
	Restarting   bool
	RestartCount int
	ExitCode     int
	StartedAt    string
}

type ContainerMetrics struct {
	CPUPercent       float64 `json:"cpuPercent"`
	MemoryBytes      uint64  `json:"memoryBytes"`
	MemoryLimitBytes uint64  `json:"memoryLimitBytes"`
	MemoryPercent    float64 `json:"memoryPercent"`
}

type ExecResult struct {
	ExitCode int    `json:"exitCode"`
	Output   string `json:"output"`
}

// imagePullTimeout bounds a (possibly large) image pull.
const imagePullTimeout = 5 * time.Minute

// PullImage pulls a public image.
func (c *Client) PullImage(ctx context.Context, image string) error {
	return c.pull(ctx, image, nil)
}

// PullImageAuth pulls a private image, authenticating with the given registry
// credential (sent in the X-Registry-Auth header, base64url-encoded JSON — the
// encoding Docker's own client uses).
func (c *Client) PullImageAuth(ctx context.Context, image string, auth RegistryAuth) error {
	return c.pull(ctx, image, &auth)
}

// pull pulls an image and DRAINS the progress stream to completion — the image
// is not guaranteed present until the stream ends. Returns an error if the
// stream reports one.
func (c *Client) pull(ctx context.Context, image string, auth *RegistryAuth) error {
	ref, tag := splitImageRef(image)
	q := url.Values{}
	q.Set("fromImage", ref)
	if tag != "" {
		q.Set("tag", tag)
	}

	ctx, cancel := context.WithTimeout(ctx, imagePullTimeout)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/images/create?"+q.Encode(), nil)
	if err != nil {
		return err
	}

	if auth != nil {
		encoded, err := json.Marshal(auth)
		if err != nil {
			return err
		}
		req.Header.Set("X-Registry-Auth", base64.URLEncoding.EncodeToString(encoded))
	}

	resp, err := c.stream.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return fmt.Errorf("pull %s: status %d: %s", image, resp.StatusCode, body)
	}

	// The body is a stream of JSON progress objects; an error appears as
	// {"error":"...","errorDetail":{...}}. Drain to EOF and surface any error.
	decoder := json.NewDecoder(resp.Body)
	for {
		var msg struct {
			Error string `json:"error"`
		}
		if err := decoder.Decode(&msg); err != nil {
			if err == io.EOF {
				break
			}
			return fmt.Errorf("pull %s: stream decode: %w", image, err)
		}
		if msg.Error != "" {
			return fmt.Errorf("pull %s: %s", image, msg.Error)
		}
	}

	return nil
}

// EnsureNetwork creates a user-defined bridge network, treating "already exists"
// (409) as success.
func (c *Client) EnsureNetwork(ctx context.Context, name string) error {
	body, _ := json.Marshal(map[string]any{"Name": name, "Driver": "bridge"})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/networks/create", bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusConflict {
		return nil // already exists
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return fmt.Errorf("create network %s: status %d: %s", name, resp.StatusCode, b)
	}
	return nil
}

// InspectContainer reports whether a container exists and is running.
func (c *Client) InspectContainer(ctx context.Context, name string) (ContainerState, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://docker/containers/"+name+"/json", nil)
	if err != nil {
		return ContainerState{}, err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return ContainerState{}, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return ContainerState{Exists: false}, nil
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return ContainerState{}, fmt.Errorf("inspect %s: status %d: %s", name, resp.StatusCode, b)
	}

	var out struct {
		RestartCount int `json:"RestartCount"`
		State        struct {
			Running    bool   `json:"Running"`
			Restarting bool   `json:"Restarting"`
			ExitCode   int    `json:"ExitCode"`
			StartedAt  string `json:"StartedAt"`
		} `json:"State"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return ContainerState{}, err
	}

	return ContainerState{
		Exists:       true,
		Running:      out.State.Running,
		Restarting:   out.State.Restarting,
		RestartCount: out.RestartCount,
		ExitCode:     out.State.ExitCode,
		StartedAt:    out.State.StartedAt,
	}, nil
}

// RemoveContainer force-removes a container, treating "not found" (404) as success.
func (c *Client) RemoveContainer(ctx context.Context, name string) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodDelete, "http://docker/containers/"+name+"?force=true", nil)
	if err != nil {
		return err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return fmt.Errorf("remove %s: status %d: %s", name, resp.StatusCode, b)
	}
	return nil
}

func (c *Client) ContainerStats(ctx context.Context, name string) (ContainerMetrics, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://docker/containers/"+name+"/stats?stream=false", nil)
	if err != nil {
		return ContainerMetrics{}, err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return ContainerMetrics{}, err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return ContainerMetrics{}, fmt.Errorf("stats %s: status %d: %s", name, resp.StatusCode, b)
	}

	var out struct {
		CPUStats struct {
			CPUUsage struct {
				TotalUsage  uint64   `json:"total_usage"`
				PercpuUsage []uint64 `json:"percpu_usage"`
			} `json:"cpu_usage"`
			SystemCPUUsage uint64 `json:"system_cpu_usage"`
			OnlineCPUs     uint32 `json:"online_cpus"`
		} `json:"cpu_stats"`
		PreCPUStats struct {
			CPUUsage struct {
				TotalUsage uint64 `json:"total_usage"`
			} `json:"cpu_usage"`
			SystemCPUUsage uint64 `json:"system_cpu_usage"`
		} `json:"precpu_stats"`
		MemoryStats struct {
			Usage uint64 `json:"usage"`
			Limit uint64 `json:"limit"`
		} `json:"memory_stats"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return ContainerMetrics{}, err
	}

	cpuDelta := uintDelta(out.CPUStats.CPUUsage.TotalUsage, out.PreCPUStats.CPUUsage.TotalUsage)
	systemDelta := uintDelta(out.CPUStats.SystemCPUUsage, out.PreCPUStats.SystemCPUUsage)
	onlineCPUs := float64(out.CPUStats.OnlineCPUs)
	if onlineCPUs == 0 {
		onlineCPUs = float64(len(out.CPUStats.CPUUsage.PercpuUsage))
	}
	cpuPercent := 0.0
	if systemDelta > 0 && cpuDelta > 0 && onlineCPUs > 0 {
		cpuPercent = (cpuDelta / systemDelta) * onlineCPUs * 100
	}

	memoryPercent := 0.0
	if out.MemoryStats.Limit > 0 {
		memoryPercent = (float64(out.MemoryStats.Usage) / float64(out.MemoryStats.Limit)) * 100
	}

	return ContainerMetrics{
		CPUPercent:       cpuPercent,
		MemoryBytes:      out.MemoryStats.Usage,
		MemoryLimitBytes: out.MemoryStats.Limit,
		MemoryPercent:    memoryPercent,
	}, nil
}

func (c *Client) ContainerLogs(ctx context.Context, name string, tail int) (string, error) {
	if tail < 1 {
		tail = 200
	}
	if tail > 1000 {
		tail = 1000
	}

	q := url.Values{}
	q.Set("stdout", "1")
	q.Set("stderr", "1")
	q.Set("timestamps", "1")
	q.Set("tail", fmt.Sprintf("%d", tail))
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://docker/containers/"+url.PathEscape(name)+"/logs?"+q.Encode(), nil)
	if err != nil {
		return "", err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return "", nil
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return "", fmt.Errorf("logs %s: status %d: %s", name, resp.StatusCode, b)
	}

	raw, err := io.ReadAll(io.LimitReader(resp.Body, 512*1024))
	if err != nil {
		return "", err
	}
	return demuxDockerStream(raw), nil
}

func uintDelta(current uint64, previous uint64) float64 {
	if current <= previous {
		return 0
	}
	return float64(current - previous)
}

func (c *Client) ExecContainer(ctx context.Context, name string, command string) (ExecResult, error) {
	createBody, _ := json.Marshal(map[string]any{
		"AttachStdout": true,
		"AttachStderr": true,
		"Cmd":          []string{"sh", "-lc", command},
	})
	createReq, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/containers/"+name+"/exec", bytes.NewReader(createBody))
	if err != nil {
		return ExecResult{}, err
	}
	createReq.Header.Set("Content-Type", "application/json")

	createResp, err := c.http.Do(createReq)
	if err != nil {
		return ExecResult{}, err
	}
	defer createResp.Body.Close()
	if createResp.StatusCode < 200 || createResp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(createResp.Body, 2048))
		return ExecResult{}, fmt.Errorf("create exec %s: status %d: %s", name, createResp.StatusCode, b)
	}

	var created struct {
		ID string `json:"Id"`
	}
	if err := json.NewDecoder(createResp.Body).Decode(&created); err != nil {
		return ExecResult{}, err
	}
	if created.ID == "" {
		return ExecResult{}, fmt.Errorf("create exec %s: missing id", name)
	}

	startBody, _ := json.Marshal(map[string]any{"Detach": false, "Tty": false})
	startReq, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/exec/"+created.ID+"/start", bytes.NewReader(startBody))
	if err != nil {
		return ExecResult{}, err
	}
	startReq.Header.Set("Content-Type", "application/json")
	startResp, err := c.stream.Do(startReq)
	if err != nil {
		return ExecResult{}, err
	}
	defer startResp.Body.Close()
	if startResp.StatusCode < 200 || startResp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(startResp.Body, 2048))
		return ExecResult{}, fmt.Errorf("start exec %s: status %d: %s", name, startResp.StatusCode, b)
	}

	raw, err := io.ReadAll(io.LimitReader(startResp.Body, 64*1024))
	if err != nil {
		return ExecResult{}, err
	}

	inspectReq, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://docker/exec/"+created.ID+"/json", nil)
	if err != nil {
		return ExecResult{}, err
	}
	inspectResp, err := c.http.Do(inspectReq)
	if err != nil {
		return ExecResult{}, err
	}
	defer inspectResp.Body.Close()
	if inspectResp.StatusCode < 200 || inspectResp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(inspectResp.Body, 2048))
		return ExecResult{}, fmt.Errorf("inspect exec %s: status %d: %s", name, inspectResp.StatusCode, b)
	}
	var inspected struct {
		ExitCode int `json:"ExitCode"`
	}
	if err := json.NewDecoder(inspectResp.Body).Decode(&inspected); err != nil {
		return ExecResult{}, err
	}

	return ExecResult{ExitCode: inspected.ExitCode, Output: demuxDockerStream(raw)}, nil
}

// ExecContainerStdout runs a command in a container and streams ONLY its
// stdout frames to w (stderr is captured separately for error reporting).
// Unlike ExecContainer it does not buffer output, so it is safe for large
// payloads (e.g. streaming a database dump). Returns the exec's exit code.
func (c *Client) ExecContainerStdout(ctx context.Context, name string, command string, w io.Writer) (int, string, error) {
	createBody, _ := json.Marshal(map[string]any{
		"AttachStdout": true,
		"AttachStderr": true,
		"Cmd":          []string{"sh", "-lc", command},
	})
	createReq, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/containers/"+name+"/exec", bytes.NewReader(createBody))
	if err != nil {
		return 0, "", err
	}
	createReq.Header.Set("Content-Type", "application/json")

	createResp, err := c.http.Do(createReq)
	if err != nil {
		return 0, "", err
	}
	defer createResp.Body.Close()
	if createResp.StatusCode < 200 || createResp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(createResp.Body, 2048))
		return 0, "", fmt.Errorf("create exec %s: status %d: %s", name, createResp.StatusCode, b)
	}

	var created struct {
		ID string `json:"Id"`
	}
	if err := json.NewDecoder(createResp.Body).Decode(&created); err != nil {
		return 0, "", err
	}
	if created.ID == "" {
		return 0, "", fmt.Errorf("create exec %s: missing id", name)
	}

	startBody, _ := json.Marshal(map[string]any{"Detach": false, "Tty": false})
	startReq, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/exec/"+created.ID+"/start", bytes.NewReader(startBody))
	if err != nil {
		return 0, "", err
	}
	startReq.Header.Set("Content-Type", "application/json")
	startResp, err := c.stream.Do(startReq)
	if err != nil {
		return 0, "", err
	}
	defer startResp.Body.Close()
	if startResp.StatusCode < 200 || startResp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(startResp.Body, 2048))
		return 0, "", fmt.Errorf("start exec %s: status %d: %s", name, startResp.StatusCode, b)
	}

	var stderr strings.Builder
	header := make([]byte, 8)
	for {
		if _, err := io.ReadFull(startResp.Body, header); err != nil {
			if err == io.EOF {
				break
			}
			return 0, stderr.String(), fmt.Errorf("exec stream %s: %w", name, err)
		}
		size := int(header[4])<<24 | int(header[5])<<16 | int(header[6])<<8 | int(header[7])
		if size <= 0 {
			continue
		}
		frame := io.LimitReader(startResp.Body, int64(size))
		if header[0] == 1 {
			if _, err := io.Copy(w, frame); err != nil {
				return 0, stderr.String(), fmt.Errorf("exec stream %s: %w", name, err)
			}
		} else {
			// Keep a bounded stderr tail for error messages.
			if _, err := io.Copy(limitedBuilder{&stderr, 8 * 1024}, frame); err != nil {
				return 0, stderr.String(), fmt.Errorf("exec stream %s: %w", name, err)
			}
		}
	}

	inspectReq, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://docker/exec/"+created.ID+"/json", nil)
	if err != nil {
		return 0, stderr.String(), err
	}
	inspectResp, err := c.http.Do(inspectReq)
	if err != nil {
		return 0, stderr.String(), err
	}
	defer inspectResp.Body.Close()
	var inspected struct {
		ExitCode int `json:"ExitCode"`
	}
	if err := json.NewDecoder(inspectResp.Body).Decode(&inspected); err != nil {
		return 0, stderr.String(), err
	}
	return inspected.ExitCode, stderr.String(), nil
}

// limitedBuilder discards writes past its cap but never errors, so io.Copy
// always drains the source frame.
type limitedBuilder struct {
	b   *strings.Builder
	cap int
}

func (l limitedBuilder) Write(p []byte) (int, error) {
	if remaining := l.cap - l.b.Len(); remaining > 0 {
		if len(p) > remaining {
			l.b.Write(p[:remaining])
		} else {
			l.b.Write(p)
		}
	}
	return len(p), nil
}

func demuxDockerStream(raw []byte) string {
	var out strings.Builder
	for len(raw) >= 8 {
		size := int(raw[4])<<24 | int(raw[5])<<16 | int(raw[6])<<8 | int(raw[7])
		raw = raw[8:]
		if size < 0 || size > len(raw) {
			break
		}
		out.Write(raw[:size])
		raw = raw[size:]
	}
	if out.Len() == 0 && len(raw) > 0 {
		return string(raw)
	}
	return out.String()
}

// CreateContainer creates a container with the given name and spec, returning its id.
func (c *Client) CreateContainer(ctx context.Context, name string, spec ContainerSpec) (string, error) {
	body, err := json.Marshal(spec)
	if err != nil {
		return "", err
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/containers/create?name="+url.QueryEscape(name), bytes.NewReader(body))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.http.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return "", fmt.Errorf("create container %s: status %d: %s", name, resp.StatusCode, b)
	}

	var out struct {
		ID string `json:"Id"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return "", err
	}
	return out.ID, nil
}

// StartContainer starts a created container. A port-bind conflict surfaces here.
func (c *Client) StartContainer(ctx context.Context, id string) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/containers/"+id+"/start", nil)
	if err != nil {
		return err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	// 204 = started, 304 = already started.
	if resp.StatusCode == http.StatusNoContent || resp.StatusCode == http.StatusNotModified {
		return nil
	}
	b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
	return fmt.Errorf("start %s: status %d: %s", id, resp.StatusCode, b)
}

// RestartContainer restarts an existing container by name or id.
func (c *Client) RestartContainer(ctx context.Context, name string) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/containers/"+name+"/restart", nil)
	if err != nil {
		return err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNoContent {
		return nil
	}
	b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
	return fmt.Errorf("restart %s: status %d: %s", name, resp.StatusCode, b)
}

// splitImageRef splits "repo:tag" into (repo, tag), defaulting tag to "latest".
// It is digest- and registry-port-aware (only splits on a tag after the last /).
func splitImageRef(image string) (string, string) {
	slash := -1
	for i := 0; i < len(image); i++ {
		if image[i] == '/' {
			slash = i
		}
	}
	lastColon := -1
	for i := slash + 1; i < len(image); i++ {
		if image[i] == ':' {
			lastColon = i
		}
	}
	if lastColon == -1 {
		return image, "latest"
	}
	return image[:lastColon], image[lastColon+1:]
}
