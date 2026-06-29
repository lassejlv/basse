// Package middleware holds HTTP middleware for the agent.
package middleware

import (
	"crypto/subtle"
	"net/http"
	"strings"

	"github.com/lassejlv/basse/apps/agent/internal/httpx"
)

// Bearer guards a handler with a constant-time bearer-token check. The token is
// the per-server credential the control plane provisions. The comparison is
// constant-time to avoid leaking it via timing.
func Bearer(token string, next http.Handler) http.Handler {
	expected := []byte("Bearer " + token)

	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		provided := strings.TrimSpace(r.Header.Get("Authorization"))

		if subtle.ConstantTimeCompare([]byte(provided), expected) != 1 {
			httpx.Error(w, http.StatusUnauthorized, "unauthorized")
			return
		}

		next.ServeHTTP(w, r)
	})
}
