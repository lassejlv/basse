//! Git sources against a mock of GitHub's API: the App manifest flow, the install callback,
//! repositories and branches, push webhooks, and personal access tokens. Nothing here talks to
//! the real GitHub; the mock checks the app JWT with the key the manifest flow handed out.

mod support;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use common::api::*;
use common::*;
use hmac::{Hmac, Mac as _};
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use support::{Daemon, Engine};

const DOMAIN: &str = "deploy.example.test";
const APP_ID: u64 = 4242;
const INSTALLATION: u64 = 77;
const INSTALL_TOKEN: &str = "ghs_installation";
const PAT: &str = "ghp_personal";
const WEBHOOK_SECRET: &str = "whsec_test";

struct Keys {
    private: String,
    public: String,
}

/// A fresh 2048-bit RSA key pair, as GitHub hands out for an app.
fn keys() -> Keys {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("app.pem");
    let pubkey = dir.path().join("app.pub");
    let ok = Command::new("openssl")
        .args([
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
            "-out",
        ])
        .arg(&key)
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
        && Command::new("openssl")
            .args(["pkey", "-pubout", "-in"])
            .arg(&key)
            .arg("-out")
            .arg(&pubkey)
            .status()
            .unwrap()
            .success();
    assert!(ok, "openssl could not make a key");
    Keys {
        private: std::fs::read_to_string(key).unwrap(),
        public: std::fs::read_to_string(pubkey).unwrap(),
    }
}

struct Mock {
    keys: Keys,
}

fn bearer(headers: &HeaderMap) -> String {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default()
        .to_string()
}

fn unauthorized() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({"message": "Bad credentials"})),
    )
        .into_response()
}

/// Whether the request carries a JWT signed with the app's key for the app id.
fn app_authorized(mock: &Mock, headers: &HeaderMap) -> bool {
    let key = jsonwebtoken::DecodingKey::from_rsa_pem(mock.keys.public.as_bytes()).unwrap();
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_issuer(&[APP_ID.to_string()]);
    jsonwebtoken::decode::<Value>(&bearer(headers), &key, &validation).is_ok()
}

fn acme() -> Value {
    json!({"login": "acme", "type": "Organization"})
}

fn repos() -> Value {
    json!([
        {"full_name": "acme/api", "private": true, "default_branch": "main", "pushed_at": "2026-11-21T10:00:00Z"},
        {"full_name": "acme/web", "private": false, "default_branch": "trunk", "pushed_at": "2026-11-20T10:00:00Z"},
        {"full_name": "acme/api-docs", "private": false, "default_branch": "main", "pushed_at": "2026-11-21T11:00:00Z"},
    ])
}

async fn conversions(
    State(m): State<Arc<Mock>>,
    Path(code): Path<String>,
) -> axum::response::Response {
    if code != "good-code" {
        return (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response();
    }
    Json(json!({
        "id": APP_ID, "slug": "acme-deploys", "owner": acme(), "pem": m.keys.private,
        "webhook_secret": WEBHOOK_SECRET, "client_id": "Iv1.abc", "client_secret": "csecret",
    }))
    .into_response()
}

async fn app(State(m): State<Arc<Mock>>, headers: HeaderMap) -> axum::response::Response {
    if !app_authorized(&m, &headers) {
        return unauthorized();
    }
    Json(json!({"id": APP_ID, "slug": "acme-deploys", "owner": acme()})).into_response()
}

async fn installation(
    State(m): State<Arc<Mock>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !app_authorized(&m, &headers) {
        return unauthorized();
    }
    if id != INSTALLATION {
        return (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response();
    }
    Json(json!({"id": INSTALLATION, "account": acme()})).into_response()
}

async fn installations(State(m): State<Arc<Mock>>, headers: HeaderMap) -> axum::response::Response {
    if !app_authorized(&m, &headers) {
        return unauthorized();
    }
    Json(json!([{"id": INSTALLATION, "account": acme()}])).into_response()
}

async fn access_token(
    State(m): State<Arc<Mock>>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !app_authorized(&m, &headers) || id != INSTALLATION {
        return unauthorized();
    }
    (StatusCode::CREATED, Json(json!({"token": INSTALL_TOKEN}))).into_response()
}

async fn installation_repos(headers: HeaderMap) -> axum::response::Response {
    if bearer(&headers) != INSTALL_TOKEN {
        return unauthorized();
    }
    Json(json!({"total_count": 3, "repositories": repos()})).into_response()
}

async fn user(headers: HeaderMap) -> axum::response::Response {
    if bearer(&headers) != PAT {
        return unauthorized();
    }
    Json(json!({"login": "lasse", "type": "User"})).into_response()
}

async fn user_repos(headers: HeaderMap) -> axum::response::Response {
    if bearer(&headers) != PAT {
        return unauthorized();
    }
    Json(repos()).into_response()
}

fn token_ok(headers: &HeaderMap) -> bool {
    let t = bearer(headers);
    t == INSTALL_TOKEN || t == PAT
}

async fn repo(
    Path((owner, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !token_ok(&headers) {
        return unauthorized();
    }
    Json(json!({"full_name": format!("{owner}/{name}"), "default_branch": "main", "private": true}))
        .into_response()
}

async fn contents(
    Path((_owner, name)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !token_ok(&headers) {
        return unauthorized();
    }
    let files: &[&str] = match (name.as_str(), q.get("ref").map(String::as_str)) {
        ("api", Some("main")) => &["Dockerfile", "compose.yaml", "README.md"],
        ("web", Some("feat/x")) => &["docker-compose.yml", "package.json"],
        _ => &["package.json"],
    };
    let mut entries: Vec<Value> = files
        .iter()
        .map(|f| json!({"name": f, "type": "file"}))
        .collect();
    entries.push(json!({"name": "Dockerfile", "type": "dir"}));
    Json(entries).into_response()
}

/// `acme/mono`: apps/api has a Dockerfile, apps/web a compose file, apps/worker neither.
async fn contents_path(
    Path((_owner, name, path)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !token_ok(&headers) {
        return unauthorized();
    }
    let entry = |name: &str, kind: &str| json!({"name": name, "type": kind});
    let entries = match (name.as_str(), path.as_str()) {
        ("mono", "apps") => vec![
            entry("worker", "dir"),
            entry("api", "dir"),
            entry("web", "dir"),
            entry("README.md", "file"),
        ],
        ("mono", "apps/api") => vec![entry("Dockerfile", "file"), entry("src", "dir")],
        ("mono", "apps/web") => vec![entry("compose.yaml", "file"), entry("Dockerfile", "dir")],
        ("mono", "apps/worker") => vec![entry("package.json", "file")],
        ("mono", "README.md") => {
            return Json(json!({"name": "README.md", "type": "file", "content": ""}))
                .into_response();
        }
        _ => {
            return (StatusCode::NOT_FOUND, Json(json!({"message": "Not Found"}))).into_response();
        }
    };
    Json(entries).into_response()
}

async fn branches(
    Path((_owner, _name)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> axum::response::Response {
    if !token_ok(&headers) {
        return unauthorized();
    }
    let page = q.get("page").map_or("1", String::as_str);
    if page != "1" {
        return Json(json!([])).into_response();
    }
    Json(json!([{"name": "dev"}, {"name": "main"}, {"name": "release"}])).into_response()
}

/// Serves the mock and returns its base URL.
async fn mock_github() -> String {
    let state = Arc::new(Mock { keys: keys() });
    let router = Router::new()
        .route("/app-manifests/{code}/conversions", post(conversions))
        .route("/app", get(app))
        .route("/app/installations", get(installations))
        .route("/app/installations/{id}", get(installation))
        .route("/app/installations/{id}/access_tokens", post(access_token))
        .route("/installation/repositories", get(installation_repos))
        .route("/user", get(user))
        .route("/user/repos", get(user_repos))
        .route("/repos/{owner}/{name}", get(repo))
        .route("/repos/{owner}/{name}/branches", get(branches))
        .route("/repos/{owner}/{name}/contents", get(contents))
        .route("/repos/{owner}/{name}/contents/{*path}", get(contents_path))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    url
}

/// A daemon whose instance domain counts as verified, as after a passing check.
async fn daemon_with_domain() -> Option<Daemon> {
    Daemon::start_with(Engine::None, |dir| {
        let path = dir.join("config.toml");
        let mut config: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        config.insert("domain".into(), DOMAIN.into());
        config.insert(
            "domain_verified_at".into(),
            toml::Value::String("2026-11-21T09:00:00Z".into()),
        );
        std::fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
        Vec::new()
    })
    .await
}

fn sign(body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(WEBHOOK_SECRET.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[tokio::test]
async fn github_apps_need_a_verified_instance_domain() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let api = daemon.client();
    let err = api
        .create_github_app(&CreateGithubApp {
            name: "acme".into(),
            ..Default::default()
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("instance domain"), "{err}");
    assert!(
        err.contains("instance domain set"),
        "points at the guide: {err}"
    );

    let d = api.instance_domain().await.unwrap();
    assert_eq!(d.domain, None);
    assert!(!d.verified);
    assert!(
        api.check_instance_domain().await.is_err(),
        "nothing to check yet"
    );
    assert!(api.set_instance_domain("not a domain").await.is_err());

    let d = api
        .set_instance_domain("Deploy.Example.Invalid")
        .await
        .unwrap();
    assert_eq!(d.domain.as_deref(), Some("deploy.example.invalid"));
    assert!(!d.verified);
    assert_eq!(d.checks.len(), 5);
    assert_eq!(d.checks[0].kind, DomainCheckKind::Dns);
    assert_eq!(d.checks[0].status, CheckStatus::Fail, "{:?}", d.checks[0]);
    assert!(
        d.checks[1..]
            .iter()
            .all(|c| c.status == CheckStatus::Skipped)
    );
    let settings = api.settings().await.unwrap();
    assert_eq!(settings.domain.as_deref(), Some("deploy.example.invalid"));
    assert!(!settings.domain_verified);
    assert_eq!(
        api.info().await.unwrap().url,
        None,
        "unverified domains are not advertised"
    );

    let d = api.remove_instance_domain().await.unwrap();
    assert_eq!(d.domain, None);
    assert!(d.checks.is_empty());
}

#[tokio::test]
async fn manifest_flow_install_repos_and_push_deploys() {
    let Some(daemon) = daemon_with_domain().await else {
        return;
    };
    let gh = mock_github().await;
    let api = daemon.client();
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    assert_eq!(
        api.info().await.unwrap().url.as_deref(),
        Some("https://deploy.example.test")
    );

    let setup = api
        .create_github_app(&CreateGithubApp {
            name: "acme".into(),
            organization: Some("acme".into()),
            html_url: Some(gh.clone()),
            api_url: Some(gh.clone()),
        })
        .await
        .unwrap();
    let id = setup.source.id.clone();
    assert_eq!(setup.source.status, GitSourceStatus::Pending);
    let prefix = format!("https://{DOMAIN}/github/{id}/start?state=");
    let state = setup
        .setup_url
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("unexpected setup url {}", setup.setup_url))
        .to_string();
    let local = |path: &str| format!("{}{path}", daemon.url);

    // The start page posts the manifest to the organization's new-app form.
    let res = http
        .get(local(&format!("/github/{id}/start?state=wrong")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let page = http
        .get(local(&format!("/github/{id}/start?state={state}")))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains(&format!(
        "action=\"{gh}/organizations/acme/settings/apps/new?state={state}\""
    )));
    assert!(page.contains(&format!("https://{DOMAIN}/github/{id}/callback")));
    assert!(page.contains(&format!("https://{DOMAIN}/github/{id}/webhook")));
    assert!(
        page.contains("&quot;push&quot;"),
        "the manifest is escaped JSON"
    );

    // GitHub redirects back with a code; the daemon trades it and sends the browser on.
    let res = http
        .get(local(&format!(
            "/github/{id}/callback?code=good-code&state={state}"
        )))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_redirection(), "{}", res.status());
    assert_eq!(
        res.headers()["location"],
        format!("{gh}/apps/acme-deploys/installations/new").as_str()
    );
    let res = http
        .get(local(&format!(
            "/github/{id}/callback?code=good-code&state={state}"
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "the state is single-use"
    );

    let s = api.source(&id).await.unwrap();
    assert_eq!(s.status, GitSourceStatus::NeedsInstall);
    assert_eq!(
        (s.app_id, s.app_slug.as_deref()),
        (Some(APP_ID), Some("acme-deploys"))
    );
    assert_eq!(s.owner_kind, Some(GitOwnerKind::Organization));
    assert_eq!(
        s.webhook_url.as_deref(),
        Some(format!("https://{DOMAIN}/github/{id}/webhook").as_str())
    );

    // Credentials live in a github_app secret that is never revealed.
    let secrets = api.secrets().await.unwrap();
    let secret = secrets
        .iter()
        .find(|x| Some(&x.id) == s.secret_id.as_ref())
        .unwrap();
    assert_eq!(secret.kind, SecretKind::GithubApp);
    assert_eq!(secret.used_by[0].kind, UsageKind::Source);
    assert!(api.reveal_secret(&secret.id).await.is_err());
    assert!(
        api.delete_secret(&secret.id).await.is_err(),
        "the source uses it"
    );
    let body = serde_json::to_string(&s).unwrap();
    assert!(!body.contains("BEGIN") && !body.contains(WEBHOOK_SECRET) && !body.contains("csecret"));

    // The install callback only accepts installations of this app.
    let res = http
        .get(local(&format!(
            "/github/{id}/setup?installation_id=999&setup_action=install"
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let res = http
        .get(local(&format!(
            "/github/{id}/setup?installation_id={INSTALLATION}&setup_action=install"
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let s = api.check_source(&id).await.unwrap();
    assert_eq!(s.status, GitSourceStatus::Ready, "{:?}", s.status_detail);
    assert_eq!(s.installation_id, Some(INSTALLATION));
    assert_eq!(s.owner.as_deref(), Some("acme"));

    // Repositories: newest push first, searchable, paged.
    let page = api.source_repos(&id, "", 1, 2).await.unwrap();
    assert_eq!((page.total, page.pages()), (3, 2));
    assert_eq!(page.repos[0].full_name, "acme/api-docs");
    let page = api.source_repos(&id, "API", 1, 30).await.unwrap();
    let names: Vec<_> = page.repos.iter().map(|r| r.full_name.as_str()).collect();
    assert_eq!(names, ["acme/api-docs", "acme/api"]);
    let branches = api.source_branches(&id, "acme/api").await.unwrap();
    assert_eq!(branches[0].name, "main");
    assert!(branches[0].default);
    assert_eq!(branches.len(), 3);
    assert!(api.source_branches(&id, "../etc").await.is_err());

    // Build method detection reads the branch's root.
    let d = api.detect_build(&id, "acme/api", "main", "").await.unwrap();
    assert_eq!(
        (d.method, d.file.as_deref()),
        (BuildMethod::Dockerfile, Some("Dockerfile"))
    );
    let d = api
        .detect_build(&id, "acme/web", "feat/x", "")
        .await
        .unwrap();
    assert_eq!(
        (d.method, d.file.as_deref()),
        (BuildMethod::Compose, Some("docker-compose.yml"))
    );
    let d = api.detect_build(&id, "acme/web", "main", "").await.unwrap();
    assert_eq!(
        (d.method, d.file),
        (BuildMethod::Railpack, None),
        "a directory named Dockerfile is not one"
    );

    // Monorepos: directories to pick a root from, and detection inside a root.
    let dirs = api
        .source_dirs(&id, "acme/mono", "main", "apps")
        .await
        .unwrap();
    assert_eq!(dirs, ["api", "web", "worker"]);
    for (root, method, file) in [
        ("apps/api", BuildMethod::Dockerfile, Some("Dockerfile")),
        ("./apps/web/", BuildMethod::Compose, Some("compose.yaml")),
        ("apps/worker", BuildMethod::Railpack, None),
    ] {
        let d = api
            .detect_build(&id, "acme/mono", "main", root)
            .await
            .unwrap();
        assert_eq!((d.method, d.file.as_deref()), (method, file), "{root}");
    }
    let err = api
        .detect_build(&id, "acme/mono", "main", "apps/../..")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("inside the repository"), "{err}");
    let err = api
        .source_dirs(&id, "acme/mono", "main", "README.md")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a directory"), "{err}");
    assert!(
        api.source_dirs(&id, "acme/mono", "main", "nope")
            .await
            .is_err()
    );

    // A service on the source; the source cannot go while it is used.
    let project = api
        .create_project(&CreateProject {
            name: "shop".into(),
            description: None,
        })
        .await
        .unwrap();
    let env = api.tree().await.unwrap().projects[0].environments[0]
        .environment
        .clone();
    assert_eq!(env.project_id, project.id);
    let git = |repo: &str, branch: &str, auto_deploy: bool| Source::Git {
        repo: repo.into(),
        branch: branch.into(),
        dockerfile: "Dockerfile".into(),
        context: ".".into(),
        root: String::new(),
        watch_paths: true,
        source_id: Some(id.clone()),
        auto_deploy,
        build: Build::default(),
    };
    let in_root = |root: &str, watch_paths: bool| {
        let mut source = git("acme/mono", "main", true);
        if let Source::Git {
            root: r,
            watch_paths: w,
            ..
        } = &mut source
        {
            *r = root.into();
            *w = watch_paths;
        }
        source
    };
    let service = |name: &str, source: Source| CreateService {
        environment_id: env.id.clone(),
        server_id: None,
        name: name.into(),
        source,
        port: Some(8080),
        command: None,
        restart: RestartPolicy::UnlessStopped,
        health_path: None,
        env: vec![],
        domains: vec![],
        volumes: vec![],
        deploy: false,
        replicas: None,
    };
    let err = api
        .create_service(&service(
            "bad",
            git("https://example.com/x.git", "main", true),
        ))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("owner/name"), "{err}");
    let api_svc = api
        .create_service(&service("api", git("acme/api", "main", true)))
        .await
        .unwrap();
    let quiet = api
        .create_service(&service("quiet", git("acme/api", "main", false)))
        .await
        .unwrap();
    let err = api.delete_source(&id).await.unwrap_err().to_string();
    assert!(err.contains("service api"), "{err}");
    assert_eq!(api.source(&id).await.unwrap().used_by.len(), 2);

    // Webhooks: signed pushes to the tracked branch deploy services with auto-deploy on.
    let hook = local(&format!("/github/{id}/webhook"));
    let push = |r#ref: &str| {
        serde_json::to_vec(&json!({
            "ref": r#ref, "after": "abc", "deleted": false,
            "repository": {"full_name": "Acme/API"},
        }))
        .unwrap()
    };
    let deliver = |event: &'static str, body: Vec<u8>, signature: String| {
        http.post(&hook)
            .header("x-github-event", event)
            .header("x-hub-signature-256", signature)
            .header("content-type", "application/json")
            .body(body)
            .send()
    };
    let body = push("refs/heads/main");
    let res = deliver("push", body.clone(), "sha256=00".into())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = deliver("ping", b"{}".to_vec(), sign(b"{}")).await.unwrap();
    assert!(res.status().is_success());
    let res = deliver(
        "push",
        push("refs/heads/dev"),
        sign(&push("refs/heads/dev")),
    )
    .await
    .unwrap();
    assert_eq!(res.json::<Value>().await.unwrap()["deployments"], 0);
    let res = deliver("push", body.clone(), sign(&body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    assert_eq!(res.json::<Value>().await.unwrap()["deployments"], 1);
    let deployments = api.deployments(&api_svc.service.id, 10).await.unwrap();
    assert_eq!(deployments.len(), 1);
    assert_eq!(deployments[0].trigger, "webhook");
    assert!(
        api.deployments(&quiet.service.id, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Roots are normalized and kept inside the repository, on create and on update.
    let err = api
        .create_service(&service("escape", in_root("apps/../../etc", true)))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("must stay inside the repository"), "{err}");
    let mono_api = api
        .create_service(&service("mono-api", in_root("./apps/api/", true)))
        .await
        .unwrap();
    let Source::Git { root, .. } = &mono_api.service.source else {
        panic!("not git");
    };
    assert_eq!(root, "apps/api");
    let mono_web = api
        .create_service(&service("mono-web", in_root("apps/web", true)))
        .await
        .unwrap();
    let mono_all = api
        .create_service(&service("mono-all", in_root("apps/worker", false)))
        .await
        .unwrap();
    let err = api
        .update_service(
            &mono_all.service.id,
            &UpdateService {
                source: Some(in_root("..", false)),
                ..Default::default()
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("must stay inside the repository"), "{err}");
    let moved = api
        .update_service(
            &mono_all.service.id,
            &UpdateService {
                source: Some(in_root("/apps/worker/", false)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(&moved.service.source, Source::Git { root, watch_paths: false, .. } if root == "apps/worker")
    );

    // Watch paths: a push deploys a service with a root only when it changed a file inside it.
    let mono_push = |files: Value| {
        serde_json::to_vec(&json!({
            "ref": "refs/heads/main", "deleted": false,
            "repository": {"full_name": "acme/mono"},
            "commits": [{"added": [], "modified": files, "removed": []}],
        }))
        .unwrap()
    };
    let count = |svc: &ServiceDetail| {
        let id = svc.service.id.clone();
        let api = &api;
        async move { api.deployments(&id, 10).await.unwrap().len() }
    };
    let body = mono_push(json!(["apps/web/compose.yaml", "README.md"]));
    let res = deliver("push", body.clone(), sign(&body)).await.unwrap();
    assert_eq!(
        res.json::<Value>().await.unwrap()["deployments"],
        2,
        "mono-web and mono-all, which does not watch paths"
    );
    assert_eq!(count(&mono_api).await, 0);
    assert_eq!(count(&mono_web).await, 1);
    assert_eq!(count(&mono_all).await, 1);

    // Uninstalling the app on GitHub shows up on the source.
    let event = serde_json::to_vec(&json!({
        "action": "deleted", "installation": {"id": INSTALLATION, "account": acme()},
    }))
    .unwrap();
    let res = deliver("installation", event.clone(), sign(&event))
        .await
        .unwrap();
    assert!(res.status().is_success());
    let s = api.source(&id).await.unwrap();
    assert_eq!(s.status, GitSourceStatus::NeedsInstall);
    assert_eq!(s.installation_id, None);

    for svc in [&api_svc, &quiet, &mono_api, &mono_web, &mono_all] {
        api.delete_service(&svc.service.id, false).await.unwrap();
    }
    api.delete_source(&id).await.unwrap();
    assert!(api.sources().await.unwrap().is_empty());
    assert!(
        !api.secrets()
            .await
            .unwrap()
            .iter()
            .any(|x| x.kind == SecretKind::GithubApp),
        "the credentials go with the source"
    );
}

#[tokio::test]
async fn existing_apps_and_tokens_connect_after_github_accepts_them() {
    let Some(daemon) = daemon_with_domain().await else {
        return;
    };
    let gh = mock_github().await;
    let api = daemon.client();

    let err = api
        .connect_github_token(&ConnectGithubToken {
            name: "mine".into(),
            token: "ghp_wrong".into(),
            html_url: Some(gh.clone()),
            api_url: Some(gh.clone()),
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("Bad credentials"), "{err}");
    assert!(
        api.sources().await.unwrap().is_empty(),
        "rejected sources are not kept"
    );
    assert!(api.secrets().await.unwrap().is_empty());

    let token = api
        .connect_github_token(&ConnectGithubToken {
            name: "mine".into(),
            token: PAT.into(),
            html_url: Some(gh.clone()),
            api_url: Some(gh.clone()),
        })
        .await
        .unwrap();
    assert_eq!(token.status, GitSourceStatus::Ready);
    assert_eq!(token.kind, GitSourceKind::GithubToken);
    assert_eq!(token.owner.as_deref(), Some("lasse"));
    assert_eq!(token.webhook_url, None);
    assert_eq!(
        api.source_repos(&token.id, "web", 1, 30)
            .await
            .unwrap()
            .total,
        1
    );

    let err = api
        .connect_github_app(&ConnectGithubApp {
            name: "bad-key".into(),
            app_id: APP_ID,
            private_key: "not a key".into(),
            ..Default::default()
        })
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("RSA"), "{err}");

    // The mock was started with its own key; the same flow hands that key to the daemon here.
    let pem = mock_key_pem(&gh).await;
    let app = api
        .connect_github_app(&ConnectGithubApp {
            name: "existing".into(),
            app_id: APP_ID,
            private_key: pem,
            installation_id: None,
            webhook_secret: Some(WEBHOOK_SECRET.into()),
            html_url: Some(gh.clone()),
            api_url: Some(gh.clone()),
        })
        .await
        .unwrap();
    assert_eq!(
        app.status,
        GitSourceStatus::Ready,
        "{:?}",
        app.status_detail
    );
    assert_eq!(
        app.installation_id,
        Some(INSTALLATION),
        "the only installation is picked"
    );
    assert_eq!(app.app_slug.as_deref(), Some("acme-deploys"));
    assert_eq!(api.sources().await.unwrap().len(), 2);
}

/// The private key the mock signs conversions with, read through the manifest exchange.
async fn mock_key_pem(gh: &str) -> String {
    let v: Value = reqwest::Client::new()
        .post(format!("{gh}/app-manifests/good-code/conversions"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    v["pem"].as_str().unwrap().to_string()
}
