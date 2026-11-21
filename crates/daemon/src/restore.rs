//! `<daemon> restore`: disaster recovery from an instance backup on a new machine.
//!
//! Runs on a freshly installed server before the daemon serves anything: it decrypts the
//! archive, stops the service, puts back `config.toml`, `master.key` and `ssh/`, replaces the
//! store with the dump, starts the service again and checks every server.

use crate::backup_store::BackupStore;
use crate::config::{Config, DataDir, set_mode};
use crate::db::{Db, new_id};
use crate::instance::{self, Manifest};
use crate::secrets::{MasterKey, Secrets, write_private};
use crate::state::AppState;
use crate::store;
use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use common::{ServerStatus, StorageConnection, brand};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub struct Options {
    /// A local file or `s3://<bucket>/<key>`.
    pub source: String,
    pub passphrase_file: PathBuf,
    pub s3_endpoint: String,
    pub s3_region: String,
    pub s3_path_style: bool,
    /// Keep the restored store here instead of where the backed-up config says.
    pub database_url: Option<String>,
    /// Leave the systemd service alone.
    pub no_service: bool,
}

pub async fn run(dirs: &DataDir, opts: Options) -> Result<()> {
    let passphrase = Zeroizing::new(
        std::fs::read_to_string(&opts.passphrase_file)
            .with_context(|| format!("cannot read {}", opts.passphrase_file.display()))?,
    );
    let passphrase = Zeroizing::new(passphrase.trim_end_matches(['\r', '\n']).to_string());
    std::fs::create_dir_all(dirs.root())
        .with_context(|| format!("cannot create {}", dirs.root().display()))?;
    let staging = dirs.root().join(format!(".restore-{}", new_id()));
    std::fs::create_dir_all(&staging)?;
    set_mode(&staging, 0o700)?;
    let result = restore(dirs, &opts, &staging, &passphrase).await;
    let _ = std::fs::remove_dir_all(&staging);
    result
}

async fn restore(dirs: &DataDir, opts: &Options, staging: &Path, passphrase: &str) -> Result<()> {
    step("Fetching the backup");
    let encrypted = fetch(opts, staging).await?;
    step("Decrypting and unpacking");
    let files = staging.join("files");
    {
        let encrypted = encrypted.clone();
        let files = files.clone();
        let passphrase = Zeroizing::new(passphrase.to_string());
        let archive = staging.join("archive.tar.gz");
        tokio::task::spawn_blocking(move || -> Result<()> {
            instance::decrypt(
                std::io::BufReader::new(std::fs::File::open(&encrypted)?),
                std::io::BufWriter::new(crate::secrets::create_private(&archive)?),
                &passphrase,
            )?;
            let gz = flate2::read::GzDecoder::new(std::fs::File::open(&archive)?);
            tar::Archive::new(gz)
                .unpack(&files)
                .context("cannot unpack the archive")?;
            std::fs::remove_file(&archive)?;
            Ok(())
        })
        .await??;
    }
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(files.join("manifest.json"))?)
        .context("the archive has no valid manifest.json")?;
    if manifest.format > instance::FORMAT {
        bail!(
            "the backup was made by version {} with a newer archive format; upgrade {} first",
            manifest.version,
            brand::DAEMON_BIN
        );
    }
    println!(
        "  backup of {} taken {} by version {}",
        manifest.hostname,
        manifest.created_at.format("%Y-%m-%d %H:%M UTC"),
        manifest.version
    );

    let service = !opts.no_service && systemd_unit_exists();
    if service {
        step(&format!("Stopping {}", brand::DAEMON_BIN));
        systemctl("stop")?;
    }

    step("Restoring config.toml, master.key and ssh/");
    dirs.create()?;
    let mut config = Config::load_path(&files.join("config.toml"))?;
    if let Some(url) = &opts.database_url {
        config.database_url = Some(url.clone());
        config.postgres = None;
    }
    config.save(dirs)?;
    replace_file(
        &dirs.master_key(),
        &std::fs::read(files.join("master.key"))?,
    )?;
    restore_ssh(&dirs.ssh_dir(), &files.join("ssh"))?;

    step(if store::is_external(&config) {
        "Restoring the store into the external database"
    } else {
        "Restoring the store into the managed Postgres container"
    });
    let url = store::prepare(&config).await?;
    let mut dump = tokio::fs::File::open(files.join("store.dump")).await?;
    store::restore(&config, &mut dump).await?;
    let db = Db::connect(&url).await?;
    if let Some(mut taken) = db.instance_backup(&manifest.backup_id).await? {
        taken.status = common::BackupStatus::Done;
        taken.size_bytes = std::fs::metadata(&encrypted)?.len();
        taken.finished_at = Some(manifest.created_at);
        db.update_instance_backup(&taken).await?;
    }

    let secrets = Secrets::new(db.clone(), MasterKey::load(&dirs.master_key())?);
    let all = db.secrets().await?;
    let mut unreadable = 0;
    for s in &all {
        if secrets.value(&s.id).await.is_err() {
            unreadable += 1;
        }
    }
    if unreadable > 0 {
        bail!(
            "{unreadable} of {} secrets cannot be decrypted with the restored master key",
            all.len()
        );
    }
    println!(
        "  {} secrets decrypt with the restored master key",
        all.len()
    );
    let missing: Vec<&String> = manifest
        .local_backups
        .iter()
        .filter(|key| !dirs.backups_dir().join(key).exists())
        .collect();
    if !missing.is_empty() {
        println!(
            "  {} local database backup file(s) were not part of the instance backup; copy {}/ \
             from the old machine if you still have it (backups in S3 storage are unaffected)",
            missing.len(),
            dirs.backups_dir().display()
        );
    }

    if service {
        step(&format!("Starting {}", brand::DAEMON_BIN));
        systemctl("start")?;
        wait_healthy(&config.listen).await?;
    }

    step("Checking servers");
    let state = AppState::new(db.clone(), dirs.clone(), &config, config.listen.clone())?;
    let mut offline = 0;
    for server in db.servers().await? {
        let checked = crate::api::check_server(&state, server).await?;
        let status = match checked.status {
            ServerStatus::Online => "online",
            ServerStatus::Offline => {
                offline += 1;
                "offline"
            }
            ServerStatus::Unknown => "unknown",
        };
        println!(
            "  {:<20} {status:<8} {}",
            checked.name,
            checked.docker_version.as_deref().unwrap_or("")
        );
    }
    let restored = db
        .insert_activity(&common::Activity {
            id: 0,
            at: Utc::now(),
            level: common::ActivityLevel::Success,
            service_id: None,
            service_name: None,
            project_id: None,
            message: "Instance restored from a backup".into(),
            detail: Some(format!(
                "taken {} on {}",
                manifest.created_at.format("%Y-%m-%d %H:%M UTC"),
                manifest.hostname
            )),
        })
        .await;
    if let Err(e) = restored {
        tracing::warn!("cannot record the restore: {e:#}");
    }

    println!();
    println!(
        "Restored {}. The API token is the one of the backed-up instance, so existing \
         `{} login` contexts keep working once they point at this machine.",
        dirs.root().display(),
        brand::CLI_BIN
    );
    if offline > 0 {
        println!(
            "{offline} server(s) are offline: check that they can be reached from this machine."
        );
    }
    if !service {
        println!("Start the daemon with `{} serve`.", brand::DAEMON_BIN);
    }
    Ok(())
}

fn step(what: &str) {
    println!("==> {what}");
}

/// The encrypted archive as a local file: the source itself, or a download from S3.
async fn fetch(opts: &Options, staging: &Path) -> Result<PathBuf> {
    let Some(rest) = opts.source.strip_prefix("s3://") else {
        let path = PathBuf::from(&opts.source);
        if !path.is_file() {
            bail!("{} does not exist", path.display());
        }
        return Ok(path);
    };
    let (bucket, key) = rest
        .split_once('/')
        .filter(|(b, k)| !b.is_empty() && !k.is_empty())
        .context("an S3 source looks like s3://<bucket>/<key>")?;
    let access_key = std::env::var("AWS_ACCESS_KEY_ID")
        .context("set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY to read the backup from S3")?;
    let secret_key = Zeroizing::new(
        std::env::var("AWS_SECRET_ACCESS_KEY").context("set AWS_SECRET_ACCESS_KEY")?,
    );
    let conn = StorageConnection {
        id: String::new(),
        name: "restore".into(),
        endpoint: opts.s3_endpoint.clone(),
        region: opts.s3_region.clone(),
        bucket: bucket.into(),
        path_prefix: String::new(),
        path_style: opts.s3_path_style,
        credentials_secret_id: String::new(),
        created_at: Utc::now(),
        last_test: None,
    };
    let store = BackupStore::s3(&conn, &access_key, &secret_key)?;
    let mut reader = store.get(key).await?;
    let path = staging.join("archive.enc");
    let mut file = tokio::fs::File::create(&path).await?;
    let bytes = tokio::io::copy(&mut reader, &mut file)
        .await
        .with_context(|| format!("cannot download {}", opts.source))?;
    tokio::io::AsyncWriteExt::shutdown(&mut file).await?;
    println!("  downloaded {bytes} bytes");
    Ok(path)
}

fn replace_file(path: &Path, contents: &[u8]) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("cannot replace {}", path.display())),
    }
    write_private(path, contents)
}

/// Replaces the key files in `ssh_dir` with the restored ones; sockets are left alone.
fn restore_ssh(ssh_dir: &Path, from: &Path) -> Result<()> {
    for entry in std::fs::read_dir(ssh_dir)?.flatten() {
        if entry.file_type()?.is_file() {
            std::fs::remove_file(entry.path())?;
        }
    }
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let target = ssh_dir.join(entry.file_name());
        write_private(&target, &std::fs::read(entry.path())?)?;
        if target.extension().is_some_and(|e| e == "pub") {
            set_mode(&target, 0o644)?;
        }
    }
    Ok(())
}

fn systemd_unit_exists() -> bool {
    Command::new("systemctl")
        .args(["cat", brand::DAEMON_BIN])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn systemctl(action: &str) -> Result<()> {
    let status = Command::new("systemctl")
        .args([action, brand::DAEMON_BIN])
        .status()
        .context("cannot run systemctl")?;
    if !status.success() {
        bail!(
            "systemctl {action} {} failed with {status}",
            brand::DAEMON_BIN
        );
    }
    Ok(())
}

async fn wait_healthy(listen: &str) -> Result<()> {
    let port = listen.rsplit(':').next().unwrap_or("");
    let url = format!("http://127.0.0.1:{port}/healthz");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if reqwest::get(&url)
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "{} did not come up on {url}; check `journalctl -u {}`",
                brand::DAEMON_BIN,
                brand::DAEMON_BIN
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
