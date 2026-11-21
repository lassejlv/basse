//! GitHub's REST API: the App manifest flow, app JWTs, installation tokens, repositories,
//! branches and webhook signatures. Every call takes the API base URL, so GitHub Enterprise and
//! test mocks work the same way.

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use chrono::Utc;
use common::{GitBranch, GitOwnerKind, GitRepo, brand};
use hmac::{Hmac, Mac as _};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::Sha256;
use std::time::Duration;
use zeroize::Zeroizing;

pub const DEFAULT_HTML_URL: &str = "https://github.com";
pub const DEFAULT_API_URL: &str = "https://api.github.com";
/// Pages fetched at most when listing repositories or branches, 100 items each.
const MAX_PAGES: u32 = 10;
/// GitHub refuses app names longer than this.
const APP_NAME_MAX: usize = 34;

/// The web and API base URLs for an optional GitHub Enterprise web URL and API URL.
pub fn base_urls(html: Option<&str>, api: Option<&str>) -> Result<(String, String), String> {
    let clean = |url: &str| url.trim().trim_end_matches('/').to_string();
    let html = html
        .map(clean)
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_HTML_URL.into());
    let api = api.map(clean).filter(|u| !u.is_empty()).unwrap_or_else(|| {
        if html == DEFAULT_HTML_URL {
            DEFAULT_API_URL.into()
        } else {
            format!("{html}/api/v3")
        }
    });
    for url in [&html, &api] {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(format!("'{url}' is not an http(s) URL"));
        }
    }
    Ok((html, api))
}

/// Name of the app the manifest flow creates: the product name plus the instance domain, cut
/// to GitHub's limit.
pub fn app_name(domain: &str) -> String {
    let name = format!("{} {domain}", brand::NAME);
    name.chars()
        .take(APP_NAME_MAX)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The manifest posted to GitHub to create an app whose callbacks reach this instance.
pub fn manifest(name: &str, instance_url: &str, source_id: &str) -> serde_json::Value {
    let base = format!("{instance_url}/github/{source_id}");
    serde_json::json!({
        "name": name,
        "url": instance_url,
        "hook_attributes": { "url": format!("{base}/webhook"), "active": true },
        "redirect_url": format!("{base}/callback"),
        "callback_urls": [format!("{base}/callback")],
        "setup_url": format!("{base}/setup"),
        "setup_on_update": true,
        "public": false,
        "default_permissions": { "contents": "read", "metadata": "read" },
        "default_events": ["push"],
    })
}

/// Where the manifest is posted: the personal account's or an organization's new-app page.
pub fn new_app_url(html_url: &str, organization: Option<&str>, state: &str) -> String {
    match organization {
        Some(org) => format!("{html_url}/organizations/{org}/settings/apps/new?state={state}"),
        None => format!("{html_url}/settings/apps/new?state={state}"),
    }
}

pub fn install_url(html_url: &str, slug: &str) -> String {
    format!("{html_url}/apps/{slug}/installations/new")
}

/// A JWT that authenticates as the app for ten minutes minus clock-skew margin.
pub fn app_jwt(app_id: u64, private_key_pem: &str) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Claims {
        iat: i64,
        exp: i64,
        iss: String,
    }
    let now = Utc::now().timestamp();
    let key = jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem.as_bytes())
        .context("the private key is not an RSA key in PEM format")?;
    let claims = Claims {
        iat: now - 60,
        exp: now + 540,
        iss: app_id.to_string(),
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &key,
    )
    .context("cannot sign the app JWT")
}

/// Checks `X-Hub-Signature-256` (`sha256=<hex>`) against the body, in constant time.
pub fn signature_matches(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(hex_sig) = header.trim().strip_prefix("sha256=") else {
        return false;
    };
    let Ok(sig) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

/// Git configuration, passed through the environment, that authenticates HTTPS clones with a
/// token without writing it to disk or the command line.
pub fn git_auth_env(token: &str) -> Vec<(String, String)> {
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    vec![
        ("GIT_CONFIG_COUNT".into(), "1".into()),
        ("GIT_CONFIG_KEY_0".into(), "http.extraHeader".into()),
        (
            "GIT_CONFIG_VALUE_0".into(),
            format!("Authorization: Basic {basic}"),
        ),
    ]
}

#[derive(Debug, Clone, Deserialize)]
pub struct Account {
    pub login: String,
    #[serde(rename = "type", default)]
    pub kind: String,
}

impl Account {
    pub fn owner_kind(&self) -> GitOwnerKind {
        if self.kind.eq_ignore_ascii_case("organization") {
            GitOwnerKind::Organization
        } else {
            GitOwnerKind::User
        }
    }
}

/// What GitHub returns when a manifest code is exchanged: the new app and its credentials.
#[derive(Deserialize)]
pub struct ManifestConversion {
    pub id: u64,
    pub slug: String,
    pub owner: Option<Account>,
    pub pem: String,
    #[serde(default)]
    pub webhook_secret: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub client_secret: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct App {
    pub id: u64,
    pub slug: String,
    pub owner: Option<Account>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Installation {
    pub id: u64,
    pub account: Option<Account>,
}

#[derive(Deserialize)]
struct AccessToken {
    token: String,
}

#[derive(Deserialize)]
struct RawRepo {
    full_name: String,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    default_branch: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    pushed_at: Option<chrono::DateTime<Utc>>,
}

impl From<RawRepo> for GitRepo {
    fn from(r: RawRepo) -> Self {
        Self {
            full_name: r.full_name,
            private: r.private,
            default_branch: r.default_branch,
            description: r.description.filter(|d| !d.is_empty()),
            pushed_at: r.pushed_at,
        }
    }
}

#[derive(Deserialize)]
struct RepoList {
    repositories: Vec<RawRepo>,
}

#[derive(Deserialize)]
struct RawBranch {
    name: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    message: String,
}

/// How a call authenticates.
pub enum Auth<'a> {
    None,
    /// An app JWT, for `/app` routes.
    App(&'a str),
    /// An installation token or a personal access token.
    Token(&'a str),
}

/// A client for one GitHub API base URL.
pub struct Github {
    http: reqwest::Client,
    api: String,
}

impl Github {
    pub fn new(api_url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(format!(
                "{}/{}",
                brand::DAEMON_BIN,
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self {
            http,
            api: api_url.trim_end_matches('/').to_string(),
        })
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        auth: Auth<'_>,
    ) -> Result<T> {
        let url = format!("{}{path}", self.api);
        let mut rb = self
            .http
            .request(method, &url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        rb = match auth {
            Auth::None => rb,
            Auth::App(jwt) | Auth::Token(jwt) => rb.bearer_auth(jwt),
        };
        let res = rb
            .send()
            .await
            .with_context(|| format!("cannot reach GitHub at {}", self.api))?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            let message = serde_json::from_str::<ErrorBody>(&body)
                .map(|e| e.message)
                .unwrap_or(body);
            bail!("GitHub returned {status} for {path}: {message}");
        }
        res.json()
            .await
            .with_context(|| format!("unexpected response from GitHub for {path}"))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, auth: Auth<'_>) -> Result<T> {
        self.call(reqwest::Method::GET, path, auth).await
    }

    /// Exchanges the one-time code of the manifest flow for the new app's credentials.
    pub async fn convert_manifest(&self, code: &str) -> Result<ManifestConversion> {
        if !code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            bail!("invalid manifest code");
        }
        self.call(
            reqwest::Method::POST,
            &format!("/app-manifests/{code}/conversions"),
            Auth::None,
        )
        .await
    }

    pub async fn app(&self, jwt: &str) -> Result<App> {
        self.get("/app", Auth::App(jwt)).await
    }

    /// An installation of the app the JWT is for; fails for another app's installation.
    pub async fn installation(&self, jwt: &str, id: u64) -> Result<Installation> {
        self.get(&format!("/app/installations/{id}"), Auth::App(jwt))
            .await
    }

    pub async fn installations(&self, jwt: &str) -> Result<Vec<Installation>> {
        self.get("/app/installations?per_page=100", Auth::App(jwt))
            .await
    }

    /// A token for one installation, valid for an hour.
    pub async fn installation_token(
        &self,
        jwt: &str,
        installation_id: u64,
    ) -> Result<Zeroizing<String>> {
        let t: AccessToken = self
            .call(
                reqwest::Method::POST,
                &format!("/app/installations/{installation_id}/access_tokens"),
                Auth::App(jwt),
            )
            .await?;
        Ok(Zeroizing::new(t.token))
    }

    /// The login a personal access token belongs to.
    pub async fn token_user(&self, token: &str) -> Result<Account> {
        self.get("/user", Auth::Token(token)).await
    }

    /// Every repository an installation token can see, in GitHub's order.
    pub async fn installation_repos(&self, token: &str) -> Result<Vec<GitRepo>> {
        let mut repos = Vec::new();
        for page in 1..=MAX_PAGES {
            let list: RepoList = self
                .get(
                    &format!("/installation/repositories?per_page=100&page={page}"),
                    Auth::Token(token),
                )
                .await?;
            let n = list.repositories.len();
            repos.extend(list.repositories.into_iter().map(GitRepo::from));
            if n < 100 {
                break;
            }
        }
        Ok(repos)
    }

    /// Every repository a personal access token can see, recently pushed first.
    pub async fn token_repos(&self, token: &str) -> Result<Vec<GitRepo>> {
        let mut repos = Vec::new();
        for page in 1..=MAX_PAGES {
            let list: Vec<RawRepo> = self
                .get(
                    &format!("/user/repos?per_page=100&page={page}&sort=pushed"),
                    Auth::Token(token),
                )
                .await?;
            let n = list.len();
            repos.extend(list.into_iter().map(GitRepo::from));
            if n < 100 {
                break;
            }
        }
        Ok(repos)
    }

    /// Branches of `owner/name`, the default branch first.
    pub async fn branches(&self, token: &str, full_name: &str) -> Result<Vec<GitBranch>> {
        validate_full_name(full_name)?;
        let repo: RawRepo = self
            .get(&format!("/repos/{full_name}"), Auth::Token(token))
            .await?;
        let mut names = Vec::new();
        for page in 1..=MAX_PAGES {
            let list: Vec<RawBranch> = self
                .get(
                    &format!("/repos/{full_name}/branches?per_page=100&page={page}"),
                    Auth::Token(token),
                )
                .await?;
            let n = list.len();
            names.extend(list.into_iter().map(|b| b.name));
            if n < 100 {
                break;
            }
        }
        let mut branches: Vec<GitBranch> = names
            .into_iter()
            .map(|name| GitBranch {
                default: name == repo.default_branch,
                name,
            })
            .collect();
        branches.sort_by_key(|b| !b.default);
        Ok(branches)
    }
}

#[derive(Deserialize)]
struct ContentEntry {
    name: String,
    #[serde(rename = "type", default)]
    kind: String,
}

impl Github {
    /// Names of the entries of one kind (`file` or `dir`) directly inside `path` on a
    /// branch; an empty `path` is the repository root. `path` must be normalized with
    /// [`common::clean_root`].
    pub async fn list_dir(
        &self,
        token: &str,
        full_name: &str,
        branch: &str,
        path: &str,
        kind: &str,
    ) -> Result<Vec<String>> {
        validate_full_name(full_name)?;
        let branch = url_encode(branch);
        let path: String = path
            .split('/')
            .filter(|p| !p.is_empty())
            .map(|p| format!("/{}", url_encode(p)))
            .collect();
        let listing: serde_json::Value = self
            .get(
                &format!("/repos/{full_name}/contents{path}?ref={branch}"),
                Auth::Token(token),
            )
            .await?;
        let Ok(entries) = serde_json::from_value::<Vec<ContentEntry>>(listing) else {
            bail!("'{}' is not a directory", path.trim_start_matches('/'));
        };
        Ok(entries
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.name)
            .collect())
    }
}

/// Percent-encodes everything but unreserved characters, for a query value.
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// `owner/name` with characters GitHub allows, so it can go into a URL path.
pub fn validate_full_name(full_name: &str) -> Result<()> {
    let ok_part = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    match full_name.split_once('/') {
        Some((owner, name)) if ok_part(owner) && ok_part(name) => Ok(()),
        _ => bail!("'{full_name}' is not a repository: expected owner/name"),
    }
}

/// The push event fields a deploy needs.
#[derive(Debug, Deserialize)]
pub struct PushEvent {
    #[serde(rename = "ref")]
    pub git_ref: String,
    #[serde(default)]
    pub deleted: bool,
    pub repository: PushRepo,
    #[serde(default)]
    pub commits: Vec<PushCommit>,
}

/// The files a pushed commit changed. GitHub leaves the lists out of some payloads.
#[derive(Debug, Deserialize)]
pub struct PushCommit {
    pub added: Option<Vec<String>>,
    pub modified: Option<Vec<String>>,
    pub removed: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct PushRepo {
    pub full_name: String,
}

impl PushEvent {
    /// The pushed branch; `None` for tags.
    pub fn branch(&self) -> Option<&str> {
        self.git_ref.strip_prefix("refs/heads/")
    }

    /// Whether the push should deploy a service living in `root` that watches its paths: a
    /// changed file is inside `root`, or the payload does not say which files changed (no
    /// commits, or a commit without file lists), so the push cannot be ruled out.
    pub fn touches(&self, root: &str) -> bool {
        if root.is_empty() {
            return true;
        }
        let prefix = format!("{root}/");
        self.commits.is_empty()
            || self.commits.iter().any(|c| {
                let lists = [&c.added, &c.modified, &c.removed];
                lists.iter().all(|l| l.is_none())
                    || lists
                        .into_iter()
                        .flatten()
                        .flatten()
                        .any(|f| f.starts_with(&prefix))
            })
    }
}

#[derive(Debug, Deserialize)]
pub struct InstallationEvent {
    pub action: String,
    pub installation: Installation,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enterprise_urls_default_to_the_v3_api() {
        assert_eq!(
            base_urls(None, None).unwrap(),
            (DEFAULT_HTML_URL.into(), DEFAULT_API_URL.into())
        );
        assert_eq!(
            base_urls(Some("https://git.acme.dev/"), None).unwrap(),
            (
                "https://git.acme.dev".into(),
                "https://git.acme.dev/api/v3".into()
            )
        );
        assert_eq!(
            base_urls(Some("https://git.acme.dev"), Some("https://api.acme.dev")).unwrap(),
            ("https://git.acme.dev".into(), "https://api.acme.dev".into())
        );
        assert!(base_urls(Some("git.acme.dev"), None).is_err());
    }

    #[test]
    fn app_names_fit_github_limit() {
        assert_eq!(
            app_name("deploy.example.com"),
            format!("{} deploy.example.com", brand::NAME)
        );
        let long = app_name("a-very-long-subdomain.of-a-long-domain.example.com");
        assert!(long.chars().count() <= APP_NAME_MAX);
        assert!(!long.ends_with(' '));
    }

    #[test]
    fn manifests_point_back_at_the_instance() {
        let m = manifest("app", "https://deploy.example.com", "src1");
        assert_eq!(
            m["redirect_url"],
            "https://deploy.example.com/github/src1/callback"
        );
        assert_eq!(
            m["hook_attributes"]["url"],
            "https://deploy.example.com/github/src1/webhook"
        );
        assert_eq!(
            m["setup_url"],
            "https://deploy.example.com/github/src1/setup"
        );
        assert_eq!(m["default_events"][0], "push");
        assert_eq!(m["public"], false);
        assert_eq!(
            new_app_url("https://github.com", Some("acme"), "s"),
            "https://github.com/organizations/acme/settings/apps/new?state=s"
        );
        assert_eq!(
            new_app_url("https://github.com", None, "s"),
            "https://github.com/settings/apps/new?state=s"
        );
    }

    #[test]
    fn webhook_signatures_are_verified() {
        let body = br#"{"zen":"hi"}"#;
        let mut mac = Hmac::<Sha256>::new_from_slice(b"s3cret").unwrap();
        mac.update(body);
        let good = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        assert!(signature_matches("s3cret", body, &good));
        assert!(!signature_matches("other", body, &good));
        assert!(!signature_matches("s3cret", b"{}", &good));
        assert!(!signature_matches("s3cret", body, "sha1=abc"));
        assert!(!signature_matches("s3cret", body, "sha256=zz"));
    }

    #[test]
    fn clone_tokens_travel_in_the_environment() {
        let env = git_auth_env("tok");
        assert_eq!(env[1].1, "http.extraHeader");
        let expected = base64::engine::general_purpose::STANDARD.encode("x-access-token:tok");
        assert_eq!(env[2].1, format!("Authorization: Basic {expected}"));
    }

    #[test]
    fn repository_names_are_checked_before_use_in_paths() {
        assert!(validate_full_name("acme/api.v2").is_ok());
        assert!(validate_full_name("acme").is_err());
        assert!(validate_full_name("acme/../x").is_err());
        assert!(validate_full_name("acme/a?b").is_err());
        assert!(validate_full_name("../api").is_err());
    }

    #[test]
    fn pushes_name_their_branch() {
        let e: PushEvent = serde_json::from_str(
            r#"{"ref":"refs/heads/main","repository":{"full_name":"acme/api"}}"#,
        )
        .unwrap();
        assert_eq!(e.branch(), Some("main"));
        let tag: PushEvent =
            serde_json::from_str(r#"{"ref":"refs/tags/v1","repository":{"full_name":"acme/api"}}"#)
                .unwrap();
        assert_eq!(tag.branch(), None);
    }

    #[test]
    fn pushes_deploy_a_root_only_when_a_file_inside_it_changed() {
        let push = |commits: &str| -> PushEvent {
            serde_json::from_str(&format!(
                r#"{{"ref":"refs/heads/main","repository":{{"full_name":"acme/mono"}},"commits":{commits}}}"#
            ))
            .unwrap()
        };
        let web_only = push(
            r#"[{"added":["apps/web/page.tsx"],"modified":["README.md"],"removed":[]},
                {"added":[],"modified":["apps/web/package.json"],"removed":["apps/api-old/x"]}]"#,
        );
        assert!(web_only.touches("apps/web"));
        assert!(web_only.touches(""));
        assert!(!web_only.touches("apps/api"));
        assert!(!web_only.touches("app"));
        let removed = push(r#"[{"added":[],"modified":[],"removed":["apps/api/main.go"]}]"#);
        assert!(removed.touches("apps/api"));
        assert!(push("[]").touches("apps/api"));
        assert!(push(r#"[{"id":"abc"}]"#).touches("apps/api"));
        let no_commits: PushEvent = serde_json::from_str(
            r#"{"ref":"refs/heads/main","repository":{"full_name":"acme/mono"}}"#,
        )
        .unwrap();
        assert!(no_commits.touches("apps/api"));
    }
}
