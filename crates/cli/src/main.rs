//! Command line interface. With no subcommand it opens the TUI.

mod instance;
mod notify;
mod output;
mod resolve;
mod setup;
mod sources;

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand};
use client::{Client, Config, Context};
use common::api::*;
use common::metrics::{HttpPoint, MetricPoint, MetricsRange, ServiceMetrics};
use common::*;
use futures_util::StreamExt;
use output::{Cell, Tone, muted, paint};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = brand::CLI_BIN, version, about = "Deploy and run apps on your own servers", propagate_version = true)]
struct Cli {
    /// Daemon context to use instead of the current one.
    #[arg(long, short = 'c', global = true)]
    context: Option<String>,
    /// Print machine-readable JSON where supported.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the terminal UI (the default).
    Tui,
    /// Connect to a daemon and make it the current context.
    Login {
        /// Daemon URL, e.g. http://203.0.113.10:7700.
        url: String,
        #[arg(long)]
        token: String,
        /// Context name; defaults to the daemon's hostname.
        #[arg(long)]
        name: Option<String>,
    },
    /// Manage saved daemon contexts.
    #[command(subcommand)]
    Context(ContextCmd),
    /// Forget a saved daemon context on this machine (default: the current one).
    ///
    /// This only deletes the local copy of the token: tokens are bearer tokens without a session,
    /// so the daemon keeps accepting it. Run "token rotate" against the daemon to revoke it.
    Logout {
        context: Option<String>,
        /// Forget every saved context.
        #[arg(long, conflicts_with = "context")]
        all: bool,
    },
    /// Show or change the settings of the daemon itself.
    Settings {
        #[command(subcommand)]
        cmd: Option<instance::SettingsCmd>,
    },
    /// Manage the API token.
    #[command(subcommand)]
    Token(TokenCmd),
    /// The reverse proxy on the daemon's own server.
    #[command(subcommand)]
    Proxy(ProxyCmd),
    /// Remove old build images, dangling images and the build cache on every server.
    ///
    /// The live and previous image of each service are kept so rollbacks keep working.
    Prune,
    /// Update this CLI, then the daemon of the current context, to the latest release.
    ///
    /// Each side that already runs that release is left alone. Without a logged-in context only
    /// this CLI is updated.
    Update {
        /// Release tag to install, e.g. v0.4.1 [default: the latest release].
        #[arg(long)]
        tag: Option<String>,
        /// Reinstall even when the version is not newer, and update a daemon systemd does not supervise.
        #[arg(long)]
        force: bool,
        /// Only update this CLI binary.
        #[arg(long, alias = "self")]
        cli: bool,
        /// Only update the daemon of the current context.
        #[arg(long)]
        server: bool,
    },
    /// Update this CLI binary, like "update --cli".
    Upgrade {
        /// Release tag to install, e.g. v0.4.1 [default: the latest release].
        #[arg(long)]
        tag: Option<String>,
        /// Reinstall even when the version is not newer.
        #[arg(long)]
        force: bool,
    },
    /// Daemon, host and server health.
    Status,
    /// Walk through setting up this instance: domain, git source, backups, notifications and a
    /// first app. Picks up where the TUI or an earlier run left off.
    Setup {
        /// Only show the checklist.
        #[arg(long)]
        status: bool,
        /// Server the first app runs on, instead of asking.
        #[arg(long)]
        server: Option<String>,
        /// Directory of the first app inside its GitHub repository, for monorepos, e.g. apps/web.
        #[arg(long)]
        root: Option<String>,
    },
    /// Servers the daemon deploys to.
    #[command(subcommand)]
    Server(ServerCmd),
    /// Projects group environments and services.
    #[command(subcommand, alias = "projects")]
    Project(ProjectCmd),
    /// Environments inside a project, like production and staging.
    #[command(subcommand, alias = "environments")]
    Environment(EnvironmentCmd),
    /// List, create and remove services.
    #[command(subcommand, alias = "services", alias = "svc")]
    Service(ServiceCmd),
    /// Build and roll out a service.
    Deploy {
        service: String,
        /// Return right away instead of streaming the build.
        #[arg(long, short)]
        detach: bool,
    },
    /// Show or follow a service's container logs.
    Logs {
        service: String,
        #[arg(long, short)]
        follow: bool,
        #[arg(long, short = 'n', default_value_t = 200)]
        tail: usize,
        /// For compose builds: the compose service to show.
        #[arg(long)]
        container: Option<String>,
    },
    /// Restart a service's container.
    Restart { service: String },
    /// Stop a service until it is started again.
    Stop { service: String },
    /// Start a stopped service.
    Start { service: String },
    /// Recent deployments of a service.
    Deployments {
        service: String,
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    /// Redeploy an earlier image. Defaults to the deployment before the live one.
    Rollback {
        service: String,
        /// Deployment number to roll back to.
        number: Option<i64>,
    },
    /// Domains routed to a service by the proxy.
    #[command(subcommand, alias = "domains")]
    Domain(DomainCmd),
    /// Environment variables of a service.
    #[command(subcommand, alias = "env")]
    Vars(VarsCmd),
    /// Persistent volumes of a service.
    #[command(subcommand, alias = "volumes")]
    Volume(VolumeCmd),
    /// Connection details, public access, backups and passwords of database services.
    #[command(subcommand, alias = "database")]
    Db(DbCmd),
    /// Encrypted secrets: SSH keys for servers, S3 credentials and other values.
    #[command(subcommand, alias = "secret")]
    Secrets(SecretsCmd),
    /// S3-compatible storage connections for backups (AWS S3, R2, B2, MinIO, ...).
    #[command(subcommand)]
    Storage(StorageCmd),
    /// Encrypted backups of the daemon itself, and the domain its API is served on.
    #[command(subcommand)]
    Instance(InstanceCmd),
    /// Notifications: channels, rules with presets, and the delivery log.
    #[command(subcommand)]
    Notify(notify::NotifyCmd),
    /// Git sources services clone from: GitHub Apps and tokens.
    #[command(subcommand, alias = "sources")]
    Source(sources::SourceCmd),
}

#[derive(Subcommand)]
enum InstanceCmd {
    /// The domain the API is served on with automatic HTTPS; GitHub Apps need it.
    #[command(subcommand)]
    Domain(sources::DomainCmd),
    /// Back up the daemon (store, config, master key, SSH keys) and wait for the result.
    /// The passphrase comes from --passphrase-file or stdin and is never stored.
    Backup {
        /// Storage connection to write to; defaults to the daemon's data directory.
        #[arg(long)]
        storage: Option<String>,
        #[arg(long)]
        passphrase_file: Option<PathBuf>,
    },
    /// Instance backups, newest first.
    Backups,
    /// Save an instance backup's encrypted archive.
    Download {
        /// Backup id or file name; defaults to the newest finished one.
        backup: Option<String>,
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Delete an instance backup.
    #[command(alias = "remove")]
    Rm {
        backup: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Show or change daily instance backups, e.g. "instance schedule --hour 2 --keep 7
    /// --storage offsite --passphrase-file pass.txt". The passphrase is stored encrypted under
    /// the master key so scheduled runs can use it.
    Schedule {
        #[arg(long, conflicts_with = "off")]
        hour: Option<u8>,
        #[arg(long, conflicts_with = "off")]
        keep: Option<u32>,
        /// A storage connection, or "local".
        #[arg(long, conflicts_with = "off")]
        storage: Option<String>,
        #[arg(long, conflicts_with = "off")]
        passphrase_file: Option<PathBuf>,
        #[arg(long)]
        off: bool,
    },
}

#[derive(Subcommand)]
enum StorageCmd {
    List,
    /// Add a connection and test it. Credentials are an s3 secret (see "secrets add --kind s3").
    Add {
        name: String,
        /// S3 API base URL, e.g. https://s3.eu-west-1.amazonaws.com.
        #[arg(long)]
        endpoint: String,
        /// Signing region; "auto" for Cloudflare R2.
        #[arg(long, default_value = "us-east-1")]
        region: String,
        #[arg(long)]
        bucket: String,
        /// Key prefix inside the bucket.
        #[arg(long, default_value = "")]
        prefix: String,
        /// Address the bucket as <endpoint>/<bucket> (MinIO and most self-hosted stores).
        #[arg(long)]
        path_style: bool,
        /// The s3 credentials secret.
        #[arg(long)]
        credentials: String,
    },
    /// Put, read back and delete a probe object.
    Test {
        storage: String,
    },
    #[command(alias = "remove")]
    Rm {
        storage: String,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum SecretsCmd {
    List,
    /// Import a secret. The value is read from --file, or from stdin, never from arguments.
    Add {
        name: String,
        /// ssh-key, s3 or generic.
        #[arg(long)]
        kind: SecretKind,
        /// File holding the value: the private key, the S3 secret access key, or the value.
        #[arg(long, visible_alias = "from-file")]
        file: Option<PathBuf>,
        /// Read the value from stdin (the default without --file).
        #[arg(long, conflicts_with = "file")]
        stdin: bool,
        /// Access key id of s3 credentials (the secret access key comes from --file or stdin).
        #[arg(long)]
        access_key_id: Option<String>,
    },
    /// Generate a new ed25519 key in the daemon and print its public key.
    GenerateSshKey {
        name: String,
    },
    /// Kind, public part and users of a secret; never its value.
    Show {
        secret: String,
    },
    /// Print the decrypted value. Every reveal is recorded in the activity feed.
    Reveal {
        secret: String,
    },
    Rename {
        secret: String,
        new_name: String,
    },
    /// Replace the value, read from --file or stdin like `add`.
    Replace {
        secret: String,
        #[arg(long, visible_alias = "from-file")]
        file: Option<PathBuf>,
        #[arg(long, conflicts_with = "file")]
        stdin: bool,
        #[arg(long)]
        access_key_id: Option<String>,
    },
    /// Delete a secret that nothing uses any more.
    #[command(alias = "remove")]
    Rm {
        secret: String,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ContextCmd {
    List,
    Use { name: String },
    Remove { name: String },
}

#[derive(Subcommand)]
enum TokenCmd {
    /// Issue a new API token; the old one stops working at once. The new token is saved to the
    /// current context and printed once.
    Rotate,
}

#[derive(Subcommand)]
enum ProxyCmd {
    /// Reinstall the proxy on the daemon's own server.
    Restart,
}

#[derive(Subcommand)]
enum ServerCmd {
    List,
    /// Add a server reached over SSH with the daemon's key (see "server key") or --key.
    Add {
        name: String,
        host: String,
        #[arg(long, default_value = "root")]
        user: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
        /// An ssh-key secret to connect with instead of the daemon's key.
        #[arg(long)]
        key: Option<String>,
    },
    /// Connect with an ssh-key secret, or with the daemon's own key when none is given.
    SetKey {
        name: String,
        secret: Option<String>,
    },
    Check {
        name: String,
    },
    /// Install or restart the reverse proxy on a server.
    Proxy {
        name: String,
    },
    /// Install Docker on a server that lacks it (get.docker.com, as root or with passwordless sudo).
    InstallDocker {
        name: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Stop managing a server. With --purge, first remove everything the daemon put there.
    Remove {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        name: Option<String>,
        /// Every SSH server.
        #[arg(long)]
        all: bool,
        /// Delete the services assigned to it and remove its containers, proxy and networks.
        #[arg(long)]
        purge: bool,
        /// With --purge, also delete the Docker volumes there, and the data in them.
        #[arg(long, requires = "purge")]
        volumes: bool,
        #[arg(long, short)]
        yes: bool,
    },
    /// Print the public key servers must trust.
    Key,
}

#[derive(Subcommand)]
enum ProjectCmd {
    List,
    Create {
        name: String,
        #[arg(long, short)]
        description: Option<String>,
    },
    Delete {
        name: String,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum EnvironmentCmd {
    List {
        project: String,
    },
    /// Add an environment to a project, e.g. "environment create acme staging".
    Create {
        project: String,
        name: String,
    },
    /// Delete "project/environment" and its services.
    Delete {
        environment: String,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// All services, or those of a project or "project/environment".
    List {
        scope: Option<String>,
    },
    /// Details of one service.
    Show {
        service: String,
    },
    Create(Box<CreateArgs>),
    /// CPU, memory, network, disk and requests over a range, with sparklines.
    Metrics {
        service: String,
        /// 1h, 24h, 7d or 30d.
        #[arg(long, default_value = "1h")]
        range: MetricsRange,
        /// One compose service instead of the whole group.
        #[arg(long)]
        container: Option<String>,
    },
    /// Show or change how a git service is built; applies on the next deploy.
    Build {
        service: String,
        /// auto, dockerfile, compose or railpack.
        #[arg(long)]
        method: Option<BuildMethod>,
        /// Dockerfile stage to build; "" clears it.
        #[arg(long)]
        target: Option<String>,
        /// Compose file in the root directory; "" goes back to the default names.
        #[arg(long)]
        compose_file: Option<String>,
        #[arg(long)]
        dockerfile: Option<String>,
        /// Build context directory inside the root directory.
        #[arg(long = "build-context")]
        build_context: Option<String>,
        /// Directory the service lives in, for monorepos; "" is the repository root.
        #[arg(long)]
        root: Option<String>,
        /// With a root, only deploy on push when a changed file is inside it: true or false.
        #[arg(long)]
        watch_paths: Option<bool>,
    },
    /// Run a service on another server. The old server keeps serving until the new one is
    /// healthy; a failed move leaves everything as it was.
    Move {
        service: String,
        /// Server to move to.
        #[arg(long)]
        server: String,
        /// Copy the named volumes too. The old containers stop while they copy. Databases
        /// always copy their data.
        #[arg(long)]
        copy_volumes: bool,
        /// Return right away instead of streaming the move.
        #[arg(long, short)]
        detach: bool,
    },
    Delete {
        service: String,
        /// Also delete the service's volumes and their data.
        #[arg(long)]
        volumes: bool,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Args)]
struct CreateArgs {
    /// "project" or "project/environment".
    environment: String,
    /// Service name; derived from the source when left out.
    name: Option<String>,
    /// Prebuilt image, e.g. ghcr.io/acme/web:latest.
    #[arg(long, group = "origin")]
    image: Option<String>,
    /// Git repository to build with its Dockerfile: a clone URL, or owner/name with --source.
    #[arg(long, group = "origin")]
    repo: Option<String>,
    /// Git source to clone --repo owner/name with (see "source list").
    #[arg(long, requires = "repo")]
    source: Option<String>,
    /// Don't deploy when the branch is pushed (GitHub App sources deploy on push by default).
    #[arg(long, requires = "source")]
    no_auto_deploy: bool,
    /// How to build: auto (Dockerfile, else compose file, else Railpack), dockerfile, compose
    /// or railpack.
    #[arg(long, default_value = "auto", requires = "repo")]
    build: BuildMethod,
    /// Dockerfile stage to build.
    #[arg(long, requires = "repo")]
    target: Option<String>,
    /// Compose file in the root directory, for --build compose.
    #[arg(long, requires = "repo")]
    compose_file: Option<String>,
    /// Directory the service lives in, for monorepos, e.g. apps/api. The Dockerfile, build
    /// context and compose file are relative to it.
    #[arg(long, requires = "repo")]
    root: Option<String>,
    /// Deploy on every push, not only when a file inside --root changed.
    #[arg(long, requires = "root")]
    no_watch_paths: bool,
    #[arg(long, default_value = "main", requires = "repo")]
    branch: String,
    #[arg(long, default_value = "Dockerfile", requires = "repo")]
    dockerfile: String,
    /// Build context directory inside the root directory.
    #[arg(long = "build-context", default_value = ".", requires = "repo")]
    build_context: String,
    /// One-click database: postgres, mysql, mariadb, redis or mongo, optionally with :version.
    #[arg(long, group = "origin")]
    database: Option<String>,
    /// Port the app listens on inside the container.
    #[arg(long, short)]
    port: Option<u16>,
    /// Domain to route here with automatic HTTPS. Repeatable.
    #[arg(long = "domain")]
    domains: Vec<String>,
    /// KEY=VALUE environment variable. Repeatable.
    #[arg(long = "env", short = 'e')]
    env: Vec<String>,
    /// name:/mount/path volume. Repeatable.
    #[arg(long = "volume", short = 'v')]
    volumes: Vec<String>,
    /// HTTP path probed before traffic switches, e.g. /healthz.
    #[arg(long)]
    health: Option<String>,
    /// Command run with "sh -c" instead of the image default.
    #[arg(long)]
    command: Option<String>,
    /// Server name; defaults to the daemon's own machine.
    #[arg(long)]
    server: Option<String>,
    /// Create without deploying.
    #[arg(long)]
    no_deploy: bool,
}

#[derive(Subcommand)]
enum DomainCmd {
    List {
        service: String,
    },
    Add {
        service: String,
        host: String,
        /// Container port, if not the service port.
        #[arg(long)]
        port: Option<u16>,
        /// Serve plain HTTP without a certificate.
        #[arg(long)]
        http: bool,
        /// For compose builds: the compose service to route to.
        #[arg(long)]
        compose_service: Option<String>,
    },
    Remove {
        service: String,
        host: String,
    },
}

#[derive(Subcommand)]
enum VarsCmd {
    List {
        service: String,
        /// Show secret values.
        #[arg(long)]
        reveal: bool,
    },
    /// Set variables, e.g. "vars set web KEY=value OTHER=value".
    Set {
        service: String,
        #[arg(required = true)]
        pairs: Vec<String>,
        /// Mark the values secret.
        #[arg(long)]
        secret: bool,
        /// Redeploy so the change takes effect.
        #[arg(long)]
        deploy: bool,
    },
    Unset {
        service: String,
        #[arg(required = true)]
        keys: Vec<String>,
        #[arg(long)]
        deploy: bool,
    },
}

#[derive(Subcommand)]
enum VolumeCmd {
    List {
        service: String,
    },
    Add {
        service: String,
        name: String,
        path: String,
    },
    Remove {
        service: String,
        name: String,
        /// Delete the data too.
        #[arg(long)]
        purge: bool,
    },
}

#[derive(Subcommand)]
enum DbCmd {
    /// Connection details. The password is masked unless --reveal is given.
    Info {
        service: String,
        /// Show the password.
        #[arg(long)]
        reveal: bool,
    },
    /// Publish the database on a port of its server, reachable from outside.
    Expose { service: String, port: u16 },
    /// Make the database reachable only from its environment again.
    Unexpose { service: String },
    /// Back up now and wait for the result.
    Backup {
        service: String,
        /// Storage connection to write to (see "storage list"); defaults to the data directory.
        #[arg(long)]
        storage: Option<String>,
    },
    /// Backups of a database, newest first.
    Backups { service: String },
    /// Replace the database's data with a backup.
    Restore {
        service: String,
        /// Backup id or file name.
        backup: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Save a backup's gzip file.
    Download {
        service: String,
        /// Backup id or file name.
        backup: String,
        /// Where to write it; defaults to the backup's file name.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Show or change daily automatic backups, e.g. "db schedule main --hour 3 --keep 7".
    Schedule {
        service: String,
        /// Hour of the day in UTC (0-23); enables the schedule.
        #[arg(long, conflicts_with = "off")]
        hour: Option<u8>,
        /// Scheduled backups to keep; enables the schedule.
        #[arg(long, conflicts_with = "off")]
        keep: Option<u32>,
        /// Turn scheduled backups off.
        #[arg(long)]
        off: bool,
        /// Where scheduled backups go: a storage connection, or "local" for the data directory.
        #[arg(long, conflicts_with = "off")]
        storage: Option<String>,
    },
    /// Set a new password inside the database and in its stored connection URL.
    RotatePassword {
        service: String,
        #[arg(long, short)]
        yes: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("{} {e:#}", paint("error:", Tone::Err));
        std::process::exit(1);
    }
}

/// A command line the user can copy, like "<cli> deploy web".
fn hint(args: &str) -> String {
    paint(format!("{} {args}", brand::CLI_BIN), Tone::Accent)
}

async fn run(cli: Cli) -> Result<()> {
    let mut config = Config::load()?;
    let json = cli.json;
    match cli.command {
        Some(Cmd::Login { url, token, name }) => return login(&mut config, url, token, name).await,
        Some(Cmd::Context(cmd)) => return context(&mut config, cmd),
        Some(Cmd::Logout { context, all }) => return instance::logout(&mut config, context, all),
        Some(Cmd::Upgrade { tag, force }) => {
            let steps = instance::Steps::CLI;
            return instance::update(&config, cli.context.as_deref(), steps, tag, force, json)
                .await;
        }
        Some(Cmd::Update {
            tag,
            force,
            cli: only_cli,
            server,
        }) => {
            let steps = instance::Steps::from_flags(only_cli, server);
            return instance::update(&config, cli.context.as_deref(), steps, tag, force, json)
                .await;
        }
        _ => {}
    }
    let (name, ctx) = config.resolve(cli.context.as_deref())?;
    let c = Client::from_context(&ctx)?;
    let command = cli.command.unwrap_or(Cmd::Tui);
    let daemon = shows_skew_hint(&command, json).then(|| {
        let c = c.clone();
        tokio::spawn(async move { c.info().await })
    });
    let result = match command {
        Cmd::Instance(InstanceCmd::Domain(cmd)) => {
            sources::domain(&mut config, &name, &c, cmd, json).await
        }
        command => dispatch(c, name, command, json).await,
    };
    if let Some(daemon) = daemon {
        skew_hint(daemon).await;
    }
    result
}

/// Human commands end with a hint when this CLI and the daemon run different versions. JSON
/// output stays clean, and the TUI shows the hint in its own top bar.
fn shows_skew_hint(command: &Cmd, json: bool) -> bool {
    !json && !matches!(command, Cmd::Tui)
}

/// Prints the version-skew hint on stderr, so piped output stays clean. The daemon's info was
/// requested alongside the command, so this rarely waits.
async fn skew_hint(daemon: tokio::task::JoinHandle<Result<DaemonInfo>>) {
    let Ok(Ok(Ok(info))) = tokio::time::timeout(std::time::Duration::from_secs(1), daemon).await
    else {
        return;
    };
    if let Some(hint) = version_skew(env!("CARGO_PKG_VERSION"), &info.version) {
        eprintln!("{}", muted(hint));
    }
}

async fn dispatch(c: Client, name: String, command: Cmd, json: bool) -> Result<()> {
    match command {
        Cmd::Tui => tui::run(c, name).await,
        Cmd::Login { .. }
        | Cmd::Context(_)
        | Cmd::Logout { .. }
        | Cmd::Upgrade { .. }
        | Cmd::Update { .. } => unreachable!("handled above"),
        Cmd::Settings { cmd } => instance::settings(&c, cmd, json).await,
        Cmd::Token(TokenCmd::Rotate) => instance::rotate_token(&c, &name, json).await,
        Cmd::Proxy(ProxyCmd::Restart) => instance::restart_proxy(&c, json).await,
        Cmd::Prune => instance::prune(&c, json).await,
        Cmd::Status => status(&c, &name, json).await,
        Cmd::Setup {
            status,
            server,
            root,
        } => {
            use std::io::IsTerminal;
            if status || json {
                let o = c.onboarding().await?;
                if json {
                    return output::json(&o);
                }
                setup::print_status(&o);
                return Ok(());
            }
            if !std::io::stdin().is_terminal() {
                bail!("setup asks questions; run it in a terminal, or use --status");
            }
            let mut stdin = std::io::stdin().lock();
            setup::run(
                &c,
                &mut setup::Prompt::new(&mut stdin, true),
                &setup::FirstApp { server, root },
            )
            .await?;
            Ok(())
        }
        Cmd::Server(cmd) => server(&c, cmd, json).await,
        Cmd::Project(cmd) => project(&c, cmd, json).await,
        Cmd::Environment(cmd) => environment(&c, cmd, json).await,
        Cmd::Service(cmd) => service(&c, cmd, json).await,
        Cmd::Deploy { service, detach } => {
            let (_, _, s) = find(&c, &service).await?;
            let d = c.deploy(&s.id, "cli").await?;
            if detach {
                output::ok(format!("deployment #{} of {} queued", d.number, s.name));
                return Ok(());
            }
            follow_deployment(&c, d).await
        }
        Cmd::Logs {
            service,
            follow,
            tail,
            container,
        } => {
            let (_, _, s) = find(&c, &service).await?;
            let mut stream = c
                .service_logs(&s.id, tail, follow, container.as_deref())
                .await?;
            while let Some(line) = stream.next().await {
                print_log(&line?, true);
            }
            Ok(())
        }
        Cmd::Restart { service } => {
            let (_, _, s) = find(&c, &service).await?;
            c.restart(&s.id).await?;
            output::ok(format!("{} restarted", s.name));
            Ok(())
        }
        Cmd::Stop { service } => {
            let (_, _, s) = find(&c, &service).await?;
            c.stop(&s.id).await?;
            output::ok(format!("{} stopped", s.name));
            Ok(())
        }
        Cmd::Start { service } => {
            let (_, _, s) = find(&c, &service).await?;
            c.start(&s.id).await?;
            output::ok(format!("{} started", s.name));
            Ok(())
        }
        Cmd::Deployments { service, limit } => {
            let (_, _, s) = find(&c, &service).await?;
            let list = c.deployments(&s.id, limit).await?;
            if json {
                return output::json(&list);
            }
            if list.is_empty() {
                output::note(format!("{} has never been deployed", s.name));
            }
            output::table(
                &["#", "STATUS", "COMMIT", "MESSAGE", "TRIGGER", "STARTED"],
                list.iter()
                    .map(|d| {
                        let message = d
                            .error
                            .clone()
                            .or_else(|| d.commit_message.clone())
                            .or_else(|| d.image.clone());
                        vec![
                            Cell::from(d.number.to_string()),
                            output::deployment(d.status),
                            Cell::from(d.commit.as_deref().map_or("—", |c| &c[..c.len().min(7)])),
                            Cell(message.unwrap_or_default(), Tone::Muted),
                            Cell(d.trigger.clone(), Tone::Muted),
                            Cell(output::ago(d.created_at), Tone::Muted),
                        ]
                    })
                    .collect(),
            );
            Ok(())
        }
        Cmd::Rollback { service, number } => {
            let (_, _, s) = find(&c, &service).await?;
            let list = c.deployments(&s.id, 50).await?;
            let target = match number {
                Some(n) => list
                    .iter()
                    .find(|d| d.number == n)
                    .with_context(|| format!("no deployment #{n}"))?,
                None => {
                    let start = list
                        .iter()
                        .position(|d| d.status == DeploymentStatus::Live)
                        .map_or(0, |i| i + 1);
                    list[start..]
                        .iter()
                        .find(|d| d.image.is_some() && d.status == DeploymentStatus::Superseded)
                        .context("no earlier deployment to roll back to")?
                }
            };
            let d = c.rollback(&target.id).await?;
            output::note(format!("rolling back {} to #{}", s.name, target.number));
            follow_deployment(&c, d).await
        }
        Cmd::Domain(cmd) => domain(&c, cmd, json).await,
        Cmd::Vars(cmd) => vars(&c, cmd, json).await,
        Cmd::Volume(cmd) => volume(&c, cmd, json).await,
        Cmd::Db(cmd) => db(&c, cmd, json).await,
        Cmd::Secrets(cmd) => secrets(&c, cmd, json).await,
        Cmd::Storage(cmd) => storage(&c, cmd, json).await,
        Cmd::Instance(cmd) => instance(&c, cmd, json).await,
        Cmd::Notify(cmd) => notify::run(&c, cmd, json).await,
        Cmd::Source(cmd) => sources::source(&c, cmd, json).await,
    }
}

/// One row per measure: now, average, peak and a sparkline of the range.
fn print_metrics(scope: &str, s: &Service, m: &ServiceMetrics) {
    const WIDTH: usize = 32;
    let which = m
        .container
        .as_deref()
        .map(|c| format!(" · {c}"))
        .unwrap_or_default();
    println!(
        "{}{which} {}",
        s.name,
        muted(format!(
            "{scope} · last {} · {} points of {}s",
            m.range,
            m.points.len(),
            m.step_secs
        ))
    );
    if !m.containers.is_empty() {
        println!(
            "{}",
            muted(format!(
                "containers {} · --container picks one",
                m.containers.join(" ")
            ))
        );
    }
    let Some(last) = m.points.last() else {
        output::note("collecting · first samples in ~10s");
        return;
    };
    let pct: fn(f64) -> String = |v| format!("{v:.0}%");
    let mem: fn(f64) -> String = |v| output::bytes(v as u64);
    let rate: fn(f64) -> String = |v| format!("{}/s", output::size(v as u64));
    let ms: fn(f64) -> String = |v| format!("{v:.0} ms");
    let rps: fn(f64) -> String = |v| format!("{v:.1}/s");
    type Pick = fn(&MetricPoint) -> Option<f64>;
    type Fmt = fn(f64) -> String;
    let mut rows: Vec<(&str, Pick, Fmt)> = vec![
        ("cpu", |p| Some(p.cpu), pct),
        ("memory", |p| Some(p.mem as f64), mem),
        ("net rx", |p| Some(p.net_rx), rate),
        ("net tx", |p| Some(p.net_tx), rate),
        ("disk read", |p| Some(p.disk_read), rate),
        ("disk write", |p| Some(p.disk_write), rate),
        ("pids", |p| Some(p.pids as f64), |v| format!("{v:.0}")),
    ];
    if m.http {
        rows.push(("requests", |p| p.http.as_ref().map(|h| h.rps), rps));
        rows.push(("p50", |p| p.http.as_ref().and_then(|h| h.p50_ms), ms));
        rows.push(("p95", |p| p.http.as_ref().and_then(|h| h.p95_ms), ms));
    }
    let table: Vec<Vec<output::Cell>> = rows
        .into_iter()
        .map(|(name, pick, fmt)| {
            let values: Vec<f64> = m.points.iter().filter_map(pick).collect();
            let now = pick(last).map_or("—".into(), fmt);
            let avg = if values.is_empty() {
                "—".into()
            } else {
                fmt(values.iter().sum::<f64>() / values.len() as f64)
            };
            let peak = values
                .iter()
                .copied()
                .reduce(f64::max)
                .map_or("—".into(), fmt);
            vec![
                output::Cell(name.into(), Tone::Muted),
                now.into(),
                avg.into(),
                peak.into(),
                output::Cell(output::sparkline(&values, WIDTH), Tone::Info),
            ]
        })
        .collect();
    output::table(
        &["", "NOW", "AVG", "PEAK", &format!("LAST {}", m.range)],
        table,
    );
    let mut facts = vec![
        format!("limit {}", output::bytes(last.mem_limit)),
        format!("restarts {}", last.restarts),
        format!("up {}", output::duration(last.uptime_secs)),
    ];
    let http: Vec<&HttpPoint> = m.points.iter().filter_map(|p| p.http.as_ref()).collect();
    let total: u64 = http.iter().map(|h| h.requests()).sum();
    if total > 0 {
        let share = |f: fn(&HttpPoint) -> u64| {
            http.iter().map(|h| f(h)).sum::<u64>() as f64 / total as f64 * 100.0
        };
        facts.push(format!(
            "2xx {:.0}% · 4xx {:.0}% · 5xx {:.0}%",
            share(|h| h.s2xx + h.s3xx),
            share(|h| h.s4xx),
            share(|h| h.s5xx)
        ));
    }
    if !m.deploys.is_empty() {
        let deploys: Vec<String> = m
            .deploys
            .iter()
            .map(|d| format!("#{} {}", d.number, output::ago(d.at)))
            .collect();
        facts.push(format!("deploys {}", deploys.join(", ")));
    }
    println!("{}", muted(facts.join(" · ")));
}

async fn find(c: &Client, query: &str) -> Result<(Project, Environment, Service)> {
    let tree = c.tree().await?;
    let (p, e, s) = resolve::service(&tree, query)?;
    Ok((p.project.clone(), e.environment.clone(), s.clone()))
}

async fn login(
    config: &mut Config,
    url: String,
    token: String,
    name: Option<String>,
) -> Result<()> {
    let url = if url.contains("://") {
        url
    } else {
        format!("http://{url}")
    };
    let info = Client::new(&url, &token)?
        .info()
        .await
        .context("could not log in")?;
    let url = match info.url.as_deref() {
        Some(domain_url) if domain_url != url.trim_end_matches('/') => {
            if Client::new(domain_url, &token)?.info().await.is_ok() {
                output::note(format!(
                    "using the instance domain {domain_url} instead of {url}"
                ));
                domain_url.to_string()
            } else {
                url
            }
        }
        _ => url,
    };
    let name = name.unwrap_or_else(|| info.hostname.clone());
    config.contexts.insert(
        name.clone(),
        Context {
            url: url.clone(),
            token,
        },
    );
    config.current = Some(name.clone());
    config.save()?;
    output::ok(format!(
        "connected to {name} ({} {} at {url})",
        brand::DAEMON_BIN,
        info.version
    ));
    let client = Client::new(&url, config.contexts[&name].token.as_str())?;
    let pending = client.onboarding().await.is_ok_and(|o| o.open_on_connect());
    if pending {
        println!(
            "{} run {} to set up this instance, or {} without the dashboard",
            muted("next:"),
            paint(brand::CLI_BIN, Tone::Accent),
            paint(format!("{} setup", brand::CLI_BIN), Tone::Accent)
        );
    } else {
        println!(
            "{} run {} to open the dashboard",
            muted("next:"),
            paint(brand::CLI_BIN, Tone::Accent)
        );
    }
    Ok(())
}

fn context(config: &mut Config, cmd: ContextCmd) -> Result<()> {
    match cmd {
        ContextCmd::List => {
            if config.contexts.is_empty() {
                println!(
                    "{} {}",
                    muted("no contexts yet:"),
                    hint("login <url> --token <token>")
                );
            }
            output::table(
                &["", "NAME", "URL"],
                config
                    .contexts
                    .iter()
                    .map(|(name, ctx)| {
                        let current = config.current.as_deref() == Some(name);
                        vec![
                            Cell(if current { "*" } else { "" }.into(), Tone::Accent),
                            Cell(
                                name.clone(),
                                if current { Tone::Accent } else { Tone::Text },
                            ),
                            Cell(ctx.url.clone(), Tone::Muted),
                        ]
                    })
                    .collect(),
            );
        }
        ContextCmd::Use { name } => {
            if !config.contexts.contains_key(&name) {
                bail!("unknown context '{name}'");
            }
            config.current = Some(name.clone());
            config.save()?;
            output::ok(format!("now using {name}"));
        }
        ContextCmd::Remove { name } => instance::logout(config, Some(name), false)?,
    }
    Ok(())
}

async fn status(c: &Client, name: &str, json: bool) -> Result<()> {
    let (info, servers, tree) = tokio::try_join!(c.info(), c.servers(), c.tree())?;
    if json {
        return output::json(
            &serde_json::json!({ "context": name, "daemon": info, "servers": servers }),
        );
    }
    let services: Vec<&Service> = tree
        .projects
        .iter()
        .flat_map(|p| &p.environments)
        .flat_map(|e| &e.services)
        .collect();
    let count = |s: ServiceState| services.iter().filter(|x| x.state == s).count();
    let failed = count(ServiceState::Failed);
    println!(
        "{} {name}  {}",
        paint(brand::NAME.to_lowercase(), Tone::Accent),
        muted(format!(
            "{} {} on {}",
            brand::DAEMON_BIN,
            info.version,
            info.hostname
        ))
    );
    println!(
        "{}cpu {:.0}%  mem {}/{}  disk {}%  proxy {}",
        muted("host  "),
        info.cpu_percent,
        output::bytes(info.mem_used),
        output::bytes(info.mem_total),
        (info.disk_used * 100)
            .checked_div(info.disk_total)
            .unwrap_or(0),
        match info.proxy {
            ProxyStatus::Running => paint("running", Tone::Ok),
            ProxyStatus::Stopped => paint("stopped", Tone::Err),
            ProxyStatus::Missing => paint("not set up", Tone::Accent),
        }
    );
    println!(
        "{}{} projects  {} services  {} running  {} failed",
        muted("apps  "),
        tree.projects.len(),
        services.len(),
        paint(count(ServiceState::Running).to_string(), Tone::Ok),
        paint(
            failed.to_string(),
            if failed > 0 { Tone::Err } else { Tone::Muted }
        ),
    );
    println!();
    print_servers(&servers);
    Ok(())
}

fn print_servers(servers: &[Server]) {
    output::table(
        &["", "SERVER", "KIND", "HOST", "DOCKER", "PROXY"],
        servers
            .iter()
            .map(|s| {
                let (g, t) = match s.status {
                    ServerStatus::Online => ("●", Tone::Ok),
                    ServerStatus::Offline => ("✕", Tone::Err),
                    ServerStatus::Unknown => ("○", Tone::Muted),
                };
                let host = s.host.clone().map_or("this machine".into(), |h| {
                    format!("{}@{h}", s.user.as_deref().unwrap_or("root"))
                });
                vec![
                    Cell(g.into(), t),
                    Cell::from(s.name.clone()),
                    Cell(
                        if s.kind == ServerKind::Local {
                            "local"
                        } else {
                            "ssh"
                        }
                        .into(),
                        Tone::Muted,
                    ),
                    Cell::from(host),
                    Cell(s.docker_version.clone().unwrap_or("—".into()), Tone::Muted),
                    match s.proxy {
                        ProxyStatus::Running => Cell("running".into(), Tone::Ok),
                        ProxyStatus::Stopped => Cell("stopped".into(), Tone::Err),
                        ProxyStatus::Missing => Cell("not set up".into(), Tone::Accent),
                    },
                ]
            })
            .collect(),
    );
}

async fn find_server(c: &Client, name: &str) -> Result<Server> {
    c.servers()
        .await?
        .into_iter()
        .find(|s| s.name == name || s.id == name)
        .with_context(|| format!("no server '{name}'"))
}

async fn server(c: &Client, cmd: ServerCmd, json: bool) -> Result<()> {
    match cmd {
        ServerCmd::List => {
            let servers = c.servers().await?;
            if json {
                return output::json(&servers);
            }
            print_servers(&servers);
        }
        ServerCmd::Add {
            name,
            host,
            user,
            port,
            key,
        } => {
            let key = match key {
                Some(key) => Some(find_secret(c, &key).await?),
                None => None,
            };
            let s = c
                .create_server(&CreateServer {
                    name,
                    host,
                    user,
                    port,
                    ssh_key_secret_id: key.as_ref().map(|k| k.id.clone()),
                })
                .await?;
            let s = c.check_server(&s.id).await?;
            report_server_check(c, s).await?;
        }
        ServerCmd::SetKey { name, secret } => {
            let server = find_server(c, &name).await?;
            let key = match secret {
                Some(secret) => Some(find_secret(c, &secret).await?),
                None => None,
            };
            let body = UpdateServer {
                ssh_key_secret_id: Some(key.as_ref().map(|k| k.id.clone()).unwrap_or_default()),
            };
            let s = c.update_server(&server.id, &body).await?;
            report_server_check(c, s).await?;
        }
        ServerCmd::Check { name } => {
            let s = c.check_server(&find_server(c, &name).await?.id).await?;
            print_servers(std::slice::from_ref(&s));
            if s.status != ServerStatus::Online {
                report_server_check(c, s).await?;
            }
        }
        ServerCmd::InstallDocker { name, yes } => {
            let s = find_server(c, &name).await?;
            confirm(
                &format!("Install Docker on {} with get.docker.com over SSH?", s.name),
                yes,
            )?;
            install_docker(c, &s).await?;
        }
        ServerCmd::Proxy { name } => {
            let s = c.setup_proxy(&find_server(c, &name).await?.id).await?;
            output::ok(format!("proxy running on {}", s.name));
        }
        ServerCmd::Remove {
            name,
            all,
            purge,
            volumes,
            yes,
        } => remove_servers(c, name, all, purge, volumes, yes).await?,
        ServerCmd::Key => match c.info().await?.ssh_public_key {
            Some(key) => println!("{key}"),
            None => bail!("the daemon has no SSH key"),
        },
    }
    Ok(())
}

/// Prints whether a newly added or re-keyed server answers, and what to do when it does not.
/// Prints the outcome of a server check: online, or the preflight checklist with the problem
/// and its fix. Offers to install Docker when the daemon can, and returns the server after it.
async fn report_server_check(c: &Client, s: Server) -> Result<Server> {
    use std::io::IsTerminal as _;
    if s.status == ServerStatus::Online {
        output::ok(format!(
            "{} is online (docker {})",
            s.name,
            s.docker_version.clone().unwrap_or_default()
        ));
        return Ok(s);
    }
    print_checks(&s);
    let Some(problem) = &s.problem else {
        output::note(format!("{} is not reachable.", s.name));
        return Ok(s);
    };
    println!("{} {}", paint("✕", Tone::Err), problem.summary);
    if let Some(fix) = &problem.fix {
        println!("  {} {fix}", muted("fix:"));
    }
    if !problem.can_install_docker {
        println!(
            "  {} {}",
            muted("then:"),
            hint(&format!("server check {}", s.name))
        );
        return Ok(s);
    }
    if !std::io::stdin().is_terminal() {
        println!(
            "  {} {}",
            muted("or let the daemon do it:"),
            hint(&format!("server install-docker {} --yes", s.name))
        );
        return Ok(s);
    }
    if confirm(&format!("Install Docker on {} now?", s.name), false).is_err() {
        return Ok(s);
    }
    install_docker(c, &s).await
}

fn print_checks(s: &Server) {
    for check in &s.checks {
        let glyph = match check.status {
            CheckStatus::Pass => paint("✓", Tone::Ok),
            CheckStatus::Fail => paint("✕", Tone::Err),
            CheckStatus::Skipped => muted("·"),
        };
        let label = match check.status {
            CheckStatus::Skipped => muted(&check.label),
            _ => check.label.clone(),
        };
        match &check.detail {
            Some(detail) => println!("  {glyph} {label}  {}", muted(detail)),
            None => println!("  {glyph} {label}"),
        }
    }
}

async fn install_docker(c: &Client, s: &Server) -> Result<Server> {
    output::note(format!(
        "installing Docker on {} with get.docker.com, this takes a minute or two…",
        s.name
    ));
    let s = c.install_docker(&s.id).await?;
    if s.status == ServerStatus::Online {
        output::ok(format!(
            "Docker {} installed, {} is online",
            s.docker_version.clone().unwrap_or_default(),
            s.name
        ));
        return Ok(s);
    }
    Box::pin(report_server_check(c, s)).await
}

async fn find_secret(c: &Client, query: &str) -> Result<Secret> {
    c.secrets()
        .await?
        .into_iter()
        .find(|s| s.name == query || s.id == query)
        .with_context(|| format!("no secret '{query}'"))
}

/// Reads a secret value from `file`, or from stdin when no file is given.
fn read_value(file: Option<&PathBuf>) -> Result<String> {
    use std::io::{IsTerminal as _, Read as _};
    if let Some(path) = file {
        let path = match (path.strip_prefix("~"), std::env::var_os("HOME")) {
            (Ok(rest), Some(home)) => PathBuf::from(home).join(rest),
            _ => path.clone(),
        };
        return std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()));
    }
    if std::io::stdin().is_terminal() {
        eprintln!("{}", muted("Paste the value, then press Ctrl-D:"));
    }
    let mut value = String::new();
    std::io::stdin()
        .read_to_string(&mut value)
        .context("cannot read the value from stdin")?;
    Ok(value)
}

fn secret_value(
    kind: SecretKind,
    file: Option<&PathBuf>,
    access_key_id: Option<String>,
) -> Result<SecretValue> {
    if kind != SecretKind::S3Credentials && access_key_id.is_some() {
        bail!("--access-key-id only applies to s3 credentials");
    }
    let raw = read_value(file)?;
    Ok(match kind {
        SecretKind::SshKey => {
            common::ssh::inspect_private_key(&raw).map_err(anyhow::Error::msg)?;
            SecretValue::SshKey {
                private_key: common::ssh::normalize_private_key(&raw),
            }
        }
        SecretKind::S3Credentials => SecretValue::S3Credentials {
            access_key_id: access_key_id.context("s3 credentials need --access-key-id")?,
            secret_access_key: raw.trim().to_string(),
        },
        SecretKind::Generic => SecretValue::Generic {
            value: raw.strip_suffix('\n').unwrap_or(&raw).to_string(),
        },
        SecretKind::GithubApp => bail!(
            "GitHub App credentials are added through {}",
            hint("source github connect")
        ),
    })
}

fn used_by(secret: &Secret) -> String {
    if secret.used_by.is_empty() {
        return "—".into();
    }
    secret
        .used_by
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_secret(s: &Secret) {
    let row = |label: &str, value: String| println!("{:<10} {value}", muted(label));
    row("name", s.name.clone());
    row("kind", s.kind.label().into());
    match s.kind {
        SecretKind::SshKey => row("public key", s.public_part.clone().unwrap_or_default()),
        SecretKind::S3Credentials => row("access key", s.public_part.clone().unwrap_or_default()),
        SecretKind::Generic => {}
        SecretKind::GithubApp => row("app", s.public_part.clone().unwrap_or_default()),
    }
    row("used by", used_by(s));
    row("updated", output::ago(s.updated_at));
}

async fn secrets(c: &Client, cmd: SecretsCmd, json: bool) -> Result<()> {
    match cmd {
        SecretsCmd::List => {
            let list = c.secrets().await?;
            if json {
                return output::json(&list);
            }
            if list.is_empty() {
                output::note(format!(
                    "no secrets yet; add one with {} or {}",
                    hint("secrets add <name> --kind ssh-key --file <key>"),
                    hint("secrets generate-ssh-key <name>")
                ));
            }
            output::table(
                &["SECRET", "KIND", "PUBLIC", "USED BY", "UPDATED"],
                list.iter()
                    .map(|s| {
                        let public = s.public_part.as_deref().map_or("—".into(), |p| {
                            let short: String = p.chars().take(32).collect();
                            if short.len() < p.len() {
                                format!("{short}…")
                            } else {
                                short
                            }
                        });
                        vec![
                            Cell::from(s.name.clone()),
                            Cell(s.kind.label().into(), Tone::Muted),
                            Cell(public, Tone::Muted),
                            Cell::from(used_by(s)),
                            Cell(output::ago(s.updated_at), Tone::Muted),
                        ]
                    })
                    .collect(),
            );
        }
        SecretsCmd::Add {
            name,
            kind,
            file,
            access_key_id,
            ..
        } => {
            let value = secret_value(kind, file.as_ref(), access_key_id)?;
            let s = c.create_secret(&CreateSecret { name, value }).await?;
            if json {
                return output::json(&s);
            }
            output::ok(format!("added {} ({})", s.name, s.kind.label()));
            if let (SecretKind::SshKey, Some(public)) = (s.kind, &s.public_part) {
                println!("{public}");
            }
        }
        SecretsCmd::GenerateSshKey { name } => {
            let s = c.generate_ssh_key(&name).await?;
            if json {
                return output::json(&s);
            }
            output::ok(format!(
                "generated {}; add this public key to ~/.ssh/authorized_keys on your servers:",
                s.name
            ));
            println!("{}", s.public_part.unwrap_or_default());
        }
        SecretsCmd::Show { secret } => {
            let s = find_secret(c, &secret).await?;
            if json {
                return output::json(&s);
            }
            print_secret(&s);
        }
        SecretsCmd::Reveal { secret } => {
            let s = find_secret(c, &secret).await?;
            let revealed = c.reveal_secret(&s.id).await?;
            if json {
                return output::json(&revealed);
            }
            match revealed.value {
                SecretValue::SshKey { private_key } => print!("{private_key}"),
                SecretValue::S3Credentials {
                    access_key_id,
                    secret_access_key,
                } => {
                    println!("{:<18} {access_key_id}", muted("access key id"));
                    println!("{:<18} {secret_access_key}", muted("secret access key"));
                }
                SecretValue::Generic { value } => println!("{value}"),
                SecretValue::GithubApp { .. } => bail!("GitHub App credentials are never revealed"),
            }
        }
        SecretsCmd::Rename { secret, new_name } => {
            let s = find_secret(c, &secret).await?;
            let body = UpdateSecret {
                name: Some(new_name),
                value: None,
            };
            let s = c.update_secret(&s.id, &body).await?;
            output::ok(format!("renamed to {}", s.name));
        }
        SecretsCmd::Replace {
            secret,
            file,
            access_key_id,
            ..
        } => {
            let s = find_secret(c, &secret).await?;
            let body = UpdateSecret {
                name: None,
                value: Some(secret_value(s.kind, file.as_ref(), access_key_id)?),
            };
            let s = c.update_secret(&s.id, &body).await?;
            output::ok(format!("replaced the value of {}", s.name));
        }
        SecretsCmd::Rm { secret, yes } => {
            let s = find_secret(c, &secret).await?;
            confirm(&format!("Delete secret {}?", s.name), yes)?;
            c.delete_secret(&s.id).await?;
            output::ok(format!("deleted {}", s.name));
        }
    }
    Ok(())
}

async fn remove_servers(
    c: &Client,
    name: Option<String>,
    all: bool,
    purge: bool,
    volumes: bool,
    yes: bool,
) -> Result<()> {
    let servers = match name {
        Some(name) if !all => vec![find_server(c, &name).await?],
        _ => c
            .servers()
            .await?
            .into_iter()
            .filter(|s| s.kind == ServerKind::Ssh)
            .collect(),
    };
    if servers.is_empty() {
        output::note("no remote servers to remove");
        return Ok(());
    }
    if !purge {
        for s in &servers {
            c.delete_server(&s.id).await?;
            output::ok(format!("removed {}", s.name));
        }
        return Ok(());
    }
    let names: Vec<&str> = servers.iter().map(|s| s.name.as_str()).collect();
    confirm(
        &format!(
            "Delete the services on {} and remove everything {} put there{}?",
            names.join(", "),
            brand::DAEMON_BIN,
            if volumes {
                ", volumes and their data included"
            } else {
                ""
            }
        ),
        yes,
    )?;
    let mut failed = 0;
    for s in &servers {
        match c.purge_server(&s.id, volumes).await {
            Ok(r) => output::ok(format!(
                "{}: removed {} containers, {} networks, {} volumes and {} services",
                s.name, r.containers, r.networks, r.volumes, r.services
            )),
            Err(e) => {
                failed += 1;
                eprintln!("{} {}: {e:#}", paint("!", Tone::Accent), s.name);
                eprintln!("  {}", muted("clean it up by hand with:"));
                eprintln!("  {}", manual_cleanup(s, volumes));
            }
        }
    }
    if failed > 0 {
        bail!(
            "{failed} of {} servers could not be cleaned up",
            servers.len()
        );
    }
    Ok(())
}

/// What `--purge` does, as one command to run against an unreachable server later.
fn manual_cleanup(s: &Server, volumes: bool) -> String {
    let managed = format!("label={}=true", brand::label("managed"));
    let mut script = format!(
        "docker rm -f $(docker ps -aq --filter {managed}) {proxy} 2>/dev/null; docker network prune -f --filter {managed}",
        proxy = brand::PROXY_CONTAINER
    );
    if volumes {
        script.push_str(&format!(
            "; docker volume rm $(docker volume ls -q --filter {managed}) {}-caddy-data",
            brand::PREFIX
        ));
    }
    format!(
        "ssh -p {} {}@{} '{script}'",
        s.port.unwrap_or(22),
        s.user.as_deref().unwrap_or("root"),
        s.host.as_deref().unwrap_or("<host>")
    )
}

async fn find_storage(c: &Client, query: &str) -> Result<StorageConnection> {
    c.storages()
        .await?
        .into_iter()
        .find(|s| s.name == query || s.id == query)
        .with_context(|| format!("no storage connection '{query}'"))
}

/// `local` or the storage connection's name.
fn storage_label(storages: &[StorageConnection], storage_id: Option<&str>) -> String {
    match storage_id {
        None => "local".into(),
        Some(id) => storages
            .iter()
            .find(|s| s.id == id)
            .map_or_else(|| id.to_string(), |s| format!("s3:{}", s.name)),
    }
}

fn print_storage_test(name: &str, t: &StorageTest) {
    if t.ok {
        output::ok(format!(
            "{name} works: put, get and delete took {} ms",
            t.latency_ms.unwrap_or_default()
        ));
    } else {
        eprintln!(
            "{} {name}: {}",
            paint("✕", Tone::Err),
            t.error.as_deref().unwrap_or("test failed")
        );
    }
}

async fn storage(c: &Client, cmd: StorageCmd, json: bool) -> Result<()> {
    match cmd {
        StorageCmd::List => {
            let list = c.storages().await?;
            if json {
                return output::json(&list);
            }
            if list.is_empty() {
                output::note(format!(
                    "no storage connections yet; add one with {}",
                    hint("storage add <name> --endpoint <url> --bucket <b> --credentials <secret>")
                ));
            }
            output::table(
                &["STORAGE", "ENDPOINT", "BUCKET", "PREFIX", "LAST TEST"],
                list.iter()
                    .map(|s| {
                        let test = match &s.last_test {
                            None => Cell("never tested".into(), Tone::Muted),
                            Some(t) if t.ok => Cell(
                                format!(
                                    "ok, {} ms, {}",
                                    t.latency_ms.unwrap_or_default(),
                                    output::ago(t.at)
                                ),
                                Tone::Ok,
                            ),
                            Some(t) => Cell(
                                format!(
                                    "failed {}: {}",
                                    output::ago(t.at),
                                    t.error.as_deref().unwrap_or_default()
                                ),
                                Tone::Err,
                            ),
                        };
                        vec![
                            Cell::from(s.name.clone()),
                            Cell(s.endpoint.clone(), Tone::Muted),
                            Cell::from(s.bucket.clone()),
                            Cell(
                                if s.path_prefix.is_empty() {
                                    "—".into()
                                } else {
                                    s.path_prefix.clone()
                                },
                                Tone::Muted,
                            ),
                            test,
                        ]
                    })
                    .collect(),
            );
        }
        StorageCmd::Add {
            name,
            endpoint,
            region,
            bucket,
            prefix,
            path_style,
            credentials,
        } => {
            let secret = find_secret(c, &credentials).await?;
            let s = c
                .create_storage(&CreateStorage {
                    name,
                    endpoint,
                    region,
                    bucket,
                    path_prefix: prefix,
                    path_style,
                    credentials_secret_id: secret.id,
                })
                .await?;
            let t = c.test_storage(&s.id).await?;
            if json {
                return output::json(&c.storage(&s.id).await?);
            }
            output::ok(format!("added {}", s.name));
            print_storage_test(&s.name, &t);
        }
        StorageCmd::Test { storage } => {
            let s = find_storage(c, &storage).await?;
            let t = c.test_storage(&s.id).await?;
            if json {
                return output::json(&t);
            }
            print_storage_test(&s.name, &t);
            if !t.ok {
                std::process::exit(1);
            }
        }
        StorageCmd::Rm { storage, yes } => {
            let s = find_storage(c, &storage).await?;
            confirm(&format!("Remove storage connection {}?", s.name), yes)?;
            c.delete_storage(&s.id).await?;
            output::ok(format!("removed {}", s.name));
        }
    }
    Ok(())
}

/// The passphrase from a file, or from stdin when it is piped in. Typing it on a terminal would
/// echo it, so that is refused.
fn read_passphrase(file: Option<&PathBuf>) -> Result<String> {
    use std::io::IsTerminal as _;
    if file.is_none() && std::io::stdin().is_terminal() {
        bail!("pass --passphrase-file, or pipe the passphrase in on stdin");
    }
    let raw = read_value(file)?;
    let passphrase = raw.trim_end_matches(['\r', '\n']).to_string();
    if passphrase.chars().count() < MIN_PASSPHRASE_LEN {
        bail!("the passphrase must be at least {MIN_PASSPHRASE_LEN} characters");
    }
    Ok(passphrase)
}

fn print_instance_backups(list: &[InstanceBackup], storages: &[StorageConnection]) {
    output::table(
        &[
            "ID", "STATUS", "FILE", "SIZE", "WHERE", "TRIGGER", "CREATED",
        ],
        list.iter()
            .map(|b| {
                vec![
                    Cell::from(b.id.clone()),
                    backup_status(b.status),
                    Cell::from(b.file_name.clone()),
                    Cell(output::size(b.size_bytes), Tone::Muted),
                    Cell(
                        storage_label(storages, b.storage_id.as_deref()),
                        Tone::Muted,
                    ),
                    Cell(b.trigger.as_str().to_string(), Tone::Muted),
                    Cell(
                        b.error.clone().unwrap_or_else(|| output::ago(b.created_at)),
                        if b.error.is_some() {
                            Tone::Err
                        } else {
                            Tone::Muted
                        },
                    ),
                ]
            })
            .collect(),
    );
}

async fn instance(c: &Client, cmd: InstanceCmd, json: bool) -> Result<()> {
    match cmd {
        InstanceCmd::Domain(_) => unreachable!("handled in run()"),
        InstanceCmd::Backup {
            storage,
            passphrase_file,
        } => {
            let passphrase = read_passphrase(passphrase_file.as_ref())?;
            let storage_id = match storage {
                Some(name) => Some(find_storage(c, &name).await?.id),
                None => None,
            };
            let started = c
                .create_instance_backup(&CreateInstanceBackup {
                    storage_id,
                    passphrase,
                })
                .await?;
            output::note(format!("backing up the instance to {}", started.file_name));
            let b = loop {
                let b = c
                    .instance_backups()
                    .await?
                    .into_iter()
                    .find(|b| b.id == started.id)
                    .context("the instance backup disappeared")?;
                if b.status != BackupStatus::Running {
                    break b;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            };
            if json {
                return output::json(&b);
            }
            match b.status {
                BackupStatus::Done => output::ok(format!(
                    "instance backup {} done ({}, {})",
                    b.id,
                    b.file_name,
                    output::size(b.size_bytes)
                )),
                _ => bail!("instance backup failed: {}", b.error.unwrap_or_default()),
            }
        }
        InstanceCmd::Backups => {
            let list = c.instance_backups().await?;
            if json {
                return output::json(&list);
            }
            if list.is_empty() {
                println!(
                    "{} {}",
                    muted("no instance backups yet:"),
                    hint("instance backup --passphrase-file <file>")
                );
            }
            print_instance_backups(&list, &c.storages().await?);
        }
        InstanceCmd::Download { backup, output } => {
            use tokio::io::AsyncWriteExt as _;
            let list = c.instance_backups().await?;
            let b = match &backup {
                Some(q) => list.iter().find(|b| b.id == *q || b.file_name == *q),
                None => list.iter().find(|b| b.status == BackupStatus::Done),
            }
            .with_context(|| match &backup {
                Some(q) => format!("no instance backup '{q}'"),
                None => "no finished instance backup yet".into(),
            })?;
            let path = output.unwrap_or_else(|| PathBuf::from(&b.file_name));
            let mut stream = c.download_instance_backup(&b.id).await?;
            let mut file = tokio::fs::File::create(&path)
                .await
                .with_context(|| format!("cannot create {}", path.display()))?;
            while let Some(chunk) = stream.next().await {
                file.write_all(&chunk?).await?;
            }
            file.flush().await?;
            output::ok(format!(
                "saved {} ({}); restore it on a new machine with {} restore {} --passphrase-file <file>",
                path.display(),
                output::size(b.size_bytes),
                brand::DAEMON_BIN,
                path.display()
            ));
        }
        InstanceCmd::Rm { backup, yes } => {
            let b = c
                .instance_backups()
                .await?
                .into_iter()
                .find(|b| b.id == backup || b.file_name == backup)
                .with_context(|| format!("no instance backup '{backup}'"))?;
            confirm(&format!("Delete instance backup {}?", b.file_name), yes)?;
            c.delete_instance_backup(&b.id).await?;
            output::ok(format!("deleted {}", b.file_name));
        }
        InstanceCmd::Schedule {
            hour,
            keep,
            storage,
            passphrase_file,
            off,
        } => {
            let current = c.instance_backup_schedule().await?;
            let changes =
                hour.is_some() || keep.is_some() || storage.is_some() || passphrase_file.is_some();
            let schedule = if off || changes {
                let storage_id = match storage.as_deref() {
                    None => current.storage_id.clone(),
                    Some("local") => None,
                    Some(name) => Some(find_storage(c, name).await?.id),
                };
                let passphrase = match &passphrase_file {
                    Some(file) => Some(read_passphrase(Some(file))?),
                    None => None,
                };
                c.set_instance_backup_schedule(&UpdateInstanceBackupSchedule {
                    enabled: !off,
                    hour_utc: hour.unwrap_or(current.hour_utc),
                    keep: keep.unwrap_or(current.keep),
                    storage_id,
                    passphrase,
                })
                .await?
            } else {
                current
            };
            if json {
                return output::json(&schedule);
            }
            if schedule.enabled {
                output::ok(format!(
                    "the instance is backed up daily at {:02}:00 UTC to {}, keeping the last {}",
                    schedule.hour_utc,
                    storage_label(&c.storages().await?, schedule.storage_id.as_deref()),
                    schedule.keep
                ));
            } else {
                println!(
                    "{} {}",
                    muted("scheduled instance backups are off:"),
                    hint("instance schedule --hour 2 --keep 7 --passphrase-file <file>")
                );
            }
        }
    }
    Ok(())
}

fn confirm(prompt: &str, yes: bool) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("pass --yes to confirm when not interactive");
    }
    print!("{prompt} {} ", muted("[y/N]"));
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if matches!(answer.trim(), "y" | "Y" | "yes") {
        Ok(())
    } else {
        bail!("cancelled")
    }
}

async fn project(c: &Client, cmd: ProjectCmd, json: bool) -> Result<()> {
    match cmd {
        ProjectCmd::List => {
            let tree = c.tree().await?;
            if json {
                return output::json(&tree.projects.iter().map(|p| &p.project).collect::<Vec<_>>());
            }
            if tree.projects.is_empty() {
                println!(
                    "{} {}",
                    muted("no projects yet:"),
                    hint("project create <name>")
                );
            }
            output::table(
                &["PROJECT", "ENVIRONMENTS", "SERVICES", "DESCRIPTION"],
                tree.projects
                    .iter()
                    .map(|p| {
                        let envs: Vec<&str> = p
                            .environments
                            .iter()
                            .map(|e| e.environment.name.as_str())
                            .collect();
                        let n: usize = p.environments.iter().map(|e| e.services.len()).sum();
                        vec![
                            Cell::from(p.project.name.clone()),
                            Cell(envs.join(", "), Tone::Muted),
                            Cell::from(n.to_string()),
                            Cell(
                                p.project.description.clone().unwrap_or_default(),
                                Tone::Muted,
                            ),
                        ]
                    })
                    .collect(),
            );
        }
        ProjectCmd::Create { name, description } => {
            let p = c
                .create_project(&CreateProject { name, description })
                .await?;
            output::ok(format!("created {} with a production environment", p.name));
        }
        ProjectCmd::Delete { name, yes } => {
            let tree = c.tree().await?;
            let p = resolve::project(&tree, &name)?;
            let n: usize = p.environments.iter().map(|e| e.services.len()).sum();
            confirm(
                &format!("Delete {} and its {n} services?", p.project.name),
                yes,
            )?;
            c.delete_project(&p.project.id).await?;
            output::ok(format!("deleted {}", p.project.name));
        }
    }
    Ok(())
}

async fn environment(c: &Client, cmd: EnvironmentCmd, json: bool) -> Result<()> {
    let tree = c.tree().await?;
    match cmd {
        EnvironmentCmd::List { project } => {
            let p = resolve::project(&tree, &project)?;
            if json {
                return output::json(
                    &p.environments
                        .iter()
                        .map(|e| &e.environment)
                        .collect::<Vec<_>>(),
                );
            }
            output::table(
                &["ENVIRONMENT", "SERVICES"],
                p.environments
                    .iter()
                    .map(|e| {
                        vec![
                            Cell::from(e.environment.name.clone()),
                            Cell::from(e.services.len().to_string()),
                        ]
                    })
                    .collect(),
            );
        }
        EnvironmentCmd::Create { project, name } => {
            let p = resolve::project(&tree, &project)?;
            let e = c
                .create_environment(&p.project.id, &CreateEnvironment { name })
                .await?;
            output::ok(format!("created {}/{}", p.project.name, e.name));
        }
        EnvironmentCmd::Delete { environment, yes } => {
            let (p, e) = resolve::environment(&tree, &environment)?;
            let prompt = format!(
                "Delete {}/{} and its {} services?",
                p.project.name,
                e.environment.name,
                e.services.len()
            );
            confirm(&prompt, yes)?;
            c.delete_environment(&e.environment.id).await?;
            output::ok(format!("deleted {}/{}", p.project.name, e.environment.name));
        }
    }
    Ok(())
}

fn parse_env(pairs: &[String], secret: bool) -> Result<Vec<EnvVar>> {
    pairs
        .iter()
        .map(|pair| {
            let (key, value) = pair
                .split_once('=')
                .with_context(|| format!("'{pair}' is not KEY=VALUE"))?;
            Ok(EnvVar {
                key: key.to_string(),
                value: value.to_string(),
                secret,
            })
        })
        .collect()
}

async fn service(c: &Client, cmd: ServiceCmd, json: bool) -> Result<()> {
    match cmd {
        ServiceCmd::List { scope } => {
            let tree = c.tree().await?;
            let mut rows = Vec::new();
            for p in &tree.projects {
                for e in &p.environments {
                    let in_scope = match scope
                        .as_deref()
                        .map(|s| s.split_once('/').unwrap_or((s, "")))
                    {
                        None => true,
                        Some((proj, "")) => p.project.name == proj,
                        Some((proj, env)) => p.project.name == proj && e.environment.name == env,
                    };
                    if in_scope {
                        rows.extend(e.services.iter().map(|s| (p, e, s)));
                    }
                }
            }
            if json {
                return output::json(&rows.iter().map(|r| r.2).collect::<Vec<_>>());
            }
            if rows.is_empty() {
                output::note("no services");
            }
            output::table(
                &["STATE", "SERVICE", "SOURCE", "DOMAIN", "DEPLOYED"],
                rows.iter()
                    .map(|(p, e, s)| {
                        vec![
                            output::state(s.state),
                            Cell::from(format!(
                                "{}/{}/{}",
                                p.project.name, e.environment.name, s.name
                            )),
                            Cell(s.source.label(), Tone::Muted),
                            Cell::from(s.primary_domain.clone().unwrap_or("—".into())),
                            Cell(
                                s.last_deployed_at.map_or("never".into(), output::ago),
                                Tone::Muted,
                            ),
                        ]
                    })
                    .collect(),
            );
        }
        ServiceCmd::Show { service } => {
            let (_, _, s) = find(c, &service).await?;
            let d = c.service(&s.id).await?;
            if json {
                return output::json(&d);
            }
            let k = |key: &str| muted(format!("{key:<11}"));
            let state = output::state(d.service.state);
            println!(
                "{} {}  {}",
                d.service.name,
                muted(format!("{}/{}", d.project.name, d.environment.name)),
                paint(state.0, state.1)
            );
            println!("{}{}", k("source"), d.service.source.label());
            if let Source::Git {
                build,
                root,
                watch_paths,
                ..
            } = &d.service.source
            {
                if !root.is_empty() {
                    let watch = if *watch_paths {
                        "deploys on pushes that change it"
                    } else {
                        "deploys on every push"
                    };
                    println!("{}{root} {}", k("root"), muted(format!("({watch})")));
                }
                println!("{}{}", k("build"), build_label(&d.service.source, build));
            }
            println!("{}{}", k("server"), d.server.name);
            println!(
                "{}{}",
                k("port"),
                d.service.port.map_or("—".into(), |p| p.to_string())
            );
            for dom in &d.domains {
                println!(
                    "{}{}://{}{}",
                    k("domain"),
                    if dom.tls { "https" } else { "http" },
                    dom.host,
                    dom.compose_service
                        .as_deref()
                        .map(|s| muted(format!(" → {s}")))
                        .unwrap_or_default()
                );
            }
            for cs in &d.compose {
                let state = cs
                    .container
                    .as_ref()
                    .map_or("not created", |c| c.state.as_str());
                println!("{}{} {}", k("compose"), cs.name, muted(state));
            }
            for v in &d.volumes {
                println!("{}{} → {}", k("volume"), v.name, v.mount_path);
            }
            println!("{}{} variables", k("env"), d.env.len());
            if let Some(cont) = &d.container {
                let id = &cont.id[..cont.id.len().min(12)];
                println!(
                    "{}{id} {} ({} restarts)",
                    k("container"),
                    cont.state,
                    cont.restart_count
                );
            }
        }
        ServiceCmd::Create(args) => create_service(c, *args).await?,
        ServiceCmd::Metrics {
            service,
            range,
            container,
        } => {
            let (p, e, s) = find(c, &service).await?;
            let m = c
                .service_metrics(&s.id, range, container.as_deref())
                .await?;
            if json {
                return output::json(&m);
            }
            print_metrics(&format!("{}/{}", p.name, e.name), &s, &m);
        }
        ServiceCmd::Build {
            service,
            method,
            target,
            compose_file,
            dockerfile,
            build_context,
            root,
            watch_paths,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let mut source = s.source.clone();
            let Source::Git {
                build,
                dockerfile: df,
                context,
                root: current_root,
                watch_paths: current_watch,
                ..
            } = &mut source
            else {
                bail!("{} is not built from git", s.name);
            };
            let changed = method.is_some()
                || target.is_some()
                || compose_file.is_some()
                || dockerfile.is_some()
                || build_context.is_some()
                || root.is_some()
                || watch_paths.is_some();
            if let Some(r) = root {
                *current_root = clean_root(&r).map_err(anyhow::Error::msg)?;
            }
            if let Some(w) = watch_paths {
                *current_watch = w;
            }
            if let Some(m) = method {
                build.method = m;
            }
            if let Some(t) = target {
                build.target = Some(t).filter(|t| !t.is_empty());
            }
            if let Some(f) = compose_file {
                build.compose_file = Some(f).filter(|f| !f.is_empty());
            }
            if let Some(d) = dockerfile {
                *df = d;
            }
            if let Some(ctx) = build_context {
                *context = ctx;
            }
            let d = if changed {
                let d = c
                    .update_service(
                        &s.id,
                        &UpdateService {
                            source: Some(source),
                            ..Default::default()
                        },
                    )
                    .await?;
                output::ok(format!("{} builds with {}; deploy to apply", s.name, {
                    let Source::Git { build, .. } = &d.service.source else {
                        unreachable!()
                    };
                    build.method
                }));
                d
            } else {
                c.service(&s.id).await?
            };
            if json {
                return output::json(&d.service.source);
            }
            if let Source::Git { build, root, .. } = &d.service.source {
                if !root.is_empty() {
                    println!("{} {root}", muted("root"));
                }
                println!("{}", build_label(&d.service.source, build));
            }
        }
        ServiceCmd::Move {
            service,
            server,
            copy_volumes,
            detach,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let to = find_server(c, &server).await?;
            let d = c
                .move_service(
                    &s.id,
                    &MoveService {
                        server_id: to.id.clone(),
                        copy_volumes,
                    },
                )
                .await?;
            if json {
                return output::json(&d);
            }
            if detach {
                output::ok(format!(
                    "moving {} to {} as deployment #{}",
                    s.name, to.name, d.number
                ));
                return Ok(());
            }
            follow_deployment(c, d).await?;
            output::ok(format!("{} runs on {} now", s.name, to.name));
        }
        ServiceCmd::Delete {
            service,
            volumes,
            yes,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let what = if volumes { " and its volumes" } else { "" };
            confirm(&format!("Delete {}{what}?", s.name), yes)?;
            c.delete_service(&s.id, volumes).await?;
            output::ok(format!("deleted {}", s.name));
        }
    }
    Ok(())
}

/// `dockerfile (Dockerfile, target prod)`, `compose (compose.yaml)`, `railpack`.
fn build_label(source: &Source, build: &Build) -> String {
    let Source::Git {
        dockerfile,
        context,
        ..
    } = source
    else {
        return String::new();
    };
    let detail = match build.method {
        BuildMethod::Dockerfile => {
            let mut parts = vec![if context == "." {
                dockerfile.clone()
            } else {
                format!("{context}/{dockerfile}")
            }];
            if let Some(t) = &build.target {
                parts.push(format!("target {t}"));
            }
            Some(parts.join(", "))
        }
        BuildMethod::Compose => Some(
            build
                .compose_file
                .clone()
                .unwrap_or_else(|| "compose.yaml or docker-compose.yml".into()),
        ),
        BuildMethod::Auto => Some("detected on the next deploy".into()),
        BuildMethod::Railpack => None,
    };
    match detail {
        Some(d) => format!("{} {}", build.method, muted(format!("({d})"))),
        None => build.method.to_string(),
    }
}

async fn create_service(c: &Client, args: CreateArgs) -> Result<()> {
    let tree = c.tree().await?;
    let (_, env) = resolve::environment(&tree, &args.environment)?;
    let source_id = match &args.source {
        Some(name) => Some(sources::find_source(c, name).await?.id),
        None => None,
    };
    let source = match (args.image, args.repo, args.database) {
        (Some(image), None, None) => Source::Image { image },
        (None, Some(repo), None) => Source::Git {
            repo,
            branch: args.branch,
            dockerfile: args.dockerfile,
            context: args.build_context,
            root: clean_root(args.root.as_deref().unwrap_or_default())
                .map_err(anyhow::Error::msg)?,
            watch_paths: !args.no_watch_paths,
            source_id,
            auto_deploy: !args.no_auto_deploy,
            build: Build {
                method: args.build,
                target: args.target,
                compose_file: args.compose_file,
            },
        },
        (None, None, Some(db)) => {
            let (kind, version) = db
                .split_once(':')
                .map_or((db.as_str(), None), |(k, v)| (k, Some(v.to_string())));
            let kind = kind.parse::<DatabaseKind>().map_err(anyhow::Error::msg)?;
            Source::Database {
                kind,
                version: version.unwrap_or_else(|| kind.default_version().into()),
            }
        }
        _ => bail!("pick a source: --image, --repo or --database"),
    };
    let name = match args.name {
        Some(n) => n,
        None => match &source {
            Source::Image { image } => derive_name(image),
            Source::Git { repo, .. } => derive_name(repo),
            Source::Database { kind, .. } => kind.image().to_string(),
        },
    };
    validate_name(&name).map_err(anyhow::Error::msg)?;
    let server_id = match args.server {
        Some(s) => Some(find_server(c, &s).await?.id),
        None => None,
    };
    let volumes = args
        .volumes
        .iter()
        .map(|v| {
            let (name, path) = v
                .split_once(':')
                .with_context(|| format!("'{v}' is not name:/path"))?;
            Ok(VolumeSpec {
                name: name.to_string(),
                mount_path: path.to_string(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let deploy = !args.no_deploy;
    let detail = c
        .create_service(&CreateService {
            environment_id: env.environment.id.clone(),
            server_id,
            name,
            source,
            port: args.port,
            command: args.command,
            restart: RestartPolicy::UnlessStopped,
            health_path: args.health,
            env: parse_env(&args.env, false)?,
            domains: args.domains,
            volumes,
            deploy,
        })
        .await?;
    output::ok(format!("created {}", detail.service.name));
    if deploy
        && let Some(d) = c
            .deployments(&detail.service.id, 1)
            .await?
            .into_iter()
            .next()
    {
        return follow_deployment(c, d).await;
    }
    Ok(())
}

/// Last path segment without tag, digest or ".git": "ghcr.io/acme/web:v3" gives "web".
fn derive_name(source: &str) -> String {
    let path = source.trim_end_matches('/').trim_end_matches(".git");
    let segment = path.rsplit('/').next().unwrap_or(path);
    let segment = segment.split([':', '@']).next().unwrap_or(segment);
    let name: String = segment
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.starts_with(|c: char| c.is_ascii_lowercase()) {
        name
    } else {
        format!("app-{name}")
    }
}

async fn domain(c: &Client, cmd: DomainCmd, json: bool) -> Result<()> {
    match cmd {
        DomainCmd::List { service } => {
            let (_, _, s) = find(c, &service).await?;
            let d = c.service(&s.id).await?;
            if json {
                return output::json(&d.domains);
            }
            output::table(
                &["DOMAIN", "PROTOCOL", "PORT"],
                d.domains
                    .iter()
                    .map(|dom| {
                        vec![
                            Cell::from(dom.host.clone()),
                            if dom.tls {
                                Cell("https".into(), Tone::Ok)
                            } else {
                                Cell("http".into(), Tone::Muted)
                            },
                            Cell::from(
                                dom.port
                                    .or(d.service.port)
                                    .map_or("—".into(), |p| p.to_string()),
                            ),
                        ]
                    })
                    .collect(),
            );
        }
        DomainCmd::Add {
            service,
            host,
            port,
            http,
            compose_service,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let d = c
                .add_domain(
                    &s.id,
                    &CreateDomain {
                        host,
                        port,
                        tls: !http,
                        compose_service,
                    },
                )
                .await?;
            output::ok(format!(
                "{}://{} → {}",
                if d.tls { "https" } else { "http" },
                d.host,
                s.name
            ));
            if let Some(ip) = c.info().await.ok().and_then(|i| i.public_ip) {
                output::note(format!("point an A record for {} at {ip}", d.host));
            }
        }
        DomainCmd::Remove { service, host } => {
            let (_, _, s) = find(c, &service).await?;
            let d = c.service(&s.id).await?;
            let dom = d
                .domains
                .iter()
                .find(|x| x.host == host)
                .with_context(|| format!("{} has no domain {host}", s.name))?;
            c.delete_domain(&dom.id).await?;
            output::ok(format!("removed {host}"));
        }
    }
    Ok(())
}

async fn vars(c: &Client, cmd: VarsCmd, json: bool) -> Result<()> {
    match cmd {
        VarsCmd::List { service, reveal } => {
            let (_, _, s) = find(c, &service).await?;
            let d = c.service(&s.id).await?;
            if json {
                return output::json(&d.env);
            }
            for v in &d.env {
                let value = if v.secret && !reveal {
                    muted("••••••••")
                } else {
                    v.value.clone()
                };
                println!("{}={value}", v.key);
            }
        }
        VarsCmd::Set {
            service,
            pairs,
            secret,
            deploy,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let mut current = c.service(&s.id).await?.env;
            for var in parse_env(&pairs, secret)? {
                match current.iter_mut().find(|v| v.key == var.key) {
                    Some(existing) => *existing = var,
                    None => current.push(var),
                }
            }
            c.set_env(&s.id, &current).await?;
            output::ok(format!(
                "set {} variable{} on {}",
                pairs.len(),
                if pairs.len() == 1 { "" } else { "s" },
                s.name
            ));
            redeploy_or_hint(c, &s, deploy).await?;
        }
        VarsCmd::Unset {
            service,
            keys,
            deploy,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let mut current = c.service(&s.id).await?.env;
            current.retain(|v| !keys.contains(&v.key));
            c.set_env(&s.id, &current).await?;
            output::ok(format!("removed {} from {}", keys.join(", "), s.name));
            redeploy_or_hint(c, &s, deploy).await?;
        }
    }
    Ok(())
}

async fn redeploy_or_hint(c: &Client, s: &Service, deploy: bool) -> Result<()> {
    if deploy {
        let d = c.deploy(&s.id, "cli").await?;
        return follow_deployment(c, d).await;
    }
    println!(
        "{} {}",
        muted("takes effect on the next deploy:"),
        hint(&format!("deploy {}", s.name))
    );
    Ok(())
}

async fn volume(c: &Client, cmd: VolumeCmd, json: bool) -> Result<()> {
    match cmd {
        VolumeCmd::List { service } => {
            let (_, _, s) = find(c, &service).await?;
            let d = c.service(&s.id).await?;
            if json {
                return output::json(&d.volumes);
            }
            output::table(
                &["VOLUME", "MOUNT"],
                d.volumes
                    .iter()
                    .map(|v| vec![Cell::from(v.name.clone()), Cell::from(v.mount_path.clone())])
                    .collect(),
            );
        }
        VolumeCmd::Add {
            service,
            name,
            path,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let v = c
                .add_volume(
                    &s.id,
                    &CreateVolume {
                        name,
                        mount_path: path,
                    },
                )
                .await?;
            output::ok(format!(
                "{} will mount at {} on the next deploy",
                v.name, v.mount_path
            ));
        }
        VolumeCmd::Remove {
            service,
            name,
            purge,
        } => {
            let (_, _, s) = find(c, &service).await?;
            let d = c.service(&s.id).await?;
            let v = d
                .volumes
                .iter()
                .find(|v| v.name == name)
                .with_context(|| format!("{} has no volume {name}", s.name))?;
            c.delete_volume(&v.id, purge).await?;
            output::ok(format!(
                "removed {name}{}",
                if purge { " and its data" } else { "" }
            ));
        }
    }
    Ok(())
}

const MASK: &str = "••••••••";

/// The URL with the password in its userinfo replaced by a mask.
fn mask_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let Some(at) = rest[..authority_end].rfind('@') else {
        return url.to_string();
    };
    let userinfo = &rest[..at];
    let user = userinfo.split_once(':').map_or(userinfo, |(user, _)| user);
    format!("{scheme}://{user}:{MASK}{}", &rest[at..])
}

/// The service, checked to be a database.
async fn find_database(c: &Client, query: &str) -> Result<Service> {
    let (_, _, s) = find(c, query).await?;
    if !matches!(s.source, Source::Database { .. }) {
        bail!("{} is not a database service", s.name);
    }
    Ok(s)
}

/// A backup of the service by id, id prefix or file name.
async fn find_backup(c: &Client, service: &Service, query: &str) -> Result<Backup> {
    let backups = c.backups(&service.id).await?;
    let mut matches = backups
        .iter()
        .filter(|b| b.id == query || b.file_name == query || b.id.starts_with(query));
    match (matches.next(), matches.next()) {
        (Some(b), None) => Ok(b.clone()),
        (Some(_), Some(_)) => bail!("'{query}' matches several backups of {}", service.name),
        (None, _) => bail!("{} has no backup '{query}'", service.name),
    }
}

fn print_database(info: &DatabaseInfo, name: &str, reveal: bool) {
    let k = |key: &str| muted(format!("{key:<10}"));
    let secret = |s: &str| {
        if reveal { s.to_string() } else { mask_url(s) }
    };
    println!(
        "{name} {}",
        muted(format!("{} {}", info.kind, info.version))
    );
    println!("{}{}:{}", k("host"), info.host, info.port);
    if let Some(user) = &info.username {
        println!("{}{user}", k("user"));
    }
    if let Some(db) = &info.database {
        println!("{}{db}", k("database"));
    }
    println!(
        "{}{}",
        k("password"),
        if reveal {
            info.password.clone()
        } else {
            muted(MASK)
        }
    );
    println!("{}{}", k("internal"), secret(&info.internal_url));
    match (&info.public_url, info.public_port) {
        (Some(url), _) => println!("{}{}", k("public"), secret(url)),
        (None, Some(port)) => println!(
            "{}port {port} {}",
            k("public"),
            muted("(the server's address is unknown)")
        ),
        (None, None) => println!(
            "{}{} {}",
            k("public"),
            muted("off:"),
            hint(&format!("db expose {name} <port>"))
        ),
    }
    if !reveal {
        output::note(format!(
            "add --reveal to show the password: {}",
            hint(&format!("db info {name} --reveal"))
        ));
    }
}

/// Follows the redeploy a database change queued, if any.
async fn follow_reconfigure(c: &Client, s: &Service) -> Result<()> {
    match c.deployments(&s.id, 1).await?.into_iter().next() {
        Some(d) if !d.status.is_finished() => follow_deployment(c, d).await,
        _ => Ok(()),
    }
}

fn backup_status(status: BackupStatus) -> Cell {
    let tone = match status {
        BackupStatus::Done => Tone::Ok,
        BackupStatus::Running => Tone::Info,
        BackupStatus::Failed => Tone::Err,
    };
    Cell(status.as_str().to_string(), tone)
}

async fn db(c: &Client, cmd: DbCmd, json: bool) -> Result<()> {
    match cmd {
        DbCmd::Info { service, reveal } => {
            let s = find_database(c, &service).await?;
            let mut info = c
                .service(&s.id)
                .await?
                .database
                .context("the daemon returned no connection details")?;
            if json {
                if !reveal {
                    info.password = MASK.into();
                    info.internal_url = mask_url(&info.internal_url);
                    info.public_url = info.public_url.as_deref().map(mask_url);
                }
                return output::json(&info);
            }
            print_database(&info, &s.name, reveal);
        }
        DbCmd::Expose { service, port } => {
            if port == 0 {
                bail!("port must be between 1 and 65535");
            }
            let s = find_database(c, &service).await?;
            let info = c
                .update_database(
                    &s.id,
                    &UpdateDatabase {
                        public_port: Some(port),
                    },
                )
                .await?;
            output::ok(format!("{} is published on port {port}", s.name));
            follow_reconfigure(c, &s).await?;
            if let Some(url) = info.public_url {
                println!("{} {}", muted("public:"), mask_url(&url));
            }
        }
        DbCmd::Unexpose { service } => {
            let s = find_database(c, &service).await?;
            c.update_database(&s.id, &UpdateDatabase { public_port: None })
                .await?;
            output::ok(format!("{} is internal-only", s.name));
            follow_reconfigure(c, &s).await?;
        }
        DbCmd::Backup { service, storage } => {
            let s = find_database(c, &service).await?;
            let target = match storage {
                Some(name) => Some(find_storage(c, &name).await?),
                None => None,
            };
            let mut b = c
                .create_backup(&s.id, target.as_ref().map(|t| t.id.as_str()))
                .await?;
            output::note(format!(
                "backing up {} to {} in {}",
                s.name,
                b.file_name,
                target.map_or("local storage".into(), |t| t.name)
            ));
            while b.status == BackupStatus::Running {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                b = c.backup(&b.id).await?;
            }
            if json {
                return output::json(&b);
            }
            match b.status {
                BackupStatus::Done => output::ok(format!(
                    "backup {} done ({}, {})",
                    b.id,
                    b.file_name,
                    output::size(b.size_bytes)
                )),
                _ => bail!("backup failed: {}", b.error.unwrap_or_default()),
            }
        }
        DbCmd::Backups { service } => {
            let s = find_database(c, &service).await?;
            let list = c.backups(&s.id).await?;
            if json {
                return output::json(&list);
            }
            let storages = c.storages().await?;
            if list.is_empty() {
                println!(
                    "{} {}",
                    muted("no backups yet:"),
                    hint(&format!("db backup {}", s.name))
                );
            }
            output::table(
                &[
                    "ID", "STATUS", "FILE", "SIZE", "WHERE", "TRIGGER", "CREATED",
                ],
                list.iter()
                    .map(|b| {
                        vec![
                            Cell::from(b.id.clone()),
                            backup_status(b.status),
                            Cell::from(b.file_name.clone()),
                            Cell(output::size(b.size_bytes), Tone::Muted),
                            Cell(
                                storage_label(&storages, b.storage_id.as_deref()),
                                Tone::Muted,
                            ),
                            Cell(b.trigger.as_str().to_string(), Tone::Muted),
                            Cell(
                                b.error.clone().unwrap_or_else(|| output::ago(b.created_at)),
                                if b.error.is_some() {
                                    Tone::Err
                                } else {
                                    Tone::Muted
                                },
                            ),
                        ]
                    })
                    .collect(),
            );
        }
        DbCmd::Restore {
            service,
            backup,
            yes,
        } => {
            let s = find_database(c, &service).await?;
            let b = find_backup(c, &s, &backup).await?;
            confirm(
                &format!(
                    "Restore {} into {}? Its current data is replaced.",
                    b.file_name, s.name
                ),
                yes,
            )?;
            output::note(format!("restoring {} into {}", b.file_name, s.name));
            c.restore_backup(&b.id).await?;
            output::ok(format!("restored {} into {}", b.file_name, s.name));
        }
        DbCmd::Download {
            service,
            backup,
            output: path,
        } => {
            use tokio::io::AsyncWriteExt as _;
            let s = find_database(c, &service).await?;
            let b = find_backup(c, &s, &backup).await?;
            let path = path.unwrap_or_else(|| PathBuf::from(&b.file_name));
            let mut stream = c.download_backup(&b.id).await?;
            let mut file = tokio::fs::File::create(&path)
                .await
                .with_context(|| format!("cannot create {}", path.display()))?;
            while let Some(chunk) = stream.next().await {
                file.write_all(&chunk?).await?;
            }
            file.flush().await?;
            output::ok(format!(
                "saved {} ({})",
                path.display(),
                output::size(b.size_bytes)
            ));
        }
        DbCmd::Schedule {
            service,
            hour,
            keep,
            off,
            storage,
        } => {
            let s = find_database(c, &service).await?;
            let mut schedule = c.backup_schedule(&s.id).await?;
            let changes = hour.is_some() || keep.is_some() || storage.is_some();
            if off {
                schedule.enabled = false;
            } else if changes {
                schedule.enabled = true;
                schedule.hour_utc = hour.unwrap_or(schedule.hour_utc);
                schedule.keep = keep.unwrap_or(schedule.keep);
                schedule.storage_id = match storage.as_deref() {
                    None => schedule.storage_id,
                    Some("local") => None,
                    Some(name) => Some(find_storage(c, name).await?.id),
                };
            }
            if off || changes {
                schedule = c.set_backup_schedule(&s.id, &schedule).await?;
            }
            if json {
                return output::json(&schedule);
            }
            if schedule.enabled {
                let storages = c.storages().await?;
                output::ok(format!(
                    "{} is backed up daily at {:02}:00 UTC to {}, keeping the last {}",
                    s.name,
                    schedule.hour_utc,
                    storage_label(&storages, schedule.storage_id.as_deref()),
                    schedule.keep
                ));
            } else {
                println!(
                    "{} {}",
                    muted(format!("scheduled backups of {} are off:", s.name)),
                    hint(&format!("db schedule {} --hour 3 --keep 7", s.name))
                );
            }
        }
        DbCmd::RotatePassword { service, yes } => {
            let s = find_database(c, &service).await?;
            confirm(
                &format!(
                    "Set a new password for {}? Clients using the old one are cut off.",
                    s.name
                ),
                yes,
            )?;
            let info = c.rotate_database_password(&s.id).await?;
            if json {
                return output::json(&info);
            }
            output::ok(format!("rotated the password of {}", s.name));
            println!("{} {}", muted("internal url:"), info.internal_url);
            if info.kind == DatabaseKind::Redis {
                output::note(format!(
                    "{} is redeployed so a restart keeps the new password",
                    s.name
                ));
            }
            output::note(
                "services that copied the old connection URL keep failing until they are updated and redeployed",
            );
        }
    }
    Ok(())
}

fn print_log(line: &LogLine, timestamps: bool) {
    let ts = match (timestamps, line.ts) {
        (true, Some(t)) => format!(
            "{} ",
            muted(
                t.with_timezone(&chrono::Local)
                    .format("%H:%M:%S%.3f")
                    .to_string()
            )
        ),
        _ => String::new(),
    };
    let text = match line.stream {
        LogStream::System => paint(&line.line, Tone::Info),
        LogStream::Stdout | LogStream::Stderr => line.line.clone(),
    };
    println!("{ts}{text}");
}

/// Stream a deployment's build output and fail if the deployment fails.
async fn follow_deployment(c: &Client, d: Deployment) -> Result<()> {
    output::note(format!(
        "deployment #{} · ctrl+c stops watching, the deploy keeps running",
        d.number
    ));
    let mut stream = c.deployment_logs(&d.id, true).await?;
    while let Some(line) = stream.next().await {
        print_log(&line?, false);
    }
    let mut d = c.deployment(&d.id).await?;
    while !d.status.is_finished() {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        d = c.deployment(&d.id).await?;
    }
    let took = d
        .finished_at
        .map(|f| (f - d.created_at).num_seconds())
        .unwrap_or_default();
    match d.status {
        DeploymentStatus::Live => {
            output::ok(format!("deployment #{} is live after {took}s", d.number));
            Ok(())
        }
        status => bail!(
            "deployment #{} {}: {}",
            d.number,
            status.as_str(),
            d.error.unwrap_or_default()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    fn parse(args: &[&str]) -> Option<Cmd> {
        let argv = std::iter::once(brand::CLI_BIN).chain(args.iter().copied());
        Cli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn update_flags_pick_the_sides() {
        let steps = |args: &[&str]| match parse(args) {
            Some(Cmd::Update { cli, server, .. }) => instance::Steps::from_flags(cli, server),
            Some(Cmd::Upgrade { .. }) => instance::Steps::CLI,
            _ => panic!("{args:?} is not an update"),
        };
        let both = instance::Steps {
            cli: true,
            server: true,
        };
        let server = instance::Steps {
            cli: false,
            server: true,
        };
        assert_eq!(steps(&["update"]), both);
        assert_eq!(steps(&["update", "--cli", "--server"]), both);
        assert_eq!(steps(&["update", "--cli"]), instance::Steps::CLI);
        assert_eq!(steps(&["update", "--self"]), instance::Steps::CLI);
        assert_eq!(steps(&["upgrade", "--tag", "v0.4.1"]), instance::Steps::CLI);
        assert_eq!(steps(&["update", "--server", "--force"]), server);

        let mut cmd = Cli::command();
        let help = cmd
            .find_subcommand_mut("update")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(help.contains("--cli") && help.contains("--server"));
        assert!(!help.contains("--self"), "--self stays a hidden alias");
    }

    #[test]
    fn service_move_and_setup_flags_parse() {
        match parse(&[
            "service",
            "move",
            "web",
            "--server",
            "edge",
            "--copy-volumes",
        ]) {
            Some(Cmd::Service(ServiceCmd::Move {
                service,
                server,
                copy_volumes,
                detach,
            })) => {
                assert_eq!((service.as_str(), server.as_str()), ("web", "edge"));
                assert!(copy_volumes && !detach);
            }
            _ => panic!("not a service move"),
        }
        let argv = [brand::CLI_BIN, "service", "move", "web"];
        assert!(Cli::try_parse_from(argv).is_err(), "--server is required");
        match parse(&["setup", "--server", "edge", "--root", "apps/web"]) {
            Some(Cmd::Setup {
                status,
                server,
                root,
            }) => {
                assert!(!status);
                assert_eq!(server.as_deref(), Some("edge"));
                assert_eq!(root.as_deref(), Some("apps/web"));
            }
            _ => panic!("not setup"),
        }
    }

    #[test]
    fn skew_hint_is_only_for_human_commands() {
        let status = parse(&["status"]).unwrap();
        assert!(shows_skew_hint(&status, false));
        assert!(!shows_skew_hint(&status, true));
        assert!(!shows_skew_hint(&parse(&[]).unwrap_or(Cmd::Tui), false));
        assert!(!shows_skew_hint(&parse(&["tui"]).unwrap(), false));
    }

    #[test]
    fn urls_are_masked() {
        assert_eq!(
            mask_url("postgres://app:s3cret@db:5432/app"),
            format!("postgres://app:{MASK}@db:5432/app")
        );
        assert_eq!(
            mask_url("redis://:s3cret@cache:6379"),
            format!("redis://:{MASK}@cache:6379")
        );
        assert_eq!(
            mask_url("mongodb://app:p%40ss@mongo:27017/?authSource=admin"),
            format!("mongodb://app:{MASK}@mongo:27017/?authSource=admin")
        );
        assert_eq!(mask_url("not a url"), "not a url");
    }

    #[test]
    fn names_derive_from_sources() {
        assert_eq!(derive_name("https://github.com/acme/api.git"), "api");
        assert_eq!(derive_name("ghcr.io/imgproxy/imgproxy:v3"), "imgproxy");
        assert_eq!(derive_name("nginx:1.27"), "nginx");
    }
}
