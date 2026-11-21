//! First-run setup of a new instance: the steps a new operator walks through, and what the
//! daemon remembers about them.
//!
//! The daemon stores which steps were skipped and whether the flow was finished or closed;
//! whether a step is done comes from the instance itself (a verified domain, a ready git source,
//! an enabled backup schedule, a channel with a rule, a live service), so it stays true when the
//! work happens outside the flow.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Welcome,
    Domain,
    Source,
    Backups,
    Notifications,
    FirstApp,
    Done,
}

impl Step {
    pub const ALL: [Self; 7] = [
        Self::Welcome,
        Self::Domain,
        Self::Source,
        Self::Backups,
        Self::Notifications,
        Self::FirstApp,
        Self::Done,
    ];

    /// The steps that do something and count toward the checklist ("setup 3/5").
    pub const TASKS: [Self; 5] = [
        Self::Domain,
        Self::Source,
        Self::Backups,
        Self::Notifications,
        Self::FirstApp,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Domain => "Instance domain",
            Self::Source => "Git source",
            Self::Backups => "Backups",
            Self::Notifications => "Notifications",
            Self::FirstApp => "First app",
            Self::Done => "Done",
        }
    }

    pub fn is_task(self) -> bool {
        Self::TASKS.contains(&self)
    }

    /// Recommended steps keep the Overview checklist up until they are done or skipped.
    pub fn recommended(self) -> bool {
        matches!(self, Self::Domain | Self::FirstApp)
    }

    pub fn optional(self) -> bool {
        matches!(self, Self::Source | Self::Backups | Self::Notifications)
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(i + 1).min(Self::ALL.len() - 1)]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[i.saturating_sub(1)]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Welcome => "welcome",
            Self::Domain => "domain",
            Self::Source => "source",
            Self::Backups => "backups",
            Self::Notifications => "notifications",
            Self::FirstApp => "first_app",
            Self::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|step| step.as_str() == s || step.as_str().replace('_', "-") == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    #[default]
    Todo,
    Done,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingStatus {
    /// Not walked through yet: clients open the flow when they connect.
    #[default]
    Pending,
    /// Walked through to the end.
    Finished,
    /// Closed before the end; the Overview checklist takes over.
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepStatus {
    pub step: Step,
    pub state: StepState,
}

/// `GET /onboarding`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Onboarding {
    pub status: OnboardingStatus,
    /// The step the flow was last on; reopening resumes there.
    pub current: Step,
    /// Every one of [`Step::TASKS`], in order.
    pub steps: Vec<StepStatus>,
    /// The Overview checklist was dismissed.
    pub checklist_dismissed: bool,
    /// The instance already had projects when it upgraded to a version with onboarding, so it
    /// was marked as onboarded without asking.
    #[serde(default)]
    pub automatic: bool,
}

impl Onboarding {
    pub fn state(&self, step: Step) -> StepState {
        self.steps
            .iter()
            .find(|s| s.step == step)
            .map_or(StepState::Todo, |s| s.state)
    }

    pub fn done(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.state == StepState::Done)
            .count()
    }

    pub fn total(&self) -> usize {
        self.steps.len()
    }

    /// Clients open the full-screen flow on connect only while this is true.
    pub fn open_on_connect(&self) -> bool {
        self.status == OnboardingStatus::Pending
    }

    /// The small Overview checklist: after the flow, until every recommended step is done or
    /// skipped, or it is dismissed.
    pub fn show_checklist(&self) -> bool {
        self.status != OnboardingStatus::Pending
            && !self.checklist_dismissed
            && self
                .steps
                .iter()
                .any(|s| s.step.recommended() && s.state == StepState::Todo)
    }

    /// The first task neither done nor skipped, else the last step.
    pub fn first_open(&self) -> Step {
        self.steps
            .iter()
            .find(|s| s.state == StepState::Todo)
            .map_or(Step::Done, |s| s.step)
    }
}

/// The one-click sample of the first-app step: a tiny public image that answers HTTP.
pub const SAMPLE_IMAGE: &str = "traefik/whoami";
pub const SAMPLE_PORT: u16 = 80;
pub const SAMPLE_PROJECT: &str = "hello";
pub const SAMPLE_SERVICE: &str = "whoami";

/// A hostname that resolves to `ip` without any DNS setup, through sslip.io:
/// `whoami-5-75-12-4.sslip.io`. `None` when `ip` is not an IPv4 address.
pub fn sslip_domain(name: &str, ip: &str) -> Option<String> {
    let addr: std::net::Ipv4Addr = ip.parse().ok()?;
    Some(format!("{name}-{}.sslip.io", addr.to_string().replace('.', "-")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn onboarding(status: OnboardingStatus, states: [StepState; 5]) -> Onboarding {
        Onboarding {
            status,
            current: Step::Welcome,
            steps: Step::TASKS
                .into_iter()
                .zip(states)
                .map(|(step, state)| StepStatus { step, state })
                .collect(),
            checklist_dismissed: false,
            automatic: false,
        }
    }

    #[test]
    fn steps_walk_in_order_and_parse() {
        assert_eq!(Step::Welcome.next(), Step::Domain);
        assert_eq!(Step::FirstApp.next(), Step::Done);
        assert_eq!(Step::Done.next(), Step::Done);
        assert_eq!(Step::Welcome.prev(), Step::Welcome);
        for step in Step::ALL {
            assert_eq!(Step::parse(step.as_str()), Some(step));
        }
        assert_eq!(Step::parse("first-app"), Some(Step::FirstApp));
        assert_eq!(Step::parse("nope"), None);
    }

    #[test]
    fn checklist_shows_after_the_flow_until_recommended_steps_are_resolved() {
        use StepState::*;
        let pending = onboarding(OnboardingStatus::Pending, [Todo; 5]);
        assert!(pending.open_on_connect());
        assert!(!pending.show_checklist());

        let mut o = onboarding(OnboardingStatus::Dismissed, [Done, Todo, Todo, Skipped, Todo]);
        assert!(!o.open_on_connect());
        assert!(o.show_checklist());
        assert_eq!((o.done(), o.total()), (1, 5));
        assert_eq!(o.first_open(), Step::Source);

        o.steps[4].state = Done;
        assert!(!o.show_checklist(), "optional steps alone keep it hidden");

        o.steps[4].state = Todo;
        o.checklist_dismissed = true;
        assert!(!o.show_checklist());

        let all = onboarding(OnboardingStatus::Finished, [Done; 5]);
        assert_eq!(all.first_open(), Step::Done);
    }

    #[test]
    fn sample_domains_use_sslip() {
        assert_eq!(
            sslip_domain("whoami", "5.75.12.4").as_deref(),
            Some("whoami-5-75-12-4.sslip.io")
        );
        assert_eq!(sslip_domain("whoami", "2a01::1"), None);
        assert_eq!(sslip_domain("whoami", "<server-ip>"), None);
    }
}
