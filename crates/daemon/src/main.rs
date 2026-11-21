//! The control daemon: serves the HTTP API and drives Docker on every registered server.

mod api;
mod backup_store;
mod build;
mod cloudflare;
mod config;
mod database;
mod db;
mod deploy;
mod docker;
mod domain;
mod error;
mod github;
mod host;
mod instance;
mod notify;
mod onboarding;
mod preflight;
mod prune;
mod restore;
mod secrets;
mod services;
mod sources;
mod sqlite_import;
mod ssh;
mod state;
mod stats;
mod store;
#[cfg(test)]
#[path = "../tests/support/minio.rs"]
mod testminio;
#[cfg(test)]
#[path = "../tests/support/pg.rs"]
mod testpg;
mod update;

use anyhow::{Context as _, Result, bail};
use chrono::Utc;
use clap::{CommandFactory as _, FromArgMatches as _, Parser, Subcommand};
use common::{DEFAULT_PORT, ProxyStatus, Server, ServerKind, ServerStatus, brand};
use config::{Config, DataDir};
use db::Db;
use state::AppState;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = brand::DAEMON_BIN, version, about = "Control daemon: HTTP API, deploys and Docker orchestration")]
struct Cli {
    /// Data directory.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    /// Address the API listens on; overrides `listen` in config.toml (default 0.0.0.0:7700).
    #[arg(long, global = true)]
    listen: Option<String>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Create the data directory, API token, SSH key and the local server.
    Init {
        /// Re-initialize: issue a new token (data, servers and the SSH key are kept).
        #[arg(long)]
        force: bool,
        /// Also write the token to this file (mode 0600), for install scripts.
        #[arg(long)]
        token_file: Option<PathBuf>,
        /// Don't install and supervise the reverse proxy on this machine.
        #[arg(long)]
        no_proxy: bool,
        /// Keep state in this Postgres instead of running a managed container.
        #[arg(long)]
        database_url: Option<String>,
        /// Port on 127.0.0.1 the managed Postgres container is published on [default: 5439].
        #[arg(long)]
        postgres_port: Option<u16>,
    },
    /// Import a SQLite store from an older version into the (empty) Postgres store.
    /// `serve` does this on its own when it finds one in the data directory.
    MigrateFromSqlite {
        /// The SQLite file, e.g. /var/lib/<dir>/<dir>.db.
        path: PathBuf,
    },
    /// Serve the API (the default).
    Serve,
    /// Disaster recovery: restore an instance backup onto this (freshly installed) machine.
    /// Stops the service, restores the store, config, master key and SSH keys, starts the
    /// service again and checks every server. S3 sources read AWS_ACCESS_KEY_ID and
    /// AWS_SECRET_ACCESS_KEY.
    Restore {
        /// The encrypted archive: a local file or s3://<bucket>/<key>.
        source: String,
        /// File holding the backup's passphrase.
        #[arg(long)]
        passphrase_file: PathBuf,
        /// S3 endpoint of an s3:// source, e.g. https://<account>.r2.cloudflarestorage.com.
        #[arg(long, default_value = "https://s3.amazonaws.com")]
        s3_endpoint: String,
        #[arg(long, default_value = "us-east-1")]
        s3_region: String,
        /// Address the bucket as <endpoint>/<bucket> (MinIO and most self-hosted stores).
        #[arg(long)]
        s3_path_style: bool,
        /// Keep the restored store in this Postgres instead of where the backup's config says.
        #[arg(long)]
        database_url: Option<String>,
        /// Don't stop and start the systemd service.
        #[arg(long)]
        no_service: bool,
    },
    /// Manage the API token.
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// Write a systemd unit for the daemon.
    InstallService {
        /// Where to write the unit [default: /etc/systemd/system/<daemon>.service].
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum TokenAction {
    /// Issue a new token; the old one stops working immediately.
    Rotate,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,bollard=warn,hyper=warn,sqlx=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = parse_cli()?;
    let dirs = DataDir::new(cli.data_dir.unwrap_or_else(config::default_data_dir));
    let listen = cli.listen;
    match cli.command.unwrap_or(Command::Serve) {
        Command::Init {
            force,
            token_file,
            no_proxy,
            database_url,
            postgres_port,
        } => tokio::runtime::Runtime::new()?.block_on(init(
            &dirs,
            InitOptions {
                force,
                token_file,
                manage_proxy: !no_proxy,
                database_url,
                postgres_port,
            },
            listen,
        )),
        Command::Serve => tokio::runtime::Runtime::new()?.block_on(serve(dirs, listen)),
        Command::Restore {
            source,
            passphrase_file,
            s3_endpoint,
            s3_region,
            s3_path_style,
            database_url,
            no_service,
        } => tokio::runtime::Runtime::new()?.block_on(restore::run(
            &dirs,
            restore::Options {
                source,
                passphrase_file,
                s3_endpoint,
                s3_region,
                s3_path_style,
                database_url,
                no_service,
            },
        )),
        Command::MigrateFromSqlite { path } => {
            tokio::runtime::Runtime::new()?.block_on(migrate_from_sqlite(&dirs, &path))
        }
        Command::Token {
            action: TokenAction::Rotate,
        } => rotate_token(&dirs),
        Command::InstallService { path } => install_service(&dirs, path),
    }
}

/// Parses arguments with the env var names and defaults that derive from the brand.
fn parse_cli() -> Result<Cli> {
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let data_dir_env = leak(brand::env_var("DATA_DIR"));
    let listen_env = leak(brand::env_var("LISTEN"));
    let default_dir = leak(config::default_data_dir().display().to_string());
    let matches = Cli::command()
        .mut_arg("data_dir", |a| {
            a.env(data_dir_env).default_value(default_dir)
        })
        .mut_arg("listen", |a| a.env(listen_env))
        .get_matches();
    Ok(Cli::from_arg_matches(&matches)?)
}

fn require_initialized(dirs: &DataDir) -> Result<()> {
    if !dirs.is_initialized() {
        bail!(
            "{} is not initialized: run `{} init` first",
            dirs.root().display(),
            brand::DAEMON_BIN
        );
    }
    Ok(())
}

struct InitOptions {
    force: bool,
    token_file: Option<PathBuf>,
    manage_proxy: bool,
    database_url: Option<String>,
    postgres_port: Option<u16>,
}

async fn init(dirs: &DataDir, opts: InitOptions, listen: Option<String>) -> Result<()> {
    if dirs.is_initialized() && !opts.force {
        bail!(
            "{} is already initialized; pass --force to issue a new token",
            dirs.root().display()
        );
    }
    dirs.create()?;
    secrets::MasterKey::load_or_create(&dirs.master_key())?;
    if !dirs.ssh_key().exists() {
        let status = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C"])
            .arg(format!("{}@{}", brand::DAEMON_BIN, host::hostname()))
            .arg("-f")
            .arg(dirs.ssh_key())
            .status()
            .context("cannot run ssh-keygen (install openssh-client)")?;
        if !status.success() {
            bail!("ssh-keygen failed with {status}");
        }
    }
    let token = config::generate_token();
    let existing = Config::load(dirs).ok().filter(|_| opts.force);
    let existing_ip_override = existing.as_ref().and_then(|c| c.public_ip_override.clone());
    let existing_domain = existing.as_ref().and_then(|c| c.domain.clone());
    let existing_domain_verified_at = existing.as_ref().and_then(|c| c.domain_verified_at);
    let manage_proxy = opts.manage_proxy && existing.as_ref().is_none_or(|c| c.manage_proxy);
    let database_url = opts
        .database_url
        .or_else(|| existing.as_ref().and_then(|c| c.database_url.clone()));
    let postgres = match existing.and_then(|c| c.postgres) {
        Some(mut managed) => {
            managed.port = opts.postgres_port.unwrap_or(managed.port);
            managed
        }
        None => config::ManagedPostgres::generate(
            opts.postgres_port
                .unwrap_or_else(config::default_postgres_port),
        ),
    };
    let mut config = Config {
        token_sha256: hex::encode(config::hash_token(&token)),
        listen: listen.unwrap_or_else(config::default_listen),
        manage_proxy,
        postgres: database_url.is_none().then_some(postgres),
        database_url,
        public_ip_override: existing_ip_override,
        domain: existing_domain,
        domain_verified_at: existing_domain_verified_at,
    };
    config.save(dirs)?;
    if let Some(path) = &opts.token_file {
        std::fs::write(
            path,
            format!(
                "{token}
"
            ),
        )
        .with_context(|| format!("cannot write {}", path.display()))?;
        config::set_mode(path, 0o600)?;
    }
    open_store(dirs, &mut config).await?;

    let port = config
        .listen
        .rsplit(':')
        .next()
        .unwrap_or("")
        .parse()
        .unwrap_or(DEFAULT_PORT);
    let ip = config.public_ip().unwrap_or_else(|| "<server-ip>".into());
    let public_key = std::fs::read_to_string(dirs.ssh_public_key()).unwrap_or_default();
    println!("Initialized {}", dirs.root().display());
    println!();
    println!("SSH public key (add it to ~/.ssh/authorized_keys on remote servers):");
    println!("  {}", public_key.trim());
    println!();
    println!("API token (shown only once):");
    println!("  {token}");
    println!();
    println!("Connect from your machine with:");
    println!(
        "  {} login http://{ip}:{port} --token {token}",
        brand::CLI_BIN
    );
    println!();
    println!(
        "Start the daemon with `{0} serve`, or `{0} install-service` to run it under systemd.",
        brand::DAEMON_BIN
    );
    Ok(())
}

/// Connects to the store, imports a SQLite store left by an older version, and makes sure the
/// local server exists.
async fn open_store(dirs: &DataDir, config: &mut Config) -> Result<Db> {
    let db = store::connect(dirs, config).await?;
    let sqlite = dirs.db();
    let imported = sqlite_import::import_if_present(&db, &sqlite)
        .await
        .with_context(|| {
            format!(
                "cannot import {} into Postgres; the SQLite file was left untouched. \
                 Fix the cause and restart, or run `{} migrate-from-sqlite {}`",
                sqlite.display(),
                brand::DAEMON_BIN,
                sqlite.display()
            )
        })?;
    if let Some(imported) = imported {
        record_import(&db, &sqlite, &imported).await?;
    }
    ensure_local_server(&db).await?;
    onboarding::init(&db).await?;
    Ok(db)
}

async fn record_import(
    db: &Db,
    source: &std::path::Path,
    imported: &sqlite_import::Imported,
) -> Result<()> {
    tracing::info!(
        "imported {} rows from {} into Postgres; the SQLite file is kept as {}",
        imported.rows,
        source.display(),
        imported.backup.display()
    );
    db.insert_activity(&common::Activity {
        id: 0,
        at: Utc::now(),
        level: common::ActivityLevel::Success,
        service_id: None,
        service_name: None,
        project_id: None,
        message: "Moved the store from SQLite to Postgres".into(),
        detail: Some(format!(
            "{} rows imported; the SQLite file is kept as {}",
            imported.rows,
            imported.backup.display()
        )),
    })
    .await
}

async fn migrate_from_sqlite(dirs: &DataDir, path: &std::path::Path) -> Result<()> {
    require_initialized(dirs)?;
    let mut config = Config::load(dirs)?;
    let db = store::connect(dirs, &mut config).await?;
    let imported = sqlite_import::import(&db, path).await?;
    record_import(&db, path, &imported).await?;
    ensure_local_server(&db).await?;
    println!(
        "Imported {} rows from {}; the file is kept as {}",
        imported.rows,
        path.display(),
        imported.backup.display()
    );
    Ok(())
}

async fn ensure_local_server(db: &Db) -> Result<()> {
    if db.local_server().await?.is_some() {
        return Ok(());
    }
    db.insert_server(&Server {
        id: db::new_id(),
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
    })
    .await
}

fn rotate_token(dirs: &DataDir) -> Result<()> {
    require_initialized(dirs)?;
    let mut config = Config::load(dirs)?;
    let token = config::generate_token();
    config.token_sha256 = hex::encode(config::hash_token(&token));
    config.save(dirs)?;
    println!("New API token (the old one no longer works):");
    println!("  {token}");
    println!();
    println!("Update your CLI with:");
    let ip = config.public_ip().unwrap_or_else(|| "<server-ip>".into());
    let port = config
        .listen
        .rsplit(':')
        .next()
        .unwrap_or("")
        .parse()
        .unwrap_or(DEFAULT_PORT);
    println!(
        "  {} login http://{ip}:{port} --token {token}",
        brand::CLI_BIN
    );
    Ok(())
}

fn install_service(dirs: &DataDir, path: Option<PathBuf>) -> Result<()> {
    let unit_name = format!("{}.service", brand::DAEMON_BIN);
    let path = path.unwrap_or_else(|| PathBuf::from("/etc/systemd/system").join(&unit_name));
    let exe = std::env::current_exe().context("cannot locate the daemon binary")?;
    let unit = format!(
        "[Unit]
Description={name} control daemon
After=network-online.target docker.service
Wants=network-online.target
Requires=docker.service

[Service]
ExecStart={exe} --data-dir {data} serve
Restart=always
RestartSec=2
Environment=RUST_LOG=info,sqlx=warn

[Install]
WantedBy=multi-user.target
",
        name = brand::NAME,
        exe = exe.display(),
        data = dirs.root().display(),
    );
    std::fs::write(&path, unit).with_context(|| format!("cannot write {}", path.display()))?;
    println!("Wrote {}", path.display());
    if !dirs.is_initialized() {
        println!(
            "Initialize first: {} --data-dir {} init",
            brand::DAEMON_BIN,
            dirs.root().display()
        );
    }
    println!("Then enable and start it:");
    println!("  systemctl daemon-reload");
    println!("  systemctl enable --now {unit_name}");
    println!("  journalctl -u {unit_name} -f");
    Ok(())
}

async fn serve(dirs: DataDir, listen: Option<String>) -> Result<()> {
    require_initialized(&dirs)?;
    let mut config = Config::load(&dirs)?;
    dirs.create()?;
    secrets::MasterKey::load_or_create(&dirs.master_key())?;
    ssh::clear_temp_files(&ssh::paths(&dirs.ssh_dir()));
    if let Ok(entries) = std::fs::read_dir(dirs.builds_dir()) {
        for entry in entries.flatten() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    let db = open_store(&dirs, &mut config).await?;
    let interrupted = db.fail_interrupted_deployments().await?;
    if interrupted > 0 {
        tracing::warn!("marked {interrupted} interrupted deployment(s) as failed");
    }
    let interrupted = db.fail_interrupted_backups().await?;
    if interrupted > 0 {
        tracing::warn!("marked {interrupted} interrupted backup(s) as failed");
    }
    let interrupted = db.fail_interrupted_instance_backups().await?;
    if interrupted > 0 {
        tracing::warn!("marked {interrupted} interrupted instance backup(s) as failed");
    }
    let _ = std::fs::remove_dir_all(dirs.backups_dir().join(".staging"));
    let listen = listen.unwrap_or_else(|| config.listen.clone());
    let state = Arc::new(AppState::new(db, dirs, &config, listen.clone())?);
    notify::spawn_dispatcher(state.clone());
    notify::monitor::startup(&state).await;
    notify::monitor::spawn(state.clone());
    stats::spawn_sampler(state.clone());
    database::spawn_scheduler(state.clone());
    instance::spawn_scheduler(state.clone());
    tokio::spawn({
        let state = state.clone();
        let manage_proxy = config.manage_proxy;
        async move {
            for server in state.db.servers().await.unwrap_or_default() {
                let Ok(server) = api::check_server(&state, server).await else {
                    continue;
                };
                if manage_proxy
                    && server.kind == ServerKind::Local
                    && server.status == ServerStatus::Online
                    && server.proxy != ProxyStatus::Running
                {
                    api::ensure_local_proxy(&state, server).await;
                } else if server.proxy == ProxyStatus::Running
                    && let Err(e) = api::sync_proxy(&state, server).await
                {
                    tracing::warn!("cannot update the proxy: {e:#}");
                }
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("cannot listen on {listen}"))?;
    tracing::info!("listening on http://{}", listener.local_addr()?);
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}
