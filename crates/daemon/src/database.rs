//! One-click databases: connection details, public access, password rotation, backups,
//! restores and the daily backup scheduler.
//!
//! Every operation that touches the database itself runs a command inside its live container
//! over the server's Docker connection, so local and SSH servers behave the same.

use crate::backup_store::{BackupStore, backup_key};
use crate::db::{ScheduleRecord, ServiceRecord, new_id};
use crate::deploy;
use crate::docker::{self, RunState};
use crate::error::{ApiResult, AppError, bad_request, conflict};
use crate::host;
use crate::services;
use crate::state::AppState;
use anyhow::{Context as _, Result, bail};
use async_compression::tokio::bufread::GzipDecoder;
use async_compression::tokio::write::GzipEncoder;
use bollard::Docker;
use chrono::Utc;
use common::*;
use rand::Rng as _;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWriteExt as _, BufReader};
use tokio::sync::OwnedMutexGuard;

/// How often the scheduler looks for due backups.
const SCHEDULER_TICK: Duration = Duration::from_secs(300);
/// Seconds a database gets to shut down cleanly before a restore replaces its files.
const STOP_TIMEOUT_SECS: i32 = 30;

/// A fresh password: 24 characters from an alphabet without look-alikes (about 140 bits).
pub fn generate_password() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::rng();
    (0..24)
        .map(|_| char::from(ALPHABET[rng.random_range(0..ALPHABET.len())]))
        .collect()
}

// Credentials and connection URLs

/// Environment variables that hold a database's credentials.
struct Keys {
    user: Option<&'static str>,
    password: &'static str,
    database: Option<&'static str>,
    root_password: Option<&'static str>,
    url: &'static str,
}

fn keys(kind: DatabaseKind) -> Keys {
    match kind {
        DatabaseKind::Postgres => Keys {
            user: Some("POSTGRES_USER"),
            password: "POSTGRES_PASSWORD",
            database: Some("POSTGRES_DB"),
            root_password: None,
            url: "DATABASE_URL",
        },
        DatabaseKind::Mysql => Keys {
            user: Some("MYSQL_USER"),
            password: "MYSQL_PASSWORD",
            database: Some("MYSQL_DATABASE"),
            root_password: Some("MYSQL_ROOT_PASSWORD"),
            url: "DATABASE_URL",
        },
        DatabaseKind::Mariadb => Keys {
            user: Some("MARIADB_USER"),
            password: "MARIADB_PASSWORD",
            database: Some("MARIADB_DATABASE"),
            root_password: Some("MARIADB_ROOT_PASSWORD"),
            url: "DATABASE_URL",
        },
        DatabaseKind::Redis => Keys {
            user: None,
            password: "REDIS_PASSWORD",
            database: None,
            root_password: None,
            url: "REDIS_URL",
        },
        DatabaseKind::Mongo => Keys {
            user: Some("MONGO_INITDB_ROOT_USERNAME"),
            password: "MONGO_INITDB_ROOT_PASSWORD",
            database: None,
            root_password: None,
            url: "DATABASE_URL",
        },
    }
}

/// A database's credentials as stored in its environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    /// `None` for Redis.
    pub user: Option<String>,
    pub password: String,
    pub database: Option<String>,
    /// MySQL and MariaDB root password, used for dumps and admin statements.
    pub root_password: Option<String>,
}

impl Credentials {
    pub fn from_env(kind: DatabaseKind, vars: &[EnvVar]) -> Self {
        let keys = keys(kind);
        let get = |key: Option<&str>| {
            key.and_then(|k| vars.iter().find(|v| v.key == k))
                .map(|v| v.value.clone())
        };
        let mut user = get(keys.user);
        let mut database = get(keys.database);
        if kind == DatabaseKind::Postgres {
            let user = user.get_or_insert_with(|| "postgres".into());
            database.get_or_insert_with(|| user.clone());
        }
        Self {
            user,
            password: get(Some(keys.password)).unwrap_or_default(),
            database,
            root_password: get(keys.root_password),
        }
    }
}

/// Connection URL for `host:port`, e.g. `postgres://user:pass@db:5432/db`.
pub fn connection_url(kind: DatabaseKind, creds: &Credentials, host: &str, port: u16) -> String {
    let user = creds.user.as_deref().map(escape).unwrap_or_default();
    let pass = escape(&creds.password);
    let path = creds
        .database
        .as_deref()
        .map(|db| format!("/{}", escape(db)))
        .unwrap_or_default();
    match kind {
        DatabaseKind::Postgres => format!("postgres://{user}:{pass}@{host}:{port}{path}"),
        DatabaseKind::Mysql | DatabaseKind::Mariadb => {
            format!("mysql://{user}:{pass}@{host}:{port}{path}")
        }
        DatabaseKind::Redis => format!("redis://:{pass}@{host}:{port}"),
        DatabaseKind::Mongo => {
            format!("mongodb://{user}:{pass}@{host}:{port}/?authSource=admin")
        }
    }
}

/// Percent-encodes everything but RFC 3986 unreserved characters.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Connection details of a database service; `None` for other services.
pub fn info(
    service: &Service,
    server: &Server,
    vars: &[EnvVar],
    public_port: Option<u16>,
) -> Option<DatabaseInfo> {
    let Source::Database { kind, version } = &service.source else {
        return None;
    };
    let kind = *kind;
    let creds = Credentials::from_env(kind, vars);
    let port = service.port.unwrap_or(kind.port());
    let public_host = match (public_port, server.kind) {
        (None, _) => None,
        (Some(_), ServerKind::Local) => host::public_ip(),
        (Some(_), ServerKind::Ssh) => server.host.clone(),
    };
    Some(DatabaseInfo {
        kind,
        version: version.clone(),
        host: service.name.clone(),
        port,
        username: creds.user.clone(),
        database: creds.database.clone(),
        password: creds.password.clone(),
        internal_url: connection_url(kind, &creds, &service.name, port),
        public_port,
        public_url: public_port
            .zip(public_host)
            .map(|(p, h)| connection_url(kind, &creds, &h, p)),
    })
}

fn database_kind(service: &Service) -> ApiResult<DatabaseKind> {
    match &service.source {
        Source::Database { kind, .. } => Ok(*kind),
        _ => bad_request!("'{}' is not a database service", service.name),
    }
}

// Public access

/// Publishes the database port on the server's `port`, or stops publishing it for `None`
/// or 0. A live container picks the change up through a redeploy of its image.
pub async fn set_public_port(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    port: Option<u16>,
) -> ApiResult<()> {
    let svc = &rec.service;
    database_kind(svc)?;
    let port = port.filter(|p| *p != 0);
    if let Some(p) = port {
        if matches!(p, 80 | 443) {
            bad_request!("port {p} is reserved for the proxy");
        }
        if state
            .db
            .public_port_taken(&svc.server_id, p, &svc.id)
            .await?
        {
            conflict!("another service on this server already publishes port {p}");
        }
    }
    if state.db.public_port(&svc.id).await? == port {
        return Ok(());
    }
    state.db.set_public_port(&svc.id, port).await?;
    let (message, detail) = match port {
        Some(p) => ("Public access enabled", Some(format!("port {p}"))),
        None => ("Public access disabled", None),
    };
    state
        .service_activity(svc, ActivityLevel::Info, message.into(), detail)
        .await;
    deploy::reconfigure(state, rec).await?;
    Ok(())
}

// Running commands in the database container

/// A database service with a running container, ready for commands.
struct Target {
    service: Service,
    kind: DatabaseKind,
    docker: Docker,
    container: String,
    image: String,
    creds: Credentials,
}

async fn target(state: &AppState, rec: &ServiceRecord) -> ApiResult<Target> {
    let service = rec.service.clone();
    let kind = database_kind(&service)?;
    if rec.busy.is_some() {
        conflict!("a deployment of '{}' is in progress", service.name);
    }
    let Some(container) = rec.live_container.clone() else {
        bad_request!("'{}' has not been deployed yet", service.name);
    };
    let (_, docker) = state.docker_for(&service.server_id).await?;
    let inspected = docker::inspect(&docker, &container).await?;
    let Some(inspected) = inspected.filter(|c| RunState::from_inspect(c) == RunState::Running)
    else {
        bad_request!("'{}' is not running; start it first", service.name);
    };
    let image = inspected
        .config
        .and_then(|c| c.image)
        .unwrap_or_else(|| service.source.label());
    let creds = Credentials::from_env(kind, &state.db.env(&service.id).await?);
    Ok(Target {
        service,
        kind,
        docker,
        container,
        image,
        creds,
    })
}

impl Target {
    /// Credentials handed to commands as `DB_*` variables, so scripts never embed secrets and
    /// always use the stored values even after a rotation the container env predates.
    fn env(&self) -> Vec<String> {
        let mut env = vec![
            format!("DB_USER={}", self.creds.user.as_deref().unwrap_or_default()),
            format!("DB_PASSWORD={}", self.creds.password),
            format!(
                "DB_NAME={}",
                self.creds.database.as_deref().unwrap_or_default()
            ),
            format!(
                "DB_ROOT_PASSWORD={}",
                self.creds.root_password.as_deref().unwrap_or_default()
            ),
        ];
        if self.kind == DatabaseKind::Redis {
            env.push(format!("REDISCLI_AUTH={}", self.creds.password));
        }
        env
    }

    /// Runs `script` with `sh -c` in the database container.
    async fn run(
        &self,
        script: &str,
        extra_env: &[String],
        stdin: Option<&mut (dyn AsyncRead + Unpin + Send)>,
        stdout: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    ) -> Result<docker::ExecOutcome> {
        let mut env = self.env();
        env.extend_from_slice(extra_env);
        docker::exec_stream(
            &self.docker,
            &self.container,
            sh(script),
            env,
            stdin,
            stdout,
        )
        .await
    }
}

fn sh(script: &str) -> Vec<String> {
    vec!["sh".into(), "-c".into(), script.into()]
}

/// MySQL and MariaDB command line client and dump tool.
fn mysql_tools(kind: DatabaseKind) -> (&'static str, &'static str) {
    if kind == DatabaseKind::Mariadb {
        ("mariadb", "mariadb-dump")
    } else {
        ("mysql", "mysqldump")
    }
}

/// File extension (before `.gz`) and shell script writing a dump to stdout.
///
/// MySQL and MariaDB dump every schema except the system ones, so a restore never rolls back
/// users and passwords; the app database is always included.
fn dump_script(kind: DatabaseKind) -> (&'static str, String) {
    match kind {
        DatabaseKind::Postgres => (
            "dump",
            r#"exec pg_dump -U "$DB_USER" -d "$DB_NAME" -Fc"#.into(),
        ),
        DatabaseKind::Mysql | DatabaseKind::Mariadb => {
            let (client, dump) = mysql_tools(kind);
            let gtid = if kind == DatabaseKind::Mysql {
                " --set-gtid-purged=OFF"
            } else {
                ""
            };
            (
                "sql",
                format!(
                    r#"set -e
dbs=$({client} -uroot -p"$DB_ROOT_PASSWORD" -N -B -e "SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('mysql', 'information_schema', 'performance_schema', 'sys')")
dbs=$(printf '%s\n' $dbs $DB_NAME | sort -u)
exec {dump} -uroot -p"$DB_ROOT_PASSWORD" --single-transaction --routines --events --triggers{gtid} --databases $dbs"#
                ),
            )
        }
        DatabaseKind::Redis => (
            "rdb",
            r#"f=$(mktemp) && redis-cli --no-auth-warning --rdb "$f" >/dev/null && cat "$f"
rc=$?
rm -f "$f"
exit $rc"#
                .into(),
        ),
        DatabaseKind::Mongo => (
            "archive",
            r#"exec mongodump --quiet --archive -u "$DB_USER" -p "$DB_PASSWORD" --authenticationDatabase admin"#
                .into(),
        ),
    }
}

/// Shell script reading a dump from stdin into the running database. Redis restores by
/// replacing files instead, see [`restore_redis`].
fn restore_script(kind: DatabaseKind) -> String {
    match kind {
        DatabaseKind::Postgres => {
            r#"exec pg_restore -U "$DB_USER" -d "$DB_NAME" --clean --if-exists --single-transaction"#
                .into()
        }
        DatabaseKind::Mysql | DatabaseKind::Mariadb => {
            let (client, _) = mysql_tools(kind);
            format!(r#"exec {client} -uroot -p"$DB_ROOT_PASSWORD""#)
        }
        DatabaseKind::Redis => unreachable!("redis restores through its data files"),
        DatabaseKind::Mongo => {
            r#"exec mongorestore --quiet --archive --drop --nsExclude='admin.*' -u "$DB_USER" -p "$DB_PASSWORD" --authenticationDatabase admin"#
                .into()
        }
    }
}

/// Run in a helper container on the stopped Redis volume: checks the dump, then installs it
/// both as `dump.rdb` and as the base of a fresh append-only file, because Redis loads only the
/// AOF when `appendonly` is on.
const REDIS_RESTORE: &str = r#"set -e
cd "$DATA_DIR"
cat > restore.rdb
redis-check-rdb restore.rdb >/dev/null
rm -rf appendonlydir
mkdir appendonlydir
cp restore.rdb appendonlydir/appendonly.aof.1.base.rdb
printf 'file appendonly.aof.1.base.rdb seq 1 type b\n' > appendonlydir/appendonly.aof.manifest
mv restore.rdb dump.rdb
chown -R redis:redis . 2>/dev/null || true"#;

// Locks

/// Serializes backups, restores and password rotations per service.
#[derive(Default)]
pub struct Locks {
    inner: Mutex<HashMap<Id, Arc<tokio::sync::Mutex<()>>>>,
}

impl Locks {
    fn try_acquire(&self, service: &Service) -> ApiResult<OwnedMutexGuard<()>> {
        let lock = lock(&self.inner)
            .entry(service.id.clone())
            .or_default()
            .clone();
        lock.try_lock_owned().map_err(|_| {
            AppError::Conflict(format!(
                "a backup, restore or password rotation of '{}' is already running",
                service.name
            ))
        })
    }

    pub fn forget_service(&self, service_id: &str) {
        lock(&self.inner).remove(service_id);
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// Password rotation

/// Sets a new password inside the running database, then stores it in the env vars and
/// connection URL. Nothing is stored when the database refuses the change.
pub async fn rotate_password(state: &Arc<AppState>, rec: &ServiceRecord) -> ApiResult<()> {
    let t = target(state, rec).await?;
    let _guard = state.db_locks.try_acquire(&t.service)?;
    let new = generate_password();
    change_password(&t, &new)
        .await
        .map_err(|e| e.context("the database refused the new password; nothing was changed"))?;

    let keys = keys(t.kind);
    let mut creds = t.creds.clone();
    creds.password.clone_from(&new);
    let port = t.service.port.unwrap_or(t.kind.port());
    let url = connection_url(t.kind, &creds, &t.service.name, port);
    let mut vars = state.db.env(&t.service.id).await?;
    for (key, value) in [(keys.password, new), (keys.url, url)] {
        match vars.iter_mut().find(|v| v.key == key) {
            Some(var) => var.value = value,
            None => vars.push(EnvVar {
                key: key.into(),
                value,
                secret: true,
            }),
        }
    }
    state.db.replace_env(&t.service.id, &vars).await?;
    state
        .service_activity(
            &t.service,
            ActivityLevel::Success,
            "Database password rotated".into(),
            Some("redeploy services that copied the old connection URL".into()),
        )
        .await;
    if t.kind == DatabaseKind::Redis {
        // The server reads `$REDIS_PASSWORD` at start; recreate the container so a restart keeps
        // the new password.
        deploy::reconfigure(state, &services::record(state, &t.service.id).await?).await?;
    }
    Ok(())
}

async fn change_password(t: &Target, new: &str) -> Result<()> {
    let new_env = [format!("DB_NEW_PASSWORD={new}")];
    let mut out: Vec<u8> = Vec::new();
    match t.kind {
        DatabaseKind::Postgres => {
            let mut sql: &[u8] = b"ALTER USER :\"u\" WITH PASSWORD :'p';\n";
            t.run(
                r#"exec psql -X -q -v ON_ERROR_STOP=1 -v u="$DB_USER" -v p="$DB_NEW_PASSWORD" -U "$DB_USER" -d "$DB_NAME" -f -"#,
                &new_env,
                Some(&mut sql),
                &mut out,
            )
            .await?
            .check("psql")
        }
        DatabaseKind::Mysql | DatabaseKind::Mariadb => {
            let user = t
                .creds
                .user
                .as_deref()
                .context("the service has no application user to rotate")?;
            let sql = format!(
                "ALTER USER {}@'%' IDENTIFIED BY {};\n",
                sql_quote(user),
                sql_quote(new)
            );
            let (client, _) = mysql_tools(t.kind);
            t.run(
                &format!(r#"exec {client} -uroot -p"$DB_ROOT_PASSWORD""#),
                &new_env,
                Some(&mut sql.as_bytes()),
                &mut out,
            )
            .await?
            .check(client)
        }
        DatabaseKind::Redis => {
            t.run(
                r#"exec redis-cli --no-auth-warning CONFIG SET requirepass "$DB_NEW_PASSWORD""#,
                &new_env,
                None,
                &mut out,
            )
            .await?
            .check("redis-cli")?;
            let reply = String::from_utf8_lossy(&out);
            if reply.trim() != "OK" {
                bail!("redis-cli: {}", reply.trim());
            }
            Ok(())
        }
        DatabaseKind::Mongo => t
            .run(
                r#"exec mongosh --quiet -u "$DB_USER" -p "$DB_PASSWORD" --authenticationDatabase admin admin --eval 'db.changeUserPassword(process.env.DB_USER, process.env.DB_NEW_PASSWORD)'"#,
                &new_env,
                None,
                &mut out,
            )
            .await?
            .check("mongosh"),
    }
}

/// A single-quoted SQL string literal.
fn sql_quote(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
}

// Backups

/// Starts a backup in the background and returns its `running` row.
pub async fn start_backup(
    state: &Arc<AppState>,
    rec: &ServiceRecord,
    trigger: BackupTrigger,
    storage_id: Option<&str>,
) -> ApiResult<Backup> {
    let (storage, conn) = state.storage_target(storage_id).await?;
    let t = target(state, rec).await?;
    let guard = state.db_locks.try_acquire(&t.service)?;
    let (extension, script) = dump_script(t.kind);
    let mut backup = Backup {
        id: new_id(),
        service_id: t.service.id.clone(),
        created_at: Utc::now(),
        size_bytes: 0,
        status: BackupStatus::Running,
        trigger,
        file_name: String::new(),
        error: None,
        storage,
        storage_id: conn.map(|c| c.id),
        finished_at: None,
    };
    state.db.insert_backup(&mut backup, extension).await?;
    let task = {
        let state = state.clone();
        let backup = backup.clone();
        async move {
            let _guard = guard;
            run_backup(&state, &t, backup, &script).await;
        }
    };
    tokio::spawn(task);
    Ok(backup)
}

async fn run_backup(state: &AppState, t: &Target, mut backup: Backup, script: &str) {
    let result = dump(state, t, &backup, script).await;
    backup.finished_at = Some(Utc::now());
    match result {
        Ok(size) => {
            backup.status = BackupStatus::Done;
            backup.size_bytes = size;
            state
                .service_activity(
                    &t.service,
                    ActivityLevel::Success,
                    "Backup done".into(),
                    Some(format!("{} ({})", backup.file_name, human_size(size))),
                )
                .await;
        }
        Err(e) => {
            let message = format!("{e:#}");
            backup.status = BackupStatus::Failed;
            backup.error = Some(message.clone());
            state
                .service_activity(
                    &t.service,
                    ActivityLevel::Error,
                    "Backup failed".into(),
                    Some(message),
                )
                .await;
        }
    }
    if let Err(e) = state.db.update_backup(&backup).await {
        tracing::warn!(backup = %backup.id, "cannot save backup: {e:#}");
    }
    if backup.trigger == BackupTrigger::Schedule
        && let Err(e) = prune(state, &t.service.id).await
    {
        tracing::warn!("cannot prune backups of {}: {e:#}", t.service.name);
    }
}

/// Streams the dump through gzip into a staging file, then moves it into storage.
async fn dump(state: &AppState, t: &Target, backup: &Backup, script: &str) -> Result<u64> {
    let staging = state.dirs.backups_dir().join(".staging");
    tokio::fs::create_dir_all(&staging)
        .await
        .with_context(|| format!("cannot create {}", staging.display()))?;
    let path = staging.join(format!("{}.gz", backup.id));
    let result = async {
        let file = tokio::fs::File::create(&path)
            .await
            .with_context(|| format!("cannot create {}", path.display()))?;
        let mut gz = GzipEncoder::new(file);
        let outcome = t.run(script, &[], None, &mut gz).await?;
        gz.shutdown()
            .await
            .context("cannot finish the backup file")?;
        outcome.check("the dump command")?;
        state
            .backup_store(backup.storage, backup.storage_id.as_deref())
            .await?
            .put(&backup_key(&backup.service_id, &backup.file_name), &path)
            .await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    result
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Opens a backup file, decompressed.
pub async fn open_backup(
    state: &AppState,
    backup: &Backup,
) -> Result<std::pin::Pin<Box<dyn AsyncRead + Send>>> {
    state
        .backup_store(backup.storage, backup.storage_id.as_deref())
        .await?
        .get(&backup_key(&backup.service_id, &backup.file_name))
        .await
}

/// Replaces the database's data with a finished backup. Returns when the restore is done.
pub async fn restore(state: &Arc<AppState>, backup: &Backup) -> ApiResult<()> {
    if backup.status != BackupStatus::Done {
        bad_request!(
            "backup {} is {}, only finished backups can be restored",
            backup.file_name,
            backup.status.as_str()
        );
    }
    let rec = services::record(state, &backup.service_id).await?;
    let t = target(state, &rec).await?;
    let _guard = state.db_locks.try_acquire(&t.service)?;
    let result = async {
        let mut reader = GzipDecoder::new(BufReader::new(open_backup(state, backup).await?));
        if t.kind == DatabaseKind::Redis {
            return restore_redis(state, &t, &mut reader).await;
        }
        t.run(
            &restore_script(t.kind),
            &[],
            Some(&mut reader),
            &mut tokio::io::sink(),
        )
        .await?
        .check("the restore command")
    }
    .await;
    match result {
        Ok(()) => {
            state
                .service_activity(
                    &t.service,
                    ActivityLevel::Success,
                    "Backup restored".into(),
                    Some(backup.file_name.clone()),
                )
                .await;
            Ok(())
        }
        Err(e) => {
            state
                .service_activity(
                    &t.service,
                    ActivityLevel::Error,
                    "Restore failed".into(),
                    Some(format!("{e:#}")),
                )
                .await;
            Err(e
                .context(format!("cannot restore {}", backup.file_name))
                .into())
        }
    }
}

/// Stops Redis, swaps its data files from a helper container on the same volume, and starts
/// it again, also when the swap fails.
async fn restore_redis(
    state: &AppState,
    t: &Target,
    reader: &mut (dyn AsyncRead + Unpin + Send),
) -> Result<()> {
    let data = state
        .db
        .volumes(&t.service.id)
        .await?
        .into_iter()
        .find(|v| v.name == "data")
        .context("the service has no data volume")?;
    docker::stop_container(&t.docker, &t.container, STOP_TIMEOUT_SECS).await?;
    let swapped = async {
        let helper = docker::start_helper(
            &t.docker,
            &format!("{}-restore-{}", brand::PREFIX, t.service.id),
            &t.image,
            vec![format!(
                "{}:{}",
                docker::volume_name(&t.service.id, &data.name),
                data.mount_path
            )],
            None,
        )
        .await?;
        let outcome = docker::exec_stream(
            &t.docker,
            &helper,
            sh(REDIS_RESTORE),
            vec![format!("DATA_DIR={}", data.mount_path)],
            Some(reader),
            &mut tokio::io::sink(),
        )
        .await;
        let removed = docker::remove_container(&t.docker, &helper).await;
        outcome?.check("installing the dump")?;
        removed
    }
    .await;
    let started = docker::start_container(&t.docker, &t.container).await;
    swapped.and(started)
}

/// Deletes a backup's file and row. Running backups cannot be deleted.
pub async fn delete_backup(state: &AppState, backup: &Backup) -> ApiResult<()> {
    if backup.status == BackupStatus::Running {
        conflict!("backup {} is still running", backup.file_name);
    }
    state
        .backup_store(backup.storage, backup.storage_id.as_deref())
        .await?
        .delete(&backup_key(&backup.service_id, &backup.file_name))
        .await?;
    state.db.delete_backup(&backup.id).await?;
    Ok(())
}

/// Deletes every stored backup file of a service that is being deleted: each backup from its
/// own store, then anything else left in the local directory.
pub async fn delete_backup_files(state: &AppState, service_id: &str) {
    for backup in state.db.backups(service_id).await.unwrap_or_default() {
        if backup.storage == BackupStorage::Local {
            continue;
        }
        let deleted = async {
            state
                .backup_store(backup.storage, backup.storage_id.as_deref())
                .await?
                .delete(&backup_key(&backup.service_id, &backup.file_name))
                .await
        }
        .await;
        if let Err(e) = deleted {
            tracing::warn!("cannot delete backup {}: {e:#}", backup.file_name);
        }
    }
    let store = BackupStore::Local {
        root: state.dirs.backups_dir(),
    };
    let files = match store.list(service_id).await {
        Ok(files) => files,
        Err(e) => {
            tracing::warn!("cannot list backups of {service_id}: {e:#}");
            return;
        }
    };
    for file in files {
        if let Err(e) = store.delete(&file.key).await {
            tracing::warn!("cannot delete backup {}: {e:#}", file.key);
        }
    }
}

// Schedules

pub async fn set_schedule(
    state: &AppState,
    rec: &ServiceRecord,
    schedule: BackupSchedule,
) -> ApiResult<BackupSchedule> {
    let svc = &rec.service;
    database_kind(svc)?;
    if schedule.hour_utc > 23 {
        bad_request!("hour_utc must be between 0 and 23");
    }
    if schedule.keep == 0 {
        bad_request!("keep must be at least 1");
    }
    let schedule = BackupSchedule {
        storage_id: schedule.storage_id.filter(|id| !id.is_empty()),
        ..schedule
    };
    let (_, conn) = state.storage_target(schedule.storage_id.as_deref()).await?;
    let mut record = state.db.backup_schedule(&svc.id).await?;
    let restarts = schedule.enabled
        && (!record.schedule.enabled || record.schedule.hour_utc != schedule.hour_utc);
    if restarts {
        record.last_run_at = Some(Utc::now());
    }
    record.schedule = schedule.clone();
    state.db.save_backup_schedule(&record).await?;
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
        .service_activity(
            svc,
            ActivityLevel::Info,
            "Backup schedule updated".into(),
            Some(detail),
        )
        .await;
    Ok(schedule)
}

/// Start of the most recent daily slot at `hour` UTC at or before `now`.
fn latest_slot(hour: u8, now: Timestamp) -> Option<Timestamp> {
    let today = now
        .date_naive()
        .and_hms_opt(u32::from(hour), 0, 0)?
        .and_utc();
    Some(if today <= now {
        today
    } else {
        today - chrono::Duration::days(1)
    })
}

/// Whether a schedule's latest slot passed without a run. A missed slot (daemon down) runs as
/// soon as the daemon is back.
pub fn is_due(schedule: &BackupSchedule, last_run_at: Option<Timestamp>, now: Timestamp) -> bool {
    if !schedule.enabled {
        return false;
    }
    let Some(slot) = latest_slot(schedule.hour_utc, now) else {
        return false;
    };
    last_run_at.is_none_or(|last| last < slot)
}

/// Scheduled backups beyond the newest `keep` finished ones, and failed ones beyond the
/// newest `keep` failures, so a run of failures never pushes out good backups.
/// `backups` is newest first. Manual backups are never pruned.
fn prune_candidates(backups: &[Backup], keep: u32) -> Vec<&Backup> {
    let keep = usize::try_from(keep).unwrap_or(usize::MAX);
    let (mut done, mut failed) = (0, 0);
    backups
        .iter()
        .filter(|b| b.trigger == BackupTrigger::Schedule)
        .filter(|b| match b.status {
            BackupStatus::Done => {
                done += 1;
                done > keep
            }
            BackupStatus::Failed => {
                failed += 1;
                failed > keep
            }
            BackupStatus::Running => false,
        })
        .collect()
}

/// Applies retention to each store separately, so moving a schedule to another store never
/// deletes the backups kept in the old one.
async fn prune(state: &AppState, service_id: &str) -> Result<()> {
    let keep = state.db.backup_schedule(service_id).await?.schedule.keep;
    let backups = state.db.backups(service_id).await?;
    let mut stores: Vec<(BackupStorage, Option<&str>)> = Vec::new();
    for b in &backups {
        let store = (b.storage, b.storage_id.as_deref());
        if !stores.contains(&store) {
            stores.push(store);
        }
    }
    for (storage, storage_id) in stores {
        let in_store: Vec<Backup> = backups
            .iter()
            .filter(|b| b.storage == storage && b.storage_id.as_deref() == storage_id)
            .cloned()
            .collect();
        let candidates = prune_candidates(&in_store, keep);
        if candidates.is_empty() {
            continue;
        }
        let store = state.backup_store(storage, storage_id).await?;
        for backup in candidates {
            store
                .delete(&backup_key(&backup.service_id, &backup.file_name))
                .await?;
            state.db.delete_backup(&backup.id).await?;
        }
    }
    Ok(())
}

/// Starts due scheduled backups every few minutes.
pub fn spawn_scheduler(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SCHEDULER_TICK);
        loop {
            tick.tick().await;
            run_due_backups(&state).await;
        }
    });
}

async fn run_due_backups(state: &Arc<AppState>) {
    let now = Utc::now();
    let schedules = match state.db.enabled_backup_schedules().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("cannot read backup schedules: {e:#}");
            return;
        }
    };
    for record in schedules {
        if !is_due(&record.schedule, record.last_run_at, now) {
            continue;
        }
        let Ok(Some(rec)) = state.db.service(&record.service_id).await else {
            continue;
        };
        match start_backup(
            state,
            &rec,
            BackupTrigger::Schedule,
            record.schedule.storage_id.as_deref(),
        )
        .await
        {
            Ok(_) => {}
            // Another backup or a deploy holds the database: try again on the next tick.
            Err(AppError::Conflict(_)) => continue,
            Err(e) => {
                state
                    .service_activity(
                        &rec.service,
                        ActivityLevel::Error,
                        "Scheduled backup skipped".into(),
                        Some(e.to_string()),
                    )
                    .await
            }
        }
        let record = ScheduleRecord {
            last_run_at: Some(now),
            ..record
        };
        if let Err(e) = state.db.save_backup_schedule(&record).await {
            tracing::warn!("cannot save backup schedule: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn creds(user: Option<&str>, db: Option<&str>) -> Credentials {
        Credentials {
            user: user.map(String::from),
            password: "p@ss w/rd".into(),
            database: db.map(String::from),
            root_password: None,
        }
    }

    #[test]
    fn urls_follow_each_engine_and_escape_credentials() {
        let c = creds(Some("app"), Some("app"));
        assert_eq!(
            connection_url(DatabaseKind::Postgres, &c, "db", 5432),
            "postgres://app:p%40ss%20w%2Frd@db:5432/app"
        );
        assert_eq!(
            connection_url(DatabaseKind::Mariadb, &c, "db", 3306),
            "mysql://app:p%40ss%20w%2Frd@db:3306/app"
        );
        assert_eq!(
            connection_url(DatabaseKind::Redis, &creds(None, None), "cache", 6379),
            "redis://:p%40ss%20w%2Frd@cache:6379"
        );
        assert_eq!(
            connection_url(
                DatabaseKind::Mongo,
                &creds(Some("app"), None),
                "mongo",
                27017
            ),
            "mongodb://app:p%40ss%20w%2Frd@mongo:27017/?authSource=admin"
        );
    }

    #[test]
    fn credentials_come_from_env_with_postgres_defaults() {
        let var = |k: &str, v: &str| EnvVar {
            key: k.into(),
            value: v.into(),
            secret: false,
        };
        let pg = Credentials::from_env(
            DatabaseKind::Postgres,
            &[var("POSTGRES_PASSWORD", "secret")],
        );
        assert_eq!(pg.user.as_deref(), Some("postgres"));
        assert_eq!(pg.database.as_deref(), Some("postgres"));
        assert_eq!(pg.password, "secret");
        let mysql = Credentials::from_env(
            DatabaseKind::Mysql,
            &[
                var("MYSQL_USER", "app"),
                var("MYSQL_PASSWORD", "pw"),
                var("MYSQL_DATABASE", "shop"),
                var("MYSQL_ROOT_PASSWORD", "root"),
            ],
        );
        assert_eq!(mysql.root_password.as_deref(), Some("root"));
        assert_eq!(mysql.database.as_deref(), Some("shop"));
    }

    #[test]
    fn info_has_a_public_url_only_when_published() {
        let service = Service {
            id: "s".into(),
            environment_id: "e".into(),
            server_id: "srv".into(),
            name: "db".into(),
            source: Source::Database {
                kind: DatabaseKind::Redis,
                version: "8-alpine".into(),
            },
            port: Some(6379),
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: None,
            created_at: Utc::now(),
            state: ServiceState::Running,
            primary_domain: None,
            last_deployed_at: None,
            live_deployment_id: None,
        };
        let server = Server {
            id: "srv".into(),
            name: "edge".into(),
            kind: ServerKind::Ssh,
            host: Some("203.0.113.7".into()),
            user: None,
            port: None,
            ssh_key_secret_id: None,
            status: ServerStatus::Online,
            proxy: ProxyStatus::Running,
            docker_version: None,
            last_checked_at: None,
            created_at: Utc::now(),
        };
        let vars = [EnvVar {
            key: "REDIS_PASSWORD".into(),
            value: "pw".into(),
            secret: true,
        }];
        let internal = info(&service, &server, &vars, None).unwrap();
        assert_eq!(internal.internal_url, "redis://:pw@db:6379");
        assert_eq!(internal.username, None);
        assert_eq!(internal.public_url, None);
        let public = info(&service, &server, &vars, Some(16379)).unwrap();
        assert_eq!(
            public.public_url.as_deref(),
            Some("redis://:pw@203.0.113.7:16379")
        );
    }

    #[test]
    fn schedules_are_due_once_per_daily_slot() {
        let schedule = BackupSchedule {
            enabled: true,
            hour_utc: 3,
            keep: 7,
            storage_id: None,
        };
        let now = at("2026-11-20T03:10:00Z");
        assert!(is_due(&schedule, None, now));
        assert!(is_due(&schedule, Some(at("2026-11-19T03:00:00Z")), now));
        assert!(!is_due(&schedule, Some(at("2026-11-20T03:05:00Z")), now));
        // Before today's slot, yesterday's slot counts.
        let early = at("2026-11-20T02:00:00Z");
        assert!(!is_due(&schedule, Some(at("2026-11-19T03:00:01Z")), early));
        assert!(is_due(&schedule, Some(at("2026-11-18T23:00:00Z")), early));
        // Enabling stamps last_run_at, so the first run waits for the next slot.
        assert!(!is_due(
            &schedule,
            Some(at("2026-11-20T10:00:00Z")),
            at("2026-11-20T12:00:00Z")
        ));
        assert!(is_due(
            &schedule,
            Some(at("2026-11-20T10:00:00Z")),
            at("2026-11-21T03:00:00Z")
        ));
        let off = BackupSchedule {
            enabled: false,
            ..schedule
        };
        assert!(!is_due(&off, None, now));
    }

    #[test]
    fn pruning_keeps_the_newest_scheduled_backups() {
        let backup = |n: u32, trigger, status| Backup {
            id: n.to_string(),
            service_id: "s".into(),
            created_at: Utc::now(),
            size_bytes: 1,
            status,
            trigger,
            file_name: format!("{n}.sql.gz"),
            error: None,
            storage: BackupStorage::Local,
            storage_id: None,
            finished_at: None,
        };
        use BackupStatus::{Done, Failed, Running};
        use BackupTrigger::{Manual, Schedule};
        let newest_first = [
            backup(9, Schedule, Running),
            backup(8, Schedule, Failed),
            backup(7, Schedule, Done),
            backup(6, Manual, Done),
            backup(5, Schedule, Failed),
            backup(4, Schedule, Failed),
            backup(3, Schedule, Done),
            backup(2, Schedule, Done),
            backup(1, Manual, Done),
        ];
        let pruned: Vec<&str> = prune_candidates(&newest_first, 2)
            .into_iter()
            .map(|b| b.id.as_str())
            .collect();
        assert_eq!(pruned, ["4", "2"]);
        assert!(prune_candidates(&newest_first, 10).is_empty());
    }

    #[test]
    fn sql_strings_are_quoted() {
        assert_eq!(sql_quote("it's"), "'it''s'");
        assert_eq!(sql_quote("a\\b"), "'a\\\\b'");
    }
}
