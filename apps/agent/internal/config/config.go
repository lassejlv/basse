// Package config loads the agent's runtime configuration from the environment.
package config

import (
	"fmt"
	"os"
)

// Config holds the agent's runtime settings, all sourced from the environment.
type Config struct {
	// Mode controls how the control plane reaches this agent. "serve" is the
	// default SSH-tunnel mode; "outbound" additionally polls the control plane.
	Mode string
	// Port the HTTP server listens on. The control plane reaches this over an
	// SSH local-port-forward, so the container publishes it to host loopback only.
	Port string
	// Token is the bearer credential the control plane must present on /v1/* routes.
	// The agent fails closed (refuses to start) if it is empty.
	Token string
	// DockerHost is the Docker Engine API socket path.
	DockerHost string
	// ControlPlaneURL is required when Mode is "outbound".
	ControlPlaneURL string

	// Proxy (Caddy) settings.
	CaddyImage     string // image the agent runs as the proxy
	CaddyContainer string // container name
	ProxyNetwork   string // shared Docker network Caddy joins to reach upstreams
	ProxyHTTPPort  string // host HTTP port for Caddy, or "none" to avoid binding
	ProxyHTTPSPort string // host HTTPS port for Caddy, or "none" to avoid binding
	DataVolume     string // named volume for ACME certs/state (NEVER removed)
	AdminVolume    string // named volume holding the admin unix socket + init.json
	// AdminDir is where AdminVolume is mounted in BOTH the agent and Caddy. The
	// admin socket lives at AdminDir/admin.sock; the boot config at AdminDir/init.json.
	AdminDir string
}

// AdminSocketPath is the unix socket the agent dials to drive Caddy's admin API.
func (c Config) AdminSocketPath() string {
	return c.AdminDir + "/admin.sock"
}

// InitConfigPath is the boot config Caddy starts from (admin socket + empty http).
func (c Config) InitConfigPath() string {
	return c.AdminDir + "/init.json"
}

// Load reads configuration from the environment and validates it.
func Load() (Config, error) {
	cfg := Config{
		Mode:            envOr("BASSE_AGENT_MODE", "serve"),
		Port:            envOr("BASSE_AGENT_PORT", "8888"),
		Token:           os.Getenv("BASSE_AGENT_TOKEN"),
		DockerHost:      envOr("BASSE_DOCKER_SOCKET", "/var/run/docker.sock"),
		ControlPlaneURL: os.Getenv("BASSE_CONTROL_PLANE_URL"),
		CaddyImage:      envOr("BASSE_CADDY_IMAGE", "caddy:2"),
		CaddyContainer:  envOr("BASSE_CADDY_CONTAINER", "basse-caddy"),
		ProxyNetwork:    envOr("BASSE_PROXY_NETWORK", "basse"),
		ProxyHTTPPort:   envOr("BASSE_PROXY_HTTP_PORT", "80"),
		ProxyHTTPSPort:  envOr("BASSE_PROXY_HTTPS_PORT", "443"),
		DataVolume:      envOr("BASSE_CADDY_DATA_VOLUME", "basse_caddy_data"),
		AdminVolume:     envOr("BASSE_CADDY_ADMIN_VOLUME", "basse_caddy_admin"),
		AdminDir:        envOr("BASSE_CADDY_ADMIN_DIR", "/run/caddy-admin"),
	}

	if cfg.Token == "" {
		return Config{}, fmt.Errorf("BASSE_AGENT_TOKEN is required")
	}
	if cfg.Mode != "serve" && cfg.Mode != "outbound" {
		return Config{}, fmt.Errorf("BASSE_AGENT_MODE must be serve or outbound")
	}
	if cfg.Mode == "outbound" && cfg.ControlPlaneURL == "" {
		return Config{}, fmt.Errorf("BASSE_CONTROL_PLANE_URL is required in outbound mode")
	}

	return cfg, nil
}

func envOr(key, fallback string) string {
	if value := os.Getenv(key); value != "" {
		return value
	}
	return fallback
}
