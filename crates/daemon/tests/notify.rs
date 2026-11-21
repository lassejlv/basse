//! Notifications against local mocks: an HTTP server standing in for webhooks and a Mailpit
//! SMTP sink. No real credentials are involved.

mod support;

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, Uri};
use common::api::{CreateNotifyChannel, CreateNotifyRule, CreateSecret, UpdateNotifyRule};
use common::notify::*;
use common::{SecretValue, brand};
use hmac::{Hmac, Mac as _};
use sha2::Sha256;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use support::{Daemon, mailpit};

#[derive(Debug, Clone)]
struct Hit {
    path: String,
    headers: HeaderMap,
    body: Bytes,
}

/// Records every request. `/flaky` answers 500 the first time and 204 after.
async fn mock() -> (String, Arc<Mutex<Vec<Hit>>>) {
    let hits = Arc::new(Mutex::new(Vec::<Hit>::new()));
    let recorded = hits.clone();
    let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
        let recorded = recorded.clone();
        async move {
            let mut hits = recorded.lock().unwrap();
            let path = uri.path().to_string();
            let earlier = hits.iter().filter(|h| h.path == path).count();
            hits.push(Hit {
                path: path.clone(),
                headers,
                body,
            });
            if path == "/flaky" && earlier == 0 {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::NO_CONTENT
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, hits)
}

async fn wait_for<T>(what: &str, mut check: impl AsyncFnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(v) = check().await {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn webhook(url: String, secret: Option<&str>) -> ChannelConfig {
    ChannelConfig::Webhook {
        url,
        secret: secret.map(Into::into),
        headers: vec![WebhookHeader {
            name: "Authorization".into(),
            value: "Bearer header-credential".into(),
        }],
    }
}

fn secret(name: &str) -> CreateSecret {
    CreateSecret {
        name: name.into(),
        value: SecretValue::Generic {
            value: "plaintext-secret-value".into(),
        },
    }
}

#[tokio::test]
async fn events_reach_channels_through_rules() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let c = daemon.client();
    let (url, hits) = mock().await;

    let ch = c
        .create_notify_channel(&CreateNotifyChannel {
            name: "hooks".into(),
            config: webhook(format!("{url}/hook?token=url-credential"), Some("hmac-key")),
            enabled: true,
        })
        .await
        .unwrap();
    let listed = serde_json::to_string(&c.notify_channels().await.unwrap()).unwrap();
    for credential in ["hmac-key", "header-credential", "url-credential"] {
        assert!(
            !listed.contains(credential),
            "{credential} leaked: {listed}"
        );
    }
    assert_eq!(ch.kind, ChannelKind::Webhook);
    assert_eq!(
        ch.target,
        format!("{}/…", url.trim_start_matches("http://"))
    );

    // A test message is sent at once and signed.
    let test = c.test_notify_channel(&ch.id).await.unwrap();
    assert_eq!(test.status, DeliveryStatus::Sent, "{:?}", test.error);
    let hit = hits.lock().unwrap()[0].clone();
    assert_eq!(hit.path, "/hook");
    assert_eq!(hit.headers["authorization"], "Bearer header-credential");
    let mut mac = Hmac::<Sha256>::new_from_slice(b"hmac-key").unwrap();
    mac.update(&hit.body);
    let expected = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let header = format!("x-{}-signature", brand::NAME.to_lowercase());
    assert_eq!(hit.headers[header.as_str()], expected.as_str());
    let body: serde_json::Value = serde_json::from_slice(&hit.body).unwrap();
    assert_eq!(body["test"], true);
    assert!(
        c.notify_channel(&ch.id)
            .await
            .unwrap()
            .last_test
            .unwrap()
            .ok
    );

    // A rule sends a real event; the rate limit drops the next one.
    let rule = c
        .create_notify_rule(&CreateNotifyRule {
            channel_id: ch.id.clone(),
            scope: RuleScope::Instance,
            preset: None,
            events: vec![NotifyEvent::SecretChanged],
            enabled: true,
            dedupe_secs: Some(0),
            max_per_hour: Some(1),
        })
        .await
        .unwrap();
    assert_eq!(rule.scope_label, "instance");
    c.create_secret(&secret("first")).await.unwrap();
    let sent = wait_for("the secret event", async || {
        hits.lock()
            .unwrap()
            .iter()
            .find(|h| String::from_utf8_lossy(&h.body).contains("security.secret_changed"))
            .cloned()
    })
    .await;
    let body: serde_json::Value = serde_json::from_slice(&sent.body).unwrap();
    assert_eq!(body["event"], "security.secret_changed");
    assert_eq!(body["subject"], "first");
    assert!(!String::from_utf8_lossy(&sent.body).contains("plaintext-secret-value"));
    c.create_secret(&secret("second")).await.unwrap();
    let suppressed = wait_for("the suppressed delivery", async || {
        c.notify_deliveries(10, None, Some(&rule.id))
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.status == DeliveryStatus::Suppressed)
    })
    .await;
    assert!(suppressed.error.unwrap().starts_with("rate limit"));
    assert_eq!(suppressed.subject, "second");

    // A failing endpoint is retried with backoff.
    let flaky = c
        .create_notify_channel(&CreateNotifyChannel {
            name: "flaky".into(),
            config: webhook(format!("{url}/flaky"), None),
            enabled: true,
        })
        .await
        .unwrap();
    let flaky_rule = c
        .create_notify_rule(&CreateNotifyRule {
            channel_id: flaky.id.clone(),
            scope: RuleScope::Instance,
            preset: Some("everything".into()),
            events: vec![NotifyEvent::SecretRevealed],
            enabled: true,
            dedupe_secs: None,
            max_per_hour: None,
        })
        .await
        .unwrap();
    assert_eq!(flaky_rule.preset.as_deref(), Some("everything"));
    let first = c.secrets().await.unwrap().into_iter().next().unwrap();
    c.reveal_secret(&first.id).await.unwrap();
    let retried = wait_for("the retried delivery", async || {
        c.notify_deliveries(10, Some(&flaky.id), None)
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.status == DeliveryStatus::Sent)
    })
    .await;
    assert_eq!(retried.attempts, 2);
    assert_eq!(retried.event, Some(NotifyEvent::SecretRevealed));

    // A disabled rule sends nothing; removing a channel removes its rules.
    c.update_notify_rule(
        &rule.id,
        &UpdateNotifyRule {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    c.delete_notify_channel(&flaky.id).await.unwrap();
    let rules = c.notify_rules().await.unwrap();
    assert_eq!(rules.len(), 1);
    assert!(!rules[0].enabled);
}

#[tokio::test]
async fn invalid_channels_and_rules_are_rejected() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let c = daemon.client();
    let err = c
        .create_notify_channel(&CreateNotifyChannel {
            name: "bad".into(),
            config: ChannelConfig::Discord {
                webhook_url: "discord.com/api/webhooks/1".into(),
            },
            enabled: true,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("https://"), "{err}");
    let ch = c
        .create_notify_channel(&CreateNotifyChannel {
            name: "ok".into(),
            config: ChannelConfig::Ntfy {
                server: "http://127.0.0.1:9".into(),
                topic: "t".into(),
                token: None,
            },
            enabled: true,
        })
        .await
        .unwrap();
    let rule = |events: Vec<NotifyEvent>, preset: Option<&str>, scope| CreateNotifyRule {
        channel_id: ch.id.clone(),
        scope,
        preset: preset.map(Into::into),
        events,
        enabled: true,
        dedupe_secs: None,
        max_per_hour: None,
    };
    assert!(
        c.create_notify_rule(&rule(vec![], None, RuleScope::Instance))
            .await
            .is_err()
    );
    assert!(
        c.create_notify_rule(&rule(
            vec![NotifyEvent::DeployFailed],
            Some("nope"),
            RuleScope::Instance
        ))
        .await
        .is_err()
    );
    assert!(
        c.create_notify_rule(&rule(
            vec![NotifyEvent::DeployFailed],
            None,
            RuleScope::Project {
                id: "missing".into()
            }
        ))
        .await
        .is_err()
    );
    let test = c.test_notify_channel(&ch.id).await.unwrap();
    assert_eq!(test.status, DeliveryStatus::Failed);
    assert!(test.error.is_some());
    let catalog = c.notify_catalog().await.unwrap();
    assert_eq!(catalog.events.len(), NotifyEvent::ALL.len());
    assert_eq!(catalog.presets.len(), PRESETS.len());
}

#[tokio::test]
async fn smtp_channels_deliver_to_a_real_smtp_server() {
    let Some(daemon) = Daemon::start().await else {
        return;
    };
    let Some(sink) = mailpit::server() else {
        return;
    };
    let c = daemon.client();
    let ch = c
        .create_notify_channel(&CreateNotifyChannel {
            name: "mail".into(),
            config: ChannelConfig::Email {
                provider: EmailProvider::Smtp {
                    host: "127.0.0.1".into(),
                    port: sink.smtp_port,
                    security: SmtpSecurity::None,
                    username: None,
                    password: None,
                },
                from: "Ops <ops@example.com>".into(),
                to: vec!["oncall@example.com".into()],
            },
            enabled: true,
        })
        .await
        .unwrap();
    let test = c.test_notify_channel(&ch.id).await.unwrap();
    assert_eq!(test.status, DeliveryStatus::Sent, "{:?}", test.error);
    let messages: serde_json::Value = reqwest::get(format!("{}/api/v1/messages", sink.api))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let subjects: Vec<&str> = messages["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["Subject"].as_str().unwrap())
        .collect();
    assert!(
        subjects.contains(&format!("[{}] Test notification · mail", brand::NAME).as_str()),
        "{subjects:?}"
    );
}
