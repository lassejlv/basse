//! Branded and custom error pages: the per-service switch, custom pages per service and the
//! instance defaults, and repository pages read on every deploy. The proxy gets them through
//! [`AppState::proxy_pages`].

use crate::db::{ServiceRecord, StoredErrorPage};
use crate::error::{ApiResult, AppError, bad_request};
use crate::state::AppState;
use anyhow::Result;
use chrono::Utc;
use common::api::SetErrorPage;
use common::*;
use std::path::{Component, Path};

/// The branded switch and custom pages of a service.
pub async fn of_service(state: &AppState, service_id: &str) -> Result<ErrorPages> {
    Ok(ErrorPages {
        branded: state.db.branded_error_pages(service_id).await?,
        custom: pages(state, Some(service_id)).await?,
    })
}

/// The instance defaults.
pub async fn of_instance(state: &AppState) -> Result<ErrorPages> {
    Ok(ErrorPages {
        branded: true,
        custom: pages(state, None).await?,
    })
}

async fn pages(state: &AppState, service_id: Option<&str>) -> Result<Vec<CustomErrorPage>> {
    Ok(state
        .db
        .error_pages(service_id)
        .await?
        .into_iter()
        .map(|p| p.page)
        .collect())
}

/// Sets a custom page of a service, or of the instance for `None`.
pub async fn set(
    state: &AppState,
    service: Option<&ServiceRecord>,
    kind: ErrorPageKind,
    body: SetErrorPage,
) -> ApiResult<CustomErrorPage> {
    let (source, html, warnings) = match body {
        SetErrorPage::Upload { html, name } => {
            let warnings = check_error_page(&html).map_err(AppError::BadRequest)?;
            let name = name
                .map(|n| n.rsplit(['/', '\\']).next().unwrap_or_default().to_string())
                .filter(|n| !n.is_empty());
            (ErrorPageSource::Upload { name }, Some(html), warnings)
        }
        SetErrorPage::Repo { path } => {
            let Some(rec) = service else {
                bad_request!(
                    "instance error pages are uploaded; only services read them from a repository"
                );
            };
            if !matches!(rec.service.source, Source::Git { .. }) {
                bad_request!(
                    "'{}' is not built from a repository; upload the page instead",
                    rec.service.name
                );
            }
            let path = clean_path(&path).map_err(AppError::BadRequest)?;
            (
                ErrorPageSource::Repo { path },
                None,
                vec!["read from the repository on the next deploy".to_string()],
            )
        }
    };
    let stored = StoredErrorPage {
        service_id: service.map(|r| r.service.id.clone()),
        page: CustomErrorPage {
            kind,
            source,
            size: html.as_ref().map(|h| h.len() as u64),
            warnings,
            updated_at: Utc::now(),
        },
        html,
    };
    state.db.set_error_page(&stored).await?;
    refresh(state, service).await?;
    Ok(stored.page)
}

/// Removes a custom page, so the instance default or the built-in page is served again.
pub async fn delete(
    state: &AppState,
    service: Option<&ServiceRecord>,
    kind: ErrorPageKind,
) -> ApiResult<()> {
    let owner = service.map(|r| r.service.id.as_str());
    if !state.db.delete_error_page(owner, kind).await? {
        return Err(AppError::NotFound(format!("no custom {kind} page is set")));
    }
    refresh(state, service).await?;
    Ok(())
}

/// Rewrites the pages on the service's proxy, or on every proxy for the instance defaults.
async fn refresh(state: &AppState, service: Option<&ServiceRecord>) -> Result<()> {
    match service {
        Some(rec) => state.refresh_proxy_pages(&rec.service.server_id).await,
        None => {
            for server in state.db.servers().await? {
                state.refresh_proxy_pages(&server.id).await;
            }
        }
    }
    Ok(())
}

/// A repository path relative to the service's root, without `.` segments.
fn clean_path(path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() || path.contains(['\\', '\0']) || path.starts_with('/') {
        return Err(format!(
            "invalid page path '{path}': give a file relative to the service's root directory, such as errors/503.html"
        ));
    }
    let mut parts = Vec::new();
    for c in Path::new(path).components() {
        match c {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "page path '{path}' must stay inside the repository: '..' is not allowed"
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

/// Reads the service's repository pages from `root`, the service's root directory in a fresh
/// clone, and stores them. A page that cannot be read falls back to the default and says why;
/// it never fails the deploy. Returns one log line per page.
pub async fn load_from_repo(
    state: &AppState,
    service_id: &str,
    root: &Path,
) -> Result<Vec<String>> {
    let mut log = Vec::new();
    for mut stored in state.db.error_pages(Some(service_id)).await? {
        let ErrorPageSource::Repo { path } = &stored.page.source else {
            continue;
        };
        let kind = stored.page.kind;
        let read = read_page(root, path);
        let (html, warnings) = match read {
            Ok(html) => match check_error_page(&html) {
                Ok(warnings) => {
                    log.push(format!(
                        "Error page {kind}: {path} ({} KB)",
                        html.len().div_ceil(1024)
                    ));
                    log.extend(warnings.iter().map(|w| format!("Error page {kind}: {w}")));
                    (Some(html), warnings)
                }
                Err(e) => (None, vec![format!("{path}: {e}")]),
            },
            Err(e) => (None, vec![e]),
        };
        if html.is_none() {
            log.extend(
                warnings
                    .iter()
                    .map(|w| format!("Error page {kind}: {w}; serving the default page")),
            );
        }
        stored.page.size = html.as_ref().map(|h| h.len() as u64);
        stored.page.warnings = warnings;
        stored.page.updated_at = Utc::now();
        stored.html = html;
        state.db.set_error_page(&stored).await?;
    }
    Ok(log)
}

fn read_page(root: &Path, path: &str) -> Result<String, String> {
    let file = root.join(path);
    let inside = match (file.canonicalize(), root.canonicalize()) {
        (Ok(f), Ok(r)) => f.starts_with(r),
        _ => return Err(format!("{path} is not in the repository")),
    };
    if !inside {
        return Err(format!("{path} must stay inside the repository"));
    }
    let meta = std::fs::metadata(&file).map_err(|e| format!("cannot read {path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path} is not a file"));
    }
    if meta.len() > MAX_ERROR_PAGE_BYTES as u64 {
        return Err(format!(
            "{path} is {} KB; error pages can be at most {} KB",
            meta.len().div_ceil(1024),
            MAX_ERROR_PAGE_BYTES / 1024
        ));
    }
    let bytes = std::fs::read(&file).map_err(|e| format!("cannot read {path}: {e}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8 text"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_paths_stay_inside_the_repository() {
        assert_eq!(clean_path("./errors//503.html").unwrap(), "errors/503.html");
        for bad in [
            "",
            "/etc/passwd",
            "../x.html",
            "a/../../x.html",
            "a\\b.html",
        ] {
            assert!(clean_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn repo_pages_are_read_with_limits() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("errors")).unwrap();
        std::fs::write(root.join("errors/503.html"), "<h1>down</h1>").unwrap();
        std::fs::write(root.join("big.html"), "x".repeat(MAX_ERROR_PAGE_BYTES + 1)).unwrap();
        assert_eq!(read_page(root, "errors/503.html").unwrap(), "<h1>down</h1>");
        assert!(
            read_page(root, "missing.html")
                .unwrap_err()
                .contains("not in the repository")
        );
        assert!(
            read_page(root, "errors")
                .unwrap_err()
                .contains("not a file")
        );
        assert!(
            read_page(root, "big.html")
                .unwrap_err()
                .contains("at most 256 KB")
        );
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("secret.html"), "x").unwrap();
            std::os::unix::fs::symlink(outside.path().join("secret.html"), root.join("link.html"))
                .unwrap();
            assert!(read_page(root, "link.html").unwrap_err().contains("inside"));
        }
    }
}
