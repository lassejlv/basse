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

// ContainerState is the subset of GET /containers/{id}/json we read.
type ContainerState struct {
	Exists  bool
	Running bool
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
		State struct {
			Running bool `json:"Running"`
		} `json:"State"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return ContainerState{}, err
	}

	return ContainerState{Exists: true, Running: out.State.Running}, nil
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
