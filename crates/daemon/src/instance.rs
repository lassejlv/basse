//! Instance backups: one encrypted archive of the daemon itself.
//!
//! The archive is a `tar.gz` of `store.dump` (`pg_dump -Fc` of the control store),
//! `config.toml`, `master.key`, `ssh/` (without sockets and handshake keys) and
//! `manifest.json`, which lists the local database backup files the store refers to (the files
//! themselves are not included). Because it holds the master key, the archive is encrypted with a
//! key derived from a passphrase:
//!
//! ```text
//! magic (8) | argon2id m_cost, t_cost, p_cost (3 x u32 LE) | salt (16) | nonce prefix (19)
//! then XChaCha20-Poly1305 STREAM (BE32) chunks of 64 KiB plaintext, the header as associated data
//! ```
//!
//! The STREAM construction authenticates chunk order and the final chunk, so a reordered or
//! truncated file fails to decrypt. Restoring is `<daemon> restore` (see `restore.rs`).

use crate::backup_store::BackupStore;
use crate::db::new_id;
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::state::AppState;
use crate::store;
use anyhow::{Context as _, Result, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::stream::{DecryptorBE32, EncryptorBE32};
use chacha20poly1305::aead::{KeyInit as _, Payload};
use chrono::Utc;
use common::api::{MIN_PASSPHRASE_LEN, UpdateInstanceBackupSchedule};
use common::*;
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"IBAK\x00\x01\r\n";
const SALT_LEN: usize = 16;
const NONCE_PREFIX_LEN: usize = 19;
const HEADER_LEN: usize = MAGIC.len() + 12 + SALT_LEN + NONCE_PREFIX_LEN;
const CHUNK: usize = 64 * 1024;
const TAG_LEN: usize = 16;
/// Argon2id with 64 MiB and 3 passes: about a second on a small server.
const M_COST_KIB: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 1;
/// Upper bounds accepted when reading a header, so a crafted file cannot exhaust memory.
const MAX_M_COST_KIB: u32 = 1024 * 1024;
const MAX_T_COST: u32 = 64;
/// Associated data of the schedule's sealed passphrase.
const SCHEDULE_AAD: &str = "instance-backup-schedule";
/// Prefix of instance backups in a store: `instance/<file name>`.
const STORE_DIR: &str = "instance";
const SCHEDULER_TICK: Duration = Duration::from_secs(300);
/// Archive format, recorded in the manifest.
pub const FORMAT: u32 = 1;

/// `manifest.json` inside the archive.
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// The instance backup this archive is; the dump still shows it as running.
    pub backup_id: Id,
    pub version: String,
    pub created_at: Timestamp,
    pub hostname: String,
    /// `managed` or `external`.
    pub store: String,
    /// Local database backup files (`<service id>/<file>`) that existed; they are not included.
    pub local_backups: Vec<String>,
}

fn derive_key(
    passphrase: &str,
    salt: &[u8],
    m: u32,
    t: u32,
    p: u32,
) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(m, t, p, Some(32)).map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
    Ok(key)
}

/// Fills `buf` as far as the reader allows; returns how many bytes were read.
fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Encrypts `reader` into `writer` under a key derived from `passphrase`.
pub fn encrypt(mut reader: impl Read, mut writer: impl Write, passphrase: &str) -> Result<()> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_PREFIX_LEN];
    rand::rng().fill_bytes(&mut salt);
    rand::rng().fill_bytes(&mut nonce);
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    for v in [M_COST_KIB, T_COST, P_COST] {
        header.extend_from_slice(&v.to_le_bytes());
    }
    header.extend_from_slice(&salt);
    header.extend_from_slice(&nonce);
    let key = derive_key(passphrase, &salt, M_COST_KIB, T_COST, P_COST)?;
    let cipher = XChaCha20Poly1305::new(key.as_ref().into());
    let mut stream = EncryptorBE32::from_aead(cipher, nonce.as_ref().into());
    writer.write_all(&header)?;
    let mut buf = Zeroizing::new(vec![0u8; CHUNK]);
    let mut next = Zeroizing::new(vec![0u8; CHUNK]);
    let mut n = read_full(&mut reader, &mut buf)?;
    loop {
        let m = if n == CHUNK {
            read_full(&mut reader, &mut next)?
        } else {
            0
        };
        let payload = Payload {
            msg: &buf[..n],
            aad: &header,
        };
        if m == 0 {
            let sealed = stream
                .encrypt_last(payload)
                .map_err(|_| anyhow::anyhow!("encryption failed"))?;
            writer.write_all(&sealed)?;
            break;
        }
        let sealed = stream
            .encrypt_next(payload)
            .map_err(|_| anyhow::anyhow!("encryption failed"))?;
        writer.write_all(&sealed)?;
        std::mem::swap(&mut buf, &mut next);
        n = m;
    }
    writer.flush()?;
    Ok(())
}

/// Decrypts a file written by [`encrypt`]. Fails on a wrong passphrase and on any change to the
/// file, truncation included.
pub fn decrypt(mut reader: impl Read, mut writer: impl Write, passphrase: &str) -> Result<()> {
    let mut header = [0u8; HEADER_LEN];
    if read_full(&mut reader, &mut header)? < HEADER_LEN || !header.starts_with(MAGIC) {
        bail!("not an instance backup (wrong file, or it is incomplete)");
    }
    let u32_at = |i: usize| {
        let at = MAGIC.len() + i * 4;
        u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes"))
    };
    let (m, t, p) = (u32_at(0), u32_at(1), u32_at(2));
    if m > MAX_M_COST_KIB || t > MAX_T_COST || p == 0 || p > 16 {
        bail!("the backup asks for unreasonable key derivation parameters");
    }
    let salt_at = MAGIC.len() + 12;
    let salt = &header[salt_at..salt_at + SALT_LEN];
    let nonce = &header[salt_at + SALT_LEN..];
    let key = derive_key(passphrase, salt, m, t, p)?;
    let cipher = XChaCha20Poly1305::new(key.as_ref().into());
    let mut stream = DecryptorBE32::from_aead(cipher, nonce.into());
    let wrong = || {
        anyhow::anyhow!("cannot decrypt: wrong passphrase, or the file was changed or cut short")
    };
    let mut buf = vec![0u8; CHUNK + TAG_LEN];
    let mut next = vec![0u8; CHUNK + TAG_LEN];
    let mut n = read_full(&mut reader, &mut buf)?;
    loop {
        let m = if n == buf.len() {
            read_full(&mut reader, &mut next)?
        } else {
            0
        };
        let payload = Payload {
            msg: &buf[..n],
            aad: &header,
        };
        if m == 0 {
            let plain = Zeroizing::new(stream.decrypt_last(payload).map_err(|_| wrong())?);
            writer.write_all(&plain)?;
            break;
        }
        let plain = Zeroizing::new(stream.decrypt_next(payload).map_err(|_| wrong())?);
        writer.write_all(&plain)?;
        std::mem::swap(&mut buf, &mut next);
        n = m;
    }
    writer.flush()?;
    Ok(())
}

fn check_passphrase(passphrase: &str) -> ApiResult<()> {
    if passphrase.chars().count() < MIN_PASSPHRASE_LEN {
        bad_request!("the passphrase must be at least {MIN_PASSPHRASE_LEN} characters");
    }
    Ok(())
}

/// Key of an instance backup in its store.
pub fn store_key(file_name: &str) -> String {
    format!("{STORE_DIR}/{file_name}")
}

/// Starts an instance backup in the background and returns its `running` row.
pub async fn start(
    state: &Arc<AppState>,
    trigger: BackupTrigger,
    storage_id: Option<&str>,
    passphrase: Zeroizing<String>,
) -> ApiResult<InstanceBackup> {
    check_passphrase(&passphrase)?;
    let (storage, conn) = state.storage_target(storage_id).await?;
    let Ok(guard) = state.instance_backup_lock.clone().try_lock_owned() else {
        conflict!("an instance backup is already running");
    };
    let mut backup = InstanceBackup {
        id: new_id(),
        created_at: Utc::now(),
        status: BackupStatus::Running,
        trigger,
        file_name: String::new(),
        size_bytes: 0,
        storage,
        storage_id: conn.as_ref().map(|c| c.id.clone()),
        error: None,
        finished_at: None,
    };
    state.db.insert_instance_backup(&mut backup).await?;
    let task = {
        let state = state.clone();
        let backup = backup.clone();
        async move {
            let _guard = guard;
            run(&state, backup, passphrase).await;
        }
    };
    tokio::spawn(task);
    Ok(backup)
}

async fn run(state: &AppState, mut backup: InstanceBackup, passphrase: Zeroizing<String>) {
    let staging = state
        .dirs
        .backups_dir()
        .join(".staging")
        .join(format!("instance-{}", backup.id));
    let result = write_archive(state, &backup, &staging, passphrase).await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    backup.finished_at = Some(Utc::now());
    match result {
        Ok(size) => {
            backup.status = BackupStatus::Done;
            backup.size_bytes = size;
            state
                .activity(
                    ActivityLevel::Success,
                    "Instance backup done".into(),
                    Some(backup.file_name.clone()),
                )
                .await;
        }
        Err(e) => {
            let message = format!("{e:#}");
            backup.status = BackupStatus::Failed;
            backup.error = Some(message.clone());
            state
                .activity(
                    ActivityLevel::Error,
                    "Instance backup failed".into(),
                    Some(message),
                )
                .await;
        }
    }
    if let Err(e) = state.db.update_instance_backup(&backup).await {
        tracing::warn!(backup = %backup.id, "cannot save instance backup: {e:#}");
    }
    if backup.trigger == BackupTrigger::Schedule
        && let Err(e) = prune(state).await
    {
        tracing::warn!("cannot prune instance backups: {e:#}");
    }
}

/// Dumps the store, packs and encrypts the archive in `staging`, and moves it into its store.
async fn write_archive(
    state: &AppState,
    backup: &InstanceBackup,
    staging: &Path,
    passphrase: Zeroizing<String>,
) -> Result<u64> {
    tokio::fs::create_dir_all(staging)
        .await
        .with_context(|| format!("cannot create {}", staging.display()))?;
    crate::config::set_mode(staging, 0o700)?;
    let config = state.config();
    let dump_path = staging.join("store.dump");
    let mut dump = tokio::fs::File::create(&dump_path).await?;
    store::dump(&config, &mut dump).await?;
    tokio::io::AsyncWriteExt::shutdown(&mut dump).await?;
    let local_backups = BackupStore::Local {
        root: state.dirs.backups_dir(),
    };
    let mut files = Vec::new();
    for service in state.db.services().await? {
        for file in local_backups.list(&service.service.id).await? {
            files.push(file.key);
        }
    }
    let manifest = Manifest {
        format: FORMAT,
        backup_id: backup.id.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        created_at: backup.created_at,
        hostname: crate::host::hostname(),
        store: if store::is_external(&config) {
            "external"
        } else {
            "managed"
        }
        .into(),
        local_backups: files,
    };
    let root = state.dirs.root().to_path_buf();
    let staging_dir = staging.to_path_buf();
    let encrypted = tokio::task::spawn_blocking(move || {
        pack_and_encrypt(&root, &staging_dir, &manifest, &passphrase)
    })
    .await??;
    state
        .backup_store(backup.storage, backup.storage_id.as_deref())
        .await?
        .put(&store_key(&backup.file_name), &encrypted)
        .await
}

/// Writes `archive.tar.gz` (deleted again once encrypted) and returns the encrypted file.
fn pack_and_encrypt(
    root: &Path,
    staging: &Path,
    manifest: &Manifest,
    passphrase: &str,
) -> Result<PathBuf> {
    let plain = staging.join("archive.tar.gz");
    {
        let file = crate::secrets::create_private(&plain)?;
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        tar.mode(tar::HeaderMode::Deterministic);
        let json = serde_json::to_vec_pretty(manifest)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(json.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, "manifest.json", json.as_slice())?;
        tar.append_path_with_name(staging.join("store.dump"), "store.dump")?;
        for name in ["config.toml", "master.key"] {
            tar.append_path_with_name(root.join(name), name)
                .with_context(|| format!("cannot add {name}"))?;
        }
        let ssh = root.join("ssh");
        if ssh.is_dir() {
            for entry in std::fs::read_dir(&ssh)? {
                let entry = entry?;
                let name = entry.file_name();
                if !entry.file_type()?.is_file() {
                    continue;
                }
                tar.append_path_with_name(entry.path(), Path::new("ssh").join(&name))?;
            }
        }
        tar.into_inner()?.finish()?.sync_all()?;
    }
    let sealed = staging.join("archive.enc");
    let result = encrypt(
        std::io::BufReader::new(std::fs::File::open(&plain)?),
        std::io::BufWriter::new(crate::secrets::create_private(&sealed)?),
        passphrase,
    );
    std::fs::remove_file(&plain)?;
    result?;
    Ok(sealed)
}

/// Opens the encrypted archive of a finished instance backup.
pub async fn open(
    state: &AppState,
    backup: &InstanceBackup,
) -> Result<std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>> {
    state
        .backup_store(backup.storage, backup.storage_id.as_deref())
        .await?
        .get(&store_key(&backup.file_name))
        .await
}

pub async fn delete(state: &AppState, backup: &InstanceBackup) -> ApiResult<()> {
    if backup.status == BackupStatus::Running {
        conflict!("instance backup {} is still running", backup.file_name);
    }
    state
        .backup_store(backup.storage, backup.storage_id.as_deref())
        .await?
        .delete(&store_key(&backup.file_name))
        .await?;
    state.db.delete_instance_backup(&backup.id).await?;
    Ok(())
}

/// Keeps the newest `keep` finished (and `keep` failed) scheduled backups in each store.
async fn prune(state: &AppState) -> Result<()> {
    let (schedule, _, _) = state.db.instance_backup_schedule().await?;
    let keep = usize::try_from(schedule.keep).unwrap_or(usize::MAX);
    let backups = state.db.instance_backups().await?;
    let mut seen: Vec<(BackupStorage, Option<&str>, BackupStatus, usize)> = Vec::new();
    for b in backups
        .iter()
        .filter(|b| b.trigger == BackupTrigger::Schedule)
    {
        if b.status == BackupStatus::Running {
            continue;
        }
        let slot = (b.storage, b.storage_id.as_deref(), b.status);
        let count = match seen.iter_mut().find(|s| (s.0, s.1, s.2) == slot) {
            Some(s) => {
                s.3 += 1;
                s.3
            }
            None => {
                seen.push((slot.0, slot.1, slot.2, 1));
                1
            }
        };
        if count > keep {
            delete(state, b).await.map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }
    Ok(())
}

pub async fn schedule(state: &AppState) -> ApiResult<InstanceBackupSchedule> {
    Ok(state.db.instance_backup_schedule().await?.0)
}

pub async fn set_schedule(
    state: &AppState,
    body: UpdateInstanceBackupSchedule,
) -> ApiResult<InstanceBackupSchedule> {
    if body.hour_utc > 23 {
        bad_request!("hour_utc must be between 0 and 23");
    }
    if body.keep == 0 {
        bad_request!("keep must be at least 1");
    }
    let storage_id = body.storage_id.filter(|id| !id.is_empty());
    let (_, conn) = state.storage_target(storage_id.as_deref()).await?;
    let (old, sealed, last_run_at) = state.db.instance_backup_schedule().await?;
    let sealed = match body.passphrase {
        Some(passphrase) => {
            let passphrase = Zeroizing::new(passphrase);
            check_passphrase(&passphrase)?;
            Some(
                state
                    .secrets
                    .key_seal(SCHEDULE_AAD, passphrase.as_bytes())?,
            )
        }
        None => sealed,
    };
    if body.enabled && sealed.is_none() {
        bad_request!("scheduled instance backups need a passphrase");
    }
    let schedule = InstanceBackupSchedule {
        enabled: body.enabled,
        hour_utc: body.hour_utc,
        keep: body.keep,
        storage_id,
        has_passphrase: sealed.is_some(),
    };
    let restarts = schedule.enabled && (!old.enabled || old.hour_utc != schedule.hour_utc);
    let last_run_at = if restarts {
        Some(Utc::now())
    } else {
        last_run_at
    };
    state
        .db
        .save_instance_backup_schedule(&schedule, sealed.as_deref(), last_run_at)
        .await?;
    let detail = if schedule.enabled {
        format!(
            "daily at {:02}:00 UTC to {}, keeping {}",
            schedule.hour_utc,
            conn.map_or("local storage".into(), |c| c.name),
            schedule.keep
        )
    } else {
        "off".into()
    };
    state
        .activity(
            ActivityLevel::Info,
            "Instance backup schedule updated".into(),
            Some(detail),
        )
        .await;
    Ok(schedule)
}

/// Starts the daily instance backup when its slot has passed.
pub fn spawn_scheduler(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SCHEDULER_TICK);
        loop {
            tick.tick().await;
            if let Err(e) = run_if_due(&state).await {
                tracing::warn!("scheduled instance backup: {e:#}");
            }
        }
    });
}

async fn run_if_due(state: &Arc<AppState>) -> Result<()> {
    let now = Utc::now();
    let (schedule, sealed, last_run_at) = state.db.instance_backup_schedule().await?;
    let as_db = BackupSchedule {
        enabled: schedule.enabled,
        hour_utc: schedule.hour_utc,
        keep: schedule.keep,
        storage_id: None,
    };
    if !crate::database::is_due(&as_db, last_run_at, now) {
        return Ok(());
    }
    let Some(sealed) = sealed else {
        return Ok(());
    };
    let passphrase = state.secrets.key_open(SCHEDULE_AAD, &sealed)?;
    let passphrase = Zeroizing::new(String::from_utf8(passphrase.to_vec())?);
    match start(
        state,
        BackupTrigger::Schedule,
        schedule.storage_id.as_deref(),
        passphrase,
    )
    .await
    {
        Ok(_) => {}
        Err(AppError::Conflict(_)) => return Ok(()),
        Err(e) => {
            state
                .activity(
                    ActivityLevel::Error,
                    "Scheduled instance backup skipped".into(),
                    Some(e.to_string()),
                )
                .await;
        }
    }
    state
        .db
        .save_instance_backup_schedule(&schedule, Some(&sealed), Some(now))
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(data: &[u8]) {
        let mut sealed = Vec::new();
        encrypt(data, &mut sealed, "correct horse battery").unwrap();
        assert!(sealed.starts_with(MAGIC));
        let mut plain = Vec::new();
        decrypt(sealed.as_slice(), &mut plain, "correct horse battery").unwrap();
        assert!(plain == data, "round trip of {} bytes differs", data.len());
    }

    #[test]
    fn archives_round_trip_at_chunk_boundaries() {
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 3 * CHUNK] {
            let data: Vec<u8> = (0..len).map(|i| (i % 253) as u8).collect();
            roundtrip(&data);
        }
    }

    #[test]
    fn wrong_passphrases_and_tampering_are_rejected() {
        let data = vec![7u8; 2 * CHUNK + 10];
        let mut sealed = Vec::new();
        encrypt(data.as_slice(), &mut sealed, "correct horse battery").unwrap();
        let fails = |bytes: &[u8], pass: &str| decrypt(bytes, std::io::sink(), pass).is_err();
        assert!(fails(&sealed, "wrong passphrase!"));
        let mut flipped = sealed.clone();
        flipped[HEADER_LEN + 5] ^= 1;
        assert!(fails(&flipped, "correct horse battery"));
        let mut header = sealed.clone();
        header[MAGIC.len() + 12] ^= 1;
        assert!(
            fails(&header, "correct horse battery"),
            "the header is authenticated"
        );
        let cut = &sealed[..HEADER_LEN + CHUNK + TAG_LEN];
        assert!(
            fails(cut, "correct horse battery"),
            "truncation at a chunk boundary"
        );
        assert!(fails(b"not a backup", "correct horse battery"));
    }
}
