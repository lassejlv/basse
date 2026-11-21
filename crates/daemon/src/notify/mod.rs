//! Notifications: places that detect something emit an [`Event`]; the dispatcher matches it
//! against rules and delivers to channels in the background, with retries, dedupe and a rate
//! limit per rule, and records every attempt in the delivery log.

pub mod message;
pub mod monitor;
pub mod send;

pub use message::{Event, Message, Ref};

use crate::state::AppState;
use anyhow::{Context as _, Result};
use chrono::Utc;
use common::Service;
use common::notify::{
    ChannelConfig, ChannelTest, DeliveryStatus, NotifyChannel, NotifyDelivery, NotifyRule,
    RuleScope,
};
use send::{Endpoints, SendError};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Waits before the second, third and fourth attempt.
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
];
/// Deliveries kept in the log.
const KEEP_DELIVERIES: i64 = 1000;
const HOUR: Duration = Duration::from_secs(3600);

/// Dedupe and rate-limit memory. It lives in memory: after a restart every rule starts fresh.
#[derive(Default)]
struct Limits {
    /// Per rule, when each message of the last hour was admitted.
    sent: HashMap<String, VecDeque<Instant>>,
    /// Per rule, event and subject, when it was last admitted.
    last: HashMap<(String, &'static str, String), Instant>,
}

pub struct Notifier {
    tx: mpsc::UnboundedSender<Event>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
    limits: Mutex<Limits>,
    http: reqwest::Client,
    endpoints: Endpoints,
    backoff: [Duration; 3],
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Default for Notifier {
    fn default() -> Self {
        Self::new(Endpoints::default(), BACKOFF)
    }
}

impl Notifier {
    pub fn new(endpoints: Endpoints, backoff: [Duration; 3]) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
            limits: Mutex::default(),
            http: send::http_client(),
            endpoints,
            backoff,
        }
    }

    /// Queues an event for the dispatcher. Never blocks and never fails the caller.
    pub fn emit(&self, event: Event) {
        let _ = self.tx.send(event);
    }

    /// Whether a rule may send this event now; the reason when it may not.
    fn admit(&self, rule: &NotifyRule, event: &Event, now: Instant) -> Result<(), String> {
        let mut limits = lock(&self.limits);
        let key = (rule.id.clone(), event.kind.id(), event.key.clone());
        if rule.dedupe_secs > 0
            && let Some(last) = limits.last.get(&key)
            && now.duration_since(*last) < Duration::from_secs(u64::from(rule.dedupe_secs))
        {
            return Err(format!(
                "duplicate: the same event was sent {}s ago (dedupe window {}s)",
                now.duration_since(*last).as_secs(),
                rule.dedupe_secs
            ));
        }
        let window = limits.sent.entry(rule.id.clone()).or_default();
        while window
            .front()
            .is_some_and(|t| now.duration_since(*t) >= HOUR)
        {
            window.pop_front();
        }
        if rule.max_per_hour > 0 && window.len() >= rule.max_per_hour as usize {
            return Err(format!(
                "rate limit: {} messages in the last hour",
                rule.max_per_hour
            ));
        }
        window.push_back(now);
        limits.last.insert(key, now);
        if limits.last.len() > 10_000 {
            limits
                .last
                .retain(|_, at| now.duration_since(*at) < Duration::from_secs(86_400));
        }
        Ok(())
    }
}

/// Whether an event falls inside a rule's scope.
fn in_scope(scope: &RuleScope, event: &Event) -> bool {
    let is = |r: &Option<Ref>, id: &str| r.as_ref().is_some_and(|r| r.id == id);
    match scope {
        RuleScope::Instance => true,
        RuleScope::Project { id } => is(&event.project, id),
        RuleScope::Environment { id } => is(&event.environment, id),
        RuleScope::Service { id } => is(&event.service, id),
    }
}

impl AppState {
    /// Announces an event to every matching rule. Returns at once.
    pub fn notify(&self, event: Event) {
        self.notifier.emit(event);
    }

    /// Announces an event about a service, filling in its project, environment and link.
    pub async fn notify_service(&self, service: &Service, mut event: Event) {
        let env = self
            .db
            .environment(&service.environment_id)
            .await
            .ok()
            .flatten();
        let project = match &env {
            Some(env) => self.db.project(&env.project_id).await.ok().flatten(),
            None => None,
        };
        event.project = project.map(|p| Ref {
            id: p.id,
            name: p.name,
        });
        event.environment = env.map(|e| Ref {
            id: e.id,
            name: e.name,
        });
        event.service = Some(Ref {
            id: service.id.clone(),
            name: service.name.clone(),
        });
        if event.key.is_empty() {
            event.key.clone_from(&service.id);
        }
        if event.link.is_none() {
            let domain = match &service.primary_domain {
                Some(domain) => Some(domain.clone()),
                None => self
                    .db
                    .domains(&service.id)
                    .await
                    .ok()
                    .and_then(|d| d.into_iter().next().map(|d| d.host)),
            };
            event.link = domain.map(|d| format!("https://{d}"));
        }
        self.notify(event);
    }

    /// The decrypted config of a channel.
    pub async fn channel_config(&self, id: &str) -> Result<ChannelConfig> {
        let sealed = self
            .db
            .notify_channel_sealed(id)
            .await?
            .with_context(|| format!("channel '{id}' not found"))?;
        let json = self.secrets.key_open(&channel_aad(id), &sealed)?;
        serde_json::from_slice(&json).context("the decrypted channel config is not valid")
    }
}

/// Associated data a channel's config is sealed with.
pub fn channel_aad(id: &str) -> String {
    format!("notify_channel:{id}")
}

/// Starts the dispatcher. Call once.
pub fn spawn_dispatcher(state: Arc<AppState>) {
    let Some(mut rx) = lock(&state.notifier.rx).take() else {
        return;
    };
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Err(e) = dispatch(&state, event).await {
                tracing::warn!("cannot dispatch a notification: {e:#}");
            }
        }
    });
}

async fn dispatch(state: &Arc<AppState>, event: Event) -> Result<()> {
    let rules = state.db.notify_rules().await?;
    let rules: Vec<NotifyRule> = rules
        .into_iter()
        .filter(|r| r.enabled && r.events.contains(&event.kind) && in_scope(&r.scope, &event))
        .collect();
    if rules.is_empty() {
        return Ok(());
    }
    let channels: HashMap<String, NotifyChannel> = state
        .db
        .notify_channels()
        .await?
        .into_iter()
        .map(|c| (c.id.clone(), c))
        .collect();
    let message = event.message();
    for rule in rules {
        let Some(channel) = channels.get(&rule.channel_id).filter(|c| c.enabled) else {
            continue;
        };
        let mut delivery = NotifyDelivery {
            id: 0,
            at: Utc::now(),
            channel_id: channel.id.clone(),
            channel_name: channel.name.clone(),
            rule_id: Some(rule.id.clone()),
            event: Some(event.kind),
            subject: message.subject.clone(),
            title: message.title.clone(),
            status: DeliveryStatus::Suppressed,
            attempts: 0,
            error: None,
            latency_ms: None,
        };
        if let Err(reason) = state.notifier.admit(&rule, &event, Instant::now()) {
            delivery.error = Some(reason);
            record(state, &mut delivery).await;
            continue;
        }
        tokio::spawn(deliver(state.clone(), delivery, message.clone()));
    }
    Ok(())
}

async fn record(state: &AppState, delivery: &mut NotifyDelivery) {
    let result = if delivery.id == 0 {
        match state.db.insert_delivery(delivery).await {
            Ok(id) => {
                delivery.id = id;
                state.db.prune_deliveries(KEEP_DELIVERIES).await
            }
            Err(e) => Err(e),
        }
    } else {
        state.db.update_delivery(delivery).await
    };
    if let Err(e) = result {
        tracing::warn!("cannot record a notification delivery: {e:#}");
    }
}

/// One attempt: decrypt the channel's config and send.
async fn attempt(state: &AppState, channel_id: &str, message: &Message) -> Result<u64, SendError> {
    let config = state
        .channel_config(channel_id)
        .await
        .map_err(|e| SendError {
            message: format!("{e:#}"),
            retryable: false,
        })?;
    let started = Instant::now();
    send::send(
        &state.notifier.http,
        &state.notifier.endpoints,
        &config,
        message,
    )
    .await?;
    Ok(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX))
}

/// Sends with retries and keeps the delivery's log row current.
async fn deliver(state: Arc<AppState>, mut delivery: NotifyDelivery, message: Message) {
    loop {
        delivery.attempts += 1;
        let backoff = state
            .notifier
            .backoff
            .get(delivery.attempts as usize - 1)
            .copied();
        match attempt(&state, &delivery.channel_id, &message).await {
            Ok(ms) => {
                delivery.status = DeliveryStatus::Sent;
                delivery.error = None;
                delivery.latency_ms = Some(ms);
                record(&state, &mut delivery).await;
                return;
            }
            Err(e) => {
                delivery.error = Some(e.message);
                match backoff.filter(|_| e.retryable) {
                    Some(wait) => {
                        delivery.status = DeliveryStatus::Retrying;
                        record(&state, &mut delivery).await;
                        tokio::time::sleep(wait).await;
                    }
                    None => {
                        delivery.status = DeliveryStatus::Failed;
                        record(&state, &mut delivery).await;
                        tracing::warn!(
                            channel = %delivery.channel_name,
                            "notification failed: {}",
                            delivery.error.as_deref().unwrap_or_default()
                        );
                        return;
                    }
                }
            }
        }
    }
}

/// Sends a test message once, records it and the channel's `last_test`.
pub async fn test_channel(state: &AppState, mut channel: NotifyChannel) -> Result<NotifyDelivery> {
    let message = Message::test(&channel.name);
    let mut delivery = NotifyDelivery {
        id: 0,
        at: Utc::now(),
        channel_id: channel.id.clone(),
        channel_name: channel.name.clone(),
        rule_id: None,
        event: None,
        subject: channel.name.clone(),
        title: message.title.clone(),
        status: DeliveryStatus::Sent,
        attempts: 1,
        error: None,
        latency_ms: None,
    };
    match attempt(state, &channel.id, &message).await {
        Ok(ms) => delivery.latency_ms = Some(ms),
        Err(e) => {
            delivery.status = DeliveryStatus::Failed;
            delivery.error = Some(e.message);
        }
    }
    record(state, &mut delivery).await;
    channel.last_test = Some(ChannelTest {
        ok: delivery.status == DeliveryStatus::Sent,
        at: delivery.at,
        error: delivery.error.clone(),
        latency_ms: delivery.latency_ms,
    });
    state.db.save_notify_channel(&channel, None).await?;
    Ok(delivery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::notify::NotifyEvent;

    fn rule(dedupe_secs: u32, max_per_hour: u32) -> NotifyRule {
        NotifyRule {
            id: "r1".into(),
            channel_id: "c1".into(),
            scope: RuleScope::Instance,
            scope_label: String::new(),
            preset: None,
            events: vec![NotifyEvent::ServiceCrashed],
            enabled: true,
            dedupe_secs,
            max_per_hour,
            created_at: Utc::now(),
        }
    }

    fn crash(service: &str) -> Event {
        let mut e = Event::new(NotifyEvent::ServiceCrashed);
        e.key = service.into();
        e
    }

    #[test]
    fn dedupe_drops_the_same_event_about_the_same_subject() {
        let n = Notifier::default();
        let r = rule(300, 0);
        let t0 = Instant::now();
        assert!(n.admit(&r, &crash("a"), t0).is_ok());
        let err = n
            .admit(&r, &crash("a"), t0 + Duration::from_secs(10))
            .unwrap_err();
        assert!(err.starts_with("duplicate"), "{err}");
        assert!(
            n.admit(&r, &crash("b"), t0 + Duration::from_secs(10))
                .is_ok()
        );
        assert!(
            n.admit(&r, &crash("a"), t0 + Duration::from_secs(301))
                .is_ok()
        );
    }

    #[test]
    fn rate_limit_caps_messages_per_hour() {
        let n = Notifier::default();
        let r = rule(0, 3);
        let t0 = Instant::now();
        for i in 0..3 {
            assert!(n.admit(&r, &crash(&i.to_string()), t0).is_ok());
        }
        let err = n.admit(&r, &crash("x"), t0).unwrap_err();
        assert!(err.starts_with("rate limit"), "{err}");
        assert!(n.admit(&r, &crash("x"), t0 + HOUR).is_ok());
    }

    #[test]
    fn scopes_match_by_id() {
        let mut e = crash("s1");
        e.project = Some(Ref {
            id: "p1".into(),
            name: "acme".into(),
        });
        e.service = Some(Ref {
            id: "s1".into(),
            name: "api".into(),
        });
        assert!(in_scope(&RuleScope::Instance, &e));
        assert!(in_scope(&RuleScope::Project { id: "p1".into() }, &e));
        assert!(!in_scope(&RuleScope::Project { id: "p2".into() }, &e));
        assert!(!in_scope(&RuleScope::Environment { id: "e1".into() }, &e));
        assert!(in_scope(&RuleScope::Service { id: "s1".into() }, &e));
        let server = Event::new(NotifyEvent::ServerUnreachable);
        assert!(!in_scope(&RuleScope::Project { id: "p1".into() }, &server));
    }
}
