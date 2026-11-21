//! Application state, background requests and the actions keys trigger.

use crate::form::{Field, Form, FormKind};
use crate::notify::{self, NotifyView, RuleDialog};
use crate::picker::{GitPick, PickFor, PickItem, PickKind, Picker, repo_item};
use crate::theme;
use client::{Client, Config};
use common::api::*;
use common::notify as notify_types;
use common::*;
use futures_util::StreamExt;
use ratatui::text::Line;
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::AbortHandle;

const LOG_CAP: usize = 5000;
const REFRESH: Duration = Duration::from_secs(3);
/// Settings are heavier to compute (disk usage, Docker), so they refresh less often.
const SETTINGS_REFRESH: Duration = Duration::from_secs(15);
/// Repositories per page of the repository picker.
pub const REPO_PAGE: u32 = 10;
/// How long typing in the repository search pauses before the daemon is asked.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

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
    OpenLinkedDeployment(Id, Result<Box<Deployment>, String>),
    DeploymentCancelled(Result<Box<Deployment>, String>),
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
    Settings(Box<InstanceSettings>),
    Release(VersionCheck),
    /// The token was rotated and, unless the context came from the environment, saved.
    TokenRotated(Result<(String, bool), String>),
    /// An update finished: this CLI binary was replaced, and the daemon accepted its update and
    /// is restarting. `None` for a side that was not asked to update.
    Updated {
        cli: Option<Result<UpdateResult, String>>,
        server: Option<Result<UpdateResult, String>>,
    },
    Backups(Id, Vec<Backup>),
    Schedule(Id, BackupSchedule),
    /// A backup was started or polled.
    Backup(Box<Backup>),
    /// A blocking confirm finished: close it and show the outcome.
    ConfirmDone(Result<String, String>),
    /// The password was rotated; show the new URL once.
    Rotated(Box<DatabaseInfo>),
    /// Progress of a running download, shown as a busy toast.
    Progress(String),
    Secrets(Vec<Secret>),
    /// A secret's value, asked for with reveal.
    Revealed(Box<RevealedSecret>),
    /// A key generated from the picker.
    PickerKey(Result<Box<Secret>, String>),
    Storages(Vec<StorageConnection>),
    /// A storage test finished.
    StorageTested(Id, Result<StorageTest, String>),
    InstanceBackups(Vec<InstanceBackup>),
    InstanceSchedule(InstanceBackupSchedule),
    NotifyChannels(Vec<notify_types::NotifyChannel>),
    NotifyRules(Vec<notify_types::NotifyRule>),
    NotifyDeliveries(Vec<notify_types::NotifyDelivery>),
    /// A test sent from the channel list finished.
    NotifyTested(Id, Result<notify_types::NotifyDelivery, String>),
    /// The wizard saved a channel; `true` when the wizard created it.
    NotifyStep(Box<notify_types::NotifyChannel>, bool),
    /// The wizard's test message finished.
    NotifyStepTested(Id, Result<notify_types::NotifyDelivery, String>),
    /// The rule dialog or the wizard's last step finished saving.
    NotifyDialogDone(Result<String, String>),
    Sources(Vec<GitSource>),
    /// A source polled while its setup dialog is open.
    Source(GitSource),
    /// A source check finished.
    SourceChecked(Id, Result<GitSource, String>),
    /// The instance domain, with the toast to show when an action produced it.
    Domain(Result<InstanceDomain, String>, Option<String>),
    Repos {
        source: Id,
        query: String,
        page: u32,
        result: Result<RepoPage, String>,
    },
    Branches {
        source: Id,
        repo: String,
        result: Result<Vec<GitBranch>, String>,
    },
    /// What the picked repository and branch build with.
    Detected {
        source: Id,
        repo: String,
        branch: String,
        result: Result<BuildDetection, String>,
    },
}

/// Where to go after a form succeeds.
pub enum Then {
    Stay,
    /// Show the settings the daemon answered with.
    Settings(Box<InstanceSettings>),
    OpenService(Id, Tab),
    OpenDeploy(Box<Deployment>),
    /// Show the public key of a key the daemon just generated.
    ShowKey(Box<Secret>),
    InstanceSchedule(InstanceBackupSchedule),
    /// Show the URL that creates a GitHub App, and follow its setup.
    GithubSetup(Box<GithubAppSetup>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Service,
    Deploy,
    Servers,
    Settings,
    Secrets,
    Storage,
    Notifications,
    Sources,
    /// The instance domain guide, reached from Settings.
    Domain,
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
    Database,
    Backups,
    Env,
    Domains,
    Volumes,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 9] = [
        Tab::Overview,
        Tab::Deployments,
        Tab::Logs,
        Tab::Database,
        Tab::Backups,
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
            Tab::Database => "Database",
            Tab::Backups => "Backups",
            Tab::Env => "Env",
            Tab::Domains => "Domains",
            Tab::Volumes => "Volumes",
            Tab::Settings => "Settings",
        }
    }

    /// Tabs of a service; the database tabs only exist for database services.
    pub fn of(database: bool) -> Vec<Tab> {
        Tab::ALL
            .into_iter()
            .filter(|t| database || !matches!(t, Tab::Database | Tab::Backups))
            .collect()
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
    pub activity_offset: usize,
}

/// Scrollback with follow mode, search and filters, used for runtime and build logs.
#[derive(Default)]
pub struct LogBuffer {
    pub lines: VecDeque<LogLine>,
    pub follow: bool,
    /// Lines scrolled up from the bottom when not following.
    pub offset: usize,
    pub step_anchor: Option<usize>,
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
            self.step_anchor = self.step_anchor.and_then(|line| line.checked_sub(1));
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
        if let Some(start) = self.step_anchor {
            let next = start.saturating_add_signed(-delta);
            if next > self.lines.len().saturating_sub(viewport) {
                self.step_anchor = Some(next.min(self.lines.len().saturating_sub(1)));
                return;
            }
            self.step_anchor = None;
            self.offset = self.lines.len().saturating_sub(start + viewport);
        }
        let max = self.visible().len().saturating_sub(viewport);
        let next = (self.offset as isize + delta).clamp(0, max as isize) as usize;
        self.offset = next;
        self.follow = next == 0;
    }

    pub fn jump_to_step(&mut self, kind: StepKind) -> bool {
        let header = format!("==> {}", kind.as_str());
        let Some(index) = self
            .lines
            .iter()
            .position(|line| line.stream == LogStream::System && line.line == header)
        else {
            return false;
        };
        self.follow = false;
        self.step_anchor = Some(index);
        true
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
    pub deployment_row: usize,
    pub logs: LogBuffer,
    pub log_task: Option<AbortHandle>,
    pub row: usize,
    pub reveal: bool,
    /// A database service: shows the Database and Backups tabs.
    pub database: bool,
    /// The compose service whose logs the Logs tab shows.
    pub container: Option<String>,
}

impl ServiceView {
    /// The services of a compose build; empty for other build methods.
    pub fn compose(&self) -> &[ComposeService] {
        self.detail.as_ref().map_or(&[], |d| d.compose.as_slice())
    }

    /// The compose service the Logs tab follows: the one picked, else the first.
    pub fn log_container(&self) -> Option<&str> {
        let compose = self.compose();
        self.container
            .as_deref()
            .filter(|c| compose.iter().any(|s| s.name == *c))
            .or(compose.first().map(|s| s.name.as_str()))
    }

    pub fn tabs(&self) -> Vec<Tab> {
        Tab::of(self.database)
    }

    pub fn info(&self) -> Option<&DatabaseInfo> {
        self.detail.as_ref()?.database.as_ref()
    }

    pub fn active_deployment(&self) -> Option<&Deployment> {
        self.deployments
            .iter()
            .find(|d| {
                matches!(
                    d.status,
                    DeploymentStatus::Building | DeploymentStatus::Starting
                )
            })
            .or_else(|| {
                self.deployments
                    .iter()
                    .find(|d| d.status == DeploymentStatus::Queued)
            })
    }
}

pub struct DeployView {
    pub deployment: Deployment,
    pub logs: LogBuffer,
    pub task: Option<AbortHandle>,
    /// Screen to return to on esc.
    pub back: Screen,
    pub announced: bool,
    pub pipeline_offset: usize,
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
    RemoveContext(String),
    RotateToken,
    Prune,
    Update {
        force: bool,
        cli: bool,
        server: bool,
    },
    Rollback(Id),
    CancelDeploy(Id),
    Stop(Id),
    /// Publish a database service on a host port, or make it internal with `None`.
    PublicPort(Id, Option<u16>),
    RotatePassword(Id),
    RestoreBackup(Id, String),
    DeleteBackup(Id),
    DeleteSecret(Id),
    DeleteStorage(Id),
    DeleteInstanceBackup(Id),
    DeleteNotifyChannel(Id),
    DeleteNotifyRule(Id),
    DeleteSource(Id),
    RemoveDomain,
}

impl ConfirmAction {
    /// Actions that keep the dialog open with a spinner until the request returns.
    fn progress(&self) -> Option<&'static str> {
        match self {
            ConfirmAction::PublicPort(..) => Some("applying"),
            ConfirmAction::RotatePassword(_) => Some("rotating"),
            ConfirmAction::RestoreBackup(..) => Some("restoring"),
            ConfirmAction::CancelDeploy(_) => Some("cancelling"),
            ConfirmAction::RotateToken => Some("rotating"),
            ConfirmAction::Prune => Some("pruning"),
            ConfirmAction::Update { .. } => Some("downloading"),
            ConfirmAction::DeleteSecret(_)
            | ConfirmAction::DeleteStorage(_)
            | ConfirmAction::DeleteSource(_) => Some("deleting"),
            _ => None,
        }
    }
}

pub struct Confirm {
    pub title: String,
    pub message: String,
    pub danger: bool,
    pub action: ConfirmAction,
    /// Label on the confirm chip, e.g. `restore`.
    pub verb: &'static str,
    /// Set while a blocking action runs.
    pub busy: Option<(Instant, &'static str)>,
    /// Why the daemon refused, with what to do about it; the dialog stays open to show it.
    pub error: Vec<String>,
}

/// A secret shown once, e.g. the URL after a password rotation.
pub struct Reveal {
    pub title: String,
    pub subtitle: String,
    pub message: String,
    pub value: String,
    pub note: String,
    /// What the copy toast names, e.g. "the token".
    pub what: &'static str,
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
    Settings,
    Secrets,
    Storage,
    Notifications,
    Sources,
    Domain,
    BackUpInstance,
    AddServer,
    SetupProxy,
    Refresh,
    Help,
    Quit,
}

impl Command {
    pub const ALL: [Command; 21] = [
        Command::Deploy,
        Command::Restart,
        Command::Stop,
        Command::Start,
        Command::Logs,
        Command::NewService,
        Command::NewProject,
        Command::NewEnvironment,
        Command::Servers,
        Command::Settings,
        Command::Secrets,
        Command::Storage,
        Command::Notifications,
        Command::Sources,
        Command::Domain,
        Command::BackUpInstance,
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
            Command::Settings => "settings",
            Command::Secrets => "secrets",
            Command::Storage => "storage",
            Command::Notifications => "notifications",
            Command::Sources => "sources",
            Command::Domain => "instance domain",
            Command::BackUpInstance => "back up this instance",
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
            Command::Settings => ",",
            Command::Secrets
            | Command::Storage
            | Command::Notifications
            | Command::Sources
            | Command::Domain
            | Command::BackUpInstance
            | Command::AddServer
            | Command::SetupProxy => "",
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
    Reveal(Reveal),
    Picker(Picker),
    Rule(RuleDialog),
    ChannelTest(notify::ChannelTestStep),
    /// The URL that creates a GitHub App and the setup's progress, updated live.
    GithubSetup(GithubSetup),
    /// A GitHub App needs a verified instance domain first; holds the dialog's title.
    DomainNeeded(&'static str),
    Help,
}

/// A GitHub App being created with the manifest flow.
pub struct GithubSetup {
    pub source: GitSource,
    /// The manifest URL, known only right after creation; later the install URL is shown.
    pub setup_url: Option<String>,
}

impl GithubSetup {
    /// Where the browser needs to go next.
    pub fn url(&self) -> Option<&str> {
        match self.source.status {
            GitSourceStatus::Pending => self
                .setup_url
                .as_deref()
                .or(self.source.install_url.as_deref()),
            _ => self
                .source
                .install_url
                .as_deref()
                .or(self.setup_url.as_deref()),
        }
    }
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    /// Work in progress: drawn with a spinner instead of a check mark.
    pub busy: bool,
    pub at: Instant,
}

/// Results sent from background work, tagged with the connection they were made on so replies
/// from a daemon the TUI has switched away from are dropped.
#[derive(Clone)]
pub struct Sender {
    tx: UnboundedSender<(u64, Msg)>,
    epoch: u64,
}

impl Sender {
    pub fn send(&self, msg: Msg) -> bool {
        self.tx.send((self.epoch, msg)).is_ok()
    }
}

/// Which list on the Settings screen the arrows and ⏎ act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsFocus {
    #[default]
    Backups,
    Contexts,
}

/// The Settings screen: the instance, its backups, its release, and the contexts saved on this
/// machine.
#[derive(Default)]
pub struct SettingsView {
    pub focus: SettingsFocus,
    /// Instance backups, newest first.
    pub backups: Vec<InstanceBackup>,
    pub backup_row: usize,
    pub schedule: Option<InstanceBackupSchedule>,
    pub settings: Option<InstanceSettings>,
    pub release: Option<VersionCheck>,
    pub contexts: Config,
    pub row: usize,
    loaded_at: Option<Instant>,
    /// Screen to return to on esc.
    pub back: Option<Screen>,
}

impl SettingsView {
    pub fn context_names(&self) -> Vec<String> {
        self.contexts.contexts.keys().cloned().collect()
    }

    pub fn selected_backup(&self) -> Option<&InstanceBackup> {
        self.backups.get(self.backup_row)
    }
}

/// The Secrets screen. A revealed value is dropped as soon as another secret is selected.
#[derive(Default)]
pub struct SecretsView {
    pub list: Vec<Secret>,
    pub row: usize,
    /// The value asked for with reveal, and when it arrived.
    pub revealed: Option<(RevealedSecret, chrono::DateTime<chrono::Utc>)>,
    /// Screen to return to on esc.
    pub back: Option<Screen>,
}

impl SecretsView {
    pub fn selected(&self) -> Option<&Secret> {
        self.list.get(self.row)
    }

    /// The revealed value of the selected secret.
    pub fn value(&self) -> Option<&SecretValue> {
        let (r, _) = self.revealed.as_ref()?;
        (Some(&r.secret.id) == self.selected().map(|s| &s.id)).then_some(&r.value)
    }

    pub fn name(&self, id: &str) -> Option<&str> {
        self.list
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.as_str())
    }
}

/// The instance domain guide: the domain, the DNS records to create and the checks.
#[derive(Default)]
pub struct DomainView {
    pub data: Option<InstanceDomain>,
    /// The domain being typed; `Some` while the input is open.
    pub input: Option<String>,
    /// What runs right now, e.g. "checking".
    pub busy: Option<&'static str>,
    /// Screen to return to on esc.
    pub back: Option<Screen>,
}

/// The Sources screen: where services get their code.
#[derive(Default)]
pub struct SourcesView {
    pub list: Vec<GitSource>,
    pub loaded: bool,
    pub row: usize,
    /// Sources being checked right now.
    pub checking: HashSet<Id>,
    /// Screen to return to on esc.
    pub back: Option<Screen>,
}

impl SourcesView {
    pub fn selected(&self) -> Option<&GitSource> {
        self.list.get(self.row)
    }

    pub fn ready(&self) -> Vec<&GitSource> {
        self.list
            .iter()
            .filter(|s| s.status == GitSourceStatus::Ready)
            .collect()
    }
}

/// "github app · acme", what a source is and whose.
pub fn source_summary(s: &GitSource) -> String {
    match &s.owner {
        Some(owner) => format!("{} · {owner}", s.kind.label()),
        None => s.kind.label().to_string(),
    }
}

/// The Storage screen: S3-compatible buckets for backups.
#[derive(Default)]
pub struct StorageView {
    pub list: Vec<StorageConnection>,
    pub row: usize,
    /// Connections being tested right now.
    pub testing: HashSet<Id>,
    /// Screen to return to on esc.
    pub back: Option<Screen>,
}

impl StorageView {
    pub fn selected(&self) -> Option<&StorageConnection> {
        self.list.get(self.row)
    }

    pub fn name(&self, id: &str) -> Option<&str> {
        self.list
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.as_str())
    }
}

pub struct App {
    pub client: Client,
    pub context: String,
    tx: UnboundedSender<(u64, Msg)>,
    /// Bumped on every context switch; see `Sender`.
    pub epoch: u64,
    pub tick: usize,
    pub info: Option<DaemonInfo>,
    pub connection_error: Option<String>,
    pub tree: Option<Tree>,
    pub activity: Vec<Activity>,
    pub servers: Vec<Server>,
    pub details: HashMap<Id, ServiceDetail>,
    pub stats: HashMap<Id, ServiceStats>,
    /// Backups of database services, newest first.
    pub backups: HashMap<Id, Vec<Backup>>,
    pub schedules: HashMap<Id, BackupSchedule>,
    pub screen: Screen,
    pub overview: OverviewState,
    pub service: Option<ServiceView>,
    pub deploy: Option<DeployView>,
    pub server_row: usize,
    servers_back: Screen,
    pub settings: SettingsView,
    pub secrets: SecretsView,
    pub storage: StorageView,
    pub notify: NotifyView,
    pub sources: SourcesView,
    pub domain: DomainView,
    /// Version the daemon is restarting into after an update.
    pub updating: Option<String>,
    /// Version this CLI binary was replaced with. The running TUI stays the old one until restarted.
    pub cli_updated: Option<String>,
    /// No new requests while the token is being rotated: the old one is about to stop working.
    rotating: bool,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    pub quit: bool,
    last_refresh: Option<Instant>,
    pub mouse: crate::mouse::MouseState,
    /// Height of the log viewport at the last draw, for paging.
    pub log_viewport: Cell<usize>,
}

impl App {
    pub fn new(client: Client, context: String, tx: UnboundedSender<(u64, Msg)>) -> Self {
        Self {
            client,
            context,
            tx,
            epoch: 0,
            tick: 0,
            info: None,
            connection_error: None,
            tree: None,
            activity: Vec::new(),
            servers: Vec::new(),
            details: HashMap::new(),
            stats: HashMap::new(),
            backups: HashMap::new(),
            schedules: HashMap::new(),
            screen: Screen::Overview,
            overview: OverviewState::default(),
            service: None,
            deploy: None,
            server_row: 0,
            servers_back: Screen::Overview,
            settings: SettingsView::default(),
            secrets: SecretsView::default(),
            storage: StorageView::default(),
            notify: NotifyView::default(),
            sources: SourcesView::default(),
            domain: DomainView::default(),
            updating: None,
            cli_updated: None,
            rotating: false,
            overlay: None,
            toast: None,
            quit: false,
            last_refresh: None,
            mouse: crate::mouse::MouseState::new(),
            log_viewport: Cell::new(20),
        }
    }

    pub fn sender(&self) -> Sender {
        Sender {
            tx: self.tx.clone(),
            epoch: self.epoch,
        }
    }

    fn spawn<F>(&self, f: impl FnOnce(Client) -> F)
    where
        F: Future<Output = Msg> + Send + 'static,
    {
        let tx = self.sender();
        let fut = f(self.client.clone());
        tokio::spawn(async move {
            tx.send(fut.await);
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
            busy: false,
            at: Instant::now(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: true,
            busy: false,
            at: Instant::now(),
        });
    }

    fn progress(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            error: false,
            busy: true,
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
        if self.tick.is_multiple_of(16) {
            self.poll_setup();
        }
        self.search_repos_when_due();
        if self.tick.is_multiple_of(8) {
            self.poll_running_backups();
            if self.screen == Screen::Settings
                && self
                    .settings
                    .backups
                    .iter()
                    .any(|b| b.status == BackupStatus::Running)
            {
                self.load_instance_backups();
            }
        }
    }

    /// Follow backups that are still running in the open service.
    fn poll_running_backups(&self) {
        let Some(view) = self.service.as_ref().filter(|v| v.database) else {
            return;
        };
        for b in self.backups.get(&view.id).into_iter().flatten() {
            if b.status == BackupStatus::Running {
                let id = b.id.clone();
                self.spawn(|c| async move {
                    match c.backup(&id).await {
                        Ok(b) => Msg::Backup(Box::new(b)),
                        Err(e) => Msg::Error(format!("{e:#}")),
                    }
                });
            }
        }
    }

    pub fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        if self.rotating {
            return;
        }
        if self.screen == Screen::Settings
            && self
                .settings
                .loaded_at
                .is_none_or(|t| t.elapsed() >= SETTINGS_REFRESH)
        {
            self.load_settings();
        }
        self.spawn(|c| async move { Msg::Info(c.info().await.map_err(|e| format!("{e:#}"))) });
        self.spawn(|c| async move {
            match c.tree().await {
                Ok(t) => Msg::Tree(t),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
        self.spawn(|c| async move { Msg::Activity(c.activity(50).await.unwrap_or_default()) });
        if matches!(self.screen, Screen::Servers | Screen::Settings) || self.servers.is_empty() {
            self.spawn(|c| async move { Msg::Servers(c.servers().await.unwrap_or_default()) });
        }
        if matches!(
            self.screen,
            Screen::Servers | Screen::Settings | Screen::Secrets | Screen::Storage
        ) {
            self.load_secrets();
        }
        if matches!(self.screen, Screen::Settings | Screen::Storage)
            || self.service.as_ref().is_some_and(|v| v.database)
        {
            self.load_storages();
        }
        if self.screen == Screen::Settings {
            self.load_instance_backups();
        }
        if self.screen == Screen::Notifications {
            self.load_notify();
        }
        if matches!(
            self.screen,
            Screen::Overview | Screen::Settings | Screen::Sources
        ) {
            self.load_sources();
        }
        if self.screen == Screen::Domain && self.domain.busy.is_none() {
            self.load_domain();
        }
        let ids: Vec<Id> = match self.screen {
            Screen::Overview => self.env_services().iter().map(|s| s.id.clone()).collect(),
            Screen::Service | Screen::Deploy => self.service.iter().map(|s| s.id.clone()).collect(),
            Screen::Servers
            | Screen::Settings
            | Screen::Secrets
            | Screen::Storage
            | Screen::Notifications
            | Screen::Sources
            | Screen::Domain => Vec::new(),
        };
        for id in ids {
            self.load_stats(&id);
        }
        if let Some(s) = self.selected_service() {
            let (id, database) = (s.id.clone(), is_database(s));
            self.load_detail(&id);
            if database {
                self.load_backups(&id);
            }
        }
        if let Some(view) = &self.service {
            let (id, database) = (view.id.clone(), view.database);
            self.load_detail(&id);
            self.load_deployments(&id);
            if database {
                self.load_backups(&id);
            }
        }
    }

    pub fn load_storages(&self) {
        self.spawn(|c| async move {
            match c.storages().await {
                Ok(s) => Msg::Storages(s),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    pub fn load_secrets(&self) {
        self.spawn(|c| async move {
            match c.secrets().await {
                Ok(s) => Msg::Secrets(s),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    /// Backups and the schedule of a database service.
    fn load_backups(&self, id: &str) {
        let id = id.to_string();
        let sid = id.clone();
        self.spawn(|c| async move {
            match c.backups(&id).await {
                Ok(b) => Msg::Backups(id, b),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
        self.spawn(|c| async move {
            match c.backup_schedule(&sid).await {
                Ok(s) => Msg::Schedule(sid, s),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
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
                if self.updating.as_deref() == Some(info.version.as_str()) {
                    self.updating = None;
                    let mut text = format!("{} updated to {}", brand::DAEMON_BIN, info.version);
                    if let Some(to) = &self.cli_updated {
                        text.push_str(&format!(" · restart {} to use {to}", brand::CLI_BIN));
                    }
                    self.toast(text);
                    self.load_settings();
                    self.load_release();
                }
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
                    view.database |= d.database.is_some();
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
            Msg::OpenLinkedDeployment(service_id, result) => {
                if self.screen == Screen::Service
                    && self.overlay.is_none()
                    && self
                        .service
                        .as_ref()
                        .is_some_and(|view| view.id == service_id)
                {
                    match result {
                        Ok(deployment) => self.open_deploy(*deployment),
                        Err(e) => self.error(e),
                    }
                }
            }
            Msg::DeploymentCancelled(result) => {
                let result = result.map(|d| {
                    let number = d.number;
                    if let Some(service) = self.service.as_mut().filter(|v| v.id == d.service_id)
                        && let Some(row) = service.deployments.iter_mut().find(|row| row.id == d.id)
                    {
                        *row = (*d).clone();
                    }
                    if let Some(view) = self.deploy.as_mut().filter(|v| v.deployment.id == d.id) {
                        view.deployment = *d;
                        view.announced = true;
                    }
                    format!("deployment #{number} cancelled")
                });
                self.apply(Msg::ConfirmDone(result));
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
                    Then::Settings(s) => self.apply(Msg::Settings(s)),
                    Then::OpenService(id, tab) => self.open_service(&id, tab),
                    Then::OpenDeploy(d) => self.open_deploy(*d),
                    Then::ShowKey(s) => self.show_key(&s),
                    Then::InstanceSchedule(s) => self.settings.schedule = Some(s),
                    Then::GithubSetup(setup) => {
                        let GithubAppSetup { source, setup_url } = *setup;
                        self.upsert_source(source.clone());
                        self.overlay = Some(Overlay::GithubSetup(GithubSetup {
                            source,
                            setup_url: Some(setup_url),
                        }));
                    }
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
            Msg::Settings(s) => {
                self.settings.settings = Some(*s);
                self.settings.loaded_at = Some(Instant::now());
            }
            Msg::Release(r) => self.settings.release = Some(r),
            Msg::TokenRotated(Err(e)) => {
                self.rotating = false;
                self.apply(Msg::ConfirmDone(Err(e)));
            }
            Msg::TokenRotated(Ok((token, saved))) => {
                self.rotating = false;
                self.client = self.client.with_token(&token);
                self.settings.contexts = Config::load().unwrap_or_default();
                if !saved {
                    self.error(format!(
                        "token rotated · update {} where it is set",
                        brand::env_var("TOKEN")
                    ));
                }
                self.overlay = Some(Overlay::Reveal(Reveal {
                    title: "New API token".into(),
                    subtitle: self.context.clone(),
                    message: format!(
                        "The old token stopped working. This machine's context {} now uses the new one.",
                        self.context
                    ),
                    value: token,
                    note: "Shown once. Copy it now to log in from another machine.".into(),
                    what: "the token",
                }));
                self.refresh();
            }
            Msg::Updated { cli, server } => {
                if matches!(self.overlay, Some(Overlay::Confirm(_))) {
                    self.overlay = None;
                }
                let mut done = Vec::new();
                let mut failed = Vec::new();
                match cli {
                    Some(Ok(r)) if r.updated => {
                        done.push(format!(
                            "{} {} → {} · restart it to use it",
                            brand::CLI_BIN,
                            r.from,
                            r.to
                        ));
                        self.cli_updated = Some(r.to);
                    }
                    Some(Ok(r)) => {
                        done.push(format!("{} {} is up to date", brand::CLI_BIN, r.from))
                    }
                    Some(Err(e)) => failed.push(format!("{}: {e}", brand::CLI_BIN)),
                    None => {}
                }
                match server {
                    Some(Ok(r)) if r.updated => self.updating = Some(r.to),
                    Some(Ok(r)) => {
                        done.push(format!("{} {} is up to date", brand::DAEMON_BIN, r.from))
                    }
                    Some(Err(e)) => failed.push(format!("{}: {e}", brand::DAEMON_BIN)),
                    None => {}
                }
                match (&self.updating, failed.is_empty()) {
                    (_, false) => self.error(failed.join(" · ")),
                    (Some(to), true) => self.progress(format!("installed {to} · restarting")),
                    (None, true) => self.toast(done.join(" · ")),
                }
            }
            Msg::Backups(id, backups) => {
                if let Some(view) = self.service.as_mut().filter(|v| v.id == id)
                    && view.tab == Tab::Backups
                {
                    view.row = view.row.min(backups.len().saturating_sub(1));
                }
                let finished: Vec<Backup> = backups
                    .iter()
                    .filter(|b| b.status != BackupStatus::Running && self.was_running(b))
                    .cloned()
                    .collect();
                self.backups.insert(id, backups);
                for b in &finished {
                    self.announce(b);
                }
            }
            Msg::Schedule(id, schedule) => {
                self.schedules.insert(id, schedule);
            }
            Msg::Backup(b) => {
                let finished = b.status != BackupStatus::Running && self.was_running(&b);
                let list = self.backups.entry(b.service_id.clone()).or_default();
                match list.iter_mut().find(|x| x.id == b.id) {
                    Some(slot) => *slot = (*b).clone(),
                    None => list.insert(0, (*b).clone()),
                }
                if finished {
                    self.announce(&b);
                }
            }
            Msg::ConfirmDone(Err(e))
                if matches!(
                    &self.overlay,
                    Some(Overlay::Confirm(Confirm {
                        action: ConfirmAction::DeleteSecret(_)
                            | ConfirmAction::DeleteStorage(_)
                            | ConfirmAction::DeleteSource(_),
                        ..
                    }))
                ) =>
            {
                let hint = match (self.screen, self.secrets.selected().map(|s| s.kind)) {
                    (Screen::Storage, _) => None,
                    (Screen::Sources, _) => {
                        Some("Point those services at another source first, on their Settings tab.")
                    }
                    (_, Some(SecretKind::GithubApp)) => {
                        Some("Delete the source that uses it instead, on the Sources screen.")
                    }
                    (_, Some(SecretKind::SshKey)) => Some(
                        "Pick another key for those servers first: Servers, select the server, K change key.",
                    ),
                    _ => Some("Point those at other credentials first, on the Storage screen."),
                };
                if let Some(Overlay::Confirm(c)) = &mut self.overlay {
                    c.busy = None;
                    c.error = std::iter::once(e).chain(hint.map(String::from)).collect();
                }
            }
            Msg::ConfirmDone(result) => {
                if matches!(self.overlay, Some(Overlay::Confirm(_))) {
                    self.overlay = None;
                }
                match result {
                    Ok(t) => self.toast(t),
                    Err(e) => self.error(e),
                }
                self.refresh();
            }
            Msg::Rotated(info) => {
                self.overlay = Some(Overlay::Reveal(Reveal {
                    title: "Password rotated".into(),
                    subtitle: info.host.clone(),
                    message: format!(
                        "The password changed. This is the new internal URL; {} on {} already has it.",
                        crate::database::url_var(info.kind),
                        info.host
                    ),
                    value: info.internal_url.clone(),
                    note: format!(
                        "Redeploy the services that connect to {} so they pick it up. It stays masked on the Database tab after you close this.",
                        info.host
                    ),
                    what: "the new URL",
                }));
                self.refresh();
            }
            Msg::Progress(text) => self.progress(text),
            Msg::Secrets(list) => {
                let view = &mut self.secrets;
                let selected = view.selected().map(|s| s.id.clone());
                view.list = list;
                view.row = selected
                    .and_then(|id| view.list.iter().position(|s| s.id == id))
                    .unwrap_or(view.row)
                    .min(view.list.len().saturating_sub(1));
                self.refill_picker();
            }
            Msg::Revealed(r) => {
                if self.secrets.selected().map(|s| &s.id) == Some(&r.secret.id) {
                    self.secrets.revealed = Some((*r, chrono::Utc::now()));
                }
            }
            Msg::PickerKey(Err(e)) => {
                if let Some(Overlay::Picker(p)) = &mut self.overlay {
                    p.busy = false;
                }
                self.error(e);
            }
            Msg::PickerKey(Ok(s)) => {
                let name = s.name.clone();
                self.secrets.list.push((*s).clone());
                self.refill_picker();
                if let Some(Overlay::Picker(p)) = &mut self.overlay {
                    p.busy = false;
                    p.select(&s.id);
                }
                self.toast(format!("generated {name} · c copies its public key"));
                self.load_secrets();
            }
            Msg::Storages(list) => {
                let view = &mut self.storage;
                let selected = view.selected().map(|s| s.id.clone());
                view.list = list;
                view.row = selected
                    .and_then(|id| view.list.iter().position(|s| s.id == id))
                    .unwrap_or(view.row)
                    .min(view.list.len().saturating_sub(1));
                self.refill_picker();
            }
            Msg::InstanceBackups(list) => {
                let finished: Vec<InstanceBackup> =
                    list.iter()
                        .filter(|b| {
                            b.status != BackupStatus::Running
                                && self.settings.backups.iter().any(|old| {
                                    old.id == b.id && old.status == BackupStatus::Running
                                })
                        })
                        .cloned()
                        .collect();
                self.settings.backups = list;
                self.settings.backup_row = self
                    .settings
                    .backup_row
                    .min(self.settings.backups.len().saturating_sub(1));
                for b in finished {
                    match b.status {
                        BackupStatus::Done => self.toast(format!(
                            "instance backup done · {} · {}",
                            crate::util::bytes(b.size_bytes),
                            b.file_name
                        )),
                        _ => self.error(format!(
                            "instance backup failed: {}",
                            b.error.as_deref().unwrap_or("see the daemon log")
                        )),
                    }
                }
            }
            Msg::InstanceSchedule(s) => self.settings.schedule = Some(s),
            Msg::NotifyChannels(list) => {
                let view = &mut self.notify;
                let old = view.selected_channel().map(|c| c.id.clone());
                view.channel_row =
                    notify::keep_row(old.as_deref(), &list, |c| &c.id, view.channel_row);
                view.channels = list;
            }
            Msg::NotifyRules(list) => {
                let view = &mut self.notify;
                let old = view.selected_rule().map(|r| r.id.clone());
                view.rule_row = notify::keep_row(old.as_deref(), &list, |r| &r.id, view.rule_row);
                view.rules = list;
            }
            Msg::NotifyDeliveries(list) => {
                let view = &mut self.notify;
                let old = view.selected_delivery().map(|d| d.id);
                view.deliveries = list;
                let shown = view.deliveries();
                view.delivery_row = old
                    .and_then(|id| shown.iter().position(|d| d.id == id))
                    .unwrap_or(view.delivery_row)
                    .min(shown.len().saturating_sub(1));
                view.loaded_at = Some(Instant::now());
            }
            Msg::NotifyTested(id, result) => {
                self.notify.testing.remove(&id);
                let name = self
                    .notify
                    .channel(&id)
                    .map_or("channel".to_string(), |c| c.name.clone());
                match result {
                    Ok(d) if d.status == notify_types::DeliveryStatus::Sent => self.toast(format!(
                        "{name} ok · delivered in {}ms",
                        d.latency_ms.unwrap_or_default()
                    )),
                    Ok(d) => self.error(format!(
                        "{name} failed: {}",
                        d.error.as_deref().unwrap_or("no reason given")
                    )),
                    Err(e) => self.error(e),
                }
                self.load_notify();
            }
            Msg::NotifyStep(channel, created) => {
                let Some(Overlay::Form(mut form)) = self.overlay.take() else {
                    return;
                };
                if let FormKind::NotifyChannel {
                    channel_id,
                    created: was,
                    ..
                } = &mut form.kind
                {
                    *channel_id = Some(channel.id.clone());
                    *was = created;
                }
                form.busy = false;
                let id = channel.id.clone();
                self.overlay = Some(Overlay::ChannelTest(notify::ChannelTestStep {
                    channel: *channel,
                    form: Box::new(form),
                    created,
                    result: None,
                    saving: false,
                }));
                self.spawn(|c| async move {
                    let result = c
                        .test_notify_channel(&id)
                        .await
                        .map_err(|e| format!("{e:#}"));
                    Msg::NotifyStepTested(id, result)
                });
                self.load_notify();
            }
            Msg::NotifyStepTested(id, result) => {
                if let Some(Overlay::ChannelTest(step)) = &mut self.overlay
                    && step.channel.id == id
                {
                    step.result = Some(result);
                }
            }
            Msg::NotifyDialogDone(Ok(toast)) => {
                self.overlay = None;
                self.toast(toast);
                self.load_notify();
            }
            Msg::NotifyDialogDone(Err(e)) => match &mut self.overlay {
                Some(Overlay::Rule(d)) => {
                    d.busy = false;
                    d.error = Some(e);
                }
                Some(Overlay::ChannelTest(step)) => {
                    step.saving = false;
                    self.error(e);
                }
                _ => self.error(e),
            },
            Msg::Sources(list) => {
                let view = &mut self.sources;
                let selected = view.selected().map(|s| s.id.clone());
                view.list = list;
                view.loaded = true;
                view.row = selected
                    .and_then(|id| view.list.iter().position(|s| s.id == id))
                    .unwrap_or(view.row)
                    .min(view.list.len().saturating_sub(1));
            }
            Msg::Source(s) => self.upsert_source(s),
            Msg::SourceChecked(id, result) => {
                self.sources.checking.remove(&id);
                match result {
                    Ok(s) => {
                        match s.status {
                            GitSourceStatus::Ready => self.toast(format!("{} is ready", s.name)),
                            status => self.error(format!(
                                "{} is {}{}",
                                s.name,
                                status.as_str(),
                                s.status_detail
                                    .as_deref()
                                    .map(|d| format!(": {d}"))
                                    .unwrap_or_default()
                            )),
                        }
                        self.upsert_source(s);
                    }
                    Err(e) => self.error(e),
                }
            }
            Msg::Domain(result, toast) => {
                let acted = toast.is_some();
                self.domain.busy = None;
                match result {
                    Ok(d) => {
                        if acted || d.domain.is_none() {
                            self.domain.input = d.domain.is_none().then(String::new);
                        }
                        self.domain.data = Some(d);
                        if let Some(t) = toast {
                            self.toast(t);
                        }
                    }
                    Err(e) => self.error(e),
                }
                if acted {
                    self.load_settings();
                }
            }
            Msg::Repos {
                source,
                query,
                page,
                result,
            } => {
                let Some(Overlay::Picker(p)) = &mut self.overlay else {
                    return;
                };
                let Some(git) = p.git.as_mut().filter(|g| {
                    p.kind == PickKind::Repo
                        && g.source_id == source
                        && g.search == query
                        && g.page == page
                }) else {
                    return;
                };
                git.loading = false;
                match result {
                    Ok(repos) => {
                        git.error = None;
                        git.total = repos.total;
                        git.pages = repos.pages();
                        p.items = repos.repos.iter().map(repo_item).collect();
                        p.row = p
                            .current
                            .as_ref()
                            .and_then(|c| p.items.iter().position(|i| i.id.as_ref() == Some(c)))
                            .unwrap_or(0);
                    }
                    Err(e) => git.error = Some(e),
                }
            }
            Msg::Branches {
                source,
                repo,
                result,
            } => {
                let Some(Overlay::Picker(p)) = &mut self.overlay else {
                    return;
                };
                let Some(git) = p.git.as_mut().filter(|g| {
                    p.kind == PickKind::Branch && g.source_id == source && g.repo == repo
                }) else {
                    return;
                };
                git.loading = false;
                match result {
                    Ok(branches) => {
                        git.error = None;
                        git.branches = branches;
                        p.filter_branches();
                    }
                    Err(e) => git.error = Some(e),
                }
            }
            Msg::Detected {
                source,
                repo,
                branch,
                result,
            } => {
                let Some(Overlay::Form(form)) = &mut self.overlay else {
                    return;
                };
                if !matches!(form.kind, FormKind::NewService { .. })
                    || form.raw("git_source") != source
                    || form.raw("gh_repo") != repo
                    || form.raw("gh_branch") != branch
                {
                    return;
                }
                let Ok(found) = result else {
                    return;
                };
                let auto = form.choice("build") == 0;
                if let Some(build) = form.field_mut("build") {
                    build.placeholder = detected_note(&found);
                    if auto {
                        build.choice = BuildMethod::ALL
                            .iter()
                            .position(|m| *m == found.method)
                            .unwrap_or(0);
                    }
                }
                let key = match found.method {
                    BuildMethod::Compose => Some("compose_file"),
                    BuildMethod::Dockerfile => Some("dockerfile"),
                    _ => None,
                };
                if auto
                    && let (Some(key), Some(file)) = (key, found.file)
                    && let Some(f) = form.field_mut(key)
                {
                    f.value = file;
                }
                form.refresh();
            }
            Msg::StorageTested(id, result) => {
                self.storage.testing.remove(&id);
                let name = self.storage.name(&id).unwrap_or("storage").to_string();
                match result {
                    Ok(t) if t.ok => self.toast(format!(
                        "{name} ok · put, get and delete in {}ms",
                        t.latency_ms.unwrap_or_default()
                    )),
                    Ok(t) => self.error(format!(
                        "{name} failed: {}",
                        t.error.as_deref().unwrap_or("no reason given")
                    )),
                    Err(e) => self.error(e),
                }
                self.load_storages();
            }
        }
    }

    /// Rebuild an open picker's list from the latest secrets and storages.
    fn refill_picker(&mut self) {
        let key = self.daemon_key();
        if let Some(Overlay::Picker(p)) = &mut self.overlay {
            p.fill(&self.secrets.list, &self.storage.list, key.as_deref());
        }
    }

    fn was_running(&self, b: &Backup) -> bool {
        self.backups
            .get(&b.service_id)
            .and_then(|list| list.iter().find(|x| x.id == b.id))
            .is_some_and(|x| x.status == BackupStatus::Running)
    }

    /// Toast the outcome of a backup that was running.
    fn announce(&mut self, b: &Backup) {
        match b.status {
            BackupStatus::Done => self.toast(format!(
                "backup done · {} · {}",
                crate::util::bytes(b.size_bytes),
                b.file_name
            )),
            BackupStatus::Failed => self.error(format!(
                "backup failed: {}",
                b.error.as_deref().unwrap_or("see the daemon log")
            )),
            BackupStatus::Running => {}
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

    pub fn service_path(&self, id: &str) -> Option<(&Project, &Environment, &Service)> {
        if let Some(detail) = self
            .service
            .as_ref()
            .filter(|view| view.id == id)
            .and_then(|view| view.detail.as_ref())
            .or_else(|| self.details.get(id))
        {
            return Some((&detail.project, &detail.environment, &detail.service));
        }
        for p in &self.tree.as_ref()?.projects {
            for env in &p.environments {
                if let Some(service) = env.services.iter().find(|s| s.id == id) {
                    return Some((&p.project, &env.environment, service));
                }
            }
        }
        None
    }

    pub fn open_scope(&mut self, project: &str, environment: Option<&str>) {
        let Some((pi, p)) = self.tree.as_ref().and_then(|tree| {
            tree.projects
                .iter()
                .enumerate()
                .find(|(_, p)| p.project.id == project)
        }) else {
            return self.error("project is not in the overview");
        };
        let item = match environment {
            Some(id) => {
                let Some(ei) = p.environments.iter().position(|e| e.environment.id == id) else {
                    return self.error("environment is not in the overview");
                };
                SideItem::Environment(pi, ei)
            }
            None => SideItem::Project(pi),
        };
        self.overview.collapsed.remove(project);
        self.overview.side = self
            .side_items()
            .iter()
            .position(|x| *x == item)
            .unwrap_or(0);
        self.overview.row = 0;
        self.overview.pane = if environment.is_some() {
            Pane::Services
        } else {
            Pane::Sidebar
        };
        self.overview.filter.clear();
        self.overview.filtering = false;
        self.leave_deploy();
        self.close_service();
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
            deployment_row: 0,
            logs: LogBuffer::new(),
            log_task: None,
            row: 0,
            reveal: false,
            database: matches!(tab, Tab::Database | Tab::Backups)
                || self.details.get(id).is_some_and(|d| d.database.is_some())
                || self.all_services().any(|s| s.id == id && is_database(s)),
            container: None,
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
        view.logs.searching = false;
        if tab == Tab::Logs && view.log_task.is_none() {
            self.start_service_logs();
        }
    }

    pub fn open_service_overview(&mut self) {
        self.leave_deploy();
        self.screen = Screen::Service;
        self.set_tab(Tab::Overview);
    }

    pub fn open_service_deployment(&mut self, id: &str) {
        let Some(view) = &self.service else { return };
        if let Some(deployment) = view.deployments.iter().find(|d| d.id == id) {
            return self.open_deploy(deployment.clone());
        }
        let (id, service_id) = (id.to_string(), view.id.clone());
        self.spawn(|c| async move {
            Msg::OpenLinkedDeployment(
                service_id,
                c.deployment(&id)
                    .await
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    pub fn close_service(&mut self) {
        self.stop_streams();
        self.service = None;
        self.screen = Screen::Overview;
        self.refresh();
    }

    fn start_service_logs(&mut self) {
        let tx = self.sender();
        let client = self.client.clone();
        let Some(view) = self.service.as_mut() else {
            return;
        };
        if let Some(h) = view.log_task.take() {
            h.abort();
        }
        view.logs = LogBuffer::new();
        let id = view.id.clone();
        let container = view.log_container().map(str::to_string);
        let handle = tokio::spawn(async move {
            match client
                .service_logs(&id, 500, true, container.as_deref())
                .await
            {
                Ok(mut stream) => {
                    while let Some(item) = stream.next().await {
                        match item {
                            Ok(line) => {
                                if !tx.send(Msg::ServiceLog(id.clone(), line)) {
                                    return;
                                }
                            }
                            Err(e) => {
                                tx.send(Msg::ServiceLogEnd(id, Some(format!("{e:#}"))));
                                return;
                            }
                        }
                    }
                    tx.send(Msg::ServiceLogEnd(id, None));
                }
                Err(e) => {
                    tx.send(Msg::ServiceLogEnd(id, Some(format!("{e:#}"))));
                }
            }
        });
        view.log_task = Some(handle.abort_handle());
    }

    pub fn restart_service_logs(&mut self) {
        self.start_service_logs();
    }

    /// Follow the logs of compose service `i` on the Logs tab.
    pub fn pick_container(&mut self, i: usize) {
        let Some(view) = self.service.as_mut() else {
            return;
        };
        let Some(name) = view.compose().get(i).map(|s| s.name.clone()) else {
            return;
        };
        view.container = Some(name);
        if view.tab == Tab::Logs {
            self.start_service_logs();
        } else {
            self.set_tab(Tab::Logs);
        }
    }

    /// "c" on the Logs tab of a compose service: the next container.
    pub fn next_container(&mut self) {
        let Some(view) = self.service.as_ref() else {
            return;
        };
        let compose = view.compose();
        if compose.is_empty() {
            return;
        }
        let at = view
            .log_container()
            .and_then(|c| compose.iter().position(|s| s.name == c))
            .unwrap_or(0);
        self.pick_container((at + 1) % compose.len());
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
        let tx = self.sender();
        let handle = tokio::spawn(async move {
            if let Ok(mut stream) = client.deployment_logs(&id, true).await {
                while let Some(Ok(line)) = stream.next().await {
                    if !tx.send(Msg::DeployLog(id.clone(), line)) {
                        return;
                    }
                }
            }
            tx.send(Msg::DeployLogEnd(id));
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
            pipeline_offset: 0,
        });
        self.screen = Screen::Deploy;
    }

    pub fn close_deploy(&mut self) {
        let back = self.deploy.as_ref().map_or(Screen::Overview, |v| v.back);
        self.leave_deploy();
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

    fn leave_deploy(&mut self) {
        if let Some(view) = self.deploy.take()
            && let Some(task) = view.task
        {
            task.abort();
        }
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
            ConfirmAction::RemoveContext(_)
            | ConfirmAction::RotateToken
            | ConfirmAction::Prune
            | ConfirmAction::Update { .. } => "",
            ConfirmAction::Rollback(_) => "",
            ConfirmAction::CancelDeploy(_) => "",
            ConfirmAction::Stop(_) => "service stopped",
            ConfirmAction::DeleteBackup(_) => "backup deleted",
            ConfirmAction::PublicPort(..)
            | ConfirmAction::RotatePassword(_)
            | ConfirmAction::RestoreBackup(..)
            | ConfirmAction::DeleteSecret(_)
            | ConfirmAction::DeleteStorage(_) => "",
            ConfirmAction::DeleteInstanceBackup(_) => "instance backup deleted",
            ConfirmAction::DeleteNotifyChannel(_) => "channel deleted",
            ConfirmAction::DeleteNotifyRule(_) => "rule deleted",
            ConfirmAction::DeleteSource(_) | ConfirmAction::RemoveDomain => "",
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
            ConfirmAction::DeleteNotifyChannel(id) => {
                self.act(ok, |c| async move { c.delete_notify_channel(&id).await })
            }
            ConfirmAction::DeleteNotifyRule(id) => {
                self.act(ok, |c| async move { c.delete_notify_rule(&id).await })
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
            ConfirmAction::RemoveContext(name) => self.remove_context(&name),
            ConfirmAction::RotateToken => self.rotate_token(),
            ConfirmAction::Prune => {
                self.spawn(|c| async move {
                    Msg::ConfirmDone(
                        c.prune()
                            .await
                            .map(|r| {
                                format!(
                                    "removed {} images · freed {}",
                                    r.removed_images.len(),
                                    crate::util::bytes(r.reclaimed_bytes)
                                )
                            })
                            .map_err(|e| format!("{e:#}")),
                    )
                });
                self.settings.loaded_at = None;
            }
            ConfirmAction::Update { force, cli, server } => {
                let version = self
                    .settings
                    .release
                    .as_ref()
                    .and_then(|r| r.latest.clone());
                self.spawn(move |c| async move {
                    let cli = match (cli, &version) {
                        (true, Some(tag)) => {
                            Some(update_cli(tag).await.map_err(|e| format!("{e:#}")))
                        }
                        _ => None,
                    };
                    let server = match server {
                        true => Some(
                            c.update(&UpdateRequest { version, force })
                                .await
                                .map_err(|e| format!("{e:#}")),
                        ),
                        false => None,
                    };
                    Msg::Updated { cli, server }
                });
            }
            ConfirmAction::Rollback(id) => self.spawn(|c| async move {
                match c.rollback(&id).await {
                    Ok(d) => Msg::Deployed(Box::new(d)),
                    Err(e) => Msg::Error(format!("{e:#}")),
                }
            }),
            ConfirmAction::CancelDeploy(id) => self.spawn(move |c| async move {
                Msg::DeploymentCancelled(match c.cancel_deployment(&id).await {
                    Ok(d) if d.status == DeploymentStatus::Cancelled => Ok(Box::new(d)),
                    Ok(d) => Err(format!("deployment #{} is {}", d.number, d.status.as_str())),
                    Err(e) => Err(format!("{e:#}")),
                })
            }),
            ConfirmAction::Stop(id) => self.act(ok, |c| async move { c.stop(&id).await }),
            ConfirmAction::DeleteBackup(id) => {
                self.act(ok, |c| async move { c.delete_backup(&id).await })
            }
            ConfirmAction::PublicPort(id, port) => self.spawn(|c| async move {
                let body = UpdateDatabase { public_port: port };
                let result =
                    c.update_database(&id, &body)
                        .await
                        .map(|info| match info.public_port {
                            Some(p) => format!("{} is public on :{p}", info.host),
                            None => format!("{} is internal only again", info.host),
                        });
                Msg::ConfirmDone(result.map_err(|e| format!("{e:#}")))
            }),
            ConfirmAction::RotatePassword(id) => self.spawn(|c| async move {
                match c.rotate_database_password(&id).await {
                    Ok(info) => Msg::Rotated(Box::new(info)),
                    Err(e) => Msg::ConfirmDone(Err(format!("{e:#}"))),
                }
            }),
            ConfirmAction::DeleteSecret(id) => self.spawn(|c| async move {
                let result = c
                    .delete_secret(&id)
                    .await
                    .map(|_| "secret deleted".to_string());
                Msg::ConfirmDone(result.map_err(|e| format!("{e:#}")))
            }),
            ConfirmAction::DeleteInstanceBackup(id) => {
                self.act(ok, |c| async move { c.delete_instance_backup(&id).await })
            }
            ConfirmAction::DeleteStorage(id) => self.spawn(|c| async move {
                let result = c
                    .delete_storage(&id)
                    .await
                    .map(|_| "storage deleted".to_string());
                Msg::ConfirmDone(result.map_err(|e| format!("{e:#}")))
            }),
            ConfirmAction::DeleteSource(id) => self.spawn(|c| async move {
                let result = c
                    .delete_source(&id)
                    .await
                    .map(|_| "source deleted".to_string());
                Msg::ConfirmDone(result.map_err(|e| format!("{e:#}")))
            }),
            ConfirmAction::RemoveDomain => {
                self.domain.busy = Some("removing");
                self.spawn(|c| async move {
                    Msg::Domain(
                        c.remove_instance_domain()
                            .await
                            .map_err(|e| format!("{e:#}")),
                        Some("domain removed · the API answers on its IP again".into()),
                    )
                });
            }
            ConfirmAction::RestoreBackup(id, file) => self.spawn(|c| async move {
                let result = c
                    .restore_backup(&id)
                    .await
                    .map(|_| format!("restored {file}"));
                Msg::ConfirmDone(result.map_err(|e| format!("{e:#}")))
            }),
        }
    }

    /// The confirm dialog was accepted. Blocking actions keep it open with a spinner until
    /// their request returns; the rest close it at once.
    pub fn accept_confirm(&mut self) {
        let Some(Overlay::Confirm(c)) = &mut self.overlay else {
            return;
        };
        if c.busy.is_some() {
            return;
        }
        if let Some(label) = c.action.progress() {
            c.busy = Some((Instant::now(), label));
            let action = c.action.clone();
            return self.confirm(action);
        }
        if let Some(Overlay::Confirm(c)) = self.overlay.take() {
            self.confirm(c.action);
        }
    }

    pub fn copy(&mut self, text: &str, what: &str) {
        crate::mouse::copy_to_clipboard(text);
        self.toast(format!("copied {what}"));
    }

    /// "b" on the Backups tab: straight to the data directory, or ask where when there are
    /// storage connections.
    pub fn backup_now(&mut self, id: &str, name: &str) {
        if self.storage.list.is_empty() {
            return self.start_backup(id, name, None);
        }
        let scheduled = self.schedules.get(id).and_then(|s| s.storage_id.clone());
        self.open_picker(Picker {
            title: "Back up now".into(),
            subtitle: name.to_string(),
            kind: PickKind::Store,
            items: Vec::new(),
            row: 0,
            target: PickFor::BackupNow {
                service_id: id.to_string(),
                name: name.to_string(),
            },
            current: None,
            note: vec![Line::styled(
                "Files go to <prefix>/<service id>/<file> in the bucket. Restore and download work from any store.",
                theme::muted(),
            )],
            busy: false,
            git: None,
        });
        if let (Some(Overlay::Picker(p)), Some(s)) = (&mut self.overlay, scheduled) {
            p.select(&s);
        }
    }

    fn start_backup(&mut self, id: &str, name: &str, storage: Option<Id>) {
        let place = storage
            .as_deref()
            .and_then(|s| self.storage.name(s))
            .map(|s| format!(" to {s}"))
            .unwrap_or_default();
        self.progress(format!("backing up {name}{place}…"));
        let id = id.to_string();
        self.spawn(|c| async move {
            match c.create_backup(&id, storage.as_deref()).await {
                Ok(b) => Msg::Backup(Box::new(b)),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
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

    pub fn open_servers(&mut self) {
        if self.screen == Screen::Settings && self.settings.back == Some(Screen::Servers) {
            return self.close_settings();
        }
        if self.screen != Screen::Servers {
            self.servers_back = self.screen;
            self.screen = Screen::Servers;
            self.refresh();
        }
    }

    pub fn close_servers(&mut self) {
        self.screen = self.servers_back;
    }

    // Settings

    pub fn open_settings(&mut self) {
        if self.screen == Screen::Servers && self.servers_back == Screen::Settings {
            return self.close_servers();
        }
        if self.screen != Screen::Settings {
            self.settings.back = Some(self.screen);
        }
        self.screen = Screen::Settings;
        self.settings.contexts = Config::load().unwrap_or_default();
        let names = self.settings.context_names();
        self.settings.row = names.iter().position(|n| *n == self.context).unwrap_or(0);
        self.load_settings();
        self.load_release();
        self.refresh();
    }

    /// The latest release when the daemon runs an older one.
    pub fn daemon_update(&self) -> Option<String> {
        self.settings
            .release
            .as_ref()
            .filter(|r| r.update_available)
            .and_then(|r| r.latest.clone())
    }

    /// The latest release when this CLI binary is older and was not replaced yet.
    pub fn cli_update(&self) -> Option<String> {
        let latest = self.settings.release.as_ref()?.latest.clone()?;
        (self.cli_updated.is_none() && release::is_newer(&latest, env!("CARGO_PKG_VERSION")))
            .then_some(latest)
    }

    fn load_release(&self) {
        self.spawn(|c| async move {
            match c.version_check().await {
                Ok(r) => Msg::Release(r),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    pub fn close_settings(&mut self) {
        self.screen = match self.settings.back.take() {
            Some(Screen::Service) if self.service.is_some() => Screen::Service,
            Some(Screen::Servers) => Screen::Servers,
            Some(Screen::Deploy) if self.deploy.is_some() => Screen::Deploy,
            _ => Screen::Overview,
        };
        self.refresh();
    }

    fn load_instance_backups(&self) {
        self.spawn(|c| async move {
            match c.instance_backups().await {
                Ok(b) => Msg::InstanceBackups(b),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    fn load_settings(&mut self) {
        self.load_instance_backups();
        self.spawn(|c| async move {
            match c.instance_backup_schedule().await {
                Ok(s) => Msg::InstanceSchedule(s),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
        self.settings.loaded_at = Some(Instant::now());
        self.spawn(|c| async move {
            match c.settings().await {
                Ok(s) => Msg::Settings(Box::new(s)),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    /// Applies a settings change and shows the result.
    pub fn change_settings(&mut self, body: UpdateSettings, ok: String) {
        self.spawn(|c| async move {
            match c.update_settings(&body).await {
                Ok(s) => Msg::FormDone {
                    toast: ok,
                    then: Then::Settings(Box::new(s)),
                },
                Err(e) => Msg::FormError(format!("{e:#}")),
            }
        });
    }

    pub fn restart_proxy(&mut self) {
        self.progress("reinstalling the proxy…");
        self.settings.loaded_at = None;
        self.act("proxy reinstalled".into(), |c| async move {
            c.restart_proxy().await
        });
    }

    /// Rotates the token and saves it to the context file before anything else is sent.
    fn rotate_token(&mut self) {
        self.rotating = true;
        let (context, url, old) = (
            self.context.clone(),
            self.client.url().to_string(),
            self.client.token().to_string(),
        );
        self.spawn(move |c| async move {
            let token = match c.rotate_token().await {
                Ok(r) => r.token,
                Err(e) => return Msg::TokenRotated(Err(format!("{e:#}"))),
            };
            let saved = context != "env"
                && Config::load().is_ok_and(|mut config| {
                    config.replace_token(&url, &old, &token) > 0 && config.save().is_ok()
                });
            Msg::TokenRotated(Ok((token, saved)))
        });
    }

    /// Connects the TUI to another saved daemon and starts over with its data.
    pub fn switch_context(&mut self, name: &str) {
        if name == self.context {
            return self.toast(format!("already connected to {name}"));
        }
        let Some(ctx) = self.settings.contexts.contexts.get(name).cloned() else {
            return;
        };
        let client = match Client::from_context(&ctx) {
            Ok(c) => c,
            Err(e) => return self.error(format!("{e:#}")),
        };
        self.settings.contexts.current = Some(name.to_string());
        if let Err(e) = self.settings.contexts.save() {
            self.error(format!("{e:#}"));
        }
        self.stop_streams();
        if let Some(h) = self.deploy.take().and_then(|v| v.task) {
            h.abort();
        }
        self.epoch += 1;
        self.client = client;
        self.context = name.to_string();
        self.info = None;
        self.connection_error = None;
        self.tree = None;
        self.activity.clear();
        self.servers.clear();
        self.details.clear();
        self.stats.clear();
        self.service = None;
        self.overview = OverviewState::default();
        self.server_row = 0;
        self.servers_back = Screen::Overview;
        self.updating = None;
        self.rotating = false;
        self.settings.settings = None;
        self.settings.release = None;
        self.settings.backups.clear();
        self.settings.backup_row = 0;
        self.settings.schedule = None;
        self.secrets = SecretsView::default();
        self.storage = StorageView::default();
        self.notify = NotifyView::default();
        self.settings.back = Some(Screen::Overview);
        self.open_settings();
        self.toast(format!("connected to {name}"));
    }

    /// Forgets a saved context, the same way `logout` does on the command line.
    fn remove_context(&mut self, name: &str) {
        if name == self.context {
            return self.error("switch to another context before removing this one");
        }
        let mut config = Config::load().unwrap_or_default();
        match config.forget(name).and_then(|_| config.save()) {
            Ok(()) => {
                self.toast(format!(
                    "forgot {name} · its token still works until rotated"
                ));
                self.settings.contexts = config;
                let n = self.settings.contexts.contexts.len();
                self.settings.row = self.settings.row.min(n.saturating_sub(1));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    pub fn form_public_ip(&mut self) {
        let Some(s) = &self.settings.settings else {
            return;
        };
        let hint = match (&s.public_ip_override, &s.public_ip) {
            (Some(_), _) => "auto detects it again".to_string(),
            (None, Some(ip)) => format!("detected {ip}"),
            (None, None) => "not detected".to_string(),
        };
        let form = Form::new(
            FormKind::PublicIp,
            "Public IP",
            vec![
                Field::choice("mode", "address", vec!["auto".into(), "fixed".into()])
                    .selected(usize::from(s.public_ip_override.is_some())),
                Field::text("ip", "ip")
                    .value(s.public_ip_override.clone().unwrap_or_default())
                    .placeholder("203.0.113.10")
                    .hint(hint),
            ],
        )
        .subtitle("shown in hints and the top bar")
        .submit("save", None)
        .focus_on(if s.public_ip_override.is_some() {
            "ip"
        } else {
            "mode"
        });
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_purge_server(&mut self, server: &Server) {
        let services = self
            .all_services()
            .filter(|s| s.server_id == server.id)
            .count();
        let form = Form::new(
            FormKind::PurgeServer {
                server_id: server.id.clone(),
            },
            "Remove server",
            vec![Field::toggle("volumes", "volumes", false).hint("also delete app data")],
        )
        .subtitle(server.name.clone())
        .warning(vec![
            format!(
                "Deletes its {services} service{} and everything the daemon created on {}.",
                if services == 1 { "" } else { "s" },
                server.name
            ),
            "Containers, the proxy and the networks go, over SSH.".into(),
        ])
        .submit("remove & clean up", None);
        self.overlay = Some(Overlay::Form(form));
    }

    // Secrets

    pub fn open_secrets(&mut self) {
        if self.screen != Screen::Secrets {
            self.secrets.back = Some(self.screen);
        }
        self.screen = Screen::Secrets;
        self.secrets.revealed = None;
        self.refresh();
    }

    pub fn close_secrets(&mut self) {
        self.secrets.revealed = None;
        self.screen = match self.secrets.back.take() {
            Some(Screen::Service) if self.service.is_some() => Screen::Service,
            Some(back @ (Screen::Servers | Screen::Settings)) => back,
            _ => Screen::Overview,
        };
        self.refresh();
    }

    pub fn select_secret(&mut self, row: usize) {
        let n = self.secrets.list.len();
        if n == 0 {
            return;
        }
        let row = row.min(n - 1);
        if row != self.secrets.row {
            self.secrets.row = row;
            self.secrets.revealed = None;
        }
    }

    /// Asks the daemon for the selected value, or hides it again. The daemon records each reveal.
    pub fn toggle_reveal(&mut self) {
        if self.secrets.value().is_some() {
            self.secrets.revealed = None;
            return;
        }
        let Some(id) = self.secrets.selected().map(|s| s.id.clone()) else {
            return;
        };
        self.spawn(|c| async move {
            match c.reveal_secret(&id).await {
                Ok(r) => Msg::Revealed(Box::new(r)),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    pub fn copy_public_part(&mut self) {
        let Some(s) = self.secrets.selected() else {
            return;
        };
        let what = match s.kind {
            SecretKind::SshKey => "the public key",
            SecretKind::S3Credentials => "the access key id",
            SecretKind::GithubApp => "the app id",
            SecretKind::Generic => return self.error("a generic secret has no public part"),
        };
        match s.public_part.clone() {
            Some(p) => self.copy(&p, what),
            None => self.error("no public part"),
        }
    }

    /// A GitHub App secret belongs to its source: the daemon refuses to reveal or change it.
    pub fn refuse_managed_secret(&mut self) -> bool {
        let Some(s) = self.secrets.selected() else {
            return false;
        };
        if s.kind != SecretKind::GithubApp {
            return false;
        }
        let msg = format!("{} · change it on the Sources screen", managed_by(s));
        self.error(msg);
        true
    }

    pub fn copy_secret_value(&mut self) {
        match self.secrets.value().map(secret_text) {
            Some(v) => self.copy(&v, "the value"),
            None => self.error("reveal it first (v)"),
        }
    }

    fn show_key(&mut self, s: &Secret) {
        self.overlay = Some(Overlay::Reveal(Reveal {
            title: "SSH key generated".into(),
            subtitle: s.name.clone(),
            message: "The daemon generated an ed25519 key. Only the public key leaves it; the private key is sealed with the master key.".into(),
            value: s.public_part.clone().unwrap_or_default(),
            note: format!(
                "Add it to ~/.ssh/authorized_keys on the server, then pick {} when you add the server or change its key.",
                s.name
            ),
            what: "the public key",
        }));
    }

    pub fn form_add_secret(&mut self) {
        let form = Form::new(
            FormKind::AddSecret,
            "Add secret",
            vec![
                Field::choice(
                    "kind",
                    "kind",
                    ADDABLE_SECRETS
                        .iter()
                        .map(|k| k.label().to_string())
                        .collect(),
                ),
                Field::text("name", "name").placeholder("github-deploy"),
                Field::text("private_key", "private key")
                    .masked()
                    .hint("paste, or a file path"),
                Field::text("access_key_id", "access key id"),
                Field::text("secret_access_key", "secret key").masked(),
                Field::text("value", "value").masked(),
            ],
        )
        .subtitle("create or import")
        .note(vec![
            "Paste an OpenSSH private key without a passphrase, or type a path like".into(),
            "~/.ssh/id_ed25519 to read it from this machine. Values stay masked.".into(),
        ])
        .submit("add", None)
        .focus_on("name");
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_generate_key(&mut self) {
        let form = Form::new(
            FormKind::GenerateKey,
            "Generate SSH key",
            vec![Field::text("name", "name").placeholder("edge-key")],
        )
        .subtitle("ed25519, made by the daemon")
        .submit("generate", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_rename_secret(&mut self) {
        let Some(s) = self.secrets.selected() else {
            return;
        };
        let form = Form::new(
            FormKind::RenameSecret {
                secret_id: s.id.clone(),
            },
            "Rename secret",
            vec![Field::text("name", "name").value(s.name.clone())],
        )
        .subtitle(s.name.clone())
        .submit("rename", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_replace_secret(&mut self) {
        let Some(s) = self.secrets.selected() else {
            return;
        };
        let fields = match s.kind {
            SecretKind::SshKey => vec![
                Field::text("private_key", "private key")
                    .masked()
                    .hint("paste, or a file path"),
            ],
            SecretKind::S3Credentials => vec![
                Field::text("access_key_id", "access key id")
                    .value(s.public_part.clone().unwrap_or_default()),
                Field::text("secret_access_key", "secret key").masked(),
            ],
            SecretKind::Generic => vec![Field::text("value", "value").masked()],
            SecretKind::GithubApp => return,
        };
        let users = if s.used_by.is_empty() {
            "Nothing uses it yet.".to_string()
        } else {
            let names: Vec<String> = s.used_by.iter().map(ToString::to_string).collect();
            format!("{} use the new value from now on.", names.join(", "))
        };
        let form = Form::new(
            FormKind::ReplaceSecret {
                secret_id: s.id.clone(),
                kind: s.kind,
            },
            "Replace value",
            fields,
        )
        .subtitle(s.name.clone())
        .note(vec![users])
        .submit("replace", None);
        self.overlay = Some(Overlay::Form(form));
    }

    // Storage

    pub fn open_storage(&mut self) {
        if self.screen != Screen::Storage {
            self.storage.back = Some(self.screen);
        }
        self.screen = Screen::Storage;
        self.refresh();
    }

    pub fn close_storage(&mut self) {
        self.screen = match self.storage.back.take() {
            Some(Screen::Service) if self.service.is_some() => Screen::Service,
            Some(back @ (Screen::Servers | Screen::Settings | Screen::Secrets)) => back,
            _ => Screen::Overview,
        };
        self.refresh();
    }

    pub fn select_storage(&mut self, row: usize) {
        self.storage.row = row.min(self.storage.list.len().saturating_sub(1));
    }

    /// Puts, reads and deletes a probe object; the result lands in the list and a toast.
    pub fn test_storage(&mut self) {
        let Some(s) = self.storage.selected() else {
            return;
        };
        let (id, name) = (s.id.clone(), s.name.clone());
        if !self.storage.testing.insert(id.clone()) {
            return;
        }
        self.progress(format!("testing {name}…"));
        self.spawn(|c| async move {
            let result = c.test_storage(&id).await.map_err(|e| format!("{e:#}"));
            Msg::StorageTested(id, result)
        });
    }

    /// A pick field for where backups go: local or a storage connection.
    fn store_field(&self, id: Option<&str>) -> Field {
        let storage = id.and_then(|id| self.storage.list.iter().find(|s| s.id == id));
        let mut field = Field::pick(
            "store",
            "store",
            id.unwrap_or_default(),
            storage.map_or("local".to_string(), |s| s.name.clone()),
        );
        field.placeholder = storage.map_or("the data directory".into(), crate::picker::bucket_path);
        field
    }

    /// Add a storage connection, or edit the selected one.
    pub fn form_storage(&mut self, edit: bool) {
        let existing = if edit {
            match self.storage.selected() {
                Some(s) => Some(s.clone()),
                None => return,
            }
        } else {
            None
        };
        let s = existing.as_ref();
        let get = |f: fn(&StorageConnection) -> String| s.map(f).unwrap_or_default();
        let creds_id = get(|s| s.credentials_secret_id.clone());
        let creds = self
            .secrets
            .list
            .iter()
            .find(|c| c.id == creds_id)
            .or_else(|| {
                self.secrets
                    .list
                    .iter()
                    .find(|c| !edit && c.kind == SecretKind::S3Credentials)
            });
        let mut credentials = Field::pick(
            "credentials",
            "credentials",
            creds.map(|c| c.id.clone()).unwrap_or_default(),
            creds.map_or("none".to_string(), |c| c.name.clone()),
        );
        credentials.placeholder = creds
            .and_then(|c| c.public_part.clone())
            .unwrap_or_else(|| "pick s3 credentials".into());
        let form = Form::new(
            match s {
                Some(s) => FormKind::EditStorage {
                    storage_id: s.id.clone(),
                },
                None => FormKind::AddStorage,
            },
            if edit { "Edit storage" } else { "Add storage" },
            vec![
                Field::text("name", "name")
                    .value(get(|s| s.name.clone()))
                    .placeholder("r2-backups"),
                Field::text("endpoint", "endpoint")
                    .value(get(|s| s.endpoint.clone()))
                    .placeholder("https://<account>.r2.cloudflarestorage.com")
                    .hint("S3 API base URL"),
                Field::text("region", "region")
                    .value(get(|s| s.region.clone()))
                    .placeholder("us-east-1")
                    .hint("auto for R2"),
                Field::text("bucket", "bucket").value(get(|s| s.bucket.clone())),
                Field::text("prefix", "path prefix")
                    .value(get(|s| s.path_prefix.clone()))
                    .hint("optional"),
                Field::toggle("path_style", "path-style", s.is_some_and(|s| s.path_style))
                    .hint("MinIO needs it on"),
                credentials,
            ],
        )
        .subtitle(s.map_or("S3-compatible".to_string(), |s| s.name.clone()))
        .note(vec![
            "Credentials are s3_credentials secrets; add them on the Secrets screen first.".into(),
            "The connection is tested after it is saved.".into(),
        ])
        .submit(if edit { "save & test" } else { "add & test" }, None);
        self.overlay = Some(Overlay::Form(form));
    }

    // Notifications

    pub fn open_notifications(&mut self) {
        if self.screen != Screen::Notifications {
            self.notify.back = Some(self.screen);
        }
        self.screen = Screen::Notifications;
        self.refresh();
    }

    pub fn close_notifications(&mut self) {
        self.screen = match self.notify.back.take() {
            Some(Screen::Service) if self.service.is_some() => Screen::Service,
            Some(
                back @ (Screen::Servers | Screen::Settings | Screen::Secrets | Screen::Storage),
            ) => back,
            _ => Screen::Overview,
        };
        self.refresh();
    }

    pub fn load_notify(&self) {
        self.spawn(|c| async move {
            match c.notify_channels().await {
                Ok(list) => Msg::NotifyChannels(list),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
        self.spawn(|c| async move {
            match c.notify_rules().await {
                Ok(list) => Msg::NotifyRules(list),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
        self.load_deliveries();
    }

    pub fn load_deliveries(&self) {
        let channel = self.notify.channel_filter.clone();
        self.spawn(move |c| async move {
            match c
                .notify_deliveries(notify::LOG_LIMIT, channel.as_deref(), None)
                .await
            {
                Ok(list) => Msg::NotifyDeliveries(list),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    /// Sends a test message through the selected channel; the result lands in the list.
    pub fn test_channel(&mut self) {
        let Some(c) = self.notify.selected_channel() else {
            return;
        };
        let (id, name) = (c.id.clone(), c.name.clone());
        if !self.notify.testing.insert(id.clone()) {
            return;
        }
        self.progress(format!("sending a test message to {name}…"));
        self.spawn(|c| async move {
            let result = c
                .test_notify_channel(&id)
                .await
                .map_err(|e| format!("{e:#}"));
            Msg::NotifyTested(id, result)
        });
    }

    /// Enable or disable the selected channel or rule.
    pub fn toggle_notify(&mut self) {
        match self.notify.section {
            notify::Section::Channels => {
                let Some(c) = self.notify.selected_channel() else {
                    return;
                };
                let (id, on) = (c.id.clone(), !c.enabled);
                let ok = format!("{} {}", c.name, if on { "enabled" } else { "disabled" });
                let body = UpdateNotifyChannel {
                    enabled: Some(on),
                    ..Default::default()
                };
                self.act(
                    ok,
                    |c| async move { c.update_notify_channel(&id, &body).await },
                );
            }
            notify::Section::Rules => {
                let Some(r) = self.notify.selected_rule() else {
                    return;
                };
                let (id, on) = (r.id.clone(), !r.enabled);
                let ok = format!("rule {}", if on { "enabled" } else { "disabled" });
                let body = UpdateNotifyRule {
                    enabled: Some(on),
                    ..Default::default()
                };
                self.act(
                    ok,
                    |c| async move { c.update_notify_rule(&id, &body).await },
                );
            }
            notify::Section::Deliveries => {}
        }
    }

    /// First step of the channel wizard.
    pub fn form_notify_kind(&mut self) {
        self.overlay = Some(Overlay::Form(notify::kind_form()));
    }

    pub fn form_edit_channel(&mut self) {
        let Some(c) = self.notify.selected_channel().cloned() else {
            return;
        };
        self.overlay = Some(Overlay::Form(notify::channel_form(c.kind, Some(&c), false)));
    }

    fn notify_kind_chosen(&mut self) {
        let Some(Overlay::Form(form)) = &self.overlay else {
            return;
        };
        let kind = notify::chosen_kind(form);
        self.overlay = Some(Overlay::Form(notify::channel_form(kind, None, false)));
    }

    /// Saves the settings step; a new channel is created disabled until the last step saves it.
    fn submit_notify_channel(&mut self) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        let FormKind::NotifyChannel {
            channel_id,
            created,
            ..
        } = form.kind.clone()
        else {
            return;
        };
        let (name, config) = match notify::channel_config(form) {
            Ok(v) => v,
            Err(e) => return form.fail(e),
        };
        form.busy = true;
        form.error = None;
        self.spawn(move |c| async move {
            let saved = match &channel_id {
                Some(id) => {
                    let body = UpdateNotifyChannel {
                        name: Some(name),
                        enabled: None,
                        config: Some(config),
                    };
                    c.update_notify_channel(id, &body).await
                }
                None => {
                    let body = CreateNotifyChannel {
                        name,
                        config,
                        enabled: false,
                    };
                    c.create_notify_channel(&body).await
                }
            };
            match saved {
                Ok(ch) => Msg::NotifyStep(Box::new(ch), channel_id.is_none() || created),
                Err(e) => Msg::FormError(format!("{e:#}")),
            }
        });
    }

    /// Esc on a form; a channel the wizard created is deleted again.
    pub fn cancel_form(&mut self) {
        if let Some(Overlay::Form(Form {
            kind:
                FormKind::NotifyChannel {
                    channel_id: Some(id),
                    created: true,
                    ..
                },
            ..
        })) = self.overlay.take()
        {
            self.spawn(|c| async move {
                match c.delete_notify_channel(&id).await {
                    Ok(()) => Msg::Toast("channel discarded".into()),
                    Err(e) => Msg::Error(format!("{e:#}")),
                }
            });
        }
    }

    pub fn retest_step(&mut self) {
        let Some(Overlay::ChannelTest(step)) = &mut self.overlay else {
            return;
        };
        if step.saving || step.result.is_none() {
            return;
        }
        step.result = None;
        let id = step.channel.id.clone();
        self.spawn(|c| async move {
            let result = c
                .test_notify_channel(&id)
                .await
                .map_err(|e| format!("{e:#}"));
            Msg::NotifyStepTested(id, result)
        });
    }

    pub fn save_step(&mut self) {
        let Some(Overlay::ChannelTest(step)) = &mut self.overlay else {
            return;
        };
        if step.saving {
            return;
        }
        let (id, name) = (step.channel.id.clone(), step.channel.name.clone());
        if !step.created {
            self.overlay = None;
            self.toast(format!("{name} saved"));
            return self.load_notify();
        }
        step.saving = true;
        self.spawn(move |c| async move {
            let body = UpdateNotifyChannel {
                enabled: Some(true),
                ..Default::default()
            };
            Msg::NotifyDialogDone(
                c.update_notify_channel(&id, &body)
                    .await
                    .map(|_| format!("{name} added · add a rule to choose its events"))
                    .map_err(|e| format!("{e:#}")),
            )
        });
    }

    pub fn back_from_step(&mut self) {
        if let Some(Overlay::ChannelTest(step)) = self.overlay.take() {
            self.overlay = Some(Overlay::Form(*step.form));
        }
    }

    pub fn cancel_step(&mut self) {
        let Some(Overlay::ChannelTest(step)) = self.overlay.take() else {
            return;
        };
        self.overlay = Some(Overlay::Form(*step.form));
        self.cancel_form();
    }

    pub fn rule_dialog(&mut self, edit: bool) {
        let dialog = if edit {
            let Some(r) = self.notify.selected_rule() else {
                return;
            };
            RuleDialog::edit(r, self.notify.channel(&r.channel_id))
        } else {
            let Some(c) = self.notify.selected_channel() else {
                return self.error("add a channel first, on the Channels tab");
            };
            RuleDialog::new(c)
        };
        self.overlay = Some(Overlay::Rule(dialog));
    }

    pub fn pick_for_rule(&mut self, what: notify::RuleFocus) {
        let Some(Overlay::Rule(dialog)) = self.overlay.take() else {
            return;
        };
        let (kind, title, items, current, note) = if what == notify::RuleFocus::Channel {
            (
                PickKind::Channel,
                "Channel",
                notify::channel_items(&self.notify.channels),
                dialog.channel_id.clone(),
                "Channels are added on the Channels tab.",
            )
        } else {
            (
                PickKind::Scope,
                "Scope",
                notify::scope_items(self.tree.as_ref()),
                notify::scope_id(&dialog.scope),
                "Server and daemon events only reach instance rules.",
            )
        };
        let mut picker = Picker {
            title: title.into(),
            subtitle: "rule".into(),
            kind,
            items,
            row: 0,
            target: PickFor::Rule(Box::new(dialog)),
            current: Some(current),
            note: vec![Line::styled(note, theme::muted())],
            busy: false,
            git: None,
        };
        picker.mark_current();
        picker.row = picker
            .items
            .iter()
            .position(|i| i.id == picker.current)
            .unwrap_or(0);
        self.overlay = Some(Overlay::Picker(picker));
    }

    pub fn submit_rule(&mut self) {
        let Some(Overlay::Rule(d)) = &mut self.overlay else {
            return;
        };
        if d.events.is_empty() {
            d.error = Some("pick at least one event".into());
            return;
        }
        d.busy = true;
        d.error = None;
        let rule_id = d.rule_id.clone();
        let channel_id = d.channel_id.clone();
        let scope = d.scope.clone();
        let label = format!("{} → {}", d.channel, d.scope_label);
        let preset = d.preset.map(String::from);
        let events: Vec<notify_types::NotifyEvent> = d.events.iter().copied().collect();
        let (dedupe, rate) = (d.dedupe, d.rate);
        self.spawn(move |c| async move {
            let result = match rule_id {
                Some(id) => {
                    let body = UpdateNotifyRule {
                        channel_id: Some(channel_id),
                        scope: Some(scope),
                        preset: Some(preset.unwrap_or_default()),
                        events: Some(events),
                        enabled: None,
                        dedupe_secs: Some(dedupe),
                        max_per_hour: Some(rate),
                    };
                    c.update_notify_rule(&id, &body)
                        .await
                        .map(|_| format!("rule saved · {label}"))
                }
                None => {
                    let body = CreateNotifyRule {
                        channel_id,
                        scope,
                        preset,
                        events,
                        enabled: true,
                        dedupe_secs: Some(dedupe),
                        max_per_hour: Some(rate),
                    };
                    c.create_notify_rule(&body)
                        .await
                        .map(|r| format!("rule added · {label} · {} events", r.events.len()))
                }
            };
            Msg::NotifyDialogDone(result.map_err(|e| format!("{e:#}")))
        });
    }

    // Sources

    pub fn load_sources(&self) {
        self.spawn(|c| async move {
            match c.sources().await {
                Ok(s) => Msg::Sources(s),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    fn upsert_source(&mut self, s: GitSource) {
        if let Some(Overlay::GithubSetup(setup)) = &mut self.overlay
            && setup.source.id == s.id
        {
            if s.status == GitSourceStatus::Ready && setup.source.status != GitSourceStatus::Ready {
                self.toast = Some(Toast {
                    text: format!("{} connected", s.name),
                    error: false,
                    busy: false,
                    at: Instant::now(),
                });
            }
            setup.source = s.clone();
        }
        match self.sources.list.iter_mut().find(|x| x.id == s.id) {
            Some(slot) => *slot = s,
            None => self.sources.list.push(s),
        }
    }

    /// Follow a GitHub App setup while its dialog is open.
    fn poll_setup(&self) {
        let Some(Overlay::GithubSetup(setup)) = &self.overlay else {
            return;
        };
        if setup.source.status == GitSourceStatus::Ready {
            return;
        }
        let id = setup.source.id.clone();
        self.spawn(|c| async move {
            match c.source(&id).await {
                Ok(s) => Msg::Source(s),
                Err(e) => Msg::Error(format!("{e:#}")),
            }
        });
    }

    pub fn open_sources(&mut self) {
        if self.screen != Screen::Sources {
            self.sources.back = Some(self.screen);
        }
        self.screen = Screen::Sources;
        self.load_settings();
        self.load_domain();
        self.refresh();
    }

    pub fn close_sources(&mut self) {
        self.screen = match self.sources.back.take() {
            Some(Screen::Service) if self.service.is_some() => Screen::Service,
            Some(
                back @ (Screen::Servers | Screen::Settings | Screen::Secrets | Screen::Storage),
            ) => back,
            _ => Screen::Overview,
        };
        self.refresh();
    }

    pub fn select_source(&mut self, row: usize) {
        self.sources.row = row.min(self.sources.list.len().saturating_sub(1));
    }

    /// Mints a token with the source's credentials and updates its status.
    pub fn check_source(&mut self) {
        let Some(s) = self.sources.selected() else {
            return;
        };
        let (id, name) = (s.id.clone(), s.name.clone());
        if !self.sources.checking.insert(id.clone()) {
            return;
        }
        self.progress(format!("checking {name}…"));
        self.spawn(|c| async move {
            let result = c.check_source(&id).await.map_err(|e| format!("{e:#}"));
            Msg::SourceChecked(id, result)
        });
    }

    /// ⏎ on a GitHub App that is not ready: show where to finish it.
    pub fn continue_setup(&mut self) {
        let Some(s) = self.sources.selected() else {
            return;
        };
        if s.kind != GitSourceKind::GithubApp || s.status == GitSourceStatus::Ready {
            return;
        }
        self.overlay = Some(Overlay::GithubSetup(GithubSetup {
            source: s.clone(),
            setup_url: None,
        }));
    }

    pub fn open_url(&mut self, url: &str) {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let opened = std::process::Command::new(program)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
        if opened {
            self.toast("opened in your browser");
        } else {
            self.error("no browser to open here · c copies the URL");
        }
    }

    /// The GitHub URL a form starts with, and what a typed one means for the request.
    fn github_url(form: &Form) -> Option<String> {
        let url = form.get("html_url");
        let url = url.trim_end_matches('/');
        (!url.is_empty() && url != DEFAULT_GITHUB).then(|| url.to_string())
    }

    /// GitHub calls back to the instance domain, so apps wait until it is verified.
    fn domain_needed(&mut self, title: &'static str) -> bool {
        let needed = self
            .settings
            .settings
            .as_ref()
            .is_some_and(|s| !s.domain_verified);
        if needed {
            self.overlay = Some(Overlay::DomainNeeded(title));
        }
        needed
    }

    /// "n" on Sources: the manifest flow, once the instance domain is verified.
    pub fn form_github_app(&mut self) {
        if self.domain_needed("New GitHub App") {
            return;
        }
        let form = Form::new(
            FormKind::CreateGithubApp,
            "New GitHub App",
            vec![
                Field::text("name", "name")
                    .placeholder("acme-github")
                    .hint("shown in New service"),
                Field::choice(
                    "owner",
                    "owner",
                    vec!["personal account".into(), "organization".into()],
                ),
                Field::text("organization", "organization").hint("its login on GitHub"),
                Field::text("html_url", "github url")
                    .value(DEFAULT_GITHUB)
                    .hint("change for Enterprise"),
            ],
        )
        .subtitle("created for you on GitHub")
        .note(vec![
            "GitHub asks you to confirm the app in your browser, then where to install it.".into(),
        ])
        .submit("continue on GitHub", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_connect_app(&mut self) {
        if self.domain_needed("Connect GitHub App") {
            return;
        }
        let form = Form::new(
            FormKind::ConnectGithubApp,
            "Connect GitHub App",
            vec![
                Field::text("name", "name").placeholder("acme-github"),
                Field::text("html_url", "github url")
                    .value(DEFAULT_GITHUB)
                    .hint("or your Enterprise URL"),
                Field::text("app_id", "app id").hint("from the app's page"),
                Field::text("private_key", "private key")
                    .masked()
                    .hint("a .pem path, or paste"),
                Field::text("installation", "installation")
                    .placeholder("found on its own if empty")
                    .hint("optional"),
                Field::text("webhook_secret", "webhook secret")
                    .masked()
                    .placeholder("none")
                    .hint("optional"),
            ],
        )
        .subtitle("one you created on GitHub")
        .note(vec![
            "For deploy on push, set the app's webhook URL to the one Sources shows after this."
                .into(),
        ])
        .submit("connect & check", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_github_token(&mut self) {
        let form = Form::new(
            FormKind::AddGithubToken,
            "Add token",
            vec![
                Field::text("name", "name").placeholder("deploy-token"),
                Field::text("token", "token").masked().hint("paste it"),
                Field::text("html_url", "github url")
                    .value(DEFAULT_GITHUB)
                    .hint("or Enterprise"),
            ],
        )
        .subtitle("GitHub")
        .note(vec![
            "Needs read access to contents and metadata. Tokens can't deploy on push; use a".into(),
            "GitHub App for that.".into(),
        ])
        .submit("add & check", None);
        self.overlay = Some(Overlay::Form(form));
    }

    // Instance domain

    pub fn load_domain(&self) {
        self.spawn(|c| async move {
            Msg::Domain(
                c.instance_domain().await.map_err(|e| format!("{e:#}")),
                None,
            )
        });
    }

    pub fn open_domain(&mut self) {
        if self.screen != Screen::Domain {
            self.domain.back = Some(self.screen);
        }
        self.screen = Screen::Domain;
        self.domain.input = None;
        self.domain.busy = None;
        self.load_domain();
        self.refresh();
    }

    pub fn close_domain(&mut self) {
        self.domain.input = None;
        self.screen = match self.domain.back.take() {
            Some(Screen::Sources) => Screen::Sources,
            Some(Screen::Service) if self.service.is_some() => Screen::Service,
            Some(Screen::Overview) => Screen::Overview,
            _ => Screen::Settings,
        };
        self.refresh();
    }

    /// "e" on the guide: type another domain.
    pub fn edit_domain(&mut self) {
        let current = self.domain.data.as_ref().and_then(|d| d.domain.clone());
        self.domain.input = Some(current.unwrap_or_default());
    }

    /// Esc while typing: back to the guide, or out when there is no domain yet.
    pub fn cancel_domain_input(&mut self) {
        if self
            .domain
            .data
            .as_ref()
            .is_some_and(|d| d.domain.is_some())
        {
            self.domain.input = None;
        } else {
            self.close_domain();
        }
    }

    /// ⏎ while typing: route the domain to the API and run the checks.
    pub fn save_domain(&mut self) {
        if self.domain.busy.is_some() {
            return;
        }
        let Some(input) = &self.domain.input else {
            return;
        };
        let domain = input
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/')
            .to_lowercase();
        if !domain.contains('.') || domain.contains(char::is_whitespace) {
            return self.error("enter a hostname like deploy.example.com");
        }
        self.domain.busy = Some("checking");
        self.spawn(|c| async move {
            let result = c.set_instance_domain(&domain).await;
            let toast = match &result {
                Ok(d) => domain_toast(d),
                Err(_) => None,
            };
            Msg::Domain(result.map_err(|e| format!("{e:#}")), toast)
        });
    }

    /// "r" on the guide: run the five checks again.
    pub fn check_domain(&mut self) {
        if self.domain.busy.is_some() {
            return;
        }
        if self.domain.data.as_ref().is_none_or(|d| d.domain.is_none()) {
            return self.error("enter a domain first");
        }
        self.domain.busy = Some("checking");
        self.spawn(|c| async move {
            let result = c.check_instance_domain().await;
            let toast = match &result {
                Ok(d) => domain_toast(d),
                Err(_) => None,
            };
            Msg::Domain(result.map_err(|e| format!("{e:#}")), toast)
        });
    }

    // Instance backups

    pub fn select_instance_backup(&mut self, row: usize) {
        self.settings.focus = SettingsFocus::Backups;
        self.settings.backup_row = row.min(self.settings.backups.len().saturating_sub(1));
    }

    /// "b" in Settings: where the archive goes and the passphrase that encrypts it.
    pub fn form_instance_backup(&mut self) {
        let scheduled = self
            .settings
            .schedule
            .as_ref()
            .and_then(|s| s.storage_id.clone());
        let form = Form::new(
            FormKind::InstanceBackup,
            "Back up instance",
            vec![
                self.store_field(scheduled.as_deref()),
                Field::text("passphrase", "passphrase")
                    .masked()
                    .hint(format!("{MIN_PASSPHRASE_LEN}+ characters")),
                Field::text("repeat", "repeat").masked(),
            ],
        )
        .subtitle(self.context.clone())
        .note(vec![
            "Archives the store, config.toml, master.key and the SSH directory, encrypted with"
                .into(),
            "this passphrase. It is never stored: without it the archive cannot be restored."
                .into(),
            restore_hint(),
        ])
        .submit("back up", None)
        .focus_on("passphrase");
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_instance_schedule(&mut self) {
        let s = self.settings.schedule.clone().unwrap_or_default();
        let form = Form::new(
            FormKind::InstanceSchedule {
                has_passphrase: s.has_passphrase,
            },
            "Instance backup schedule",
            vec![
                Field::toggle("enabled", "daily", s.enabled).hint("off keeps old archives"),
                Field::text("hour", "hour (UTC)").value(s.hour_utc.to_string()),
                Field::text("keep", "keep")
                    .value(s.keep.to_string())
                    .hint("per store"),
                self.store_field(s.storage_id.as_deref()),
                Field::text("passphrase", "passphrase")
                    .masked()
                    .placeholder(if s.has_passphrase {
                        "stored · empty keeps it"
                    } else {
                        "needed for scheduled runs"
                    })
                    .hint(format!("{MIN_PASSPHRASE_LEN}+ characters")),
                Field::text("repeat", "repeat").masked(),
            ],
        )
        .subtitle(self.context.clone())
        .note(vec![
            "Scheduled runs need the passphrase, so the daemon stores it sealed with its".into(),
            "master key. A new one replaces it; it is never shown again.".into(),
        ])
        .submit("save", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_instance_download(&mut self) {
        let Some(b) = self.settings.selected_backup().cloned() else {
            return self.error("no instance backup selected · b to take one");
        };
        if b.status != BackupStatus::Done {
            return self.error(match b.status {
                BackupStatus::Running => "that backup is still running",
                _ => "that backup failed and has no file",
            });
        }
        let form = Form::new(
            FormKind::DownloadBackup {
                backup_id: b.id.clone(),
                file_name: b.file_name.clone(),
                instance: true,
            },
            "Download instance backup",
            vec![
                Field::text("path", "save to")
                    .value(format!("./{}", b.file_name))
                    .hint(format!("{} · encrypted", crate::util::bytes(b.size_bytes))),
            ],
        )
        .subtitle(b.file_name.clone())
        .note(vec![restore_hint()])
        .submit("download", None);
        self.overlay = Some(Overlay::Form(form));
    }

    // Pickers

    fn daemon_key(&self) -> Option<String> {
        self.info.as_ref().and_then(|i| i.ssh_public_key.clone())
    }

    fn open_picker(&mut self, picker: Picker) {
        let kind = picker.kind;
        self.overlay = Some(Overlay::Picker(picker));
        self.refill_picker();
        match kind {
            PickKind::Store => self.load_storages(),
            PickKind::SshKey | PickKind::S3Credentials => self.load_secrets(),
            PickKind::GitSource
            | PickKind::Repo
            | PickKind::Branch
            | PickKind::Channel
            | PickKind::Scope => {}
        }
    }

    /// ⏎ on the git source field: the ready sources, then the repository and branch pickers.
    fn pick_git_source(&mut self, form: Form) {
        let current = Some(form.raw("git_source")).filter(|v| !v.is_empty());
        let mut items: Vec<PickItem> = self
            .sources
            .ready()
            .into_iter()
            .map(|s| {
                let mut item = PickItem::new(Some(s.id.clone()), &s.name, source_summary(s));
                item.glyph = Some(("●", theme::fg(theme::OK)));
                item.note = match (s.kind, s.installation_id) {
                    (GitSourceKind::GithubApp, Some(i)) => format!("installation {i}"),
                    (GitSourceKind::GithubApp, None) => "github app".into(),
                    (GitSourceKind::GithubToken, _) => "no deploy on push".into(),
                };
                item
            })
            .collect();
        let mut connect = PickItem::new(None, "+ connect a source", "opens the Sources screen");
        connect.generate = true;
        items.push(connect);
        let waiting: Vec<&str> = self
            .sources
            .list
            .iter()
            .filter(|s| s.status != GitSourceStatus::Ready)
            .map(|s| s.name.as_str())
            .collect();
        let note = match waiting.as_slice() {
            [] => "Sources are GitHub Apps and tokens, set up on the Sources screen.".to_string(),
            [one] => format!("{one} is not ready yet; finish it on the Sources screen."),
            many => format!(
                "{} sources are not ready yet; finish them on the Sources screen.",
                many.len()
            ),
        };
        let row = current
            .as_ref()
            .and_then(|c| items.iter().position(|i| i.id.as_ref() == Some(c)))
            .unwrap_or(0);
        self.open_picker(Picker {
            title: "Git source".into(),
            subtitle: form.subtitle.clone(),
            kind: PickKind::GitSource,
            items,
            row,
            target: PickFor::Field(Box::new(form), "git_source"),
            current,
            note: vec![Line::styled(note, theme::muted())],
            busy: false,
            git: None,
        });
    }

    fn pick_repo(&mut self, form: Form) {
        let source_id = form.raw("git_source");
        if source_id.is_empty() {
            let mut form = form;
            form.fail("pick a git source first");
            self.overlay = Some(Overlay::Form(form));
            return;
        }
        let name = self
            .sources
            .list
            .iter()
            .find(|s| s.id == source_id)
            .map_or(String::new(), |s| s.name.clone());
        let current = Some(form.raw("gh_repo")).filter(|v| !v.is_empty());
        self.open_picker(Picker {
            title: "Repository".into(),
            subtitle: name,
            kind: PickKind::Repo,
            items: Vec::new(),
            row: 0,
            target: PickFor::Field(Box::new(form), "gh_repo"),
            current,
            note: Vec::new(),
            busy: false,
            git: Some(GitPick {
                source_id,
                page: 1,
                pages: 1,
                ..GitPick::default()
            }),
        });
        self.request_repos();
    }

    fn pick_branch(&mut self, form: Form) {
        let (source_id, repo) = (form.raw("git_source"), form.raw("gh_repo"));
        if repo.is_empty() {
            let mut form = form;
            form.fail("pick a repository first");
            self.overlay = Some(Overlay::Form(form));
            return;
        }
        let current = Some(form.raw("gh_branch")).filter(|v| !v.is_empty());
        let (source, name) = (source_id.clone(), repo.clone());
        self.open_picker(Picker {
            title: "Branch".into(),
            subtitle: repo.clone(),
            kind: PickKind::Branch,
            items: Vec::new(),
            row: 0,
            target: PickFor::Field(Box::new(form), "gh_branch"),
            current,
            note: Vec::new(),
            busy: false,
            git: Some(GitPick {
                source_id,
                repo,
                loading: true,
                ..GitPick::default()
            }),
        });
        self.spawn(|c| async move {
            let result = c
                .source_branches(&source, &name)
                .await
                .map_err(|e| format!("{e:#}"));
            Msg::Branches {
                source,
                repo: name,
                result,
            }
        });
    }

    /// Ask the daemon for the open repository picker's page and search.
    pub fn request_repos(&mut self) {
        let Some(Overlay::Picker(p)) = &mut self.overlay else {
            return;
        };
        let Some(git) = p.git.as_mut().filter(|_| p.kind == PickKind::Repo) else {
            return;
        };
        git.typed_at = None;
        git.loading = true;
        let (source, query, page) = (git.source_id.clone(), git.search.clone(), git.page);
        self.spawn(move |c| async move {
            let result = c
                .source_repos(&source, &query, page, REPO_PAGE)
                .await
                .map_err(|e| format!("{e:#}"));
            Msg::Repos {
                source,
                query,
                page,
                result,
            }
        });
    }

    /// Typing in the repository search waits for a pause before it asks the daemon.
    fn search_repos_when_due(&mut self) {
        let due = matches!(
            &self.overlay,
            Some(Overlay::Picker(Picker { git: Some(git), .. }))
                if git.typed_at.is_some_and(|t| t.elapsed() >= SEARCH_DEBOUNCE)
        );
        if due {
            self.request_repos();
        }
    }

    /// A typed character or backspace in a search picker.
    pub fn picker_search(&mut self, edit: impl FnOnce(&mut String)) {
        let Some(Overlay::Picker(p)) = &mut self.overlay else {
            return;
        };
        let Some(git) = p.git.as_mut() else {
            return;
        };
        let before = git.search.clone();
        edit(&mut git.search);
        if git.search == before {
            return;
        }
        match p.kind {
            PickKind::Repo => {
                git.page = 1;
                git.typed_at = Some(Instant::now());
            }
            _ => p.filter_branches(),
        }
    }

    /// ← and → in the repository picker.
    pub fn picker_page(&mut self, step: i32) {
        let Some(Overlay::Picker(p)) = &mut self.overlay else {
            return;
        };
        let Some(git) = p.git.as_mut().filter(|_| p.kind == PickKind::Repo) else {
            return;
        };
        let page = git
            .page
            .saturating_add_signed(step)
            .clamp(1, git.pages.max(1));
        if page != git.page {
            git.page = page;
            p.row = 0;
            self.request_repos();
        }
    }

    /// A choice in one of the GitHub pickers fills its field, then the next picker opens.
    fn accept_git_pick(&mut self, p: Picker) {
        let PickFor::Field(mut form, key) = p.target else {
            return;
        };
        let Some(item) = p.items.get(p.row) else {
            self.overlay = Some(Overlay::Form(*form));
            return;
        };
        let value = item.id.clone().unwrap_or_default();
        let set = |form: &mut Form, key: &str, value: &str, shown: &str, detail: &str| {
            if let Some(f) = form.field_mut(key) {
                f.value = value.to_string();
                f.shown = shown.to_string();
                f.placeholder = detail.to_string();
            }
        };
        form.error = None;
        match p.kind {
            PickKind::GitSource if item.generate => {
                self.overlay = None;
                self.open_sources();
            }
            PickKind::GitSource => {
                set(&mut form, key, &value, &item.name, &item.detail);
                if form.raw("gh_repo").is_empty() || value != p.current.unwrap_or_default() {
                    set(&mut form, "gh_repo", "", "", "pick a repository");
                    set(&mut form, "gh_branch", "", "", "pick a branch");
                }
                form.refresh();
                self.pick_repo(*form);
            }
            PickKind::Repo => {
                set(
                    &mut form,
                    key,
                    &value,
                    &item.name,
                    if item.badge.is_empty() {
                        "public"
                    } else {
                        &item.badge
                    },
                );
                set(
                    &mut form,
                    "gh_branch",
                    &item.detail,
                    &item.detail,
                    "default",
                );
                if form.get("name").is_empty()
                    && let Some(name) = form.field_mut("name")
                {
                    name.value = derive_name(&value);
                }
                form.refresh();
                self.pick_branch(*form);
            }
            _ => {
                set(&mut form, key, &value, &item.name, &item.badge);
                if let Some(build) = form.field_mut("build") {
                    build.placeholder.clear();
                }
                form.refresh();
                if let Some(i) = form.fields.iter().position(|f| f.key == key) {
                    form.focus = i;
                    form.move_focus(1);
                }
                let (source, repo) = (form.raw("git_source"), form.raw("gh_repo"));
                self.overlay = Some(Overlay::Form(*form));
                self.spawn(move |c| async move {
                    let result = c
                        .detect_build(&source, &repo, &value)
                        .await
                        .map_err(|e| format!("{e:#}"));
                    Msg::Detected {
                        source,
                        repo,
                        branch: value,
                        result,
                    }
                });
            }
        }
    }

    /// "k" on the Servers screen: the key a server connects with.
    pub fn pick_server_key(&mut self, server: &Server) {
        if server.kind == ServerKind::Local {
            return self.error("the local server needs no ssh key");
        }
        self.open_picker(Picker {
            title: "SSH key".into(),
            subtitle: server.name.clone(),
            kind: PickKind::SshKey,
            items: Vec::new(),
            row: 0,
            target: PickFor::ServerKey {
                server_id: server.id.clone(),
                server: server.name.clone(),
            },
            current: server.ssh_key_secret_id.clone(),
            note: vec![
                Line::styled(
                    format!(
                        "Add the public key to ~/.ssh/authorized_keys on {} first.",
                        server.name
                    ),
                    theme::muted(),
                ),
                Line::styled(
                    "The daemon reconnects with the new key and checks it.",
                    theme::muted(),
                ),
            ],
            busy: false,
            git: None,
        });
    }

    /// ⏎ on a pick field of the open form.
    pub fn pick_for_form(&mut self, key: &'static str) {
        let Some(Overlay::Form(form)) = self.overlay.take() else {
            return;
        };
        match key {
            "git_source" => return self.pick_git_source(form),
            "gh_repo" => return self.pick_repo(form),
            "gh_branch" => return self.pick_branch(form),
            _ => {}
        }
        let current = Some(form.raw(key)).filter(|v| !v.is_empty());
        let (kind, title) = match key {
            "ssh_key" => (PickKind::SshKey, "SSH key"),
            "credentials" => (PickKind::S3Credentials, "Credentials"),
            _ => (PickKind::Store, "Store"),
        };
        let subtitle = form.title.to_lowercase();
        let note = match kind {
            PickKind::SshKey => vec![Line::styled(
                "Add the public key to ~/.ssh/authorized_keys on the server first.",
                theme::muted(),
            )],
            PickKind::S3Credentials => vec![Line::styled(
                "S3 credentials secrets; add them on the Secrets screen.",
                theme::muted(),
            )],
            PickKind::Store | PickKind::GitSource | PickKind::Repo | PickKind::Branch => {
                vec![Line::styled(
                    "Storage connections are managed on the Storage screen (o in Settings).",
                    theme::muted(),
                )]
            }
            PickKind::Channel | PickKind::Scope => Vec::new(),
        };
        self.open_picker(Picker {
            title: title.into(),
            subtitle,
            kind,
            items: Vec::new(),
            row: 0,
            target: PickFor::Field(Box::new(form), key),
            current,
            note,
            busy: false,
            git: None,
        });
    }

    pub fn picker_cancel(&mut self) {
        let Some(Overlay::Picker(p)) = self.overlay.take() else {
            return;
        };
        match p.target {
            PickFor::Field(form, _) => self.overlay = Some(Overlay::Form(*form)),
            PickFor::Rule(dialog) => self.overlay = Some(Overlay::Rule(*dialog)),
            _ => {}
        }
    }

    pub fn picker_copy(&mut self) {
        let Some(Overlay::Picker(p)) = &self.overlay else {
            return;
        };
        if let Some(text) = p.selected().and_then(|i| i.copy.clone()) {
            self.copy(&text, "the public key");
        }
    }

    pub fn picker_accept(&mut self) {
        let Some(Overlay::Picker(p)) = &mut self.overlay else {
            return;
        };
        if p.busy {
            return;
        }
        if matches!(
            p.kind,
            PickKind::GitSource | PickKind::Repo | PickKind::Branch
        ) {
            if let Some(Overlay::Picker(p)) = self.overlay.take() {
                self.accept_git_pick(p);
            }
            return;
        }
        let Some(item) = p.selected() else {
            return self.picker_cancel();
        };
        if item.generate {
            p.busy = true;
            let base = match &p.target {
                PickFor::ServerKey { server, .. } => server.clone(),
                PickFor::BackupNow { name, .. } => name.clone(),
                PickFor::Field(form, _) => Some(form.get("name"))
                    .filter(|n| !n.is_empty())
                    .unwrap_or("server".into()),
                PickFor::Rule(_) => "rule".into(),
            };
            let name = unique_name(&format!("{base}-key"), &self.secrets.list);
            return self.spawn(|c| async move {
                Msg::PickerKey(
                    c.generate_ssh_key(&name)
                        .await
                        .map(Box::new)
                        .map_err(|e| format!("{e:#}")),
                )
            });
        }
        let (id, name) = (item.id.clone(), item.name.clone());
        let Some(Overlay::Picker(p)) = self.overlay.take() else {
            return;
        };
        match p.target {
            PickFor::Field(mut form, key) => {
                let detail = p.items[p.row].detail.clone();
                if let Some(field) = form.field_mut(key) {
                    field.value = id.unwrap_or_default();
                    field.shown = name;
                    field.placeholder = detail;
                }
                form.error = None;
                form.move_focus(1);
                self.overlay = Some(Overlay::Form(*form));
            }
            PickFor::BackupNow { service_id, name } => {
                self.start_backup(&service_id, &name, id);
            }
            PickFor::Rule(mut dialog) => {
                let id = id.unwrap_or_default();
                if p.kind == PickKind::Channel {
                    if let Some(c) = self.notify.channel(&id) {
                        dialog.channel_detail = c.kind.label().to_string();
                    }
                    dialog.channel_id = id;
                    dialog.channel = name;
                    dialog.focus = notify::RuleFocus::Scope;
                } else {
                    dialog.scope = notify::parse_scope(&id);
                    dialog.scope_label = name;
                    dialog.focus = notify::RuleFocus::Dedupe;
                }
                dialog.error = None;
                self.overlay = Some(Overlay::Rule(*dialog));
            }
            PickFor::ServerKey { server_id, server } => {
                if id == p.current {
                    return;
                }
                self.progress(format!("connecting to {server} with {name}…"));
                let body = UpdateServer {
                    ssh_key_secret_id: Some(id.unwrap_or_default()),
                };
                self.spawn(move |c| async move {
                    match c.update_server(&server_id, &body).await {
                        Ok(s) if s.status == ServerStatus::Online => {
                            Msg::Toast(format!("{server} now connects with {name}"))
                        }
                        Ok(_) => Msg::Error(format!(
                            "{server} switched to {name} but is unreachable · check authorized_keys"
                        )),
                        Err(e) => Msg::Error(format!("{e:#}")),
                    }
                });
            }
        }
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
        let ready = self.sources.ready();
        let only = match ready.as_slice() {
            [one] => Some(*one),
            _ => None,
        };
        let github = !ready.is_empty() || !self.sources.loaded;
        let mut source = Field::pick(
            "git_source",
            "git source",
            only.map(|s| s.id.clone()).unwrap_or_default(),
            only.map(|s| s.name.clone()).unwrap_or_default(),
        );
        source.placeholder = only.map_or("pick a source".into(), source_summary);
        let mut gh_repo = Field::pick("gh_repo", "repository", "", "");
        gh_repo.placeholder = "pick a repository".into();
        let mut gh_branch = Field::pick("gh_branch", "branch", "", "");
        gh_branch.placeholder = "pick a branch".into();
        let mut fields = vec![
            Field::choice(
                "source",
                "source",
                vec![
                    "GitHub".into(),
                    "Git URL".into(),
                    "Docker image".into(),
                    "Database".into(),
                ],
            )
            .selected(if github { 0 } else { 1 }),
            source,
            gh_repo,
            gh_branch,
            Field::text("repo", "repository")
                .placeholder("https://github.com/acme/api")
                .hint("https or ssh url"),
            Field::text("branch", "branch").value("main"),
        ];
        fields.extend(build_fields(None, ""));
        fields.extend([
            Field::text("image", "image").placeholder("ghcr.io/acme/web:latest"),
            Field::choice("engine", "engine", engines),
            Field::text("version", "version").hint("empty = newest"),
            Field::text("name", "name"),
            Field::text("port", "port")
                .placeholder("3000")
                .hint("app listen port"),
            Field::text("domain", "domain")
                .placeholder("app.example.com")
                .hint("optional · https"),
            Field::toggle("auto_deploy", "auto-deploy", true),
        ]);
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
        .focus_on(match (github, only) {
            (false, _) => "repo",
            (true, None) => "git_source",
            (true, Some(_)) => "gh_repo",
        });
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
        let mut git_source = None;
        match &s.source {
            Source::Git {
                repo,
                branch,
                source_id,
                auto_deploy,
                ..
            } => {
                fields.push(Field::text("repo", "repository").value(repo.clone()));
                fields.push(Field::text("branch", "branch").value(branch.clone()));
                fields.extend(build_fields(Some(&s.source), "applies on next deploy"));
                if source_id.is_some() {
                    fields.push(
                        Field::toggle("auto_deploy", "auto-deploy", *auto_deploy)
                            .hint("deploys every push"),
                    );
                }
                git_source = source_id.clone();
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
                source_id: git_source,
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
        let compose: Vec<String> = view.compose().iter().map(|s| s.name.clone()).collect();
        let mut fields = vec![
            Field::text("host", "host")
                .placeholder("shop.example.com")
                .hint("point an A record here"),
        ];
        if !compose.is_empty() {
            fields.push(
                Field::choice("compose_service", "service", compose).hint("in the compose file"),
            );
        }
        fields.extend([
            Field::text("port", "port")
                .placeholder(port.map_or("service port".into(), |p| p.to_string()))
                .hint("container port"),
            Field::toggle("tls", "https", true).hint("automatic certificate"),
        ]);
        let subtitle = match (&view.detail, view.compose().is_empty()) {
            (Some(d), false) => format!("{} · compose", d.service.name),
            _ => self
                .info
                .as_ref()
                .and_then(|i| i.public_ip.clone())
                .unwrap_or_default(),
        };
        let form = Form::new(
            FormKind::AddDomain {
                service_id: view.id.clone(),
            },
            "Add domain",
            fields,
        )
        .subtitle(subtitle)
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
                Field::pick("ssh_key", "ssh key", "", "daemon's own key"),
            ],
        )
        .note(vec![
            "The key must be in ~/.ssh/authorized_keys on the server. ⏎ on ssh key picks it."
                .into(),
        ])
        .subtitle(if has_key {
            "trust the daemon key first"
        } else {
            "over ssh"
        })
        .submit("add & check", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_expose(&mut self) {
        let Some(view) = &self.service else { return };
        let (Some(info), Some(detail)) = (view.info(), view.detail.as_ref()) else {
            return;
        };
        let name = detail.service.name.clone();
        let form = Form::new(
            FormKind::ExposeDatabase {
                service_id: view.id.clone(),
                name: name.clone(),
            },
            "Expose database",
            vec![
                Field::text("port", "port")
                    .value(crate::database::suggested_public_port(info.port).to_string())
                    .hint("host port"),
            ],
        )
        .subtitle(name.clone())
        .submit("continue", None)
        .warning(vec![
            format!(
                "{name} becomes reachable from the internet on {}.",
                self.server_address(&detail.server)
            ),
            "Anyone who finds the port can try the password. Unexpose it when you are done.".into(),
        ]);
        self.overlay = Some(Overlay::Form(form));
    }

    /// The address a published port is reachable on.
    pub fn server_address(&self, server: &Server) -> String {
        match server.kind {
            ServerKind::Ssh => server.host.clone(),
            ServerKind::Local => self
                .info
                .as_ref()
                .map(|i| i.public_ip.clone().unwrap_or_else(|| i.hostname.clone())),
        }
        .unwrap_or_else(|| server.name.clone())
    }

    fn submit_expose(&mut self) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        let FormKind::ExposeDatabase { service_id, name } = form.kind.clone() else {
            return;
        };
        let p = match port(&form.get("port"), true) {
            Ok(Some(p)) => p,
            Ok(None) => return,
            Err(e) => return form.fail(e),
        };
        let address = self
            .service
            .as_ref()
            .and_then(|v| v.detail.as_ref())
            .map(|d| self.server_address(&d.server))
            .unwrap_or_default();
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: "Expose database".into(),
            message: format!(
                "Publish {name} on {address}:{p}? It becomes reachable from the internet, and the container restarts once to apply it."
            ),
            danger: true,
            action: ConfirmAction::PublicPort(service_id, Some(p)),
            verb: "confirm",
            busy: None,
            error: Vec::new(),
        }));
    }

    pub fn form_schedule(&mut self) {
        let Some(view) = &self.service else { return };
        let s = self.schedules.get(&view.id).cloned().unwrap_or_default();
        let store = self.store_field(s.storage_id.as_deref());
        let name = view
            .detail
            .as_ref()
            .map(|d| d.service.name.clone())
            .unwrap_or_default();
        let form = Form::new(
            FormKind::BackupSchedule {
                service_id: view.id.clone(),
            },
            "Backup schedule",
            vec![
                Field::toggle("enabled", "daily", s.enabled).hint("off keeps old backups"),
                Field::text("hour", "hour (UTC)").value(s.hour_utc.to_string()),
                Field::text("keep", "keep")
                    .value(s.keep.to_string())
                    .hint("per store"),
                store,
            ],
        )
        .subtitle(name)
        .submit("save", None);
        self.overlay = Some(Overlay::Form(form));
    }

    pub fn form_download(&mut self, backup: &Backup) {
        let form = Form::new(
            FormKind::DownloadBackup {
                backup_id: backup.id.clone(),
                file_name: backup.file_name.clone(),
                instance: false,
            },
            "Download backup",
            vec![
                Field::text("path", "save to")
                    .value(format!("./{}", backup.file_name))
                    .hint(format!("{} · gzip", crate::util::bytes(backup.size_bytes))),
            ],
        )
        .subtitle(backup.file_name.clone())
        .submit("download", None);
        self.overlay = Some(Overlay::Form(form));
    }

    fn submit_download(&mut self) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        let FormKind::DownloadBackup {
            backup_id,
            file_name,
            instance,
        } = form.kind.clone()
        else {
            return;
        };
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let target =
            crate::database::download_target(&form.raw("path"), &file_name, home.as_deref());
        if target.exists() {
            return form.fail(format!("{} already exists", target.display()));
        }
        if let Some(dir) = target.parent().filter(|d| !d.as_os_str().is_empty())
            && !dir.is_dir()
        {
            return form.fail(format!("folder {} does not exist", dir.display()));
        }
        let total = self
            .backups
            .values()
            .flatten()
            .find(|b| b.id == backup_id)
            .map(|b| b.size_bytes)
            .or_else(|| {
                self.settings
                    .backups
                    .iter()
                    .find(|b| b.id == backup_id)
                    .map(|b| b.size_bytes)
            })
            .unwrap_or(0);
        self.overlay = None;
        self.progress(format!("downloading {file_name}"));
        let client = self.client.clone();
        let tx = self.sender();
        tokio::spawn(async move {
            let file = Download {
                id: &backup_id,
                name: &file_name,
                instance,
                total,
            };
            let msg = match download(&client, file, &target, &tx).await {
                Ok(n) => Msg::Toast(format!(
                    "saved {} · {}",
                    target.display(),
                    crate::util::bytes(n)
                )),
                Err(e) => Msg::Error(format!("download failed: {e:#}")),
            };
            tx.send(msg);
        });
    }

    /// Validate the open form and send its request.
    pub fn submit_form(&mut self, alt: bool) {
        let Some(Overlay::Form(form)) = &mut self.overlay else {
            return;
        };
        match form.kind {
            FormKind::ExposeDatabase { .. } => return self.submit_expose(),
            FormKind::DownloadBackup { .. } => return self.submit_download(),
            FormKind::NotifyKind => return self.notify_kind_chosen(),
            FormKind::NotifyChannel { .. } => return self.submit_notify_channel(),
            _ => {}
        }
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

const DEFAULT_GITHUB: &str = "https://github.com";

/// What a finished set or check of the instance domain says.
/// Replaces this CLI binary with the release `tag`.
async fn update_cli(tag: &str) -> anyhow::Result<UpdateResult> {
    release::update_cli(&release::Releases::new()?, tag, false).await
}

fn domain_toast(d: &InstanceDomain) -> Option<String> {
    let domain = d.domain.clone()?;
    if d.verified {
        return Some(format!(
            "{domain} verified · {} answers",
            d.url.as_deref().unwrap_or(&domain)
        ));
    }
    let passed = d
        .checks
        .iter()
        .filter(|c| c.status == CheckStatus::Pass)
        .count();
    Some(format!(
        "{domain} · {passed} of {} checks pass",
        DomainCheckKind::ALL.len()
    ))
}

/// How an instance backup comes back on a new machine.
pub fn restore_hint() -> String {
    format!(
        "Restore with: {} restore <file|s3://bucket/key> --passphrase-file <file>",
        brand::DAEMON_BIN
    )
}

/// The secret part of a value: what "copy value" copies.
pub fn secret_text(value: &SecretValue) -> String {
    match value {
        SecretValue::SshKey { private_key } => private_key.clone(),
        SecretValue::S3Credentials {
            secret_access_key, ..
        } => secret_access_key.clone(),
        SecretValue::Generic { value } => value.clone(),
        SecretValue::GithubApp { private_key, .. } => private_key.clone(),
    }
}

/// Kinds a person can add on the Secrets screen; GitHub App credentials come from a source.
pub const ADDABLE_SECRETS: [SecretKind; 3] = [
    SecretKind::SshKey,
    SecretKind::S3Credentials,
    SecretKind::Generic,
];

/// "managed by source acme-github", from what uses a GitHub App secret.
pub fn managed_by(s: &Secret) -> String {
    match s.used_by.iter().find(|u| u.kind == UsageKind::Source) {
        Some(u) => format!("managed by source {}", u.name),
        None => "managed by its source".to_string(),
    }
}

/// `base`, or `base-2`, `base-3`… when a secret already has that name.
fn unique_name(base: &str, secrets: &[Secret]) -> String {
    let base: String = base.chars().take(44).collect();
    let taken = |n: &str| secrets.iter().any(|s| s.name == n);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|i| format!("{base}-{i}"))
        .find(|n| !taken(n))
        .unwrap_or(base)
}

pub fn is_database(s: &Service) -> bool {
    matches!(s.source, Source::Database { .. })
}

/// Stream a backup to `<target>.part`, reporting progress, then move it into place.
/// A backup file to download: a database backup, or an instance backup when `instance`.
struct Download<'a> {
    id: &'a str,
    name: &'a str,
    instance: bool,
    total: u64,
}

async fn download(
    client: &Client,
    file: Download<'_>,
    target: &std::path::Path,
    tx: &Sender,
) -> anyhow::Result<u64> {
    use tokio::io::AsyncWriteExt;
    let Download {
        id,
        name,
        instance,
        total,
    } = file;
    let part = PathBuf::from(format!("{}.part", target.display()));
    let result = async {
        let mut stream = if instance {
            client.download_instance_backup(id).await?
        } else {
            client.download_backup(id).await?
        };
        let mut file = tokio::fs::File::create(&part).await?;
        let mut written = 0u64;
        let mut last = Instant::now();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            written += chunk.len() as u64;
            if last.elapsed() >= Duration::from_millis(200) {
                last = Instant::now();
                tx.send(Msg::Progress(format!(
                    "downloading {name} · {} of {}",
                    crate::util::bytes(written),
                    crate::util::bytes(total)
                )));
            }
        }
        file.flush().await?;
        tokio::fs::rename(&part, target).await?;
        Ok(written)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    result
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
    Settings(UpdateSettings),
    PurgeServer {
        id: Id,
        volumes: bool,
    },
    Schedule(Id, BackupSchedule),
    Secret(CreateSecret),
    GenerateKey(String),
    UpdateSecret(Id, UpdateSecret),
    Storage(Option<Id>, CreateStorage),
    InstanceBackup(CreateInstanceBackup),
    InstanceSchedule(UpdateInstanceBackupSchedule),
    GithubApp(CreateGithubApp),
    ConnectApp(ConnectGithubApp),
    Token(ConnectGithubToken),
}

/// "acme-github is ready", or what is wrong with a source that was just connected.
fn connected(s: &GitSource) -> String {
    match (s.status, &s.status_detail) {
        (GitSourceStatus::Ready, _) => format!("{} connected · ready", s.name),
        (status, Some(detail)) => format!("{} added · {}: {detail}", s.name, status.as_str()),
        (status, None) => format!("{} added · {}", s.name, status.as_str()),
    }
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
                if is_database(&d.service) {
                    let note = if deploy { " · deploying" } else { "" };
                    return Ok((
                        format!("{name} created{note}"),
                        Then::OpenService(d.service.id, Tab::Database),
                    ));
                }
                if deploy
                    && let Some(first) = c.deployments(&d.service.id, 1).await?.into_iter().next()
                {
                    return Ok((
                        format!("{name} created · deploying"),
                        Then::OpenDeploy(Box::new(first)),
                    ));
                }
                (
                    format!("{name} created"),
                    Then::OpenService(d.service.id, Tab::Overview),
                )
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
            Request::Settings(body) => {
                let s = c.update_settings(&body).await?;
                let toast = match &s.public_ip_override {
                    Some(ip) => format!("public IP set to {ip}"),
                    None => "public IP is detected again".into(),
                };
                (toast, Then::Settings(Box::new(s)))
            }
            Request::PurgeServer { id, volumes } => {
                let r = c.purge_server(&id, volumes).await?;
                (
                    format!(
                        "server removed · {} containers, {} networks, {} volumes, {} services",
                        r.containers, r.networks, r.volumes, r.services
                    ),
                    Then::Stay,
                )
            }
            Request::Secret(body) => {
                let s = c.create_secret(&body).await?;
                (format!("secret {} added", s.name), Then::Stay)
            }
            Request::GenerateKey(name) => {
                let s = c.generate_ssh_key(&name).await?;
                (
                    format!("ssh key {} generated", s.name),
                    Then::ShowKey(Box::new(s)),
                )
            }
            Request::UpdateSecret(id, body) => {
                let renamed = body.name.is_some();
                let s = c.update_secret(&id, &body).await?;
                let toast = if renamed {
                    format!("renamed to {}", s.name)
                } else {
                    format!("value of {} replaced", s.name)
                };
                (toast, Then::Stay)
            }
            Request::Storage(id, body) => {
                let s = match id {
                    Some(id) => {
                        let update = UpdateStorage {
                            name: Some(body.name),
                            endpoint: Some(body.endpoint),
                            region: Some(body.region),
                            bucket: Some(body.bucket),
                            path_prefix: Some(body.path_prefix),
                            path_style: Some(body.path_style),
                            credentials_secret_id: Some(body.credentials_secret_id),
                        };
                        c.update_storage(&id, &update).await?
                    }
                    None => c.create_storage(&body).await?,
                };
                let test = match c.test_storage(&s.id).await {
                    Ok(t) if t.ok => format!("test ok {}ms", t.latency_ms.unwrap_or_default()),
                    Ok(t) => format!(
                        "test failed: {}",
                        t.error.as_deref().unwrap_or("no reason given")
                    ),
                    Err(e) => format!("test failed: {e:#}"),
                };
                (format!("{} saved · {test}", s.name), Then::Stay)
            }
            Request::InstanceBackup(body) => {
                let b = c.create_instance_backup(&body).await?;
                (
                    format!("backing up the instance · {}", b.file_name),
                    Then::Stay,
                )
            }
            Request::InstanceSchedule(body) => {
                let s = c.set_instance_backup_schedule(&body).await?;
                let toast = if s.enabled {
                    format!(
                        "instance backups daily at {:02}:00 UTC · keep {}",
                        s.hour_utc, s.keep
                    )
                } else {
                    "instance backup schedule off".to_string()
                };
                (toast, Then::InstanceSchedule(s))
            }
            Request::GithubApp(body) => {
                let setup = c.create_github_app(&body).await?;
                (
                    format!("{} created · open the URL to continue", setup.source.name),
                    Then::GithubSetup(Box::new(setup)),
                )
            }
            Request::ConnectApp(body) => {
                (connected(&c.connect_github_app(&body).await?), Then::Stay)
            }
            Request::Token(body) => (connected(&c.connect_github_token(&body).await?), Then::Stay),
            Request::Schedule(id, body) => {
                let s = c.set_backup_schedule(&id, &body).await?;
                (
                    format!("schedule saved · {}", crate::database::schedule_summary(&s)),
                    Then::Stay,
                )
            }
        })
    }
}

/// The passphrase typed twice, or `None` when both fields are empty.
fn passphrase(form: &Form) -> Result<Option<String>, String> {
    let (p, repeat) = (form.raw("passphrase"), form.raw("repeat"));
    if p.is_empty() && repeat.is_empty() {
        return Ok(None);
    }
    if p.chars().count() < MIN_PASSPHRASE_LEN {
        return Err(format!(
            "the passphrase needs at least {MIN_PASSPHRASE_LEN} characters"
        ));
    }
    if p != repeat {
        return Err("the passphrases do not match".into());
    }
    Ok(Some(p))
}

/// The value fields of a secret form. A private key that is not pasted is read from the path typed.
fn secret_value(form: &Form, kind: SecretKind) -> Result<SecretValue, String> {
    let required = |key: &str, what: &str| {
        let v = form.raw(key);
        if v.trim().is_empty() {
            Err(format!("{what} is required"))
        } else {
            Ok(v)
        }
    };
    Ok(match kind {
        SecretKind::SshKey => {
            let input = required("private_key", "the private key")?;
            let home = std::env::var_os("HOME").map(PathBuf::from);
            SecretValue::SshKey {
                private_key: read_private_key(input.trim(), home.as_deref())?,
            }
        }
        SecretKind::S3Credentials => SecretValue::S3Credentials {
            access_key_id: required("access_key_id", "the access key id")?
                .trim()
                .to_string(),
            secret_access_key: required("secret_access_key", "the secret key")?
                .trim()
                .to_string(),
        },
        SecretKind::Generic => SecretValue::Generic {
            value: required("value", "the value")?,
        },
        SecretKind::GithubApp => {
            return Err("GitHub App credentials are managed by their source".into());
        }
    })
}

/// A pasted OpenSSH private key as is, or the contents of the file at the path typed.
pub fn read_private_key(input: &str, home: Option<&std::path::Path>) -> Result<String, String> {
    if input.contains("PRIVATE KEY") {
        return Ok(format!("{}\n", input.trim_end()));
    }
    let path = match (input.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(input),
    };
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if !text.contains("PRIVATE KEY") {
        return Err(format!("{} is not a private key", path.display()));
    }
    Ok(text)
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

/// Chip labels of the build methods, in `BuildMethod::ALL` order.
const BUILD_LABELS: [&str; 4] = ["auto", "Dockerfile", "Compose", "Railpack"];

/// The build method chips and the fields each method needs, filled from a git source when
/// editing. `note` is a fixed hint on the chips, e.g. what the repository was detected as.
fn build_fields(git: Option<&Source>, note: &str) -> Vec<Field> {
    let (dockerfile, context, build) = match git {
        Some(Source::Git {
            dockerfile,
            context,
            build,
            ..
        }) => (dockerfile.clone(), context.clone(), build.clone()),
        _ => (
            "Dockerfile".into(),
            ".".into(),
            Build {
                method: BuildMethod::Auto,
                ..Build::default()
            },
        ),
    };
    let mut method = Field::choice(
        "build",
        "build",
        BUILD_LABELS.iter().map(|l| l.to_string()).collect(),
    )
    .selected(
        BuildMethod::ALL
            .iter()
            .position(|m| *m == build.method)
            .unwrap_or(0),
    );
    method.placeholder = note.into();
    vec![
        method,
        Field::text("dockerfile", "dockerfile")
            .value(dockerfile)
            .hint("path in the repo"),
        Field::text("context", "context")
            .value(context)
            .hint("folder sent to build"),
        Field::text("target", "target")
            .value(build.target.unwrap_or_default())
            .placeholder("stage to build")
            .hint("optional"),
        Field::text("compose_file", "compose file")
            .value(build.compose_file.unwrap_or_default())
            .placeholder("found on its own")
            .hint("one group"),
    ]
}

/// The Dockerfile, build context and build settings typed in the build fields.
fn build_parts(form: &Form) -> (String, String, Build) {
    let typed = |key: &str| Some(form.get(key)).filter(|v| !v.is_empty());
    let method = BuildMethod::ALL[form.choice("build").min(BuildMethod::ALL.len() - 1)];
    (
        typed("dockerfile").unwrap_or("Dockerfile".into()),
        typed("context").unwrap_or(".".into()),
        Build {
            method,
            target: typed("target"),
            compose_file: typed("compose_file"),
        },
    )
}

/// "✓ compose.yaml", what decided the build at the repository's root; short enough for a hint.
fn detected_note(d: &BuildDetection) -> String {
    let what = match (d.method, &d.file) {
        (BuildMethod::Railpack, _) => "Railpack".to_string(),
        (_, Some(file)) => file.clone(),
        (method, None) => BUILD_LABELS[BuildMethod::ALL
            .iter()
            .position(|m| *m == method)
            .unwrap_or(0)]
        .to_string(),
    };
    format!("✓ {what}")
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
                    let source_id = form.get("git_source");
                    if source_id.is_empty() {
                        return Err("pick a git source (⏎ on git source)".into());
                    }
                    let repo = form.get("gh_repo");
                    if repo.is_empty() {
                        return Err("pick a repository (⏎ on repository)".into());
                    }
                    let (dockerfile, context, build) = build_parts(form);
                    Source::Git {
                        repo,
                        branch: Some(form.get("gh_branch"))
                            .filter(|b| !b.is_empty())
                            .unwrap_or("main".into()),
                        dockerfile,
                        context,
                        source_id: Some(source_id),
                        auto_deploy: form.on("auto_deploy"),
                        build,
                    }
                }
                1 => {
                    let repo = form.get("repo");
                    if repo.is_empty() {
                        return Err("repository is required".into());
                    }
                    let or = |key: &str, default: &str| {
                        Some(form.get(key))
                            .filter(|v| !v.is_empty())
                            .unwrap_or(default.into())
                    };
                    let (dockerfile, context, build) = build_parts(form);
                    Source::Git {
                        repo,
                        branch: or("branch", "main"),
                        dockerfile,
                        context,
                        source_id: None,
                        auto_deploy: false,
                        build,
                    }
                }
                2 => {
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
        FormKind::EditService {
            service_id,
            source_id,
        } => {
            let source = if !form.get("repo").is_empty() {
                let (dockerfile, context, build) = build_parts(form);
                Some(Source::Git {
                    repo: form.get("repo"),
                    branch: form.get("branch"),
                    dockerfile,
                    context,
                    source_id: source_id.clone(),
                    auto_deploy: source_id.is_none() || form.on("auto_deploy"),
                    build,
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
                    compose_service: form.choice_label("compose_service"),
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
                ssh_key_secret_id: Some(form.get("ssh_key")).filter(|k| !k.is_empty()),
            })
        }
        FormKind::PublicIp => {
            let ip = form.get("ip");
            if form.choice("mode") == 1 && ip.is_empty() {
                return Err("enter the address, or pick auto".into());
            }
            Request::Settings(UpdateSettings {
                public_ip_override: Some(if form.choice("mode") == 1 {
                    ip
                } else {
                    String::new()
                }),
                manage_proxy: None,
            })
        }
        FormKind::PurgeServer { server_id } => Request::PurgeServer {
            id: server_id.clone(),
            volumes: form.on("volumes"),
        },
        FormKind::BackupSchedule { service_id } => Request::Schedule(
            service_id.clone(),
            crate::database::parse_schedule(
                form.on("enabled"),
                &form.get("hour"),
                &form.get("keep"),
                Some(form.get("store")).filter(|s| !s.is_empty()),
            )?,
        ),
        FormKind::AddSecret => {
            let kind = ADDABLE_SECRETS[form.choice("kind")];
            Request::Secret(CreateSecret {
                name: named("name")?,
                value: secret_value(form, kind)?,
            })
        }
        FormKind::GenerateKey => Request::GenerateKey(named("name")?),
        FormKind::InstanceBackup => Request::InstanceBackup(CreateInstanceBackup {
            storage_id: Some(form.get("store")).filter(|s| !s.is_empty()),
            passphrase: passphrase(form)?.ok_or("enter a passphrase")?,
        }),
        FormKind::InstanceSchedule { has_passphrase } => {
            let s = crate::database::parse_schedule(
                form.on("enabled"),
                &form.get("hour"),
                &form.get("keep"),
                Some(form.get("store")).filter(|s| !s.is_empty()),
            )?;
            let passphrase = passphrase(form)?;
            if s.enabled && passphrase.is_none() && !has_passphrase {
                return Err("scheduled runs need a passphrase".into());
            }
            Request::InstanceSchedule(UpdateInstanceBackupSchedule {
                enabled: s.enabled,
                hour_utc: s.hour_utc,
                keep: s.keep,
                storage_id: s.storage_id,
                passphrase,
            })
        }
        FormKind::AddStorage | FormKind::EditStorage { .. } => {
            let endpoint = form.get("endpoint");
            if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                return Err("endpoint must start with https:// or http://".into());
            }
            let bucket = form.get("bucket");
            if bucket.is_empty() {
                return Err("bucket is required".into());
            }
            let credentials = form.get("credentials");
            if credentials.is_empty() {
                return Err("pick s3 credentials (⏎ on credentials)".into());
            }
            let region = form.get("region");
            let id = match &form.kind {
                FormKind::EditStorage { storage_id } => Some(storage_id.clone()),
                _ => None,
            };
            Request::Storage(
                id,
                CreateStorage {
                    name: named("name")?,
                    endpoint: endpoint.trim_end_matches('/').to_string(),
                    region: if region.is_empty() {
                        "us-east-1".into()
                    } else {
                        region
                    },
                    bucket,
                    path_prefix: form.get("prefix").trim_matches('/').to_string(),
                    path_style: form.on("path_style"),
                    credentials_secret_id: credentials,
                },
            )
        }
        FormKind::CreateGithubApp => {
            let organization = (form.choice("owner") == 1)
                .then(|| form.get("organization"))
                .map(|o| o.trim_start_matches('@').to_string());
            if organization.as_ref().is_some_and(String::is_empty) {
                return Err("enter the organization's login".into());
            }
            Request::GithubApp(CreateGithubApp {
                name: named("name")?,
                organization,
                html_url: App::github_url(form),
                api_url: None,
            })
        }
        FormKind::ConnectGithubApp => {
            let app_id = form
                .get("app_id")
                .parse::<u64>()
                .map_err(|_| "the app id is a number, on the app's settings page")?;
            let key = form.raw("private_key");
            if key.trim().is_empty() {
                return Err("the private key is required, a .pem path or pasted".into());
            }
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let installation = form.get("installation");
            let installation_id = if installation.is_empty() {
                None
            } else {
                Some(
                    installation
                        .parse::<u64>()
                        .map_err(|_| "the installation id is a number, or leave it empty")?,
                )
            };
            Request::ConnectApp(ConnectGithubApp {
                name: named("name")?,
                app_id,
                private_key: read_private_key(key.trim(), home.as_deref())?,
                installation_id,
                webhook_secret: Some(form.raw("webhook_secret")).filter(|s| !s.is_empty()),
                html_url: App::github_url(form),
                api_url: None,
            })
        }
        FormKind::AddGithubToken => {
            let token = form.raw("token").trim().to_string();
            if token.is_empty() {
                return Err("paste the token".into());
            }
            Request::Token(ConnectGithubToken {
                name: named("name")?,
                token,
                html_url: App::github_url(form),
                api_url: None,
            })
        }
        FormKind::RenameSecret { secret_id } => Request::UpdateSecret(
            secret_id.clone(),
            UpdateSecret {
                name: Some(named("name")?),
                value: None,
            },
        ),
        FormKind::ReplaceSecret { secret_id, kind } => Request::UpdateSecret(
            secret_id.clone(),
            UpdateSecret {
                name: None,
                value: Some(secret_value(form, *kind)?),
            },
        ),
        FormKind::EnvVar { .. }
        | FormKind::ExposeDatabase { .. }
        | FormKind::DownloadBackup { .. }
        | FormKind::NotifyKind
        | FormKind::NotifyChannel { .. } => {
            return Err("internal: this form submits separately".into());
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_service_sends_the_build_method_and_its_fields() {
        let mut form = Form::new(
            FormKind::NewService {
                environment_id: "e".into(),
            },
            "New service",
            [
                vec![
                    Field::choice("source", "source", vec!["GitHub".into(), "Git URL".into()])
                        .selected(1),
                    Field::text("repo", "repository").value("https://github.com/acme/api"),
                    Field::text("branch", "branch").value("main"),
                ],
                build_fields(None, ""),
                vec![Field::text("name", "name"), Field::text("port", "port")],
            ]
            .concat(),
        );
        form.field_mut("build").unwrap().choice = 1;
        form.field_mut("target").unwrap().value = "runtime".into();
        form.refresh();
        let Ok(Request::Service(body)) = build_request(&form, false) else {
            panic!("a create request");
        };
        let Source::Git {
            dockerfile,
            context,
            build,
            ..
        } = &body.source
        else {
            panic!("a git source");
        };
        assert_eq!((dockerfile.as_str(), context.as_str()), ("Dockerfile", "."));
        assert_eq!(build.method, BuildMethod::Dockerfile);
        assert_eq!(build.target.as_deref(), Some("runtime"));
        assert_eq!(build.compose_file, None);

        form.field_mut("build").unwrap().choice = 2;
        form.refresh();
        let Ok(Request::Service(body)) = build_request(&form, false) else {
            panic!("a create request");
        };
        let Source::Git { build, .. } = &body.source else {
            panic!("a git source");
        };
        assert_eq!(build.method, BuildMethod::Compose);
        assert_eq!(build.target, None, "hidden fields are not sent");
        assert_eq!(
            detected_note(&BuildDetection {
                method: BuildMethod::Railpack,
                file: None
            }),
            "✓ Railpack"
        );
    }

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
    fn private_keys_are_pasted_or_read_from_a_path() {
        let dir = std::env::temp_dir().join(format!("tui-key-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".ssh")).unwrap();
        let key = "-----BEGIN OPENSSH PRIVATE KEY-----
abc
-----END OPENSSH PRIVATE KEY-----
";
        std::fs::write(dir.join(".ssh/id"), key).unwrap();
        assert_eq!(read_private_key(key.trim(), None).unwrap(), key);
        assert_eq!(read_private_key("~/.ssh/id", Some(&dir)).unwrap(), key);
        assert!(read_private_key("~/.ssh/missing", Some(&dir)).is_err());
        std::fs::write(dir.join("plain"), "hello").unwrap();
        assert!(
            read_private_key(&dir.join("plain").display().to_string(), None)
                .unwrap_err()
                .contains("not a private key")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn instance_backup_passphrases_are_checked_before_sending() {
        let form = |p: &str, repeat: &str| {
            let mut f = Form::new(
                FormKind::InstanceBackup,
                "Back up instance",
                vec![
                    Field::pick("store", "store", "", "local"),
                    Field::text("passphrase", "passphrase").masked(),
                    Field::text("repeat", "repeat").masked(),
                ],
            );
            f.focus = 1;
            f.paste(p);
            f.focus = 2;
            f.paste(repeat);
            f
        };
        let err = |f: &Form| build_request(f, false).err().unwrap_or_default();
        assert!(err(&form("short", "short")).contains("at least 12"));
        assert!(err(&form("long enough phrase", "long enough phrasE")).contains("do not match"));
        assert_eq!(err(&form("", "")), "enter a passphrase");
        assert!(matches!(
            build_request(&form("long enough phrase", "long enough phrase"), false),
            Ok(Request::InstanceBackup(CreateInstanceBackup {
                storage_id: None,
                ..
            }))
        ));
        assert_eq!(form("abc", "").fields[1].hint, "3 of 12 characters");
    }

    #[test]
    fn generated_key_names_do_not_collide() {
        let secret = |name: &str| Secret {
            id: name.into(),
            name: name.into(),
            kind: SecretKind::SshKey,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            public_part: None,
            used_by: Vec::new(),
        };
        assert_eq!(unique_name("edge-key", &[]), "edge-key");
        let taken = [secret("edge-key"), secret("edge-key-2")];
        assert_eq!(unique_name("edge-key", &taken), "edge-key-3");
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
