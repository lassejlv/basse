//! `notify`: notification channels, the rules that send events to them, and the delivery log.

use crate::output::{self, Cell, Tone, muted, paint};
use crate::{confirm, hint, resolve};
use anyhow::{Context as _, Result, bail};
use clap::Subcommand;
use client::Client;
use common::api::{CreateNotifyChannel, CreateNotifyRule, UpdateNotifyChannel};
use common::notify::*;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum NotifyCmd {
    /// Every event a rule can send, grouped by category.
    Events,
    /// The presets "rule add --preset" starts from.
    Presets,
    /// Destinations: email, Discord, Slack, Telegram, Teams, ntfy, Gotify, Pushover, webhooks.
    #[command(subcommand, alias = "channels")]
    Channel(ChannelCmd),
    /// Which events go to which channel.
    #[command(subcommand, alias = "rules")]
    Rule(RuleCmd),
    /// Recent delivery attempts, newest first.
    Log {
        #[arg(long, short = 'n', default_value_t = 30)]
        limit: usize,
        /// Only this channel's deliveries.
        #[arg(long)]
        channel: Option<String>,
        /// Only this rule's deliveries.
        #[arg(long)]
        rule: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ChannelCmd {
    List,
    /// Add a channel and send it a test message. The credential (password, API key, token or
    /// webhook URL) is read from --secret-file, or from stdin, never from arguments.
    Add(Box<ChannelArgs>),
    /// Send a test message now.
    Test {
        channel: String,
    },
    /// Start sending to a channel again.
    Enable {
        channel: String,
    },
    /// Stop sending to a channel without removing it or its rules.
    Disable {
        channel: String,
    },
    /// Remove a channel and its rules.
    #[command(alias = "remove")]
    Rm {
        channel: String,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(clap::Args)]
pub struct ChannelArgs {
    name: String,
    /// smtp, resend, cloudflare, discord, slack, mattermost, google-chat, teams, telegram, ntfy,
    /// gotify, pushover or webhook.
    #[arg(long)]
    kind: ChannelKind,
    /// File holding the credential; "-" reads stdin. Required credentials default to stdin.
    #[arg(long)]
    secret_file: Option<PathBuf>,
    /// Email sender, e.g. "Ops <ops@example.com>".
    #[arg(long)]
    from: Option<String>,
    /// Email recipient; repeat for several.
    #[arg(long)]
    to: Vec<String>,
    /// SMTP server.
    #[arg(long)]
    host: Option<String>,
    /// SMTP port; defaults to 587 for starttls, 465 for tls and 25 for none.
    #[arg(long)]
    port: Option<u16>,
    /// SMTP security: starttls, tls or none.
    #[arg(long, default_value = "starttls")]
    security: SmtpSecurity,
    /// SMTP user; the password is the credential.
    #[arg(long)]
    username: Option<String>,
    /// Cloudflare account id.
    #[arg(long)]
    account_id: Option<String>,
    /// Telegram chat id, e.g. -1001234567890.
    #[arg(long)]
    chat_id: Option<String>,
    /// ntfy or Gotify server.
    #[arg(long)]
    server: Option<String>,
    /// ntfy topic.
    #[arg(long)]
    topic: Option<String>,
    /// Pushover user or group key.
    #[arg(long)]
    user: Option<String>,
    /// Webhook URL of the generic webhook kind.
    #[arg(long)]
    url: Option<String>,
    /// Extra webhook header as "Name: value"; repeat for several.
    #[arg(long)]
    header: Vec<String>,
    /// Add it without sending a test message.
    #[arg(long)]
    no_test: bool,
}

#[derive(Subcommand)]
pub enum RuleCmd {
    List,
    /// Send events to a channel. Start from --preset and adjust with --event and --exclude.
    Add {
        #[arg(long)]
        channel: String,
        /// essentials, deploys, databases or everything.
        #[arg(long)]
        preset: Option<String>,
        /// Event id such as deploy.failed, or a group such as "deploy.*"; repeat for several.
        #[arg(long)]
        event: Vec<String>,
        /// Leave out an event or group the preset includes.
        #[arg(long)]
        exclude: Vec<String>,
        /// Only events of this project.
        #[arg(long, conflicts_with_all = ["environment", "service"])]
        project: Option<String>,
        /// Only events of this environment, as project/environment.
        #[arg(long, conflicts_with = "service")]
        environment: Option<String>,
        /// Only events of this service.
        #[arg(long)]
        service: Option<String>,
        /// Send the same event about the same thing at most once in this many seconds.
        #[arg(long)]
        dedupe: Option<u32>,
        /// At most this many messages an hour; 0 is unlimited.
        #[arg(long)]
        max_per_hour: Option<u32>,
    },
    #[command(alias = "remove")]
    Rm {
        rule: String,
        #[arg(long, short)]
        yes: bool,
    },
}

/// The events an argument names: an id, or a group like `deploy.*`.
fn events_matching(pattern: &str) -> Result<Vec<NotifyEvent>> {
    if let Some(group) = pattern.strip_suffix(".*") {
        let events: Vec<_> = NotifyEvent::ALL
            .iter()
            .copied()
            .filter(|e| e.id().split('.').next() == Some(group))
            .collect();
        if events.is_empty() {
            bail!("no events match '{pattern}'; see {}", hint("notify events"));
        }
        return Ok(events);
    }
    pattern
        .parse::<NotifyEvent>()
        .map(|e| vec![e])
        .map_err(|e| anyhow::anyhow!("{e}; see {}", hint("notify events")))
}

fn severity_cell(s: Severity) -> Cell {
    let tone = match s {
        Severity::Info => Tone::Info,
        Severity::Success => Tone::Ok,
        Severity::Warning => Tone::Accent,
        Severity::Critical => Tone::Err,
    };
    Cell(s.as_str().into(), tone)
}

fn status_cell(s: DeliveryStatus) -> Cell {
    let tone = match s {
        DeliveryStatus::Sent => Tone::Ok,
        DeliveryStatus::Retrying => Tone::Accent,
        DeliveryStatus::Failed => Tone::Err,
        DeliveryStatus::Suppressed => Tone::Muted,
    };
    Cell(s.as_str().into(), tone)
}

async fn find_channel(c: &Client, query: &str) -> Result<NotifyChannel> {
    c.notify_channels()
        .await?
        .into_iter()
        .find(|ch| ch.name == query || ch.id == query)
        .with_context(|| format!("no notification channel '{query}'"))
}

fn print_delivery(d: &NotifyDelivery) {
    match d.status {
        DeliveryStatus::Sent => output::ok(format!(
            "test message sent to {} in {} ms",
            d.channel_name,
            d.latency_ms.unwrap_or_default()
        )),
        _ => eprintln!(
            "{} {}: {}",
            paint("✕", Tone::Err),
            d.channel_name,
            d.error.as_deref().unwrap_or("the test failed")
        ),
    }
}

pub async fn run(c: &Client, cmd: NotifyCmd, json: bool) -> Result<()> {
    match cmd {
        NotifyCmd::Events => {
            if json {
                return output::json(&NotifyCatalog::builtin().events);
            }
            for category in EventCategory::ALL {
                println!("{}", paint(category.label(), Tone::Info));
                output::table(
                    &["  EVENT", "SEVERITY", "LABEL"],
                    category
                        .events()
                        .map(|e| {
                            vec![
                                Cell::from(format!("  {}", e.id())),
                                severity_cell(e.severity()),
                                Cell(e.label().into(), Tone::Muted),
                            ]
                        })
                        .collect(),
                );
                println!();
            }
            Ok(())
        }
        NotifyCmd::Presets => {
            if json {
                return output::json(&NotifyCatalog::builtin().presets);
            }
            output::table(
                &["PRESET", "EVENTS", "DESCRIPTION"],
                PRESETS
                    .iter()
                    .map(|p| {
                        vec![
                            Cell::from(p.id),
                            Cell(p.events().len().to_string(), Tone::Muted),
                            Cell(p.description.into(), Tone::Muted),
                        ]
                    })
                    .collect(),
            );
            output::note(format!(
                "start a rule from one with {}, then adjust it with --event and --exclude",
                hint("notify rule add --channel <channel> --preset <preset>")
            ));
            Ok(())
        }
        NotifyCmd::Channel(cmd) => channel(c, cmd, json).await,
        NotifyCmd::Rule(cmd) => rule(c, cmd, json).await,
        NotifyCmd::Log {
            limit,
            channel,
            rule,
        } => {
            let channel_id = match channel {
                Some(q) => Some(find_channel(c, &q).await?.id),
                None => None,
            };
            let list = c
                .notify_deliveries(limit, channel_id.as_deref(), rule.as_deref())
                .await?;
            if json {
                return output::json(&list);
            }
            if list.is_empty() {
                output::note("no deliveries yet");
            }
            output::table(
                &[
                    "WHEN", "CHANNEL", "EVENT", "SUBJECT", "STATUS", "TRIES", "DETAIL",
                ],
                list.iter()
                    .map(|d| {
                        let detail = match (&d.error, d.latency_ms) {
                            (Some(e), _) => Cell(e.clone(), Tone::Muted),
                            (None, Some(ms)) => Cell(format!("{ms} ms"), Tone::Muted),
                            (None, None) => Cell(String::new(), Tone::Muted),
                        };
                        vec![
                            Cell(output::ago(d.at), Tone::Muted),
                            Cell::from(d.channel_name.clone()),
                            Cell::from(d.event.map_or("test", NotifyEvent::id)),
                            Cell::from(d.subject.clone()),
                            status_cell(d.status),
                            Cell(d.attempts.to_string(), Tone::Muted),
                            detail,
                        ]
                    })
                    .collect(),
            );
            Ok(())
        }
    }
}

/// Reads the credential from --secret-file ("-" is stdin), or from stdin when it is required.
fn credential(file: Option<&PathBuf>, required: bool) -> Result<Option<String>> {
    let value = match file {
        Some(path) if path.as_os_str() == "-" => Some(crate::read_value(None)?),
        Some(path) => Some(crate::read_value(Some(path))?),
        None if required => Some(crate::read_value(None)?),
        None => None,
    };
    let value = value.map(|v| v.trim().to_string());
    if required && value.as_deref().is_none_or(str::is_empty) {
        bail!("this kind needs a credential: pass --secret-file or pipe it on stdin");
    }
    Ok(value.filter(|v| !v.is_empty()))
}

fn need(value: Option<String>, flag: &str, kind: ChannelKind) -> Result<String> {
    value.with_context(|| format!("a {} channel needs --{flag}", kind.as_str()))
}

fn config(a: ChannelArgs) -> Result<ChannelConfig> {
    let kind = a.kind;
    let secret = |required| credential(a.secret_file.as_ref(), required);
    let required = |r: Result<Option<String>>| r.map(Option::unwrap_or_default);
    let email = |provider| -> Result<ChannelConfig> {
        Ok(ChannelConfig::Email {
            provider,
            from: need(a.from.clone(), "from", kind)?,
            to: a.to.clone(),
        })
    };
    Ok(match kind {
        ChannelKind::EmailSmtp => {
            let password = secret(a.username.is_some())?;
            email(EmailProvider::Smtp {
                host: need(a.host.clone(), "host", kind)?,
                port: a.port.unwrap_or(match a.security {
                    SmtpSecurity::Starttls => 587,
                    SmtpSecurity::Tls => 465,
                    SmtpSecurity::None => 25,
                }),
                security: a.security,
                username: a.username.clone(),
                password,
            })?
        }
        ChannelKind::EmailResend => email(EmailProvider::Resend {
            api_key: required(secret(true))?,
        })?,
        ChannelKind::EmailCloudflare => email(EmailProvider::Cloudflare {
            account_id: need(a.account_id.clone(), "account-id", kind)?,
            api_token: required(secret(true))?,
        })?,
        ChannelKind::Discord => ChannelConfig::Discord {
            webhook_url: required(secret(true))?,
        },
        ChannelKind::Slack => ChannelConfig::Slack {
            webhook_url: required(secret(true))?,
        },
        ChannelKind::Mattermost => ChannelConfig::Mattermost {
            webhook_url: required(secret(true))?,
        },
        ChannelKind::GoogleChat => ChannelConfig::GoogleChat {
            webhook_url: required(secret(true))?,
        },
        ChannelKind::Teams => ChannelConfig::Teams {
            webhook_url: required(secret(true))?,
        },
        ChannelKind::Telegram => ChannelConfig::Telegram {
            chat_id: need(a.chat_id.clone(), "chat-id", kind)?,
            bot_token: required(secret(true))?,
        },
        ChannelKind::Ntfy => ChannelConfig::Ntfy {
            server: a.server.clone().unwrap_or_else(|| "https://ntfy.sh".into()),
            topic: need(a.topic.clone(), "topic", kind)?,
            token: secret(false)?,
        },
        ChannelKind::Gotify => ChannelConfig::Gotify {
            server: need(a.server.clone(), "server", kind)?,
            app_token: required(secret(true))?,
        },
        ChannelKind::Pushover => ChannelConfig::Pushover {
            user_key: need(a.user.clone(), "user", kind)?,
            app_token: required(secret(true))?,
        },
        ChannelKind::Webhook => ChannelConfig::Webhook {
            url: need(a.url.clone(), "url", kind)?,
            secret: secret(false)?,
            headers: a
                .header
                .iter()
                .map(|h| {
                    let (name, value) = h
                        .split_once(':')
                        .with_context(|| format!("header '{h}' must be \"Name: value\""))?;
                    Ok(WebhookHeader {
                        name: name.trim().into(),
                        value: value.trim().into(),
                    })
                })
                .collect::<Result<_>>()?,
        },
    })
}

async fn channel(c: &Client, cmd: ChannelCmd, json: bool) -> Result<()> {
    match cmd {
        ChannelCmd::List => {
            let list = c.notify_channels().await?;
            if json {
                return output::json(&list);
            }
            if list.is_empty() {
                output::note(format!(
                    "no channels yet; add one with {}",
                    hint("notify channel add <name> --kind discord")
                ));
            }
            output::table(
                &["CHANNEL", "KIND", "TARGET", "STATE", "LAST TEST"],
                list.iter()
                    .map(|ch| {
                        let test = match &ch.last_test {
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
                            Cell::from(ch.name.clone()),
                            Cell(ch.kind.label().into(), Tone::Muted),
                            Cell::from(ch.target.clone()),
                            if ch.enabled {
                                Cell("on".into(), Tone::Ok)
                            } else {
                                Cell("off".into(), Tone::Muted)
                            },
                            test,
                        ]
                    })
                    .collect(),
            );
            Ok(())
        }
        ChannelCmd::Add(args) => {
            let name = args.name.clone();
            let no_test = args.no_test;
            let config = config(*args)?;
            config.validate().map_err(anyhow::Error::msg)?;
            let ch = c
                .create_notify_channel(&CreateNotifyChannel {
                    name,
                    config,
                    enabled: true,
                })
                .await?;
            let test = if no_test {
                None
            } else {
                Some(c.test_notify_channel(&ch.id).await?)
            };
            if json {
                return output::json(&c.notify_channel(&ch.id).await?);
            }
            output::ok(format!(
                "added {} ({}, {})",
                ch.name,
                ch.kind.label(),
                ch.target
            ));
            if let Some(d) = &test {
                print_delivery(d);
            }
            if c.notify_rules()
                .await?
                .iter()
                .all(|r| r.channel_id != ch.id)
            {
                output::note(format!(
                    "nothing is sent until a rule says so: {}",
                    hint(&format!(
                        "notify rule add --channel {} --preset essentials",
                        ch.name
                    ))
                ));
            }
            Ok(())
        }
        ChannelCmd::Test { channel } => {
            let ch = find_channel(c, &channel).await?;
            let d = c.test_notify_channel(&ch.id).await?;
            if json {
                return output::json(&d);
            }
            print_delivery(&d);
            if d.status != DeliveryStatus::Sent {
                std::process::exit(1);
            }
            Ok(())
        }
        ChannelCmd::Enable { channel } => set_enabled(c, &channel, true, json).await,
        ChannelCmd::Disable { channel } => set_enabled(c, &channel, false, json).await,
        ChannelCmd::Rm { channel, yes } => {
            let ch = find_channel(c, &channel).await?;
            let rules = c
                .notify_rules()
                .await?
                .into_iter()
                .filter(|r| r.channel_id == ch.id)
                .count();
            let prompt = match rules {
                0 => format!("Remove channel {}?", ch.name),
                n => format!("Remove channel {} and its {n} rule(s)?", ch.name),
            };
            confirm(&prompt, yes)?;
            c.delete_notify_channel(&ch.id).await?;
            if !json {
                output::ok(format!("removed {}", ch.name));
            }
            Ok(())
        }
    }
}

async fn set_enabled(c: &Client, channel: &str, enabled: bool, json: bool) -> Result<()> {
    let ch = find_channel(c, channel).await?;
    let ch = c
        .update_notify_channel(
            &ch.id,
            &UpdateNotifyChannel {
                enabled: Some(enabled),
                ..Default::default()
            },
        )
        .await?;
    if json {
        return output::json(&ch);
    }
    output::ok(format!(
        "{} is {}",
        ch.name,
        if enabled { "on" } else { "off" }
    ));
    Ok(())
}

async fn rule(c: &Client, cmd: RuleCmd, json: bool) -> Result<()> {
    match cmd {
        RuleCmd::List => {
            let rules = c.notify_rules().await?;
            if json {
                return output::json(&rules);
            }
            if rules.is_empty() {
                output::note(format!(
                    "no rules yet; add one with {}",
                    hint("notify rule add --channel <channel> --preset essentials")
                ));
            }
            let channels = c.notify_channels().await?;
            output::table(
                &[
                    "RULE", "CHANNEL", "SCOPE", "PRESET", "EVENTS", "LIMITS", "STATE",
                ],
                rules
                    .iter()
                    .map(|r| {
                        let channel = channels
                            .iter()
                            .find(|ch| ch.id == r.channel_id)
                            .map_or_else(|| r.channel_id.clone(), |ch| ch.name.clone());
                        vec![
                            Cell(r.id.clone(), Tone::Muted),
                            Cell::from(channel),
                            Cell::from(r.scope_label.clone()),
                            Cell(
                                r.preset.clone().unwrap_or_else(|| "custom".into()),
                                Tone::Muted,
                            ),
                            Cell::from(format!("{} events", r.events.len())),
                            Cell(
                                format!("{}s dedupe, {}/h", r.dedupe_secs, r.max_per_hour),
                                Tone::Muted,
                            ),
                            if r.enabled {
                                Cell("on".into(), Tone::Ok)
                            } else {
                                Cell("off".into(), Tone::Muted)
                            },
                        ]
                    })
                    .collect(),
            );
            Ok(())
        }
        RuleCmd::Add {
            channel,
            preset,
            event,
            exclude,
            project,
            environment,
            service,
            dedupe,
            max_per_hour,
        } => {
            let ch = find_channel(c, &channel).await?;
            let preset = preset.filter(|p| p != "custom");
            let mut events = match &preset {
                Some(id) => common::notify::preset(id)
                    .with_context(|| {
                        format!("unknown preset '{id}'; see {}", hint("notify presets"))
                    })?
                    .events(),
                None => Vec::new(),
            };
            for pattern in &event {
                events.extend(events_matching(pattern)?);
            }
            for pattern in &exclude {
                let drop = events_matching(pattern)?;
                events.retain(|e| !drop.contains(e));
            }
            events.sort();
            events.dedup();
            if events.is_empty() {
                bail!(
                    "the rule has no events: pass --preset or --event (see {})",
                    hint("notify events")
                );
            }
            let scope = if project.is_some() || environment.is_some() || service.is_some() {
                let tree = c.tree().await?;
                if let Some(p) = project {
                    RuleScope::Project {
                        id: resolve::project(&tree, &p)?.project.id.clone(),
                    }
                } else if let Some(e) = environment {
                    RuleScope::Environment {
                        id: resolve::environment(&tree, &e)?.1.environment.id.clone(),
                    }
                } else {
                    let query = service.unwrap_or_default();
                    RuleScope::Service {
                        id: resolve::service(&tree, &query)?.2.id.clone(),
                    }
                }
            } else {
                RuleScope::Instance
            };
            let r = c
                .create_notify_rule(&CreateNotifyRule {
                    channel_id: ch.id,
                    scope,
                    preset,
                    events,
                    enabled: true,
                    dedupe_secs: dedupe,
                    max_per_hour,
                })
                .await?;
            if json {
                return output::json(&r);
            }
            output::ok(format!(
                "rule {} sends {} events about {} to {}",
                r.id,
                r.events.len(),
                r.scope_label,
                ch.name
            ));
            println!(
                "{}",
                muted(
                    r.events
                        .iter()
                        .map(|e| e.id())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            );
            Ok(())
        }
        RuleCmd::Rm { rule, yes } => {
            confirm(&format!("Remove rule {rule}?"), yes)?;
            c.delete_notify_rule(&rule).await?;
            if !json {
                output::ok(format!("removed rule {rule}"));
            }
            Ok(())
        }
    }
}
