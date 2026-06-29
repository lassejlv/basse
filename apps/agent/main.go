// Command agent is the Basse on-server agent. It runs as a container on each
// user server, mounts the Docker socket, and exposes a small bearer-authenticated
// HTTP API the control plane reaches over an SSH tunnel.
package main

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"os"
	"time"

	"github.com/lassejlv/basse/apps/agent/internal/config"
	"github.com/lassejlv/basse/apps/agent/internal/server"
)

// version is overridden at build time via -ldflags "-X main.version=<sha>".
var version = "dev"

func main() {
	cmd := "serve"
	if len(os.Args) > 1 {
		cmd = os.Args[1]
	}

	switch cmd {
	case "serve":
		runServe()
	case "healthcheck":
		runHealthcheck()
	case "version":
		fmt.Println(version)
	default:
		slog.Error("unknown command", "command", cmd)
		os.Exit(2)
	}
}

func runServe() {
	cfg, err := config.Load()
	if err != nil {
		slog.Error("invalid configuration", "error", err)
		os.Exit(1)
	}

	if err := server.Run(cfg, version); err != nil {
		slog.Error("server error", "error", err)
		os.Exit(1)
	}
}

// runHealthcheck self-probes /healthz. It is the container HEALTHCHECK, so the
// image needs no curl/wget.
func runHealthcheck() {
	port := os.Getenv("BASSE_AGENT_PORT")
	if port == "" {
		port = "8888"
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://127.0.0.1:"+port+"/healthz", nil)
	if err != nil {
		os.Exit(1)
	}

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		os.Exit(1)
	}
}
