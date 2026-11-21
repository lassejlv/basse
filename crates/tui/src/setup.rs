//! First-run setup: a full-screen checklist that walks a new instance from its domain to a first
//! live app. Each step reuses an existing screen or form; the daemon keeps the progress.

use crate::app::*;
use crate::mouse::{self, Hit};
use crate::theme;
use crate::util::{self, truncate};
use crate::view::{self, inside, lines_of, panel, title};
use common::api::{CreateNotifyRule, CreateProject, CreateService, UpdateOnboarding};
use common::notify::{NotifyDelivery, RuleScope, preset};
use common::onboarding::{self, Onboarding, OnboardingStatus, Step, StepState};
use common::*;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Width of the progress rail.
const RAIL_W: u16 = 30;

#[derive(Default)]
pub struct SetupView {
    pub data: Option<Onboarding>,
    /// The step on screen.
    pub step: Step,
    /// Selected row of a step's choices (git source, first app).
    pub choice: usize,
    /// The flow opened by itself once on this connection.
    pub auto_opened: bool,
    /// The channel the notifications step created or found; tests go to it.
    pub channel: Option<Id>,
    pub testing: bool,
    pub test: Option<Result<NotifyDelivery, String>>,
    /// The service the first-app step created.
    pub app: Option<FirstApp>,
    pub busy: Option<&'static str>,
}

pub struct FirstApp {
    pub service_id: Id,
    pub name: String,
}

/// The two ways to a first app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppChoice {
    Sample,
    Github,
}

/// The three ways to connect GitHub, in the order of the Sources screen's keys.
pub const SOURCE_CHOICES: [(&str, &str); 3] = [
    ("GitHub App", "recommended · push deploys, org repos"),
    ("Existing GitHub App", "one you already made on GitHub"),
    ("Personal access token", "quickest · no push deploys"),
];

impl SetupView {
    pub fn state(&self, step: Step) -> StepState {
        self.data
            .as_ref()
            .map_or(StepState::Todo, |o| o.state(step))
    }
}

impl App {
    pub fn load_onboarding(&self) {
        self.spawn(|c| async move {
            Msg::Onboarding(c.onboarding().await.map_err(|e| format!("{e:#}")))
        });
    }

    fn patch_onboarding(&self, body: UpdateOnboarding) {
        self.spawn(|c| async move {
            Msg::Onboarding(
                c.update_onboarding(&body)
                    .await
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    /// A fresh state from the daemon. Opens the flow once per connection while it is pending.
    pub fn onboarding_loaded(&mut self, o: Onboarding) {
        let open = !self.setup.auto_opened
            && o.open_on_connect()
            && self.screen == Screen::Overview
            && self.overlay.is_none();
        let first = self.setup.data.is_none();
        self.setup.data = Some(o);
        if first || open {
            self.setup.auto_opened = true;
        }
        if open {
            self.open_setup();
        }
    }

    /// Opens the flow where it was left, or at the first open step once it was finished.
    pub fn open_setup(&mut self) {
        let step = match &self.setup.data {
            Some(o) if o.status == OnboardingStatus::Finished => o.first_open(),
            Some(o) => o.current,
            None => Step::Welcome,
        };
        self.overlay = None;
        self.screen = Screen::Setup;
        self.go_to_step(step);
    }

    pub fn go_to_step(&mut self, step: Step) {
        self.setup.step = step;
        self.setup.choice = 0;
        self.domain.input = None;
        if self.setup.data.as_ref().is_none_or(|o| o.current != step) {
            self.patch_onboarding(UpdateOnboarding {
                current: Some(step),
                ..Default::default()
            });
        }
        self.load_step();
    }

    /// What the step on screen shows, loaded again on every refresh.
    pub fn load_step(&mut self) {
        match self.setup.step {
            Step::Welcome => self.load_settings(),
            Step::Domain if self.domain.busy.is_none() => self.load_domain(),
            Step::Domain | Step::Done => {}
            Step::Source | Step::FirstApp => self.load_sources(),
            Step::Backups => {
                self.load_storages();
                self.load_settings();
            }
            Step::Notifications => self.load_notify(),
        }
    }

    pub fn next_step(&mut self) {
        let next = self.setup.step.next();
        self.go_to_step(next);
    }

    pub fn skip_step(&mut self) {
        let step = self.setup.step;
        if step.is_task() && self.setup.state(step) == StepState::Todo {
            self.patch_onboarding(UpdateOnboarding {
                skip: vec![step],
                ..Default::default()
            });
        }
        self.next_step();
    }

    /// Esc: leave the flow; it stays reachable from the Overview checklist and the palette.
    pub fn close_setup(&mut self) {
        if self
            .setup
            .data
            .as_ref()
            .is_some_and(|o| o.open_on_connect())
        {
            self.patch_onboarding(UpdateOnboarding {
                status: Some(OnboardingStatus::Dismissed),
                ..Default::default()
            });
        }
        self.leave_setup();
    }

    pub fn finish_setup(&mut self) {
        self.patch_onboarding(UpdateOnboarding {
            status: Some(OnboardingStatus::Finished),
            current: Some(Step::Done),
            ..Default::default()
        });
        self.leave_setup();
    }

    fn leave_setup(&mut self) {
        self.domain.input = None;
        if self
            .deploy
            .as_ref()
            .is_some_and(|d| d.back == Screen::Setup)
        {
            self.close_deploy();
        }
        self.service = None;
        self.screen = Screen::Overview;
        self.refresh();
    }

    pub fn dismiss_checklist(&mut self) {
        self.patch_onboarding(UpdateOnboarding {
            checklist_dismissed: Some(true),
            ..Default::default()
        });
        self.toast("setup checklist hidden · reopen it with : setup");
    }

    /// ⏎ on the step on screen.
    pub fn setup_enter(&mut self) {
        let step = self.setup.step;
        let done = self.setup.state(step) == StepState::Done;
        match step {
            Step::Welcome => self.next_step(),
            Step::Done => self.finish_setup(),
            Step::Source if !done => match self.setup.choice {
                0 => self.form_github_app(),
                1 => self.form_connect_app(),
                _ => self.form_github_token(),
            },
            Step::FirstApp if self.deploy_live() || done => self.next_step(),
            Step::FirstApp if self.setup.app.is_some() => {}
            Step::FirstApp => match self.app_choices().get(self.setup.choice) {
                Some(AppChoice::Sample) => self.create_sample_app(),
                Some(AppChoice::Github) => self.form_first_app(),
                None => {}
            },
            _ if done => self.next_step(),
            Step::Domain => self.error("continue once the domain is verified, or s to skip"),
            _ => self.error("finish the step first, or s to skip"),
        }
    }

    pub fn app_choices(&self) -> Vec<AppChoice> {
        let mut choices = vec![AppChoice::Sample];
        if !self.sources.ready().is_empty() {
            choices.push(AppChoice::Github);
        }
        choices
    }

    pub fn setup_choices(&self) -> usize {
        match self.setup.step {
            Step::Source if self.setup.state(Step::Source) != StepState::Done => {
                SOURCE_CHOICES.len()
            }
            Step::FirstApp if self.setup.app.is_none() => self.app_choices().len(),
            _ => 0,
        }
    }

    pub fn choose(&mut self, row: usize) {
        let n = self.setup_choices();
        if n > 0 {
            self.setup.choice = row.min(n - 1);
        }
    }

    /// The sample app's domain: a name on sslip.io that resolves to this server.
    pub fn sample_domain(&self) -> Option<String> {
        let ip = self.info.as_ref()?.public_ip.as_deref()?;
        onboarding::sslip_domain(onboarding::SAMPLE_SERVICE, ip)
    }

    /// One click to a running app: a project, its production environment and a tiny image.
    fn create_sample_app(&mut self) {
        if self.setup.busy.is_some() {
            return;
        }
        let taken: Vec<String> = self
            .tree
            .iter()
            .flat_map(|t| &t.projects)
            .map(|p| p.project.name.clone())
            .collect();
        let project = unique_name(onboarding::SAMPLE_PROJECT, &taken);
        let domain = self.sample_domain();
        self.setup.busy = Some("creating");
        self.spawn(move |c| async move {
            let result = async {
                let p = c
                    .create_project(&CreateProject {
                        name: project,
                        description: None,
                    })
                    .await?;
                let env = c
                    .tree()
                    .await?
                    .projects
                    .into_iter()
                    .find(|n| n.project.id == p.id)
                    .and_then(|n| n.environments.into_iter().next())
                    .ok_or_else(|| anyhow::anyhow!("the new project has no environment"))?;
                let detail = c
                    .create_service(&CreateService {
                        environment_id: env.environment.id,
                        server_id: None,
                        name: onboarding::SAMPLE_SERVICE.into(),
                        source: Source::Image {
                            image: onboarding::SAMPLE_IMAGE.into(),
                        },
                        port: Some(onboarding::SAMPLE_PORT),
                        command: None,
                        restart: RestartPolicy::default(),
                        health_path: None,
                        env: Vec::new(),
                        domains: domain.into_iter().collect(),
                        volumes: Vec::new(),
                        deploy: true,
                        replicas: None,
                    })
                    .await?;
                let first = c
                    .deployments(&detail.service.id, 1)
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("the deploy did not start"))?;
                anyhow::Ok(first)
            }
            .await;
            match result {
                Ok(d) => Msg::FormDone {
                    toast: format!("{} created · deploying", onboarding::SAMPLE_SERVICE),
                    then: Then::OpenDeploy(Box::new(d)),
                },
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    /// The New service form, creating a project named after the service on submit.
    fn form_first_app(&mut self) {
        self.form_new_service_in(None, "a new project named after the service".into());
    }

    /// A deploy the first-app step started: follow it inside the flow.
    pub fn follow_first_deploy(&mut self, d: Deployment) {
        self.setup.busy = None;
        let service_id = d.service_id.clone();
        self.open_deploy(d);
        if let Some(view) = self.deploy.as_mut() {
            view.back = Screen::Setup;
        }
        self.screen = Screen::Setup;
        let detail = self.details.get(&service_id);
        let name = detail.map_or_else(
            || onboarding::SAMPLE_SERVICE.to_string(),
            |d| d.service.name.clone(),
        );
        self.setup.app = Some(FirstApp {
            service_id: service_id.clone(),
            name,
        });
        self.load_detail(&service_id);
    }

    pub fn deploy_live(&self) -> bool {
        self.deploy.as_ref().is_some_and(|d| {
            d.back == Screen::Setup && d.deployment.status == DeploymentStatus::Live
        })
    }

    /// The first app's address once it has a domain.
    pub fn first_app_url(&self) -> Option<String> {
        let app = self.setup.app.as_ref()?;
        let detail = self.details.get(&app.service_id)?;
        let d = detail.domains.first()?;
        Some(format!(
            "{}://{}",
            if d.tls { "https" } else { "http" },
            d.host
        ))
    }

    /// The notifications step's channel: the one it created, else the first one.
    pub fn setup_channel(&self) -> Option<&common::notify::NotifyChannel> {
        match &self.setup.channel {
            Some(id) => self.notify.channel(id),
            None => self.notify.channels.first(),
        }
    }

    /// A channel the wizard created while the flow is open gets the Essentials preset.
    pub fn channel_created(&mut self, channel: &common::notify::NotifyChannel) {
        if self.screen != Screen::Setup {
            return;
        }
        self.setup.channel = Some(channel.id.clone());
        if self.notify.rules.iter().any(|r| r.channel_id == channel.id) {
            return;
        }
        let Some(essentials) = preset("essentials") else {
            return;
        };
        let body = CreateNotifyRule {
            channel_id: channel.id.clone(),
            scope: RuleScope::Instance,
            preset: Some(essentials.id.to_string()),
            events: essentials.events(),
            enabled: true,
            dedupe_secs: None,
            max_per_hour: None,
        };
        let name = channel.name.clone();
        self.spawn(|c| async move {
            match c.create_notify_rule(&body).await {
                Ok(_) => Msg::Toast(format!("{name} gets the Essentials events")),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    pub fn setup_test_channel(&mut self) {
        let Some(id) = self.setup_channel().map(|c| c.id.clone()) else {
            return self.error("add a channel first");
        };
        if self.setup.testing {
            return;
        }
        self.setup.testing = true;
        self.setup.test = None;
        self.spawn(|c| async move {
            Msg::SetupTested(
                c.test_notify_channel(&id)
                    .await
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    /// The schedule dialog, turned on and pointed at the first bucket.
    pub fn setup_schedule(&mut self) {
        self.form_instance_schedule();
        let storage = self.storage.list.first().cloned();
        if let Some(Overlay::Form(form)) = self.overlay.as_mut() {
            if let Some(f) = form.field_mut("enabled") {
                f.on = true;
            }
            if let (Some(s), Some(f)) = (storage, form.field_mut("store"))
                && f.value.is_empty()
            {
                f.value = s.id.clone();
                f.shown = s.name.clone();
                f.placeholder = crate::picker::bucket_path(&s);
            }
            form.refresh();
        }
    }
}

fn unique_name(base: &str, taken: &[String]) -> String {
    (1..)
        .map(|n| {
            if n == 1 {
                base.to_string()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|name| !taken.contains(name))
        .unwrap_or_default()
}

// Keys

pub fn keys(app: &mut App, key: KeyEvent) {
    let step = app.setup.step;
    let choices = app.setup_choices();
    match key.code {
        KeyCode::Esc => return app.close_setup(),
        KeyCode::Enter => return app.setup_enter(),
        KeyCode::Char('s') if step != Step::Done && step != Step::Welcome => {
            return app.skip_step();
        }
        KeyCode::Up | KeyCode::Char('k') if choices > 0 => {
            return app.choose(app.setup.choice.saturating_sub(1));
        }
        KeyCode::Down | KeyCode::Char('j') if choices > 0 => {
            return app.choose(app.setup.choice + 1);
        }
        KeyCode::BackTab => return app.go_to_step(step.prev()),
        KeyCode::Tab => return app.go_to_step(step.next()),
        KeyCode::Up | KeyCode::Char('k') => return app.go_to_step(step.prev()),
        KeyCode::Down | KeyCode::Char('j') => return app.go_to_step(step.next()),
        _ => {}
    }
    match (step, key.code) {
        (Step::Domain, KeyCode::Char('e')) => app.edit_domain(),
        (Step::Domain, KeyCode::Char('r')) => app.check_domain(),
        (Step::Domain, KeyCode::Char('c')) => {
            if let Some(url) = app.domain.data.as_ref().and_then(|d| d.url.clone()) {
                app.copy(&url, "the URL");
            }
        }
        (Step::Backups, KeyCode::Char('a')) => app.form_storage(false),
        (Step::Backups, KeyCode::Char('t')) => {
            if !app.storage.list.is_empty() {
                app.storage.row = 0;
                app.test_storage();
            }
        }
        (Step::Backups, KeyCode::Char('e')) => app.setup_schedule(),
        (Step::Notifications, KeyCode::Char('a')) => app.form_notify_kind(),
        (Step::Notifications, KeyCode::Char('t')) => app.setup_test_channel(),
        (Step::FirstApp, KeyCode::Char('o')) => {
            if let Some(url) = app.first_app_url() {
                app.open_url(&url);
            }
        }
        (Step::FirstApp, KeyCode::Char('y')) => {
            if let Some(url) = app.first_app_url() {
                app.copy(&url, "the URL");
            }
        }
        _ => {}
    }
}

/// Typing a domain inside the flow: like the guide, but esc only closes the input.
pub fn domain_input_esc(app: &mut App) {
    app.domain.input = None;
}

pub type Hints = Vec<(&'static str, &'static str)>;

pub fn key_hints(app: &App) -> (Hints, Hints) {
    let step = app.setup.step;
    let done = app.setup.state(step) == StepState::Done;
    if step == Step::Domain && app.domain.input.is_some() {
        return (
            vec![("type", "domain"), ("⏎", "save & check")],
            vec![("esc", "back")],
        );
    }
    let mut left: Hints = Vec::new();
    let mut steps = true;
    match step {
        Step::Welcome => left.push(("⏎", "start")),
        Step::Domain => {
            left.extend([("e", "edit domain"), ("r", "check again"), ("s", "skip")]);
            if done {
                left.push(("⏎", "continue"));
            }
        }
        Step::Source if done => left.push(("⏎", "continue")),
        Step::Source => {
            left.extend([("↑↓", "choose"), ("⏎", "connect"), ("s", "skip")]);
            steps = false;
        }
        Step::Backups => {
            left.extend([("a", "add storage"), ("e", "schedule"), ("s", "skip")]);
            if done {
                left.push(("⏎", "continue"));
            }
        }
        Step::Notifications => {
            left.push(("a", "add channel"));
            if app.setup_channel().is_some() {
                left.push(("t", "send test"));
            }
            left.push(("s", "skip"));
            if done {
                left.push(("⏎", "continue"));
            }
        }
        Step::FirstApp if app.deploy_live() => {
            left.push(("⏎", "continue"));
            if app.first_app_url().is_some() {
                left.extend([("o", "open in browser"), ("y", "copy")]);
            }
        }
        Step::FirstApp if app.setup.app.is_some() => {
            steps = false;
            if app
                .deploy
                .as_ref()
                .is_some_and(|d| d.deployment.can_cancel())
            {
                left.push(("c", "cancel"));
            }
        }
        Step::FirstApp if done => left.push(("⏎", "continue")),
        Step::FirstApp => {
            left.extend([("↑↓", "choose"), ("⏎", "create and deploy"), ("s", "skip")]);
            steps = false;
        }
        Step::Done => {
            return (vec![("⏎", "go to overview")], vec![("q", "quit")]);
        }
    }
    if steps && step != Step::Welcome {
        left.push(("↑↓", "step"));
    }
    (left, vec![("esc", "later"), ("q", "quit")])
}

// Drawing

fn glyph(state: StepState, current: bool) -> Span<'static> {
    match state {
        StepState::Done => Span::styled("✓", theme::fg(theme::OK)),
        StepState::Skipped => Span::styled("–", theme::muted()),
        StepState::Todo if current => Span::styled("●", theme::accent()),
        StepState::Todo => Span::styled("○", theme::muted()),
    }
}

/// Welcome and Done count as done once the flow is past them.
fn rail_state(app: &App, step: Step) -> StepState {
    match step {
        Step::Welcome if app.setup.step == Step::Welcome => StepState::Todo,
        Step::Welcome => StepState::Done,
        Step::Done if app.setup.step == Step::Done => StepState::Done,
        Step::Done => StepState::Todo,
        _ => app.setup.state(step),
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let [rail_area, body] = Layout::horizontal([Constraint::Length(RAIL_W), Constraint::Fill(1)])
        .spacing(1)
        .areas(area);
    rail(frame, rail_area, app);
    let step = app.setup.step;
    let number = Step::TASKS
        .iter()
        .position(|s| *s == step)
        .map(|i| format!("{}  ", i + 1))
        .unwrap_or_default();
    let kind = if step.recommended() {
        Some(Line::from(Span::styled("recommended", theme::muted())))
    } else if step.optional() {
        Some(Line::from(Span::styled("optional", theme::muted())))
    } else {
        None
    };
    let block = panel(
        title(&format!("{number}{}", step.label()), true),
        kind,
        true,
    );
    let inner = inside(&block, body).inner(Margin::new(2, 1));
    frame.render_widget(block, body);
    match step {
        Step::Welcome => welcome(frame, inner, app),
        Step::Domain => domain(frame, inner, app),
        Step::Source => source(frame, inner, app),
        Step::Backups => backups(frame, inner, app),
        Step::Notifications => notifications(frame, inner, app),
        Step::FirstApp => first_app(frame, inner, app),
        Step::Done => done(frame, inner),
    }
}

/// Rows of the rail: Welcome, the five tasks, then Done, with a blank row between the groups.
fn rail_row(step: Step) -> u16 {
    match step {
        Step::Welcome => 0,
        Step::Done => 8,
        task => 2 + Step::TASKS.iter().position(|s| *s == task).unwrap_or(0) as u16,
    }
}

fn rail(frame: &mut Frame, area: Rect, app: &App) {
    let block = panel(title("Setup", false), None, false);
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    for step in Step::ALL {
        let y = rail_row(step);
        if y >= inner.height {
            break;
        }
        let row = Rect::new(inner.x, inner.y + y, inner.width, 1);
        let current = step == app.setup.step;
        let state = rail_state(app, step);
        let name_style = if current {
            theme::accent()
        } else if state == StepState::Skipped {
            theme::muted()
        } else {
            theme::text()
        };
        let suffix = match state {
            StepState::Skipped => "skipped",
            StepState::Todo if step.optional() => "optional",
            _ => "",
        };
        let name = format!(" {}", step.label());
        let pad = (row.width as usize).saturating_sub(1 + name.chars().count() + suffix.len());
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                glyph(state, current),
                Span::styled(name, name_style),
                Span::raw(" ".repeat(pad)),
                Span::styled(suffix, theme::muted()),
            ])),
            row,
        );
        mouse::hit(row, Hit::SetupStep(step));
    }
    if let Some(o) = &app.setup.data
        && inner.height > 10
    {
        let bottom = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
        frame.render_widget(
            Paragraph::new(format!("{} of {} done", o.done(), o.total())).style(theme::muted()),
            bottom,
        );
    }
}

fn note(frame: &mut Frame, row: Rect, text: &str) {
    frame.render_widget(Paragraph::new(text.to_string()).style(theme::muted()), row);
}

fn welcome(frame: &mut Frame, area: Rect, app: &App) {
    let rows = lines_of(area, 10);
    let host = app.info.as_ref().map_or("", |i| i.hostname.as_str());
    frame.render_widget(
        Paragraph::new(host.to_string()).style(theme::text()),
        rows[0],
    );
    if let Some(info) = &app.info {
        let mut meta = vec![
            info.public_ip.clone().unwrap_or_default(),
            format!("{} {}", brand::DAEMON_BIN, info.version),
        ];
        if let Some(docker) = app
            .settings
            .settings
            .as_ref()
            .and_then(|s| s.docker_version.clone())
        {
            meta.push(format!("Docker {docker}"));
        }
        meta.retain(|m| !m.is_empty());
        note(frame, rows[1], &meta.join(" · "));
    }
    for (i, (step, what)) in [
        (Step::Domain, "HTTPS for the API and GitHub callbacks"),
        (Step::Source, "deploy private repos on every push"),
        (Step::Backups, "the whole instance, to your own bucket"),
        (Step::Notifications, "hear when something breaks"),
        (Step::FirstApp, "a live URL in a minute"),
    ]
    .into_iter()
    .enumerate()
    {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{:<20}", step.label()), theme::text()),
                Span::styled(what, theme::muted()),
            ])),
            rows[3 + i],
        );
    }
    note(
        frame,
        rows[9],
        "Every step can be skipped. Reopen with : setup.",
    );
}

fn domain(frame: &mut Frame, area: Rect, app: &App) {
    let [head, guide] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)])
        .spacing(1)
        .areas(area);
    note(
        frame,
        head,
        "Serves the API and GitHub callbacks over HTTPS.",
    );
    view::domain(frame, guide, app);
}

/// A row of a step's menu; the selected one sits on the raised surface with its name in accent.
fn choice_row(frame: &mut Frame, row: Rect, i: usize, selected: bool, name: &str, what: &str) {
    let (row_style, name_style) = if selected {
        (theme::selected(), theme::accent())
    } else {
        (theme::text(), theme::text())
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("  {name:<24}"), name_style),
            Span::styled(what.to_string(), theme::muted()),
        ]))
        .style(row_style),
        row,
    );
    mouse::hit(row, Hit::SetupChoice(i));
}

fn source(frame: &mut Frame, area: Rect, app: &App) {
    let rows = lines_of(area, 6);
    note(
        frame,
        rows[0],
        "Deploy from private repos and on every push.",
    );
    if let Some(s) = app.sources.ready().first() {
        let repos = s.used_by.len();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("✓ ", theme::fg(theme::OK)),
                Span::styled(
                    format!("{:<24}", truncate(&source_summary(s), 23)),
                    theme::text(),
                ),
                Span::styled(format!("{:<9}", "ready"), theme::fg(theme::OK)),
                Span::styled(
                    match repos {
                        0 => String::new(),
                        1 => "1 service".into(),
                        n => format!("{n} services"),
                    },
                    theme::muted(),
                ),
            ])),
            rows[2],
        );
        return;
    }
    for (i, (name, what)) in SOURCE_CHOICES.iter().enumerate() {
        choice_row(frame, rows[2 + i], i, i == app.setup.choice, name, what);
    }
}

/// "1  Storage    ✓ r2-backups   s3://…  tested ok        a add".
fn task_row(
    frame: &mut Frame,
    row: Rect,
    n: usize,
    label: &str,
    value: Option<(String, String)>,
    todo: &str,
    hint: Option<(&'static str, &'static str)>,
) {
    let mut spans = vec![
        Span::styled(format!("{n}  "), theme::muted()),
        Span::styled(format!("{label:<11}"), theme::text()),
    ];
    match value {
        Some((value, detail)) => spans.extend([
            Span::styled("✓ ", theme::fg(theme::OK)),
            Span::styled(format!("{:<13}", format!("{value}  ")), theme::text()),
            Span::styled(detail, theme::muted()),
        ]),
        None => spans.extend([
            Span::styled("○ ", theme::muted()),
            Span::styled(todo.to_string(), theme::muted()),
        ]),
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), row);
    if let Some(hint) = hint {
        mouse::hints(frame, row, &[hint], Alignment::Right);
    }
}

fn backups(frame: &mut Frame, area: Rect, app: &App) {
    let rows = lines_of(area, 6);
    note(
        frame,
        rows[0],
        "A lost server is one restore away: config, secrets, projects and the store.",
    );
    let storage = app.storage.list.first().map(|s| {
        let (_, test, _) = crate::picker::test_state(s);
        (
            s.name.clone(),
            format!("{:<26}{test}", crate::picker::bucket_path(s)),
        )
    });
    task_row(
        frame,
        rows[2],
        1,
        "Storage",
        storage,
        "add an S3 bucket: AWS, R2, B2, MinIO",
        Some(("a", "add")),
    );
    let schedule = app
        .settings
        .schedule
        .as_ref()
        .filter(|s| s.enabled)
        .map(|s| {
            let store = s
                .storage_id
                .as_deref()
                .and_then(|id| app.storage.name(id))
                .unwrap_or("data directory");
            (
                format!("{:02}:00 UTC", s.hour_utc),
                format!("daily · keep {} · {store} · encrypted", s.keep),
            )
        });
    let current = app.settings.schedule.clone().unwrap_or_default();
    let hint = if schedule.is_some() {
        "edit"
    } else {
        "turn on"
    };
    task_row(
        frame,
        rows[4],
        2,
        "Schedule",
        schedule,
        &format!(
            "daily {:02}:00 UTC · keep {} · encrypted",
            current.hour_utc, current.keep
        ),
        Some(("e", hint)),
    );
}

fn notifications(frame: &mut Frame, area: Rect, app: &App) {
    let rows = lines_of(area, 8);
    note(frame, rows[0], "Hear about failures before your users do.");
    let channel = app.setup_channel();
    task_row(
        frame,
        rows[2],
        1,
        "Channel",
        channel.map(|c| (c.name.clone(), c.kind.label().to_string())),
        "Discord, Slack, Teams, email, ntfy, webhooks",
        Some(("a", "add")),
    );
    let rule = channel.and_then(|c| app.notify.rules.iter().find(|r| r.channel_id == c.id));
    task_row(
        frame,
        rows[4],
        2,
        "Rule",
        rule.map(|r| {
            let label = r
                .preset
                .as_deref()
                .and_then(preset)
                .map_or_else(|| "custom".to_string(), |p| p.label.to_string());
            let events: Vec<&str> = r.events.iter().take(5).map(|e| e.label()).collect();
            (label, events.join(", ").to_lowercase())
        }),
        "Essentials: failures and outages only",
        None,
    );
    let test_hint = channel.is_some().then_some(("t", "send a test"));
    let row = rows[6];
    match (&app.setup.test, app.setup.testing) {
        (_, true) => task_row(
            frame,
            row,
            3,
            "Test",
            None,
            &format!("{} sending", util::spinner(app.tick)),
            test_hint,
        ),
        (Some(Ok(d)), _) if d.error.is_none() => task_row(
            frame,
            row,
            3,
            "Test",
            Some((
                "delivered".into(),
                d.latency_ms.map(|ms| format!("{ms}ms")).unwrap_or_default(),
            )),
            "",
            test_hint,
        ),
        (Some(result), _) => {
            let error = match result {
                Ok(d) => d.error.clone().unwrap_or_default(),
                Err(e) => e.clone(),
            };
            task_row(frame, row, 3, "Test", None, "", test_hint);
            frame.render_widget(
                Paragraph::new(format!("✕ {}", truncate(&error, 80))).style(theme::fg(theme::ERR)),
                Rect {
                    x: row.x + 16,
                    width: row.width.saturating_sub(30),
                    ..row
                },
            );
        }
        (None, false) => task_row(
            frame,
            row,
            3,
            "Test",
            None,
            "send a test message",
            test_hint,
        ),
    }
}

fn first_app(frame: &mut Frame, area: Rect, app: &App) {
    if app.deploy_live() {
        return live(frame, area, app);
    }
    if app.setup.app.is_some() && app.deploy.is_some() {
        return view::deploy(frame, area, app);
    }
    let rows = lines_of(area, 14);
    if app.setup.state(Step::FirstApp) == StepState::Done {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("✓ ", theme::fg(theme::OK)),
                Span::styled("a service is live on this instance", theme::text()),
            ])),
            rows[0],
        );
        return;
    }
    match app.setup.busy {
        Some(busy) => frame.render_widget(
            Paragraph::new(format!("{} {busy}…", util::spinner(app.tick)))
                .style(theme::fg(theme::INFO)),
            rows[0],
        ),
        None => note(frame, rows[0], "Deploy something small to see it all work."),
    }
    let choices = app.app_choices();
    let github = app
        .sources
        .ready()
        .first()
        .map(|s| format!("pick a repo from {}", source_summary(s)))
        .unwrap_or_default();
    for (i, choice) in choices.iter().enumerate() {
        let (name, what) = match choice {
            AppChoice::Sample => (
                "Sample app",
                format!("{} · a tiny hello-world page", onboarding::SAMPLE_IMAGE),
            ),
            AppChoice::Github => ("From GitHub", github.clone()),
        };
        choice_row(frame, rows[2 + i], i, i == app.setup.choice, name, &what);
    }
    if choices.get(app.setup.choice) != Some(&AppChoice::Sample) {
        return;
    }
    let first = 3 + choices.len();
    let fields = [
        ("project", onboarding::SAMPLE_PROJECT.to_string()),
        ("environment", "production".to_string()),
        ("service", onboarding::SAMPLE_SERVICE.to_string()),
        ("server", String::new()),
        (
            "domain",
            app.sample_domain()
                .unwrap_or_else(|| "none · this server's public address is unknown".into()),
        ),
    ];
    for (i, (key, value)) in fields.into_iter().enumerate() {
        let row = rows[first + i];
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{key:<14}"), theme::muted()),
                Span::styled(value, theme::text()),
            ])),
            row,
        );
        if key == "server" {
            server_slot(
                frame,
                Rect {
                    x: row.x + 14,
                    width: 32.min(row.width.saturating_sub(14)),
                    ..row
                },
                app,
            );
        }
    }
}

/// Where the first app runs. The sample always uses the local server.
fn server_slot(frame: &mut Frame, area: Rect, app: &App) {
    let name = app.local_server().map_or("local", |s| s.name.as_str());
    let line = Line::from(vec![
        Span::styled(format!(" {name}"), theme::text()),
        Span::styled("  this server", theme::muted()),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme::selected()), area);
}

fn live(frame: &mut Frame, area: Rect, app: &App) {
    let rows = lines_of(area, 6);
    let name = app.setup.app.as_ref().map_or("", |a| a.name.as_str());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("✓ ", theme::fg(theme::OK)),
            Span::styled(format!("{name} is live"), theme::text()),
        ])),
        rows[0],
    );
    let url = app.first_app_url();
    if let Some(url) = &url {
        frame.render_widget(
            Paragraph::new(url.clone()).style(theme::fg(theme::INFO)),
            rows[2],
        );
    }
    if let Some(d) = app.deploy.as_ref().map(|v| &v.deployment) {
        let took = d
            .finished_at
            .map(|f| (f - d.created_at).num_seconds())
            .unwrap_or_default();
        note(
            frame,
            rows[3],
            &format!("deployed in {}", util::duration(took)),
        );
    }
    if url.is_some() {
        mouse::hints(
            frame,
            rows[5],
            &[("o", "open in browser"), ("y", "copy")],
            Alignment::Left,
        );
    }
}

fn done(frame: &mut Frame, area: Rect) {
    let rows = lines_of(area, 8);
    frame.render_widget(Paragraph::new("You're set.").style(theme::text()), rows[0]);
    for (i, (key, what)) in [
        ("n", "new service"),
        ("d", "deploy the selected service"),
        (":", "command palette (try : setup)"),
        (",", "settings"),
    ]
    .into_iter()
    .enumerate()
    {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{key}  "), theme::accent()),
                Span::styled(what, theme::muted()),
            ])),
            rows[2 + i],
        );
    }
}

/// The small "Setup 3/5" panel on the Overview, once the flow is closed.
pub fn checklist_height(app: &App) -> u16 {
    if app
        .setup
        .data
        .as_ref()
        .is_some_and(Onboarding::show_checklist)
    {
        Step::TASKS.len() as u16 + 3
    } else {
        0
    }
}

pub fn checklist(frame: &mut Frame, area: Rect, app: &App) {
    let Some(o) = &app.setup.data else { return };
    let block = panel(
        title(&format!("Setup {}/{}", o.done(), o.total()), false),
        None,
        false,
    );
    let inner = inside(&block, area);
    frame.render_widget(block, area);
    mouse::hit(area, Hit::SetupChecklist);
    let rows = lines_of(inner, inner.height);
    for (row, s) in rows.iter().zip(&o.steps) {
        let style = if s.state == StepState::Skipped {
            theme::muted()
        } else {
            theme::text()
        };
        let mut spans = vec![
            glyph(s.state, false),
            Span::styled(format!(" {}", truncate(s.step.label(), 20)), style),
        ];
        if s.state == StepState::Skipped {
            spans.push(Span::styled("  skipped", theme::muted()));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), *row);
        mouse::hit(*row, Hit::SetupStep(s.step));
    }
    if let Some(last) = rows.last() {
        mouse::hints(
            frame,
            *last,
            &[("o", "open"), ("X", "dismiss")],
            Alignment::Left,
        );
    }
}

/// A click on a step: inside the flow it goes there; on the checklist it opens the flow there.
pub fn click_step(app: &mut App, step: Step) {
    if app.screen != Screen::Setup {
        app.open_setup();
    }
    if app.setup.step != step {
        app.go_to_step(step);
    }
}

pub fn click_choice(app: &mut App, i: usize, double: bool) {
    app.choose(i);
    if double {
        app.setup_enter();
    }
}
