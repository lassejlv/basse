//! Git sources: credentials, short-lived access tokens and status checks, shared by the API,
//! the GitHub callbacks and the deploy pipeline.

use crate::db::SourceRecord;
use crate::github::{self, Github};
use crate::state::AppState;
use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use common::*;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// How long a source's repository list is reused for paging and searching.
const REPO_CACHE: Duration = Duration::from_secs(60);

/// The GitHub App credentials of a source.
pub struct AppCredentials {
    pub app_id: u64,
    pub private_key: Zeroizing<String>,
    pub webhook_secret: Option<Zeroizing<String>>,
}

pub async fn app_credentials(state: &AppState, source: &GitSource) -> Result<AppCredentials> {
    let secret = source
        .secret_id
        .as_ref()
        .context("the GitHub App was not created yet")?;
    match state.secrets.value(secret).await? {
        SecretValue::GithubApp {
            app_id,
            private_key,
            webhook_secret,
            ..
        } => Ok(AppCredentials {
            app_id,
            private_key: Zeroizing::new(private_key),
            webhook_secret: webhook_secret.map(Zeroizing::new),
        }),
        other => bail!(
            "the secret of source '{}' holds {}, not GitHub App credentials",
            source.name,
            other.kind().label()
        ),
    }
}

/// Seals and stores a secret a source owns. Unlike the secrets API this accepts GitHub App
/// credentials, which only sources may create.
pub async fn store_secret(
    state: &AppState,
    name: &str,
    value: &SecretValue,
    public_part: Option<String>,
) -> Result<Secret, crate::error::AppError> {
    if state.db.secret_name_taken(name).await? {
        return Err(crate::error::AppError::Conflict(format!(
            "a secret named '{name}' already exists; rename it or pick another source name"
        )));
    }
    let now = Utc::now();
    let secret = Secret {
        id: crate::db::new_id(),
        name: name.to_string(),
        kind: value.kind(),
        created_at: now,
        updated_at: now,
        public_part,
        used_by: Vec::new(),
    };
    let sealed = state.secrets.seal(&secret.id, value)?;
    state.db.insert_secret(&secret, &sealed).await?;
    Ok(secret)
}

/// A JWT signed with the source's app key.
pub async fn app_jwt(state: &AppState, source: &GitSource) -> Result<String> {
    let creds = app_credentials(state, source).await?;
    github::app_jwt(creds.app_id, &creds.private_key)
}

/// A token to call the API and clone with: a fresh installation token for an app, valid for an
/// hour, or the stored personal access token.
pub async fn access_token(state: &AppState, source: &GitSource) -> Result<Zeroizing<String>> {
    match source.kind {
        GitSourceKind::GithubApp => {
            let installation = source.installation_id.with_context(|| {
                format!(
                    "the GitHub App of source '{}' is not installed on an account yet",
                    source.name
                )
            })?;
            let jwt = app_jwt(state, source).await?;
            Github::new(&source.api_url)?
                .installation_token(&jwt, installation)
                .await
        }
        GitSourceKind::GithubToken => {
            let secret = source.secret_id.as_ref().context("the token is missing")?;
            match state.secrets.value(secret).await? {
                SecretValue::Generic { value } => Ok(Zeroizing::new(value)),
                other => bail!("the token secret holds {}", other.kind().label()),
            }
        }
    }
}

/// The HTTPS clone URL of `owner/name` and a token for it.
pub async fn clone_access(
    state: &AppState,
    source_id: &str,
    repo: &str,
) -> Result<(String, Zeroizing<String>)> {
    let rec = state
        .db
        .source(source_id)
        .await?
        .context("the git source of this service was deleted")?;
    github::validate_full_name(repo)?;
    let token = access_token(state, &rec.source)
        .await
        .with_context(|| format!("cannot get a token from git source '{}'", rec.source.name))?;
    Ok((format!("{}/{repo}.git", rec.source.html_url), token))
}

/// Fills in the URLs the API derives rather than stores.
pub fn present(state: &AppState, mut source: GitSource) -> GitSource {
    if source.kind == GitSourceKind::GithubApp {
        source.install_url = source
            .app_slug
            .as_deref()
            .map(|slug| github::install_url(&source.html_url, slug));
        source.webhook_url = state
            .instance_url()
            .map(|url| format!("{url}/github/{}/webhook", source.id));
    }
    source
}

/// Asks GitHub whether the source still works and stores the outcome.
pub async fn check(state: &AppState, mut rec: SourceRecord) -> Result<GitSource> {
    let outcome = probe(state, &mut rec.source).await;
    let s = &mut rec.source;
    s.last_checked_at = Some(Utc::now());
    match outcome {
        Ok((status, detail)) => {
            s.status = status;
            s.status_detail = detail;
        }
        Err(e) => {
            s.status = GitSourceStatus::Error;
            s.status_detail = Some(format!("{e:#}"));
        }
    }
    state.db.save_source(&rec).await?;
    state.repo_cache_forget(&rec.source.id);
    Ok(present(state, rec.source))
}

async fn probe(state: &AppState, s: &mut GitSource) -> Result<(GitSourceStatus, Option<String>)> {
    let gh = Github::new(&s.api_url)?;
    match s.kind {
        GitSourceKind::GithubToken => {
            let token = access_token(state, s).await?;
            let user = gh.token_user(&token).await?;
            s.owner = Some(user.login);
            s.owner_kind = Some(GitOwnerKind::User);
            Ok((GitSourceStatus::Ready, None))
        }
        GitSourceKind::GithubApp => {
            if s.secret_id.is_none() {
                return Ok((
                    GitSourceStatus::Pending,
                    Some("open the setup URL to create the app on GitHub".into()),
                ));
            }
            let jwt = app_jwt(state, s).await?;
            let app = gh.app(&jwt).await?;
            s.app_id = Some(app.id);
            s.app_slug = Some(app.slug);
            let installation = match s.installation_id {
                Some(id) => Some(gh.installation(&jwt, id).await?),
                None => {
                    let mut all = gh.installations(&jwt).await?;
                    (all.len() == 1).then(|| all.remove(0))
                }
            };
            let Some(installation) = installation else {
                return Ok((
                    GitSourceStatus::NeedsInstall,
                    Some("install the app on an account and choose its repositories".into()),
                ));
            };
            s.installation_id = Some(installation.id);
            if let Some(account) = installation.account.or(app.owner) {
                s.owner_kind = Some(account.owner_kind());
                s.owner = Some(account.login);
            }
            gh.installation_token(&jwt, installation.id).await?;
            Ok((GitSourceStatus::Ready, None))
        }
    }
}

/// Every repository the source can clone, from a short-lived cache.
pub async fn repos(state: &AppState, source: &GitSource) -> Result<Vec<GitRepo>> {
    if let Some(cached) = state.repo_cache_get(&source.id, REPO_CACHE) {
        return Ok(cached);
    }
    let token = access_token(state, source).await?;
    let gh = Github::new(&source.api_url)?;
    let repos = match source.kind {
        GitSourceKind::GithubApp => gh.installation_repos(&token).await?,
        GitSourceKind::GithubToken => gh.token_repos(&token).await?,
    };
    let mut repos = repos;
    repos.sort_by_key(|r| std::cmp::Reverse(r.pushed_at));
    state.repo_cache_put(&source.id, repos.clone(), Instant::now());
    Ok(repos)
}

/// One page of the repositories whose `owner/name` contains `query`, any case.
pub fn page(repos: &[GitRepo], query: &str, page: u32, per_page: u32) -> RepoPage {
    let per_page = per_page.clamp(1, 100);
    let page = page.max(1);
    let q = query.trim().to_ascii_lowercase();
    let matching: Vec<&GitRepo> = repos
        .iter()
        .filter(|r| q.is_empty() || r.full_name.to_ascii_lowercase().contains(&q))
        .collect();
    let start = (page as usize - 1) * per_page as usize;
    RepoPage {
        total: u32::try_from(matching.len()).unwrap_or(u32::MAX),
        repos: matching
            .into_iter()
            .skip(start)
            .take(per_page as usize)
            .cloned()
            .collect(),
        page,
        per_page,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> GitRepo {
        GitRepo {
            full_name: name.into(),
            private: false,
            default_branch: "main".into(),
            description: None,
            pushed_at: None,
        }
    }

    #[test]
    fn pages_filter_then_slice() {
        let repos: Vec<GitRepo> = (1..=45).map(|i| repo(&format!("acme/app-{i}"))).collect();
        let p = page(&repos, "", 2, 20);
        assert_eq!((p.total, p.pages(), p.repos.len()), (45, 3, 20));
        assert_eq!(p.repos[0].full_name, "acme/app-21");
        let p = page(&repos, "APP-4", 1, 20);
        assert_eq!(
            p.repos
                .iter()
                .map(|r| r.full_name.as_str())
                .collect::<Vec<_>>(),
            [
                "acme/app-4",
                "acme/app-40",
                "acme/app-41",
                "acme/app-42",
                "acme/app-43",
                "acme/app-44",
                "acme/app-45"
            ]
        );
        let p = page(&repos, "nothing", 1, 20);
        assert_eq!((p.total, p.pages()), (0, 1));
        assert!(page(&repos, "", 9, 20).repos.is_empty());
    }
}
