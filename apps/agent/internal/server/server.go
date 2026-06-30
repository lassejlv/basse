// Package server wires the agent's routes and runs the HTTP server.
package server

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/lassejlv/basse/apps/agent/internal/caddyx"
	"github.com/lassejlv/basse/apps/agent/internal/config"
	"github.com/lassejlv/basse/apps/agent/internal/dockerx"
	"github.com/lassejlv/basse/apps/agent/internal/handlers"
	"github.com/lassejlv/basse/apps/agent/internal/middleware"
)

// Run builds the router and serves until interrupted, then shuts down gracefully.
func Run(cfg config.Config, version string) error {
	docker := dockerx.New(cfg.DockerHost)
	caddy := caddyx.New(cfg.AdminSocketPath())

	health := handlers.Health{Docker: docker}
	system := handlers.System{Docker: docker, Version: version}
	proxy := handlers.Proxy{Docker: docker, Caddy: caddy, Cfg: cfg}
	apps := handlers.Apps{Docker: docker, Cfg: cfg}

	mux := http.NewServeMux()

	// Unauthenticated probes.
	mux.HandleFunc("GET /healthz", health.Live)
	mux.HandleFunc("GET /readyz", health.Ready)

	// Authenticated API.
	mux.Handle("GET /v1/info", middleware.Bearer(cfg.Token, http.HandlerFunc(system.Info)))
	mux.Handle("GET /v1/version", middleware.Bearer(cfg.Token, http.HandlerFunc(system.VersionInfo)))
	mux.Handle("POST /v1/proxy/ensure", middleware.Bearer(cfg.Token, http.HandlerFunc(proxy.Ensure)))
	mux.Handle("GET /v1/proxy/status", middleware.Bearer(cfg.Token, http.HandlerFunc(proxy.Status)))
	mux.Handle("POST /v1/proxy/sync", middleware.Bearer(cfg.Token, http.HandlerFunc(proxy.Sync)))
	mux.Handle("POST /v1/apps/deploy", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Deploy)))
	mux.Handle("GET /v1/apps/{appId}/status", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Status)))
	mux.Handle("GET /v1/apps/{appId}/metrics", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Metrics)))
	mux.Handle("GET /v1/apps/{appId}/logs", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Logs)))
	mux.Handle("POST /v1/apps/{appId}/exec", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Exec)))
	mux.Handle("DELETE /v1/apps/{appId}", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Remove)))

	srv := &http.Server{
		Addr:              ":" + cfg.Port,
		Handler:           middleware.Logging(mux),
		ReadHeaderTimeout: 5 * time.Second,
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	errCh := make(chan error, 1)
	go func() {
		slog.Info("agent listening", "port", cfg.Port, "version", version)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			errCh <- err
		}
	}()

	select {
	case err := <-errCh:
		return err
	case <-ctx.Done():
		slog.Info("shutting down")
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		return srv.Shutdown(shutdownCtx)
	}
}
