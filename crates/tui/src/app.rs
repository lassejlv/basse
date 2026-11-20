//! Application state, background requests and the actions keys trigger.

use crate::form::{Field, Form, FormKind};
use client::Client;
use common::api::*;
use common::*;
use futures_util::StreamExt;
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::AbortHandle;

const LOG_CAP: usize = 5000;
const REFRESH: Duration = Duration::from_secs(3);

/// Results of background work, applied on the UI thread.
pub enum Msg {
    Info(Result<DaemonInfo, String>),
    Tree(Tree),
    Activity(Vec<Activity>),
    Servers(Vec<Server>),
    Detail(Box<ServiceDetail>),
    Stats(Id, ServiceStats),
    Deployments(Id, Vec<Deployment>),
    Deployment(Box<Deployment>),
    ServiceLog(Id, LogLine),
    ServiceLogEnd(Id, Option<String>),
    DeployLog(Id, LogLine),
    DeployLogEnd(Id),
    /// A deploy request was accepted: follow it on the deploy screen.
    Deployed(Box<Deployment>),
    FormDone {
        toast: String,
        then: Then,
    },
    FormError(String),
    Toast(String),
    Error(String),
}

/// Where to go after a form succeeds.
pub enum Then {
    Stay,
    OpenService(Id),
    OpenDeploy(Box<Deployment>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Service,
    Deploy,
    Servers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    Sidebar,
    #[default]
    Services,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Deployments,
    Logs,
    Env,
    Domains,
    Volumes,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 7] = [
        Tab::Overview,
        Tab::Deployments,
        Tab::Logs,
        Tab::Env,
        Tab::Domains,
        Tab::Volumes,
        Tab::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Deployments => "Deployments",
            Tab::Logs => "Logs",
            Tab::Env => "Env",
            Tab::Domains => "Domains",
            Tab::Volumes => "Volumes",
            Tab::Settings => "Settings",
        }
    }

    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideItem {
    Project(usize),
    Environment(usize, usize),
}

#[derive(Default)]
pub struct OverviewState {
    pub pane: Pane,
    pub side: usize,
    pub row: usize,
    pub collapsed: HashSet<Id>,
    pub filter: String,
    pub filtering: bool,
}

/// Scrollback with follow mode, search and filters, used for runtime and build logs.
#[derive(Default)]
pub struct LogBuffer {
    pub lines: VecDeque<LogLine>,
    pub follow: bool,
    /// Lines scrolled up from the bottom when not following.
    pub offset: usize,
    pub timestamps: bool,
    pub wrap: bool,
    pub errors_only: bool,
    pub search: String,
    pub searching: bool,
    pub match_idx: usize,
    pub ended: Option<String>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self {
            follow: true,
            timestamps: true,
            ..Self::default()
        }
    }

    pub fn push(&mut self, line: LogLine) {
        if self.lines.len() == LOG_CAP {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
        if !self.follow {
            self.offset += 1;
        }
    }

    pub fn is_error(line: &LogLine) -> bool {
        let l = &line.line;
        line.stream == LogStream::Stderr && level_of(l) != Some(Level::Info)
            || matches!(level_of(l), Some(Level::Error))
    }

    /// Indices of lines that pass the errors-only filter.
    pub fn visible(&self) -> Vec<usize> {
        (0..self.lines.len())
            .filter(|i| !self.errors_only || Self::is_error(&self.lines[*i]))
            .collect()
    }

    pub fn matches(&self) -> Vec<usize> {
        if self.search.is_empty() {
            return Vec::new();
        }
        let needle = self.search.to_lowercase();
        self.visible()
            .into_iter()
            .enumerate()
            .filter(|(_, i)| self.lines[*i].line.to_lowercase().contains(&needle))
            .map(|(pos, _)| pos)
            .collect()
    }

    pub fn scroll(&mut self, delta: isize, viewport: usize) {
        let max = self.visible().len().saturating_sub(viewport);
        let next = (self.offset as isize + delta).clamp(0, max as isize) as usize;
        self.offset = next;
        self.follow = next == 0;
    }

    /// Jump so the n-th match (in visible order) sits in the middle of the viewport.
    pub fn jump(&mut self, step: isize, viewport: usize) {
        let matches = self.matches();
        if matches.is_empty() {
            return;
        }
        let n = matches.len() as isize;
        self.match_idx = (self.match_idx as isize + step).rem_euclid(n) as usize;
        let total = self.visible().len();
        let pos = matches[self.match_idx];
        let bottom = (pos + viewport / 2 + 1).min(total);
        self.offset = total - bottom;
        self.follow = false;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

/// Detect a log level token near the start of a line.
pub fn level_of(line: &str) -> Option<Level> {
    let head: String = line
        .chars()
        .take(48)
        .collect::<String>()
        .to_ascii_lowercase();
    let words: Vec<&str> = head
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().find_map(|w| match *w {
        "error" | "err" | "fatal" | "panic" | "crit" | "critical" => Some(Level::Error),
        "warn" | "warning" => Some(Level::Warn),
        "info" | "notice" | "log" => Some(Level::Info),
        "debug" | "trace" => Some(Level::Debug),
        _ => None,
    })
}

pub struct ServiceView {
    pub id: Id,
    pub tab: Tab,
    pub detail: Option<ServiceDetail>,
    pub deployments: Vec<Deployment>,
    pub logs: LogBuffer,
    pub log_task: Option<AbortHandle>,
    pub row: usize,
    pub reveal: bool,
}

pub struct DeployView {
    pub deployment: Deployment,
    pub logs: LogBuffer,
    pub task: Option<AbortHandle>,
    /// Screen to return to on esc.
    pub back: Screen,
    pub announced: bool,
}

#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteService(Id),
    DeleteProject(Id),
    DeleteEnvironment(Id),
    DeleteDomain(Id),
    DeleteVolume(Id, bool),
    DeleteEnvVar(Id, String),
    RemoveServer(Id),
    Rollback(Id),
    CancelDeploy(Id),
    Stop(Id),
}

pub struct Confirm {
    pub title: String,
    pub message: String,
    pub danger: bool,
    pub action: ConfirmAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Deploy,
    Restart,
    Stop,
    Start,
    Logs,
    NewService,
    NewProject,
    NewEnvironment,
    Servers,
    AddServer,
    SetupProxy,
    Refresh,
    Help,
    Quit,
}

impl Command {
    pub const ALL: [Command; 14] = [
        Command::Deploy,
        Command::Restart,
        Command::Stop,
        Command::Start,
        Command::Logs,
        Command::NewService,
        Command::NewProject,
        Command::NewEnvironment,
        Command::Servers,
        Command::AddServer,
        Command::SetupProxy,
        Command::Refresh,
        Command::Help,
        Command::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::Deploy => "deploy service",
            Command::Restart => "restart service",
            Command::Stop => "stop service",
            Command::Start => "start service",
            Command::Logs => "open logs",
            Command::NewService => "new service",
            Command::NewProject => "new project",
            Command::NewEnvironment => "new environment",
            Command::Servers => "servers",
            Command::AddServer => "add server",
            Command::SetupProxy => "set up proxy on this server",
            Command::Refresh => "refresh",
            Command::Help => "help",
            Command::Quit => "quit",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Command::Deploy => "d",
            Command::Restart => "r",
            Command::Stop | Command::Start => "s",
            Command::Logs => "l",
            Command::NewService => "n",
            Command::NewProject => "N",
            Command::NewEnvironment => "E",
            Command::Servers => "S",
            Command::AddServer => "",
            Command::SetupProxy => "",
            Command::Refresh => "ctrl+r",
            Command::Help => "?",
            Command::Quit => "q",
        }
    }
}

#[derive(Default)]
pub struct Palette {
    pub input: String,
    pub row: usize,
}

impl Palette {
    pub fn commands(&self) -> Vec<Command> {
        let q = self.input.to_lowercase();
        Command::ALL
            .into_iter()
            .filter(|c| q.split_whitespace().all(|w| c.label().contains(w)))
            .collect()
    }
}

pub enum Overlay {
    Form(Form),
    Confirm(Confirm),
    Palette(Palette),
    Help,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

pub struct App {
    pub client: Client,
    pub context: String,
    tx: UnboundedSender<Msg>,
    pub tick: usize,
    pub info: Option<DaemonInfo>,
    pub connection_error: Option<String>,
    pub tree: Option<Tree>,
    pub activity: Vec<Activity>,
    pub servers: Vec<Server>,
    pub details: HashMap<Id, ServiceDetail>,
    pub stats: HashMap<Id, ServiceStats>,
    pub screen: Screen,
    pub overview: OverviewState,
    pub service: Option<ServiceView>,
    pub deploy: Option<DeployView>,
    pub server_row: usize,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    pub quit: bool,
    last_refresh: Option<Instant>,
    pub mouse: crate::mouse::MouseState,
    /// Height of the log viewport at the last draw, for paging.
    pub log_viewport: Cell<usize>,
}

impl App {
    pub fn new(client: Client, context: String, tx: UnboundedSender<Msg>) -> Self {
        Self {
            client,
            context,
            tx,
            tick: 0,
            info: None,
            connection_error: None,
            tree: None,
            activity: Vec::new(),
            servers: Vec::new(),
            details: HashMap::new(),
            stats: HashMap::new(),
            screen: Screen::Overview,
            overview: OverviewState::default(),
            service: None,
            deploy: None,
            server_row: 0,
            overlay: None,
            toast: None,
            quit: false,
            last_refresh: None,
            mouse: crate::mouse::MouseState::new(),
            log_viewport: Cell::new(20),
        }
    }

    fn spawn<F>(&self, f: impl FnOnce(Client) -> F)
    where
        F: Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        let fut = f(self.client.clone());
        tokio::spawn(async move {
            let _ = tx.send(fut.await);
        });
    }

    /// Run a request and report its outcome as a toast, refreshing afterwards.
    fn act<T, F>(&self, ok: String, f: impl FnOnce(Client) -> F)
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn(|c| {
            let fut = f(c);
            async move {
                match fut.await {
                    Ok(_) => Msg::Toast(ok),
                    Err(e) => Msg::Error(format!("{e:#}")),
                }
            }
        });
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    // Data

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        if self
            .toast
            .as_ref()
            .is_some_and(|t| t.at.elapsed() > Duration::from_secs(5))
        {
            self.toast = None;
        }
        if self.last_refresh.is_none_or(|t| t.elapsed() >= REFRESH) {
            self.refresh();
        }
        if self.screen == Screen::Deploy && self.tick.is_multiple_of(4) {
            self.refresh_deployment();
        }
    }

    pub fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        self.spawn(|c| async move { Msg::Info(c.info().await.map_err(|e| format!("{e:#}"))) });
        self.spawn(|c| async move {
            match c.tree().await {
                Ok(t) => Msg::Tree(t),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
        self.spawn(|c| async move { Msg::Activity(c.activity(50).await.unwrap_or_default()) });
        if self.screen == Screen::Servers || self.servers.is_empty() {
            self.spawn(|c| async move { Msg::Servers(c.servers().await.unwrap_or_default()) });
        }
        let ids: Vec<Id> = match self.screen {
            Screen::Overview => self.env_services().iter().map(|s| s.id.clone()).collect(),
            Screen::Service | Screen::Deploy => self.service.iter().map(|s| s.id.clone()).collect(),
            Screen::Servers => Vec::new(),
        };
        for id in ids {
            self.load_stats(&id);
        }
        if let Some(id) = self.selected_service().map(|s| s.id.clone()) {
            self.load_detail(&id);
        }
        if let Some(view) = &self.service {
            let id = view.id.clone();
            self.load_detail(&id);
            self.load_deployments(&id);
        }
    }

    fn load_detail(&self, id: &str) {
        let id = id.to_string();
        self.spawn(|c| async move {
            match c.service(&id).await {
                Ok(d) => Msg::Detail(Box::new(d)),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    fn load_stats(&self, id: &str) {
        let id = id.to_string();
        self.spawn(|c| async move {
            match c.service_stats(&id).await {
                Ok(s) => Msg::Stats(id, s),
                Err(_) => Msg::Stats(
                    id,
                    ServiceStats {
                        cpu_percent: 0.0,
                        mem_used: 0,
                        mem_limit: 0,
                        cpu_history: Vec::new(),
                        mem_history: Vec::new(),
                    },
                ),
            }
        });
    }

    fn load_deployments(&self, id: &str) {
        let id = id.to_string();
        self.spawn(|c| async move {
            match c.deployments(&id, 30).await {
                Ok(d) => Msg::Deployments(id, d),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    fn refresh_deployment(&self) {
        if let Some(view) = &self.deploy {
            let id = view.deployment.id.clone();
            self.spawn(|c| async move {
                match c.deployment(&id).await {
                    Ok(d) => Msg::Deployment(Box::new(d)),
                    Err(e) => Msg::Error(format!("{e:#}")),
                }
            });
        }
    }

    pub fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Info(Ok(info)) => {
                self.info = Some(info);
                self.connection_error = None;
            }
            Msg::Info(Err(e)) => self.connection_error = Some(e),
            Msg::Tree(tree) => {
                self.tree = Some(tree);
                self.clamp_overview();
            }
            Msg::Activity(a) => self.activity = a,
            Msg::Servers(s) => {
                self.servers = s;
                self.server_row = self.server_row.min(self.servers.len().saturating_sub(1));
            }
            Msg::Detail(d) => {
                if let Some(view) = self.service.as_mut().filter(|v| v.id == d.service.id) {
                    view.detail = Some((*d).clone());
                }
                self.details.insert(d.service.id.clone(), *d);
            }
            Msg::Stats(id, s) => {
                self.stats.insert(id, s);
            }
            Msg::Deployments(id, d) => {
                if let Some(view) = self.service.as_mut().filter(|v| v.id == id) {
                    view.deployments = d;
                    if view.tab == Tab::Deployments {
                        view.row = view.row.min(view.deployments.len().saturating_sub(1));
                    }
                }
            }
            Msg::Deployment(d) => {
                let Some(view) = self.deploy.as_mut().filter(|v| v.deployment.id == d.id) else {
                    return;
                };
                view.deployment = *d;
                if view.deployment.status.is_finished() && !view.announced {
                    view.announced = true;
                    let n = view.deployment.number;
                    match view.deployment.status {
                        DeploymentStatus::Live => self.toast(format!("deployment #{n} is live")),
                        DeploymentStatus::Cancelled => {
                            self.toast(format!("deployment #{n} cancelled"))
                        }
                        _ => {
                            let why = view
                                .deployment
                                .error
                                .clone()
                                .unwrap_or_else(|| "see build output".into());
                            self.error(format!("deployment #{n} failed: {why}"));
                        }
                    }
                    self.refresh();
                }
            }
            Msg::ServiceLog(id, line) => {
                if let Some(view) = self.service.as_mut().filter(|v| v.id == id) {
                    view.logs.push(line);
                }
            }
            Msg::ServiceLogEnd(id, err) => {
                if let Some(view) = self.service.as_mut().filter(|v| v.id == id) {
                    view.logs.ended = Some(err.unwrap_or_else(|| "stream ended".into()));
                    view.log_task = None;
                }
            }
            Msg::DeployLog(id, line) => {
                if let Some(view) = self.deploy.as_mut().filter(|v| v.deployment.id == id) {
                    view.logs.push(line);
                }
            }
            Msg::DeployLogEnd(id) => {
                if let Some(view) = self.deploy.as_mut().filter(|v| v.deployment.id == id) {
                    view.task = None;
                    self.refresh_deployment();
                }
            }
            Msg::Deployed(d) => {
                self.toast(format!("deployment #{} started", d.number));
                self.open_deploy(*d);
            }
            Msg::FormDone { toast, then } => {
                self.overlay = None;
                self.toast(toast);
                self.refresh();
                match then {
                    Then::Stay => {}
                    Then::OpenService(id) => self.open_service(&id, Tab::Overview),
                    Then::OpenDeploy(d) => self.open_deploy(*d),
                }
            }
            Msg::FormError(e) => {
                if let Some(Overlay::Form(form)) = &mut self.overlay {
                    form.fail(e);
                } else {
                    self.error(e);
                }
            }
            Msg::Toast(t) => {
                self.toast(t);
                self.refresh();
            }
            Msg::Error(e) => self.error(e),
        }
    }

    // Navigation and selection

    pub fn side_items(&self) -> Vec<SideItem> {
        let Some(tree) = &self.tree else {
            return Vec::new();
        };
        let mut items = Vec::new();
        for (pi, p) in tree.projects.iter().enumerate() {
            items.push(SideItem::Project(pi));
            if !self.overview.collapsed.contains(&p.project.id) {
                items.extend((0..p.environments.len()).map(|ei| SideItem::Environment(pi, ei)));
            }
        }
        items
    }

    /// The environment whose services fill the main pane.
    pub fn current_env(&self) -> Option<(&ProjectNode, &EnvironmentNode)> {
        let tree = self.tree.as_ref()?;
        let (pi, ei) = match self.side_items().get(self.overview.side)? {
            SideItem::Project(pi) => (*pi, 0),
            SideItem::Environment(pi, ei) => (*pi, *ei),
        };
        let project = tree.projects.get(pi)?;
        Some((project, project.environments.get(ei)?))
    }

    pub fn current_project(&self) -> Option<&ProjectNode> {
        let tree = self.tree.as_ref()?;
        let pi = match self.side_items().get(self.overview.side)? {
            SideItem::Project(pi) | SideItem::Environment(pi, _) => *pi,
        };
        tree.projects.get(pi)
    }

    pub fn env_services(&self) -> Vec<&Service> {
        let Some((_, env)) = self.current_env() else {
            return Vec::new();
        };
        let f = self.overview.filter.to_lowercase();
        env.services
            .iter()
            .filter(|s| {
                f.is_empty() || s.name.contains(&f) || s.source.label().to_lowercase().contains(&f)
            })
            .collect()
    }

    pub fn selected_service(&self) -> Option<&Service> {
        match self.screen {
            Screen::Overview => self.env_services().get(self.overview.row).copied(),
            _ => {
                let id = &self.service.as_ref()?.id;
                self.all_services().find(|s| &s.id == id)
            }
        }
    }

    pub fn all_services(&self) -> impl Iterator<Item = &Service> {
        self.tree
            .iter()
            .flat_map(|t| &t.projects)
            .flat_map(|p| &p.environments)
            .flat_map(|e| &e.services)
    }

    fn clamp_overview(&mut self) {
        let n = self.side_items().len();
        self.overview.side = self.overview.side.min(n.saturating_sub(1));
        let m = self.env_services().len();
        self.overview.row = self.overview.row.min(m.saturating_sub(1));
    }

    pub fn move_side(&mut self, delta: isize) {
        let n = self.side_items().len();
        if n == 0 {
            return;
        }
        self.overview.side =
            (self.overview.side as isize + delta).clamp(0, n as isize - 1) as usize;
        self.overview.row = 0;
        self.after_selection();
    }

    pub fn move_row(&mut self, delta: isize) {
        let n = self.env_services().len();
        if n == 0 {
            return;
        }
        self.overview.row = (self.overview.row as isize + delta).clamp(0, n as isize - 1) as usize;
        self.after_selection();
    }

    fn after_selection(&mut self) {
        let ids: Vec<Id> = self.env_services().iter().map(|s| s.id.clone()).collect();
        for id in &ids {
            if !self.stats.contains_key(id) {
                self.load_stats(id);
            }
        }
        if let Some(id) = self.selected_service().map(|s| s.id.clone())
            && !self.details.contains_key(&id)
        {
            self.load_detail(&id);
        }
    }

    pub fn toggle_project(&mut self) {
        if let Some(SideItem::Project(pi)) = self.side_items().get(self.overview.side).copied()
            && let Some(id) = self
                .tree
                .as_ref()
                .map(|t| t.projects[pi].project.id.clone())
            && !self.overview.collapsed.remove(&id)
        {
            self.overview.collapsed.insert(id);
        }
    }

    pub fn open_service(&mut self, id: &str, tab: Tab) {
        self.stop_streams();
        self.screen = Screen::Service;
        self.service = Some(ServiceView {
            id: id.to_string(),
            tab: Tab::Overview,
            detail: self.details.get(id).cloned(),
            deployments: Vec::new(),
            logs: LogBuffer::new(),
            log_task: None,
            row: 0,
            reveal: false,
        });
        self.load_detail(id);
        self.load_deployments(id);
        self.load_stats(id);
        self.set_tab(tab);
    }

    pub fn set_tab(&mut self, tab: Tab) {
        let Some(view) = self.service.as_mut() else {
            return;
        };
        view.tab = tab;
        view.row = 0;
        if tab == Tab::Logs && view.log_task.is_none() {
            self.start_service_logs();
        }
    }

    pub fn close_service(&mut self) {
        self.stop_streams();
        self.service = None;
        self.screen = Screen::Overview;
        self.refresh();
    }

    fn start_service_logs(&mut self) {
        let Some(view) = self.service.as_mut() else {
            return;
        };
        if let Some(h) = view.log_task.take() {
            h.abort();
        }
        view.logs = LogBuffer::new();
        let id = view.id.clone();
        let client = self.client.clone();
        let tx = self.tx.clone();
        let handle = tokio::spawn(async move {
            match client.service_logs(&id, 500, true).await {
                Ok(mut stream) => {
                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(line) => {
                                if tx.send(Msg::ServiceLog(id.clone(), line)).is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                let _ = tx.send(Msg::ServiceLogEnd(id, Some(format!("{e:#}"))));
                                return;
                            }
                        }
                    }
                    let _ = tx.send(Msg::ServiceLogEnd(id, None));
                }
                Err(e) => {
                    let _ = tx.send(Msg::ServiceLogEnd(id, Some(format!("{e:#}"))));
                }
            }
        });
        view.log_task = Some(handle.abort_handle());
    }

    pub fn restart_service_logs(&mut self) {
        self.start_service_logs();
    }

    pub fn open_deploy(&mut self, deployment: Deployment) {
        if let Some(view) = self.deploy.take()
            && let Some(h) = view.task
        {
            h.abort();
        }
        let back = match self.screen {
            Screen::Deploy => Screen::Service,
            other => other,
        };
        if self
            .service
            .as_ref()
            .is_none_or(|v| v.id != deployment.service_id)
        {
            let id = deployment.service_id.clone();
            self.open_service(&id, Tab::Deployments);
        }
        let id = deployment.id.clone();
        let client = self.client.clone();
        let tx = self.tx.clone();
        let handle = tokio::spawn(async move {
            if let Ok(mut stream) = client.deployment_logs(&id, true).await {
                while let Some(Ok(line)) = stream.next().await {
                    if tx.send(Msg::DeployLog(id.clone(), line)).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send(Msg::DeployLogEnd(id));
        });
        self.deploy = Some(DeployView {
            announced: deployment.status.is_finished(),
            deployment,
            logs: LogBuffer {
                timestamps: false,
                ..LogBuffer::new()
            },
            task: Some(handle.abort_handle()),
            back,
        });
        self.screen = Screen::Deploy;
    }

    pub fn close_deploy(&mut self) {
        let back = self.deploy.as_ref().map_or(Screen::Overview, |v| v.back);
        if let Some(view) = self.deploy.take()
            && let Some(h) = view.task
        {
            h.abort();
        }
        self.screen = match back {
            Screen::Service if self.service.is_some() => Screen::Service,
            Screen::Servers => Screen::Servers,
            _ => {
                self.service = None;
                Screen::Overview
            }
        };
        self.refresh();
    }

    fn stop_streams(&mut self) {
        if let Some(h) = self.service.as_mut().and_then(|v| v.log_task.take()) {
            h.abort();
        }
    }

    // Actions

    pub fn deploy_service(&self, id: &str) {
        let id = id.to_string();
        self.spawn(|c| async move {
            match c.deploy(&id, "tui").await {
                Ok(d) => Msg::Deployed(Box::new(d)),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    pub fn restart_service(&mut self, s: &Service) {
        let id = s.id.clone();
        self.toast(format!("restarting {}…", s.name));
        self.act(format!("{} restarted", s.name), |c| async move {
            c.restart(&id).await
        });
    }

    pub fn start_service(&mut self, s: &Service) {
        let id = s.id.clone();
        self.toast(format!("starting {}…", s.name));
        self.act(format!("{} started", s.name), |c| async move {
            c.start(&id).await
        });
    }

    pub fn confirm(&mut self, action: ConfirmAction) {
        let ok = match &action {
            ConfirmAction::DeleteService(_) => "service deleted",
            ConfirmAction::DeleteProject(_) => "project deleted",
            ConfirmAction::DeleteEnvironment(_) => "environment deleted",
            ConfirmAction::DeleteDomain(_) => "domain removed",
            ConfirmAction::DeleteVolume(..) => "volume removed",
            ConfirmAction::DeleteEnvVar(..) => "variable removed · deploy to apply",
            ConfirmAction::RemoveServer(_) => "server removed",
            ConfirmAction::Rollback(_) => "",
            ConfirmAction::CancelDeploy(_) => "deployment cancelled",
            ConfirmAction::Stop(_) => "service stopped",
        }
        .to_string();
        match action {
            ConfirmAction::DeleteService(id) => {
                if self.service.as_ref().is_some_and(|v| v.id == id) {
                    self.close_service();
                }
                self.act(ok, |c| async move { c.delete_service(&id, false).await });
            }
            ConfirmAction::DeleteProject(id) => {
                self.overview.side = 0;
                self.act(ok, |c| async move { c.delete_project(&id).await });
            }
            ConfirmAction::DeleteEnvironment(id) => {
                self.overview.side = self.overview.side.saturating_sub(1);
                self.act(ok, |c| async move { c.delete_environment(&id).await });
            }
            ConfirmAction::DeleteDomain(id) => {
                self.act(ok, |c| async move { c.delete_domain(&id).await })
            }
            ConfirmAction::DeleteVolume(id, purge) => {
                self.act(ok, |c| async move { c.delete_volume(&id, purge).await });
            }
            ConfirmAction::DeleteEnvVar(service_id, key) => {
                let vars: Vec<EnvVar> = self
                    .service
                    .as_ref()
                    .and_then(|v| v.detail.as_ref())
                    .map(|d| d.env.iter().filter(|v| v.key != key).cloned().collect())
                    .unwrap_or_default();
                self.act(ok, |c| async move { c.set_env(&service_id, &vars).await });
            }
            ConfirmAction::RemoveServer(id) => {
                self.act(ok, |c| async move { c.delete_server(&id).await })
            }
            ConfirmAction::Rollback(id) => self.spawn(|c| async move {
                match c.rollback(&id).await {
                    Ok(d) => Msg::Deployed(Box::new(d)),
                    Err(e) => Msg::Error(format!("{e:#}")),
                }
            }),
            ConfirmAction::CancelDeploy(id) => {
                self.act(ok, |c| async move { c.cancel_deployment(&id).await })
            }
            ConfirmAction::Stop(id) => self.act(ok, |c| async move { c.stop(&id).await }),
        }
    }

    pub fn check_server(&mut self, server: &Server) {
        let id = server.id.clone();
        self.toast(format!("checking {}…", server.name));
        self.act(format!("{} checked", server.name), |c| async move {
            c.check_server(&id).await
        });
    }

    pub fn setup_proxy(&mut self, server: &Server) {
        let id = server.id.clone();
        self.toast(format!("setting up the proxy on {}…", server.name));
        self.act(
            format!("proxy running on {}", server.name),
            |c| async move { c.setup_proxy(&id).await },
        );
    }

    pub fn local_server(&self) -> Option<&Server> {
        self.servers.iter().find(|s| s.kind == ServerKind::Local)
    }

    // Forms

    pub fn form_new_project(&mut self) {
        let form = Form::new(
            FormKind::NewProject,
            "New project",
            vec![
                Field::text("name", "name")
                    .placeholder("acme-shop")
                    .hint("lowercase, dashes"),
                Field::text("description", "description").placeholder("optional"),
            ],
        )
        .subtitle("starts with a production environment")
        .submit("create", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_new_environment(&mut self) {
        let Some(project) = self.current_project() else {
            return self.error("create a project first (N)");
        };
        let form = Form::new(
            FormKind::NewEnvironment {
                project_id: project.project.id.clone(),
            },
            "New environment",
            vec![Field::text("name", "name").placeholder("staging")],
        )
        .subtitle(project.project.name.clone())
        .submit("create", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_new_service(&mut self) {
        let Some((project, env)) = self.current_env() else {
            return self.error("create a project first (N)");
        };
        let servers: Vec<String> = if self.servers.is_empty() {
            vec!["local".into()]
        } else {
            self.servers.iter().map(|s| s.name.clone()).collect()
        };
        let engines = DatabaseKind::ALL
            .iter()
            .map(|k| k.image().to_string())
            .collect();
        let mut fields = vec![
            Field::choice(
                "source",
                "source",
                vec![
                    "Git repository".into(),
                    "Docker image".into(),
                    "Database".into(),
                ],
            ),
            Field::text("repo", "repository")
                .placeholder("https://github.com/acme/api")
                .hint("https or ssh url"),
            Field::text("branch", "branch").value("main"),
            Field::text("dockerfile", "dockerfile")
                .value("Dockerfile")
                .hint("path in the repo"),
            Field::text("image", "image").placeholder("ghcr.io/acme/web:latest"),
            Field::choice("engine", "engine", engines),
            Field::text("version", "version").hint("image tag"),
            Field::text("name", "name"),
            Field::text("port", "port")
                .placeholder("3000")
                .hint("app listen port"),
            Field::text("domain", "domain")
                .placeholder("app.example.com")
                .hint("optional · https"),
        ];
        if servers.len() > 1 {
            fields.push(Field::choice("server", "server", servers));
        }
        let form = Form::new(
            FormKind::NewService {
                environment_id: env.environment.id.clone(),
            },
            "New service",
            fields,
        )
        .subtitle(format!(
            "{} / {}",
            project.project.name, env.environment.name
        ))
        .submit("create & deploy", Some("create only"))
        .focus_on("repo");
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_edit_service(&mut self) {
        let Some(detail) = self.service.as_ref().and_then(|v| v.detail.as_ref()) else {
            return;
        };
        let s = &detail.service;
        let policies = [
            RestartPolicy::UnlessStopped,
            RestartPolicy::Always,
            RestartPolicy::OnFailure,
            RestartPolicy::No,
        ];
        let mut fields = vec![Field::text("name", "name").value(s.name.clone())];
        match &s.source {
            Source::Git {
                repo,
                branch,
                dockerfile,
                ..
            } => {
                fields.push(Field::text("repo", "repository").value(repo.clone()));
                fields.push(Field::text("branch", "branch").value(branch.clone()));
                fields.push(Field::text("dockerfile", "dockerfile").value(dockerfile.clone()));
            }
            Source::Image { image } => {
                fields.push(Field::text("image", "image").value(image.clone()))
            }
            Source::Database { version, .. } => {
                fields.push(Field::text("version", "version").value(version.clone()))
            }
        }
        fields.extend([
            Field::text("port", "port").value(s.port.map(|p| p.to_string()).unwrap_or_default()),
            Field::text("command", "command")
                .value(s.command.clone().unwrap_or_default())
                .placeholder("image default"),
            Field::text("health", "health path")
                .value(s.health_path.clone().unwrap_or_default())
                .placeholder("/healthz")
                .hint("probed before switch"),
            Field::choice(
                "restart",
                "restart",
                policies.iter().map(|p| p.as_str().to_string()).collect(),
            )
            .selected(policies.iter().position(|p| *p == s.restart).unwrap_or(0)),
        ]);
        let form = Form::new(
            FormKind::EditService {
                service_id: s.id.clone(),
            },
            "Settings",
            fields,
        )
        .subtitle(s.name.clone())
        .submit("save", Some("save & deploy"));
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_domain(&mut self) {
        let Some(view) = &self.service else { return };
        let port = view.detail.as_ref().and_then(|d| d.service.port);
        let form = Form::new(
            FormKind::AddDomain {
                service_id: view.id.clone(),
            },
            "Add domain",
            vec![
                Field::text("host", "host")
                    .placeholder("shop.example.com")
                    .hint("point an A record here"),
                Field::text("port", "port")
                    .placeholder(port.map_or("service port".into(), |p| p.to_string()))
                    .hint("container port"),
                Field::toggle("tls", "https", true).hint("automatic certificate"),
            ],
        )
        .subtitle(
            self.info
                .as_ref()
                .and_then(|i| i.public_ip.clone())
                .unwrap_or_default(),
        )
        .submit("add", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_env(&mut self, existing: Option<EnvVar>) {
        let Some(view) = &self.service else { return };
        let editing = existing.is_some();
        let var = existing.unwrap_or(EnvVar {
            key: String::new(),
            value: String::new(),
            secret: false,
        });
        let mut value = Field::text("value", "value").value(var.value.clone());
        value.masked = var.secret && !view.reveal;
        let form = Form::new(
            FormKind::EnvVar {
                service_id: view.id.clone(),
                original_key: editing.then(|| var.key.clone()),
            },
            if editing {
                "Edit variable"
            } else {
                "Add variable"
            },
            vec![
                Field::text("key", "key")
                    .value(var.key.clone())
                    .placeholder("DATABASE_URL"),
                value,
                Field::toggle("secret", "secret", var.secret).hint("masked in listings"),
            ],
        )
        .subtitle("applies on next deploy")
        .submit("save", Some("save & deploy"));
        let form = if editing {
            form.focus_on("value")
        } else {
            form
        };
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_volume(&mut self) {
        let Some(view) = &self.service else { return };
        let form = Form::new(
            FormKind::AddVolume {
                service_id: view.id.clone(),
            },
            "Add volume",
            vec![
                Field::text("name", "name").placeholder("uploads"),
                Field::text("path", "mount path").placeholder("/app/uploads"),
            ],
        )
        .subtitle("mounted on next deploy")
        .submit("add", Some("add & deploy"));
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_server(&mut self) {
        let has_key = self
            .info
            .as_ref()
            .is_some_and(|i| i.ssh_public_key.is_some());
        let form = Form::new(
            FormKind::AddServer,
            "Add server",
            vec![
                Field::text("name", "name").placeholder("edge-fra"),
                Field::text("host", "host").placeholder("203.0.113.10"),
                Field::text("user", "user").value("root"),
                Field::text("port", "ssh port").value("22"),
            ],
        )
        .subtitle(if has_key {
            "trust the daemon key first"
        } else {
            "over ssh"
        })
        .submit("add & check", None);
        self.overlay = Some(Overlay::Form(form));
    }

    /// Validate the open form and send its request.
    pub fn submit_form(&mut self, alt: bool) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        match build_request(form, alt) {
            Ok(req) => {
                form.busy = true;
                form.error = None;
                self.spawn(move |c| async move {
                    match req.send(&c).await {
                        Ok((toast, then)) => Msg::FormDone { toast, then },
                        Err(e) => Msg::FormError(format!("{e:#}")),
                    }
                });
            }
            Err(e) => form.fail(e),
        }
    }

    /// The current variables with `var` inserted, or replacing `original` when renamed.
    fn merged_env(&self, original: Option<&str>, var: EnvVar) -> Vec<EnvVar> {
        let mut vars: Vec<EnvVar> = self
            .service
            .as_ref()
            .and_then(|v| v.detail.as_ref())
            .map(|d| d.env.clone())
            .unwrap_or_default();
        let replace = original.unwrap_or(&var.key).to_string();
        vars.retain(|v| v.key != var.key || v.key == replace);
        match vars.iter().position(|v| v.key == replace) {
            Some(i) => vars[i] = var,
            None => vars.push(var),
        }
        vars
    }

    pub fn submit_env_form(&mut self, alt: bool) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        let FormKind::EnvVar {
            service_id,
            original_key,
        } = form.kind.clone()
        else {
            return;
        };
        let key = form.get("key");
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return form.fail("key must be letters, digits and _");
        }
        form.busy = true;
        let var = EnvVar {
            key,
            value: form.raw("value"),
            secret: form.on("secret"),
        };
        let vars = self.merged_env(original_key.as_deref(), var);
        self.spawn(move |c| async move {
            let result = async {
                c.set_env(&service_id, &vars).await?;
                if alt {
                    let d = c.deploy(&service_id, "tui").await?;
                    return Ok::<_, anyhow::Error>((
                        "saved".to_string(),
                        Then::OpenDeploy(Box::new(d)),
                    ));
                }
                Ok(("saved · deploy to apply".to_string(), Then::Stay))
            };
            match result.await {
                Ok((toast, then)) => Msg::FormDone { toast, then },
                Err(e) => Msg::FormError(format!("{e:#}")),
            }
        });
    }
}

/// A validated request from a form, ready to send.
enum Request {
    Project(CreateProject),
    Environment(Id, CreateEnvironment),
    Service(Box<CreateService>),
    Update {
        id: Id,
        body: UpdateService,
        deploy: bool,
    },
    Domain(Id, CreateDomain),
    Volume {
        id: Id,
        body: CreateVolume,
        deploy: bool,
    },
    Server(CreateServer),
}

impl Request {
    async fn send(self, c: &Client) -> anyhow::Result<(String, Then)> {
        Ok(match self {
            Request::Project(body) => {
                let p = c.create_project(&body).await?;
                (format!("project {} created", p.name), Then::Stay)
            }
            Request::Environment(project, body) => {
                let e = c.create_environment(&project, &body).await?;
                (format!("environment {} created", e.name), Then::Stay)
            }
            Request::Service(body) => {
                let deploy = body.deploy;
                let d = c.create_service(&body).await?;
                let name = d.service.name.clone();
                if deploy
                    && let Some(first) = c.deployments(&d.service.id, 1).await?.into_iter().next()
                {
                    return Ok((
                        format!("{name} created · deploying"),
                        Then::OpenDeploy(Box::new(first)),
                    ));
                }
                (format!("{name} created"), Then::OpenService(d.service.id))
            }
            Request::Update { id, body, deploy } => {
                c.update_service(&id, &body).await?;
                if deploy {
                    let d = c.deploy(&id, "tui").await?;
                    return Ok(("settings saved".into(), Then::OpenDeploy(Box::new(d))));
                }
                ("settings saved · deploy to apply".into(), Then::Stay)
            }
            Request::Domain(id, body) => {
                let d = c.add_domain(&id, &body).await?;
                (format!("{} added", d.host), Then::Stay)
            }
            Request::Volume { id, body, deploy } => {
                let v = c.add_volume(&id, &body).await?;
                if deploy {
                    let d = c.deploy(&id, "tui").await?;
                    return Ok((
                        format!("volume {} added", v.name),
                        Then::OpenDeploy(Box::new(d)),
                    ));
                }
                (
                    format!("volume {} added · deploy to mount", v.name),
                    Then::Stay,
                )
            }
            Request::Server(body) => {
                let s = c.create_server(&body).await?;
                let s = c.check_server(&s.id).await.unwrap_or(s);
                let state = match s.status {
                    ServerStatus::Online => "online",
                    _ => "added but unreachable · check ssh access",
                };
                (format!("{} {state}", s.name), Then::Stay)
            }
        })
    }
}

fn port(text: &str, required: bool) -> Result<Option<u16>, String> {
    if text.is_empty() {
        return if required {
            Err("port is required".into())
        } else {
            Ok(None)
        };
    }
    text.parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .map(Some)
        .ok_or_else(|| format!("'{text}' is not a valid port"))
}

/// Derive a service name from a repo url or image reference: `acme/api.git` gives `api`.
pub fn derive_name(source: &str) -> String {
    let path = source.trim_end_matches('/').trim_end_matches(".git");
    let segment = path.rsplit('/').next().unwrap_or(path);
    let segment = segment.split([':', '@']).next().unwrap_or(segment);
    let name: String = segment
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.starts_with(|c: char| c.is_ascii_lowercase()) {
        name
    } else {
        format!("app-{name}")
    }
}

fn build_request(form: &Form, alt: bool) -> Result<Request, String> {
    let named = |key: &str| -> Result<String, String> {
        let name = form.get(key);
        validate_name(&name)?;
        Ok(name)
    };
    let policies = [
        RestartPolicy::UnlessStopped,
        RestartPolicy::Always,
        RestartPolicy::OnFailure,
        RestartPolicy::No,
    ];
    Ok(match &form.kind {
        FormKind::NewProject => Request::Project(CreateProject {
            name: named("name")?,
            description: Some(form.get("description")).filter(|d| !d.is_empty()),
        }),
        FormKind::NewEnvironment { project_id } => Request::Environment(
            project_id.clone(),
            CreateEnvironment {
                name: named("name")?,
            },
        ),
        FormKind::NewService { environment_id } => {
            let source = match form.choice("source") {
                0 => {
                    let repo = form.get("repo");
                    if repo.is_empty() {
                        return Err("repository is required".into());
                    }
                    let or = |key: &str, default: &str| {
                        Some(form.get(key))
                            .filter(|v| !v.is_empty())
                            .unwrap_or(default.into())
                    };
                    Source::Git {
                        repo,
                        branch: or("branch", "main"),
                        dockerfile: or("dockerfile", "Dockerfile"),
                        context: ".".into(),
                    }
                }
                1 => {
                    let image = form.get("image");
                    if image.is_empty() {
                        return Err("image is required".into());
                    }
                    Source::Image { image }
                }
                _ => {
                    let kind = DatabaseKind::ALL[form.choice("engine")];
                    let version = Some(form.get("version")).filter(|v| !v.is_empty());
                    Source::Database {
                        kind,
                        version: version.unwrap_or_else(|| kind.default_version().into()),
                    }
                }
            };
            let name = match form.get("name") {
                n if n.is_empty() => match &source {
                    Source::Git { repo, .. } => derive_name(repo),
                    Source::Image { image } => derive_name(image),
                    Source::Database { kind, .. } => kind.image().to_string(),
                },
                n => n,
            };
            validate_name(&name)?;
            let domain = form.get("domain");
            let port = port(&form.get("port"), !domain.is_empty())?;
            Request::Service(Box::new(CreateService {
                environment_id: environment_id.clone(),
                server_id: None,
                name,
                source,
                port,
                command: None,
                restart: RestartPolicy::UnlessStopped,
                health_path: None,
                env: Vec::new(),
                domains: if domain.is_empty() {
                    Vec::new()
                } else {
                    vec![domain]
                },
                volumes: Vec::new(),
                deploy: !alt,
            }))
        }
        FormKind::EditService { service_id } => {
            let source = if !form.get("repo").is_empty() {
                Some(Source::Git {
                    repo: form.get("repo"),
                    branch: form.get("branch"),
                    dockerfile: form.get("dockerfile"),
                    context: ".".into(),
                })
            } else if !form.get("image").is_empty() {
                Some(Source::Image {
                    image: form.get("image"),
                })
            } else {
                None
            };
            Request::Update {
                id: service_id.clone(),
                body: UpdateService {
                    name: Some(named("name")?),
                    source,
                    port: Some(port(&form.get("port"), false)?.unwrap_or(0)),
                    command: Some(form.get("command")),
                    restart: Some(policies[form.choice("restart")]),
                    health_path: Some(form.get("health")),
                    server_id: None,
                },
                deploy: alt,
            }
        }
        FormKind::AddDomain { service_id } => {
            let host = form.get("host").to_lowercase();
            let host = host
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_end_matches('/');
            if !host.contains('.') || host.contains(' ') {
                return Err("enter a hostname like shop.example.com".into());
            }
            Request::Domain(
                service_id.clone(),
                CreateDomain {
                    host: host.to_string(),
                    port: port(&form.get("port"), false)?,
                    tls: form.on("tls"),
                },
            )
        }
        FormKind::AddVolume { service_id } => {
            let path = form.get("path");
            if !path.starts_with('/') {
                return Err("mount path must be absolute, like /app/data".into());
            }
            Request::Volume {
                id: service_id.clone(),
                body: CreateVolume {
                    name: named("name")?,
                    mount_path: path,
                },
                deploy: alt,
            }
        }
        FormKind::AddServer => {
            let host = form.get("host");
            if host.is_empty() {
                return Err("host is required".into());
            }
            Request::Server(CreateServer {
                name: named("name")?,
                host,
                user: Some(form.get("user"))
                    .filter(|u| !u.is_empty())
                    .unwrap_or("root".into()),
                port: port(&form.get("port"), false)?.unwrap_or(22),
            })
        }
        FormKind::EnvVar { .. } => return Err("internal: variable forms submit separately".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_derive_from_sources() {
        assert_eq!(derive_name("https://github.com/acme/api.git"), "api");
        assert_eq!(
            derive_name("git@github.com:acme/Shop_Front.git"),
            "shop-front"
        );
        assert_eq!(derive_name("ghcr.io/imgproxy/imgproxy:v3"), "imgproxy");
        assert_eq!(derive_name("traefik/whoami"), "whoami");
        assert_eq!(derive_name("nginx:1.27"), "nginx");
    }

    #[test]
    fn levels_are_detected() {
        assert_eq!(
            level_of("2026-11-20 ERROR stripe webhook failed"),
            Some(Level::Error)
        );
        assert_eq!(level_of("[warn] slow query"), Some(Level::Warn));
        assert_eq!(level_of("GET / 200 38ms"), None);
    }

    #[test]
    fn log_buffer_follows_and_scrolls() {
        let mut logs = LogBuffer::new();
        for i in 0..50 {
            logs.push(LogLine {
                ts: None,
                stream: LogStream::Stdout,
                line: format!("line {i}"),
            });
        }
        logs.scroll(10, 20);
        assert!(!logs.follow);
        assert_eq!(logs.offset, 10);
        logs.push(LogLine {
            ts: None,
            stream: LogStream::Stdout,
            line: "new".into(),
        });
        assert_eq!(logs.offset, 11, "view stays put while new lines arrive");
        logs.scroll(-100, 20);
        assert!(logs.follow);
        logs.search = "line 4".into();
        assert_eq!(logs.matches().len(), 11);
    }
}
