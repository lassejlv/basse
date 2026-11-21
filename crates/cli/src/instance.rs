//! Commands about the instance itself: settings, the API token, the proxy, pruning, updates,
//! and forgetting saved contexts.

use crate::output::{self, Tone, muted, paint};
use anyhow::{Context as _, Result, bail};
use clap::{Subcommand, ValueEnum};
use client::{Client, Config};
use common::api::{UpdateRequest, UpdateSettings};
use common::*;
use std::time::Duration;

#[derive(Subcommand)]
pub enum SettingsCmd {
    /// Change a setting.
    #[command(subcommand)]
    Set(SetCmd),
}

#[derive(Subcommand)]
pub enum SetCmd {
    /// Address shown in login and DNS hints; "auto" detects it again.
    PublicIp { value: String },
    /// Install and restart the proxy on this server automatically.
    ManageProxy { value: Switch },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Switch {
    On,
    Off,
}

fn kv(key: &str, value: impl AsRef<str>) {
    println!("{}{}", muted(format!("{key:<14}")), value.as_ref());
}

pub async fn settings(c: &Client, cmd: Option<SettingsCmd>, json: bool) -> Result<()> {
    if let Some(SettingsCmd::Set(set)) = cmd {
        let body = match &set {
            SetCmd::PublicIp { value } => UpdateSettings {
                public_ip_override: Some(value.clone()),
                ..Default::default()
            },
            SetCmd::ManageProxy { value } => UpdateSettings {
                manage_proxy: Some(matches!(value, Switch::On)),
                ..Default::default()
            },
        };
        let s = c.update_settings(&body).await?;
        if json {
            return output::json(&s);
        }
        match set {
            SetCmd::PublicIp { .. } => output::ok(match &s.public_ip_override {
                Some(ip) => format!("public IP set to {ip}"),
                None => format!(
                    "public IP is detected again ({})",
                    s.public_ip.as_deref().unwrap_or("unknown")
                ),
            }),
            SetCmd::ManageProxy { .. } => output::ok(format!(
                "proxy auto-manage is {}",
                if s.manage_proxy { "on" } else { "off" }
            )),
        }
        return Ok(());
    }
    let (s, version) = tokio::join!(c.settings(), c.version_check());
    let s = s?;
    if json {
        return output::json(&serde_json::json!({ "settings": s, "release": version.ok() }));
    }
    let update = match version.ok().filter(|v| v.update_available) {
        Some(v) => paint(
            format!("  update available {}", v.latest.unwrap_or_default()),
            Tone::Accent,
        ),
        None => String::new(),
    };
    kv("version", format!("{}{update}", s.version));
    kv("hostname", &s.hostname);
    kv(
        "public ip",
        format!(
            "{}  {}",
            s.public_ip.as_deref().unwrap_or("unknown"),
            muted(if s.public_ip_override.is_some() {
                "set by hand"
            } else {
                "auto-detected"
            })
        ),
    );
    kv("listen", &s.listen);
    kv(
        "data dir",
        format!("{}  {}", s.data_dir, muted(output::bytes(s.data_dir_size))),
    );
    kv("docker", s.docker_version.as_deref().unwrap_or("unknown"));
    kv(
        "proxy",
        format!(
            "{}  {}",
            match s.proxy {
                ProxyStatus::Running => paint("running", Tone::Ok),
                ProxyStatus::Stopped => paint("stopped", Tone::Err),
                ProxyStatus::Missing => paint("not set up", Tone::Accent),
            },
            muted(&s.proxy_image)
        ),
    );
    kv(
        "auto-manage",
        if s.manage_proxy {
            paint("on", Tone::Ok)
        } else {
            muted("off")
        },
    );
    if let Some(r) = s.reclaimable {
        kv(
            "reclaimable",
            format!(
                "{}  {}",
                output::bytes(r.total()),
                muted(format!(
                    "{} images · build cache {}",
                    r.images,
                    output::bytes(r.build_cache_bytes)
                ))
            ),
        );
    }
    if let Some(key) = &s.ssh_public_key {
        kv("ssh key", key);
    }
    Ok(())
}

/// Rotates the API token and stores it in the saved contexts before anything else talks to the
/// daemon, since the old token stops working at once.
pub async fn rotate_token(c: &Client, context: &str, json: bool) -> Result<()> {
    let rotated = c.rotate_token().await?;
    let saved = if context == "env" {
        0
    } else {
        let mut config = Config::load()?;
        let n = config.replace_token(c.url(), c.token(), &rotated.token);
        config.save()?;
        n
    };
    if json {
        return output::json(&rotated);
    }
    output::ok("issued a new API token; the old one no longer works");
    if saved > 0 {
        output::note(format!("saved to context {context}"));
    } else {
        output::note(format!(
            "update {} wherever the old token is set",
            brand::env_var("TOKEN")
        ));
    }
    println!("{}", rotated.token);
    output::note("shown once: copy it now to log in from another machine");
    Ok(())
}

pub async fn restart_proxy(c: &Client, json: bool) -> Result<()> {
    let server = c.restart_proxy().await?;
    if json {
        return output::json(&server);
    }
    output::ok(format!("proxy reinstalled on {}", server.name));
    Ok(())
}

pub async fn prune(c: &Client, json: bool) -> Result<()> {
    let result = c.prune().await?;
    if json {
        return output::json(&result);
    }
    output::ok(format!(
        "removed {} images and the build cache, freed {}",
        result.removed_images.len(),
        output::bytes(result.reclaimed_bytes)
    ));
    for image in &result.removed_images {
        output::note(format!("  {image}"));
    }
    Ok(())
}

/// The sides `update` touches: this CLI binary, the current context's daemon, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Steps {
    pub cli: bool,
    pub server: bool,
}

impl Steps {
    pub const CLI: Steps = Steps {
        cli: true,
        server: false,
    };

    /// `--cli` and `--server` pick their side; with neither, both are updated.
    pub fn from_flags(cli: bool, server: bool) -> Self {
        if cli || server {
            Self { cli, server }
        } else {
            Self {
                cli: true,
                server: true,
            }
        }
    }
}

/// Updates this CLI, then the daemon of the current context, to the same release. A side that
/// already runs it is up to date, not an error. When the CLI cannot be replaced the daemon is
/// still updated and the CLI's error is returned at the end.
pub async fn update(
    config: &Config,
    context: Option<&str>,
    steps: Steps,
    tag: Option<String>,
    force: bool,
    json: bool,
) -> Result<()> {
    let mut report = serde_json::Map::new();
    let mut target = tag.as_deref().map(release::tag);
    let mut cli_error = None;
    if steps.cli {
        match update_cli(tag.as_deref(), force, json).await {
            Ok((tag, result)) => {
                target = Some(tag);
                if !json {
                    output::ok(match result.updated {
                        true => format!("{} {} → {}", brand::CLI_BIN, result.from, result.to),
                        false => format!("{} {} is up to date", brand::CLI_BIN, result.from),
                    });
                }
                report.insert("cli".into(), serde_json::to_value(result)?);
            }
            Err(e) if steps.server => {
                report.insert(
                    "cli".into(),
                    serde_json::json!({ "error": format!("{e:#}") }),
                );
                cli_error = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    if steps.server {
        match config.resolve(context) {
            Ok((name, ctx)) => {
                let c = Client::from_context(&ctx)?;
                let daemon = match c.info().await {
                    Ok(info) => {
                        update_daemon(&c, &name, info, target.as_deref(), force, json).await
                    }
                    Err(e) if steps.cli => {
                        if !json {
                            output::warn(format!(
                                "{} on {name} was not updated: {e:#}",
                                brand::DAEMON_BIN
                            ));
                        }
                        report.insert(
                            "server".into(),
                            serde_json::json!({ "warning": format!("{e:#}") }),
                        );
                        return finish(report, cli_error, json);
                    }
                    Err(e) => Err(e),
                };
                match daemon {
                    Ok(result) => {
                        if !json {
                            output::ok(match result.updated {
                                true => format!(
                                    "{} {} → {} on {name}",
                                    brand::DAEMON_BIN,
                                    result.from,
                                    result.to
                                ),
                                false => format!(
                                    "{} {} on {name} is up to date",
                                    brand::DAEMON_BIN,
                                    result.from
                                ),
                            });
                        }
                        report.insert("server".into(), serde_json::to_value(result)?);
                    }
                    Err(e) => return Err(also(cli_error, e)),
                }
            }
            Err(_) if steps.cli && context.is_none() => {}
            Err(e) => return Err(also(cli_error, e)),
        }
    }
    finish(report, cli_error, json)
}

/// The daemon's error, after the CLI's when that failed too, so neither is lost.
fn also(cli_error: Option<anyhow::Error>, e: anyhow::Error) -> anyhow::Error {
    match cli_error {
        Some(cli) => anyhow::anyhow!(
            "{cli:#}
{} {e:#}",
            paint("error:", Tone::Err)
        ),
        None => e,
    }
}

fn finish(
    report: serde_json::Map<String, serde_json::Value>,
    cli_error: Option<anyhow::Error>,
    json: bool,
) -> Result<()> {
    if json {
        output::json(&report)?;
    }
    match cli_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Replaces this binary with `tag`, or the latest release. Returns the tag it resolved, so the
/// daemon can be moved to the same release.
async fn update_cli(
    tag: Option<&str>,
    force: bool,
    json: bool,
) -> Result<(String, common::UpdateResult)> {
    let releases = release::Releases::new()?;
    let tag = releases.resolve(tag).await?;
    if !json && (force || release::is_newer(&tag, env!("CARGO_PKG_VERSION"))) {
        output::note(format!("downloading {} {tag}…", brand::CLI_BIN,));
    }
    let result = release::update_cli(&releases, &tag, force).await?;
    Ok((tag, result))
}

/// Updates the daemon behind `c` and waits until the new version answers. A daemon that
/// already runs `tag` is up to date without a request, which also covers daemons from before
/// `updated: false`, which refused instead.
async fn update_daemon(
    c: &Client,
    context: &str,
    info: DaemonInfo,
    tag: Option<&str>,
    force: bool,
    json: bool,
) -> Result<common::UpdateResult> {
    if !force && tag.is_some_and(|tag| !release::is_newer(tag, &info.version)) {
        return Ok(common::UpdateResult {
            from: info.version.clone(),
            to: info.version,
            updated: false,
        });
    }
    if !json {
        output::note(format!("downloading the release on {context}…"));
    }
    let result = c
        .update(&UpdateRequest {
            version: tag.map(str::to_string),
            force,
        })
        .await?;
    if !result.updated {
        return Ok(result);
    }
    if !json {
        output::note(format!(
            "installed {} {}, restarting…",
            brand::DAEMON_BIN,
            result.to
        ));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    tokio::time::sleep(Duration::from_secs(1)).await;
    loop {
        match c.info().await {
            Ok(info) if info.version == result.to => break,
            _ if tokio::time::Instant::now() > deadline => bail!(
                "{} did not come back as {} within 90 seconds; check its logs",
                brand::DAEMON_BIN,
                result.to
            ),
            _ => tokio::time::sleep(Duration::from_secs(1)).await,
        }
    }
    Ok(result)
}

/// Forgets saved contexts. The daemon never hears about it: the token stays valid until rotated.
pub fn logout(config: &mut Config, name: Option<String>, all: bool) -> Result<()> {
    let login = format!("{} login <url> --token <token>", brand::CLI_BIN);
    if all {
        let n = config.contexts.len();
        config.contexts.clear();
        config.current = None;
        config.save()?;
        output::ok(format!(
            "forgot {n} context{}",
            if n == 1 { "" } else { "s" }
        ));
        output::note(format!("log in again with {login}"));
        return Ok(());
    }
    let name = name
        .or_else(|| config.current.clone())
        .context("not logged in to any daemon")?;
    let current = config.forget(&name)?;
    config.save()?;
    output::ok(format!("forgot {name} on this machine"));
    output::note(format!(
        "its token still works until you run `{} token rotate` against that daemon",
        brand::CLI_BIN
    ));
    match current {
        Some(now) => output::note(format!("now using {now}")),
        None => output::note(format!("log in again with {login}")),
    }
    Ok(())
}
