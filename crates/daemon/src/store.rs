//! Where the daemon keeps its own state: a Postgres container it runs on this machine, or an
//! external database from `database_url`.

use crate::config::{Config, DataDir, ManagedPostgres};
use crate::database;
use crate::db::Db;
use crate::docker::{self, ContainerSpec, RunState};
use anyhow::{Context as _, Result, bail};
use bollard::Docker;
use common::{DatabaseKind, RestartPolicy, brand};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite};

const IMAGE_TAG: &str = "18-alpine";
const READY_TIMEOUT: Duration = Duration::from_secs(90);

impl ManagedPostgres {
    pub fn generate(port: u16) -> Self {
        Self {
            password: database::generate_password(),
            port,
        }
    }

    fn url(&self) -> String {
        format!(
            "postgres://{user}:{password}@127.0.0.1:{port}/{user}?sslmode=disable",
            user = brand::PREFIX,
            password = self.password,
            port = self.port,
        )
    }
}

/// Whether the store is an external database rather than the managed container.
pub fn is_external(config: &Config) -> bool {
    external_url(config).is_some()
}

/// The external database URL, from `<PREFIX>_DATABASE_URL` or `database_url` in config.toml.
fn external_url(config: &Config) -> Option<String> {
    std::env::var(brand::env_var("DATABASE_URL"))
        .ok()
        .filter(|url| !url.is_empty())
        .or_else(|| config.database_url.clone())
}

/// Connects to the store and applies migrations. Without an external URL this first makes sure
/// the managed container exists, runs and accepts connections; installs from before the store
/// moved to Postgres get its password generated and saved here.
pub async fn connect(dirs: &DataDir, config: &mut Config) -> Result<Db> {
    if let Some(url) = external_url(config) {
        return Db::connect(&url).await;
    }
    let managed = match &config.postgres {
        Some(managed) => managed.clone(),
        None => {
            let managed = ManagedPostgres::generate(crate::config::default_postgres_port());
            config.postgres = Some(managed.clone());
            config.save(dirs)?;
            managed
        }
    };
    ensure_container(&managed).await?;
    Db::connect(&managed.url()).await
}

async fn ensure_container(managed: &ManagedPostgres) -> Result<()> {
    let docker = Docker::connect_with_defaults().context("cannot connect to Docker")?;
    docker.ping().await.with_context(|| {
        format!(
            "Docker is not reachable; {} keeps its state in the {} container and needs it running",
            brand::DAEMON_BIN,
            brand::STORE_CONTAINER
        )
    })?;
    let name = brand::STORE_CONTAINER;
    match docker::inspect(&docker, name).await? {
        Some(c) if published_port(&c) != Some(managed.port) => {
            tracing::info!("recreating {name} to publish it on port {}", managed.port);
            docker::remove_container(&docker, name).await?;
            create_container(&docker, managed).await?;
        }
        Some(c) if RunState::from_inspect(&c) == RunState::Running => {}
        Some(_) => {
            tracing::info!("starting {name}");
            docker::start_container(&docker, name).await?;
        }
        None => create_container(&docker, managed).await?,
    }
    wait_ready(&docker).await?;
    sync_password(&docker, managed).await
}

fn published_port(c: &bollard::models::ContainerInspectResponse) -> Option<u16> {
    c.host_config
        .as_ref()?
        .port_bindings
        .as_ref()?
        .get("5432/tcp")?
        .as_ref()?
        .first()?
        .host_port
        .as_ref()?
        .parse()
        .ok()
}

async fn create_container(docker: &Docker, managed: &ManagedPostgres) -> Result<()> {
    let image = format!("postgres:{IMAGE_TAG}");
    if !docker::image_exists(docker, &image).await? {
        tracing::info!("pulling {image}");
        docker::pull_image(docker, &image, |line| tracing::debug!("{image}: {line}")).await?;
    }
    docker::ensure_network(docker, brand::STORE_NETWORK).await?;
    docker::ensure_volume(docker, brand::STORE_VOLUME).await?;
    let spec = ContainerSpec {
        name: brand::STORE_CONTAINER.to_string(),
        image,
        env: vec![
            format!("POSTGRES_USER={}", brand::PREFIX),
            format!("POSTGRES_DB={}", brand::PREFIX),
            format!("POSTGRES_PASSWORD={}", managed.password),
        ],
        labels: HashMap::from([(brand::label("store"), "true".to_string())]),
        binds: vec![format!(
            "{}:{}",
            brand::STORE_VOLUME,
            DatabaseKind::Postgres.data_path(IMAGE_TAG)
        )],
        restart: RestartPolicy::Always,
        networks: vec![(brand::STORE_NETWORK.to_string(), Vec::new())],
        published: vec![("5432/tcp".into(), managed.port)],
        publish_ip: Some("127.0.0.1".into()),
        ..Default::default()
    };
    tracing::info!(
        "creating {} on 127.0.0.1:{}",
        brand::STORE_CONTAINER,
        managed.port
    );
    docker::create_container(docker, &spec).await?;
    docker::start_container(docker, brand::STORE_CONTAINER).await
}

/// Waits until Postgres accepts TCP connections. The image's first-boot init runs a server on
/// the Unix socket only, so checking over TCP skips that phase.
async fn wait_ready(docker: &Docker) -> Result<()> {
    let deadline = Instant::now() + READY_TIMEOUT;
    let check = strings(&["pg_isready", "-q", "-h", "127.0.0.1", "-U", brand::PREFIX]);
    loop {
        if docker::exec(docker, brand::STORE_CONTAINER, check.clone())
            .await
            .is_ok_and(|code| code == 0)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "{} did not accept connections within {}s; check `docker logs {}`",
                brand::STORE_CONTAINER,
                READY_TIMEOUT.as_secs(),
                brand::STORE_CONTAINER
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Sets the role's password to the one in config.toml over the container's local socket, which
/// the image trusts, so a volume that outlived its config never locks the daemon out.
async fn sync_password(docker: &Docker, managed: &ManagedPostgres) -> Result<()> {
    let sql = format!(
        "ALTER ROLE \"{}\" PASSWORD '{}'",
        brand::PREFIX,
        managed.password
    );
    let cmd = strings(&[
        "psql",
        "-q",
        "-v",
        "ON_ERROR_STOP=1",
        "-U",
        brand::PREFIX,
        "-d",
        brand::PREFIX,
        "-c",
        &sql,
    ]);
    let code = docker::exec(docker, brand::STORE_CONTAINER, cmd).await?;
    if code != 0 {
        bail!(
            "cannot set the password in {} (psql exited with {code})",
            brand::STORE_CONTAINER
        );
    }
    Ok(())
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_string()).collect()
}

/// Where the store lives, for dumping and restoring it.
enum Target {
    /// The managed container, reached with `docker exec` as its superuser.
    Managed,
    /// An external database, reached from a throwaway Postgres container on the host network.
    External(String),
}

fn target(config: &Config) -> Target {
    external_url(config).map_or(Target::Managed, Target::External)
}

/// Runs `script` with the Postgres client tools next to the store, with `DATABASE_URL` set for
/// an external store, streaming `stdin` in and stdout out.
async fn run_tool(
    config: &Config,
    script: &str,
    stdin: Option<&mut (dyn AsyncRead + Unpin + Send)>,
    stdout: &mut (dyn AsyncWrite + Unpin + Send),
) -> Result<docker::ExecOutcome> {
    let docker = Docker::connect_with_defaults().context("cannot connect to Docker")?;
    let cmd = strings(&["sh", "-c", script]);
    match target(config) {
        Target::Managed => {
            let url = format!("postgres:///{}?user={}", brand::PREFIX, brand::PREFIX);
            docker::exec_stream(
                &docker,
                brand::STORE_CONTAINER,
                cmd,
                vec![format!("DATABASE_URL={url}")],
                stdin,
                stdout,
            )
            .await
        }
        Target::External(url) => {
            let image = format!("postgres:{IMAGE_TAG}");
            if !docker::image_exists(&docker, &image).await? {
                docker::pull_image(&docker, &image, |line| tracing::debug!("{image}: {line}"))
                    .await?;
            }
            let name = format!("{}-store-tool-{}", brand::PREFIX, crate::db::new_id());
            let helper =
                docker::start_helper(&docker, &name, &image, Vec::new(), Some("host")).await?;
            let outcome = docker::exec_stream(
                &docker,
                &helper,
                cmd,
                vec![format!("DATABASE_URL={url}")],
                stdin,
                stdout,
            )
            .await;
            let removed = docker::remove_container(&docker, &helper).await;
            let outcome = outcome?;
            removed?;
            Ok(outcome)
        }
    }
}

/// Writes a `pg_dump -Fc` of the store to `out`.
pub async fn dump(config: &Config, out: &mut (dyn AsyncWrite + Unpin + Send)) -> Result<()> {
    run_tool(
        config,
        r#"exec pg_dump -Fc --no-owner -d "$DATABASE_URL""#,
        None,
        out,
    )
    .await?
    .check("pg_dump of the store")
}

/// Replaces everything in the store with a dump from [`dump`]. The managed container must be
/// running; call [`prepare`] first.
pub async fn restore(config: &Config, dump: &mut (dyn AsyncRead + Unpin + Send)) -> Result<()> {
    let script = r#"set -e
psql -q -v ON_ERROR_STOP=1 -d "$DATABASE_URL" -c 'DROP SCHEMA public CASCADE' -c 'CREATE SCHEMA public'
exec pg_restore --no-owner --no-acl --exit-on-error -d "$DATABASE_URL""#;
    run_tool(config, script, Some(dump), &mut tokio::io::sink())
        .await?
        .check("pg_restore of the store")
}

/// Makes sure the store is reachable, starting the managed container when there is one, and
/// returns its URL without connecting or migrating.
pub async fn prepare(config: &Config) -> Result<String> {
    if let Some(url) = external_url(config) {
        return Ok(url);
    }
    let managed = config
        .postgres
        .clone()
        .context("config.toml has neither database_url nor a [postgres] section")?;
    ensure_container(&managed).await?;
    Ok(managed.url())
}
