//! Git sources (GitHub Apps and tokens) and the instance domain they need.

use crate::output::{self, Cell, Tone, muted, paint};
use crate::{confirm, hint};
use anyhow::{Context as _, Result, bail};
use clap::Subcommand;
use client::{Client, Config, Context};
use common::api::{ConnectGithubApp, ConnectGithubToken, CreateGithubApp};
use common::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Subcommand)]
pub enum DomainCmd {
    /// Show the domain, the DNS records to create and the last check.
    Show,
    /// Serve the API on a domain with automatic HTTPS, then check it.
    Set { domain: String },
    /// Run the checks again: DNS, ports 80 and 443, certificate, API.
    Check,
    /// Serve the API on IP:port only again.
    #[command(alias = "remove")]
    Rm {
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum SourceCmd {
    /// All git sources.
    List,
    /// Details of one source.
    Show { source: String },
    /// Ask GitHub whether the source still works.
    Check { source: String },
    /// Delete a source. Sources a service clones from cannot be deleted.
    #[command(alias = "remove")]
    Rm {
        source: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Repositories a source can clone.
    Repos {
        source: String,
        /// Only repositories whose owner/name contains this.
        #[arg(long, short)]
        search: Option<String>,
        #[arg(long, default_value_t = 1)]
        page: u32,
    },
    /// Branches of a repository, the default one first.
    Branches { source: String, repo: String },
    /// GitHub: create or connect apps, or add a token.
    #[command(subcommand)]
    Github(GithubCmd),
}

#[derive(Subcommand)]
pub enum GithubCmd {
    /// Create a GitHub App in your browser and install it (needs a verified instance domain).
    Create {
        name: String,
        /// Create the app in this organization instead of your personal account.
        #[arg(long, alias = "organization")]
        org: Option<String>,
        /// GitHub Enterprise URL, e.g. https://github.acme.dev.
        #[arg(long)]
        github_url: Option<String>,
        /// GitHub Enterprise API URL; defaults to <github-url>/api/v3.
        #[arg(long)]
        api_url: Option<String>,
        /// Print the setup URL and return instead of waiting for the install.
        #[arg(long)]
        no_wait: bool,
    },
    /// Connect a GitHub App that already exists.
    Connect {
        name: String,
        #[arg(long)]
        app_id: u64,
        /// The app's private key (.pem).
        #[arg(long)]
        private_key_file: PathBuf,
        /// Defaults to the app's only installation.
        #[arg(long)]
        installation_id: Option<u64>,
        /// File holding the app's webhook secret, for deploys on push.
        #[arg(long)]
        webhook_secret_file: Option<PathBuf>,
        #[arg(long)]
        github_url: Option<String>,
        #[arg(long)]
        api_url: Option<String>,
    },
    /// Use a personal access token, for single repositories without push deploys. The token
    /// comes from --token-file or stdin.
    Token {
        name: String,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        github_url: Option<String>,
        #[arg(long)]
        api_url: Option<String>,
    },
    /// Same as "source list".
    List,
    /// Same as "source rm".
    #[command(alias = "remove")]
    Rm {
        source: String,
        #[arg(long, short)]
        yes: bool,
    },
}

pub async fn find_source(c: &Client, query: &str) -> Result<GitSource> {
    c.sources()
        .await?
        .into_iter()
        .find(|s| s.name == query || s.id == query)
        .with_context(|| format!("no git source named '{query}': see {}", hint("source list")))
}

fn status_cell(s: &GitSource) -> Cell {
    let (glyph, tone) = match s.status {
        GitSourceStatus::Ready => ("●", Tone::Ok),
        GitSourceStatus::Pending => ("◌", Tone::Info),
        GitSourceStatus::NeedsInstall => ("○", Tone::Accent),
        GitSourceStatus::Error => ("✕", Tone::Err),
    };
    Cell(format!("{glyph} {}", s.status.as_str()), tone)
}

fn account(s: &GitSource) -> String {
    match (&s.owner, s.owner_kind) {
        (Some(o), Some(GitOwnerKind::Organization)) => format!("{o} (org)"),
        (Some(o), _) => o.clone(),
        (None, _) => "—".into(),
    }
}

pub async fn source(c: &Client, cmd: SourceCmd, json: bool) -> Result<()> {
    match cmd {
        SourceCmd::List | SourceCmd::Github(GithubCmd::List) => {
            let list = c.sources().await?;
            if json {
                return output::json(&list);
            }
            if list.is_empty() {
                output::note(format!(
                    "no git sources yet; create a GitHub App with {}",
                    hint("source github create <name> [--org <org>]")
                ));
            }
            output::table(
                &["SOURCE", "KIND", "ACCOUNT", "GITHUB", "STATUS", "USED BY"],
                list.iter()
                    .map(|s| {
                        vec![
                            Cell::from(s.name.clone()),
                            Cell(s.kind.label().into(), Tone::Muted),
                            Cell::from(account(s)),
                            Cell(s.html_url.clone(), Tone::Muted),
                            status_cell(s),
                            Cell(
                                match s.used_by.len() {
                                    0 => "—".into(),
                                    1 => "1 service".into(),
                                    n => format!("{n} services"),
                                },
                                Tone::Muted,
                            ),
                        ]
                    })
                    .collect(),
            );
        }
        SourceCmd::Show { source } => {
            let s = find_source(c, &source).await?;
            print_source(&s, json)?;
        }
        SourceCmd::Check { source } => {
            let s = find_source(c, &source).await?;
            let s = c.check_source(&s.id).await?;
            print_source(&s, json)?;
            if s.status == GitSourceStatus::Error {
                bail!("source {} does not work", s.name);
            }
        }
        SourceCmd::Rm { source, yes } | SourceCmd::Github(GithubCmd::Rm { source, yes }) => {
            let s = find_source(c, &source).await?;
            confirm(&format!("Delete git source {}?", s.name), yes)?;
            c.delete_source(&s.id).await?;
            output::ok(format!("deleted git source {}", s.name));
            if let Some(slug) = &s.app_slug {
                output::note(format!(
                    "the GitHub App {slug} still exists on GitHub; delete it there if you no longer need it"
                ));
            }
        }
        SourceCmd::Repos {
            source,
            search,
            page,
        } => {
            let s = find_source(c, &source).await?;
            let p = c
                .source_repos(&s.id, search.as_deref().unwrap_or(""), page, 30)
                .await?;
            if json {
                return output::json(&p);
            }
            output::table(
                &["REPOSITORY", "", "DEFAULT BRANCH", "PUSHED"],
                p.repos
                    .iter()
                    .map(|r| {
                        vec![
                            Cell::from(r.full_name.clone()),
                            Cell(if r.private { "private" } else { "" }.into(), Tone::Muted),
                            Cell::from(r.default_branch.clone()),
                            Cell(
                                r.pushed_at.map(output::ago).unwrap_or_default(),
                                Tone::Muted,
                            ),
                        ]
                    })
                    .collect(),
            );
            output::note(format!(
                "page {} of {} · {} repositories",
                p.page,
                p.pages(),
                p.total
            ));
        }
        SourceCmd::Branches { source, repo } => {
            let s = find_source(c, &source).await?;
            let branches = c.source_branches(&s.id, &repo).await?;
            if json {
                return output::json(&branches);
            }
            for b in branches {
                if b.default {
                    println!("{} {}", b.name, muted("default"));
                } else {
                    println!("{}", b.name);
                }
            }
        }
        SourceCmd::Github(cmd) => github(c, cmd, json).await?,
    }
    Ok(())
}

fn print_source(s: &GitSource, json: bool) -> Result<()> {
    if json {
        return output::json(s);
    }
    let row = |k: &str, v: String| println!("{:<14}{v}", muted(k));
    row("source", s.name.clone());
    row("kind", s.kind.label().into());
    println!("{:<14}{}", muted("status"), status_cell(s).0);
    if let Some(d) = &s.status_detail {
        row("", d.clone());
    }
    row("account", account(s));
    row("github", s.html_url.clone());
    if let Some(slug) = &s.app_slug {
        row(
            "app",
            format!(
                "{slug}{}",
                s.app_id.map(|id| format!(" (id {id})")).unwrap_or_default()
            ),
        );
    }
    if let Some(id) = s.installation_id {
        row("installation", id.to_string());
    }
    if let Some(url) = &s.install_url {
        row("install", url.clone());
    }
    if let Some(url) = &s.webhook_url {
        row("webhook", url.clone());
    }
    if !s.used_by.is_empty() {
        row(
            "used by",
            s.used_by
                .iter()
                .map(|u| u.name.clone())
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if let Some(at) = s.last_checked_at {
        row("checked", output::ago(at));
    }
    Ok(())
}

fn read_secret_file(path: Option<&PathBuf>, what: &str) -> Result<String> {
    let raw = match path {
        Some(p) => {
            std::fs::read_to_string(p).with_context(|| format!("cannot read {}", p.display()))?
        }
        None => {
            use std::io::{IsTerminal, Read};
            if std::io::stdin().is_terminal() {
                eprint!("{what}: ");
            }
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s)?;
            s
        }
    };
    let value = raw.trim().to_string();
    if value.is_empty() {
        bail!("the {what} is empty");
    }
    Ok(value)
}

/// Opens a URL in the default browser when this machine has one; failures are ignored.
fn open_browser(url: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

async fn github(c: &Client, cmd: GithubCmd, json: bool) -> Result<()> {
    match cmd {
        GithubCmd::Create {
            name,
            org,
            github_url,
            api_url,
            no_wait,
        } => {
            let setup = c
                .create_github_app(&CreateGithubApp {
                    name,
                    organization: org,
                    html_url: github_url,
                    api_url,
                })
                .await?;
            if json && no_wait {
                return output::json(&setup);
            }
            println!("Open this URL in your browser to create the app on GitHub:");
            println!();
            println!("  {}", paint(&setup.setup_url, Tone::Accent));
            println!();
            open_browser(&setup.setup_url);
            if no_wait {
                output::note(format!(
                    "the link works for an hour; see progress with {}",
                    hint(&format!("source show {}", setup.source.name))
                ));
                return Ok(());
            }
            let s = wait_for_install(c, &setup.source).await?;
            if json {
                return output::json(&s);
            }
            output::ok(format!("source {} is connected to {}", s.name, account(&s)));
            println!(
                "{} {}",
                muted("next:"),
                hint(&format!(
                    "service create <project> --source {} --repo owner/name --branch main",
                    s.name
                ))
            );
        }
        GithubCmd::Connect {
            name,
            app_id,
            private_key_file,
            installation_id,
            webhook_secret_file,
            github_url,
            api_url,
        } => {
            let private_key = std::fs::read_to_string(&private_key_file)
                .with_context(|| format!("cannot read {}", private_key_file.display()))?;
            let webhook_secret = webhook_secret_file
                .as_ref()
                .map(|p| read_secret_file(Some(p), "webhook secret"))
                .transpose()?;
            let s = c
                .connect_github_app(&ConnectGithubApp {
                    name,
                    app_id,
                    private_key,
                    installation_id,
                    webhook_secret,
                    html_url: github_url,
                    api_url,
                })
                .await?;
            print_source(&s, json)?;
            if !json && s.webhook_url.is_some() {
                output::note(
                    "set the app's webhook URL on GitHub to the one above for deploys on push",
                );
            }
        }
        GithubCmd::Token {
            name,
            token_file,
            github_url,
            api_url,
        } => {
            let token = read_secret_file(token_file.as_ref(), "token")?;
            let s = c
                .connect_github_token(&ConnectGithubToken {
                    name,
                    token,
                    html_url: github_url,
                    api_url,
                })
                .await?;
            print_source(&s, json)?;
        }
        GithubCmd::List | GithubCmd::Rm { .. } => unreachable!("handled by source()"),
    }
    Ok(())
}

/// Follows a new source through app creation and installation, printing each step once.
async fn wait_for_install(c: &Client, source: &GitSource) -> Result<GitSource> {
    const TIMEOUT: Duration = Duration::from_secs(30 * 60);
    let started = Instant::now();
    let mut last = source.status;
    output::note("waiting for GitHub; press Ctrl-C to stop waiting (the source stays pending)");
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let s = c.source(&source.id).await?;
        if s.status != last {
            last = s.status;
            match s.status {
                GitSourceStatus::NeedsInstall => {
                    output::ok(format!(
                        "created the app {}",
                        s.app_slug.as_deref().unwrap_or_default()
                    ));
                    if let Some(url) = &s.install_url {
                        println!(
                            "  install it on the account and pick repositories: {}",
                            paint(url, Tone::Accent)
                        );
                    }
                }
                GitSourceStatus::Ready => return Ok(s),
                GitSourceStatus::Error => bail!(
                    "source {} failed: {}",
                    s.name,
                    s.status_detail.unwrap_or_default()
                ),
                GitSourceStatus::Pending => {}
            }
        }
        if started.elapsed() > TIMEOUT {
            bail!(
                "gave up waiting; finish in the browser, then run {}",
                hint(&format!("source check {}", s.name))
            );
        }
    }
}

fn check_line(check: &DomainCheck) {
    let (glyph, tone, word) = match check.status {
        CheckStatus::Pass => ("●", Tone::Ok, "ok"),
        CheckStatus::Fail => ("✕", Tone::Err, "failed"),
        CheckStatus::Skipped => ("○", Tone::Muted, "not run"),
    };
    println!(
        "  {} {:<26} {:<8} {}",
        paint(glyph, tone),
        check.kind.label(),
        paint(word, tone),
        muted(&check.detail)
    );
    if let Some(h) = &check.hint {
        println!("    {h}");
    }
}

fn print_domain(d: &InstanceDomain) {
    let Some(domain) = &d.domain else {
        println!("No instance domain is set. The API is served on IP:port only.");
        println!(
            "{} {}",
            muted("set one:"),
            hint("instance domain set deploy.example.com")
        );
        return;
    };
    let state = if d.verified {
        paint("verified", Tone::Ok)
    } else {
        paint("not verified yet", Tone::Accent)
    };
    println!("{:<10}{domain}  {state}", muted("domain"));
    println!();
    println!("1  Point DNS at this server");
    if d.records.is_empty() {
        output::note(
            "   this server's public address is unknown: set it with \"settings set public-ip <ip>\"",
        );
    }
    output::table(
        &["   TYPE", "NAME", "VALUE"],
        d.records
            .iter()
            .map(|r| {
                vec![
                    Cell::from(format!("   {}", r.kind)),
                    Cell::from(r.name.clone()),
                    Cell::from(r.value.clone()),
                ]
            })
            .collect(),
    );
    if d.records.len() > 1 {
        output::note("   Create both at your DNS provider. Skip AAAA if you don't use IPv6.");
    }
    println!();
    println!(
        "2  Checks{}",
        d.checked_at
            .map(|at| muted(format!("  last run {}", output::ago(at))))
            .unwrap_or_default()
    );
    if d.checks.is_empty() {
        println!(
            "  {}",
            muted(format!("not run yet: {}", hint("instance domain check")))
        );
    }
    for check in &d.checks {
        check_line(check);
    }
    if let Some(url) = &d.url {
        println!();
        println!("{} {}", muted("log in with"), paint(url, Tone::Accent));
    }
}

/// Makes the current context use the verified domain once it reaches the same daemon with the
/// same token. IP:port keeps working; this only changes what this CLI talks to.
async fn prefer_domain(
    config: &mut Config,
    name: &str,
    c: &Client,
    d: &InstanceDomain,
) -> Result<()> {
    let Some(url) = &d.url else {
        return Ok(());
    };
    let Some(ctx) = config.contexts.get(name).cloned() else {
        return Ok(());
    };
    if ctx.url.trim_end_matches('/') == url.trim_end_matches('/') {
        return Ok(());
    }
    if Client::new(url, c.token())?.info().await.is_err() {
        output::note(format!(
            "{url} does not answer from here yet; keeping {}",
            ctx.url
        ));
        return Ok(());
    }
    config.contexts.insert(
        name.to_string(),
        Context {
            url: url.clone(),
            token: ctx.token,
        },
    );
    config.save()?;
    output::ok(format!("context {name} now uses {url} (was {})", ctx.url));
    Ok(())
}

pub async fn domain(
    config: &mut Config,
    name: &str,
    c: &Client,
    cmd: DomainCmd,
    json: bool,
) -> Result<()> {
    let d = match cmd {
        DomainCmd::Show => c.instance_domain().await?,
        DomainCmd::Set { domain } => c.set_instance_domain(&domain).await?,
        DomainCmd::Check => c.check_instance_domain().await?,
        DomainCmd::Rm { yes } => {
            confirm(
                "Remove the instance domain? GitHub App webhooks stop arriving until a domain is set again.",
                yes,
            )?;
            let d = c.remove_instance_domain().await?;
            output::ok("the API is served on IP:port only");
            d
        }
    };
    if json {
        return output::json(&d);
    }
    print_domain(&d);
    prefer_domain(config, name, c, &d).await?;
    if d.domain.is_some() && !d.verified && !d.checks.is_empty() {
        println!();
        output::note(format!(
            "fix the failed step, then run {}",
            hint("instance domain check")
        ));
    }
    Ok(())
}
