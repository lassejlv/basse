//! SQLite storage: schema, migrations and every query the daemon runs.

use anyhow::{Context as _, Result};
use chrono::Utc;
use common::*;
use rand::Rng as _;
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension as _, Row, Transaction, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

const MIGRATIONS: &[&str] = &[r"
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
"];

/// A fresh identifier: 12 lowercase alphanumerics starting with a letter.
pub fn new_id() -> Id {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    let mut id = String::with_capacity(12);
    id.push(char::from(ALPHABET[rng.random_range(0..26)]));
    for _ in 1..12 {
        id.push(char::from(ALPHABET[rng.random_range(0..ALPHABET.len())]));
    }
    id
}

/// A service row plus the stored and derived facts the API computes state from.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    /// Computed fields other than `state` are filled in.
    pub service: Service,
    /// Set by `/stop`, cleared by `/start` and deploys.
    pub stopped: bool,
    /// Status of the oldest unfinished deployment, if any.
    pub busy: Option<DeploymentStatus>,
    pub live_container: Option<String>,
    /// Whether any deployment of the service ever failed.
    pub ever_failed: bool,
}

/// New service with everything inserted alongside it.
pub struct NewService<'a> {
    pub service: &'a Service,
    pub env: &'a [EnvVar],
    pub domains: &'a [Domain],
    pub volumes: &'a [Volume],
}

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("cannot open database {}", path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;",
        )?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn tx<T>(&self, f: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>) -> Result<T> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    // Servers

    pub fn servers(&self) -> Result<Vec<Server>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!("{SERVER_SELECT} ORDER BY created_at, rowid"))?;
        let rows = stmt.query_map([], server_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn server(&self, id: &str) -> Result<Option<Server>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                &format!("{SERVER_SELECT} WHERE id = ?1"),
                [id],
                server_from_row,
            )
            .optional()?)
    }

    pub fn local_server(&self) -> Result<Option<Server>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                &format!("{SERVER_SELECT} WHERE kind = 'local' ORDER BY created_at LIMIT 1"),
                [],
                server_from_row,
            )
            .optional()?)
    }

    pub fn server_name_taken(&self, name: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM servers WHERE name = ?1", [name])
    }

    pub fn insert_server(&self, s: &Server) -> Result<()> {
        self.lock().execute(
            "INSERT INTO servers (id, name, kind, host, user, port, status, proxy, docker_version, last_checked_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                s.id,
                s.name,
                enum_str(&s.kind),
                s.host,
                s.user,
                s.port,
                enum_str(&s.status),
                enum_str(&s.proxy),
                s.docker_version,
                s.last_checked_at,
                s.created_at
            ],
        )?;
        Ok(())
    }

    pub fn update_server_check(&self, s: &Server) -> Result<()> {
        self.lock().execute(
            "UPDATE servers SET status = ?2, proxy = ?3, docker_version = ?4, last_checked_at = ?5 WHERE id = ?1",
            params![
                s.id,
                enum_str(&s.status),
                enum_str(&s.proxy),
                s.docker_version,
                s.last_checked_at
            ],
        )?;
        Ok(())
    }

    pub fn server_in_use(&self, id: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM services WHERE server_id = ?1", [id])
    }

    pub fn delete_server(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM servers WHERE id = ?1", [id])?;
        Ok(())
    }

    // Projects and environments

    pub fn projects(&self) -> Result<Vec<Project>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, created_at FROM projects ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map([], project_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn project(&self, id: &str) -> Result<Option<Project>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT id, name, description, created_at FROM projects WHERE id = ?1",
                [id],
                project_from_row,
            )
            .optional()?)
    }

    pub fn project_name_taken(&self, name: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM projects WHERE name = ?1", [name])
    }

    /// Inserts the project and its first environment together.
    pub fn insert_project(&self, p: &Project, first_env: &Environment) -> Result<()> {
        self.tx(|tx| {
            tx.execute(
                "INSERT INTO projects (id, name, description, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![p.id, p.name, p.description, p.created_at],
            )?;
            insert_environment(tx, first_env)
        })
    }

    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn environments(&self) -> Result<Vec<Environment>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, project_id, name, created_at FROM environments ORDER BY created_at, rowid",
        )?;
        let rows = stmt.query_map([], environment_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn environment(&self, id: &str) -> Result<Option<Environment>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT id, project_id, name, created_at FROM environments WHERE id = ?1",
                [id],
                environment_from_row,
            )
            .optional()?)
    }

    pub fn environment_name_taken(&self, project_id: &str, name: &str) -> Result<bool> {
        self.exists(
            "SELECT 1 FROM environments WHERE project_id = ?1 AND name = ?2",
            [project_id, name],
        )
    }

    pub fn insert_environment(&self, e: &Environment) -> Result<()> {
        self.tx(|tx| insert_environment(tx, e))
    }

    pub fn delete_environment(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM environments WHERE id = ?1", [id])?;
        Ok(())
    }

    // Services

    pub fn services(&self) -> Result<Vec<ServiceRecord>> {
        self.service_query("ORDER BY s.created_at, s.rowid", [])
    }

    pub fn services_in_environment(&self, env_id: &str) -> Result<Vec<ServiceRecord>> {
        self.service_query(
            "WHERE s.environment_id = ?1 ORDER BY s.created_at",
            [env_id],
        )
    }

    pub fn service(&self, id: &str) -> Result<Option<ServiceRecord>> {
        Ok(self
            .service_query("WHERE s.id = ?1", [id])?
            .into_iter()
            .next())
    }

    fn service_query<P: rusqlite::Params>(
        &self,
        tail: &str,
        params: P,
    ) -> Result<Vec<ServiceRecord>> {
        let conn = self.lock();
        let sql = format!(
            "SELECT s.id, s.environment_id, s.server_id, s.name, s.source, s.port, s.command, s.restart,
                    s.health_path, s.created_at, s.stopped,
                    (SELECT host FROM domains d WHERE d.service_id = s.id ORDER BY d.created_at, d.rowid LIMIT 1),
                    (SELECT id FROM deployments WHERE service_id = s.id AND status = 'live' ORDER BY number DESC LIMIT 1),
                    (SELECT container_id FROM deployments WHERE service_id = s.id AND status = 'live' ORDER BY number DESC LIMIT 1),
                    (SELECT MAX(finished_at) FROM deployments WHERE service_id = s.id AND status IN ('live', 'superseded')),
                    (SELECT status FROM deployments WHERE service_id = s.id AND status IN ('queued', 'building', 'starting') ORDER BY number LIMIT 1),
                    EXISTS (SELECT 1 FROM deployments WHERE service_id = s.id AND status = 'failed')
             FROM services s {tail}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params, |row| {
            let service = Service {
                id: row.get(0)?,
                environment_id: row.get(1)?,
                server_id: row.get(2)?,
                name: row.get(3)?,
                source: col_json(row, 4)?,
                port: row.get(5)?,
                command: row.get(6)?,
                restart: col_enum(row, 7)?,
                health_path: row.get(8)?,
                created_at: row.get(9)?,
                state: ServiceState::New,
                primary_domain: row.get(11)?,
                live_deployment_id: row.get(12)?,
                last_deployed_at: row.get(14)?,
            };
            let busy: Option<String> = row.get(15)?;
            Ok(ServiceRecord {
                service,
                stopped: row.get(10)?,
                live_container: row.get(13)?,
                busy: busy.map(|s| parse_enum(&s, 15)).transpose()?,
                ever_failed: row.get(16)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn service_name_taken(
        &self,
        env_id: &str,
        name: &str,
        except: Option<&str>,
    ) -> Result<bool> {
        self.exists(
            "SELECT 1 FROM services WHERE environment_id = ?1 AND name = ?2 AND id != ?3",
            params![env_id, name, except.unwrap_or("")],
        )
    }

    pub fn insert_service(&self, new: &NewService<'_>) -> Result<()> {
        let s = new.service;
        self.tx(|tx| {
            tx.execute(
                "INSERT INTO services (id, environment_id, server_id, name, source, port, command, restart, health_path, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    s.id,
                    s.environment_id,
                    s.server_id,
                    s.name,
                    json(&s.source),
                    s.port,
                    s.command,
                    enum_str(&s.restart),
                    s.health_path,
                    s.created_at
                ],
            )?;
            replace_env(tx, &s.id, new.env)?;
            for d in new.domains {
                insert_domain(tx, d)?;
            }
            for v in new.volumes {
                insert_volume(tx, v)?;
            }
            Ok(())
        })
    }

    pub fn update_service(&self, s: &Service) -> Result<()> {
        self.lock().execute(
            "UPDATE services SET name = ?2, source = ?3, port = ?4, command = ?5, restart = ?6,
                    health_path = ?7, server_id = ?8 WHERE id = ?1",
            params![
                s.id,
                s.name,
                json(&s.source),
                s.port,
                s.command,
                enum_str(&s.restart),
                s.health_path,
                s.server_id
            ],
        )?;
        Ok(())
    }

    pub fn set_stopped(&self, service_id: &str, stopped: bool) -> Result<()> {
        self.lock().execute(
            "UPDATE services SET stopped = ?2 WHERE id = ?1",
            params![service_id, stopped],
        )?;
        Ok(())
    }

    pub fn delete_service(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM services WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn env(&self, service_id: &str) -> Result<Vec<EnvVar>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT key, value, secret FROM env_vars WHERE service_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map([service_id], |row| {
            Ok(EnvVar {
                key: row.get(0)?,
                value: row.get(1)?,
                secret: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn replace_env(&self, service_id: &str, vars: &[EnvVar]) -> Result<()> {
        self.tx(|tx| replace_env(tx, service_id, vars))
    }

    // Domains

    pub fn domains(&self, service_id: &str) -> Result<Vec<Domain>> {
        self.domain_query(
            "WHERE service_id = ?1 ORDER BY created_at, rowid",
            [service_id],
        )
    }

    pub fn domain(&self, id: &str) -> Result<Option<Domain>> {
        Ok(self.domain_query("WHERE id = ?1", [id])?.into_iter().next())
    }

    fn domain_query<P: rusqlite::Params>(&self, tail: &str, params: P) -> Result<Vec<Domain>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, service_id, host, port, tls, created_at FROM domains {tail}"
        ))?;
        let rows = stmt.query_map(params, |row| {
            Ok(Domain {
                id: row.get(0)?,
                service_id: row.get(1)?,
                host: row.get(2)?,
                port: row.get(3)?,
                tls: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn domain_taken(&self, host: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM domains WHERE host = ?1", [host])
    }

    pub fn insert_domain(&self, d: &Domain) -> Result<()> {
        self.tx(|tx| insert_domain(tx, d))
    }

    pub fn delete_domain(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM domains WHERE id = ?1", [id])?;
        Ok(())
    }

    // Volumes

    pub fn volumes(&self, service_id: &str) -> Result<Vec<Volume>> {
        self.volume_query(
            "WHERE service_id = ?1 ORDER BY created_at, rowid",
            [service_id],
        )
    }

    pub fn volume(&self, id: &str) -> Result<Option<Volume>> {
        Ok(self.volume_query("WHERE id = ?1", [id])?.into_iter().next())
    }

    fn volume_query<P: rusqlite::Params>(&self, tail: &str, params: P) -> Result<Vec<Volume>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, service_id, name, mount_path, created_at FROM volumes {tail}"
        ))?;
        let rows = stmt.query_map(params, |row| {
            Ok(Volume {
                id: row.get(0)?,
                service_id: row.get(1)?,
                name: row.get(2)?,
                mount_path: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn volume_name_taken(&self, service_id: &str, name: &str) -> Result<bool> {
        self.exists(
            "SELECT 1 FROM volumes WHERE service_id = ?1 AND name = ?2",
            [service_id, name],
        )
    }

    pub fn insert_volume(&self, v: &Volume) -> Result<()> {
        self.tx(|tx| insert_volume(tx, v))
    }

    pub fn delete_volume(&self, id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM volumes WHERE id = ?1", [id])?;
        Ok(())
    }

    // Deployments

    /// Inserts a queued deployment, assigning the next per-service number.
    pub fn insert_deployment(&self, d: &mut Deployment, server_id: &str) -> Result<()> {
        self.tx(|tx| {
            d.number = tx.query_row(
                "SELECT COALESCE(MAX(number), 0) + 1 FROM deployments WHERE service_id = ?1",
                [&d.service_id],
                |row| row.get(0),
            )?;
            tx.execute(
                "INSERT INTO deployments (id, service_id, server_id, number, status, trigger, commit_sha, commit_message,
                                          image, container_id, steps, error, created_at, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    d.id,
                    d.service_id,
                    server_id,
                    d.number,
                    enum_str(&d.status),
                    d.trigger,
                    d.commit,
                    d.commit_message,
                    d.image,
                    d.container_id,
                    json(&d.steps),
                    d.error,
                    d.created_at,
                    d.finished_at
                ],
            )?;
            Ok(())
        })
    }

    pub fn update_deployment(&self, d: &Deployment) -> Result<()> {
        self.lock().execute(
            "UPDATE deployments SET status = ?2, commit_sha = ?3, commit_message = ?4, image = ?5,
                    container_id = ?6, steps = ?7, error = ?8, finished_at = ?9 WHERE id = ?1",
            params![
                d.id,
                enum_str(&d.status),
                d.commit,
                d.commit_message,
                d.image,
                d.container_id,
                json(&d.steps),
                d.error,
                d.finished_at
            ],
        )?;
        Ok(())
    }

    pub fn deployment(&self, id: &str) -> Result<Option<Deployment>> {
        Ok(self
            .deployment_query("WHERE id = ?1", params![id])?
            .into_iter()
            .next())
    }

    pub fn deployments(&self, service_id: &str, limit: usize) -> Result<Vec<Deployment>> {
        self.deployment_query(
            "WHERE service_id = ?1 ORDER BY number DESC LIMIT ?2",
            params![service_id, i64::try_from(limit).unwrap_or(i64::MAX)],
        )
    }

    pub fn live_deployment(&self, service_id: &str) -> Result<Option<Deployment>> {
        Ok(self
            .deployment_query(
                "WHERE service_id = ?1 AND status = 'live' ORDER BY number DESC LIMIT 1",
                params![service_id],
            )?
            .into_iter()
            .next())
    }

    /// Unfinished deployments of a service, oldest first.
    pub fn active_deployments(&self, service_id: &str) -> Result<Vec<Deployment>> {
        self.deployment_query(
            "WHERE service_id = ?1 AND status IN ('queued', 'building', 'starting') ORDER BY number",
            params![service_id],
        )
    }

    /// Service name and number of every unfinished deployment.
    pub fn unfinished_deployments(&self) -> Result<Vec<(String, i64)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT s.name, d.number FROM deployments d JOIN services s ON s.id = d.service_id
             WHERE d.status IN ('queued', 'building', 'starting') ORDER BY d.created_at",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn deployment_query(
        &self,
        tail: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<Deployment>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, service_id, number, status, trigger, commit_sha, commit_message, image, container_id,
                    steps, error, created_at, finished_at FROM deployments {tail}"
        ))?;
        let rows = stmt.query_map(params, deployment_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Servers that ever ran a deployment of this service, plus where it runs now.
    pub fn deployment_servers(&self, service_id: &str) -> Result<Vec<Id>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT server_id FROM deployments WHERE service_id = ?1
             UNION SELECT server_id FROM services WHERE id = ?1",
        )?;
        let rows = stmt.query_map([service_id], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Marks `deployment` live and the previously live one superseded, atomically.
    pub fn switch_live(&self, d: &Deployment) -> Result<()> {
        self.tx(|tx| {
            tx.execute(
                "UPDATE deployments SET status = 'superseded' WHERE service_id = ?1 AND status = 'live' AND id != ?2",
                params![d.service_id, d.id],
            )?;
            tx.execute(
                "UPDATE deployments SET status = 'live', steps = ?2, finished_at = ?3 WHERE id = ?1",
                params![d.id, json(&d.steps), d.finished_at],
            )?;
            tx.execute("UPDATE services SET stopped = 0 WHERE id = ?1", [&d.service_id])?;
            Ok(())
        })
    }

    /// Fails deployments a previous daemon process left unfinished. Returns how many.
    pub fn fail_interrupted_deployments(&self) -> Result<usize> {
        let interrupted = {
            let conn = self.lock();
            let mut stmt = conn.prepare(
                "SELECT id, service_id, number, status, trigger, commit_sha, commit_message, image, container_id,
                        steps, error, created_at, finished_at FROM deployments
                 WHERE status IN ('queued', 'building', 'starting')",
            )?;
            let rows = stmt.query_map([], deployment_from_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let now = Utc::now();
        for mut d in interrupted.iter().cloned() {
            for step in &mut d.steps {
                if matches!(step.status, StepStatus::Running) {
                    step.status = StepStatus::Failed;
                    step.finished_at = Some(now);
                }
            }
            d.status = DeploymentStatus::Failed;
            d.error = Some("daemon restarted".into());
            d.finished_at = Some(now);
            self.update_deployment(&d)?;
        }
        Ok(interrupted.len())
    }

    // Activity

    pub fn insert_activity(&self, a: &Activity) -> Result<()> {
        self.lock().execute(
            "INSERT INTO activity (at, level, service_id, service_name, project_id, message, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                a.at,
                enum_str(&a.level),
                a.service_id,
                a.service_name,
                a.project_id,
                a.message,
                a.detail
            ],
        )?;
        Ok(())
    }

    pub fn activity(&self, limit: usize) -> Result<Vec<Activity>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, at, level, service_id, service_name, project_id, message, detail
             FROM activity ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok(Activity {
                id: row.get(0)?,
                at: row.get(1)?,
                level: col_enum(row, 2)?,
                service_id: row.get(3)?,
                service_name: row.get(4)?,
                project_id: row.get(5)?,
                message: row.get(6)?,
                detail: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn exists<P: rusqlite::Params>(&self, sql: &str, params: P) -> Result<bool> {
        let conn = self.lock();
        Ok(conn
            .query_row(sql, params, |_| Ok(()))
            .optional()?
            .is_some())
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: usize = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
        conn.execute_batch(&format!(
            "BEGIN; {sql} PRAGMA user_version = {}; COMMIT;",
            i + 1
        ))
        .with_context(|| format!("database migration {} failed", i + 1))?;
    }
    Ok(())
}

const SERVER_SELECT: &str = "SELECT id, name, kind, host, user, port, status, proxy, docker_version, last_checked_at, created_at FROM servers";

fn server_from_row(row: &Row<'_>) -> rusqlite::Result<Server> {
    Ok(Server {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: col_enum(row, 2)?,
        host: row.get(3)?,
        user: row.get(4)?,
        port: row.get(5)?,
        status: col_enum(row, 6)?,
        proxy: col_enum(row, 7)?,
        docker_version: row.get(8)?,
        last_checked_at: row.get(9)?,
        created_at: row.get(10)?,
    })
}

fn project_from_row(row: &Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn environment_from_row(row: &Row<'_>) -> rusqlite::Result<Environment> {
    Ok(Environment {
        id: row.get(0)?,
        project_id: row.get(1)?,
        name: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn deployment_from_row(row: &Row<'_>) -> rusqlite::Result<Deployment> {
    Ok(Deployment {
        id: row.get(0)?,
        service_id: row.get(1)?,
        number: row.get(2)?,
        status: col_enum(row, 3)?,
        trigger: row.get(4)?,
        commit: row.get(5)?,
        commit_message: row.get(6)?,
        image: row.get(7)?,
        container_id: row.get(8)?,
        steps: col_json(row, 9)?,
        error: row.get(10)?,
        created_at: row.get(11)?,
        finished_at: row.get(12)?,
    })
}

fn insert_environment(tx: &Transaction<'_>, e: &Environment) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO environments (id, project_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![e.id, e.project_id, e.name, e.created_at],
    )?;
    Ok(())
}

fn insert_domain(tx: &Transaction<'_>, d: &Domain) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO domains (id, service_id, host, port, tls, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![d.id, d.service_id, d.host, d.port, d.tls, d.created_at],
    )?;
    Ok(())
}

fn insert_volume(tx: &Transaction<'_>, v: &Volume) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO volumes (id, service_id, name, mount_path, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![v.id, v.service_id, v.name, v.mount_path, v.created_at],
    )?;
    Ok(())
}

fn replace_env(tx: &Transaction<'_>, service_id: &str, vars: &[EnvVar]) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM env_vars WHERE service_id = ?1", [service_id])?;
    for (position, var) in vars.iter().enumerate() {
        tx.execute(
            "INSERT INTO env_vars (service_id, position, key, value, secret) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                service_id,
                i64::try_from(position).unwrap_or(i64::MAX),
                var.key,
                var.value,
                var.secret
            ],
        )?;
    }
    Ok(())
}

/// The serde `snake_case` tag of a unit enum variant.
fn enum_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        other => unreachable!("not a unit enum: {other:?}"),
    }
}

fn parse_enum<T: DeserializeOwned>(s: &str, idx: usize) -> rusqlite::Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

fn col_enum<T: DeserializeOwned>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    parse_enum(&s, idx)
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("domain types always serialize")
}

fn col_json<T: DeserializeOwned>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    serde_json::from_str(&s)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(db: &Db) -> (Project, Environment) {
        let p = Project {
            id: new_id(),
            name: "shop".into(),
            description: None,
            created_at: Utc::now(),
        };
        let e = Environment {
            id: new_id(),
            project_id: p.id.clone(),
            name: "production".into(),
            created_at: Utc::now(),
        };
        db.insert_project(&p, &e).unwrap();
        (p, e)
    }

    fn server(db: &Db) -> Server {
        let s = Server {
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
            created_at: Utc::now(),
        };
        db.insert_server(&s).unwrap();
        s
    }

    fn deployment(service_id: &str) -> Deployment {
        Deployment {
            id: new_id(),
            service_id: service_id.into(),
            number: 0,
            status: DeploymentStatus::Queued,
            trigger: "api".into(),
            commit: None,
            commit_message: None,
            image: Some("nginx:latest".into()),
            container_id: None,
            steps: vec![],
            error: None,
            created_at: Utc::now(),
            finished_at: None,
        }
    }

    #[test]
    fn ids_are_valid_names() {
        for _ in 0..100 {
            assert!(validate_name(&new_id()).is_ok());
        }
    }

    #[test]
    fn service_round_trip_with_live_switch() {
        let db = Db::open_in_memory().unwrap();
        let srv = server(&db);
        let (p, env) = project(&db);
        let svc = Service {
            id: new_id(),
            environment_id: env.id.clone(),
            server_id: srv.id.clone(),
            name: "web".into(),
            source: Source::Image {
                image: "nginx".into(),
            },
            port: Some(80),
            command: None,
            restart: RestartPolicy::UnlessStopped,
            health_path: None,
            created_at: Utc::now(),
            state: ServiceState::New,
            primary_domain: None,
            last_deployed_at: None,
            live_deployment_id: None,
        };
        let domain = Domain {
            id: new_id(),
            service_id: svc.id.clone(),
            host: "a.example.com".into(),
            port: None,
            tls: true,
            created_at: Utc::now(),
        };
        let env_vars = [EnvVar {
            key: "A".into(),
            value: "1".into(),
            secret: false,
        }];
        db.insert_service(&NewService {
            service: &svc,
            env: &env_vars,
            domains: std::slice::from_ref(&domain),
            volumes: &[],
        })
        .unwrap();
        assert!(db.domain_taken("a.example.com").unwrap());
        assert!(db.service_name_taken(&env.id, "web", None).unwrap());
        assert!(
            !db.service_name_taken(&env.id, "web", Some(&svc.id))
                .unwrap()
        );

        let mut first = deployment(&svc.id);
        db.insert_deployment(&mut first, &srv.id).unwrap();
        let mut second = deployment(&svc.id);
        db.insert_deployment(&mut second, &srv.id).unwrap();
        assert_eq!((first.number, second.number), (1, 2));

        let rec = db.service(&svc.id).unwrap().unwrap();
        assert_eq!(rec.busy, Some(DeploymentStatus::Queued));
        assert_eq!(rec.service.primary_domain.as_deref(), Some("a.example.com"));

        first.finished_at = Some(Utc::now());
        db.switch_live(&first).unwrap();
        second.finished_at = Some(Utc::now());
        db.switch_live(&second).unwrap();
        let first = db.deployment(&first.id).unwrap().unwrap();
        assert_eq!(first.status, DeploymentStatus::Superseded);
        let rec = db.service(&svc.id).unwrap().unwrap();
        assert_eq!(
            rec.service.live_deployment_id.as_deref(),
            Some(second.id.as_str())
        );
        assert_eq!(rec.busy, None);
        assert_eq!(db.env(&svc.id).unwrap(), env_vars);

        db.delete_project(&p.id).unwrap();
        assert!(db.service(&svc.id).unwrap().is_none());
        assert!(db.deployment(&second.id).unwrap().is_none());
    }

    #[test]
    fn interrupted_deployments_fail_on_startup() {
        let db = Db::open_in_memory().unwrap();
        let srv = server(&db);
        let (_, env) = project(&db);
        let svc_id = new_id();
        db.lock()
            .execute(
                "INSERT INTO services (id, environment_id, server_id, name, source, restart, created_at)
                 VALUES (?1, ?2, ?3, 'web', '{\"type\":\"image\",\"image\":\"nginx\"}', 'always', ?4)",
                params![svc_id, env.id, srv.id, Utc::now()],
            )
            .unwrap();
        let mut d = deployment(&svc_id);
        db.insert_deployment(&mut d, &srv.id).unwrap();
        assert_eq!(db.fail_interrupted_deployments().unwrap(), 1);
        let d = db.deployment(&d.id).unwrap().unwrap();
        assert_eq!(d.status, DeploymentStatus::Failed);
        assert_eq!(d.error.as_deref(), Some("daemon restarted"));
    }
}
