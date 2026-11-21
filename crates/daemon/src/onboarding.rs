//! First-run setup state. Skips and the flow's status are stored in `daemon_meta`; whether a
//! step is done is read from the instance on every request, so work done outside the flow counts.

use crate::db::Db;
use crate::state::AppState;
use anyhow::Result;
use common::GitSourceStatus;
use common::api::UpdateOnboarding;
use common::onboarding::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const KEY: &str = "onboarding";

/// What the daemon remembers; everything else is derived.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub status: OnboardingStatus,
    pub current: Option<Step>,
    #[serde(default)]
    pub skipped: BTreeSet<Step>,
    #[serde(default)]
    pub checklist_dismissed: bool,
    #[serde(default)]
    pub automatic: bool,
}

/// Which steps the instance has already done, whoever did them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Facts {
    pub domain: bool,
    pub source: bool,
    pub backups: bool,
    pub notifications: bool,
    pub first_app: bool,
}

impl Facts {
    fn done(self, step: Step) -> bool {
        match step {
            Step::Domain => self.domain,
            Step::Source => self.source,
            Step::Backups => self.backups,
            Step::Notifications => self.notifications,
            Step::FirstApp => self.first_app,
            Step::Welcome | Step::Done => false,
        }
    }
}

impl Stored {
    /// The state of a store without a record: an instance with projects predates onboarding and
    /// counts as onboarded, so nobody gets the flow on top of real work.
    pub fn initial(has_projects: bool) -> Self {
        if has_projects {
            Self {
                status: OnboardingStatus::Finished,
                current: Some(Step::Done),
                checklist_dismissed: true,
                automatic: true,
                ..Self::default()
            }
        } else {
            Self::default()
        }
    }

    pub fn view(&self, facts: Facts) -> Onboarding {
        Onboarding {
            status: self.status,
            current: self.current.unwrap_or(Step::Welcome),
            steps: Step::TASKS
                .into_iter()
                .map(|step| StepStatus {
                    step,
                    state: if facts.done(step) {
                        StepState::Done
                    } else if self.skipped.contains(&step) {
                        StepState::Skipped
                    } else {
                        StepState::Todo
                    },
                })
                .collect(),
            checklist_dismissed: self.checklist_dismissed,
            automatic: self.automatic,
        }
    }

    pub fn apply(&mut self, body: &UpdateOnboarding) -> Result<(), String> {
        if let Some(step) = body.skip.iter().chain(&body.unskip).find(|s| !s.is_task()) {
            return Err(format!("{} cannot be skipped", step.label()));
        }
        for step in &body.skip {
            self.skipped.insert(*step);
        }
        for step in &body.unskip {
            self.skipped.remove(step);
        }
        if let Some(current) = body.current {
            self.current = Some(current);
        }
        if let Some(status) = body.status {
            self.status = status;
        }
        if let Some(dismissed) = body.checklist_dismissed {
            self.checklist_dismissed = dismissed;
        }
        Ok(())
    }
}

/// Writes the initial record once, after the store is open and any SQLite import has run.
pub async fn init(db: &Db) -> Result<()> {
    if db.meta(KEY).await?.is_some() {
        return Ok(());
    }
    let stored = Stored::initial(!db.projects().await?.is_empty());
    if stored.automatic {
        tracing::info!("existing projects found: marking this instance as onboarded");
    }
    save(db, &stored).await
}

pub async fn load(db: &Db) -> Result<Stored> {
    Ok(match db.meta(KEY).await? {
        Some(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
            tracing::warn!("ignoring unreadable onboarding state: {e}");
            Stored::default()
        }),
        None => Stored::default(),
    })
}

pub async fn save(db: &Db, stored: &Stored) -> Result<()> {
    db.set_meta(KEY, &serde_json::to_string(stored)?).await
}

pub async fn facts(state: &AppState) -> Result<Facts> {
    let config = state.config();
    let rules = state.db.notify_rules().await?;
    let channels = state.db.notify_channels().await?;
    Ok(Facts {
        domain: config.domain.is_some() && config.domain_verified_at.is_some(),
        source: state
            .db
            .sources()
            .await?
            .iter()
            .any(|r| r.source.status == GitSourceStatus::Ready),
        backups: state.db.instance_backup_schedule().await?.0.enabled,
        notifications: channels.iter().filter(|c| c.enabled).any(|c| {
            rules
                .iter()
                .any(|r| r.enabled && r.channel_id == c.id && !r.events.is_empty())
        }),
        first_app: state
            .db
            .services()
            .await?
            .iter()
            .any(|r| r.service.live_deployment_id.is_some()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(f: impl FnOnce(&mut UpdateOnboarding)) -> UpdateOnboarding {
        let mut body = UpdateOnboarding::default();
        f(&mut body);
        body
    }

    #[test]
    fn a_new_instance_starts_pending_on_welcome() {
        let o = Stored::initial(false).view(Facts::default());
        assert_eq!(o.status, OnboardingStatus::Pending);
        assert_eq!(o.current, Step::Welcome);
        assert!(o.open_on_connect());
        assert_eq!(o.steps.len(), 5);
        assert!(o.steps.iter().all(|s| s.state == StepState::Todo));
    }

    #[test]
    fn an_upgraded_instance_with_projects_is_marked_onboarded() {
        let stored = Stored::initial(true);
        let o = stored.view(Facts::default());
        assert!(o.automatic);
        assert_eq!(o.status, OnboardingStatus::Finished);
        assert!(!o.open_on_connect());
        assert!(
            !o.show_checklist(),
            "no checklist on top of real work either"
        );
    }

    #[test]
    fn walking_the_flow_skips_resumes_and_finishes() {
        let mut stored = Stored::initial(false);
        let mut facts = Facts::default();

        stored
            .apply(&patch(|b| {
                b.current = Some(Step::Source);
                b.skip = vec![Step::Domain];
            }))
            .unwrap();
        let o = stored.view(facts);
        assert_eq!(o.current, Step::Source);
        assert_eq!(o.state(Step::Domain), StepState::Skipped);

        // Closing early hands over to the checklist; it resumes where it was.
        stored
            .apply(&patch(|b| b.status = Some(OnboardingStatus::Dismissed)))
            .unwrap();
        let o = stored.view(facts);
        assert!(!o.open_on_connect());
        assert!(
            o.show_checklist(),
            "first app is recommended and still open"
        );
        assert_eq!(o.current, Step::Source);

        // Work done anywhere counts, even for a skipped step.
        facts.domain = true;
        facts.first_app = true;
        let o = stored.view(facts);
        assert_eq!(o.state(Step::Domain), StepState::Done);
        assert_eq!(o.done(), 2);
        assert!(!o.show_checklist(), "every recommended step is done");

        stored
            .apply(&patch(|b| {
                b.unskip = vec![Step::Domain];
                b.status = Some(OnboardingStatus::Finished);
                b.current = Some(Step::Done);
                b.checklist_dismissed = Some(true);
            }))
            .unwrap();
        assert!(stored.skipped.is_empty());
        let o = stored.view(Facts::default());
        assert_eq!(o.status, OnboardingStatus::Finished);
        assert!(o.checklist_dismissed);
    }

    #[test]
    fn only_tasks_can_be_skipped() {
        let mut stored = Stored::default();
        for step in [Step::Welcome, Step::Done] {
            let err = stored.apply(&patch(|b| b.skip = vec![step])).unwrap_err();
            assert!(err.contains("cannot be skipped"), "{err}");
        }
        assert_eq!(
            stored,
            Stored::default(),
            "a rejected patch changes nothing"
        );
    }

    #[test]
    fn stored_state_round_trips_and_tolerates_old_fields() {
        let mut stored = Stored::initial(false);
        stored.skipped.insert(Step::Backups);
        let json = serde_json::to_string(&stored).unwrap();
        assert_eq!(serde_json::from_str::<Stored>(&json).unwrap(), stored);
        let minimal: Stored = serde_json::from_str(r#"{"status":"dismissed"}"#).unwrap();
        assert_eq!(minimal.status, OnboardingStatus::Dismissed);
        assert!(minimal.skipped.is_empty());
    }

    #[tokio::test]
    async fn init_marks_existing_instances_once() {
        let Some(db) = crate::db::tests::test_db().await else {
            return;
        };
        let project = common::Project {
            id: "p1".into(),
            name: "shop".into(),
            description: None,
            created_at: chrono::Utc::now(),
        };
        let env = common::Environment {
            id: "e1".into(),
            project_id: "p1".into(),
            name: "production".into(),
            created_at: chrono::Utc::now(),
        };
        db.insert_project(&project, &env).await.unwrap();
        init(&db).await.unwrap();
        let stored = load(&db).await.unwrap();
        assert!(stored.automatic);
        assert_eq!(stored.status, OnboardingStatus::Finished);

        // A later start never rewrites what the operator did since.
        let mut changed = stored.clone();
        changed.status = OnboardingStatus::Pending;
        save(&db, &changed).await.unwrap();
        init(&db).await.unwrap();
        assert_eq!(load(&db).await.unwrap().status, OnboardingStatus::Pending);
    }
}
