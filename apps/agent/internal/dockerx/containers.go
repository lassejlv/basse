package dockerx

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
)

type ContainerPort struct {
	IP          string `json:"ip,omitempty"`
	PrivatePort int    `json:"privatePort"`
	PublicPort  int    `json:"publicPort,omitempty"`
	Type        string `json:"type"`
}

type ContainerMount struct {
	Type        string `json:"type"`
	Name        string `json:"name,omitempty"`
	Source      string `json:"source"`
	Destination string `json:"destination"`
	ReadOnly    bool   `json:"readOnly"`
}

type ContainerSummary struct {
	ID      string            `json:"id"`
	Name    string            `json:"name"`
	Image   string            `json:"image"`
	ImageID string            `json:"imageId"`
	State   string            `json:"state"`
	Status  string            `json:"status"`
	Running bool              `json:"running"`
	Ports   []ContainerPort   `json:"ports"`
	Labels  map[string]string `json:"labels,omitempty"`
}

type ContainerDetails struct {
	ContainerSummary
	Env    []string         `json:"env"`
	Mounts []ContainerMount `json:"mounts"`
}

func normalizeContainerName(names []string) string {
	if len(names) == 0 {
		return ""
	}
	return strings.TrimPrefix(names[0], "/")
}

func inspectName(name string) string {
	return strings.TrimPrefix(name, "/")
}

func (c *Client) ListContainers(ctx context.Context) ([]ContainerSummary, error) {
	var out []struct {
		ID      string   `json:"Id"`
		Names   []string `json:"Names"`
		Image   string   `json:"Image"`
		ImageID string   `json:"ImageID"`
		State   string   `json:"State"`
		Status  string   `json:"Status"`
		Ports   []struct {
			IP          string `json:"IP"`
			PrivatePort int    `json:"PrivatePort"`
			PublicPort  int    `json:"PublicPort"`
			Type        string `json:"Type"`
		} `json:"Ports"`
		Labels map[string]string `json:"Labels"`
	}
	if err := c.get(ctx, "/containers/json?all=true", &out); err != nil {
		return nil, err
	}

	containers := make([]ContainerSummary, 0, len(out))
	for _, row := range out {
		ports := make([]ContainerPort, 0, len(row.Ports))
		for _, port := range row.Ports {
			ports = append(ports, ContainerPort{
				IP:          port.IP,
				PrivatePort: port.PrivatePort,
				PublicPort:  port.PublicPort,
				Type:        port.Type,
			})
		}
		containers = append(containers, ContainerSummary{
			ID:      row.ID,
			Name:    normalizeContainerName(row.Names),
			Image:   row.Image,
			ImageID: row.ImageID,
			State:   row.State,
			Status:  row.Status,
			Running: row.State == "running",
			Ports:   ports,
			Labels:  row.Labels,
		})
	}
	return containers, nil
}

func (c *Client) InspectContainerDetails(ctx context.Context, ref string) (ContainerDetails, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://docker/containers/"+url.PathEscape(ref)+"/json", nil)
	if err != nil {
		return ContainerDetails{}, err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return ContainerDetails{}, err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return ContainerDetails{}, fmt.Errorf("inspect %s: status %d: %s", ref, resp.StatusCode, b)
	}

	var out struct {
		ID     string `json:"Id"`
		Name   string `json:"Name"`
		Image  string `json:"Image"`
		Config struct {
			Image  string            `json:"Image"`
			Env    []string          `json:"Env"`
			Labels map[string]string `json:"Labels"`
		} `json:"Config"`
		State struct {
			Running bool   `json:"Running"`
			Status  string `json:"Status"`
		} `json:"State"`
		NetworkSettings struct {
			Ports map[string][]struct {
				HostIP   string `json:"HostIp"`
				HostPort string `json:"HostPort"`
			} `json:"Ports"`
		} `json:"NetworkSettings"`
		Mounts []struct {
			Type        string `json:"Type"`
			Name        string `json:"Name"`
			Source      string `json:"Source"`
			Destination string `json:"Destination"`
			RW          bool   `json:"RW"`
		} `json:"Mounts"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return ContainerDetails{}, err
	}

	ports := make([]ContainerPort, 0, len(out.NetworkSettings.Ports))
	for key, bindings := range out.NetworkSettings.Ports {
		privatePort, portType := splitPortKey(key)
		if len(bindings) == 0 {
			ports = append(ports, ContainerPort{PrivatePort: privatePort, Type: portType})
			continue
		}
		for _, binding := range bindings {
			ports = append(ports, ContainerPort{
				IP:          binding.HostIP,
				PrivatePort: privatePort,
				PublicPort:  atoi(binding.HostPort),
				Type:        portType,
			})
		}
	}

	mounts := make([]ContainerMount, 0, len(out.Mounts))
	for _, mount := range out.Mounts {
		if mount.Source == "" || mount.Destination == "" {
			continue
		}
		mounts = append(mounts, ContainerMount{
			Type:        mount.Type,
			Name:        mount.Name,
			Source:      mount.Source,
			Destination: mount.Destination,
			ReadOnly:    !mount.RW,
		})
	}

	image := out.Config.Image
	if image == "" {
		image = out.Image
	}

	return ContainerDetails{
		ContainerSummary: ContainerSummary{
			ID:      out.ID,
			Name:    inspectName(out.Name),
			Image:   image,
			ImageID: out.Image,
			State:   out.State.Status,
			Status:  out.State.Status,
			Running: out.State.Running,
			Ports:   ports,
			Labels:  out.Config.Labels,
		},
		Env:    out.Config.Env,
		Mounts: mounts,
	}, nil
}

func (c *Client) RenameContainer(ctx context.Context, ref string, name string) error {
	q := url.Values{}
	q.Set("name", name)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/containers/"+url.PathEscape(ref)+"/rename?"+q.Encode(), nil)
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
	return fmt.Errorf("rename %s: status %d: %s", ref, resp.StatusCode, b)
}

func (c *Client) ConnectNetwork(ctx context.Context, network string, container string, aliases []string) error {
	body, _ := json.Marshal(map[string]any{
		"Container": container,
		"EndpointConfig": map[string]any{
			"Aliases": aliases,
		},
	})
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://docker/networks/"+url.PathEscape(network)+"/connect", bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")

	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusOK || resp.StatusCode == http.StatusNoContent {
		return nil
	}
	b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
	if resp.StatusCode == http.StatusForbidden && strings.Contains(string(b), "already exists") {
		return nil
	}
	return fmt.Errorf("connect %s to %s: status %d: %s", container, network, resp.StatusCode, b)
}

func splitPortKey(key string) (int, string) {
	parts := strings.SplitN(key, "/", 2)
	if len(parts) != 2 {
		return atoi(key), "tcp"
	}
	return atoi(parts[0]), parts[1]
}

func atoi(value string) int {
	n := 0
	for _, char := range value {
		if char < '0' || char > '9' {
			return 0
		}
		n = n*10 + int(char-'0')
	}
	return n
}
