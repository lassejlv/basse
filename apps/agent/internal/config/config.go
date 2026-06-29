// Package config loads the agent's runtime configuration from the environment.
package config

import (
	"fmt"
	"os"
)

// Config holds the agent's runtime settings, all sourced from the environment.
type Config struct {
	// Port the HTTP server listens on. The control plane reaches this over an
	// SSH local-port-forward, so the container publishes it to host loopback only.
	Port string
	// Token is the bearer credential the control plane must present on /v1/* routes.
	// The agent fails closed (refuses to start) if it is empty.
	Token string
	// DockerHost is the Docker Engine API socket path.
	DockerHost string
}

// Load reads configuration from the environment and validates it.
func Load() (Config, error) {
	cfg := Config{
		Port:       envOr("BASSE_AGENT_PORT", "8888"),
		Token:      os.Getenv("BASSE_AGENT_TOKEN"),
		DockerHost: envOr("BASSE_DOCKER_SOCKET", "/var/run/docker.sock"),
	}

	if cfg.Token == "" {
		return Config{}, fmt.Errorf("BASSE_AGENT_TOKEN is required")
	}

	return cfg, nil
}

func envOr(key, fallback string) string {
	if value := os.Getenv(key); value != "" {
		return value
	}
	return fallback
}
