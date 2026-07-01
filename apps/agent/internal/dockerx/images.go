package dockerx

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
)

// ImageSummary is the subset of GET /images/json we read for pruning.
type ImageSummary struct {
	ID       string   `json:"Id"`
	RepoTags []string `json:"RepoTags"`
	Created  int64    `json:"Created"`
}

// ListImages returns all images known to the engine.
func (c *Client) ListImages(ctx context.Context) ([]ImageSummary, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://docker/images/json", nil)
	if err != nil {
		return nil, err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return nil, fmt.Errorf("list images: status %d: %s", resp.StatusCode, b)
	}

	var out []ImageSummary
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return nil, err
	}
	return out, nil
}

// RemoveImage untags/removes an image reference. Not-found is success; 409
// (image in use by a container) reports conflict=true so callers can skip it.
func (c *Client) RemoveImage(ctx context.Context, ref string) (conflict bool, err error) {
	req, err := http.NewRequestWithContext(
		ctx,
		http.MethodDelete,
		"http://docker/images/"+url.PathEscape(ref),
		nil,
	)
	if err != nil {
		return false, err
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return false, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return false, nil
	}
	if resp.StatusCode == http.StatusConflict {
		return true, nil
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 2048))
		return false, fmt.Errorf("remove image %s: status %d: %s", ref, resp.StatusCode, b)
	}
	return false, nil
}
