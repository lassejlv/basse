//! Postgres storage: the schema lives in `migrations/`; this is every query the daemon runs.

use crate::metrics::{self, Bucket, Sample};
use anyhow::{Context as _, Result};
use chrono::Utc;
use common::metrics::DeployMarker;
use common::notify::*;
use common::*;
use rand::Rng as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions, PgRow};
use sqlx::types::Json;
use sqlx::{AssertSqlSafe, Connection as _, Postgres, Row as _, Transaction};
use std::time::Duration;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

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

/// A service's backup schedule plus when the scheduler last started a backup for it.
#[derive(Debug, Clone)]
pub struct ScheduleRecord {
    pub service_id: Id,
    pub schedule: BackupSchedule,
    /// Also set when the schedule is enabled, so enabling never triggers a catch-up run.
    pub last_run_at: Option<Timestamp>,
}

/// What the TLS monitor last saw on an HTTPS domain.
#[derive(Debug, Clone)]
pub struct TlsRecord {
    pub host: String,
    /// Hex serial of the certificate.
    pub serial: String,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
    /// `tls.expiring_soon` was sent for this certificate.
    pub expiry_warned: bool,
    /// `tls.certificate_invalid` was sent since the last valid certificate.
    pub invalid_reported: bool,
}

/// A git source plus the stored facts the API does not return.
#[derive(Debug, Clone)]
pub struct SourceRecord {
    pub source: GitSource,
    /// Organization the manifest flow creates the app in.
    pub organization: Option<String>,
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
    pool: PgPool,
}

impl Db {
    /// Connects and applies pending migrations.
    pub async fn connect(url: &str) -> Result<Self> {
        let options: PgConnectOptions = url.parse().context("invalid database URL")?;
        let target = format!(
            "{}:{}/{}",
            options.get_host(),
            options.get_port(),
            options.get_database().unwrap_or_default()
        );
        PgConnection::connect_with(&options)
            .await
            .with_context(|| format!("cannot connect to the database at {target}"))?
            .close()
            .await?;
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(Duration::from_secs(10))
            .connect_lazy_with(options);
        MIGRATOR
            .run(&pool)
            .await
            .context("database migration failed")?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Whether nothing was ever stored: no servers, projects or activity.
    pub async fn is_empty(&self) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT NOT EXISTS (SELECT 1 FROM servers)
                AND NOT EXISTS (SELECT 1 FROM projects)
                AND NOT EXISTS (SELECT 1 FROM activity)",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    // Servers

    pub async fn servers(&self) -> Result<Vec<Server>> {
        self.fetch(
            format!("{SERVER_SELECT} ORDER BY created_at, seq"),
            &[],
            server_from_row,
        )
        .await
    }

    pub async fn server(&self, id: &str) -> Result<Option<Server>> {
        self.fetch_one(
            format!("{SERVER_SELECT} WHERE id = $1"),
            &[id],
            server_from_row,
        )
        .await
    }

    pub async fn local_server(&self) -> Result<Option<Server>> {
        self.fetch_one(
            format!("{SERVER_SELECT} WHERE kind = 'local' ORDER BY created_at, seq LIMIT 1"),
            &[],
            server_from_row,
        )
        .await
    }

    pub async fn server_name_taken(&self, name: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM servers WHERE name = $1", &[name])
            .await
    }

    pub async fn insert_server(&self, s: &Server) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO servers (id, name, kind, host, "user", port, status, proxy, docker_version, last_checked_at, created_at, ssh_key_secret_id, checks, problem)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)"#,
        )
        .bind(&s.id)
        .bind(&s.name)
        .bind(enum_str(&s.kind))
        .bind(&s.host)
        .bind(&s.user)
        .bind(s.port.map(i32::from))
        .bind(enum_str(&s.status))
        .bind(enum_str(&s.proxy))
        .bind(&s.docker_version)
        .bind(s.last_checked_at)
        .bind(s.created_at)
        .bind(&s.ssh_key_secret_id)
        .bind(Json(&s.checks))
        .bind(s.problem.as_ref().map(Json))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_server_key(&self, id: &str, secret_id: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE servers SET ssh_key_secret_id = $2 WHERE id = $1")
            .bind(id)
            .bind(secret_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_server_check(&self, s: &Server) -> Result<()> {
        sqlx::query(
            "UPDATE servers SET status = $2, proxy = $3, docker_version = $4, last_checked_at = $5,
                    checks = $6, problem = $7 WHERE id = $1",
        )
        .bind(&s.id)
        .bind(enum_str(&s.status))
        .bind(enum_str(&s.proxy))
        .bind(&s.docker_version)
        .bind(s.last_checked_at)
        .bind(Json(&s.checks))
        .bind(s.problem.as_ref().map(Json))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn server_in_use(&self, id: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM services WHERE server_id = $1", &[id])
            .await
    }

    pub async fn delete_server(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM servers WHERE id = $1", id).await
    }

    // Projects and environments

    pub async fn projects(&self) -> Result<Vec<Project>> {
        self.fetch(
            "SELECT id, name, description, created_at FROM projects ORDER BY created_at, seq"
                .into(),
            &[],
            project_from_row,
        )
        .await
    }

    pub async fn project(&self, id: &str) -> Result<Option<Project>> {
        self.fetch_one(
            "SELECT id, name, description, created_at FROM projects WHERE id = $1".into(),
            &[id],
            project_from_row,
        )
        .await
    }

    pub async fn project_name_taken(&self, name: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM projects WHERE name = $1", &[name])
            .await
    }

    /// Inserts the project and its first environment together.
    pub async fn insert_project(&self, p: &Project, first_env: &Environment) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO projects (id, name, description, created_at) VALUES ($1, $2, $3, $4)",
        )
        .bind(&p.id)
        .bind(&p.name)
        .bind(&p.description)
        .bind(p.created_at)
        .execute(&mut *tx)
        .await?;
        insert_environment(&mut tx, first_env).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_project(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM projects WHERE id = $1", id)
            .await?;
        self.prune_orphan_rules().await
    }

    pub async fn environments(&self) -> Result<Vec<Environment>> {
        self.fetch(
            "SELECT id, project_id, name, created_at FROM environments ORDER BY created_at, seq"
                .into(),
            &[],
            environment_from_row,
        )
        .await
    }

    pub async fn environment(&self, id: &str) -> Result<Option<Environment>> {
        self.fetch_one(
            "SELECT id, project_id, name, created_at FROM environments WHERE id = $1".into(),
            &[id],
            environment_from_row,
        )
        .await
    }

    pub async fn environment_name_taken(&self, project_id: &str, name: &str) -> Result<bool> {
        self.exists(
            "SELECT 1 FROM environments WHERE project_id = $1 AND name = $2",
            &[project_id, name],
        )
        .await
    }

    pub async fn insert_environment(&self, e: &Environment) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        insert_environment(&mut tx, e).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_environment(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM environments WHERE id = $1", id)
            .await?;
        self.prune_orphan_rules().await
    }

    // Services

    pub async fn services(&self) -> Result<Vec<ServiceRecord>> {
        self.service_query("ORDER BY s.created_at, s.seq", &[])
            .await
    }

    pub async fn services_in_environment(&self, env_id: &str) -> Result<Vec<ServiceRecord>> {
        self.service_query(
            "WHERE s.environment_id = $1 ORDER BY s.created_at, s.seq",
            &[env_id],
        )
        .await
    }

    pub async fn service(&self, id: &str) -> Result<Option<ServiceRecord>> {
        Ok(self
            .service_query("WHERE s.id = $1", &[id])
            .await?
            .into_iter()
            .next())
    }

    async fn service_query(&self, tail: &str, binds: &[&str]) -> Result<Vec<ServiceRecord>> {
        let sql = format!(
            "SELECT s.id, s.environment_id, s.server_id, s.name, s.source, s.port, s.command, s.restart,
                    s.health_path, s.created_at, s.stopped,
                    (SELECT host FROM domains d WHERE d.service_id = s.id ORDER BY d.created_at, d.seq LIMIT 1),
                    (SELECT id FROM deployments WHERE service_id = s.id AND status = 'live' ORDER BY number DESC LIMIT 1),
                    (SELECT container_id FROM deployments WHERE service_id = s.id AND status = 'live' ORDER BY number DESC LIMIT 1),
                    (SELECT MAX(finished_at) FROM deployments WHERE service_id = s.id AND status IN ('live', 'superseded')),
                    (SELECT status FROM deployments WHERE service_id = s.id AND status IN ('queued', 'building', 'starting') ORDER BY number LIMIT 1),
                    EXISTS (SELECT 1 FROM deployments WHERE service_id = s.id AND status = 'failed')
             FROM services s {tail}"
        );
        self.fetch(sql, binds, |row| {
            let service = Service {
                id: row.try_get(0)?,
                environment_id: row.try_get(1)?,
                server_id: row.try_get(2)?,
                name: row.try_get(3)?,
                source: col_json(row, 4)?,
                port: col_port(row, 5)?,
                command: row.try_get(6)?,
                restart: col_enum(row, 7)?,
                health_path: row.try_get(8)?,
                created_at: row.try_get(9)?,
                state: ServiceState::New,
                primary_domain: row.try_get(11)?,
                live_deployment_id: row.try_get(12)?,
                last_deployed_at: row.try_get(14)?,
            };
            let busy: Option<String> = row.try_get(15)?;
            Ok(ServiceRecord {
                service,
                stopped: row.try_get(10)?,
                live_container: row.try_get(13)?,
                busy: busy.map(|s| parse_enum(&s, 15)).transpose()?,
                ever_failed: row.try_get(16)?,
            })
        })
        .await
    }

    pub async fn service_name_taken(
        &self,
        env_id: &str,
        name: &str,
        except: Option<&str>,
    ) -> Result<bool> {
        self.exists(
            "SELECT 1 FROM services WHERE environment_id = $1 AND name = $2 AND id != $3",
            &[env_id, name, except.unwrap_or("")],
        )
        .await
    }

    pub async fn insert_service(&self, new: &NewService<'_>) -> Result<()> {
        let s = new.service;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO services (id, environment_id, server_id, name, source, port, command, restart, health_path, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&s.id)
        .bind(&s.environment_id)
        .bind(&s.server_id)
        .bind(&s.name)
        .bind(Json(&s.source))
        .bind(s.port.map(i32::from))
        .bind(&s.command)
        .bind(enum_str(&s.restart))
        .bind(&s.health_path)
        .bind(s.created_at)
        .execute(&mut *tx)
        .await?;
        replace_env(&mut tx, &s.id, new.env).await?;
        for d in new.domains {
            insert_domain(&mut tx, d).await?;
        }
        for v in new.volumes {
            insert_volume(&mut tx, v).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_service(&self, s: &Service) -> Result<()> {
        sqlx::query(
            "UPDATE services SET name = $2, source = $3, port = $4, command = $5, restart = $6,
                    health_path = $7, server_id = $8 WHERE id = $1",
        )
        .bind(&s.id)
        .bind(&s.name)
        .bind(Json(&s.source))
        .bind(s.port.map(i32::from))
        .bind(&s.command)
        .bind(enum_str(&s.restart))
        .bind(&s.health_path)
        .bind(&s.server_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn set_stopped(&self, service_id: &str, stopped: bool) -> Result<()> {
        sqlx::query("UPDATE services SET stopped = $2 WHERE id = $1")
            .bind(service_id)
            .bind(stopped)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_service(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM services WHERE id = $1", id)
            .await?;
        self.prune_orphan_rules().await
    }

    pub async fn env(&self, service_id: &str) -> Result<Vec<EnvVar>> {
        self.fetch(
            "SELECT key, value, secret FROM env_vars WHERE service_id = $1 ORDER BY position"
                .into(),
            &[service_id],
            |row| {
                Ok(EnvVar {
                    key: row.try_get(0)?,
                    value: row.try_get(1)?,
                    secret: row.try_get(2)?,
                })
            },
        )
        .await
    }

    pub async fn replace_env(&self, service_id: &str, vars: &[EnvVar]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        replace_env(&mut tx, service_id, vars).await?;
        tx.commit().await?;
        Ok(())
    }

    // Domains

    pub async fn domains(&self, service_id: &str) -> Result<Vec<Domain>> {
        self.domain_query(
            "WHERE service_id = $1 ORDER BY created_at, seq",
            &[service_id],
        )
        .await
    }

    pub async fn domain(&self, id: &str) -> Result<Option<Domain>> {
        Ok(self
            .domain_query("WHERE id = $1", &[id])
            .await?
            .into_iter()
            .next())
    }

    async fn domain_query(&self, tail: &str, binds: &[&str]) -> Result<Vec<Domain>> {
        self.fetch(
            format!(
                "SELECT id, service_id, host, port, tls, created_at, compose_service FROM domains {tail}"
            ),
            binds,
            |row| {
                Ok(Domain {
                    id: row.try_get(0)?,
                    service_id: row.try_get(1)?,
                    host: row.try_get(2)?,
                    port: col_port(row, 3)?,
                    tls: row.try_get(4)?,
                    created_at: row.try_get(5)?,
                    compose_service: row.try_get(6)?,
                })
            },
        )
        .await
    }

    pub async fn domain_taken(&self, host: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM domains WHERE host = $1", &[host])
            .await
    }

    pub async fn insert_domain(&self, d: &Domain) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        insert_domain(&mut tx, d).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_domain(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM domains WHERE id = $1", id).await
    }

    // Volumes

    pub async fn volumes(&self, service_id: &str) -> Result<Vec<Volume>> {
        self.volume_query("WHERE service_id = $1 ORDER BY created_at, seq", service_id)
            .await
    }

    pub async fn volume(&self, id: &str) -> Result<Option<Volume>> {
        Ok(self
            .volume_query("WHERE id = $1", id)
            .await?
            .into_iter()
            .next())
    }

    async fn volume_query(&self, tail: &str, bind: &str) -> Result<Vec<Volume>> {
        self.fetch(
            format!("SELECT id, service_id, name, mount_path, created_at FROM volumes {tail}"),
            &[bind],
            |row| {
                Ok(Volume {
                    id: row.try_get(0)?,
                    service_id: row.try_get(1)?,
                    name: row.try_get(2)?,
                    mount_path: row.try_get(3)?,
                    created_at: row.try_get(4)?,
                })
            },
        )
        .await
    }

    pub async fn volume_name_taken(&self, service_id: &str, name: &str) -> Result<bool> {
        self.exists(
            "SELECT 1 FROM volumes WHERE service_id = $1 AND name = $2",
            &[service_id, name],
        )
        .await
    }

    pub async fn insert_volume(&self, v: &Volume) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        insert_volume(&mut tx, v).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_volume(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM volumes WHERE id = $1", id).await
    }

    // Deployments

    /// Inserts a queued deployment, assigning the next per-service number.
    pub async fn insert_deployment(&self, d: &mut Deployment, server_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        lock_service(&mut tx, &d.service_id).await?;
        d.number = sqlx::query_scalar(
            "SELECT COALESCE(MAX(number), 0) + 1 FROM deployments WHERE service_id = $1",
        )
        .bind(&d.service_id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO deployments (id, service_id, server_id, number, status, trigger, commit_sha, commit_message,
                                      image, container_id, steps, error, created_at, finished_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(&d.id)
        .bind(&d.service_id)
        .bind(server_id)
        .bind(d.number)
        .bind(enum_str(&d.status))
        .bind(&d.trigger)
        .bind(&d.commit)
        .bind(&d.commit_message)
        .bind(&d.image)
        .bind(&d.container_id)
        .bind(Json(&d.steps))
        .bind(&d.error)
        .bind(d.created_at)
        .bind(d.finished_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_deployment(&self, d: &Deployment) -> Result<()> {
        sqlx::query(
            "UPDATE deployments SET status = $2, commit_sha = $3, commit_message = $4, image = $5,
                    container_id = $6, steps = $7, error = $8, finished_at = $9 WHERE id = $1",
        )
        .bind(&d.id)
        .bind(enum_str(&d.status))
        .bind(&d.commit)
        .bind(&d.commit_message)
        .bind(&d.image)
        .bind(&d.container_id)
        .bind(Json(&d.steps))
        .bind(&d.error)
        .bind(d.finished_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn deployment(&self, id: &str) -> Result<Option<Deployment>> {
        Ok(self
            .deployment_query("WHERE id = $1", &[id])
            .await?
            .into_iter()
            .next())
    }

    pub async fn deployments(&self, service_id: &str, limit: usize) -> Result<Vec<Deployment>> {
        self.deployment_query(
            &format!(
                "WHERE service_id = $1 ORDER BY number DESC LIMIT {}",
                i64::try_from(limit).unwrap_or(i64::MAX)
            ),
            &[service_id],
        )
        .await
    }

    pub async fn live_deployment(&self, service_id: &str) -> Result<Option<Deployment>> {
        Ok(self
            .deployment_query(
                "WHERE service_id = $1 AND status = 'live' ORDER BY number DESC LIMIT 1",
                &[service_id],
            )
            .await?
            .into_iter()
            .next())
    }

    /// Unfinished deployments of a service, oldest first.
    pub async fn active_deployments(&self, service_id: &str) -> Result<Vec<Deployment>> {
        self.deployment_query(
            "WHERE service_id = $1 AND status IN ('queued', 'building', 'starting') ORDER BY number",
            &[service_id],
        )
        .await
    }

    /// Service name and number of every unfinished deployment.
    pub async fn unfinished_deployments(&self) -> Result<Vec<(String, i64)>> {
        self.fetch(
            "SELECT s.name, d.number FROM deployments d JOIN services s ON s.id = d.service_id
             WHERE d.status IN ('queued', 'building', 'starting') ORDER BY d.created_at"
                .to_string(),
            &[],
            |row| Ok((row.try_get(0)?, row.try_get(1)?)),
        )
        .await
    }

    async fn deployment_query(&self, tail: &str, binds: &[&str]) -> Result<Vec<Deployment>> {
        self.fetch(
            format!("{DEPLOYMENT_SELECT} {tail}"),
            binds,
            deployment_from_row,
        )
        .await
    }

    /// Servers that ever ran a deployment of this service, plus where it runs now.
    pub async fn deployment_servers(&self, service_id: &str) -> Result<Vec<Id>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT server_id FROM deployments WHERE service_id = $1
             UNION SELECT server_id FROM services WHERE id = $1",
        )
        .bind(service_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Marks `deployment` live and the previously live one superseded, atomically. A move
    /// passes the server the deployment runs on, which the service moves to in the same step.
    pub async fn switch_live(&self, d: &Deployment, moved_to: Option<&str>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE deployments SET status = 'superseded' WHERE service_id = $1 AND status = 'live' AND id != $2",
        )
        .bind(&d.service_id)
        .bind(&d.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE deployments SET status = 'live', steps = $2, finished_at = $3 WHERE id = $1",
        )
        .bind(&d.id)
        .bind(Json(&d.steps))
        .bind(d.finished_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE services SET stopped = FALSE, server_id = COALESCE($2, server_id) WHERE id = $1")
            .bind(&d.service_id)
            .bind(moved_to)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Fails deployments a previous daemon process left unfinished. Returns how many.
    pub async fn fail_interrupted_deployments(&self) -> Result<usize> {
        let interrupted = self
            .deployment_query("WHERE status IN ('queued', 'building', 'starting')", &[])
            .await?;
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
            self.update_deployment(&d).await?;
        }
        Ok(interrupted.len())
    }

    // Databases

    pub async fn public_port(&self, service_id: &str) -> Result<Option<u16>> {
        let port: Option<Option<i32>> =
            sqlx::query_scalar("SELECT public_port FROM services WHERE id = $1")
                .bind(service_id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(port.flatten().and_then(|p| u16::try_from(p).ok()))
    }

    pub async fn set_public_port(&self, service_id: &str, port: Option<u16>) -> Result<()> {
        sqlx::query("UPDATE services SET public_port = $2 WHERE id = $1")
            .bind(service_id)
            .bind(port.map(i32::from))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Whether another service on the same server already publishes `port`.
    pub async fn public_port_taken(
        &self,
        server_id: &str,
        port: u16,
        except: &str,
    ) -> Result<bool> {
        Ok(sqlx::query(
            "SELECT 1 FROM services WHERE server_id = $1 AND public_port = $2 AND id != $3",
        )
        .bind(server_id)
        .bind(i32::from(port))
        .bind(except)
        .fetch_optional(&self.pool)
        .await?
        .is_some())
    }

    // Backups

    /// Inserts a backup, assigning the next per-service number and the file name
    /// `<created_at>-<number>.<extension>.gz`.
    pub async fn insert_backup(&self, b: &mut Backup, extension: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        lock_service(&mut tx, &b.service_id).await?;
        let number: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(number), 0) + 1 FROM backups WHERE service_id = $1",
        )
        .bind(&b.service_id)
        .fetch_one(&mut *tx)
        .await?;
        b.file_name = format!(
            "{}-{number}.{extension}.gz",
            b.created_at.format("%Y%m%d-%H%M%S")
        );
        sqlx::query(
            "INSERT INTO backups (id, service_id, number, status, trigger, file_name, size_bytes, error,
                                  storage, storage_id, created_at, finished_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(&b.id)
        .bind(&b.service_id)
        .bind(number)
        .bind(enum_str(&b.status))
        .bind(enum_str(&b.trigger))
        .bind(&b.file_name)
        .bind(i64::try_from(b.size_bytes).unwrap_or(i64::MAX))
        .bind(&b.error)
        .bind(enum_str(&b.storage))
        .bind(&b.storage_id)
        .bind(b.created_at)
        .bind(b.finished_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_backup(&self, b: &Backup) -> Result<()> {
        sqlx::query(
            "UPDATE backups SET status = $2, size_bytes = $3, error = $4, finished_at = $5 WHERE id = $1",
        )
        .bind(&b.id)
        .bind(enum_str(&b.status))
        .bind(i64::try_from(b.size_bytes).unwrap_or(i64::MAX))
        .bind(&b.error)
        .bind(b.finished_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn backup(&self, id: &str) -> Result<Option<Backup>> {
        Ok(self
            .backup_query("WHERE id = $1", id)
            .await?
            .into_iter()
            .next())
    }

    /// Backups of a service, newest first.
    pub async fn backups(&self, service_id: &str) -> Result<Vec<Backup>> {
        self.backup_query("WHERE service_id = $1 ORDER BY number DESC", service_id)
            .await
    }

    async fn backup_query(&self, tail: &str, bind: &str) -> Result<Vec<Backup>> {
        self.fetch(
            format!(
                "SELECT id, service_id, status, trigger, file_name, size_bytes, error, storage, storage_id,
                        created_at, finished_at FROM backups {tail}"
            ),
            &[bind],
            |row| {
                let size: i64 = row.try_get(5)?;
                Ok(Backup {
                    id: row.try_get(0)?,
                    service_id: row.try_get(1)?,
                    status: col_enum(row, 2)?,
                    trigger: col_enum(row, 3)?,
                    file_name: row.try_get(4)?,
                    size_bytes: u64::try_from(size).unwrap_or_default(),
                    error: row.try_get(6)?,
                    storage: col_enum(row, 7)?,
                    storage_id: row.try_get(8)?,
                    created_at: row.try_get(9)?,
                    finished_at: row.try_get(10)?,
                })
            },
        )
        .await
    }

    pub async fn delete_backup(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM backups WHERE id = $1", id).await
    }

    /// Fails backups a previous daemon process left running. Returns how many.
    pub async fn fail_interrupted_backups(&self) -> Result<usize> {
        let done = sqlx::query(
            "UPDATE backups SET status = 'failed', error = 'daemon restarted', finished_at = $1
             WHERE status = 'running'",
        )
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(usize::try_from(done.rows_affected()).unwrap_or(usize::MAX))
    }

    /// The service's schedule, or the disabled default when none was saved.
    pub async fn backup_schedule(&self, service_id: &str) -> Result<ScheduleRecord> {
        Ok(self
            .schedule_query("WHERE service_id = $1", &[service_id])
            .await?
            .into_iter()
            .next()
            .unwrap_or_else(|| ScheduleRecord {
                service_id: service_id.to_string(),
                schedule: BackupSchedule::default(),
                last_run_at: None,
            }))
    }

    pub async fn enabled_backup_schedules(&self) -> Result<Vec<ScheduleRecord>> {
        self.schedule_query("WHERE enabled", &[]).await
    }

    async fn schedule_query(&self, tail: &str, binds: &[&str]) -> Result<Vec<ScheduleRecord>> {
        self.fetch(
            format!(
                "SELECT service_id, enabled, hour_utc, keep, last_run_at, storage_id FROM backup_schedules {tail}"
            ),
            binds,
            |row| {
                let hour: i32 = row.try_get(2)?;
                let keep: i64 = row.try_get(3)?;
                Ok(ScheduleRecord {
                    service_id: row.try_get(0)?,
                    schedule: BackupSchedule {
                        enabled: row.try_get(1)?,
                        hour_utc: u8::try_from(hour).map_err(|e| decode_error(2, e))?,
                        keep: u32::try_from(keep).map_err(|e| decode_error(3, e))?,
                        storage_id: row.try_get(5)?,
                    },
                    last_run_at: row.try_get(4)?,
                })
            },
        )
        .await
    }

    pub async fn save_backup_schedule(&self, r: &ScheduleRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO backup_schedules (service_id, enabled, hour_utc, keep, last_run_at, storage_id)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (service_id) DO UPDATE SET enabled = $2, hour_utc = $3, keep = $4, last_run_at = $5,
                    storage_id = $6",
        )
        .bind(&r.service_id)
        .bind(r.schedule.enabled)
        .bind(i32::from(r.schedule.hour_utc))
        .bind(i64::from(r.schedule.keep))
        .bind(r.last_run_at)
        .bind(&r.schedule.storage_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // Secrets

    /// Every secret, oldest first, with what uses it.
    pub async fn secrets(&self) -> Result<Vec<Secret>> {
        let mut secrets = self
            .fetch(
                format!("{SECRET_SELECT} ORDER BY created_at, seq"),
                &[],
                secret_from_row,
            )
            .await?;
        let usages = self.secret_usages().await?;
        for secret in &mut secrets {
            secret.used_by = usages
                .iter()
                .filter(|(id, _)| *id == secret.id)
                .map(|(_, usage)| usage.clone())
                .collect();
        }
        Ok(secrets)
    }

    pub async fn secret(&self, id: &str) -> Result<Option<Secret>> {
        let Some(mut secret) = self
            .fetch_one(
                format!("{SECRET_SELECT} WHERE id = $1"),
                &[id],
                secret_from_row,
            )
            .await?
        else {
            return Ok(None);
        };
        secret.used_by = self
            .secret_usages()
            .await?
            .into_iter()
            .filter(|(owner, _)| *owner == secret.id)
            .map(|(_, usage)| usage)
            .collect();
        Ok(Some(secret))
    }

    /// `(secret id, usage)` for everything that references a secret.
    async fn secret_usages(&self) -> Result<Vec<(Id, Usage)>> {
        let rows = sqlx::query(SECRET_USAGES).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
                Ok((
                    row.try_get(0)?,
                    Usage {
                        kind: col_enum(row, 1)?,
                        id: row.try_get(2)?,
                        name: row.try_get(3)?,
                    },
                ))
            })
            .collect::<sqlx::Result<_>>()
            .map_err(Into::into)
    }

    pub async fn secret_name_taken(&self, name: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM secrets WHERE name = $1", &[name])
            .await
    }

    pub async fn secret_ciphertext(&self, id: &str) -> Result<Option<Vec<u8>>> {
        Ok(
            sqlx::query_scalar("SELECT ciphertext FROM secrets WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn insert_secret(&self, s: &Secret, ciphertext: &[u8]) -> Result<()> {
        sqlx::query(
            "INSERT INTO secrets (id, name, kind, public_part, ciphertext, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&s.id)
        .bind(&s.name)
        .bind(enum_str(&s.kind))
        .bind(&s.public_part)
        .bind(ciphertext)
        .bind(s.created_at)
        .bind(s.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Saves a renamed secret, and a new value when `ciphertext` is set.
    pub async fn update_secret(&self, s: &Secret, ciphertext: Option<&[u8]>) -> Result<()> {
        sqlx::query(
            "UPDATE secrets SET name = $2, public_part = $3, updated_at = $4,
                    ciphertext = COALESCE($5, ciphertext)
             WHERE id = $1",
        )
        .bind(&s.id)
        .bind(&s.name)
        .bind(&s.public_part)
        .bind(s.updated_at)
        .bind(ciphertext)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_secret(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM secrets WHERE id = $1", id).await
    }

    // Git sources

    /// Every git source, oldest first, with the services that clone from it.
    pub async fn sources(&self) -> Result<Vec<SourceRecord>> {
        let mut sources = self
            .fetch(
                format!("{SOURCE_SELECT} ORDER BY created_at, seq"),
                &[],
                source_from_row,
            )
            .await?;
        for rec in &mut sources {
            rec.source.used_by = self.source_usages(&rec.source.id).await?;
        }
        Ok(sources)
    }

    pub async fn source(&self, id: &str) -> Result<Option<SourceRecord>> {
        let Some(mut rec) = self
            .fetch_one(
                format!("{SOURCE_SELECT} WHERE id = $1"),
                &[id],
                source_from_row,
            )
            .await?
        else {
            return Ok(None);
        };
        rec.source.used_by = self.source_usages(id).await?;
        Ok(Some(rec))
    }

    async fn source_usages(&self, id: &str) -> Result<Vec<Usage>> {
        let rows = sqlx::query(
            "SELECT id, name FROM services WHERE source->>'source_id' = $1 ORDER BY created_at, seq",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(Usage {
                    kind: UsageKind::Service,
                    id: row.try_get(0)?,
                    name: row.try_get(1)?,
                })
            })
            .collect::<sqlx::Result<_>>()
            .map_err(Into::into)
    }

    pub async fn source_name_taken(&self, name: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM git_sources WHERE name = $1", &[name])
            .await
    }

    /// Inserts or updates a source. The setup state is only written by `set_source_setup`.
    pub async fn save_source(&self, r: &SourceRecord) -> Result<()> {
        let s = &r.source;
        let id = |v: Option<u64>| v.and_then(|v| i64::try_from(v).ok());
        sqlx::query(
            "INSERT INTO git_sources (id, name, kind, html_url, api_url, organization, owner,
                    owner_kind, app_id, app_slug, installation_id, status, status_detail, secret_id,
                    last_checked_at, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
             ON CONFLICT (id) DO UPDATE SET name = $2, html_url = $4, api_url = $5,
                    organization = $6, owner = $7, owner_kind = $8, app_id = $9, app_slug = $10,
                    installation_id = $11, status = $12, status_detail = $13, secret_id = $14,
                    last_checked_at = $15",
        )
        .bind(&s.id)
        .bind(&s.name)
        .bind(enum_str(&s.kind))
        .bind(&s.html_url)
        .bind(&s.api_url)
        .bind(&r.organization)
        .bind(&s.owner)
        .bind(s.owner_kind.as_ref().map(enum_str))
        .bind(id(s.app_id))
        .bind(&s.app_slug)
        .bind(id(s.installation_id))
        .bind(enum_str(&s.status))
        .bind(&s.status_detail)
        .bind(&s.secret_id)
        .bind(s.last_checked_at)
        .bind(s.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Stores the hash of the manifest flow's state, or clears it with `None`.
    pub async fn set_source_setup(
        &self,
        id: &str,
        state_sha256: Option<&str>,
        expires_at: Option<Timestamp>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE git_sources SET setup_state_sha256 = $2, setup_expires_at = $3 WHERE id = $1",
        )
        .bind(id)
        .bind(state_sha256)
        .bind(expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The hash of the manifest flow's state while it has not expired.
    pub async fn source_setup_state(&self, id: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT setup_state_sha256 FROM git_sources
             WHERE id = $1 AND setup_expires_at > now() AND setup_state_sha256 IS NOT NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Deletes a source and the secret holding its credentials.
    pub async fn delete_source(&self, id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let secret: Option<Option<String>> =
            sqlx::query_scalar("DELETE FROM git_sources WHERE id = $1 RETURNING secret_id")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(Some(secret)) = secret {
            sqlx::query("DELETE FROM secrets WHERE id = $1")
                .bind(secret)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Git services cloning from `source_id` whose repository is `full_name`, any case.
    pub async fn services_tracking(
        &self,
        source_id: &str,
        full_name: &str,
    ) -> Result<Vec<ServiceRecord>> {
        Ok(self
            .services()
            .await?
            .into_iter()
            .filter(|rec| match &rec.service.source {
                Source::Git {
                    repo,
                    source_id: Some(id),
                    ..
                } => id == source_id && repo.eq_ignore_ascii_case(full_name),
                _ => false,
            })
            .collect())
    }

    // Storage connections

    pub async fn storages(&self) -> Result<Vec<StorageConnection>> {
        self.fetch(
            format!("{STORAGE_SELECT} ORDER BY created_at, seq"),
            &[],
            storage_from_row,
        )
        .await
    }

    pub async fn storage(&self, id: &str) -> Result<Option<StorageConnection>> {
        self.fetch_one(
            format!("{STORAGE_SELECT} WHERE id = $1"),
            &[id],
            storage_from_row,
        )
        .await
    }

    pub async fn storage_name_taken(&self, name: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM storage_connections WHERE name = $1", &[name])
            .await
    }

    /// Inserts or updates a connection, `last_test` included.
    pub async fn save_storage(&self, s: &StorageConnection) -> Result<()> {
        sqlx::query(
            "INSERT INTO storage_connections (id, name, endpoint, region, bucket, path_prefix,
                    path_style, credentials_secret_id, created_at, last_test)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (id) DO UPDATE SET name = $2, endpoint = $3, region = $4, bucket = $5,
                    path_prefix = $6, path_style = $7, credentials_secret_id = $8, last_test = $10",
        )
        .bind(&s.id)
        .bind(&s.name)
        .bind(&s.endpoint)
        .bind(&s.region)
        .bind(&s.bucket)
        .bind(&s.path_prefix)
        .bind(s.path_style)
        .bind(&s.credentials_secret_id)
        .bind(s.created_at)
        .bind(s.last_test.as_ref().map(Json))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// How many backups (of databases and of the instance) live in a connection and how many
    /// schedules write to it.
    pub async fn storage_usage(&self, id: &str) -> Result<(i64, i64)> {
        Ok(sqlx::query_as(
            "SELECT (SELECT COUNT(*) FROM backups WHERE storage_id = $1)
                  + (SELECT COUNT(*) FROM instance_backups WHERE storage_id = $1),
                    (SELECT COUNT(*) FROM backup_schedules WHERE storage_id = $1)
                  + (SELECT COUNT(*) FROM instance_backup_schedule WHERE storage_id = $1)",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn delete_storage(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM storage_connections WHERE id = $1", id)
            .await
    }

    // Instance backups

    /// Inserts an instance backup, numbering it and naming its file
    /// `instance-<created_at>-<number>.tar.gz.enc`.
    pub async fn insert_instance_backup(&self, b: &mut InstanceBackup) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("LOCK TABLE instance_backups IN EXCLUSIVE MODE")
            .execute(&mut *tx)
            .await?;
        let number: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(number), 0) + 1 FROM instance_backups")
                .fetch_one(&mut *tx)
                .await?;
        b.file_name = format!(
            "instance-{}-{number}.tar.gz.enc",
            b.created_at.format("%Y%m%d-%H%M%S")
        );
        sqlx::query(
            "INSERT INTO instance_backups (id, number, status, trigger, file_name, size_bytes, storage,
                    storage_id, error, created_at, finished_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(&b.id)
        .bind(number)
        .bind(enum_str(&b.status))
        .bind(enum_str(&b.trigger))
        .bind(&b.file_name)
        .bind(i64::try_from(b.size_bytes).unwrap_or(i64::MAX))
        .bind(enum_str(&b.storage))
        .bind(&b.storage_id)
        .bind(&b.error)
        .bind(b.created_at)
        .bind(b.finished_at)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn update_instance_backup(&self, b: &InstanceBackup) -> Result<()> {
        sqlx::query(
            "UPDATE instance_backups SET status = $2, size_bytes = $3, error = $4, finished_at = $5
             WHERE id = $1",
        )
        .bind(&b.id)
        .bind(enum_str(&b.status))
        .bind(i64::try_from(b.size_bytes).unwrap_or(i64::MAX))
        .bind(&b.error)
        .bind(b.finished_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Instance backups, newest first.
    pub async fn instance_backups(&self) -> Result<Vec<InstanceBackup>> {
        self.fetch(
            format!("{INSTANCE_BACKUP_SELECT} ORDER BY number DESC"),
            &[],
            instance_backup_from_row,
        )
        .await
    }

    pub async fn instance_backup(&self, id: &str) -> Result<Option<InstanceBackup>> {
        self.fetch_one(
            format!("{INSTANCE_BACKUP_SELECT} WHERE id = $1"),
            &[id],
            instance_backup_from_row,
        )
        .await
    }

    pub async fn delete_instance_backup(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM instance_backups WHERE id = $1", id)
            .await
    }

    /// Fails instance backups a previous daemon process left running. Returns how many.
    pub async fn fail_interrupted_instance_backups(&self) -> Result<usize> {
        let done = sqlx::query(
            "UPDATE instance_backups SET status = 'failed', error = 'daemon restarted', finished_at = $1
             WHERE status = 'running'",
        )
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(usize::try_from(done.rows_affected()).unwrap_or(usize::MAX))
    }

    /// The schedule, the sealed passphrase and when the scheduler last ran it.
    pub async fn instance_backup_schedule(
        &self,
    ) -> Result<(InstanceBackupSchedule, Option<Vec<u8>>, Option<Timestamp>)> {
        let row = sqlx::query(
            "SELECT enabled, hour_utc, keep, storage_id, passphrase, last_run_at
             FROM instance_backup_schedule WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok((InstanceBackupSchedule::default(), None, None));
        };
        let hour: i32 = row.try_get(1)?;
        let keep: i64 = row.try_get(2)?;
        let passphrase: Option<Vec<u8>> = row.try_get(4)?;
        Ok((
            InstanceBackupSchedule {
                enabled: row.try_get(0)?,
                hour_utc: u8::try_from(hour)?,
                keep: u32::try_from(keep)?,
                storage_id: row.try_get(3)?,
                has_passphrase: passphrase.is_some(),
            },
            passphrase,
            row.try_get(5)?,
        ))
    }

    pub async fn save_instance_backup_schedule(
        &self,
        s: &InstanceBackupSchedule,
        passphrase: Option<&[u8]>,
        last_run_at: Option<Timestamp>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO instance_backup_schedule (id, enabled, hour_utc, keep, storage_id, passphrase, last_run_at)
             VALUES (1, $1, $2, $3, $4, $5, $6)
             ON CONFLICT (id) DO UPDATE SET enabled = $1, hour_utc = $2, keep = $3, storage_id = $4,
                    passphrase = $5, last_run_at = $6",
        )
        .bind(s.enabled)
        .bind(i32::from(s.hour_utc))
        .bind(i64::from(s.keep))
        .bind(&s.storage_id)
        .bind(passphrase)
        .bind(last_run_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // Notifications

    pub async fn notify_channels(&self) -> Result<Vec<NotifyChannel>> {
        self.fetch(
            format!("{CHANNEL_SELECT} ORDER BY created_at, seq"),
            &[],
            channel_from_row,
        )
        .await
    }

    pub async fn notify_channel(&self, id: &str) -> Result<Option<NotifyChannel>> {
        self.fetch_one(
            format!("{CHANNEL_SELECT} WHERE id = $1"),
            &[id],
            channel_from_row,
        )
        .await
    }

    pub async fn notify_channel_name_taken(&self, name: &str) -> Result<bool> {
        self.exists("SELECT 1 FROM notify_channels WHERE name = $1", &[name])
            .await
    }

    /// The channel's full config, sealed with the master key.
    pub async fn notify_channel_sealed(&self, id: &str) -> Result<Option<Vec<u8>>> {
        Ok(
            sqlx::query_scalar("SELECT sealed FROM notify_channels WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Inserts or updates a channel. `sealed` replaces the stored config when given; it is
    /// required for a new channel.
    pub async fn save_notify_channel(
        &self,
        c: &NotifyChannel,
        sealed: Option<&[u8]>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO notify_channels (id, name, kind, enabled, config, sealed, created_at, last_test)
             VALUES ($1, $2, $3, $4, $5, COALESCE($6, ''::bytea), $7, $8)
             ON CONFLICT (id) DO UPDATE SET name = $2, kind = $3, enabled = $4, config = $5,
                    sealed = COALESCE($6, notify_channels.sealed), last_test = $8",
        )
        .bind(&c.id)
        .bind(&c.name)
        .bind(enum_str(&c.kind))
        .bind(c.enabled)
        .bind(Json(&c.config))
        .bind(sealed)
        .bind(c.created_at)
        .bind(c.last_test.as_ref().map(Json))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_notify_channel(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM notify_channels WHERE id = $1", id)
            .await
    }

    /// Every rule, with an empty `scope_label` for the caller to fill in.
    pub async fn notify_rules(&self) -> Result<Vec<NotifyRule>> {
        self.fetch(
            format!("{RULE_SELECT} ORDER BY created_at, seq"),
            &[],
            rule_from_row,
        )
        .await
    }

    pub async fn notify_rule(&self, id: &str) -> Result<Option<NotifyRule>> {
        self.fetch_one(format!("{RULE_SELECT} WHERE id = $1"), &[id], rule_from_row)
            .await
    }

    pub async fn save_notify_rule(&self, r: &NotifyRule) -> Result<()> {
        sqlx::query(
            "INSERT INTO notify_rules (id, channel_id, scope_type, scope_id, preset, events, enabled,
                    dedupe_secs, max_per_hour, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (id) DO UPDATE SET channel_id = $2, scope_type = $3, scope_id = $4,
                    preset = $5, events = $6, enabled = $7, dedupe_secs = $8, max_per_hour = $9",
        )
        .bind(&r.id)
        .bind(&r.channel_id)
        .bind(r.scope.kind())
        .bind(r.scope.id())
        .bind(&r.preset)
        .bind(Json(&r.events))
        .bind(r.enabled)
        .bind(i64::from(r.dedupe_secs))
        .bind(i64::from(r.max_per_hour))
        .bind(r.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_notify_rule(&self, id: &str) -> Result<()> {
        self.execute("DELETE FROM notify_rules WHERE id = $1", id)
            .await
    }

    /// Removes rules scoped to a project, environment or service that no longer exists.
    async fn prune_orphan_rules(&self) -> Result<()> {
        sqlx::query(
            "DELETE FROM notify_rules WHERE
                (scope_type = 'project' AND scope_id NOT IN (SELECT id FROM projects))
             OR (scope_type = 'environment' AND scope_id NOT IN (SELECT id FROM environments))
             OR (scope_type = 'service' AND scope_id NOT IN (SELECT id FROM services))",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Records a delivery attempt and returns its id.
    pub async fn insert_delivery(&self, d: &NotifyDelivery) -> Result<i64> {
        let id = sqlx::query_scalar(
            "INSERT INTO notify_deliveries (at, channel_id, rule_id, event, subject, title, status,
                    attempts, error, latency_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING id",
        )
        .bind(d.at)
        .bind(&d.channel_id)
        .bind(&d.rule_id)
        .bind(d.event.map(|e| e.id()))
        .bind(&d.subject)
        .bind(&d.title)
        .bind(d.status.as_str())
        .bind(i32::try_from(d.attempts).unwrap_or(i32::MAX))
        .bind(&d.error)
        .bind(d.latency_ms.and_then(|ms| i64::try_from(ms).ok()))
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn update_delivery(&self, d: &NotifyDelivery) -> Result<()> {
        sqlx::query(
            "UPDATE notify_deliveries SET status = $2, attempts = $3, error = $4, latency_ms = $5
             WHERE id = $1",
        )
        .bind(d.id)
        .bind(d.status.as_str())
        .bind(i32::try_from(d.attempts).unwrap_or(i32::MAX))
        .bind(&d.error)
        .bind(d.latency_ms.and_then(|ms| i64::try_from(ms).ok()))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Newest first, optionally of one channel or rule.
    pub async fn deliveries(
        &self,
        limit: usize,
        channel_id: Option<&str>,
        rule_id: Option<&str>,
    ) -> Result<Vec<NotifyDelivery>> {
        let rows = sqlx::query(
            "SELECT d.id, d.at, d.channel_id, c.name, d.rule_id, d.event, d.subject, d.title,
                    d.status, d.attempts, d.error, d.latency_ms
             FROM notify_deliveries d JOIN notify_channels c ON c.id = d.channel_id
             WHERE ($1::text IS NULL OR d.channel_id = $1) AND ($2::text IS NULL OR d.rule_id = $2)
             ORDER BY d.id DESC LIMIT $3",
        )
        .bind(channel_id)
        .bind(rule_id)
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(NotifyDelivery {
                    id: row.try_get(0)?,
                    at: row.try_get(1)?,
                    channel_id: row.try_get(2)?,
                    channel_name: row.try_get(3)?,
                    rule_id: row.try_get(4)?,
                    event: row
                        .try_get::<Option<String>, _>(5)?
                        .map(|e| e.parse().map_err(|e: String| decode_error(5, e)))
                        .transpose()?,
                    subject: row.try_get(6)?,
                    title: row.try_get(7)?,
                    status: row
                        .try_get::<String, _>(8)?
                        .parse()
                        .map_err(|e: String| decode_error(8, e))?,
                    attempts: u32::try_from(row.try_get::<i32, _>(9)?).unwrap_or(0),
                    error: row.try_get(10)?,
                    latency_ms: row
                        .try_get::<Option<i64>, _>(11)?
                        .and_then(|ms| u64::try_from(ms).ok()),
                })
            })
            .collect::<sqlx::Result<_>>()
            .map_err(Into::into)
    }

    /// Keeps the newest `keep` deliveries.
    pub async fn prune_deliveries(&self, keep: i64) -> Result<()> {
        sqlx::query(
            "DELETE FROM notify_deliveries WHERE id <= (SELECT id FROM notify_deliveries
                    ORDER BY id DESC OFFSET $1 LIMIT 1)",
        )
        .bind(keep)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM daemon_meta WHERE key = $1")
                .bind(key)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO daemon_meta (key, value) VALUES ($1, $2)
             ON CONFLICT (key) DO UPDATE SET value = $2",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn tls_certificate(&self, host: &str) -> Result<Option<TlsRecord>> {
        let row = sqlx::query(
            "SELECT host, serial, not_before, not_after, expiry_warned, invalid_reported
             FROM tls_certificates WHERE host = $1",
        )
        .bind(host)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(TlsRecord {
                host: row.try_get(0)?,
                serial: row.try_get(1)?,
                not_before: row.try_get(2)?,
                not_after: row.try_get(3)?,
                expiry_warned: row.try_get(4)?,
                invalid_reported: row.try_get(5)?,
            })
        })
        .transpose()
        .map_err(|e: sqlx::Error| e.into())
    }

    pub async fn save_tls_certificate(&self, r: &TlsRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO tls_certificates (host, serial, not_before, not_after, expiry_warned,
                    invalid_reported, checked_at)
             VALUES ($1, $2, $3, $4, $5, $6, now())
             ON CONFLICT (host) DO UPDATE SET serial = $2, not_before = $3, not_after = $4,
                    expiry_warned = $5, invalid_reported = $6, checked_at = now()",
        )
        .bind(&r.host)
        .bind(&r.serial)
        .bind(r.not_before)
        .bind(r.not_after)
        .bind(r.expiry_warned)
        .bind(r.invalid_reported)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Every HTTPS domain with the service it routes to.
    pub async fn tls_domains(&self) -> Result<Vec<Domain>> {
        self.domain_query("WHERE tls ORDER BY host", &[]).await
    }

    // Activity

    pub async fn insert_activity(&self, a: &Activity) -> Result<()> {
        sqlx::query(
            "INSERT INTO activity (at, level, service_id, service_name, project_id, message, detail)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(a.at)
        .bind(enum_str(&a.level))
        .bind(&a.service_id)
        .bind(&a.service_name)
        .bind(&a.project_id)
        .bind(&a.message)
        .bind(&a.detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn activity(&self, limit: usize) -> Result<Vec<Activity>> {
        self.fetch(
            format!(
                "SELECT id, at, level, service_id, service_name, project_id, message, detail
                 FROM activity ORDER BY id DESC LIMIT {}",
                i64::try_from(limit).unwrap_or(i64::MAX)
            ),
            &[],
            |row| {
                Ok(Activity {
                    id: row.try_get(0)?,
                    at: row.try_get(1)?,
                    level: col_enum(row, 2)?,
                    service_id: row.try_get(3)?,
                    service_name: row.try_get(4)?,
                    project_id: row.try_get(5)?,
                    message: row.try_get(6)?,
                    detail: row.try_get(7)?,
                })
            },
        )
        .await
    }

    // Query helpers

    // Metrics

    /// Folds one sample of a container into its raw row and every rollup at once.
    pub async fn insert_metric_sample(
        &self,
        service_id: &str,
        container: &str,
        at: Timestamp,
        s: &Sample,
    ) -> Result<()> {
        let (resolutions, starts): (Vec<i32>, Vec<Timestamp>) = metrics::buckets_of(at)
            .iter()
            .map(|&(res, start)| (i32::try_from(res).unwrap_or(i32::MAX), start))
            .unzip();
        let big = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
        let rates = s.rates.unwrap_or_default();
        let http = s.http.unwrap_or_default();
        let requests: u64 = http.classes.iter().sum();
        let weighted = |p: Option<f64>| p.map_or(0.0, |p| p * requests as f64);
        let latency_requests = if http.p50_ms.is_some() { requests } else { 0 };
        sqlx::query(
            "INSERT INTO metrics (service_id, container, resolution, at, samples, cpu_sum, cpu_max,
                 mem_sum, mem_max, mem_limit, rate_samples, net_rx_sum, net_tx_sum, disk_read_sum,
                 disk_write_sum, pids_max, restarts, uptime, http_secs, s2xx, s3xx, s4xx, s5xx,
                 latency_requests, p50_wsum, p95_wsum)
             SELECT $1, $2, r.resolution, r.at, 1, $5, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                 $15, $16, $17, $18, $19, $20, $21, $22, $23, $24
             FROM unnest($3::int[], $4::timestamptz[]) AS r(resolution, at)
             ON CONFLICT (service_id, container, resolution, at) DO UPDATE SET
                 samples = metrics.samples + 1,
                 cpu_sum = metrics.cpu_sum + EXCLUDED.cpu_sum,
                 cpu_max = GREATEST(metrics.cpu_max, EXCLUDED.cpu_max),
                 mem_sum = metrics.mem_sum + EXCLUDED.mem_sum,
                 mem_max = GREATEST(metrics.mem_max, EXCLUDED.mem_max),
                 mem_limit = EXCLUDED.mem_limit,
                 rate_samples = metrics.rate_samples + EXCLUDED.rate_samples,
                 net_rx_sum = metrics.net_rx_sum + EXCLUDED.net_rx_sum,
                 net_tx_sum = metrics.net_tx_sum + EXCLUDED.net_tx_sum,
                 disk_read_sum = metrics.disk_read_sum + EXCLUDED.disk_read_sum,
                 disk_write_sum = metrics.disk_write_sum + EXCLUDED.disk_write_sum,
                 pids_max = GREATEST(metrics.pids_max, EXCLUDED.pids_max),
                 restarts = EXCLUDED.restarts,
                 uptime = EXCLUDED.uptime,
                 http_secs = metrics.http_secs + EXCLUDED.http_secs,
                 s2xx = metrics.s2xx + EXCLUDED.s2xx,
                 s3xx = metrics.s3xx + EXCLUDED.s3xx,
                 s4xx = metrics.s4xx + EXCLUDED.s4xx,
                 s5xx = metrics.s5xx + EXCLUDED.s5xx,
                 latency_requests = metrics.latency_requests + EXCLUDED.latency_requests,
                 p50_wsum = metrics.p50_wsum + EXCLUDED.p50_wsum,
                 p95_wsum = metrics.p95_wsum + EXCLUDED.p95_wsum",
        )
        .bind(service_id)
        .bind(container)
        .bind(resolutions)
        .bind(starts)
        .bind(s.cpu)
        .bind(s.mem as f64)
        .bind(big(s.mem))
        .bind(big(s.mem_limit))
        .bind(i32::from(s.rates.is_some()))
        .bind(rates.net_rx)
        .bind(rates.net_tx)
        .bind(rates.disk_read)
        .bind(rates.disk_write)
        .bind(big(s.pids))
        .bind(s.restarts)
        .bind(s.uptime_secs)
        .bind(http.secs)
        .bind(big(http.classes[0]))
        .bind(big(http.classes[1]))
        .bind(big(http.classes[2]))
        .bind(big(http.classes[3]))
        .bind(big(latency_requests))
        .bind(weighted(http.p50_ms))
        .bind(weighted(http.p95_ms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Buckets of one resolution from `from` on, oldest first; every container's unless one
    /// is named.
    pub async fn metric_buckets(
        &self,
        service_id: &str,
        resolution: i64,
        from: Timestamp,
        container: Option<&str>,
    ) -> Result<Vec<Bucket>> {
        let rows = sqlx::query(
            "SELECT container, at, samples, cpu_sum, cpu_max, mem_sum, mem_max, mem_limit,
                 rate_samples, net_rx_sum, net_tx_sum, disk_read_sum, disk_write_sum, pids_max,
                 restarts, uptime, http_secs, s2xx, s3xx, s4xx, s5xx, latency_requests, p50_wsum,
                 p95_wsum
             FROM metrics
             WHERE service_id = $1 AND resolution = $2 AND at >= $3
                 AND ($4::text IS NULL OR container = $4)
             ORDER BY at, container",
        )
        .bind(service_id)
        .bind(i32::try_from(resolution).unwrap_or(i32::MAX))
        .bind(from)
        .bind(container)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(bucket_from_row)
            .collect::<sqlx::Result<_>>()?)
    }

    /// The containers with buckets of this resolution from `from` on, by name.
    pub async fn metric_containers(
        &self,
        service_id: &str,
        resolution: i64,
        from: Timestamp,
    ) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT DISTINCT container FROM metrics
             WHERE service_id = $1 AND resolution = $2 AND at >= $3 ORDER BY container",
        )
        .bind(service_id)
        .bind(i32::try_from(resolution).unwrap_or(i32::MAX))
        .bind(from)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Drops the buckets each level no longer keeps at `now`; returns how many went.
    pub async fn expire_metrics(&self, now: Timestamp) -> Result<u64> {
        let (resolutions, cutoffs): (Vec<i32>, Vec<Timestamp>) = metrics::cutoffs(now)
            .iter()
            .map(|&(res, cutoff)| (i32::try_from(res).unwrap_or(i32::MAX), cutoff))
            .unzip();
        Ok(sqlx::query(
            "DELETE FROM metrics m
             USING unnest($1::int[], $2::timestamptz[]) AS c(resolution, cutoff)
             WHERE m.resolution = c.resolution AND m.at < c.cutoff",
        )
        .bind(resolutions)
        .bind(cutoffs)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// Deployments of a service that went live from `from` on, oldest first.
    pub async fn deploy_markers(
        &self,
        service_id: &str,
        from: Timestamp,
    ) -> Result<Vec<DeployMarker>> {
        let rows = sqlx::query(
            "SELECT finished_at, number, status FROM deployments
             WHERE service_id = $1 AND status IN ('live', 'superseded') AND finished_at >= $2
             ORDER BY finished_at",
        )
        .bind(service_id)
        .bind(from)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .iter()
            .map(|row| {
                Ok(DeployMarker {
                    at: row.try_get(0)?,
                    number: row.try_get(1)?,
                    status: col_enum(row, 2)?,
                })
            })
            .collect::<sqlx::Result<_>>()?)
    }

    async fn fetch<T>(
        &self,
        sql: String,
        binds: &[&str],
        map: fn(&PgRow) -> sqlx::Result<T>,
    ) -> Result<Vec<T>> {
        let mut query = sqlx::query(AssertSqlSafe(sql));
        for bind in binds {
            query = query.bind(*bind);
        }
        let rows = query.fetch_all(&self.pool).await?;
        Ok(rows.iter().map(map).collect::<sqlx::Result<_>>()?)
    }

    async fn fetch_one<T>(
        &self,
        sql: String,
        binds: &[&str],
        map: fn(&PgRow) -> sqlx::Result<T>,
    ) -> Result<Option<T>> {
        Ok(self.fetch(sql, binds, map).await?.into_iter().next())
    }

    async fn exists(&self, sql: &'static str, binds: &[&str]) -> Result<bool> {
        let mut query = sqlx::query(sql);
        for bind in binds {
            query = query.bind(*bind);
        }
        Ok(query.fetch_optional(&self.pool).await?.is_some())
    }

    async fn execute(&self, sql: &'static str, id: &str) -> Result<()> {
        sqlx::query(sql).bind(id).execute(&self.pool).await?;
        Ok(())
    }
}

const SERVER_SELECT: &str = r#"SELECT id, name, kind, host, "user", port, status, proxy, docker_version, last_checked_at, created_at, ssh_key_secret_id, checks, problem FROM servers"#;

const SECRET_SELECT: &str =
    "SELECT id, name, kind, public_part, created_at, updated_at FROM secrets";

const SECRET_USAGES: &str = "SELECT ssh_key_secret_id, 'server', id, name FROM servers
     WHERE ssh_key_secret_id IS NOT NULL
     UNION ALL
     SELECT credentials_secret_id, 'storage', id, name FROM storage_connections
     UNION ALL
     SELECT secret_id, 'source', id, name FROM git_sources WHERE secret_id IS NOT NULL
     ORDER BY 1, 2, 4";

const SOURCE_SELECT: &str =
    "SELECT id, name, kind, html_url, api_url, organization, owner, owner_kind,
        app_id, app_slug, installation_id, status, status_detail, secret_id, last_checked_at,
        created_at FROM git_sources";

fn source_from_row(row: &PgRow) -> sqlx::Result<SourceRecord> {
    let id = |idx: usize| -> sqlx::Result<Option<u64>> {
        row.try_get::<Option<i64>, _>(idx)?
            .map(|v| u64::try_from(v).map_err(|e| decode_error(idx, e)))
            .transpose()
    };
    Ok(SourceRecord {
        organization: row.try_get(5)?,
        source: GitSource {
            id: row.try_get(0)?,
            name: row.try_get(1)?,
            kind: col_enum(row, 2)?,
            html_url: row.try_get(3)?,
            api_url: row.try_get(4)?,
            owner: row.try_get(6)?,
            owner_kind: row
                .try_get::<Option<String>, _>(7)?
                .map(|s| parse_enum(&s, 7))
                .transpose()?,
            app_id: id(8)?,
            app_slug: row.try_get(9)?,
            installation_id: id(10)?,
            status: col_enum(row, 11)?,
            status_detail: row.try_get(12)?,
            secret_id: row.try_get(13)?,
            last_checked_at: row.try_get(14)?,
            created_at: row.try_get(15)?,
            install_url: None,
            webhook_url: None,
            used_by: Vec::new(),
        },
    })
}

const INSTANCE_BACKUP_SELECT: &str = "SELECT id, status, trigger, file_name, size_bytes, storage,
        storage_id, error, created_at, finished_at FROM instance_backups";

fn instance_backup_from_row(row: &PgRow) -> sqlx::Result<InstanceBackup> {
    let size: i64 = row.try_get(4)?;
    Ok(InstanceBackup {
        id: row.try_get(0)?,
        status: col_enum(row, 1)?,
        trigger: col_enum(row, 2)?,
        file_name: row.try_get(3)?,
        size_bytes: u64::try_from(size).unwrap_or_default(),
        storage: col_enum(row, 5)?,
        storage_id: row.try_get(6)?,
        error: row.try_get(7)?,
        created_at: row.try_get(8)?,
        finished_at: row.try_get(9)?,
    })
}

const CHANNEL_SELECT: &str =
    "SELECT id, name, kind, enabled, config, created_at, last_test FROM notify_channels";

fn channel_from_row(row: &PgRow) -> sqlx::Result<NotifyChannel> {
    let config: ChannelConfig = col_json(row, 4)?;
    Ok(NotifyChannel {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        kind: col_enum(row, 2)?,
        enabled: row.try_get(3)?,
        target: config.target(),
        config,
        created_at: row.try_get(5)?,
        last_test: row
            .try_get::<Option<Json<ChannelTest>>, _>(6)?
            .map(|Json(t)| t),
    })
}

const RULE_SELECT: &str = "SELECT id, channel_id, scope_type, scope_id, preset, events, enabled,
        dedupe_secs, max_per_hour, created_at FROM notify_rules";

fn rule_from_row(row: &PgRow) -> sqlx::Result<NotifyRule> {
    let scope_type: String = row.try_get(2)?;
    let scope_id: Option<String> = row.try_get(3)?;
    let scope = match (scope_type.as_str(), scope_id) {
        ("instance", _) => RuleScope::Instance,
        ("project", Some(id)) => RuleScope::Project { id },
        ("environment", Some(id)) => RuleScope::Environment { id },
        ("service", Some(id)) => RuleScope::Service { id },
        (other, _) => return Err(decode_error(2, format!("unknown rule scope '{other}'"))),
    };
    Ok(NotifyRule {
        id: row.try_get(0)?,
        channel_id: row.try_get(1)?,
        scope,
        scope_label: String::new(),
        preset: row.try_get(4)?,
        events: col_json(row, 5)?,
        enabled: row.try_get(6)?,
        dedupe_secs: u32::try_from(row.try_get::<i64, _>(7)?).unwrap_or(u32::MAX),
        max_per_hour: u32::try_from(row.try_get::<i64, _>(8)?).unwrap_or(u32::MAX),
        created_at: row.try_get(9)?,
    })
}

const STORAGE_SELECT: &str = "SELECT id, name, endpoint, region, bucket, path_prefix, path_style,
        credentials_secret_id, created_at, last_test FROM storage_connections";

fn storage_from_row(row: &PgRow) -> sqlx::Result<StorageConnection> {
    Ok(StorageConnection {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        endpoint: row.try_get(2)?,
        region: row.try_get(3)?,
        bucket: row.try_get(4)?,
        path_prefix: row.try_get(5)?,
        path_style: row.try_get(6)?,
        credentials_secret_id: row.try_get(7)?,
        created_at: row.try_get(8)?,
        last_test: row
            .try_get::<Option<Json<StorageTest>>, _>(9)?
            .map(|Json(t)| t),
    })
}

fn secret_from_row(row: &PgRow) -> sqlx::Result<Secret> {
    Ok(Secret {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        kind: col_enum(row, 2)?,
        public_part: row.try_get(3)?,
        created_at: row.try_get(4)?,
        updated_at: row.try_get(5)?,
        used_by: Vec::new(),
    })
}

const DEPLOYMENT_SELECT: &str = "SELECT id, service_id, number, status, trigger, commit_sha, commit_message, image, container_id,
        steps, error, created_at, finished_at FROM deployments";

fn server_from_row(row: &PgRow) -> sqlx::Result<Server> {
    Ok(Server {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        kind: col_enum(row, 2)?,
        host: row.try_get(3)?,
        user: row.try_get(4)?,
        port: col_port(row, 5)?,
        status: col_enum(row, 6)?,
        proxy: col_enum(row, 7)?,
        docker_version: row.try_get(8)?,
        last_checked_at: row.try_get(9)?,
        created_at: row.try_get(10)?,
        ssh_key_secret_id: row.try_get(11)?,
        checks: row.try_get::<Json<Vec<PreflightCheck>>, _>(12)?.0,
        problem: row
            .try_get::<Option<Json<ServerProblem>>, _>(13)?
            .map(|j| j.0),
    })
}

fn project_from_row(row: &PgRow) -> sqlx::Result<Project> {
    Ok(Project {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        description: row.try_get(2)?,
        created_at: row.try_get(3)?,
    })
}

fn environment_from_row(row: &PgRow) -> sqlx::Result<Environment> {
    Ok(Environment {
        id: row.try_get(0)?,
        project_id: row.try_get(1)?,
        name: row.try_get(2)?,
        created_at: row.try_get(3)?,
    })
}

fn deployment_from_row(row: &PgRow) -> sqlx::Result<Deployment> {
    Ok(Deployment {
        id: row.try_get(0)?,
        service_id: row.try_get(1)?,
        number: row.try_get(2)?,
        status: col_enum(row, 3)?,
        trigger: row.try_get(4)?,
        commit: row.try_get(5)?,
        commit_message: row.try_get(6)?,
        image: row.try_get(7)?,
        container_id: row.try_get(8)?,
        steps: col_json(row, 9)?,
        error: row.try_get(10)?,
        created_at: row.try_get(11)?,
        finished_at: row.try_get(12)?,
    })
}

type Tx = Transaction<'static, Postgres>;

/// Serializes writers that number rows per service (deployments, backups) or replace its env.
async fn lock_service(tx: &mut Tx, service_id: &str) -> sqlx::Result<()> {
    sqlx::query("SELECT 1 FROM services WHERE id = $1 FOR UPDATE")
        .bind(service_id)
        .fetch_optional(&mut **tx)
        .await?;
    Ok(())
}

async fn insert_environment(tx: &mut Tx, e: &Environment) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO environments (id, project_id, name, created_at) VALUES ($1, $2, $3, $4)",
    )
    .bind(&e.id)
    .bind(&e.project_id)
    .bind(&e.name)
    .bind(e.created_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_domain(tx: &mut Tx, d: &Domain) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO domains (id, service_id, host, port, tls, created_at, compose_service)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&d.id)
    .bind(&d.service_id)
    .bind(&d.host)
    .bind(d.port.map(i32::from))
    .bind(d.tls)
    .bind(d.created_at)
    .bind(&d.compose_service)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_volume(tx: &mut Tx, v: &Volume) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO volumes (id, service_id, name, mount_path, created_at) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&v.id)
    .bind(&v.service_id)
    .bind(&v.name)
    .bind(&v.mount_path)
    .bind(v.created_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn replace_env(tx: &mut Tx, service_id: &str, vars: &[EnvVar]) -> sqlx::Result<()> {
    lock_service(tx, service_id).await?;
    sqlx::query("DELETE FROM env_vars WHERE service_id = $1")
        .bind(service_id)
        .execute(&mut **tx)
        .await?;
    for (position, var) in vars.iter().enumerate() {
        sqlx::query(
            "INSERT INTO env_vars (service_id, position, key, value, secret) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(service_id)
        .bind(i32::try_from(position).unwrap_or(i32::MAX))
        .bind(&var.key)
        .bind(&var.value)
        .bind(var.secret)
        .execute(&mut **tx)
        .await?;
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

fn decode_error(
    idx: usize,
    e: impl Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
) -> sqlx::Error {
    sqlx::Error::ColumnDecode {
        index: idx.to_string(),
        source: e.into(),
    }
}

fn parse_enum<T: DeserializeOwned>(s: &str, idx: usize) -> sqlx::Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| decode_error(idx, e))
}

fn col_enum<T: DeserializeOwned>(row: &PgRow, idx: usize) -> sqlx::Result<T> {
    let s: String = row.try_get(idx)?;
    parse_enum(&s, idx)
}

fn col_json<T: DeserializeOwned>(row: &PgRow, idx: usize) -> sqlx::Result<T> {
    let Json(value) = row.try_get::<Json<T>, _>(idx)?;
    Ok(value)
}

fn bucket_from_row(row: &PgRow) -> sqlx::Result<Bucket> {
    Ok(Bucket {
        container: row.try_get(0)?,
        at: row.try_get(1)?,
        samples: i64::from(row.try_get::<i32, _>(2)?),
        cpu_sum: row.try_get(3)?,
        cpu_max: row.try_get(4)?,
        mem_sum: row.try_get(5)?,
        mem_max: row.try_get(6)?,
        mem_limit: row.try_get(7)?,
        rate_samples: i64::from(row.try_get::<i32, _>(8)?),
        net_rx_sum: row.try_get(9)?,
        net_tx_sum: row.try_get(10)?,
        disk_read_sum: row.try_get(11)?,
        disk_write_sum: row.try_get(12)?,
        pids_max: row.try_get(13)?,
        restarts: row.try_get(14)?,
        uptime: row.try_get(15)?,
        http_secs: row.try_get(16)?,
        classes: [
            row.try_get(17)?,
            row.try_get(18)?,
            row.try_get(19)?,
            row.try_get(20)?,
        ],
        latency_requests: row.try_get(21)?,
        p50_wsum: row.try_get(22)?,
        p95_wsum: row.try_get(23)?,
    })
}

fn col_port(row: &PgRow, idx: usize) -> sqlx::Result<Option<u16>> {
    row.try_get::<Option<i32>, _>(idx)?
        .map(|p| u16::try_from(p).map_err(|e| decode_error(idx, e)))
        .transpose()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A migrated store on its own database, or `None` when Docker is unavailable.
    pub async fn test_db() -> Option<Db> {
        let url = crate::testpg::database_url()?;
        Some(Db::connect(&url).await.unwrap())
    }

    async fn project(db: &Db) -> (Project, Environment) {
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
        db.insert_project(&p, &e).await.unwrap();
        (p, e)
    }

    async fn server(db: &Db) -> Server {
        let s = Server {
            id: new_id(),
            name: "local".into(),
            kind: ServerKind::Local,
            host: None,
            user: None,
            port: None,
            ssh_key_secret_id: None,
            status: ServerStatus::Unknown,
            proxy: ProxyStatus::Missing,
            docker_version: None,
            last_checked_at: None,
            created_at: Utc::now(),
            checks: Vec::new(),
            problem: None,
        };
        db.insert_server(&s).await.unwrap();
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

    async fn raw_service(db: &Db, env: &Environment, srv: &Server, source: &str) -> Id {
        let id = new_id();
        sqlx::query(
            "INSERT INTO services (id, environment_id, server_id, name, source, restart, created_at)
             VALUES ($1, $2, $3, 'web', $4::jsonb, 'always', $5)",
        )
        .bind(&id)
        .bind(&env.id)
        .bind(&srv.id)
        .bind(source)
        .bind(Utc::now())
        .execute(db.pool())
        .await
        .unwrap();
        id
    }

    #[test]
    fn ids_are_valid_names() {
        for _ in 0..100 {
            assert!(validate_name(&new_id()).is_ok());
        }
    }

    #[tokio::test]
    async fn service_round_trip_with_live_switch() {
        let Some(db) = test_db().await else { return };
        assert!(db.is_empty().await.unwrap());
        let srv = server(&db).await;
        assert!(!db.is_empty().await.unwrap());
        let (p, env) = project(&db).await;
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
        let now = Utc::now();
        let domains = ["a.example.com", "b.example.com"].map(|host| Domain {
            id: new_id(),
            service_id: svc.id.clone(),
            host: host.into(),
            port: None,
            tls: true,
            compose_service: None,
            created_at: now,
        });
        let env_vars = [
            EnvVar {
                key: "B".into(),
                value: "2".into(),
                secret: true,
            },
            EnvVar {
                key: "A".into(),
                value: "1".into(),
                secret: false,
            },
        ];
        db.insert_service(&NewService {
            service: &svc,
            env: &env_vars,
            domains: &domains,
            volumes: &[],
        })
        .await
        .unwrap();
        assert!(db.domain_taken("a.example.com").await.unwrap());
        assert!(db.service_name_taken(&env.id, "web", None).await.unwrap());
        assert!(
            !db.service_name_taken(&env.id, "web", Some(&svc.id))
                .await
                .unwrap()
        );
        let stored = db.domains(&svc.id).await.unwrap();
        assert_eq!(
            stored.iter().map(|d| d.host.as_str()).collect::<Vec<_>>(),
            ["a.example.com", "b.example.com"],
            "same-timestamp rows keep insertion order"
        );

        let mut first = deployment(&svc.id);
        db.insert_deployment(&mut first, &srv.id).await.unwrap();
        let mut second = deployment(&svc.id);
        db.insert_deployment(&mut second, &srv.id).await.unwrap();
        assert_eq!((first.number, second.number), (1, 2));

        let rec = db.service(&svc.id).await.unwrap().unwrap();
        assert_eq!(rec.busy, Some(DeploymentStatus::Queued));
        assert_eq!(rec.service.primary_domain.as_deref(), Some("a.example.com"));
        assert_eq!(rec.service.source, svc.source);
        assert_eq!(rec.service.port, Some(80));

        first.finished_at = Some(Utc::now());
        db.switch_live(&first, None).await.unwrap();
        second.finished_at = Some(Utc::now());
        db.switch_live(&second, None).await.unwrap();
        let first = db.deployment(&first.id).await.unwrap().unwrap();
        assert_eq!(first.status, DeploymentStatus::Superseded);
        let rec = db.service(&svc.id).await.unwrap().unwrap();
        assert_eq!(
            rec.service.live_deployment_id.as_deref(),
            Some(second.id.as_str())
        );
        assert_eq!(rec.busy, None);
        assert_eq!(db.env(&svc.id).await.unwrap(), env_vars);

        db.delete_project(&p.id).await.unwrap();
        assert!(db.service(&svc.id).await.unwrap().is_none());
        assert!(db.deployment(&second.id).await.unwrap().is_none());
    }

    /// Concurrent deploys of one service still get distinct, contiguous numbers.
    #[tokio::test]
    async fn concurrent_deployments_number_contiguously() {
        let Some(db) = test_db().await else { return };
        let srv = server(&db).await;
        let (_, env) = project(&db).await;
        let svc_id = raw_service(&db, &env, &srv, r#"{"type":"image","image":"nginx"}"#).await;
        let inserts = (0..10).map(|_| {
            let db = db.clone();
            let srv_id = srv.id.clone();
            let mut d = deployment(&svc_id);
            async move {
                db.insert_deployment(&mut d, &srv_id).await.unwrap();
                d.number
            }
        });
        let mut numbers = futures_util::future::join_all(inserts).await;
        numbers.sort_unstable();
        assert_eq!(numbers, (1..=10).collect::<Vec<i64>>());
    }

    #[tokio::test]
    async fn interrupted_deployments_fail_on_startup() {
        let Some(db) = test_db().await else { return };
        let srv = server(&db).await;
        let (_, env) = project(&db).await;
        let svc_id = raw_service(&db, &env, &srv, r#"{"type":"image","image":"nginx"}"#).await;
        let mut d = deployment(&svc_id);
        d.steps = vec![Step {
            kind: StepKind::Pull,
            status: StepStatus::Running,
            detail: None,
            started_at: Some(Utc::now()),
            finished_at: None,
        }];
        db.insert_deployment(&mut d, &srv.id).await.unwrap();
        assert_eq!(db.fail_interrupted_deployments().await.unwrap(), 1);
        let d = db.deployment(&d.id).await.unwrap().unwrap();
        assert_eq!(d.status, DeploymentStatus::Failed);
        assert_eq!(d.error.as_deref(), Some("daemon restarted"));
        assert_eq!(d.steps[0].status, StepStatus::Failed);
    }

    #[tokio::test]
    async fn backups_number_per_service_and_fail_on_restart() {
        let Some(db) = test_db().await else { return };
        let srv = server(&db).await;
        let (_, env) = project(&db).await;
        let svc_id = raw_service(
            &db,
            &env,
            &srv,
            r#"{"type":"database","kind":"redis","version":"8-alpine"}"#,
        )
        .await;
        let backup = || Backup {
            id: new_id(),
            service_id: svc_id.clone(),
            created_at: "2026-11-20T03:00:00Z".parse().unwrap(),
            size_bytes: 0,
            status: BackupStatus::Running,
            trigger: BackupTrigger::Manual,
            file_name: String::new(),
            error: None,
            storage: BackupStorage::Local,
            storage_id: None,
            finished_at: None,
        };
        let mut first = backup();
        db.insert_backup(&mut first, "rdb").await.unwrap();
        let mut second = backup();
        db.insert_backup(&mut second, "rdb").await.unwrap();
        assert_eq!(first.file_name, "20261120-030000-1.rdb.gz");
        assert_eq!(second.file_name, "20261120-030000-2.rdb.gz");
        second.status = BackupStatus::Done;
        second.size_bytes = 42;
        db.update_backup(&second).await.unwrap();

        assert_eq!(db.fail_interrupted_backups().await.unwrap(), 1);
        let list = db.backups(&svc_id).await.unwrap();
        assert_eq!(list[0].id, second.id);
        assert_eq!(list[0].size_bytes, 42);
        assert_eq!(list[1].status, BackupStatus::Failed);
        assert_eq!(list[1].error.as_deref(), Some("daemon restarted"));

        assert!(!db.backup_schedule(&svc_id).await.unwrap().schedule.enabled);
        let record = ScheduleRecord {
            service_id: svc_id.clone(),
            schedule: BackupSchedule {
                enabled: true,
                hour_utc: 5,
                keep: 3,
                storage_id: None,
            },
            last_run_at: Some(Utc::now()),
        };
        db.save_backup_schedule(&record).await.unwrap();
        assert_eq!(
            db.backup_schedule(&svc_id).await.unwrap().schedule,
            record.schedule
        );
        assert_eq!(db.enabled_backup_schedules().await.unwrap().len(), 1);

        db.set_public_port(&svc_id, Some(15432)).await.unwrap();
        assert_eq!(db.public_port(&svc_id).await.unwrap(), Some(15432));
        assert!(db.public_port_taken(&srv.id, 15432, "other").await.unwrap());
        assert!(!db.public_port_taken(&srv.id, 15432, &svc_id).await.unwrap());

        db.delete_service(&svc_id).await.unwrap();
        assert!(db.backup(&second.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn metrics_roll_up_and_expire_per_level() {
        use crate::metrics::{HttpSample, Rates};
        use chrono::TimeDelta;
        let Some(db) = test_db().await else { return };
        let (_, env) = project(&db).await;
        let srv = server(&db).await;
        let svc_id = raw_service(&db, &env, &srv, r#"{"type":"image","image":"nginx"}"#).await;
        // 2023-11-14 22:00:00 UTC: on a boundary of every level.
        let hour = chrono::DateTime::from_timestamp(1_699_999_200, 0).unwrap();
        let secs = TimeDelta::seconds;
        for i in 0..12u32 {
            let sample = Sample {
                cpu: f64::from(i),
                mem: 100 + u64::from(i) * 10,
                mem_limit: 1000,
                rates: (i > 0).then_some(Rates {
                    net_rx: 10.0,
                    net_tx: 5.0,
                    disk_read: 0.0,
                    disk_write: 2.0,
                }),
                pids: u64::from(i % 4),
                restarts: 1,
                uptime_secs: i64::from(i) * 10,
                http: Some(HttpSample {
                    secs: 10.0,
                    classes: [8, 0, 1, 1],
                    p50_ms: Some(10.0 + f64::from(i % 2) * 10.0),
                    p95_ms: Some(50.0),
                }),
            };
            db.insert_metric_sample(&svc_id, "", hour + secs(i64::from(i) * 10), &sample)
                .await
                .unwrap();
        }
        let points = |res, from| {
            let db = db.clone();
            let svc_id = svc_id.clone();
            async move {
                crate::metrics::combine(&db.metric_buckets(&svc_id, res, from, None).await.unwrap())
            }
        };

        let raw = points(10, hour).await;
        assert_eq!(raw.len(), 12);
        assert_eq!(raw[3].cpu, 3.0);
        assert_eq!(raw[0].net_rx, 0.0);

        let minutes = points(60, hour).await;
        assert_eq!(minutes.len(), 2);
        let first = &minutes[0];
        assert_eq!(first.at, hour);
        assert_eq!(first.cpu, 2.5);
        assert_eq!(first.cpu_max, 5.0);
        assert_eq!(first.mem, 125);
        assert_eq!(first.mem_max, 150);
        // The first sample had no rates, so they average over the other five.
        assert_eq!(first.net_rx, 10.0);
        assert_eq!(first.pids, 3);
        assert_eq!(first.uptime_secs, 50);
        let http = first.http.as_ref().unwrap();
        assert_eq!((http.s2xx, http.s4xx, http.s5xx), (48, 6, 6));
        assert_eq!(http.rps, 1.0);
        assert_eq!(http.p50_ms, Some(15.0));
        assert_eq!(minutes[1].at, hour + secs(60));
        assert_eq!(minutes[1].cpu, 8.5);

        let hours = points(3600, hour).await;
        assert_eq!(hours.len(), 1);
        assert_eq!(hours[0].cpu, 5.5);
        assert_eq!(hours[0].mem_max, 210);
        assert_eq!(points(600, hour).await.len(), 1);
        assert_eq!(db.metric_containers(&svc_id, 60, hour).await.unwrap(), [""]);

        let level_rows = |res| {
            let db = db.clone();
            let svc_id = svc_id.clone();
            async move {
                db.metric_buckets(&svc_id, res, hour - secs(86_400 * 60), None)
                    .await
                    .unwrap()
                    .len()
            }
        };
        assert_eq!(db.expire_metrics(hour + secs(3600 + 59)).await.unwrap(), 6);
        assert_eq!(level_rows(10).await, 6);
        db.expire_metrics(hour + TimeDelta::hours(2)).await.unwrap();
        assert_eq!(level_rows(10).await, 0);
        assert_eq!(level_rows(60).await, 2);
        db.expire_metrics(hour + TimeDelta::hours(25))
            .await
            .unwrap();
        assert_eq!(level_rows(60).await, 0);
        assert_eq!(level_rows(600).await, 1);
        db.expire_metrics(hour + TimeDelta::days(8)).await.unwrap();
        assert_eq!(level_rows(600).await, 0);
        assert_eq!(level_rows(3600).await, 1);
        db.expire_metrics(hour + TimeDelta::days(31)).await.unwrap();
        assert_eq!(level_rows(3600).await, 0);
    }

    #[tokio::test]
    async fn metrics_go_with_their_service_and_deploys_mark_the_timeline() {
        let Some(db) = test_db().await else { return };
        let (_, env) = project(&db).await;
        let srv = server(&db).await;
        let svc_id = raw_service(&db, &env, &srv, r#"{"type":"image","image":"nginx"}"#).await;
        let now = Utc::now();
        for container in ["web", "worker"] {
            db.insert_metric_sample(&svc_id, container, now, &Sample::default())
                .await
                .unwrap();
        }
        let from = now - chrono::TimeDelta::hours(1);
        assert_eq!(
            db.metric_containers(&svc_id, 10, from).await.unwrap(),
            ["web", "worker"]
        );
        assert_eq!(
            db.metric_buckets(&svc_id, 10, from, Some("web"))
                .await
                .unwrap()
                .len(),
            1
        );

        let mut statuses = Vec::new();
        for status in [
            DeploymentStatus::Superseded,
            DeploymentStatus::Failed,
            DeploymentStatus::Live,
        ] {
            let mut d = deployment(&svc_id);
            db.insert_deployment(&mut d, &srv.id).await.unwrap();
            d.status = status;
            d.finished_at = Some(now - chrono::TimeDelta::minutes(10 - d.number));
            db.update_deployment(&d).await.unwrap();
            statuses.push(d.number);
        }
        let markers = db.deploy_markers(&svc_id, from).await.unwrap();
        assert_eq!(
            markers
                .iter()
                .map(|m| (m.number, m.status))
                .collect::<Vec<_>>(),
            [
                (statuses[0], DeploymentStatus::Superseded),
                (statuses[2], DeploymentStatus::Live)
            ]
        );
        assert!(db.deploy_markers(&svc_id, now).await.unwrap().is_empty());

        db.delete_service(&svc_id).await.unwrap();
        assert!(
            db.metric_containers(&svc_id, 10, from)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn activity_is_newest_first() {
        let Some(db) = test_db().await else { return };
        for message in ["one", "two", "three"] {
            db.insert_activity(&Activity {
                id: 0,
                at: Utc::now(),
                level: ActivityLevel::Info,
                service_id: None,
                service_name: None,
                project_id: None,
                message: message.into(),
                detail: None,
            })
            .await
            .unwrap();
        }
        let list = db.activity(2).await.unwrap();
        assert_eq!(
            list.iter().map(|a| a.message.as_str()).collect::<Vec<_>>(),
            ["three", "two"]
        );
    }
}
