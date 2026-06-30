package caddyx

import (
	"encoding/json"
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

// DesiredDomain is one route the control plane wants served: a host and the
// upstream (container:port or host:port) Caddy reverse-proxies it to.
type DesiredDomain struct {
	Host     string `json:"host"`
	Upstream string `json:"upstream"`
}

// RFC-1123 hostname (no wildcards in v0). Lowercase labels, 1-63 chars each.
var hostPattern = regexp.MustCompile(`^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$`)

// upstream host is a container name / DNS name / IP (no scheme, no path).
var upstreamHostPattern = regexp.MustCompile(`^[a-zA-Z0-9](?:[a-zA-Z0-9._-]{0,253}[a-zA-Z0-9])?$`)

// ValidateHost checks an RFC-1123 fully-qualified host, rejecting wildcards.
func ValidateHost(host string) error {
	host = strings.TrimSpace(host)
	if host == "" {
		return fmt.Errorf("host is required")
	}
	if strings.Contains(host, "*") {
		return fmt.Errorf("wildcard hosts are not supported")
	}
	if len(host) > 253 || !hostPattern.MatchString(host) {
		return fmt.Errorf("invalid host %q", host)
	}
	return nil
}

// ValidateUpstream checks a "host:port" upstream (container or host).
func ValidateUpstream(upstream string) error {
	upstream = strings.TrimSpace(upstream)
	host, port, found := strings.Cut(upstream, ":")
	if !found {
		return fmt.Errorf("upstream must be host:port")
	}
	if !upstreamHostPattern.MatchString(host) {
		return fmt.Errorf("invalid upstream host %q", host)
	}
	n, err := strconv.Atoi(port)
	if err != nil || n < 1 || n > 65535 {
		return fmt.Errorf("invalid upstream port %q", port)
	}
	return nil
}

// --- Typed Caddy config structs (never string-concatenated JSON) ---

type config struct {
	Admin *adminConfig `json:"admin,omitempty"`
	Apps  appsConfig   `json:"apps"`
}

type adminConfig struct {
	Listen string `json:"listen"`
}

type appsConfig struct {
	HTTP httpApp `json:"http"`
}

type httpApp struct {
	Servers map[string]httpServer `json:"servers"`
}

type httpServer struct {
	Listen []string `json:"listen"`
	Routes []route  `json:"routes"`
}

type route struct {
	Match  []match   `json:"match"`
	Handle []handler `json:"handle"`
}

type match struct {
	Host []string `json:"host"`
}

type handler struct {
	Handler   string     `json:"handler"`
	Upstreams []upstream `json:"upstreams"`
}

type upstream struct {
	Dial string `json:"dial"`
}

// BuildConfig produces Caddy's full JSON config: admin on the unix socket, an
// HTTP server on :80/:443, and one Host-matched reverse_proxy route per domain.
// Automatic HTTPS is implicit (on by default for the matched hosts); on-demand
// TLS is intentionally never emitted. Each domain is validated first.
func BuildConfig(adminSocketPath string, domains []DesiredDomain) ([]byte, error) {
	routes := make([]route, 0, len(domains))
	for _, d := range domains {
		if err := ValidateHost(d.Host); err != nil {
			return nil, err
		}
		if err := ValidateUpstream(d.Upstream); err != nil {
			return nil, err
		}
		routes = append(routes, route{
			Match: []match{{Host: []string{d.Host}}},
			Handle: []handler{{
				Handler:   "reverse_proxy",
				Upstreams: []upstream{{Dial: d.Upstream}},
			}},
		})
	}

	cfg := config{
		Admin: &adminConfig{Listen: "unix/" + adminSocketPath},
		Apps: appsConfig{
			HTTP: httpApp{
				Servers: map[string]httpServer{
					"srv0": {Listen: []string{":80", ":443"}, Routes: routes},
				},
			},
		},
	}

	return json.Marshal(cfg)
}
