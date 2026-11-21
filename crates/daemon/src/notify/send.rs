//! Delivery to each kind of channel. Errors never carry a URL or a credential: many channels
//! put their token in the URL.

use super::message::Message;
use common::brand;
use common::notify::{ChannelConfig, EmailProvider, SmtpSecurity, WebhookHeader};
use hmac::{Hmac, Mac as _};
use lettre::message::{Mailbox, MultiPart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor};
use serde_json::Value;
use sha2::Sha256;
use std::time::Duration;

/// How long one attempt may take.
pub const TIMEOUT: Duration = Duration::from_secs(15);

/// Base URLs of the hosted APIs, replaceable so tests can point them at a mock.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub telegram: String,
    pub pushover: String,
    pub resend: String,
    pub cloudflare: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            telegram: "https://api.telegram.org".into(),
            pushover: "https://api.pushover.net".into(),
            resend: "https://api.resend.com".into(),
            cloudflare: "https://api.cloudflare.com/client/v4".into(),
        }
    }
}

/// Why an attempt failed, and whether trying again could help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError {
    pub message: String,
    pub retryable: bool,
}

impl SendError {
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }
}

type Result<T = ()> = std::result::Result<T, SendError>;

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(Duration::from_secs(5))
        .user_agent(format!(
            "{}/{}",
            brand::DAEMON_BIN,
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .expect("the notification HTTP client always builds")
}

/// Name of the header a generic webhook's HMAC signature goes in.
pub fn signature_header() -> String {
    format!("X-{}-Signature", brand::NAME)
}

/// `sha256=<hex>` of the body under the webhook secret.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

pub async fn send(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    config: &ChannelConfig,
    msg: &Message,
) -> Result {
    match config {
        ChannelConfig::Email { provider, from, to } => {
            email(http, endpoints, provider, from, to, msg).await
        }
        ChannelConfig::Discord { webhook_url } => {
            post_json(http, &with_wait(webhook_url), &msg.discord(), &[]).await
        }
        ChannelConfig::Slack { webhook_url } => {
            post_json(http, webhook_url, &msg.slack(), &[]).await
        }
        ChannelConfig::Mattermost { webhook_url } => {
            post_json(http, webhook_url, &msg.mattermost(), &[]).await
        }
        ChannelConfig::GoogleChat { webhook_url } => {
            post_json(http, webhook_url, &msg.google_chat(), &[]).await
        }
        ChannelConfig::Teams { webhook_url } => {
            post_json(http, webhook_url, &msg.teams(), &[]).await
        }
        ChannelConfig::Telegram { bot_token, chat_id } => {
            let url = format!(
                "{}/bot{bot_token}/sendMessage",
                endpoints.telegram.trim_end_matches('/')
            );
            post_json(http, &url, &msg.telegram(chat_id), &[]).await
        }
        ChannelConfig::Ntfy {
            server,
            topic,
            token,
        } => {
            let auth = token.as_ref().map(|t| format!("Bearer {t}"));
            let headers: Vec<(&str, &str)> =
                auth.iter().map(|a| ("Authorization", a.as_str())).collect();
            post_json(
                http,
                server.trim_end_matches('/'),
                &msg.ntfy(topic),
                &headers,
            )
            .await
        }
        ChannelConfig::Gotify { server, app_token } => {
            let url = format!("{}/message", server.trim_end_matches('/'));
            post_json(
                http,
                &url,
                &msg.gotify(),
                &[("X-Gotify-Key", app_token.as_str())],
            )
            .await
        }
        ChannelConfig::Pushover {
            user_key,
            app_token,
        } => {
            let url = format!(
                "{}/1/messages.json",
                endpoints.pushover.trim_end_matches('/')
            );
            post_json(http, &url, &msg.pushover(user_key, app_token), &[]).await
        }
        ChannelConfig::Webhook {
            url,
            secret,
            headers,
        } => webhook(http, url, secret.as_deref(), headers, msg).await,
    }
}

/// Discord answers 204 with no body unless asked to wait; waiting surfaces errors.
fn with_wait(url: &str) -> String {
    if url.contains('?') {
        format!("{url}&wait=true")
    } else {
        format!("{url}?wait=true")
    }
}

async fn webhook(
    http: &reqwest::Client,
    url: &str,
    secret: Option<&str>,
    headers: &[WebhookHeader],
    msg: &Message,
) -> Result {
    let body = serde_json::to_vec(&msg.webhook()).expect("JSON values always serialize");
    let mut req = http
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            format!("X-{}-Event", brand::NAME),
            msg.event.map_or("test", |e| e.id()),
        );
    for h in headers {
        req = req.header(&h.name, &h.value);
    }
    if let Some(secret) = secret.filter(|s| !s.is_empty()) {
        req = req.header(signature_header(), sign(secret, &body));
    }
    check(req.body(body).send().await).await.map(drop)
}

async fn post_json(
    http: &reqwest::Client,
    url: &str,
    body: &Value,
    headers: &[(&str, &str)],
) -> Result {
    let mut req = http.post(url).json(body);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    check(req.send().await).await.map(drop)
}

/// Turns a response into its body on 2xx and a short error otherwise.
async fn check(res: reqwest::Result<reqwest::Response>) -> Result<String> {
    let res = res.map_err(|e| {
        let e = e.without_url();
        let what = if e.is_timeout() {
            "timed out".to_string()
        } else if e.is_connect() {
            format!("cannot connect: {e}")
        } else {
            e.to_string()
        };
        SendError::transient(what)
    })?;
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    if status.is_success() {
        return Ok(text);
    }
    let snippet: String = text.trim().chars().take(300).collect();
    let message = if snippet.is_empty() {
        format!("HTTP {}", status.as_u16())
    } else {
        format!("HTTP {}: {snippet}", status.as_u16())
    };
    if status.is_server_error() || status.as_u16() == 429 || status.as_u16() == 408 {
        Err(SendError::transient(message))
    } else {
        Err(SendError::permanent(message))
    }
}

async fn email(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    provider: &EmailProvider,
    from: &str,
    to: &[String],
    msg: &Message,
) -> Result {
    let (subject, text, html) = msg.email();
    match provider {
        EmailProvider::Smtp {
            host,
            port,
            security,
            username,
            password,
        } => {
            smtp(
                host,
                *port,
                *security,
                username.as_deref().zip(password.as_deref()),
                from,
                to,
                (subject, text, html),
            )
            .await
        }
        EmailProvider::Resend { api_key } => {
            let body = serde_json::json!({
                "from": from, "to": to, "subject": subject, "text": text, "html": html,
            });
            let url = format!("{}/emails", endpoints.resend.trim_end_matches('/'));
            let auth = format!("Bearer {api_key}");
            post_json(http, &url, &body, &[("Authorization", &auth)]).await
        }
        EmailProvider::Cloudflare {
            account_id,
            api_token,
        } => {
            let body = serde_json::json!({
                "from": from, "to": to, "subject": subject, "text": text, "html": html,
            });
            let url = format!(
                "{}/accounts/{account_id}/email/sending/send",
                endpoints.cloudflare.trim_end_matches('/')
            );
            let res = http
                .post(url)
                .bearer_auth(api_token)
                .json(&body)
                .send()
                .await;
            let text = check(res).await?;
            let reply: Value = serde_json::from_str(&text).unwrap_or_default();
            if reply["success"].as_bool() == Some(false) {
                return Err(SendError::permanent(format!(
                    "Cloudflare refused the message: {}",
                    reply["errors"]
                )));
            }
            let bounced = &reply["result"]["permanent_bounces"];
            if bounced.as_array().is_some_and(|b| !b.is_empty()) {
                return Err(SendError::permanent(format!(
                    "permanently bounced: {bounced}"
                )));
            }
            Ok(())
        }
    }
}

async fn smtp(
    host: &str,
    port: u16,
    security: SmtpSecurity,
    login: Option<(&str, &str)>,
    from: &str,
    to: &[String],
    (subject, text, html): (String, String, String),
) -> Result {
    let mailbox = |field: &str, address: &str| {
        address
            .parse::<Mailbox>()
            .map_err(|e| SendError::permanent(format!("invalid {field} address '{address}': {e}")))
    };
    let mut builder = lettre::Message::builder()
        .from(mailbox("from", from)?)
        .subject(subject);
    for address in to {
        builder = builder.to(mailbox("recipient", address)?);
    }
    let email = builder
        .multipart(MultiPart::alternative_plain_html(text, html))
        .map_err(|e| SendError::permanent(format!("cannot build the email: {e}")))?;
    let transport = match security {
        SmtpSecurity::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
        SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
        SmtpSecurity::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
            host,
        )),
    }
    .map_err(|e| SendError::permanent(format!("invalid SMTP host: {e}")))?;
    let mut transport = transport.port(port).timeout(Some(TIMEOUT));
    if let Some((user, pass)) = login {
        transport = transport.credentials(Credentials::new(user.into(), pass.into()));
    }
    transport.build().send(email).await.map(drop).map_err(|e| {
        let message = format!("SMTP: {e}");
        if e.is_permanent() {
            SendError::permanent(message)
        } else {
            SendError::transient(message)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Event;
    use axum::Router;
    use axum::body::Bytes;
    use axum::http::{HeaderMap, StatusCode, Uri};
    use common::notify::NotifyEvent;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    struct Hit {
        path: String,
        headers: HeaderMap,
        body: Bytes,
    }

    impl Hit {
        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).unwrap()
        }

        fn header(&self, name: &str) -> &str {
            self.headers.get(name).unwrap().to_str().unwrap()
        }
    }

    struct Mock {
        url: String,
        hits: Arc<Mutex<Vec<Hit>>>,
    }

    impl Mock {
        fn hits(&self) -> Vec<Hit> {
            self.hits.lock().unwrap().clone()
        }
    }

    /// An HTTP server that records every request and answers with `status` and `reply`.
    async fn mock(status: StatusCode, reply: &'static str) -> Mock {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let recorded = hits.clone();
        let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
            let recorded = recorded.clone();
            async move {
                recorded.lock().unwrap().push(Hit {
                    path: uri.to_string(),
                    headers,
                    body,
                });
                (status, reply)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Mock { url, hits }
    }

    fn endpoints(url: &str) -> Endpoints {
        Endpoints {
            telegram: url.into(),
            pushover: url.into(),
            resend: url.into(),
            cloudflare: url.into(),
        }
    }

    fn message() -> Message {
        Event::new(NotifyEvent::DeployFailed)
            .about("api", "s1")
            .detail("build failed")
            .message()
    }

    async fn deliver(mock: &Mock, config: ChannelConfig) -> Result {
        send(&http_client(), &endpoints(&mock.url), &config, &message()).await
    }

    fn email(provider: EmailProvider) -> ChannelConfig {
        ChannelConfig::Email {
            provider,
            from: "Ops <ops@acme.dev>".into(),
            to: vec!["me@acme.dev".into(), "you@acme.dev".into()],
        }
    }

    #[tokio::test]
    async fn hosted_apis_get_their_documented_requests() {
        let m = mock(
            StatusCode::OK,
            r#"{"ok":true,"success":true,"result":{"permanent_bounces":[]}}"#,
        )
        .await;
        deliver(
            &m,
            ChannelConfig::Telegram {
                bot_token: "123:abc".into(),
                chat_id: "-42".into(),
            },
        )
        .await
        .unwrap();
        deliver(
            &m,
            ChannelConfig::Pushover {
                user_key: "u1".into(),
                app_token: "a1".into(),
            },
        )
        .await
        .unwrap();
        deliver(
            &m,
            email(EmailProvider::Resend {
                api_key: "re_1".into(),
            }),
        )
        .await
        .unwrap();
        deliver(
            &m,
            email(EmailProvider::Cloudflare {
                account_id: "acc1".into(),
                api_token: "cf1".into(),
            }),
        )
        .await
        .unwrap();
        let hits = m.hits();
        assert_eq!(hits[0].path, "/bot123:abc/sendMessage");
        assert_eq!(hits[0].json()["chat_id"], "-42");
        assert_eq!(hits[0].json()["parse_mode"], "HTML");
        assert_eq!(hits[1].path, "/1/messages.json");
        assert_eq!(hits[1].json()["user"], "u1");
        assert_eq!(hits[1].json()["priority"], 1);
        assert_eq!(hits[2].path, "/emails");
        assert_eq!(hits[2].header("authorization"), "Bearer re_1");
        assert_eq!(hits[2].json()["to"][1], "you@acme.dev");
        assert!(
            hits[2].json()["html"]
                .as_str()
                .unwrap()
                .contains("build failed")
        );
        assert_eq!(hits[3].path, "/accounts/acc1/email/sending/send");
        assert_eq!(hits[3].header("authorization"), "Bearer cf1");
        assert_eq!(hits[3].json()["from"], "Ops <ops@acme.dev>");
    }

    #[tokio::test]
    async fn webhooks_post_to_their_url() {
        let m = mock(StatusCode::OK, "ok").await;
        let url = |path: &str| format!("{}{path}", m.url);
        deliver(
            &m,
            ChannelConfig::Discord {
                webhook_url: url("/api/webhooks/1/t"),
            },
        )
        .await
        .unwrap();
        deliver(
            &m,
            ChannelConfig::Slack {
                webhook_url: url("/services/T/B/x"),
            },
        )
        .await
        .unwrap();
        deliver(
            &m,
            ChannelConfig::Teams {
                webhook_url: url("/workflows/x"),
            },
        )
        .await
        .unwrap();
        deliver(
            &m,
            ChannelConfig::Mattermost {
                webhook_url: url("/hooks/x"),
            },
        )
        .await
        .unwrap();
        deliver(
            &m,
            ChannelConfig::GoogleChat {
                webhook_url: url("/v1/spaces/x?key=k"),
            },
        )
        .await
        .unwrap();
        deliver(
            &m,
            ChannelConfig::Ntfy {
                server: url("/"),
                topic: "alerts".into(),
                token: Some("tk".into()),
            },
        )
        .await
        .unwrap();
        deliver(
            &m,
            ChannelConfig::Gotify {
                server: url(""),
                app_token: "g1".into(),
            },
        )
        .await
        .unwrap();
        let hits = m.hits();
        assert_eq!(hits[0].path, "/api/webhooks/1/t?wait=true");
        assert_eq!(hits[0].json()["embeds"][0]["color"], 0xE5534B);
        assert_eq!(hits[1].json()["attachments"][0]["color"], "#E5534B");
        assert_eq!(
            hits[2].json()["attachments"][0]["content"]["type"],
            "AdaptiveCard"
        );
        assert!(
            hits[3].json()["attachments"][0]["title"]
                .as_str()
                .unwrap()
                .starts_with("Deploy failed")
        );
        assert_eq!(hits[4].path, "/v1/spaces/x?key=k");
        assert_eq!(hits[5].path, "/");
        assert_eq!(hits[5].header("authorization"), "Bearer tk");
        assert_eq!(hits[5].json()["topic"], "alerts");
        assert_eq!(hits[5].json()["priority"], 5);
        assert_eq!(hits[6].path, "/message");
        assert_eq!(hits[6].header("x-gotify-key"), "g1");
    }

    #[tokio::test]
    async fn generic_webhooks_are_signed() {
        let m = mock(StatusCode::NO_CONTENT, "").await;
        let config = ChannelConfig::Webhook {
            url: format!("{}/hook", m.url),
            secret: Some("s3cret".into()),
            headers: vec![WebhookHeader {
                name: "X-Team".into(),
                value: "ops".into(),
            }],
        };
        deliver(&m, config).await.unwrap();
        let hit = &m.hits()[0];
        assert_eq!(hit.header("x-team"), "ops");
        assert_eq!(
            hit.header(&format!("x-{}-event", brand::NAME.to_lowercase())),
            "deploy.failed"
        );
        let signature = hit.header(&signature_header().to_lowercase());
        assert_eq!(signature, sign("s3cret", &hit.body));
        assert!(signature.starts_with("sha256=") && signature.len() == 7 + 64);
        assert_eq!(hit.json()["event"], "deploy.failed");
    }

    #[tokio::test]
    async fn errors_say_whether_a_retry_helps() {
        let down = mock(StatusCode::BAD_GATEWAY, "upstream down").await;
        let err = deliver(
            &down,
            ChannelConfig::Slack {
                webhook_url: down.url.clone(),
            },
        )
        .await
        .unwrap_err();
        assert!(err.retryable);
        assert_eq!(err.message, "HTTP 502: upstream down");
        let bad = mock(StatusCode::NOT_FOUND, "no_such_hook").await;
        let err = deliver(
            &bad,
            ChannelConfig::Slack {
                webhook_url: bad.url.clone(),
            },
        )
        .await
        .unwrap_err();
        assert!(!err.retryable);
        let refused = mock(
            StatusCode::OK,
            r#"{"success":false,"errors":[{"code":10105}]}"#,
        )
        .await;
        let err = deliver(
            &refused,
            email(EmailProvider::Cloudflare {
                account_id: "a".into(),
                api_token: "t".into(),
            }),
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("10105"), "{}", err.message);
    }

    #[tokio::test]
    async fn connection_errors_never_show_the_url() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = ChannelConfig::Telegram {
            bot_token: "123:topsecret".into(),
            chat_id: "1".into(),
        };
        let err = send(
            &http_client(),
            &endpoints(&format!("http://127.0.0.1:{port}")),
            &config,
            &message(),
        )
        .await
        .unwrap_err();
        assert!(err.retryable);
        assert!(!err.message.contains("topsecret"), "{}", err.message);
    }
}
