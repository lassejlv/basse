// Package server wires the agent's routes and runs the HTTP server.
package server

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"strings"
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
	backups := handlers.Backups{Docker: docker}

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
	mux.Handle("GET /v1/apps/importable-containers", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.ImportableContainers)))
	mux.Handle("POST /v1/apps/import-container", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.ImportContainer)))
	mux.Handle("POST /v1/apps/deploy", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Deploy)))
	mux.Handle("GET /v1/apps/{appId}/status", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Status)))
	mux.Handle("GET /v1/apps/{appId}/metrics", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Metrics)))
	mux.Handle("GET /v1/apps/{appId}/logs", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Logs)))
	mux.Handle("POST /v1/apps/{appId}/exec", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Exec)))
	mux.Handle("DELETE /v1/apps/{appId}", middleware.Bearer(cfg.Token, http.HandlerFunc(apps.Remove)))
	mux.Handle("POST /v1/apps/{appId}/backups", middleware.Bearer(cfg.Token, http.HandlerFunc(backups.Create)))
	mux.Handle("POST /v1/apps/{appId}/backups/{backupId}/restore", middleware.Bearer(cfg.Token, http.HandlerFunc(backups.Restore)))
	mux.Handle("DELETE /v1/apps/{appId}/backups/{backupId}", middleware.Bearer(cfg.Token, http.HandlerFunc(backups.Delete)))
	mux.Handle("GET /v1/apps/{appId}/backups/{backupId}/download", middleware.Bearer(cfg.Token, http.HandlerFunc(backups.Download)))

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

	if cfg.Mode == "outbound" {
		go runOutboundPoller(ctx, cfg)
	}

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

type outboundCommand struct {
	ID     string          `json:"id"`
	Method string          `json:"method"`
	Path   string          `json:"path"`
	Body   json.RawMessage `json:"body"`
}

type outboundResult struct {
	Status int    `json:"status"`
	Body   string `json:"body,omitempty"`
	Error  string `json:"error,omitempty"`
}

func runOutboundPoller(ctx context.Context, cfg config.Config) {
	client := &http.Client{Timeout: 10 * time.Second}
	baseURL := strings.TrimRight(cfg.ControlPlaneURL, "/")
	localBaseURL := "http://127.0.0.1:" + cfg.Port

	slog.Info("outbound agent polling enabled", "controlPlane", baseURL)
	for {
		select {
		case <-ctx.Done():
			return
		default:
		}

		cmd, err := pollOutboundCommand(ctx, client, baseURL, cfg.Token)
		if err != nil {
			slog.Warn("outbound poll failed", "error", err)
			sleepContext(ctx, 3*time.Second)
			continue
		}
		if cmd == nil {
			sleepContext(ctx, 2*time.Second)
			continue
		}

		result := executeOutboundCommand(ctx, client, localBaseURL, cfg.Token, *cmd)
		if err := postOutboundResult(ctx, client, baseURL, cfg.Token, cmd.ID, result); err != nil {
			slog.Warn("outbound result post failed", "command", cmd.ID, "error", err)
			sleepContext(ctx, 2*time.Second)
		}
	}
}

func pollOutboundCommand(ctx context.Context, client *http.Client, baseURL string, token string) (*outboundCommand, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, baseURL+"/api/agent/outbound/poll", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)

	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNoContent {
		return nil, nil
	}
	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		return nil, fmt.Errorf("poll returned %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}

	var cmd outboundCommand
	if err := json.NewDecoder(resp.Body).Decode(&cmd); err != nil {
		return nil, err
	}
	if cmd.ID == "" || cmd.Method == "" || cmd.Path == "" || !strings.HasPrefix(cmd.Path, "/") {
		return nil, fmt.Errorf("invalid outbound command")
	}
	return &cmd, nil
}

func executeOutboundCommand(ctx context.Context, client *http.Client, localBaseURL string, token string, cmd outboundCommand) outboundResult {
	method := strings.ToUpper(cmd.Method)
	if method == "" {
		method = http.MethodGet
	}

	var body io.Reader
	if len(cmd.Body) > 0 && string(cmd.Body) != "null" {
		body = bytes.NewReader(cmd.Body)
	}

	req, err := http.NewRequestWithContext(ctx, method, localBaseURL+cmd.Path, body)
	if err != nil {
		return outboundResult{Status: http.StatusBadGateway, Error: err.Error()}
	}
	if strings.HasPrefix(cmd.Path, "/v1/") {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}

	resp, err := client.Do(req)
	if err != nil {
		return outboundResult{Status: http.StatusBadGateway, Error: err.Error()}
	}
	defer resp.Body.Close()

	responseBody, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return outboundResult{Status: http.StatusBadGateway, Error: err.Error()}
	}
	return outboundResult{Status: resp.StatusCode, Body: string(responseBody)}
}

func postOutboundResult(ctx context.Context, client *http.Client, baseURL string, token string, commandID string, result outboundResult) error {
	payload, err := json.Marshal(result)
	if err != nil {
		return err
	}
	req, err := http.NewRequestWithContext(
		ctx,
		http.MethodPost,
		baseURL+"/api/agent/outbound/commands/"+commandID+"/result",
		bytes.NewReader(payload),
	)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	req.Header.Set("Content-Type", "application/json")

	resp, err := client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 4096))
		return fmt.Errorf("result returned %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	return nil
}

func sleepContext(ctx context.Context, duration time.Duration) {
	timer := time.NewTimer(duration)
	defer timer.Stop()
	select {
	case <-ctx.Done():
	case <-timer.C:
	}
}
