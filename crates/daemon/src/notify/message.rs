//! What a notification says and how each channel formats it. Everything here is pure, so the
//! templates are tested without a network.

use common::brand;
use common::notify::{NotifyEvent, Severity};
use common::{Id, Timestamp};
use serde_json::{Value, json};

/// A project, environment or service an event is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub id: Id,
    pub name: String,
}

/// Something that happened, as the place that detected it reports it.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: NotifyEvent,
    pub at: Timestamp,
    pub project: Option<Ref>,
    pub environment: Option<Ref>,
    pub service: Option<Ref>,
    pub deploy: Option<i64>,
    /// What it is about when it is not a service: a server, a storage connection, a domain.
    pub subject: Option<String>,
    /// Identifies the subject for dedupe, e.g. a service or server id.
    pub key: String,
    /// One or two lines: an error, a reason, a number.
    pub detail: Option<String>,
    pub fields: Vec<(String, String)>,
    /// A command that helps, e.g. `<cli> logs api`.
    pub hint: Option<String>,
    pub link: Option<String>,
}

impl Event {
    pub fn new(kind: NotifyEvent) -> Self {
        Self {
            kind,
            at: chrono::Utc::now(),
            project: None,
            environment: None,
            service: None,
            deploy: None,
            subject: None,
            key: String::new(),
            detail: None,
            fields: Vec::new(),
            hint: None,
            link: None,
        }
    }

    /// What the event is about and the key dedupe tells subjects apart by.
    pub fn about(mut self, subject: impl Into<String>, key: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self.key = key.into();
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn field(mut self, name: &str, value: impl Into<String>) -> Self {
        self.fields.push((name.into(), value.into()));
        self
    }

    pub fn hint(mut self, command: impl Into<String>) -> Self {
        self.hint = Some(command.into());
        self
    }

    pub fn deploy(mut self, number: i64) -> Self {
        self.deploy = Some(number);
        self
    }

    /// `acme/production/api #42`, or the subject, or `instance`.
    pub fn subject_label(&self) -> String {
        let path: Vec<&str> = [&self.project, &self.environment, &self.service]
            .into_iter()
            .flatten()
            .map(|r| r.name.as_str())
            .collect();
        let base = if path.is_empty() {
            self.subject.clone().unwrap_or_else(|| "instance".into())
        } else {
            path.join("/")
        };
        match self.deploy {
            Some(n) => format!("{base} #{n}"),
            None => base,
        }
    }

    pub fn message(&self) -> Message {
        let mut fields = Vec::new();
        if let Some(p) = &self.project {
            fields.push(("Project".to_string(), p.name.clone()));
        }
        if let Some(e) = &self.environment {
            fields.push(("Environment".to_string(), e.name.clone()));
        }
        if let Some(s) = &self.service {
            fields.push(("Service".to_string(), s.name.clone()));
        }
        if let Some(n) = self.deploy {
            fields.push(("Deploy".to_string(), format!("#{n}")));
        }
        fields.extend(self.fields.iter().cloned());
        let subject = self.subject_label();
        Message {
            event: Some(self.kind),
            severity: self.kind.severity(),
            title: format!("{} · {subject}", self.kind.label()),
            body: self.detail.as_deref().map(clip),
            subject,
            fields,
            hint: self.hint.clone(),
            link: self.link.clone(),
            at: self.at,
            project: self.project.clone(),
            environment: self.environment.clone(),
            service: self.service.clone(),
            deploy: self.deploy,
        }
    }
}

/// Keeps a message short: channels cut long bodies anyway, and a stack trace belongs in the
/// logs the hint points to.
fn clip(text: &str) -> String {
    const MAX: usize = 900;
    let text = text.trim();
    if text.chars().count() <= MAX {
        return text.to_string();
    }
    let cut: String = text.chars().take(MAX).collect();
    format!("{cut}…")
}

/// A rendered notification, the same for every channel; each sender formats it.
#[derive(Debug, Clone)]
pub struct Message {
    /// `None` for a test message.
    pub event: Option<NotifyEvent>,
    pub severity: Severity,
    /// `Deploy failed · acme/production/api #42`.
    pub title: String,
    pub body: Option<String>,
    pub subject: String,
    pub fields: Vec<(String, String)>,
    pub hint: Option<String>,
    pub link: Option<String>,
    pub at: Timestamp,
    pub project: Option<Ref>,
    pub environment: Option<Ref>,
    pub service: Option<Ref>,
    pub deploy: Option<i64>,
}

impl Message {
    pub fn test(channel_name: &str) -> Self {
        Self {
            event: None,
            severity: Severity::Info,
            title: format!("Test notification · {channel_name}"),
            body: Some(format!(
                "{} can reach this channel. Rules decide which events land here.",
                brand::NAME
            )),
            subject: channel_name.into(),
            fields: Vec::new(),
            hint: Some(format!(
                "{} notify rule add --channel {channel_name}",
                brand::CLI_BIN
            )),
            link: None,
            at: chrono::Utc::now(),
            project: None,
            environment: None,
            service: None,
            deploy: None,
        }
    }

    fn event_id(&self) -> &'static str {
        self.event.map_or("test", NotifyEvent::id)
    }

    /// Footer line: `<name> · deploy.failed`.
    fn footer(&self) -> String {
        format!("{} · {}", brand::NAME, self.event_id())
    }

    /// Plain text for SMS-like channels and email: title, body, fields, hint, link.
    pub fn text(&self) -> String {
        let mut out = vec![self.title.clone()];
        if let Some(body) = &self.body {
            out.push(body.clone());
        }
        let facts: Vec<String> = self
            .fields
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();
        if !facts.is_empty() {
            out.push(facts.join("\n"));
        }
        if let Some(hint) = &self.hint {
            out.push(format!("$ {hint}"));
        }
        if let Some(link) = &self.link {
            out.push(link.clone());
        }
        out.join("\n\n")
    }

    /// The text without the title, for channels with a separate title field.
    fn text_body(&self) -> String {
        let text = self.text();
        text.strip_prefix(&self.title)
            .unwrap_or(&text)
            .trim_start()
            .to_string()
    }

    fn color_hex(&self) -> String {
        format!("#{:06X}", self.severity.color())
    }

    pub fn discord(&self) -> Value {
        let mut embed = json!({
            "title": truncate(&self.title, 256),
            "color": self.severity.color(),
            "timestamp": self.at.to_rfc3339(),
            "footer": { "text": self.footer() },
            "fields": self.fields.iter().take(25).map(|(k, v)| json!({
                "name": k, "value": truncate(v, 1024), "inline": true
            })).collect::<Vec<_>>(),
        });
        let mut description = self.body.clone().unwrap_or_default();
        if let Some(hint) = &self.hint {
            description.push_str(&format!("\n`{hint}`"));
        }
        if !description.trim().is_empty() {
            embed["description"] = json!(truncate(description.trim(), 4096));
        }
        if let Some(link) = &self.link {
            embed["url"] = json!(link);
        }
        json!({ "username": brand::NAME, "embeds": [embed] })
    }

    /// Block Kit inside a colored attachment, so the severity shows as a bar.
    pub fn slack(&self) -> Value {
        let mut blocks = vec![json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": format!("*{}*{}", escape_mrkdwn(&self.title),
                self.body.as_deref().map(|b| format!("\n{}", escape_mrkdwn(b))).unwrap_or_default()) }
        })];
        if !self.fields.is_empty() {
            blocks.push(json!({
                "type": "section",
                "fields": self.fields.iter().take(10).map(|(k, v)| json!({
                    "type": "mrkdwn", "text": format!("*{}*\n{}", escape_mrkdwn(k), escape_mrkdwn(v))
                })).collect::<Vec<_>>(),
            }));
        }
        let mut context = vec![json!({ "type": "mrkdwn", "text": self.footer() })];
        if let Some(hint) = &self.hint {
            context.push(json!({ "type": "mrkdwn", "text": format!("`{hint}`") }));
        }
        if let Some(link) = &self.link {
            context.push(json!({ "type": "mrkdwn", "text": format!("<{link}>") }));
        }
        blocks.push(json!({ "type": "context", "elements": context }));
        json!({
            "text": self.title,
            "attachments": [{ "color": self.color_hex(), "blocks": blocks }],
        })
    }

    pub fn mattermost(&self) -> Value {
        let text: Vec<String> = [
            self.body.clone(),
            self.hint.as_ref().map(|h| format!("`{h}`")),
        ]
        .into_iter()
        .flatten()
        .collect();
        json!({
            "username": brand::NAME,
            "text": "",
            "attachments": [{
                "fallback": self.title,
                "color": self.color_hex(),
                "title": self.title,
                "title_link": self.link,
                "text": text.join("\n"),
                "fields": self.fields.iter().map(|(k, v)| json!({
                    "short": true, "title": k, "value": v
                })).collect::<Vec<_>>(),
                "footer": self.footer(),
            }],
        })
    }

    pub fn google_chat(&self) -> Value {
        json!({ "text": format!("*{}*\n{}", self.title, self.text_body()) })
    }

    /// An Adaptive Card for a Teams Workflows webhook.
    pub fn teams(&self) -> Value {
        let color = match self.severity {
            Severity::Info => "Accent",
            Severity::Success => "Good",
            Severity::Warning => "Warning",
            Severity::Critical => "Attention",
        };
        let mut body = vec![json!({
            "type": "TextBlock", "text": self.title, "weight": "Bolder", "size": "Medium",
            "color": color, "wrap": true
        })];
        if let Some(text) = &self.body {
            body.push(json!({ "type": "TextBlock", "text": text, "wrap": true }));
        }
        if !self.fields.is_empty() {
            body.push(json!({
                "type": "FactSet",
                "facts": self.fields.iter().map(|(k, v)| json!({ "title": k, "value": v }))
                    .collect::<Vec<_>>(),
            }));
        }
        let mut footer = self.footer();
        if let Some(hint) = &self.hint {
            footer.push_str(&format!(" · {hint}"));
        }
        body.push(json!({ "type": "TextBlock", "text": footer, "isSubtle": true, "size": "Small", "wrap": true }));
        let mut card = json!({
            "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
            "type": "AdaptiveCard",
            "version": "1.4",
            "body": body,
        });
        if let Some(link) = &self.link {
            card["actions"] = json!([{ "type": "Action.OpenUrl", "title": "Open", "url": link }]);
        }
        json!({
            "type": "message",
            "attachments": [{
                "contentType": "application/vnd.microsoft.card.adaptive",
                "contentUrl": null,
                "content": card,
            }],
        })
    }

    pub fn telegram(&self, chat_id: &str) -> Value {
        let mut lines = vec![format!("<b>{}</b>", escape_html(&self.title))];
        if let Some(body) = &self.body {
            lines.push(escape_html(body));
        }
        for (k, v) in &self.fields {
            lines.push(format!("{}: {}", escape_html(k), escape_html(v)));
        }
        if let Some(hint) = &self.hint {
            lines.push(format!("<code>{}</code>", escape_html(hint)));
        }
        if let Some(link) = &self.link {
            lines.push(escape_html(link));
        }
        json!({
            "chat_id": chat_id,
            "text": truncate(&lines.join("\n"), 4096),
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        })
    }

    pub fn ntfy(&self, topic: &str) -> Value {
        let (priority, tag) = match self.severity {
            Severity::Info => (3, "information_source"),
            Severity::Success => (3, "white_check_mark"),
            Severity::Warning => (4, "warning"),
            Severity::Critical => (5, "rotating_light"),
        };
        let mut body = json!({
            "topic": topic,
            "title": self.title,
            "message": self.text_body(),
            "priority": priority,
            "tags": [tag, self.event_id()],
        });
        if let Some(link) = &self.link {
            body["click"] = json!(link);
        }
        body
    }

    pub fn gotify(&self) -> Value {
        let priority = match self.severity {
            Severity::Info | Severity::Success => 2,
            Severity::Warning => 5,
            Severity::Critical => 8,
        };
        json!({ "title": self.title, "message": self.text_body(), "priority": priority })
    }

    pub fn pushover(&self, user: &str, token: &str) -> Value {
        let mut body = json!({
            "token": token,
            "user": user,
            "title": truncate(&self.title, 250),
            "message": truncate(&self.text_body(), 1024),
            "priority": i32::from(self.severity == Severity::Critical),
            "timestamp": self.at.timestamp(),
        });
        if let Some(link) = &self.link {
            body["url"] = json!(link);
        }
        body
    }

    /// The generic webhook body: the event and its context, for scripts.
    pub fn webhook(&self) -> Value {
        let named = |r: &Option<Ref>| r.as_ref().map(|r| json!({ "id": r.id, "name": r.name }));
        json!({
            "event": self.event_id(),
            "test": self.event.is_none(),
            "severity": self.severity.as_str(),
            "category": self.event.map(|e| e.category()),
            "title": self.title,
            "message": self.body,
            "subject": self.subject,
            "project": named(&self.project),
            "environment": named(&self.environment),
            "service": named(&self.service),
            "deploy": self.deploy,
            "fields": self.fields.iter().map(|(k, v)| json!({ "name": k, "value": v }))
                .collect::<Vec<_>>(),
            "hint": self.hint,
            "link": self.link,
            "at": self.at.to_rfc3339(),
        })
    }

    /// Subject, plain text and HTML of an email.
    pub fn email(&self) -> (String, String, String) {
        let subject = format!("[{}] {}", brand::NAME, self.title);
        let mut html = format!(
            "<div style=\"font-family:ui-monospace,Menlo,monospace;font-size:13px;line-height:20px\">\
             <p style=\"margin:0 0 12px;color:{}\"><strong>{}</strong></p>",
            self.color_hex(),
            escape_html(&self.title)
        );
        if let Some(body) = &self.body {
            html.push_str(&format!(
                "<p style=\"margin:0 0 12px;white-space:pre-wrap\">{}</p>",
                escape_html(body)
            ));
        }
        if !self.fields.is_empty() {
            html.push_str("<table style=\"border-collapse:collapse;margin:0 0 12px\">");
            for (k, v) in &self.fields {
                html.push_str(&format!(
                    "<tr><td style=\"padding:0 16px 0 0;color:#7C878F\">{}</td><td>{}</td></tr>",
                    escape_html(k),
                    escape_html(v)
                ));
            }
            html.push_str("</table>");
        }
        if let Some(hint) = &self.hint {
            html.push_str(&format!(
                "<p style=\"margin:0 0 12px\"><code>$ {}</code></p>",
                escape_html(hint)
            ));
        }
        if let Some(link) = &self.link {
            let link = escape_html(link);
            html.push_str(&format!(
                "<p style=\"margin:0 0 12px\"><a href=\"{link}\">{link}</a></p>"
            ));
        }
        html.push_str(&format!(
            "<p style=\"margin:0;color:#7C878F\">{}</p></div>",
            escape_html(&self.footer())
        ));
        (
            subject,
            format!("{}\n\n{}", self.text(), self.footer()),
            html,
        )
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Slack treats `&`, `<` and `>` as control characters in mrkdwn.
fn escape_mrkdwn(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed_deploy() -> Event {
        let mut e = Event::new(NotifyEvent::DeployFailed)
            .deploy(42)
            .detail("build failed: exit code 1 <oops>")
            .hint(format!("{} deployments api", brand::CLI_BIN));
        e.project = Some(Ref {
            id: "p1".into(),
            name: "acme".into(),
        });
        e.environment = Some(Ref {
            id: "e1".into(),
            name: "production".into(),
        });
        e.service = Some(Ref {
            id: "s1".into(),
            name: "api".into(),
        });
        e.key = "s1".into();
        e.link = Some("https://api.acme.dev".into());
        e
    }

    #[test]
    fn titles_name_the_event_and_its_subject() {
        let m = failed_deploy().message();
        assert_eq!(m.title, "Deploy failed · acme/production/api #42");
        assert_eq!(m.fields[3], ("Deploy".to_string(), "#42".to_string()));
        let server = Event::new(NotifyEvent::ServerUnreachable)
            .about("edge-1", "srv1")
            .message();
        assert_eq!(server.title, "Server unreachable · edge-1");
        assert_eq!(
            Event::new(NotifyEvent::DaemonStarted).message().title,
            "Daemon started · instance"
        );
    }

    #[test]
    fn discord_embeds_use_the_status_palette() {
        let body = failed_deploy().message().discord();
        let embed = &body["embeds"][0];
        assert_eq!(embed["color"], 0xE5534B);
        assert_eq!(embed["url"], "https://api.acme.dev");
        assert!(
            embed["description"]
                .as_str()
                .unwrap()
                .contains("deployments api")
        );
        assert_eq!(embed["fields"][2]["value"], "api");
        assert!(
            embed["footer"]["text"]
                .as_str()
                .unwrap()
                .ends_with("deploy.failed")
        );
    }

    #[test]
    fn slack_uses_block_kit_and_escapes() {
        let body = failed_deploy().message().slack();
        let attachment = &body["attachments"][0];
        assert_eq!(attachment["color"], "#E5534B");
        let text = attachment["blocks"][0]["text"]["text"].as_str().unwrap();
        assert!(text.contains("&lt;oops&gt;"), "{text}");
        assert_eq!(
            attachment["blocks"][1]["fields"].as_array().unwrap().len(),
            4
        );
    }

    #[test]
    fn telegram_escapes_html_and_teams_builds_a_card() {
        let m = failed_deploy().message();
        let tg = m.telegram("42");
        assert!(tg["text"].as_str().unwrap().contains("&lt;oops&gt;"));
        assert_eq!(tg["parse_mode"], "HTML");
        let teams = m.teams();
        let card = &teams["attachments"][0]["content"];
        assert_eq!(card["type"], "AdaptiveCard");
        assert_eq!(card["body"][0]["color"], "Attention");
        assert_eq!(card["actions"][0]["url"], "https://api.acme.dev");
    }

    #[test]
    fn webhook_body_carries_ids_and_context() {
        let body = failed_deploy().message().webhook();
        assert_eq!(body["event"], "deploy.failed");
        assert_eq!(body["severity"], "critical");
        assert_eq!(body["category"], "deploys");
        assert_eq!(body["service"]["id"], "s1");
        assert_eq!(body["deploy"], 42);
        assert_eq!(body["test"], false);
        assert_eq!(Message::test("ops").webhook()["test"], true);
    }

    #[test]
    fn email_has_text_and_escaped_html() {
        let (subject, text, html) = failed_deploy().message().email();
        assert!(subject.starts_with(&format!("[{}] Deploy failed", brand::NAME)));
        assert!(text.contains(&format!("$ {} deployments api", brand::CLI_BIN)));
        assert!(html.contains("&lt;oops&gt;"));
        assert!(!html.contains("<oops>"));
    }

    #[test]
    fn long_details_are_clipped() {
        let m = Event::new(NotifyEvent::DeployFailed)
            .detail("x".repeat(5000))
            .message();
        assert!(m.body.unwrap().chars().count() <= 901);
    }
}
