//! One-time import of the SQLite store older daemons kept (`<data dir>/<dir>.db`) into Postgres.

use crate::db::Db;
use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, MAIN_DB};
use sqlx::AssertSqlSafe;
use sqlx::types::Json;
use std::path::{Path, PathBuf};

/// The SQLite schema history, so files from any earlier release can be read.
const SQLITE_MIGRATIONS: &[&str] = &[
    r"
CREATE TABLE servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    host TEXT,
    user TEXT,
    port INTEGER,
    status TEXT NOT NULL DEFAULT 'unknown',
    proxy TEXT NOT NULL DEFAULT 'missing',
    docker_version TEXT,
    last_checked_at TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE environments (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (project_id, name)
);
CREATE TABLE services (
    id TEXT PRIMARY KEY,
    environment_id TEXT NOT NULL REFERENCES environments(id) ON DELETE CASCADE,
    server_id TEXT NOT NULL REFERENCES servers(id),
    name TEXT NOT NULL,
    source TEXT NOT NULL,
    port INTEGER,
    command TEXT,
    restart TEXT NOT NULL,
    health_path TEXT,
    stopped INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    UNIQUE (environment_id, name)
);
CREATE TABLE env_vars (
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    secret INTEGER NOT NULL,
    PRIMARY KEY (service_id, key)
);
CREATE TABLE domains (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    host TEXT NOT NULL UNIQUE,
    port INTEGER,
    tls INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE volumes (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    mount_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (service_id, name)
);
CREATE TABLE deployments (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    server_id TEXT NOT NULL,
    number INTEGER NOT NULL,
    status TEXT NOT NULL,
    trigger TEXT NOT NULL,
    commit_sha TEXT,
    commit_message TEXT,
    image TEXT,
    container_id TEXT,
    steps TEXT NOT NULL,
    error TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT,
    UNIQUE (service_id, number)
);
CREATE INDEX deployments_status ON deployments(status);
CREATE TABLE activity (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    level TEXT NOT NULL,
    service_id TEXT,
    service_name TEXT,
    project_id TEXT,
    message TEXT NOT NULL,
    detail TEXT
);
",
    r"
ALTER TABLE services ADD COLUMN public_port INTEGER;
CREATE TABLE backups (
    id TEXT PRIMARY KEY,
    service_id TEXT NOT NULL REFERENCES services(id) ON DELETE CASCADE,
    number INTEGER NOT NULL,
    status TEXT NOT NULL,
    trigger TEXT NOT NULL,
    file_name TEXT NOT NULL,
    size_bytes INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    storage TEXT NOT NULL DEFAULT 'local',
    storage_id TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT,
    UNIQUE (service_id, number)
);
CREATE INDEX backups_status ON backups(status);
CREATE TABLE backup_schedules (
    service_id TEXT PRIMARY KEY REFERENCES services(id) ON DELETE CASCADE,
    enabled INTEGER NOT NULL,
    hour_utc INTEGER NOT NULL,
    keep INTEGER NOT NULL,
    last_run_at TEXT
);
",
];

/// Every table, parents first, with the columns to copy and how to convert them.
const TABLES: &[(&str, &[(&str, Kind)])] = &[
    (
        "servers",
        &[
            ("id", Kind::Text),
            ("name", Kind::Text),
            ("kind", Kind::Text),
            ("host", Kind::Text),
            ("user", Kind::Text),
            ("port", Kind::Int4),
            ("status", Kind::Text),
            ("proxy", Kind::Text),
            ("docker_version", Kind::Text),
            ("last_checked_at", Kind::Time),
            ("created_at", Kind::Time),
        ],
    ),
    (
        "projects",
        &[
            ("id", Kind::Text),
            ("name", Kind::Text),
            ("description", Kind::Text),
            ("created_at", Kind::Time),
        ],
    ),
    (
        "environments",
        &[
            ("id", Kind::Text),
            ("project_id", Kind::Text),
            ("name", Kind::Text),
            ("created_at", Kind::Time),
        ],
    ),
    (
        "services",
        &[
            ("id", Kind::Text),
            ("environment_id", Kind::Text),
            ("server_id", Kind::Text),
            ("name", Kind::Text),
            ("source", Kind::Json),
            ("port", Kind::Int4),
            ("command", Kind::Text),
            ("restart", Kind::Text),
            ("health_path", Kind::Text),
            ("stopped", Kind::Bool),
            ("public_port", Kind::Int4),
            ("created_at", Kind::Time),
        ],
    ),
    (
        "env_vars",
        &[
            ("service_id", Kind::Text),
            ("position", Kind::Int4),
            ("key", Kind::Text),
            ("value", Kind::Text),
            ("secret", Kind::Bool),
        ],
    ),
    (
        "domains",
        &[
            ("id", Kind::Text),
            ("service_id", Kind::Text),
            ("host", Kind::Text),
            ("port", Kind::Int4),
            ("tls", Kind::Bool),
            ("created_at", Kind::Time),
        ],
    ),
    (
        "volumes",
        &[
            ("id", Kind::Text),
            ("service_id", Kind::Text),
            ("name", Kind::Text),
            ("mount_path", Kind::Text),
            ("created_at", Kind::Time),
        ],
    ),
    (
        "deployments",
        &[
            ("id", Kind::Text),
            ("service_id", Kind::Text),
            ("server_id", Kind::Text),
            ("number", Kind::Int8),
            ("status", Kind::Text),
            ("trigger", Kind::Text),
            ("commit_sha", Kind::Text),
            ("commit_message", Kind::Text),
            ("image", Kind::Text),
            ("container_id", Kind::Text),
            ("steps", Kind::Json),
            ("error", Kind::Text),
            ("created_at", Kind::Time),
            ("finished_at", Kind::Time),
        ],
    ),
    (
        "activity",
        &[
            ("id", Kind::Int8),
            ("at", Kind::Time),
            ("level", Kind::Text),
            ("service_id", Kind::Text),
            ("service_name", Kind::Text),
            ("project_id", Kind::Text),
            ("message", Kind::Text),
            ("detail", Kind::Text),
        ],
    ),
    (
        "backups",
        &[
            ("id", Kind::Text),
            ("service_id", Kind::Text),
            ("number", Kind::Int8),
            ("status", Kind::Text),
            ("trigger", Kind::Text),
            ("file_name", Kind::Text),
            ("size_bytes", Kind::Int8),
            ("error", Kind::Text),
            ("storage", Kind::Text),
            ("storage_id", Kind::Text),
            ("created_at", Kind::Time),
            ("finished_at", Kind::Time),
        ],
    ),
    (
        "backup_schedules",
        &[
            ("service_id", Kind::Text),
            ("enabled", Kind::Bool),
            ("hour_utc", Kind::Int4),
            ("keep", Kind::Int8),
            ("last_run_at", Kind::Time),
        ],
    ),
];

#[derive(Debug, Clone, Copy)]
enum Kind {
    Text,
    Int4,
    Int8,
    Bool,
    Time,
    Json,
}

#[derive(Debug)]
enum Value {
    Text(Option<String>),
    Int4(Option<i32>),
    Int8(Option<i64>),
    Bool(Option<bool>),
    Time(Option<DateTime<Utc>>),
    Json(Option<serde_json::Value>),
}

/// What an import did.
#[derive(Debug)]
pub struct Imported {
    pub rows: usize,
    /// Where the SQLite file was moved to.
    pub backup: PathBuf,
}

/// Run on every start: imports `path` when it exists and the Postgres store is still empty.
pub async fn import_if_present(db: &Db, path: &Path) -> Result<Option<Imported>> {
    if !path.exists() {
        return Ok(None);
    }
    if !db.is_empty().await? {
        tracing::warn!(
            "{} exists but the Postgres store already has data; leaving the file alone",
            path.display()
        );
        return Ok(None);
    }
    import(db, path).await.map(Some)
}

/// Copies every row of the SQLite store at `path` into the empty Postgres store in one
/// transaction, checks the row counts, then renames the file to
/// `<name>.migrated-<timestamp>.bak`. On any error nothing is committed and the file is left
/// where it was.
pub async fn import(db: &Db, path: &Path) -> Result<Imported> {
    if !db.is_empty().await? {
        bail!(
            "the Postgres store already has data; refusing to import {} over it",
            path.display()
        );
    }
    let source = path.to_path_buf();
    let tables = tokio::task::spawn_blocking(move || read_sqlite(&source)).await??;

    let mut tx = db.pool().begin().await?;
    let mut rows = 0;
    for ((table, columns), values) in TABLES.iter().zip(&tables) {
        let names = columns
            .iter()
            .map(|(name, _)| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let params = (1..=columns.len())
            .map(|i| format!("${i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("INSERT INTO {table} ({names}) VALUES ({params})");
        for row in values {
            let mut query = sqlx::query(AssertSqlSafe(sql.as_str()));
            for value in row {
                query = match value {
                    Value::Text(v) => query.bind(v.clone()),
                    Value::Int4(v) => query.bind(*v),
                    Value::Int8(v) => query.bind(*v),
                    Value::Bool(v) => query.bind(*v),
                    Value::Time(v) => query.bind(*v),
                    Value::Json(v) => query.bind(v.clone().map(Json)),
                };
            }
            query
                .execute(&mut *tx)
                .await
                .with_context(|| format!("cannot insert into {table}"))?;
        }
        let count: i64 = sqlx::query_scalar(AssertSqlSafe(format!("SELECT COUNT(*) FROM {table}")))
            .fetch_one(&mut *tx)
            .await?;
        if usize::try_from(count).ok() != Some(values.len()) {
            bail!(
                "{table}: copied {count} rows but SQLite has {}",
                values.len()
            );
        }
        rows += values.len();
    }
    sqlx::query(
        "SELECT setval(pg_get_serial_sequence('activity', 'id'), COALESCE(MAX(id), 0) + 1, false) FROM activity",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let backup = backup_path(path);
    std::fs::rename(path, &backup).with_context(|| {
        format!(
            "imported {} but cannot rename it to {}",
            path.display(),
            backup.display()
        )
    })?;
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", path.display()));
        if side.exists() {
            let _ = std::fs::rename(&side, format!("{}{suffix}", backup.display()));
        }
    }
    Ok(Imported { rows, backup })
}

fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!(
        "{}.migrated-{}.bak",
        path.display(),
        Utc::now().format("%Y%m%d-%H%M%S")
    ))
}

/// Reads every table of the SQLite file into memory. The file is copied into an in-memory
/// database and migrated to the last SQLite schema there, so older files import too and the
/// original is never written to.
fn read_sqlite(path: &Path) -> Result<Vec<Vec<Vec<Value>>>> {
    let mut conn = Connection::open_in_memory()?;
    conn.restore(MAIN_DB, path, None::<fn(rusqlite::backup::Progress)>)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let version: usize = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > SQLITE_MIGRATIONS.len() {
        bail!(
            "{} has schema version {version}, newer than this daemon knows",
            path.display()
        );
    }
    for (i, sql) in SQLITE_MIGRATIONS.iter().enumerate().skip(version) {
        conn.execute_batch(&format!(
            "BEGIN; {sql} PRAGMA user_version = {}; COMMIT;",
            i + 1
        ))
        .with_context(|| {
            format!(
                "cannot upgrade {} to SQLite schema {}",
                path.display(),
                i + 1
            )
        })?;
    }
    TABLES
        .iter()
        .map(|(table, columns)| read_table(&conn, table, columns))
        .collect()
}

fn read_table(conn: &Connection, table: &str, columns: &[(&str, Kind)]) -> Result<Vec<Vec<Value>>> {
    let names = columns
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!("SELECT {names} FROM {table} ORDER BY rowid"))?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let values = columns
            .iter()
            .enumerate()
            .map(|(i, (name, kind))| {
                read_value(row, i, *kind).with_context(|| format!("{table}.{name}"))
            })
            .collect::<Result<_>>()?;
        out.push(values);
    }
    Ok(out)
}

fn read_value(row: &rusqlite::Row<'_>, i: usize, kind: Kind) -> Result<Value> {
    Ok(match kind {
        Kind::Text => Value::Text(row.get(i)?),
        Kind::Int4 => Value::Int4(row.get(i)?),
        Kind::Int8 => Value::Int8(row.get(i)?),
        Kind::Bool => Value::Bool(row.get(i)?),
        Kind::Time => Value::Time(row.get(i)?),
        Kind::Json => Value::Json(
            row.get::<_, Option<String>>(i)?
                .map(|s| serde_json::from_str(&s))
                .transpose()?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::test_db;
    use crate::db::{ScheduleRecord, new_id};
    use chrono::SubsecRound as _;
    use common::*;
    use rusqlite::params;

    fn tag<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    fn sqlite_at(path: &Path, version: usize) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .unwrap();
        for (i, sql) in SQLITE_MIGRATIONS.iter().take(version).enumerate() {
            conn.execute_batch(&format!(
                "BEGIN; {sql} PRAGMA user_version = {}; COMMIT;",
                i + 1
            ))
            .unwrap();
        }
        conn
    }

    fn insert_server(conn: &Connection, s: &Server) {
        conn.execute(
            "INSERT INTO servers (id, name, kind, host, user, port, status, proxy, docker_version, last_checked_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![s.id, s.name, tag(&s.kind), s.host, s.user, s.port, tag(&s.status), tag(&s.proxy),
                    s.docker_version, s.last_checked_at, s.created_at],
        )
        .unwrap();
    }

    fn insert_project(conn: &Connection, p: &Project, e: &Environment) {
        conn.execute(
            "INSERT INTO projects (id, name, description, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![p.id, p.name, p.description, p.created_at],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO environments (id, project_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![e.id, e.project_id, e.name, e.created_at],
        )
        .unwrap();
    }

    fn insert_service(conn: &Connection, s: &Service, stopped: bool) {
        conn.execute(
            "INSERT INTO services (id, environment_id, server_id, name, source, port, command, restart, health_path, stopped, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![s.id, s.environment_id, s.server_id, s.name, serde_json::to_string(&s.source).unwrap(),
                    s.port, s.command, tag(&s.restart), s.health_path, stopped, s.created_at],
        )
        .unwrap();
    }

    struct Fixture {
        servers: Vec<Server>,
        project: Project,
        env: Environment,
        services: Vec<Service>,
        env_vars: Vec<EnvVar>,
        domains: Vec<Domain>,
        volume: Volume,
        deployment: Deployment,
        activity: Vec<Activity>,
        backup: Backup,
        schedule: ScheduleRecord,
    }

    /// One or more rows in every table of a version-2 file.
    fn version_two_file(path: &Path) -> Fixture {
        let at = |s: &str| -> Timestamp { s.parse::<Timestamp>().unwrap().trunc_subsecs(6) };
        let created = at("2026-11-20T10:00:00.123456789Z");
        let servers = vec![
            Server {
                id: new_id(),
                name: "local".into(),
                kind: ServerKind::Local,
                host: None,
                user: None,
                port: None,
                status: ServerStatus::Online,
                proxy: ProxyStatus::Running,
                docker_version: Some("29.0.0".into()),
                last_checked_at: Some(at("2026-11-20T11:00:00.5Z")),
                created_at: created,
            },
            Server {
                id: new_id(),
                name: "edge".into(),
                kind: ServerKind::Ssh,
                host: Some("10.0.0.2".into()),
                user: Some("deploy".into()),
                port: Some(2222),
                status: ServerStatus::Offline,
                proxy: ProxyStatus::Missing,
                docker_version: None,
                last_checked_at: None,
                created_at: created,
            },
        ];
        let project = Project {
            id: new_id(),
            name: "shop".into(),
            description: Some("the store".into()),
            created_at: created,
        };
        let env = Environment {
            id: new_id(),
            project_id: project.id.clone(),
            name: "production".into(),
            created_at: created,
        };
        let service = |name: &str, source: Source, port| Service {
            id: new_id(),
            environment_id: env.id.clone(),
            server_id: servers[0].id.clone(),
            name: name.into(),
            source,
            port,
            command: Some("serve --fast".into()),
            restart: RestartPolicy::Always,
            health_path: Some("/up".into()),
            created_at: created,
            state: ServiceState::New,
            primary_domain: None,
            last_deployed_at: None,
            live_deployment_id: None,
        };
        let services = vec![
            service(
                "web",
                Source::Image {
                    image: "nginx:1.27".into(),
                },
                Some(80),
            ),
            service(
                "db",
                Source::Database {
                    kind: DatabaseKind::Postgres,
                    version: "18-alpine".into(),
                },
                None,
            ),
        ];
        let env_vars = vec![
            EnvVar {
                key: "Z_LAST".into(),
                value: "1".into(),
                secret: false,
            },
            EnvVar {
                key: "A_FIRST".into(),
                value: "s3cret".into(),
                secret: true,
            },
        ];
        let domains = ["b.example.com", "a.example.com"]
            .map(|host| Domain {
                id: new_id(),
                service_id: services[0].id.clone(),
                host: host.into(),
                port: Some(8080),
                tls: true,
                created_at: created,
            })
            .to_vec();
        let volume = Volume {
            id: new_id(),
            service_id: services[1].id.clone(),
            name: "data".into(),
            mount_path: "/var/lib/postgresql".into(),
            created_at: created,
        };
        let deployment = Deployment {
            id: new_id(),
            service_id: services[0].id.clone(),
            number: 3,
            status: DeploymentStatus::Live,
            trigger: "api".into(),
            commit: Some("abc123".into()),
            commit_message: Some("ship it".into()),
            image: Some("nginx:1.27".into()),
            container_id: Some("c0ffee".into()),
            steps: vec![Step {
                kind: StepKind::Pull,
                status: StepStatus::Done,
                detail: Some("pulled".into()),
                started_at: Some(created),
                finished_at: Some(at("2026-11-20T10:00:05Z")),
            }],
            error: None,
            created_at: created,
            finished_at: Some(at("2026-11-20T10:00:06.000001Z")),
        };
        let activity = [5, 9]
            .map(|id| Activity {
                id,
                at: created,
                level: ActivityLevel::Success,
                service_id: Some(services[0].id.clone()),
                service_name: Some("web".into()),
                project_id: Some(project.id.clone()),
                message: format!("event {id}"),
                detail: Some("detail".into()),
            })
            .to_vec();
        let backup = Backup {
            id: new_id(),
            service_id: services[1].id.clone(),
            created_at: created,
            size_bytes: 5_000_000_000,
            status: BackupStatus::Done,
            trigger: BackupTrigger::Schedule,
            file_name: "20261120-100000-1.sql.gz".into(),
            error: None,
            storage: BackupStorage::Local,
            storage_id: None,
            finished_at: Some(created),
        };
        let schedule = ScheduleRecord {
            service_id: services[1].id.clone(),
            schedule: BackupSchedule {
                enabled: true,
                hour_utc: 4,
                keep: 14,
            },
            last_run_at: Some(created),
        };

        let conn = sqlite_at(path, 2);
        for s in &servers {
            insert_server(&conn, s);
        }
        insert_project(&conn, &project, &env);
        for s in &services {
            insert_service(&conn, s, s.name == "web");
        }
        conn.execute(
            "UPDATE services SET public_port = 15432 WHERE id = ?1",
            [&services[1].id],
        )
        .unwrap();
        for (position, v) in env_vars.iter().enumerate() {
            conn.execute(
                "INSERT INTO env_vars (service_id, position, key, value, secret) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![services[0].id, position, v.key, v.value, v.secret],
            )
            .unwrap();
        }
        for d in &domains {
            conn.execute(
                "INSERT INTO domains (id, service_id, host, port, tls, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![d.id, d.service_id, d.host, d.port, d.tls, d.created_at],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO volumes (id, service_id, name, mount_path, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![volume.id, volume.service_id, volume.name, volume.mount_path, volume.created_at],
        )
        .unwrap();
        let d = &deployment;
        conn.execute(
            "INSERT INTO deployments (id, service_id, server_id, number, status, trigger, commit_sha, commit_message,
                                      image, container_id, steps, error, created_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![d.id, d.service_id, servers[0].id, d.number, tag(&d.status), d.trigger, d.commit,
                    d.commit_message, d.image, d.container_id, serde_json::to_string(&d.steps).unwrap(),
                    d.error, d.created_at, d.finished_at],
        )
        .unwrap();
        for a in &activity {
            conn.execute(
                "INSERT INTO activity (id, at, level, service_id, service_name, project_id, message, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![a.id, a.at, tag(&a.level), a.service_id, a.service_name, a.project_id, a.message, a.detail],
            )
            .unwrap();
        }
        let b = &backup;
        conn.execute(
            "INSERT INTO backups (id, service_id, number, status, trigger, file_name, size_bytes, error,
                                  storage, storage_id, created_at, finished_at)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![b.id, b.service_id, tag(&b.status), tag(&b.trigger), b.file_name,
                    i64::try_from(b.size_bytes).unwrap(), b.error, tag(&b.storage), b.storage_id,
                    b.created_at, b.finished_at],
        )
        .unwrap();
        let s = &schedule;
        conn.execute(
            "INSERT INTO backup_schedules (service_id, enabled, hour_utc, keep, last_run_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![s.service_id, s.schedule.enabled, s.schedule.hour_utc, s.schedule.keep, s.last_run_at],
        )
        .unwrap();
        Fixture {
            servers,
            project,
            env,
            services,
            env_vars,
            domains,
            volume,
            deployment,
            activity,
            backup,
            schedule,
        }
    }

    #[tokio::test]
    async fn version_two_files_import_with_every_row() {
        let Some(db) = test_db().await else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let f = version_two_file(&path);

        let imported = import(&db, &path).await.unwrap();
        assert_eq!(imported.rows, 2 + 1 + 1 + 2 + 2 + 2 + 1 + 1 + 2 + 1 + 1);
        assert!(!path.exists());
        assert!(imported.backup.exists());
        assert!(
            imported
                .backup
                .to_string_lossy()
                .contains("store.db.migrated-")
        );

        assert_eq!(db.servers().await.unwrap(), f.servers);
        assert_eq!(
            db.projects().await.unwrap(),
            std::slice::from_ref(&f.project)
        );
        assert_eq!(
            db.environments().await.unwrap(),
            std::slice::from_ref(&f.env)
        );
        let recs = db.services().await.unwrap();
        assert_eq!(recs.len(), 2);
        let web = &recs[0];
        assert_eq!(web.service.name, "web");
        assert!(web.stopped);
        assert_eq!(web.service.source, f.services[0].source);
        assert_eq!(web.service.port, Some(80));
        assert_eq!(web.service.command, f.services[0].command);
        assert_eq!(web.service.health_path, f.services[0].health_path);
        assert_eq!(web.service.created_at, f.services[0].created_at);
        assert_eq!(web.service.primary_domain.as_deref(), Some("b.example.com"));
        assert_eq!(
            web.service.live_deployment_id.as_deref(),
            Some(f.deployment.id.as_str())
        );
        assert_eq!(recs[1].service.source, f.services[1].source);
        assert!(!recs[1].stopped);
        assert_eq!(
            db.public_port(&f.services[1].id).await.unwrap(),
            Some(15432)
        );
        assert_eq!(db.env(&f.services[0].id).await.unwrap(), f.env_vars);
        assert_eq!(db.domains(&f.services[0].id).await.unwrap(), f.domains);
        assert_eq!(
            db.volumes(&f.services[1].id).await.unwrap(),
            std::slice::from_ref(&f.volume)
        );
        assert_eq!(
            db.deployment(&f.deployment.id).await.unwrap(),
            Some(f.deployment.clone())
        );
        assert_eq!(
            db.backups(&f.services[1].id).await.unwrap(),
            std::slice::from_ref(&f.backup)
        );
        let schedule = db.backup_schedule(&f.services[1].id).await.unwrap();
        assert_eq!(schedule.schedule, f.schedule.schedule);
        assert_eq!(schedule.last_run_at, f.schedule.last_run_at);
        let mut activity = f.activity.clone();
        activity.reverse();
        assert_eq!(db.activity(10).await.unwrap(), activity);

        let mut d = f.deployment.clone();
        d.id = new_id();
        db.insert_deployment(&mut d, &f.servers[0].id)
            .await
            .unwrap();
        assert_eq!(d.number, 4, "numbering continues after imported rows");
        db.insert_activity(&Activity {
            message: "after".into(),
            ..f.activity[0].clone()
        })
        .await
        .unwrap();
        assert_eq!(db.activity(1).await.unwrap()[0].id, 10);
    }

    #[tokio::test]
    async fn version_one_files_import() {
        let Some(db) = test_db().await else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let conn = sqlite_at(&path, 1);
        let server = Server {
            id: new_id(),
            name: "local".into(),
            kind: ServerKind::Local,
            host: None,
            user: None,
            port: None,
            status: ServerStatus::Unknown,
            proxy: ProxyStatus::Missing,
            docker_version: None,
            last_checked_at: None,
            created_at: Utc::now().trunc_subsecs(6),
        };
        insert_server(&conn, &server);
        drop(conn);

        let imported = import_if_present(&db, &path).await.unwrap().unwrap();
        assert_eq!(imported.rows, 1);
        assert_eq!(db.servers().await.unwrap(), [server]);
        assert!(import_if_present(&db, &path).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn failed_imports_leave_sqlite_and_postgres_untouched() {
        let Some(db) = test_db().await else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let f = version_two_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE deployments SET steps = 'not json' WHERE id = ?1",
            [&f.deployment.id],
        )
        .unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();

        let err = import(&db, &path).await.unwrap_err();
        assert!(format!("{err:#}").contains("deployments.steps"), "{err:#}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(db.is_empty().await.unwrap());
    }

    #[tokio::test]
    async fn imports_refuse_a_store_with_data() {
        let Some(db) = test_db().await else { return };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.db");
        let f = version_two_file(&path);
        db.insert_server(&f.servers[0]).await.unwrap();

        let err = import(&db, &path).await.unwrap_err();
        assert!(err.to_string().contains("already has data"), "{err}");
        assert!(import_if_present(&db, &path).await.unwrap().is_none());
        assert!(path.exists());
    }
}
