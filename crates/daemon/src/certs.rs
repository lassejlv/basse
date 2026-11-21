//! Certificates for services that run on several servers.
//!
//! Every server's proxy gets its own certificate for a domain, and with several servers behind
//! one name Let's Encrypt's challenge reaches whichever server DNS or a load balancer picks.
//! The routes relay challenges to one server (see [`crate::replicas::apply_route`]), so that one
//! obtains the certificate; this task then copies the newest certificate of each domain into
//! the other servers' proxy storage, where their next attempt finds it instead of asking Let's
//! Encrypt again.

use crate::docker;
use crate::replicas;
use crate::state::AppState;
use anyhow::{Context as _, Result};
use bollard::Docker;
use bollard::query_parameters::{DownloadFromContainerOptions, UploadToContainerOptions};
use common::brand;
use futures_util::StreamExt as _;
use std::io::Read as _;
use std::sync::Arc;
use std::time::Duration;

/// How often certificates are compared between servers.
const EVERY: Duration = Duration::from_secs(600);
/// Where the proxy keeps certificates, per issuer and domain.
const STORE: &str = "/data/caddy/certificates";

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(EVERY).await;
            if let Err(e) = round(&state).await {
                tracing::warn!("certificate sync failed: {e:#}");
            }
        }
    });
}

async fn round(state: &AppState) -> Result<()> {
    for rec in state.db.services().await? {
        let mut servers: Vec<&str> = rec.replicas.iter().map(|r| r.server_id.as_str()).collect();
        servers.sort_unstable();
        servers.dedup();
        if servers.len() < 2 {
            continue;
        }
        let hosts: Vec<String> = state
            .db
            .domains(&rec.service.id)
            .await?
            .into_iter()
            .filter(|d| d.tls)
            .map(|d| d.host.to_ascii_lowercase())
            .collect();
        if hosts.is_empty() {
            continue;
        }
        let fleet = replicas::Fleet::connect(state, servers.iter().copied()).await;
        for host in &hosts {
            if let Err(e) = share(&fleet, &servers, host).await {
                tracing::warn!("cannot share the certificate of {host}: {e:#}");
            }
        }
    }
    Ok(())
}

/// One domain's certificate files on a server, with the certificate's modification time.
struct Stored {
    mtime: u64,
    files: Vec<(String, Vec<u8>)>,
}

async fn share(fleet: &replicas::Fleet, servers: &[&str], host: &str) -> Result<()> {
    let mut found: Vec<(&str, Option<Stored>)> = Vec::new();
    for server_id in servers {
        let Some(docker) = fleet.docker(server_id) else {
            continue;
        };
        found.push((server_id, read(docker, host).await?));
    }
    let Some(newest) = found
        .iter()
        .filter_map(|(_, s)| s.as_ref())
        .max_by_key(|s| s.mtime)
    else {
        return Ok(());
    };
    for (server_id, stored) in &found {
        if stored.as_ref().is_some_and(|s| s.mtime >= newest.mtime) {
            continue;
        }
        let Some(docker) = fleet.docker(server_id) else {
            continue;
        };
        write(docker, &newest.files).await?;
        tracing::info!(
            "copied the certificate of {host} to {}",
            fleet.name(server_id)
        );
    }
    Ok(())
}

/// The files of `host` under every issuer in the proxy's store, paths relative to its parent.
async fn read(docker: &Docker, host: &str) -> Result<Option<Stored>> {
    let mut stream = docker.download_from_container(
        brand::PROXY_CONTAINER,
        Some(DownloadFromContainerOptions {
            path: STORE.to_string(),
        }),
    );
    let mut tar = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => tar.extend_from_slice(&bytes),
            Err(e) if docker::status_code(&e) == Some(404) => return Ok(None),
            Err(e) => return Err(e).context("cannot read the proxy's certificates"),
        }
    }
    pick(&tar, host)
}

/// The entries of a `certificates/<issuer>/<host>/…` archive that belong to `host`.
fn pick(tar: &[u8], host: &str) -> Result<Option<Stored>> {
    let mut archive = tar::Archive::new(tar);
    let mut files = Vec::new();
    let mut mtime = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != 4 || parts[0] != "certificates" || parts[2] != host {
            continue;
        }
        if parts[3].ends_with(".crt") {
            mtime = Some(entry.header().mtime()?.max(mtime.unwrap_or(0)));
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        files.push((path, bytes));
    }
    Ok(mtime.map(|mtime| Stored { mtime, files }))
}

async fn write(docker: &Docker, files: &[(String, Vec<u8>)]) -> Result<()> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o600);
        header.set_mtime(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        );
        header.set_cksum();
        builder.append_data(&mut header, path, bytes.as_slice())?;
    }
    let tar = builder.into_inner()?;
    docker
        .upload_to_container(
            brand::PROXY_CONTAINER,
            Some(UploadToContainerOptions {
                path: "/data/caddy".into(),
                ..Default::default()
            }),
            bollard::body_full(tar.into()),
        )
        .await
        .context("cannot write the certificate into the proxy")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(files: &[(&str, &str, u64)]) -> Vec<u8> {
        let mut b = tar::Builder::new(Vec::new());
        for (path, body, mtime) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(body.len() as u64);
            h.set_mtime(*mtime);
            h.set_mode(0o600);
            h.set_cksum();
            b.append_data(&mut h, path, body.as_bytes()).unwrap();
        }
        b.into_inner().unwrap()
    }

    #[test]
    fn only_the_domains_files_are_picked_with_the_certificates_time() {
        let tar = archive(&[
            ("certificates/acme-v02/a.com/a.com.crt", "cert", 200),
            ("certificates/acme-v02/a.com/a.com.key", "key", 100),
            ("certificates/acme-v02/b.com/b.com.crt", "other", 900),
            ("certificates/acme-v02/sub.a.com/sub.a.com.crt", "sub", 900),
        ]);
        let stored = pick(&tar, "a.com").unwrap().unwrap();
        assert_eq!(stored.mtime, 200);
        let names: Vec<&str> = stored.files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            names,
            [
                "certificates/acme-v02/a.com/a.com.crt",
                "certificates/acme-v02/a.com/a.com.key"
            ]
        );
        assert!(pick(&tar, "c.com").unwrap().is_none());
    }
}
