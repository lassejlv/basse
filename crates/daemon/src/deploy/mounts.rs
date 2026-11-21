//! Compose bind mounts on the service's server. A bind mount of the repository can't mount the
//! clone, which lives next to the daemon and only for one deploy, so the daemon turns it into a
//! volume on the server and fills it through the Docker API, which also reaches remote servers
//! over their SSH tunnel. [`ComposeVolume`] has the rule.

use super::Job;
use crate::db::new_id;
use crate::docker;
use anyhow::{Context as _, Result, bail};
use bollard::Docker;
use bollard::query_parameters::UploadToContainerOptions;
use chrono::Utc;
use common::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Component, Path};

/// The volumes and bind mounts of a resolved compose file (`docker compose config`).
#[derive(Debug, Default, PartialEq)]
pub(super) struct MountPlan {
    /// Bind mounts of paths inside the repository.
    pub repo: Vec<RepoBind>,
    /// Bind mounts of absolute host paths, as (compose service, path); they stay as they are.
    pub host: Vec<(String, String)>,
    /// Named volumes compose creates, by Docker volume name, with their mount targets.
    pub named: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, PartialEq)]
pub(super) struct RepoBind {
    pub service: String,
    pub target: String,
    pub read_only: bool,
    /// Relative to the repository root, `/`-separated; empty for the root itself.
    pub path: String,
    /// Copied on every deploy; otherwise a data volume.
    pub config: bool,
}

/// Sorts a resolved compose file's volumes into repository binds, host binds and named
/// volumes. `repo` is the clone; a bind elsewhere under `daemon_root` escaped it with `..`.
pub(super) fn plan(config: &Value, repo: &Path, daemon_root: &Path) -> Result<MountPlan> {
    let repo_real = repo.canonicalize()?;
    let mut out = MountPlan::default();
    let services = config
        .get("services")
        .and_then(Value::as_object)
        .into_iter()
        .flatten();
    for (service, spec) in services {
        let volumes = spec
            .get("volumes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for v in volumes {
            let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default();
            let target = field("target").to_string();
            match field("type") {
                "volume" if !field("source").is_empty() => {
                    let top = &config["volumes"][field("source")];
                    if top["external"].as_bool() == Some(true) {
                        continue;
                    }
                    let name = top["name"].as_str().unwrap_or(field("source"));
                    out.named.entry(name.into()).or_default().push(target);
                }
                "bind" => {
                    let source = Path::new(field("source"));
                    let Some(rel) = [repo, repo_real.as_path()]
                        .iter()
                        .find_map(|base| source.strip_prefix(base).ok())
                    else {
                        if source.starts_with(daemon_root) {
                            bail!(
                                "compose service {service} mounts {}, which is outside the repository",
                                source.display()
                            );
                        }
                        out.host
                            .push((service.clone(), source.to_string_lossy().into_owned()));
                        continue;
                    };
                    let path = relative(rel)?;
                    let real = repo.join(&path);
                    if real.exists() && !real.canonicalize()?.starts_with(&repo_real) {
                        bail!(
                            "compose service {service} mounts ./{path}, which links outside the repository"
                        );
                    }
                    let read_only = v["read_only"].as_bool().unwrap_or(false);
                    out.repo.push(RepoBind {
                        service: service.clone(),
                        target,
                        read_only,
                        config: real.is_file() || (real.is_dir() && read_only),
                        path,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

fn relative(rel: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => bail!(
                "bind mount {} must stay inside the repository",
                rel.display()
            ),
        }
    }
    Ok(parts.join("/"))
}

/// A volume name for a data path: `storage/uploads` is `storage-uploads`, the root is `repo`.
fn data_name(path: &str) -> String {
    let mut name = String::new();
    for c in path.chars().map(|c| c.to_ascii_lowercase()) {
        if c.is_ascii_lowercase() || (c.is_ascii_digit() && !name.is_empty()) {
            name.push(c);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    name.truncate(40);
    match name.trim_end_matches('-') {
        "" => "repo".into(),
        n => n.to_string(),
    }
}

/// `name`, or `name-2`, `name-3`… when another volume of the service has it.
fn free_name(name: &str, taken: &[Volume]) -> String {
    let free = |n: &str| !taken.iter().any(|v| v.name == n);
    if free(name) {
        return name.into();
    }
    (2..)
        .map(|i| format!("{name}-{i}"))
        .find(|n| free(n))
        .expect("an unused name")
}

fn targets(list: impl IntoIterator<Item = String>) -> String {
    let mut list: Vec<String> = list.into_iter().collect();
    list.sort();
    list.dedup();
    list.join(", ")
}

/// Paths that no other path in the list contains.
fn outermost(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = paths
        .iter()
        .filter(|p| {
            !paths
                .iter()
                .any(|o| o != *p && (o.is_empty() || p.starts_with(&format!("{o}/"))))
        })
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A tar of `items`, each a repository path stored under an archive name. Symlinks inside are
/// stored as links, never followed out of the repository.
fn archive(repo: &Path, items: &[(String, String)]) -> Result<Vec<u8>> {
    let mut tar = tar::Builder::new(Vec::new());
    tar.follow_symlinks(false);
    for (path, name) in items {
        let real = repo.join(path);
        if real.is_dir() {
            tar.append_dir_all(name, &real)
        } else {
            tar.append_path_with_name(&real, name)
        }
        .with_context(|| format!("cannot pack ./{path}"))?;
    }
    Ok(tar.into_inner()?)
}

/// This deploy's config files: the files volume, its path on the server, and the files.
struct ConfigCopy {
    volume: String,
    mountpoint: String,
    tar: Vec<u8>,
}

impl Job {
    /// Turns the compose file's volumes into volumes of the service: rows for its Volumes tab,
    /// Docker volumes on its server, and entries in `overrides` that mount them instead of the
    /// clone. Config files of this deploy go to `<files volume>/<deploy number>/…`, and copies no
    /// container mounts any more are removed.
    pub(super) async fn compose_mounts(
        &mut self,
        docker: &Docker,
        server: &Server,
        svc: &Service,
        repo: &Path,
        plan: &MountPlan,
        overrides: &mut Value,
    ) -> Result<()> {
        for (service, path) in &plan.host {
            self.log.system(format!(
                "Warning: {service} mounts {path} on {} as it is; {} does not copy, keep or remove it",
                server.name,
                brand::NAME
            ));
        }
        let mut rows = self.state.db.volumes(&svc.id).await?;

        for (docker_name, mounted) in &plan.named {
            let compose = ComposeVolume::Named {
                docker_name: docker_name.clone(),
            };
            let name = data_name(docker_name.rsplit('_').next().unwrap_or(docker_name));
            self.keep_row(svc, &mut rows, compose, &name, targets(mounted.clone()))
                .await?;
        }

        let mut service_volumes: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
        let mut external = serde_json::Map::new();
        let mut seeds: Vec<(String, String)> = Vec::new();
        let mut data_paths: Vec<&str> = plan
            .repo
            .iter()
            .filter(|b| !b.config)
            .map(|b| b.path.as_str())
            .collect();
        data_paths.sort();
        data_paths.dedup();
        for path in data_paths {
            let binds: Vec<&RepoBind> = plan.repo.iter().filter(|b| b.path == path).collect();
            let row = self
                .keep_row(
                    svc,
                    &mut rows,
                    ComposeVolume::Data { path: path.into() },
                    &data_name(path),
                    targets(binds.iter().map(|b| b.target.clone())),
                )
                .await?;
            let volume = row.docker_name();
            let created = docker::create_volume_if_missing(docker, &volume).await?;
            let real = repo.join(path);
            let seeded = created && real.is_dir() && std::fs::read_dir(&real)?.next().is_some();
            if seeded {
                seeds.push((path.into(), volume.clone()));
            }
            self.log.system(format!(
                "Mounting ./{path} from volume {volume}{}",
                if seeded {
                    ", filled from the repository"
                } else if created {
                    ", new and empty"
                } else {
                    ", kept from earlier deploys"
                }
            ));
            external.insert(volume.clone(), json!({ "name": volume, "external": true }));
            for b in binds {
                service_volumes.entry(&b.service).or_default().push(json!({
                    "type": "volume",
                    "source": volume,
                    "target": b.target,
                    "read_only": b.read_only,
                }));
            }
        }

        let config: Vec<&RepoBind> = plan.repo.iter().filter(|b| b.config).collect();
        let copy = if config.is_empty() {
            None
        } else {
            let paths = outermost(&config.iter().map(|b| b.path.clone()).collect::<Vec<_>>());
            let row = self
                .keep_row(
                    svc,
                    &mut rows,
                    ComposeVolume::Files {
                        paths: paths.clone(),
                    },
                    "files",
                    targets(config.iter().map(|b| b.target.clone())),
                )
                .await?;
            let volume = row.docker_name();
            docker::create_volume_if_missing(docker, &volume).await?;
            let mountpoint = docker::volume_mountpoint(docker, &volume).await?;
            let number = self.d.number.to_string();
            for b in &config {
                let source = Path::new(&mountpoint).join(&number).join(&b.path);
                service_volumes.entry(&b.service).or_default().push(json!({
                    "type": "bind",
                    "source": source.to_string_lossy(),
                    "target": b.target,
                    "read_only": b.read_only,
                }));
            }
            let items: Vec<(String, String)> = paths
                .iter()
                .map(|p| (p.clone(), format!("{number}/{p}")))
                .collect();
            self.log.system(format!(
                "Copying {} from the repository into volume {volume}",
                paths
                    .iter()
                    .map(|p| format!("./{p}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            Some(ConfigCopy {
                volume,
                mountpoint,
                tar: archive(repo, &items)?,
            })
        };

        if copy.is_some() || !seeds.is_empty() {
            self.fill_volumes(docker, svc, repo, copy, &seeds).await?;
        }
        if let Some(services) = overrides["services"].as_object_mut() {
            for (name, list) in service_volumes {
                if let Some(entry) = services.get_mut(name) {
                    entry["volumes"] = Value::Array(list);
                }
            }
        }
        if !external.is_empty() {
            overrides["volumes"] = Value::Object(external);
        }
        Ok(())
    }

    /// The service's row for `compose`, made under a free `name` when it has none yet, with
    /// its mount targets brought up to date.
    async fn keep_row(
        &self,
        svc: &Service,
        rows: &mut Vec<Volume>,
        compose: ComposeVolume,
        name: &str,
        mount_path: String,
    ) -> Result<Volume> {
        let same = |v: &Volume| match (&v.compose, &compose) {
            (Some(ComposeVolume::Files { .. }), ComposeVolume::Files { .. }) => true,
            (Some(a), b) => a == b,
            (None, _) => false,
        };
        if let Some(row) = rows.iter_mut().find(|v| same(v)) {
            if row.mount_path != mount_path || row.compose.as_ref() != Some(&compose) {
                row.mount_path = mount_path;
                row.compose = Some(compose);
                self.state.db.update_volume(row).await?;
            }
            return Ok(row.clone());
        }
        let row = Volume {
            id: new_id(),
            service_id: svc.id.clone(),
            name: free_name(name, rows),
            mount_path,
            created_at: Utc::now(),
            compose: Some(compose),
        };
        self.state.db.insert_volume(&row).await?;
        rows.push(row.clone());
        Ok(row)
    }

    /// Unpacks this deploy's config files, and the repository's content into new data volumes,
    /// in a helper container on the server. Then removes the copies of earlier deploys that no
    /// container mounts.
    async fn fill_volumes(
        &self,
        docker: &Docker,
        svc: &Service,
        repo: &Path,
        copy: Option<ConfigCopy>,
        seeds: &[(String, String)],
    ) -> Result<()> {
        if !docker::image_exists(docker, docker::HELPER_IMAGE).await? {
            let log = self.log.clone();
            docker::pull_image(docker, docker::HELPER_IMAGE, |l| log.system(l)).await?;
        }
        let mut binds: Vec<String> = copy
            .iter()
            .map(|c| format!("{}:/files", c.volume))
            .collect();
        binds.extend(seeds.iter().map(|(_, v)| format!("{v}:/seed/{v}")));
        let name = format!("{}-mounts-{}", brand::PREFIX, svc.id);
        let helper = docker::start_helper(docker, &name, docker::HELPER_IMAGE, binds, None).await?;
        let upload = async |path: String, tar: Vec<u8>| {
            docker
                .upload_to_container(
                    &helper,
                    Some(UploadToContainerOptions {
                        path,
                        ..Default::default()
                    }),
                    bollard::body_full(tar.into()),
                )
                .await
                .context("cannot copy files to the server")
        };
        let result = async {
            for (path, volume) in seeds {
                let tar = archive(repo, &[(path.clone(), ".".into())])?;
                upload(format!("/seed/{volume}"), tar).await?;
            }
            let Some(copy) = copy else {
                return Ok(());
            };
            upload("/files".into(), copy.tar).await?;
            let mut keep = vec![self.d.number.to_string()];
            let label = format!(
                "com.docker.compose.project={}",
                crate::build::compose_project(&svc.id)
            );
            for c in docker::list_by_label(docker, &label).await? {
                for m in c.mounts.unwrap_or_default() {
                    let used = m
                        .source
                        .as_deref()
                        .and_then(|s| Path::new(s).strip_prefix(&copy.mountpoint).ok())
                        .and_then(|rest| rest.components().next())
                        .map(|c| c.as_os_str().to_string_lossy().into_owned());
                    keep.extend(used);
                }
            }
            let mut cmd: Vec<String> = ["find", "/files", "-mindepth", "1", "-maxdepth", "1"]
                .map(String::from)
                .to_vec();
            for k in keep {
                cmd.extend(["!".into(), "-name".into(), k]);
            }
            cmd.extend(["-exec", "rm", "-rf", "{}", "+"].map(String::from));
            if docker::exec(docker, &helper, cmd).await? != 0 {
                self.log
                    .system("Warning: could not remove the config files of earlier deploys");
            }
            anyhow::Ok(())
        }
        .await;
        if let Err(e) = docker::remove_container(docker, &helper).await {
            tracing::warn!("cannot remove helper container {name}: {e:#}");
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn config(volumes: Value) -> Value {
        json!({
            "services": { "app": { "volumes": volumes } },
            "volumes": {
                "cache": { "name": "proj_cache" },
                "shared": { "name": "shared", "external": true }
            },
        })
    }

    #[test]
    fn binds_sort_into_config_data_host_and_named() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("builds/d1");
        std::fs::create_dir_all(repo.join("config/nginx")).unwrap();
        std::fs::create_dir_all(repo.join("data")).unwrap();
        std::fs::write(repo.join("config/app.conf"), "x").unwrap();
        let bind = |source: PathBuf, target: &str, ro: bool| json!({ "type": "bind", "source": source, "target": target, "read_only": ro });
        let cfg = config(json!([
            bind(repo.join("config/app.conf"), "/etc/app.conf", false),
            bind(repo.join("config/nginx"), "/etc/nginx", true),
            bind(repo.join("data"), "/data", false),
            bind(repo.join("uploads"), "/uploads", true),
            bind("/var/run/docker.sock".into(), "/var/run/docker.sock", false),
            { "type": "volume", "source": "cache", "target": "/cache" },
            { "type": "volume", "source": "shared", "target": "/shared" },
            { "type": "volume", "target": "/anon" },
            { "type": "tmpfs", "target": "/tmp" },
        ]));
        let plan = plan(&cfg, &repo, root.path()).unwrap();
        let kinds: Vec<(&str, bool)> = plan
            .repo
            .iter()
            .map(|b| (b.path.as_str(), b.config))
            .collect();
        assert_eq!(
            kinds,
            [
                ("config/app.conf", true),
                ("config/nginx", true),
                ("data", false),
                ("uploads", false)
            ]
        );
        assert_eq!(
            plan.host,
            [("app".to_string(), "/var/run/docker.sock".to_string())]
        );
        assert_eq!(
            plan.named,
            BTreeMap::from([("proj_cache".to_string(), vec!["/cache".to_string()])])
        );
    }

    #[test]
    fn binds_must_stay_inside_the_repository() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("builds/d1");
        std::fs::create_dir_all(&repo).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.join("escape")).unwrap();
        let bind = |source: PathBuf| json!([{ "type": "bind", "source": source, "target": "/x" }]);
        let escaped = config(bind(root.path().join("builds/other")));
        assert!(plan(&escaped, &repo, root.path()).is_err());
        let linked = config(bind(repo.join("escape")));
        assert!(plan(&linked, &repo, root.path()).is_err());
    }

    #[test]
    fn data_names_are_volume_names() {
        assert_eq!(data_name("data"), "data");
        assert_eq!(data_name("storage/Uploads"), "storage-uploads");
        assert_eq!(data_name(""), "repo");
        assert_eq!(data_name("2024_dumps/"), "dumps");
        for name in ["data", "storage-uploads", "repo", "dumps"] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
        let taken = |names: &[&str]| -> Vec<Volume> {
            names
                .iter()
                .map(|n| Volume {
                    id: new_id(),
                    service_id: "s".into(),
                    name: (*n).into(),
                    mount_path: "/x".into(),
                    created_at: Utc::now(),
                    compose: None,
                })
                .collect()
        };
        assert_eq!(free_name("data", &taken(&["files"])), "data");
        assert_eq!(free_name("data", &taken(&["data", "data-2"])), "data-3");
    }

    #[test]
    fn nested_config_paths_are_copied_once() {
        let paths = ["config".into(), "config/a.conf".into(), "other.conf".into()];
        assert_eq!(outermost(&paths), ["config", "other.conf"]);
        assert_eq!(outermost(&["".into(), "a".into()]), [""]);
    }
}
