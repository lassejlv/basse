//! Build methods for git services: detecting one from a repository, Railpack's binary, and the
//! compose override that puts a compose project under the daemon's labels and proxy.

use crate::config::DataDir;
use anyhow::{Context as _, Result, bail};
use common::*;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::sync::Mutex;

/// Compose file names Docker Compose itself looks for, in its order.
pub const COMPOSE_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// The Railpack release the daemon installs, and the frontend image matching it.
pub const RAILPACK_VERSION: &str = "v0.43.0";

pub fn railpack_frontend() -> String {
    format!("ghcr.io/railwayapp/railpack-frontend:{RAILPACK_VERSION}")
}

/// Picks the build method from the files a repository has: its Dockerfile, else a compose
/// file, else Railpack. `has` answers for paths relative to the repository root.
pub fn detect(
    has: impl Fn(&str) -> bool,
    dockerfile_path: &str,
    compose_file: Option<&str>,
) -> BuildDetection {
    if has(dockerfile_path) {
        return BuildDetection {
            method: BuildMethod::Dockerfile,
            file: Some(dockerfile_path.to_string()),
        };
    }
    let compose = match compose_file {
        Some(f) => has(f).then(|| f.to_string()),
        None => COMPOSE_FILES
            .iter()
            .find(|f| has(f))
            .map(|f| (*f).to_string()),
    };
    match compose {
        Some(file) => BuildDetection {
            method: BuildMethod::Compose,
            file: Some(file),
        },
        None => BuildDetection {
            method: BuildMethod::Railpack,
            file: None,
        },
    }
}

/// `context/dockerfile` relative to the repository root, without `./` segments.
pub fn dockerfile_path(context: &str, dockerfile: &str) -> String {
    Path::new(context)
        .join(dockerfile)
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Docker Compose project name of a service.
pub fn compose_project(service_id: &str) -> String {
    format!("{}-{service_id}", brand::PREFIX)
}

/// An override file, in JSON (which Compose reads as YAML), that labels every compose service
/// as the daemon's and routes domains to the compose services they name.
pub fn compose_override(
    service: &Service,
    compose_services: &[String],
    domains: &[Domain],
) -> Result<Value> {
    let mut services = Map::new();
    let mut routed = false;
    for name in compose_services {
        let mut labels = BTreeMap::from([
            (brand::label("managed"), "true".to_string()),
            (brand::label("service"), service.id.clone()),
            (brand::label("compose-service"), name.clone()),
        ]);
        let mine: Vec<&Domain> = domains
            .iter()
            .filter(|d| d.compose_service.as_deref() == Some(name.as_str()))
            .collect();
        for (i, d) in mine.iter().enumerate() {
            let port = d.port.or(service.port).with_context(|| {
                format!(
                    "domain {} needs a port: set one on the domain or the service",
                    d.host
                )
            })?;
            let site = if d.tls {
                d.host.clone()
            } else {
                format!("http://{}", d.host)
            };
            labels.insert(format!("caddy_{i}"), site);
            labels.insert(
                format!("caddy_{i}.reverse_proxy"),
                format!("{{{{upstreams {port}}}}}"),
            );
        }
        let mut entry = json!({ "labels": labels });
        if !mine.is_empty() {
            routed = true;
            entry["networks"] = json!(["default", brand::PROXY_NETWORK]);
        }
        services.insert(name.clone(), entry);
    }
    for d in domains {
        let Some(name) = &d.compose_service else {
            bail!(
                "domain {} does not say which compose service it routes to",
                d.host
            );
        };
        if !compose_services.contains(name) {
            bail!(
                "domain {} routes to compose service '{name}', which the compose file does not have",
                d.host
            );
        }
    }
    let mut out = json!({ "services": services });
    if routed {
        out["networks"] = json!({ brand::PROXY_NETWORK: { "external": true } });
    }
    Ok(out)
}

static RAILPACK_INSTALL: Mutex<()> = Mutex::const_new(());

fn railpack_target() -> Result<&'static str> {
    Ok(match std::env::consts::ARCH {
        "x86_64" => "x86_64-unknown-linux-musl",
        "aarch64" => "arm64-unknown-linux-musl",
        other => bail!("Railpack has no build for {other}"),
    })
}

/// The Railpack binary, downloaded and checked against the release's checksums into
/// `<data dir>/bin` the first time a service needs it.
pub async fn ensure_railpack(dirs: &DataDir, log: impl Fn(String)) -> Result<PathBuf> {
    let bin = dirs
        .root()
        .join("bin")
        .join(format!("railpack-{RAILPACK_VERSION}"));
    let _installing = RAILPACK_INSTALL.lock().await;
    if bin.is_file() {
        return Ok(bin);
    }
    let asset = format!("railpack-{RAILPACK_VERSION}-{}.tar.gz", railpack_target()?);
    let base =
        format!("https://github.com/railwayapp/railpack/releases/download/{RAILPACK_VERSION}");
    log(format!("Installing Railpack {RAILPACK_VERSION}"));
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let get = |url: String| {
        let http = http.clone();
        async move {
            let res = http
                .get(&url)
                .send()
                .await
                .with_context(|| format!("cannot download {url}"))?;
            if !res.status().is_success() {
                bail!("{url} returned {}", res.status());
            }
            Ok::<_, anyhow::Error>(res.bytes().await?)
        }
    };
    let sums = get(format!("{base}/checksums.txt")).await?;
    let archive = get(format!("{base}/{asset}")).await?;
    let expected = String::from_utf8_lossy(&sums)
        .lines()
        .find_map(|l| {
            let (sum, name) = l.split_once(char::is_whitespace)?;
            (name.trim().trim_start_matches('*') == asset).then(|| sum.to_ascii_lowercase())
        })
        .with_context(|| format!("checksums.txt does not list {asset}"))?;
    let actual = hex::encode(Sha256::digest(&archive));
    if actual != expected {
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}");
    }
    let dir = bin.parent().context("no bin directory")?.to_path_buf();
    let target = bin.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&dir)?;
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(&archive[..]));
        for entry in tar.entries()? {
            let mut entry = entry?;
            let is_binary = entry.path()?.file_name().is_some_and(|n| n == "railpack");
            if is_binary {
                let partial = target.with_extension("partial");
                entry.unpack(&partial)?;
                crate::config::set_mode(&partial, 0o755)?;
                std::fs::rename(&partial, &target)?;
                return Ok(());
            }
        }
        bail!("the Railpack archive has no railpack binary")
    })
    .await??;
    log(format!("Installed Railpack at {}", bin.display()));
    Ok(bin)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn detection_prefers_dockerfile_then_compose_then_railpack() {
        let files = |names: &'static [&'static str]| move |p: &str| names.contains(&p);
        let d = detect(files(&["Dockerfile", "compose.yaml"]), "Dockerfile", None);
        assert_eq!(d.method, BuildMethod::Dockerfile);
        let d = detect(
            files(&["docker-compose.yml", "package.json"]),
            "Dockerfile",
            None,
        );
        assert_eq!(
            (d.method, d.file.as_deref()),
            (BuildMethod::Compose, Some("docker-compose.yml"))
        );
        let d = detect(
            files(&["deploy/stack.yml"]),
            "Dockerfile",
            Some("deploy/stack.yml"),
        );
        assert_eq!(d.method, BuildMethod::Compose);
        let d = detect(files(&["package.json"]), "Dockerfile", None);
        assert_eq!((d.method, d.file), (BuildMethod::Railpack, None));
        let d = detect(
            files(&["api/Dockerfile"]),
            &dockerfile_path("./api", "Dockerfile"),
            None,
        );
        assert_eq!(d.method, BuildMethod::Dockerfile);
        assert_eq!(dockerfile_path(".", "Dockerfile"), "Dockerfile");
    }

    fn service() -> Service {
        Service {
            id: "svc1".into(),
            environment_id: "e".into(),
            server_id: "s".into(),
            name: "shop".into(),
            source: Source::Image { image: "x".into() },
            port: Some(3000),
            command: None,
            restart: RestartPolicy::default(),
            health_path: None,
            created_at: Utc::now(),
            state: ServiceState::New,
            primary_domain: None,
            last_deployed_at: None,
            live_deployment_id: None,
            replicas: 1,
            scaling: Default::default(),
        }
    }

    fn domain(host: &str, svc: Option<&str>, port: Option<u16>) -> Domain {
        Domain {
            id: "d".into(),
            service_id: "svc1".into(),
            host: host.into(),
            port,
            tls: true,
            compose_service: svc.map(Into::into),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn overrides_label_every_service_and_route_domains() {
        let names = vec!["web".to_string(), "worker".to_string()];
        let o = compose_override(
            &service(),
            &names,
            &[domain("shop.example.com", Some("web"), Some(8080))],
        )
        .unwrap();
        let web = &o["services"]["web"];
        assert_eq!(web["labels"][brand::label("service")], "svc1");
        assert_eq!(web["labels"]["caddy_0"], "shop.example.com");
        assert_eq!(web["labels"]["caddy_0.reverse_proxy"], "{{upstreams 8080}}");
        assert_eq!(web["networks"][1], brand::PROXY_NETWORK);
        let worker = &o["services"]["worker"];
        assert_eq!(worker["labels"][brand::label("managed")], "true");
        assert!(worker.get("networks").is_none());
        assert_eq!(o["networks"][brand::PROXY_NETWORK]["external"], true);

        let o = compose_override(&service(), &names, &[]).unwrap();
        assert!(o.get("networks").is_none());
        assert!(
            compose_override(&service(), &names, &[domain("a.com", Some("db"), None)]).is_err()
        );
        assert!(compose_override(&service(), &names, &[domain("a.com", None, None)]).is_err());
    }
}
