package handlers

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"time"

	"github.com/lassejlv/basse/apps/agent/internal/dockerx"
	"github.com/lassejlv/basse/apps/agent/internal/httpx"
)

// Images prunes Basse-managed images so old deployment images don't fill the
// server's disk. Only images Basse itself created or pulled are touched:
// server builds (basse-app:*) and Depot registry pulls (*.registry.depot.dev/*).
type Images struct {
	Docker *dockerx.Client
}

func managedImageTag(tag string) bool {
	return strings.HasPrefix(tag, "basse-app:") || strings.Contains(tag, ".registry.depot.dev/")
}

// Prune removes managed image tags older than the cutoff that are not in the
// keep list. In-use images (409) are skipped silently.
func (i Images) Prune(w http.ResponseWriter, r *http.Request) {
	var req struct {
		KeepRefs       []string `json:"keepRefs"`
		OlderThanHours int      `json:"olderThanHours"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		httpx.Error(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if req.OlderThanHours < 1 {
		httpx.Error(w, http.StatusBadRequest, "olderThanHours must be at least 1")
		return
	}

	keep := make(map[string]bool, len(req.KeepRefs))
	for _, ref := range req.KeepRefs {
		keep[ref] = true
	}
	cutoff := time.Now().Add(-time.Duration(req.OlderThanHours) * time.Hour).Unix()

	ctx, cancel := context.WithTimeout(r.Context(), 2*time.Minute)
	defer cancel()

	images, err := i.Docker.ListImages(ctx)
	if err != nil {
		httpx.Error(w, http.StatusBadGateway, "list images: "+err.Error())
		return
	}

	removed := 0
	skipped := 0
	for _, img := range images {
		if img.Created >= cutoff {
			continue
		}
		for _, tag := range img.RepoTags {
			if !managedImageTag(tag) || keep[tag] {
				continue
			}
			conflict, err := i.Docker.RemoveImage(ctx, tag)
			if err != nil || conflict {
				skipped++
				continue
			}
			removed++
		}
	}

	httpx.JSON(w, http.StatusOK, map[string]int{"removed": removed, "skipped": skipped})
}
