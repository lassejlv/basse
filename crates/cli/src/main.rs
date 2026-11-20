//! Command line interface. With no subcommand it opens the TUI.

mod output;
mod resolve;

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand};
use client::{Client, Config, Context};
use common::api::*;
use common::*;
use futures_util::StreamExt;
use output::{Cell, Tone, muted, paint};

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
    /// Daemon, host and server health.
    Status,
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
}

#[derive(Subcommand)]
enum ContextCmd {
    List,
    Use { name: String },
    Remove { name: String },
}

#[derive(Subcommand)]
enum ServerCmd {
    List,
    /// Add a server reached over SSH with the daemon's key (see "server key").
    Add {
        name: String,
        host: String,
        #[arg(long, default_value = "root")]
        user: String,
        #[arg(long, default_value_t = 22)]
        port: u16,
    },
    Check {
        name: String,
    },
    /// Install or restart the reverse proxy on a server.
    Proxy {
        name: String,
    },
    Remove {
        name: String,
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
    #[arg(long, group = "source")]
    image: Option<String>,
    /// Git repository to build with its Dockerfile.
    #[arg(long, group = "source")]
    repo: Option<String>,
    #[arg(long, default_value = "main", requires = "repo")]
    branch: String,
    #[arg(long, default_value = "Dockerfile", requires = "repo")]
    dockerfile: String,
    /// Build context directory inside the repository.
    #[arg(long = "build-context", default_value = ".", requires = "repo")]
    build_context: String,
    /// One-click database: postgres, mysql, mariadb, redis or mongo, optionally with :version.
    #[arg(long, group = "source")]
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
        _ => {}
    }
    let (name, ctx) = config.resolve(cli.context.as_deref())?;
    let c = Client::from_context(&ctx)?;
    match cli.command.unwrap_or(Cmd::Tui) {
        Cmd::Tui => tui::run(c, name).await,
        Cmd::Login { .. } | Cmd::Context(_) => unreachable!("handled above"),
        Cmd::Status => status(&c, &name, json).await,
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
        } => {
            let (_, _, s) = find(&c, &service).await?;
            let mut stream = c.service_logs(&s.id, tail, follow).await?;
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
    }
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
    println!(
        "{} run {} to open the dashboard",
        muted("next:"),
        paint(brand::CLI_BIN, Tone::Accent)
    );
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
        ContextCmd::Remove { name } => {
            if config.contexts.remove(&name).is_none() {
                bail!("unknown context '{name}'");
            }
            if config.current.as_deref() == Some(&name) {
                config.current = config.contexts.keys().next().cloned();
            }
            config.save()?;
            output::ok(format!("removed {name}"));
        }
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
        } => {
            let s = c
                .create_server(&CreateServer {
                    name,
                    host,
                    user,
                    port,
                })
                .await?;
            let s = c.check_server(&s.id).await?;
            if s.status == ServerStatus::Online {
                output::ok(format!(
                    "{} is online (docker {})",
                    s.name,
                    s.docker_version.unwrap_or_default()
                ));
            } else {
                output::note(format!("{} was added but is not reachable yet.", s.name));
                println!(
                    "{} trust the key from {} on it, then run {}",
                    muted("next:"),
                    hint("server key"),
                    hint(&format!("server check {}", s.name))
                );
            }
        }
        ServerCmd::Check { name } => {
            let s = c.check_server(&find_server(c, &name).await?.id).await?;
            print_servers(&[s]);
        }
        ServerCmd::Proxy { name } => {
            let s = c.setup_proxy(&find_server(c, &name).await?.id).await?;
            output::ok(format!("proxy running on {}", s.name));
        }
        ServerCmd::Remove { name } => {
            let s = find_server(c, &name).await?;
            c.delete_server(&s.id).await?;
            output::ok(format!("removed {}", s.name));
        }
        ServerCmd::Key => match c.info().await?.ssh_public_key {
            Some(key) => println!("{key}"),
            None => bail!("the daemon has no SSH key"),
        },
    }
    Ok(())
}

fn confirm(prompt: &str, yes: bool) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("refusing to delete without --yes when not interactive");
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
            println!("{}{}", k("server"), d.server.name);
            println!(
                "{}{}",
                k("port"),
                d.service.port.map_or("—".into(), |p| p.to_string())
            );
            for dom in &d.domains {
                println!(
                    "{}{}://{}",
                    k("domain"),
                    if dom.tls { "https" } else { "http" },
                    dom.host
                );
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

async fn create_service(c: &Client, args: CreateArgs) -> Result<()> {
    let tree = c.tree().await?;
    let (_, env) = resolve::environment(&tree, &args.environment)?;
    let source = match (args.image, args.repo, args.database) {
        (Some(image), None, None) => Source::Image { image },
        (None, Some(repo), None) => Source::Git {
            repo,
            branch: args.branch,
            dockerfile: args.dockerfile,
            context: args.build_context,
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
        } => {
            let (_, _, s) = find(c, &service).await?;
            let d = c
                .add_domain(
                    &s.id,
                    &CreateDomain {
                        host,
                        port,
                        tls: !http,
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

    #[test]
    fn names_derive_from_sources() {
        assert_eq!(derive_name("https://github.com/acme/api.git"), "api");
        assert_eq!(derive_name("ghcr.io/imgproxy/imgproxy:v3"), "imgproxy");
        assert_eq!(derive_name("nginx:1.27"), "nginx");
    }
}
