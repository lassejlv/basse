//! Notifications: the event catalog, presets, channels, rules and the delivery log.
//!
//! Every event the daemon can announce is a [`NotifyEvent`] with a stable string id such as
//! `deploy.failed`. A [`NotifyRule`] sends a set of events in a [`RuleScope`] to one
//! [`NotifyChannel`]. [`PRESETS`] fill a rule's event set in one pick.

use crate::{Id, Timestamp};
use serde::{Deserialize, Serialize};
use std::fmt;

/// How bad an event is. Channels color and prioritize messages by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Success,
    Warning,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }

    /// The status palette as `0xRRGGBB`, for channels that color messages (Discord embeds,
    /// Slack and Teams accents).
    pub fn color(self) -> u32 {
        match self {
            Self::Info => 0x6CA6E0,
            Self::Success => 0x4FBF7F,
            Self::Warning => 0xF5A524,
            Self::Critical => 0xE5534B,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventCategory {
    Deploys,
    Services,
    Databases,
    Backups,
    Storage,
    Domains,
    Servers,
    System,
    Security,
}

impl EventCategory {
    pub const ALL: [Self; 9] = [
        Self::Deploys,
        Self::Services,
        Self::Databases,
        Self::Backups,
        Self::Storage,
        Self::Domains,
        Self::Servers,
        Self::System,
        Self::Security,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Deploys => "Deploys",
            Self::Services => "Services",
            Self::Databases => "Databases",
            Self::Backups => "Instance backups",
            Self::Storage => "Storage",
            Self::Domains => "Domains & TLS",
            Self::Servers => "Servers",
            Self::System => "System",
            Self::Security => "Security",
        }
    }

    /// Events of this category in catalog order.
    pub fn events(self) -> impl Iterator<Item = NotifyEvent> {
        NotifyEvent::ALL
            .iter()
            .copied()
            .filter(move |e| e.category() == self)
    }
}

macro_rules! events {
    ($($variant:ident => $id:literal, $category:ident, $severity:ident, $label:literal;)*) => {
        /// Everything the daemon can notify about. Serialized as its stable id, e.g.
        /// `deploy.failed`; ids never change once released.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum NotifyEvent {
            $(#[serde(rename = $id)] $variant,)*
        }

        impl NotifyEvent {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            pub fn id(self) -> &'static str {
                match self { $(Self::$variant => $id),* }
            }

            pub fn category(self) -> EventCategory {
                match self { $(Self::$variant => EventCategory::$category),* }
            }

            pub fn severity(self) -> Severity {
                match self { $(Self::$variant => Severity::$severity),* }
            }

            /// Short human label, e.g. `Deploy failed`.
            pub fn label(self) -> &'static str {
                match self { $(Self::$variant => $label),* }
            }
        }
    };
}

events! {
    DeployQueued => "deploy.queued", Deploys, Info, "Deploy queued";
    DeployStarted => "deploy.started", Deploys, Info, "Deploy started";
    DeploySucceeded => "deploy.succeeded", Deploys, Success, "Deploy succeeded";
    DeployFailed => "deploy.failed", Deploys, Critical, "Deploy failed";
    DeployCancelled => "deploy.cancelled", Deploys, Warning, "Deploy cancelled";
    DeployRolledBack => "deploy.rolled_back", Deploys, Warning, "Rolled back";
    DeployHealthCheckFailed => "deploy.health_check_failed", Deploys, Critical, "Health check failed";
    ServiceStarted => "service.started", Services, Info, "Service started";
    ServiceStopped => "service.stopped", Services, Info, "Service stopped";
    ServiceRestarted => "service.restarted", Services, Info, "Service restarted";
    ServiceCrashed => "service.crashed", Services, Critical, "Service crashed";
    ServiceOomKilled => "service.oom_killed", Services, Critical, "Killed out of memory";
    ServiceRestartLoop => "service.restart_loop", Services, Critical, "Restart loop";
    ServiceMemoryHigh => "service.memory_high", Services, Warning, "Memory near its limit";
    DatabaseCreated => "database.created", Databases, Info, "Database created";
    DatabaseBackupSucceeded => "database.backup_succeeded", Databases, Success, "Backup succeeded";
    DatabaseBackupFailed => "database.backup_failed", Databases, Critical, "Backup failed";
    DatabaseRestored => "database.restored", Databases, Success, "Restore done";
    DatabaseRestoreFailed => "database.restore_failed", Databases, Critical, "Restore failed";
    DatabasePasswordRotated => "database.password_rotated", Databases, Info, "Password rotated";
    InstanceBackupSucceeded => "instance_backup.succeeded", Backups, Success, "Instance backup succeeded";
    InstanceBackupFailed => "instance_backup.failed", Backups, Critical, "Instance backup failed";
    StorageUnreachable => "storage.unreachable", Storage, Critical, "S3 storage unreachable";
    TlsCertificateIssued => "tls.certificate_issued", Domains, Success, "Certificate issued";
    TlsExpiringSoon => "tls.expiring_soon", Domains, Warning, "Certificate expiring soon";
    TlsCertificateInvalid => "tls.certificate_invalid", Domains, Critical, "Certificate invalid";
    ServerConnected => "server.connected", Servers, Success, "Server connected";
    ServerUnreachable => "server.unreachable", Servers, Critical, "Server unreachable";
    ServerDiskHigh => "server.disk_high", Servers, Warning, "Disk usage high";
    ServerMemoryHigh => "server.memory_high", Servers, Warning, "Memory usage high";
    DaemonStarted => "system.daemon_started", System, Info, "Daemon started";
    UpdateAvailable => "system.update_available", System, Info, "Update available";
    UpdateInstalled => "system.update_installed", System, Success, "Update installed";
    TokenRotated => "security.token_rotated", Security, Warning, "API token rotated";
    SecretChanged => "security.secret_changed", Security, Warning, "Secret changed";
    SecretRevealed => "security.secret_revealed", Security, Warning, "Secret revealed";
}

impl fmt::Display for NotifyEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl std::str::FromStr for NotifyEvent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|e| e.id() == s)
            .ok_or_else(|| format!("unknown event '{s}'"))
    }
}

/// Which events a preset selects.
#[derive(Debug, Clone, Copy)]
pub enum PresetFilter {
    /// Every event of at least this severity.
    MinSeverity(Severity),
    Categories(&'static [EventCategory]),
    All,
}

/// A named starting point for a rule's event set. Picking one fills the set; it can be edited
/// afterwards.
#[derive(Debug, Clone, Copy)]
pub struct Preset {
    /// Stable id used by `--preset` and stored on rules, e.g. `essentials`.
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub filter: PresetFilter,
}

impl Preset {
    pub fn events(&self) -> Vec<NotifyEvent> {
        NotifyEvent::ALL
            .iter()
            .copied()
            .filter(|e| match self.filter {
                PresetFilter::MinSeverity(min) => e.severity() >= min,
                PresetFilter::Categories(categories) => categories.contains(&e.category()),
                PresetFilter::All => true,
            })
            .collect()
    }
}

/// Built-in presets in picker order. "Custom" is not a preset: it is a rule with no preset.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "essentials",
        label: "Essentials",
        description: "Failures and outages only",
        filter: PresetFilter::MinSeverity(Severity::Critical),
    },
    Preset {
        id: "deploys",
        label: "Deploys",
        description: "The whole deploy lifecycle",
        filter: PresetFilter::Categories(&[EventCategory::Deploys]),
    },
    Preset {
        id: "databases",
        label: "Databases & backups",
        description: "Databases, backups, restores and storage",
        filter: PresetFilter::Categories(&[
            EventCategory::Databases,
            EventCategory::Backups,
            EventCategory::Storage,
        ]),
    },
    Preset {
        id: "everything",
        label: "Everything",
        description: "Every event in the catalog",
        filter: PresetFilter::All,
    },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// One entry of `GET /notify/catalog`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventInfo {
    pub event: NotifyEvent,
    pub label: String,
    pub category: EventCategory,
    pub severity: Severity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetInfo {
    pub id: String,
    pub label: String,
    pub description: String,
    pub events: Vec<NotifyEvent>,
}

/// The event catalog and presets, as served by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyCatalog {
    pub events: Vec<EventInfo>,
    pub presets: Vec<PresetInfo>,
}

impl NotifyCatalog {
    pub fn builtin() -> Self {
        Self {
            events: NotifyEvent::ALL
                .iter()
                .map(|&event| EventInfo {
                    event,
                    label: event.label().into(),
                    category: event.category(),
                    severity: event.severity(),
                })
                .collect(),
            presets: PRESETS
                .iter()
                .map(|p| PresetInfo {
                    id: p.id.into(),
                    label: p.label.into(),
                    description: p.description.into(),
                    events: p.events(),
                })
                .collect(),
        }
    }
}

/// Shown in place of a credential in every API response.
pub const REDACTED: &str = "[redacted]";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurity {
    /// Plain connection upgraded with STARTTLS (usually port 587).
    #[default]
    Starttls,
    /// TLS from the first byte (usually port 465).
    Tls,
    /// No encryption, for local relays only.
    None,
}

impl SmtpSecurity {
    pub const ALL: [Self; 3] = [Self::Starttls, Self::Tls, Self::None];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starttls => "starttls",
            Self::Tls => "tls",
            Self::None => "none",
        }
    }
}

impl std::str::FromStr for SmtpSecurity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "starttls" => Ok(Self::Starttls),
            "tls" | "ssl" | "implicit" => Ok(Self::Tls),
            "none" | "plain" => Ok(Self::None),
            _ => Err(format!(
                "unknown SMTP security '{s}', expected starttls, tls or none"
            )),
        }
    }
}

/// How an email channel sends mail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum EmailProvider {
    Smtp {
        host: String,
        port: u16,
        #[serde(default)]
        security: SmtpSecurity,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
    },
    /// Resend's HTTP API.
    Resend { api_key: String },
    /// Cloudflare Email Service's REST API (`/accounts/{id}/email/sending/send`).
    Cloudflare {
        account_id: String,
        api_token: String,
    },
}

/// An extra header a generic webhook sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookHeader {
    pub name: String,
    pub value: String,
}

/// Where a channel delivers and the credentials it needs. The whole config is sealed under the
/// master key in the store; responses carry [`ChannelConfig::redacted`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChannelConfig {
    Email {
        #[serde(flatten)]
        provider: EmailProvider,
        /// Sender, `ops@example.com` or `Ops <ops@example.com>`.
        from: String,
        to: Vec<String>,
    },
    /// A Discord channel webhook; messages are embeds colored by severity.
    Discord {
        webhook_url: String,
    },
    /// A Slack incoming webhook; messages use Block Kit.
    Slack {
        webhook_url: String,
    },
    /// A Mattermost incoming webhook (Slack-compatible attachments).
    Mattermost {
        webhook_url: String,
    },
    /// A Google Chat space webhook.
    GoogleChat {
        webhook_url: String,
    },
    /// A Microsoft Teams Workflows webhook ("Post to a channel when a webhook request is
    /// received"); messages are Adaptive Cards.
    Teams {
        webhook_url: String,
    },
    Telegram {
        bot_token: String,
        chat_id: String,
    },
    Ntfy {
        /// e.g. `https://ntfy.sh`.
        server: String,
        topic: String,
        #[serde(default)]
        token: Option<String>,
    },
    Gotify {
        server: String,
        app_token: String,
    },
    Pushover {
        user_key: String,
        app_token: String,
    },
    /// A JSON `POST` of the event to any URL.
    Webhook {
        url: String,
        /// Signs the body with HMAC-SHA256 into a `<prefix>-Signature: sha256=<hex>` header.
        #[serde(default)]
        secret: Option<String>,
        #[serde(default)]
        headers: Vec<WebhookHeader>,
    },
}

/// Channel kinds, for listings, parsing and pickers. Email is one kind per provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    EmailSmtp,
    EmailResend,
    EmailCloudflare,
    Discord,
    Slack,
    Mattermost,
    GoogleChat,
    Teams,
    Telegram,
    Ntfy,
    Gotify,
    Pushover,
    Webhook,
}

impl ChannelKind {
    pub const ALL: [Self; 13] = [
        Self::EmailSmtp,
        Self::EmailResend,
        Self::EmailCloudflare,
        Self::Discord,
        Self::Slack,
        Self::Mattermost,
        Self::GoogleChat,
        Self::Teams,
        Self::Telegram,
        Self::Ntfy,
        Self::Gotify,
        Self::Pushover,
        Self::Webhook,
    ];

    /// Stable id used on the command line, e.g. `smtp`, `discord`, `google-chat`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmailSmtp => "smtp",
            Self::EmailResend => "resend",
            Self::EmailCloudflare => "cloudflare",
            Self::Discord => "discord",
            Self::Slack => "slack",
            Self::Mattermost => "mattermost",
            Self::GoogleChat => "google-chat",
            Self::Teams => "teams",
            Self::Telegram => "telegram",
            Self::Ntfy => "ntfy",
            Self::Gotify => "gotify",
            Self::Pushover => "pushover",
            Self::Webhook => "webhook",
        }
    }

    /// Human label, e.g. `email · smtp`, `discord`.
    pub fn label(self) -> &'static str {
        match self {
            Self::EmailSmtp => "email · smtp",
            Self::EmailResend => "email · resend",
            Self::EmailCloudflare => "email · cloudflare",
            Self::Discord => "discord",
            Self::Slack => "slack",
            Self::Mattermost => "mattermost",
            Self::GoogleChat => "google chat",
            Self::Teams => "teams",
            Self::Telegram => "telegram",
            Self::Ntfy => "ntfy",
            Self::Gotify => "gotify",
            Self::Pushover => "pushover",
            Self::Webhook => "webhook",
        }
    }

    pub fn is_email(self) -> bool {
        matches!(
            self,
            Self::EmailSmtp | Self::EmailResend | Self::EmailCloudflare
        )
    }
}

impl fmt::Display for ChannelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ChannelKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.replace('_', "-");
        let s = s.strip_prefix("email-").unwrap_or(&s);
        Self::ALL
            .into_iter()
            .find(|k| k.as_str() == s || (s == "msteams" && *k == Self::Teams))
            .ok_or_else(|| {
                let all: Vec<_> = Self::ALL.iter().map(|k| k.as_str()).collect();
                format!(
                    "unknown channel kind '{s}', expected one of {}",
                    all.join(", ")
                )
            })
    }
}

fn redact(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        REDACTED.into()
    }
}

/// The host of a URL, without credentials, port included.
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    host.rsplit('@').next().unwrap_or(host)
}

/// `https://discord.com/api/webhooks/123/abc` becomes `https://discord.com/…`: the host shows
/// where it goes, the rest is a credential.
fn redact_url(url: &str) -> String {
    let scheme = url.split_once("://").map_or("https", |(s, _)| s);
    format!("{scheme}://{}/…", url_host(url))
}

fn check_url(field: &str, url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| format!("{field} must start with https:// or http://"))?;
    if rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .is_empty()
    {
        return Err(format!("{field} has no host"));
    }
    Ok(())
}

fn check_present(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} must not be empty"))
    } else {
        Ok(())
    }
}

fn check_email(field: &str, address: &str) -> Result<(), String> {
    let bare = address
        .rsplit_once('<')
        .map_or(address, |(_, rest)| rest.trim_end_matches('>'))
        .trim();
    match bare.split_once('@') {
        Some((local, domain)) if !local.is_empty() && domain.contains('.') => Ok(()),
        _ => Err(format!("{field} '{address}' is not an email address")),
    }
}

impl ChannelConfig {
    pub fn kind(&self) -> ChannelKind {
        match self {
            Self::Email { provider, .. } => match provider {
                EmailProvider::Smtp { .. } => ChannelKind::EmailSmtp,
                EmailProvider::Resend { .. } => ChannelKind::EmailResend,
                EmailProvider::Cloudflare { .. } => ChannelKind::EmailCloudflare,
            },
            Self::Discord { .. } => ChannelKind::Discord,
            Self::Slack { .. } => ChannelKind::Slack,
            Self::Mattermost { .. } => ChannelKind::Mattermost,
            Self::GoogleChat { .. } => ChannelKind::GoogleChat,
            Self::Teams { .. } => ChannelKind::Teams,
            Self::Telegram { .. } => ChannelKind::Telegram,
            Self::Ntfy { .. } => ChannelKind::Ntfy,
            Self::Gotify { .. } => ChannelKind::Gotify,
            Self::Pushover { .. } => ChannelKind::Pushover,
            Self::Webhook { .. } => ChannelKind::Webhook,
        }
    }

    /// Checks every field before the daemon stores the channel.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Email { provider, from, to } => {
                check_email("from", from)?;
                if to.is_empty() {
                    return Err("an email channel needs at least one recipient".into());
                }
                for address in to {
                    check_email("recipient", address)?;
                }
                match provider {
                    EmailProvider::Smtp {
                        host,
                        port,
                        username,
                        password,
                        ..
                    } => {
                        check_present("host", host)?;
                        if *port == 0 {
                            return Err("port must not be 0".into());
                        }
                        if username.as_deref().is_some_and(|u| !u.is_empty())
                            != password.as_deref().is_some_and(|p| !p.is_empty())
                        {
                            return Err("SMTP login needs both a username and a password".into());
                        }
                        Ok(())
                    }
                    EmailProvider::Resend { api_key } => check_present("api key", api_key),
                    EmailProvider::Cloudflare {
                        account_id,
                        api_token,
                    } => {
                        check_present("account id", account_id)?;
                        check_present("api token", api_token)
                    }
                }
            }
            Self::Discord { webhook_url }
            | Self::Slack { webhook_url }
            | Self::Mattermost { webhook_url }
            | Self::GoogleChat { webhook_url }
            | Self::Teams { webhook_url } => check_url("webhook url", webhook_url),
            Self::Telegram { bot_token, chat_id } => {
                check_present("bot token", bot_token)?;
                check_present("chat id", chat_id)
            }
            Self::Ntfy { server, topic, .. } => {
                check_url("server", server)?;
                check_present("topic", topic)?;
                if topic.contains(['/', ' ']) {
                    return Err("topic must not contain slashes or spaces".into());
                }
                Ok(())
            }
            Self::Gotify { server, app_token } => {
                check_url("server", server)?;
                check_present("app token", app_token)
            }
            Self::Pushover {
                user_key,
                app_token,
            } => {
                check_present("user key", user_key)?;
                check_present("app token", app_token)
            }
            Self::Webhook { url, headers, .. } => {
                check_url("url", url)?;
                for h in headers {
                    if h.name.is_empty()
                        || !h
                            .name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    {
                        return Err(format!("invalid header name '{}'", h.name));
                    }
                }
                Ok(())
            }
        }
    }

    /// The config with every credential replaced by [`REDACTED`] and webhook URLs cut to their
    /// host. This is the only form the API ever returns.
    pub fn redacted(&self) -> Self {
        let mut out = self.clone();
        match &mut out {
            Self::Email { provider, .. } => match provider {
                EmailProvider::Smtp { password, .. } => {
                    *password = password.as_deref().map(redact);
                }
                EmailProvider::Resend { api_key } => *api_key = redact(api_key),
                EmailProvider::Cloudflare { api_token, .. } => *api_token = redact(api_token),
            },
            Self::Discord { webhook_url }
            | Self::Slack { webhook_url }
            | Self::Mattermost { webhook_url }
            | Self::GoogleChat { webhook_url }
            | Self::Teams { webhook_url } => *webhook_url = redact_url(webhook_url),
            Self::Telegram { bot_token, .. } => *bot_token = redact(bot_token),
            Self::Ntfy { token, .. } => *token = token.as_deref().map(redact),
            Self::Gotify { app_token, .. } => *app_token = redact(app_token),
            Self::Pushover {
                user_key,
                app_token,
            } => {
                *user_key = redact(user_key);
                *app_token = redact(app_token);
            }
            Self::Webhook {
                url,
                secret,
                headers,
            } => {
                *url = redact_url(url);
                *secret = secret.as_deref().map(redact);
                for h in headers {
                    h.value = redact(&h.value);
                }
            }
        }
        out
    }

    /// For an edit of a stored channel: every credential still equal to its redacted form takes
    /// the stored value, so a client can send back what it was shown and change only the
    /// fields the user retyped. Credentials of a different kind are never carried over.
    pub fn keep_redacted(mut self, stored: &ChannelConfig) -> Self {
        fn keep(new: &mut String, stored: &str, shown: &str) {
            if *new == shown {
                *new = stored.to_string();
            }
        }
        fn keep_opt(new: &mut Option<String>, stored: &Option<String>, shown: &Option<String>) {
            if new.as_deref() == Some(REDACTED) && shown.is_none() {
                *new = None;
            } else if new.is_some() && new == shown {
                new.clone_from(stored);
            }
        }
        let shown = stored.redacted();
        match (&mut self, stored, &shown) {
            (
                Self::Email { provider: p, .. },
                Self::Email { provider: s, .. },
                Self::Email { provider: r, .. },
            ) => match (p, s, r) {
                (
                    EmailProvider::Smtp { password: p, .. },
                    EmailProvider::Smtp { password: s, .. },
                    EmailProvider::Smtp { password: r, .. },
                ) => keep_opt(p, s, r),
                (
                    EmailProvider::Resend { api_key: p },
                    EmailProvider::Resend { api_key: s },
                    EmailProvider::Resend { api_key: r },
                ) => keep(p, s, r),
                (
                    EmailProvider::Cloudflare { api_token: p, .. },
                    EmailProvider::Cloudflare { api_token: s, .. },
                    EmailProvider::Cloudflare { api_token: r, .. },
                ) => keep(p, s, r),
                _ => {}
            },
            (
                Self::Discord { webhook_url: p },
                Self::Discord { webhook_url: s },
                Self::Discord { webhook_url: r },
            )
            | (
                Self::Slack { webhook_url: p },
                Self::Slack { webhook_url: s },
                Self::Slack { webhook_url: r },
            )
            | (
                Self::Mattermost { webhook_url: p },
                Self::Mattermost { webhook_url: s },
                Self::Mattermost { webhook_url: r },
            )
            | (
                Self::GoogleChat { webhook_url: p },
                Self::GoogleChat { webhook_url: s },
                Self::GoogleChat { webhook_url: r },
            )
            | (
                Self::Teams { webhook_url: p },
                Self::Teams { webhook_url: s },
                Self::Teams { webhook_url: r },
            ) => {
                keep(p, s, r);
            }
            (
                Self::Telegram { bot_token: p, .. },
                Self::Telegram { bot_token: s, .. },
                Self::Telegram { bot_token: r, .. },
            ) => keep(p, s, r),
            (
                Self::Ntfy { token: p, .. },
                Self::Ntfy { token: s, .. },
                Self::Ntfy { token: r, .. },
            ) => {
                keep_opt(p, s, r);
            }
            (
                Self::Gotify { app_token: p, .. },
                Self::Gotify { app_token: s, .. },
                Self::Gotify { app_token: r, .. },
            ) => keep(p, s, r),
            (
                Self::Pushover {
                    user_key: pu,
                    app_token: pa,
                },
                Self::Pushover {
                    user_key: su,
                    app_token: sa,
                },
                Self::Pushover {
                    user_key: ru,
                    app_token: ra,
                },
            ) => {
                keep(pu, su, ru);
                keep(pa, sa, ra);
            }
            (
                Self::Webhook {
                    url: pu,
                    secret: ps,
                    headers: ph,
                },
                Self::Webhook {
                    url: su,
                    secret: ss,
                    headers: sh,
                },
                Self::Webhook {
                    url: ru,
                    secret: rs,
                    ..
                },
            ) => {
                keep(pu, su, ru);
                keep_opt(ps, ss, rs);
                for h in ph.iter_mut().filter(|h| h.value == REDACTED) {
                    if let Some(old) = sh.iter().find(|o| o.name.eq_ignore_ascii_case(&h.name)) {
                        h.value.clone_from(&old.value);
                    }
                }
            }
            _ => {}
        }
        self
    }

    /// One line saying where messages go, safe to show: `ops@acme.dev via smtp.acme.dev:587`.
    pub fn target(&self) -> String {
        match self {
            Self::Email { provider, to, .. } => {
                let to = match to.as_slice() {
                    [one] => one.clone(),
                    [first, rest @ ..] => format!("{first} +{}", rest.len()),
                    [] => String::new(),
                };
                match provider {
                    EmailProvider::Smtp { host, port, .. } => format!("{to} via {host}:{port}"),
                    EmailProvider::Resend { .. } => format!("{to} via resend"),
                    EmailProvider::Cloudflare { .. } => format!("{to} via cloudflare"),
                }
            }
            Self::Discord { webhook_url }
            | Self::Slack { webhook_url }
            | Self::Mattermost { webhook_url }
            | Self::GoogleChat { webhook_url }
            | Self::Teams { webhook_url } => format!("{}/…", url_host(webhook_url)),
            Self::Telegram { chat_id, .. } => format!("chat {chat_id}"),
            Self::Ntfy { server, topic, .. } => format!("{}/{topic}", url_host(server)),
            Self::Gotify { server, .. } => url_host(server).to_string(),
            Self::Pushover { .. } => "pushover".into(),
            Self::Webhook { url, .. } => format!("{}/…", url_host(url)),
        }
    }
}

/// Outcome of the last `POST /notify/channels/{id}/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelTest {
    pub ok: bool,
    pub at: Timestamp,
    pub error: Option<String>,
    pub latency_ms: Option<u64>,
}

/// A notification destination. `config` is always [`ChannelConfig::redacted`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyChannel {
    pub id: Id,
    pub name: String,
    pub kind: ChannelKind,
    pub enabled: bool,
    pub config: ChannelConfig,
    /// Where messages go, from [`ChannelConfig::target`].
    pub target: String,
    pub created_at: Timestamp,
    pub last_test: Option<ChannelTest>,
}

/// What part of the instance a rule listens to. Events about servers and the daemon itself
/// only match `instance` rules.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleScope {
    #[default]
    Instance,
    Project {
        id: Id,
    },
    Environment {
        id: Id,
    },
    Service {
        id: Id,
    },
}

impl RuleScope {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Instance => "instance",
            Self::Project { .. } => "project",
            Self::Environment { .. } => "environment",
            Self::Service { .. } => "service",
        }
    }

    pub fn id(&self) -> Option<&str> {
        match self {
            Self::Instance => None,
            Self::Project { id } | Self::Environment { id } | Self::Service { id } => Some(id),
        }
    }
}

/// Sends `events` within `scope` to a channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyRule {
    pub id: Id,
    pub channel_id: Id,
    pub scope: RuleScope,
    /// Human label of the scope: `instance`, `acme`, `acme/production`, `acme/production/api`.
    pub scope_label: String,
    /// The preset the event set started from; `None` for a custom pick.
    pub preset: Option<String>,
    pub events: Vec<NotifyEvent>,
    pub enabled: bool,
    /// The same event about the same subject is sent at most once in this window.
    pub dedupe_secs: u32,
    /// At most this many messages per hour; the rest are logged as suppressed.
    pub max_per_hour: u32,
    pub created_at: Timestamp,
}

pub const DEFAULT_DEDUPE_SECS: u32 = 300;
pub const DEFAULT_MAX_PER_HOUR: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    Sent,
    /// Failed, another attempt is scheduled.
    Retrying,
    /// Every attempt failed.
    Failed,
    /// Dropped by the rule's dedupe window or rate limit.
    Suppressed,
}

impl DeliveryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Retrying => "retrying",
            Self::Failed => "failed",
            Self::Suppressed => "suppressed",
        }
    }
}

impl std::str::FromStr for DeliveryStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [Self::Sent, Self::Retrying, Self::Failed, Self::Suppressed]
            .into_iter()
            .find(|d| d.as_str() == s)
            .ok_or_else(|| format!("unknown delivery status '{s}'"))
    }
}

/// One entry of the delivery log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyDelivery {
    pub id: i64,
    pub at: Timestamp,
    pub channel_id: Id,
    pub channel_name: String,
    /// `None` for a test message.
    pub rule_id: Option<Id>,
    /// `None` for a test message.
    pub event: Option<NotifyEvent>,
    /// What it was about: `acme/production/api #42`, a server name, or `instance`.
    pub subject: String,
    pub title: String,
    pub status: DeliveryStatus,
    pub attempts: u32,
    pub error: Option<String>,
    pub latency_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_ids_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for &e in NotifyEvent::ALL {
            assert!(seen.insert(e.id()), "duplicate id {}", e.id());
            assert_eq!(e.id().parse::<NotifyEvent>().unwrap(), e);
            let json = serde_json::to_string(&e).unwrap();
            assert_eq!(json, format!("\"{}\"", e.id()));
        }
        assert!("deploy.exploded".parse::<NotifyEvent>().is_err());
    }

    #[test]
    fn every_category_has_events() {
        for c in EventCategory::ALL {
            assert!(c.events().count() > 0, "{c:?} is empty");
        }
    }

    #[test]
    fn presets_select_the_documented_events() {
        let essentials = preset("essentials").unwrap().events();
        assert!(essentials.contains(&NotifyEvent::DeployFailed));
        assert!(essentials.contains(&NotifyEvent::ServerUnreachable));
        assert!(!essentials.contains(&NotifyEvent::DeploySucceeded));
        assert!(
            essentials
                .iter()
                .all(|e| e.severity() == Severity::Critical)
        );
        let deploys = preset("deploys").unwrap().events();
        assert_eq!(deploys.len(), EventCategory::Deploys.events().count());
        let dbs = preset("databases").unwrap().events();
        assert!(dbs.contains(&NotifyEvent::InstanceBackupFailed));
        assert!(dbs.contains(&NotifyEvent::StorageUnreachable));
        assert_eq!(
            preset("everything").unwrap().events().len(),
            NotifyEvent::ALL.len()
        );
        assert!(preset("custom").is_none());
    }

    fn email(provider: EmailProvider) -> ChannelConfig {
        ChannelConfig::Email {
            provider,
            from: "ops@acme.dev".into(),
            to: vec!["me@acme.dev".into()],
        }
    }

    #[test]
    fn redaction_hides_every_credential() {
        let configs = [
            email(EmailProvider::Smtp {
                host: "smtp.acme.dev".into(),
                port: 587,
                security: SmtpSecurity::Starttls,
                username: Some("ops".into()),
                password: Some("hunter2".into()),
            }),
            email(EmailProvider::Resend {
                api_key: "re_hunter2".into(),
            }),
            email(EmailProvider::Cloudflare {
                account_id: "acc".into(),
                api_token: "hunter2".into(),
            }),
            ChannelConfig::Discord {
                webhook_url: "https://discord.com/api/webhooks/1/hunter2".into(),
            },
            ChannelConfig::Telegram {
                bot_token: "123:hunter2".into(),
                chat_id: "42".into(),
            },
            ChannelConfig::Ntfy {
                server: "https://ntfy.sh".into(),
                topic: "alerts".into(),
                token: Some("tk_hunter2".into()),
            },
            ChannelConfig::Gotify {
                server: "https://push.acme.dev".into(),
                app_token: "hunter2".into(),
            },
            ChannelConfig::Pushover {
                user_key: "hunter2u".into(),
                app_token: "hunter2a".into(),
            },
            ChannelConfig::Webhook {
                url: "https://user:hunter2@hooks.acme.dev/x?token=hunter2".into(),
                secret: Some("hunter2".into()),
                headers: vec![WebhookHeader {
                    name: "Authorization".into(),
                    value: "Bearer hunter2".into(),
                }],
            },
        ];
        for config in configs {
            config.validate().unwrap();
            let redacted = serde_json::to_string(&config.redacted()).unwrap();
            assert!(!redacted.contains("hunter2"), "{redacted}");
            assert!(!config.target().contains("hunter2"), "{}", config.target());
        }
    }

    #[test]
    fn edits_keep_credentials_that_come_back_redacted() {
        let stored = ChannelConfig::Webhook {
            url: "https://hooks.acme.dev/x?token=hunter2".into(),
            secret: Some("hmac".into()),
            headers: vec![WebhookHeader {
                name: "Authorization".into(),
                value: "Bearer hunter2".into(),
            }],
        };
        let edited = stored.redacted().keep_redacted(&stored);
        assert_eq!(edited, stored);
        let retyped = ChannelConfig::Webhook {
            url: "https://hooks.acme.dev/…".into(),
            secret: Some("new".into()),
            headers: Vec::new(),
        }
        .keep_redacted(&stored);
        let ChannelConfig::Webhook { url, secret, .. } = retyped else {
            unreachable!()
        };
        assert_eq!(url, "https://hooks.acme.dev/x?token=hunter2");
        assert_eq!(secret.as_deref(), Some("new"));
        let unsigned = ChannelConfig::Webhook {
            url: "https://hooks.acme.dev/x".into(),
            secret: None,
            headers: Vec::new(),
        };
        let edited = ChannelConfig::Webhook {
            url: "https://hooks.acme.dev/…".into(),
            secret: Some(REDACTED.into()),
            headers: Vec::new(),
        }
        .keep_redacted(&unsigned);
        assert_eq!(edited, unsigned);
        let other_kind = ChannelConfig::Slack {
            webhook_url: "https://hooks.acme.dev/…".into(),
        }
        .keep_redacted(&stored);
        assert!(other_kind.validate().is_ok());
        assert_eq!(
            other_kind,
            ChannelConfig::Slack {
                webhook_url: "https://hooks.acme.dev/…".into()
            }
        );
    }

    #[test]
    fn email_config_serializes_flat() {
        let config = ChannelConfig::Email {
            provider: EmailProvider::Resend {
                api_key: "k".into(),
            },
            from: "Ops <ops@acme.dev>".into(),
            to: vec!["me@acme.dev".into()],
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["kind"], "email");
        assert_eq!(json["provider"], "resend");
        assert_eq!(
            serde_json::from_value::<ChannelConfig>(json).unwrap(),
            config
        );
        assert_eq!(config.kind(), ChannelKind::EmailResend);
        config.validate().unwrap();
    }

    #[test]
    fn invalid_configs_are_rejected() {
        let slack = ChannelConfig::Slack {
            webhook_url: "hooks.slack.com/x".into(),
        };
        assert!(slack.validate().is_err());
        let bad_from = ChannelConfig::Email {
            provider: EmailProvider::Resend {
                api_key: "k".into(),
            },
            from: "nope".into(),
            to: vec!["me@acme.dev".into()],
        };
        assert!(bad_from.validate().is_err());
        assert_eq!(
            "email-smtp".parse::<ChannelKind>().unwrap(),
            ChannelKind::EmailSmtp
        );
        assert_eq!(
            "google_chat".parse::<ChannelKind>().unwrap(),
            ChannelKind::GoogleChat
        );
    }
}
