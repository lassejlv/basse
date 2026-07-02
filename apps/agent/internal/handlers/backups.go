package handlers

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/lassejlv/basse/apps/agent/internal/dockerx"
	"github.com/lassejlv/basse/apps/agent/internal/httpx"
)

// Backups runs database-specific backup/restore commands inside database app
// containers. Backup files live under <dataDir>/basse-backups inside the
// container, which sits on the database's data volume, so they survive
// container recreation.
type Backups struct {
	Docker *dockerx.Client
}

var backupIDPattern = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9-]*$`)

const (
	backupExecTimeout = 30 * time.Minute
	quickExecTimeout  = 30 * time.Second
)

func shellQuote(value string) string {
	return "'" + strings.ReplaceAll(value, "'", `'\''`) + "'"
}

func validDataDir(dir string) bool {
	return strings.HasPrefix(dir, "/") &&
		!strings.Contains(dir, "..") &&
		!strings.ContainsAny(dir, "'\"\n\t ")
}

func backupExtension(kind string) string {
	if kind == "redis" {
		return "rdb"
	}
	return "dump"
}

func backupFilePath(dataDir string, backupID string, kind string) string {
	return strings.TrimRight(dataDir, "/") + "/basse-backups/" + backupID + "." + backupExtension(kind)
}

type backupTarget struct {
	BackupID string `json:"backupId"`
	Kind     string `json:"kind"`
	Database string `json:"database"`
	User     string `json:"user"`
	Password string `json:"password"`
	DataDir  string `json:"dataDir"`
}

func (t *backupTarget) validate(needCredentials bool) string {
	if t.Kind == "" {
		t.Kind = "postgres"
	}
	if t.Kind != "postgres" && t.Kind != "redis" {
		return "invalid backup kind"
	}
	if !backupIDPattern.MatchString(t.BackupID) {
		return "invalid backupId"
	}
	if !validDataDir(t.DataDir) {
		return "invalid dataDir"
	}
	if needCredentials && t.Kind == "postgres" && (t.Database == "" || t.User == "") {
		return "database and user are required"
	}
	if needCredentials && t.Kind == "redis" && t.Password == "" {
		return "password is required"
	}
	return ""
}

// Create runs a database-specific backup inside the app container and reports
// the resulting file size.
func (b Backups) Create(w http.ResponseWriter, r *http.Request) {
	var req backupTarget
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		httpx.Error(w, http.StatusBadRequest, "invalid request body")
		return
	}
	if msg := req.validate(true); msg != "" {
		httpx.Error(w, http.StatusBadRequest, msg)
		return
	}

	name := containerName(r.PathValue("appId"))
	file := backupFilePath(req.DataDir, req.BackupID, req.Kind)
	dir := strings.TrimRight(req.DataDir, "/") + "/basse-backups"
	var command string
	if req.Kind == "redis" {
		password := shellQuote(req.Password)
		command = fmt.Sprintf(
			"mkdir -p %[1]s && start=$(redis-cli --no-auth-warning -a %[2]s LASTSAVE) && (redis-cli --no-auth-warning -a %[2]s BGSAVE >/tmp/basse-bgsave.out 2>&1 || grep -qi progress /tmp/basse-bgsave.out) && for i in $(seq 1 120); do now=$(redis-cli --no-auth-warning -a %[2]s LASTSAVE); [ \"$now\" != \"$start\" ] && break; sleep 1; done && cp %[3]s/dump.rdb %[4]s && wc -c < %[4]s",
			shellQuote(dir), password, shellQuote(strings.TrimRight(req.DataDir, "/")), shellQuote(file),
		)
	} else {
		command = fmt.Sprintf(
			"mkdir -p %s && pg_dump --format=custom --username=%s --dbname=%s --file=%s && wc -c < %s",
			shellQuote(dir), shellQuote(req.User), shellQuote(req.Database), shellQuote(file), shellQuote(file),
		)
	}

	ctx, cancel := context.WithTimeout(r.Context(), backupExecTimeout)
	defer cancel()
	result, err := b.Docker.ExecContainer(ctx, name, command)
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	if result.ExitCode != 0 {
		// Best-effort cleanup of a partial dump.
		cleanupCtx, cleanupCancel := context.WithTimeout(context.Background(), quickExecTimeout)
		defer cleanupCancel()
		_, _ = b.Docker.ExecContainer(cleanupCtx, name, "rm -f "+shellQuote(file))
		httpx.Error(w, http.StatusBadGateway, req.Kind+" backup failed: "+strings.TrimSpace(result.Output))
		return
	}

	sizeBytes, _ := strconv.ParseInt(strings.TrimSpace(lastLine(result.Output)), 10, 64)
	httpx.JSON(w, http.StatusOK, map[string]int64{"sizeBytes": sizeBytes})
}

// Restore restores an existing backup file into the database container.
func (b Backups) Restore(w http.ResponseWriter, r *http.Request) {
	var req backupTarget
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		httpx.Error(w, http.StatusBadRequest, "invalid request body")
		return
	}
	req.BackupID = r.PathValue("backupId")
	if msg := req.validate(true); msg != "" {
		httpx.Error(w, http.StatusBadRequest, msg)
		return
	}

	name := containerName(r.PathValue("appId"))
	file := backupFilePath(req.DataDir, req.BackupID, req.Kind)
	var command string
	if req.Kind == "redis" {
		command = fmt.Sprintf(
			"test -f %[1]s && cp %[1]s %[2]s/dump.rdb && redis-cli --no-auth-warning -a %[3]s SHUTDOWN NOSAVE >/dev/null 2>&1 || true",
			shellQuote(file), shellQuote(strings.TrimRight(req.DataDir, "/")), shellQuote(req.Password),
		)
	} else {
		command = fmt.Sprintf(
			"test -f %s && pg_restore --clean --if-exists --username=%s --dbname=%s %s",
			shellQuote(file), shellQuote(req.User), shellQuote(req.Database), shellQuote(file),
		)
	}

	ctx, cancel := context.WithTimeout(r.Context(), backupExecTimeout)
	defer cancel()
	result, err := b.Docker.ExecContainer(ctx, name, command)
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	if result.ExitCode != 0 {
		httpx.Error(w, http.StatusBadGateway, req.Kind+" restore failed: "+strings.TrimSpace(result.Output))
		return
	}
	if req.Kind == "redis" {
		restartCtx, restartCancel := context.WithTimeout(r.Context(), quickExecTimeout)
		defer restartCancel()
		if err := b.Docker.RestartContainer(restartCtx, name); err != nil {
			httpx.Error(w, http.StatusBadGateway, "redis restart failed: "+err.Error())
			return
		}
	}
	httpx.JSON(w, http.StatusOK, map[string]bool{"ok": true})
}

// Delete removes a dump file. Missing files are treated as success.
func (b Backups) Delete(w http.ResponseWriter, r *http.Request) {
	req := backupTarget{BackupID: r.PathValue("backupId"), DataDir: r.URL.Query().Get("dataDir"), Kind: r.URL.Query().Get("kind")}
	if msg := req.validate(false); msg != "" {
		httpx.Error(w, http.StatusBadRequest, msg)
		return
	}

	ctx, cancel := context.WithTimeout(r.Context(), quickExecTimeout)
	defer cancel()
	file := backupFilePath(req.DataDir, req.BackupID, req.Kind)
	result, err := b.Docker.ExecContainer(ctx, containerName(r.PathValue("appId")), "rm -f "+shellQuote(file))
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	if result.ExitCode != 0 {
		httpx.Error(w, http.StatusBadGateway, "delete failed: "+strings.TrimSpace(result.Output))
		return
	}
	httpx.JSON(w, http.StatusOK, map[string]bool{"ok": true})
}

// Download streams the raw dump file. It stats the file first so a missing
// backup yields a clean 404 and the response carries a Content-Length.
func (b Backups) Download(w http.ResponseWriter, r *http.Request) {
	req := backupTarget{BackupID: r.PathValue("backupId"), DataDir: r.URL.Query().Get("dataDir"), Kind: r.URL.Query().Get("kind")}
	if msg := req.validate(false); msg != "" {
		httpx.Error(w, http.StatusBadRequest, msg)
		return
	}

	name := containerName(r.PathValue("appId"))
	file := backupFilePath(req.DataDir, req.BackupID, req.Kind)

	statCtx, statCancel := context.WithTimeout(r.Context(), quickExecTimeout)
	defer statCancel()
	stat, err := b.Docker.ExecContainer(statCtx, name, "wc -c < "+shellQuote(file))
	if err != nil {
		httpx.Error(w, http.StatusInternalServerError, err.Error())
		return
	}
	if stat.ExitCode != 0 {
		httpx.Error(w, http.StatusNotFound, "backup file not found")
		return
	}
	size := strings.TrimSpace(stat.Output)

	w.Header().Set("Content-Type", "application/octet-stream")
	w.Header().Set("Content-Disposition", `attachment; filename="`+req.BackupID+`.`+backupExtension(req.Kind)+`"`)
	if _, err := strconv.ParseInt(size, 10, 64); err == nil {
		w.Header().Set("Content-Length", size)
	}

	ctx, cancel := context.WithTimeout(r.Context(), backupExecTimeout)
	defer cancel()
	exitCode, stderr, err := b.Docker.ExecContainerStdout(ctx, name, "cat "+shellQuote(file), w)
	if err != nil || exitCode != 0 {
		// Headers are already sent; the short body signals truncation to the client.
		_ = stderr
		return
	}
}

func lastLine(output string) string {
	lines := strings.Split(strings.TrimSpace(output), "\n")
	return lines[len(lines)-1]
}
