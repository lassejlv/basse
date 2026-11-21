//! `setup`: the first-run checklist as interactive prompts, for people who don't use the TUI.
//! Progress is the daemon's, so the TUI and the CLI resume each other.

use crate::output::{self, Tone, muted, paint};
use crate::{follow_deployment, sources};
use anyhow::{Context as _, Result, bail};
use client::Client;
use common::api::*;
use common::notify::{ChannelConfig, RuleScope, preset};
use common::onboarding::{self, Onboarding, OnboardingStatus, Step, StepState};
use common::*;
use std::io::{BufRead, Write};

/// Reads answers from a terminal, or from any reader in tests.
pub struct Prompt<'a> {
    input: &'a mut dyn BufRead,
    /// Turn off echo for secrets; only when the input is a terminal.
    tty: bool,
}

impl<'a> Prompt<'a> {
    pub fn new(input: &'a mut dyn BufRead, tty: bool) -> Self {
        Self { input, tty }
    }

    /// One trimmed line; an empty answer takes `default`.
    fn ask(&mut self, question: &str, default: Option<&str>) -> Result<String> {
        match default {
            Some(d) if !d.is_empty() => print!("  {question} {} ", muted(format!("[{d}]"))),
            _ => print!("  {question} "),
        }
        std::io::stdout().flush()?;
        let mut line = String::new();
        if self.input.read_line(&mut line)? == 0 {
            bail!("input ended; run setup again to continue where you left off");
        }
        let line = line.trim();
        Ok(if line.is_empty() {
            default.unwrap_or_default().to_string()
        } else {
            line.to_string()
        })
    }

    /// Like `ask`, without echoing what is typed.
    fn secret(&mut self, question: &str) -> Result<String> {
        let stty = |arg: &str| {
            let _ = std::process::Command::new("stty")
                .arg(arg)
                .stdin(std::process::Stdio::inherit())
                .status();
        };
        if self.tty {
            stty("-echo");
        }
        let answer = self.ask(question, None);
        if self.tty {
            stty("echo");
            println!();
        }
        answer
    }

    fn yes(&mut self, question: &str) -> Result<bool> {
        let answer = self.ask(&format!("{question} {}", muted("[y/N]")), None)?;
        Ok(matches!(answer.as_str(), "y" | "Y" | "yes"))
    }

    /// A numbered menu; returns the index of the pick. The first option is the default.
    fn menu(&mut self, options: &[(&str, &str)]) -> Result<usize> {
        for (i, (label, note)) in options.iter().enumerate() {
            println!(
                "  {}  {label:<16}{}",
                paint((i + 1).to_string(), Tone::Accent),
                muted(note)
            );
        }
        loop {
            let answer = self.ask("choose", Some("1"))?;
            match answer.parse::<usize>() {
                Ok(n) if (1..=options.len()).contains(&n) => return Ok(n - 1),
                _ => println!("  {}", muted(format!("pick 1 to {}", options.len()))),
            }
        }
    }
}

/// "  ✓ message", indented under the step's heading.
fn ok(message: impl AsRef<str>) {
    println!("  {} {}", paint("✓", Tone::Ok), message.as_ref());
}

fn glyph(state: StepState, current: bool) -> String {
    match state {
        StepState::Done => paint("✓", Tone::Ok),
        StepState::Skipped => muted("–"),
        StepState::Todo if current => paint("●", Tone::Accent),
        StepState::Todo => muted("○"),
    }
}

fn kind(step: Step) -> &'static str {
    if step.recommended() {
        "recommended"
    } else if step.optional() {
        "optional"
    } else {
        ""
    }
}

pub fn print_status(o: &Onboarding) {
    println!(
        "Setup  {}",
        muted(format!("{} of {} done", o.done(), o.total()))
    );
    for s in &o.steps {
        let note = match s.state {
            StepState::Skipped => "skipped",
            _ => kind(s.step),
        };
        println!(
            "  {} {:<18}{}",
            glyph(s.state, false),
            s.step.label(),
            muted(note)
        );
    }
    if o.first_open() != Step::Done || o.steps.iter().any(|s| s.state == StepState::Skipped) {
        println!();
        output::note(format!("continue with: {} setup", brand::CLI_BIN));
    }
}

/// Walks every step that is not done yet. Returns the final state.
pub async fn run(c: &Client, p: &mut Prompt<'_>) -> Result<Onboarding> {
    let info = c.info().await?;
    println!();
    println!("Setting up {}", paint(&info.hostname, Tone::Accent));
    println!(
        "{}",
        muted(format!(
            "{} · {} {} · every step can be skipped",
            info.public_ip.as_deref().unwrap_or("address unknown"),
            brand::DAEMON_BIN,
            info.version
        ))
    );
    let mut o = c.onboarding().await?;
    let total = Step::TASKS.len();
    for (i, step) in Step::TASKS.into_iter().enumerate() {
        println!();
        let heading = format!("{}/{total}  {}", i + 1, step.label());
        if o.state(step) == StepState::Done {
            println!("{}  {}", heading, paint("✓ done", Tone::Ok));
            continue;
        }
        println!("{heading}  {}", muted(kind(step)));
        c.update_onboarding(&UpdateOnboarding {
            current: Some(step),
            ..Default::default()
        })
        .await?;
        let done = match step {
            Step::Domain => domain(c, p).await?,
            Step::Source => source(c, p, &info).await?,
            Step::Backups => backups(c, p).await?,
            Step::Notifications => notifications(c, p).await?,
            Step::FirstApp => first_app(c, p, &info).await?,
            Step::Welcome | Step::Done => true,
        };
        let mut body = UpdateOnboarding::default();
        if done {
            body.unskip = vec![step];
        } else {
            body.skip = vec![step];
            println!("  {}", muted("skipped"));
        }
        o = c.update_onboarding(&body).await?;
    }
    let o = c
        .update_onboarding(&UpdateOnboarding {
            status: Some(OnboardingStatus::Finished),
            current: Some(Step::Done),
            ..Default::default()
        })
        .await?;
    println!();
    println!("{}", paint("You're set.", Tone::Ok));
    let cli = brand::CLI_BIN;
    for (cmd, what) in [
        (cli.to_string(), "open the dashboard"),
        (format!("{cli} deploy <service>"), "deploy again"),
        (format!("{cli} logs <service> -f"), "follow logs"),
        (format!("{cli} setup --status"), "this checklist"),
    ] {
        println!(
            "  {}  {}",
            paint(format!("{cmd:<24}"), Tone::Accent),
            muted(what)
        );
    }
    Ok(o)
}

async fn domain(c: &Client, p: &mut Prompt<'_>) -> Result<bool> {
    println!(
        "  {}",
        muted("Serves the API and GitHub callbacks over HTTPS.")
    );
    let current = c.instance_domain().await?;
    let name = p.ask(
        "domain, e.g. deploy.example.com (empty skips):",
        current.domain.as_deref(),
    )?;
    if name.is_empty() {
        return Ok(false);
    }
    let mut d = if current.domain.as_deref() == Some(name.as_str()) {
        c.check_instance_domain().await?
    } else {
        c.set_instance_domain(&name).await?
    };
    println!();
    sources::print_domain(&d);
    loop {
        if d.verified {
            println!();
            ok(format!("{name} is verified"));
            return Ok(true);
        }
        println!();
        let answer = p.ask("enter checks again once DNS is in, s skips:", None)?;
        if answer == "s" {
            return Ok(false);
        }
        d = c.check_instance_domain().await?;
        for check in &d.checks {
            sources::check_line(check);
        }
    }
}

async fn source(c: &Client, p: &mut Prompt<'_>, info: &DaemonInfo) -> Result<bool> {
    println!(
        "  {}",
        muted("Deploy from private repositories and on every push.")
    );
    loop {
        match p.menu(&[
            ("GitHub App", "push deploys, org repos (needs the domain)"),
            ("access token", "a GitHub personal access token"),
            ("skip", ""),
        ])? {
            0 => {
                if !c.instance_domain().await?.verified {
                    println!(
                        "  {}",
                        muted("a GitHub App calls back to the instance domain; verify it first")
                    );
                    continue;
                }
                let name = p.ask(
                    "app name:",
                    Some(&format!("{}-{}", brand::PREFIX, info.hostname)),
                )?;
                let org = p.ask("organization (empty for your personal account):", None)?;
                let setup = c
                    .create_github_app(&CreateGithubApp {
                        name,
                        organization: Some(org).filter(|o| !o.is_empty()),
                        html_url: None,
                        api_url: None,
                    })
                    .await?;
                println!("  open this URL to create the app on GitHub:");
                println!("  {}", paint(&setup.setup_url, Tone::Accent));
                sources::open_browser(&setup.setup_url);
                let s = sources::wait_for_install(c, &setup.source).await?;
                ok(format!("source {} is ready", s.name));
                return Ok(true);
            }
            1 => {
                let token = p.secret("token (repo scope):")?;
                if token.is_empty() {
                    continue;
                }
                let s = c
                    .connect_github_token(&ConnectGithubToken {
                        name: unique("github", c.sources().await?.iter().map(|s| &s.name)),
                        token,
                        html_url: None,
                        api_url: None,
                    })
                    .await?;
                ok(format!("source {} is {}", s.name, s.status.as_str()));
                return Ok(s.status == GitSourceStatus::Ready);
            }
            _ => return Ok(false),
        }
    }
}

async fn backups(c: &Client, p: &mut Prompt<'_>) -> Result<bool> {
    println!(
        "  {}",
        muted("A lost server is one restore away: config, secrets, projects and the store.")
    );
    let endpoint = p.ask(
        "S3 endpoint, e.g. https://s3.eu-central-1.amazonaws.com (empty skips):",
        None,
    )?;
    if endpoint.is_empty() {
        return Ok(false);
    }
    let region = p.ask("region:", Some("us-east-1"))?;
    let bucket = loop {
        let b = p.ask("bucket:", None)?;
        if !b.is_empty() {
            break b;
        }
    };
    let path_style = p.yes("path-style addressing (MinIO and most self-hosted stores)?")?;
    let access_key_id = p.ask("access key id:", None)?;
    let secret_access_key = p.secret("secret access key:")?;
    let secrets = c.secrets().await?;
    let secret = c
        .create_secret(&CreateSecret {
            name: unique(&format!("{bucket}-s3"), secrets.iter().map(|s| &s.name)),
            value: SecretValue::S3Credentials {
                access_key_id,
                secret_access_key,
            },
        })
        .await?;
    let storages = c.storages().await?;
    let storage = c
        .create_storage(&CreateStorage {
            name: unique(&bucket, storages.iter().map(|s| &s.name)),
            endpoint,
            region,
            bucket,
            path_prefix: String::new(),
            path_style,
            credentials_secret_id: secret.id,
        })
        .await?;
    let test = c.test_storage(&storage.id).await?;
    if !test.ok {
        println!(
            "  {} {}",
            paint("✕ the bucket test failed:", Tone::Err),
            test.error.unwrap_or_default()
        );
        output::note(format!(
            "  fix it with \"{} storage …\", then run setup again",
            brand::CLI_BIN
        ));
        return Ok(false);
    }
    ok(format!("{} works", storage.name));
    println!(
        "  {}",
        muted("Backups are encrypted with a passphrase. Restores need it; keep it safe.")
    );
    let passphrase = loop {
        let pass = p.secret(&format!(
            "passphrase (at least {MIN_PASSPHRASE_LEN} characters):"
        ))?;
        if pass.chars().count() >= MIN_PASSPHRASE_LEN {
            break pass;
        }
        println!("  {}", muted("too short"));
    };
    let schedule = c
        .set_instance_backup_schedule(&UpdateInstanceBackupSchedule {
            enabled: true,
            hour_utc: 3,
            keep: 7,
            storage_id: Some(storage.id),
            passphrase: Some(passphrase),
        })
        .await?;
    ok(format!(
        "daily instance backups at {:02}:00 UTC, keeping {}",
        schedule.hour_utc, schedule.keep
    ));
    Ok(true)
}

async fn notifications(c: &Client, p: &mut Prompt<'_>) -> Result<bool> {
    println!(
        "  {}",
        muted("Essentials: failed deploys, crashes, failed backups, full disks, bad certificates.")
    );
    let kinds = [
        ("Discord", "webhook URL"),
        ("Slack", "incoming webhook"),
        ("Teams", "Workflows webhook"),
        ("Google Chat", "space webhook"),
        ("Mattermost", "incoming webhook"),
        ("ntfy", "topic"),
        ("webhook", "JSON POST to any URL"),
        ("skip", ""),
    ];
    let pick = p.menu(&kinds)?;
    let config = match pick {
        5 => {
            let server = p.ask("ntfy server:", Some("https://ntfy.sh"))?;
            let topic = p.ask("topic:", None)?;
            if topic.is_empty() {
                return Ok(false);
            }
            ChannelConfig::Ntfy {
                server,
                topic,
                token: None,
            }
        }
        7 => return Ok(false),
        _ => {
            let url = p.secret("URL:")?;
            if url.is_empty() {
                return Ok(false);
            }
            match pick {
                0 => ChannelConfig::Discord { webhook_url: url },
                1 => ChannelConfig::Slack { webhook_url: url },
                2 => ChannelConfig::Teams { webhook_url: url },
                3 => ChannelConfig::GoogleChat { webhook_url: url },
                4 => ChannelConfig::Mattermost { webhook_url: url },
                _ => ChannelConfig::Webhook {
                    url,
                    secret: None,
                    headers: Vec::new(),
                },
            }
        }
    };
    let channels = c.notify_channels().await?;
    let channel = c
        .create_notify_channel(&CreateNotifyChannel {
            name: unique(
                &kinds[pick].0.to_lowercase().replace(' ', "-"),
                channels.iter().map(|ch| &ch.name),
            ),
            config,
            enabled: true,
        })
        .await?;
    let essentials = preset("essentials").context("the essentials preset is missing")?;
    c.create_notify_rule(&CreateNotifyRule {
        channel_id: channel.id.clone(),
        scope: RuleScope::Instance,
        preset: Some(essentials.id.to_string()),
        events: essentials.events(),
        enabled: true,
        dedupe_secs: None,
        max_per_hour: None,
    })
    .await?;
    ok(format!(
        "{} gets the {} events",
        channel.name, essentials.label
    ));
    let delivery = c.test_notify_channel(&channel.id).await?;
    match delivery.error {
        None => ok(format!("test message {}", delivery.status.as_str())),
        Some(e) => println!("  {} {e}", paint("✕ the test message failed:", Tone::Err)),
    }
    Ok(true)
}

async fn first_app(c: &Client, p: &mut Prompt<'_>, info: &DaemonInfo) -> Result<bool> {
    let ready: Vec<GitSource> = c
        .sources()
        .await?
        .into_iter()
        .filter(|s| s.status == GitSourceStatus::Ready)
        .collect();
    let mut options = vec![("sample app", "traefik/whoami, a tiny hello-world page")];
    if !ready.is_empty() {
        options.push(("from GitHub", "pick a repository"));
    }
    options.push(("skip", ""));
    let pick = p.menu(&options)?;
    if pick == options.len() - 1 {
        return Ok(false);
    }
    let servers = c.servers().await?;
    let server_id = if servers.len() > 1 {
        let labels: Vec<(String, String)> = servers
            .iter()
            .map(|s| (s.name.clone(), format!("{:?}", s.kind).to_lowercase()))
            .collect();
        let refs: Vec<(&str, &str)> = labels
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        println!("  server");
        Some(servers[p.menu(&refs)?].id.clone())
    } else {
        None
    };
    let (project_name, name, source, port) = if pick == 0 {
        (
            onboarding::SAMPLE_PROJECT.to_string(),
            onboarding::SAMPLE_SERVICE.to_string(),
            Source::Image {
                image: onboarding::SAMPLE_IMAGE.to_string(),
            },
            onboarding::SAMPLE_PORT,
        )
    } else {
        let git = if ready.len() > 1 {
            let labels: Vec<(String, String)> = ready
                .iter()
                .map(|s| (s.name.clone(), s.owner.clone().unwrap_or_default()))
                .collect();
            let refs: Vec<(&str, &str)> = labels
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            &ready[p.menu(&refs)?]
        } else {
            &ready[0]
        };
        let repo = loop {
            let r = p.ask("repository (owner/name):", None)?;
            if r.contains('/') {
                break r;
            }
        };
        let branches = c.source_branches(&git.id, &repo).await?;
        let default = branches.first().map(|b| b.name.clone());
        let branch = p.ask("branch:", default.as_deref())?;
        let detected = c.detect_build(&git.id, &repo, &branch).await?;
        println!(
            "  {} {}",
            muted("builds with"),
            match &detected.file {
                Some(f) => format!("{} ({f})", detected.method.as_str()),
                None => detected.method.as_str().to_string(),
            }
        );
        let port: u16 = loop {
            if let Ok(port) = p.ask("port the app listens on:", Some("3000"))?.parse() {
                break port;
            }
        };
        let name = repo.rsplit('/').next().unwrap_or("app").to_string();
        (
            name.clone(),
            name,
            Source::Git {
                repo,
                branch,
                dockerfile: detected
                    .file
                    .clone()
                    .filter(|_| detected.method == BuildMethod::Dockerfile)
                    .unwrap_or_else(|| "Dockerfile".into()),
                context: ".".into(),
                source_id: Some(git.id.clone()),
                auto_deploy: true,
                build: Build {
                    method: detected.method,
                    compose_file: detected
                        .file
                        .clone()
                        .filter(|_| detected.method == BuildMethod::Compose),
                    target: None,
                },
            },
            port,
        )
    };
    let projects = c.projects().await?;
    let project = c
        .create_project(&CreateProject {
            name: unique(&project_name, projects.iter().map(|p| &p.name)),
            description: None,
        })
        .await?;
    let tree = c.tree().await?;
    let environment = tree
        .projects
        .iter()
        .find(|n| n.project.id == project.id)
        .and_then(|n| n.environments.first())
        .context("the new project has no environment")?
        .environment
        .clone();
    let domain = info
        .public_ip
        .as_deref()
        .and_then(|ip| onboarding::sslip_domain(&name, ip));
    let detail = c
        .create_service(&CreateService {
            environment_id: environment.id,
            server_id,
            name,
            source,
            port: Some(port),
            command: None,
            restart: RestartPolicy::default(),
            health_path: None,
            env: Vec::new(),
            domains: domain.iter().cloned().collect(),
            volumes: Vec::new(),
            deploy: true,
        })
        .await?;
    ok(format!(
        "created {}/{}/{}",
        project.name, environment.name, detail.service.name
    ));
    let deployment = c
        .deployments(&detail.service.id, 1)
        .await?
        .into_iter()
        .next()
        .context("the deploy did not start")?;
    follow_deployment(c, deployment).await?;
    match domain {
        Some(d) => println!(
            "  {} {}",
            muted("live at"),
            paint(format!("https://{d}"), Tone::Info)
        ),
        None => output::note(format!(
            "  add a domain with \"{} domain add {} <host>\"",
            brand::CLI_BIN,
            detail.service.name
        )),
    }
    Ok(true)
}

/// `base`, or `base-2`, `base-3`… when taken.
fn unique<'a>(base: &str, taken: impl Iterator<Item = &'a String>) -> String {
    let taken: Vec<&String> = taken.collect();
    (1..)
        .map(|n| {
            if n == 1 {
                base.to_string()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|name| !taken.contains(&name))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use chrono::Utc;
    use common::notify::{ChannelKind, DeliveryStatus, NotifyChannel, NotifyDelivery, NotifyRule};
    use common::onboarding::StepStatus;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    /// A daemon that answers the routes setup uses, and records every write.
    #[derive(Default)]
    struct Mock {
        writes: Vec<(String, String, Value)>,
        onboarding: Option<Onboarding>,
        domain_checks: usize,
        sources: Vec<GitSource>,
    }

    type Shared = Arc<Mutex<Mock>>;

    fn ok(v: impl serde::Serialize) -> Response {
        axum::Json(serde_json::to_value(v).unwrap()).into_response()
    }

    fn server() -> Server {
        Server {
            id: "srv".into(),
            name: "local".into(),
            kind: ServerKind::Local,
            host: None,
            user: None,
            port: None,
            ssh_key_secret_id: None,
            status: ServerStatus::Online,
            proxy: ProxyStatus::Running,
            docker_version: None,
            last_checked_at: None,
            created_at: Utc::now(),
        }
    }

    fn project() -> Project {
        Project {
            id: "p1".into(),
            name: "hello".into(),
            description: None,
            created_at: Utc::now(),
        }
    }

    fn environment() -> Environment {
        Environment {
            id: "e1".into(),
            project_id: "p1".into(),
            name: "production".into(),
            created_at: Utc::now(),
        }
    }

    fn service(body: &CreateService) -> Service {
        Service {
            id: "s1".into(),
            environment_id: body.environment_id.clone(),
            server_id: "srv".into(),
            name: body.name.clone(),
            source: body.source.clone(),
            port: body.port,
            command: None,
            restart: RestartPolicy::default(),
            health_path: None,
            created_at: Utc::now(),
            state: ServiceState::New,
            primary_domain: body.domains.first().cloned(),
            last_deployed_at: None,
            live_deployment_id: None,
        }
    }

    fn deployment(status: DeploymentStatus) -> Deployment {
        Deployment {
            id: "d1".into(),
            service_id: "s1".into(),
            number: 1,
            status,
            trigger: "api".into(),
            commit: None,
            commit_message: None,
            image: Some(onboarding::SAMPLE_IMAGE.into()),
            container_id: None,
            steps: Vec::new(),
            error: None,
            created_at: Utc::now(),
            finished_at: Some(Utc::now()),
        }
    }

    fn domain(verified: bool) -> InstanceDomain {
        InstanceDomain {
            domain: Some("deploy.example.com".into()),
            url: verified.then(|| "https://deploy.example.com".into()),
            verified,
            public_ipv4: Some("5.75.12.4".into()),
            records: vec![DnsRecord {
                kind: "A".into(),
                name: "deploy.example.com".into(),
                value: "5.75.12.4".into(),
            }],
            checks: vec![DomainCheck {
                kind: DomainCheckKind::Dns,
                status: if verified {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                detail: "deploy.example.com".into(),
                hint: None,
            }],
            ..Default::default()
        }
    }

    fn source() -> GitSource {
        GitSource {
            id: "g1".into(),
            name: "github".into(),
            kind: GitSourceKind::GithubToken,
            html_url: "https://github.com".into(),
            api_url: "https://api.github.com".into(),
            owner: Some("acme".into()),
            owner_kind: None,
            app_id: None,
            app_slug: None,
            installation_id: None,
            status: GitSourceStatus::Ready,
            status_detail: None,
            install_url: None,
            webhook_url: None,
            secret_id: None,
            last_checked_at: None,
            created_at: Utc::now(),
            used_by: Vec::new(),
        }
    }

    fn channel(body: &CreateNotifyChannel) -> NotifyChannel {
        NotifyChannel {
            id: "ch1".into(),
            name: body.name.clone(),
            kind: ChannelKind::Discord,
            enabled: true,
            config: body.config.redacted(),
            target: "discord.com".into(),
            created_at: Utc::now(),
            last_test: None,
        }
    }

    fn apply(o: &mut Onboarding, body: &UpdateOnboarding) {
        let set = |o: &mut Onboarding, step: Step, state: StepState| {
            for s in &mut o.steps {
                if s.step == step {
                    s.state = state;
                }
            }
        };
        for step in &body.skip {
            set(o, *step, StepState::Skipped);
        }
        for step in &body.unskip {
            set(o, *step, StepState::Done);
        }
        if let Some(c) = body.current {
            o.current = c;
        }
        if let Some(s) = body.status {
            o.status = s;
        }
    }

    fn parse<T: serde::de::DeserializeOwned>(b: &Value) -> T {
        serde_json::from_value(b.clone()).unwrap()
    }

    async fn handle(State(mock): State<Shared>, method: Method, uri: Uri, body: Bytes) -> Response {
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let path = uri.path().trim_start_matches(API_PREFIX).to_string();
        let mut m = mock.lock().unwrap();
        if method != Method::GET {
            m.writes
                .push((method.to_string(), path.clone(), body.clone()));
        }
        match (method.as_str(), path.as_str()) {
            ("GET", "/info") => ok(json!({
                "version": "0.5.0", "hostname": "prod-eu-1", "public_ip": "5.75.12.4",
                "cpu_percent": 0.0, "mem_used": 0, "mem_total": 0, "disk_used": 0,
                "disk_total": 0, "cpu_history": [], "proxy": "running", "ssh_public_key": null
            })),
            ("GET", "/onboarding") => ok(m.onboarding.clone()),
            ("PATCH", "/onboarding") => {
                let mut o = m.onboarding.clone().unwrap();
                apply(&mut o, &parse(&body));
                m.onboarding = Some(o.clone());
                ok(o)
            }
            ("GET", "/settings/domain") => ok(InstanceDomain::default()),
            ("PUT", "/settings/domain") => ok(domain(false)),
            ("POST", "/settings/domain/check") => {
                m.domain_checks += 1;
                ok(domain(true))
            }
            ("GET", "/sources") => ok(&m.sources),
            ("POST", "/sources/github/token") => {
                m.sources.push(source());
                ok(source())
            }
            ("GET", "/notify/channels") | ("GET", "/projects") => ok(json!([])),
            ("POST", "/notify/channels") => ok(channel(&parse(&body))),
            ("POST", "/notify/rules") => {
                let b: CreateNotifyRule = parse(&body);
                ok(NotifyRule {
                    id: "r1".into(),
                    channel_id: b.channel_id,
                    scope: b.scope,
                    scope_label: "instance".into(),
                    preset: b.preset,
                    events: b.events,
                    enabled: true,
                    dedupe_secs: 0,
                    max_per_hour: 0,
                    created_at: Utc::now(),
                })
            }
            ("POST", "/notify/channels/ch1/test") => ok(NotifyDelivery {
                id: 1,
                at: Utc::now(),
                channel_id: "ch1".into(),
                channel_name: "discord".into(),
                rule_id: None,
                event: None,
                subject: "instance".into(),
                title: "test".into(),
                status: DeliveryStatus::Sent,
                attempts: 1,
                error: None,
                latency_ms: Some(12),
            }),
            ("GET", "/servers") => ok(vec![server()]),
            ("POST", "/projects") => ok(project()),
            ("GET", "/tree") => ok(Tree {
                projects: vec![ProjectNode {
                    project: project(),
                    environments: vec![EnvironmentNode {
                        environment: environment(),
                        services: Vec::new(),
                    }],
                }],
            }),
            ("POST", "/services") => {
                let b: CreateService = parse(&body);
                ok(ServiceDetail {
                    service: service(&b),
                    project: project(),
                    environment: environment(),
                    server: server(),
                    env: Vec::new(),
                    domains: Vec::new(),
                    volumes: Vec::new(),
                    container: None,
                    database: None,
                    compose: Vec::new(),
                })
            }
            ("GET", "/services/s1/deployments") => ok(vec![deployment(DeploymentStatus::Building)]),
            ("GET", "/deployments/d1/logs") => {
                let line = LogLine {
                    ts: None,
                    stream: LogStream::Stdout,
                    line: "pulling traefik/whoami".into(),
                };
                format!("{}\n", serde_json::to_string(&line).unwrap()).into_response()
            }
            ("GET", "/deployments/d1") => ok(deployment(DeploymentStatus::Live)),
            _ => (
                StatusCode::NOT_FOUND,
                format!("mock has no {method} {path}"),
            )
                .into_response(),
        }
    }

    async fn daemon() -> (Client, Shared) {
        let mock = Shared::default();
        mock.lock().unwrap().onboarding = Some(Onboarding {
            status: OnboardingStatus::Pending,
            current: Step::Welcome,
            steps: Step::TASKS
                .into_iter()
                .map(|step| StepStatus {
                    step,
                    state: StepState::Todo,
                })
                .collect(),
            checklist_dismissed: false,
            automatic: false,
        });
        let app = Router::new().fallback(handle).with_state(mock.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (Client::new(&url, "token").unwrap(), mock)
    }

    async fn walk(c: &Client, answers: &[&str]) -> Onboarding {
        let text = answers.iter().map(|a| format!("{a}\n")).collect::<String>();
        let mut input = std::io::Cursor::new(text.into_bytes());
        run(c, &mut Prompt::new(&mut input, false)).await.unwrap()
    }

    fn writes(mock: &Shared, path: &str) -> Vec<Value> {
        let m = mock.lock().unwrap();
        m.writes
            .iter()
            .filter(|(_, p, _)| p == path)
            .map(|(_, _, b)| b.clone())
            .collect()
    }

    #[tokio::test]
    async fn skipping_everything_but_the_sample_app_deploys_it() {
        let (c, mock) = daemon().await;
        let o = walk(&c, &["", "3", "", "8", ""]).await;

        assert_eq!(o.status, OnboardingStatus::Finished);
        assert_eq!(o.current, Step::Done);
        for step in [
            Step::Domain,
            Step::Source,
            Step::Backups,
            Step::Notifications,
        ] {
            assert_eq!(o.state(step), StepState::Skipped, "{step:?}");
        }
        assert_eq!(o.state(Step::FirstApp), StepState::Done);

        assert_eq!(
            writes(&mock, "/projects"),
            [json!({"name": "hello", "description": null})]
        );
        let services = writes(&mock, "/services");
        assert_eq!(services.len(), 1);
        let s: CreateService = serde_json::from_value(services[0].clone()).unwrap();
        assert_eq!(s.environment_id, "e1");
        assert_eq!(s.name, "whoami");
        assert_eq!(s.port, Some(80));
        assert!(s.deploy);
        assert!(s.server_id.is_none(), "one server needs no pick");
        assert_eq!(s.domains, ["whoami-5-75-12-4.sslip.io"]);
        assert!(matches!(s.source, Source::Image { image } if image == "traefik/whoami"));
        assert!(writes(&mock, "/settings/domain").is_empty());
    }

    #[tokio::test]
    async fn domain_source_and_notifications_are_set_up() {
        let (c, mock) = daemon().await;
        let o = walk(
            &c,
            &[
                "deploy.example.com",
                "",
                "2",
                "ghp_secret",
                "",
                "1",
                "https://discord.com/api/webhooks/1/x",
                "3",
            ],
        )
        .await;

        assert_eq!(o.state(Step::Domain), StepState::Done);
        assert_eq!(o.state(Step::Source), StepState::Done);
        assert_eq!(o.state(Step::Backups), StepState::Skipped);
        assert_eq!(o.state(Step::Notifications), StepState::Done);
        assert_eq!(o.state(Step::FirstApp), StepState::Skipped);
        assert_eq!(o.status, OnboardingStatus::Finished);

        assert_eq!(
            writes(&mock, "/settings/domain"),
            [json!({"domain": "deploy.example.com"})]
        );
        assert_eq!(
            mock.lock().unwrap().domain_checks,
            1,
            "checked again on enter"
        );
        let token = writes(&mock, "/sources/github/token");
        assert_eq!(token[0]["token"], "ghp_secret");
        assert_eq!(token[0]["name"], "github");

        let channel = &writes(&mock, "/notify/channels")[0];
        assert_eq!(channel["name"], "discord");
        assert_eq!(channel["config"]["kind"], "discord");
        let rule: CreateNotifyRule =
            serde_json::from_value(writes(&mock, "/notify/rules")[0].clone()).unwrap();
        assert_eq!(rule.channel_id, "ch1");
        assert_eq!(rule.preset.as_deref(), Some("essentials"));
        assert_eq!(rule.events, preset("essentials").unwrap().events());
        assert_eq!(writes(&mock, "/notify/channels/ch1/test").len(), 1);
        assert!(writes(&mock, "/services").is_empty());
    }

    #[test]
    fn names_get_a_suffix_when_taken() {
        let taken = ["hello".to_string(), "hello-2".to_string()];
        assert_eq!(unique("hello", taken.iter()), "hello-3");
        assert_eq!(unique("whoami", taken.iter()), "whoami");
    }
}
