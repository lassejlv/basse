use std::ops::Range;

use client::{Client, Context};
use common::{
    Activity, ActivityLevel, Backup, BackupStatus, BackupTrigger, Deployment, DeploymentStatus,
    Domain, EnvVar, Environment, EnvironmentNode, InstanceSettings, LogLine, LogStream, Project,
    ProjectNode, ProxyStatus, Server, ServerKind, ServerStatus, Service, ServiceDetail, Source,
    Step, StepKind, StepStatus, Timestamp, Tree, Volume,
};
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::Modifier;

use crate::app::{
    App, Confirm, ConfirmAction, DeployView, LogBuffer, Overlay, Palette, Reveal, Screen,
    ServiceView, Tab,
};
use crate::form::{Field, Form, FormKind};
use crate::mouse::{self, HeaderLink, Hit, List};
use crate::{theme, view};

fn server(name: &str) -> Server {
    Server {
        id: name.into(),
        name: name.into(),
        kind: ServerKind::Local,
        host: None,
        user: None,
        port: None,
        ssh_key_secret_id: None,
        status: ServerStatus::Online,
        proxy: ProxyStatus::Running,
        docker_version: None,
        last_checked_at: None,
        created_at: Timestamp::default(),
        checks: Vec::new(),
        problem: None,
    }
}

fn service(name: &str) -> Service {
    Service {
        id: name.into(),
        environment_id: "fixture-env".into(),
        server_id: "fixture-node".into(),
        name: name.into(),
        source: Source::Image {
            image: "example:1".into(),
        },
        port: Some(8080),
        command: None,
        restart: Default::default(),
        health_path: None,
        created_at: Timestamp::default(),
        state: Default::default(),
        primary_domain: None,
        last_deployed_at: None,
        live_deployment_id: None,
        replicas: 1,
        scaling: Default::default(),
    }
}

fn deployment() -> Deployment {
    Deployment {
        id: "fixture-deploy".into(),
        service_id: "fixture-web".into(),
        number: 27,
        status: DeploymentStatus::Building,
        trigger: "fixture-trigger".into(),
        commit: Some("abcdef123456".into()),
        commit_message: Some("fixture commit message".into()),
        image: None,
        container_id: None,
        steps: [StepKind::Clone, StepKind::Build, StepKind::Start]
            .into_iter()
            .map(|kind| Step {
                kind,
                status: StepStatus::Pending,
                detail: None,
                started_at: None,
                finished_at: None,
            })
            .collect(),
        error: None,
        created_at: Timestamp::default(),
        finished_at: None,
    }
}

fn fixture() -> App {
    let client = Client::new("http://127.0.0.1:1", "test").expect("fixture client");
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(client, "fixture-connection".into(), tx);
    let project = Project {
        id: "fixture-project".into(),
        name: "fixture-project".into(),
        description: None,
        created_at: Timestamp::default(),
    };
    let environment = Environment {
        id: "fixture-env".into(),
        project_id: project.id.clone(),
        name: "fixture-env".into(),
        created_at: Timestamp::default(),
    };
    let web = service("fixture-web");
    app.servers = vec![server("fixture-node"), server("fixture-worker")];
    app.details.insert(
        web.id.clone(),
        ServiceDetail {
            service: web.clone(),
            project: project.clone(),
            environment: environment.clone(),
            server: app.servers[0].clone(),
            env: vec![EnvVar {
                key: "FIXTURE_MODE".into(),
                value: "sample".into(),
                secret: false,
            }],
            domains: vec![Domain {
                id: "fixture-domain".into(),
                service_id: web.id.clone(),
                host: "web.example.test".into(),
                port: None,
                tls: true,
                compose_service: None,
                created_at: Timestamp::default(),
            }],
            volumes: vec![Volume {
                id: "fixture-volume".into(),
                service_id: web.id.clone(),
                name: "fixture-data".into(),
                mount_path: "/fixture/data".into(),
                created_at: Timestamp::default(),
                compose: None,
            }],
            container: None,
            compose: Vec::new(),
            replicas: Vec::new(),
            lb: None,
            database: None,
        },
    );
    app.tree = Some(Tree {
        projects: vec![ProjectNode {
            project,
            environments: vec![EnvironmentNode {
                environment,
                services: vec![web, service("fixture-api")],
            }],
        }],
    });
    app.activity = vec![
        Activity {
            id: 1,
            at: Timestamp::default(),
            level: ActivityLevel::Success,
            service_id: Some("fixture-web".into()),
            service_name: None,
            project_id: None,
            message: "built".into(),
            detail: None,
        },
        Activity {
            id: 2,
            at: Timestamp::default(),
            level: ActivityLevel::Info,
            service_id: None,
            service_name: None,
            project_id: None,
            message: "notice".into(),
            detail: None,
        },
    ];
    app
}

fn draw(app: &App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("fixture terminal");
    terminal
        .draw(|frame| view::render(frame, app))
        .expect("fixture frame");
    terminal.backend().buffer().clone()
}

fn label_on(buf: &Buffer, y: u16, label: &str) -> Option<Rect> {
    let width = u16::try_from(label.chars().count()).expect("fixture label width");
    if width > buf.area.width {
        return None;
    }
    (buf.area.left()..=buf.area.right() - width)
        .find(|&x| {
            (x..x + width)
                .zip(label.chars())
                .all(|(col, ch)| buf[(col, y)].symbol().chars().eq([ch]))
        })
        .map(|x| Rect::new(x, y, width, 1))
}

fn label_in(buf: &Buffer, mut rows: Range<u16>, label: &str) -> Rect {
    rows.find_map(|y| label_on(buf, y, label))
        .unwrap_or_else(|| panic!("missing rendered label {label:?}"))
}

fn label(buf: &Buffer, text: &str) -> Rect {
    label_in(buf, buf.area.top()..buf.area.bottom(), text)
}

fn assert_hit(app: &App, rect: Rect, expected: Hit) -> Position {
    for pos in rect.positions() {
        assert_eq!(
            mouse::hit_at(&app.mouse, pos.x, pos.y),
            Some(expected),
            "hit at {pos:?} within {rect:?}",
        );
    }
    Position::new(rect.x, rect.y)
}

fn assert_rows(app: &App, buf: &Buffer, rows: &[(&str, Hit)]) {
    let targets = mouse::targets(&app.mouse);
    for &(text, expected) in rows {
        let rect = label(buf, text);
        assert_hit(app, rect, expected);
        let matching: Vec<_> = targets
            .iter()
            .filter(|(_, target)| *target == expected)
            .map(|(area, _)| (area.y, area.height))
            .collect();
        assert_eq!(matching, [(rect.y, 1)], "one row target for {text:?}");
    }
}

fn event(app: &mut App, kind: MouseEventKind, pos: Position) -> bool {
    mouse::handle(
        app,
        MouseEvent {
            kind,
            column: pos.x,
            row: pos.y,
            modifiers: KeyModifiers::NONE,
        },
    )
}

fn click(app: &mut App, pos: Position) {
    assert!(
        event(app, MouseEventKind::Down(MouseButton::Left), pos),
        "mouse-down at {pos:?}"
    );
    assert!(
        event(app, MouseEventKind::Up(MouseButton::Left), pos),
        "mouse-up at {pos:?}"
    );
}

fn key(code: KeyCode) -> Hit {
    Hit::Key(code, KeyModifiers::NONE)
}

fn char_key(c: char) -> Hit {
    key(KeyCode::Char(c))
}

#[tokio::test]
async fn header_breadcrumbs_open_the_project_environment_service_and_live_run() {
    let mut app = fixture();
    let mut live = deployment();
    live.id = "fixture-live".into();
    live.status = DeploymentStatus::Live;
    live.number = 8;
    let mut stage = app.tree.as_ref().unwrap().projects[0].environments[0].clone();
    stage.environment.id = "fixture-stage".into();
    stage.environment.name = "staging".into();
    app.tree.as_mut().unwrap().projects[0]
        .environments
        .push(stage.clone());
    let detail = app.details.get_mut("fixture-web").unwrap();
    detail.environment = stage.environment;
    detail.service.live_deployment_id = Some(live.id.clone());

    app.open_service("fixture-web", Tab::Overview);
    app.service.as_mut().unwrap().deployments = vec![live.clone(), deployment()];
    let buf = draw(&app, 120, 24);
    let header_y = label(&buf, "fixture-project / staging / fixture-web").y;
    for (text, link) in [
        ("fixture-project", HeaderLink::Project),
        ("staging", HeaderLink::Environment),
        ("fixture-web", HeaderLink::Service),
        ("live #8", HeaderLink::Deployment),
        ("building #27", HeaderLink::ActiveDeployment),
    ] {
        let rect = label_on(&buf, header_y, text)
            .unwrap_or_else(|| panic!("missing header segment {text}"));
        assert_hit(&app, rect, Hit::Header(link));
    }
    let live_link = label_on(&buf, header_y, "live #8").unwrap().as_position();
    click(&mut app, live_link);
    assert_eq!(app.screen, Screen::Deploy);
    assert_eq!(app.deploy.as_ref().unwrap().deployment.id, live.id);

    let buf = draw(&app, 120, 24);
    for (text, link) in [
        ("fixture-project", HeaderLink::Project),
        ("staging", HeaderLink::Environment),
        ("live #8", HeaderLink::Deployment),
    ] {
        assert_hit(
            &app,
            label_on(&buf, header_y, text).unwrap(),
            Hit::Header(link),
        );
    }
    let service_link = assert_hit(
        &app,
        label_on(&buf, header_y, "fixture-web").unwrap(),
        Hit::Header(HeaderLink::Service),
    );
    click(&mut app, service_link);
    assert_eq!(app.screen, Screen::Service);
    assert_eq!(app.service.as_ref().unwrap().tab, Tab::Overview);
    assert!(app.deploy.is_none());

    let buf = draw(&app, 120, 24);
    let env_link = label_on(&buf, header_y, "staging").unwrap().as_position();
    click(&mut app, env_link);
    assert_eq!(app.screen, Screen::Overview);
    assert_eq!(app.current_env().unwrap().1.environment.id, "fixture-stage");
    assert_eq!(app.overview.pane, crate::app::Pane::Services);
    let overview = draw(&app, 120, 24);
    let crumb_y = label(&overview, "fixture-project / staging").y;
    assert_hit(
        &app,
        label_on(&overview, crumb_y, "fixture-project").unwrap(),
        Hit::Header(HeaderLink::Project),
    );
    assert_hit(
        &app,
        label_on(&overview, crumb_y, "staging").unwrap(),
        Hit::Header(HeaderLink::Environment),
    );

    app.open_service("fixture-web", Tab::Overview);
    let buf = draw(&app, 120, 24);
    let project_link = label_on(&buf, header_y, "fixture-project")
        .unwrap()
        .as_position();
    click(&mut app, project_link);
    assert_eq!(app.screen, Screen::Overview);
    assert_eq!(app.current_project().unwrap().project.id, "fixture-project");
    assert_eq!(app.overview.pane, crate::app::Pane::Sidebar);
}

#[tokio::test]
async fn cancel_controls_target_the_running_deployment_and_hide_after_cutover() {
    let mut app = fixture();
    let mut live = deployment();
    live.id = "fixture-live".into();
    live.status = DeploymentStatus::Live;
    let running = deployment();
    app.open_service("fixture-web", Tab::Deployments);
    let mut queued = running.clone();
    queued.id = "newer-queued".into();
    queued.number += 1;
    queued.status = DeploymentStatus::Queued;
    let service = app.service.as_mut().unwrap();
    service.deployments = vec![queued, live, running.clone()];
    service.row = 1;
    let buf = draw(&app, 120, 24);
    let cancel = assert_hit(
        &app,
        label(&buf, "ctrl+x cancel deploy"),
        Hit::Key(KeyCode::Char('x'), KeyModifiers::CONTROL),
    );
    click(&mut app, cancel);
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Confirm(c)) if matches!(&c.action, ConfirmAction::CancelDeploy(id) if *id == running.id)
    ));
    let buf = draw(&app, 120, 24);
    let keep = assert_hit(&app, label(&buf, "esc keep"), key(KeyCode::Esc));
    click(&mut app, keep);
    assert!(app.overlay.is_none());

    app.open_deploy(running);
    let buf = draw(&app, 120, 24);
    let cancel = assert_hit(&app, label(&buf, "c cancel deploy"), char_key('c'));
    click(&mut app, cancel);
    let buf = draw(&app, 120, 24);
    assert_hit(&app, label(&buf, "y cancel deploy"), char_key('y'));
    let keep = assert_hit(&app, label(&buf, "esc keep"), key(KeyCode::Esc));
    click(&mut app, keep);
    app.deploy.as_mut().unwrap().deployment.steps.push(Step {
        kind: StepKind::Switch,
        status: StepStatus::Done,
        detail: None,
        started_at: None,
        finished_at: None,
    });
    let buf = draw(&app, 120, 24);
    assert!(label_on(&buf, buf.area.bottom() - 1, "c cancel deploy").is_none());
}

const OVERVIEW_HINTS: [(&str, KeyCode); 7] = [
    ("⏎ open", KeyCode::Enter),
    ("d deploy", KeyCode::Char('d')),
    ("r restart", KeyCode::Char('r')),
    ("l logs", KeyCode::Char('l')),
    ("n new", KeyCode::Char('n')),
    ("/ filter", KeyCode::Char('/')),
    (": command", KeyCode::Char(':')),
];

#[tokio::test]
async fn key_bar_targets_match_the_visible_labels_and_only_those_cells() {
    let mut app = fixture();
    for (width, visible_left) in [(120, 7), (74, 3)] {
        let buf = draw(&app, width, 24);
        let y = buf.area.bottom() - 1;
        let arrows = label_on(&buf, y, "↑↓ move").expect("visible arrow hint");
        let mut expected = vec![
            (Rect::new(arrows.x, y, 1, 1), key(KeyCode::Up)),
            (
                Rect::new(arrows.x + 1, y, arrows.width - 1, 1),
                key(KeyCode::Down),
            ),
        ];
        for (i, (text, code)) in OVERVIEW_HINTS.into_iter().enumerate() {
            let rect = label_on(&buf, y, text);
            if i < visible_left {
                expected.push((
                    rect.unwrap_or_else(|| panic!("missing key hint {text:?}")),
                    key(code),
                ));
            } else {
                assert_eq!(rect, None, "clipped hint {text:?} at width {width}");
            }
        }
        for (text, c) in [(", settings", ','), ("? help", '?'), ("q quit", 'q')] {
            expected.push((
                label_on(&buf, y, text).unwrap_or_else(|| panic!("missing key hint {text:?}")),
                char_key(c),
            ));
        }
        let mut actual: Vec<_> = mouse::targets(&app.mouse)
            .into_iter()
            .filter(|(rect, _)| rect.y == y)
            .collect();
        actual.sort_by_key(|(rect, _)| rect.x);
        assert_eq!(actual, expected, "key bar at width {width}");
        for x in buf.area.left()..buf.area.right() {
            let pos = Position::new(x, y);
            let hit = expected
                .iter()
                .find(|(rect, _)| rect.contains(pos))
                .map(|(_, hit)| *hit);
            assert_eq!(
                mouse::hit_at(&app.mouse, x, y),
                hit,
                "key bar column {x} at width {width}"
            );
        }
        click(&mut app, Position::new(arrows.x + 1, y));
        assert_eq!(app.overview.row, 1, "down-arrow click");
        click(&mut app, Position::new(arrows.x, y));
        assert_eq!(app.overview.row, 0, "up-arrow click");
    }
}

#[test]
fn overview_rows_match_the_visible_projects_services_and_activity() {
    let app = fixture();
    let buf = draw(&app, 120, 24);
    let project = label(&buf, "▾ fixture-project");
    assert_hit(&app, Rect::new(project.x, project.y, 1, 1), Hit::Fold(0));
    let environment = label_on(&buf, project.y + 1, "fixture-env").expect("sidebar environment");
    assert_hit(&app, environment, Hit::Side(1));
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-api", Hit::Service(1)),
            ("fixture-worker", Hit::SidebarServer(1)),
            ("built", Hit::Activity(0)),
        ],
    );
    let header = label(&buf, "NAME");
    let web = label_on(&buf, header.y + 1, "fixture-web").expect("first service row");
    assert_hit(&app, web, Hit::Service(0));
    let notice = label(&buf, "notice");
    assert!(matches!(
        mouse::hit_at(&app.mouse, notice.x, notice.y),
        Some(Hit::List(List::Activity(_)))
    ));
    assert_hit(&app, label(&buf, "filter: all"), Hit::Filter);
}

#[test]
fn scrolled_overview_only_registers_rendered_service_rows() {
    let mut app = fixture();
    let services =
        &mut app.tree.as_mut().expect("fixture tree").projects[0].environments[0].services;
    *services = (0..10).map(|i| service(&format!("sample-{i}"))).collect();
    app.overview.row = 9;
    let buf = draw(&app, 120, 16);
    let header = label(&buf, "NAME");
    let bottom = (header.y + 1..buf.area.bottom())
        .find(|&y| buf[(header.x, y)].symbol() == "─")
        .expect("service table bottom border");
    let visible: Vec<_> = (0..10)
        .filter_map(|i| {
            (header.y + 1..bottom)
                .find_map(|y| label_on(&buf, y, &format!("sample-{i}")))
                .map(|area| (area, Hit::Service(i)))
        })
        .collect();
    assert!(visible.len() < 10, "the fixture must clip some rows");
    assert_eq!(visible.last().map(|(_, hit)| *hit), Some(Hit::Service(9)));
    for &(area, hit) in &visible {
        assert_hit(&app, area, hit);
    }
    let actual: Vec<_> = mouse::targets(&app.mouse)
        .into_iter()
        .filter(|(_, hit)| matches!(hit, Hit::Service(_)))
        .map(|(area, hit)| (area.y, hit))
        .collect();
    assert_eq!(
        actual,
        visible
            .into_iter()
            .map(|(area, hit)| (area.y, hit))
            .collect::<Vec<_>>()
    );
}

#[test]
fn service_tab_labels_and_list_rows_match_their_targets() {
    let mut app = fixture();
    app.screen = Screen::Service;
    app.service = Some(ServiceView {
        id: "fixture-web".into(),
        tab: Tab::Overview,
        detail: app.details.remove("fixture-web"),
        deployments: vec![deployment()],
        deployment_row: 0,
        logs: LogBuffer::new(),
        log_task: None,
        row: 0,
        reveal: false,
        database: true,
        container: None,
        replica: None,
        scaling_focus: false,
        lb_open: false,
        metrics: Default::default(),
    });
    app.backups.insert(
        "fixture-web".into(),
        vec![Backup {
            id: "fixture-backup".into(),
            service_id: "fixture-web".into(),
            created_at: Timestamp::default(),
            size_bytes: 1024,
            status: BackupStatus::Done,
            trigger: BackupTrigger::Manual,
            file_name: "sample.dump".into(),
            error: None,
            storage: Default::default(),
            storage_id: None,
            finished_at: None,
        }],
    );

    let buf = draw(&app, 120, 24);
    let tab_y = label(&buf, "Overview    Deployments").y;
    for (title, tab) in [
        ("Overview", Tab::Overview),
        ("Deployments", Tab::Deployments),
        ("Logs", Tab::Logs),
        ("Metrics", Tab::Metrics),
        ("Database", Tab::Database),
        ("Backups", Tab::Backups),
        ("Env", Tab::Env),
        ("Domains", Tab::Domains),
        ("Volumes", Tab::Volumes),
        ("Settings", Tab::Settings),
    ] {
        let rect = label_on(&buf, tab_y, title).expect("visible tab label");
        assert_hit(&app, rect, Hit::Tab(tab));
        assert_eq!(
            mouse::hit_at(&app.mouse, rect.right(), tab_y),
            None,
            "gap after {title}"
        );
    }
    assert_rows(&app, &buf, &[("abcdef1", Hit::DeployRow(0))]);

    for (tab, text) in [
        (Tab::Deployments, "fixture commit message"),
        (Tab::Env, "FIXTURE_MODE"),
        (Tab::Domains, "web.example.test"),
        (Tab::Volumes, "fixture-data"),
        (Tab::Backups, "sample.dump"),
    ] {
        app.set_tab(tab);
        let buf = draw(&app, 120, 24);
        let rect = label_in(&buf, tab_y + 1..buf.area.bottom(), text);
        assert_hit(&app, rect, Hit::Row(0));
        assert_eq!(
            mouse::targets(&app.mouse)
                .into_iter()
                .filter(|(_, hit)| matches!(hit, Hit::Row(_)))
                .map(|(area, hit)| (area.y, area.height, hit))
                .collect::<Vec<_>>(),
            [(rect.y, 1, Hit::Row(0))],
            "{tab:?} registers only the visible fixture row"
        );
    }
}

#[test]
fn server_and_settings_rows_exclude_table_headers() {
    let mut app = fixture();
    app.screen = Screen::Servers;
    app.servers[1].proxy = ProxyStatus::Missing;
    let buf = draw(&app, 120, 24);
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-node", Hit::Server(0)),
            ("fixture-worker", Hit::Server(1)),
        ],
    );
    assert_hit(&app, label(&buf, "NAME"), Hit::List(List::Servers));
    assert_hit(&app, label(&buf, "not set up · p"), Hit::SetupProxy(1));

    app.screen = Screen::Settings;
    app.settings.settings = Some(InstanceSettings {
        version: "1.0.0".into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        public_ip_override: None,
        listen: "127.0.0.1:7700".into(),
        data_dir: "/fixture".into(),
        data_dir_size: 0,
        manage_proxy: true,
        proxy: ProxyStatus::Running,
        proxy_image: "example:1".into(),
        docker_version: None,
        ssh_public_key: None,
        reclaimable: None,
        supervised: false,
        domain: None,
        domain_verified: false,
    });
    for name in ["fixture-primary", "fixture-stage"] {
        app.settings.contexts.contexts.insert(
            name.into(),
            Context {
                url: format!("https://{name}.test"),
                token: "test".into(),
            },
        );
    }
    let buf = draw(&app, 120, 24);
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-primary", Hit::Context(0)),
            ("fixture-stage", Hit::Context(1)),
        ],
    );
    let header = label(&buf, "NAME");
    assert_hit(&app, header, Hit::List(List::Contexts));
    assert_hit(
        &app,
        label(&buf, "https://fixture-stage.test"),
        Hit::ContextUrl(1),
    );
}

#[test]
fn deploy_pipeline_click_jumps_to_a_logged_step_and_wheel_scrolls_output() {
    let mut app = fixture();
    app.screen = Screen::Deploy;
    let mut logs = LogBuffer {
        timestamps: false,
        ..LogBuffer::new()
    };
    for i in 0..60 {
        logs.push(LogLine {
            ts: None,
            stream: if i == 7 {
                LogStream::System
            } else {
                LogStream::Stdout
            },
            line: if i == 7 {
                "==> build".into()
            } else {
                format!("fixture line {i:02}")
            },
        });
    }
    app.deploy = Some(DeployView {
        deployment: deployment(),
        logs,
        task: None,
        back: Screen::Service,
        announced: false,
        pipeline_offset: 0,
        moving_to: None,
    });
    let buf = draw(&app, 120, 24);
    let pipe_y = label(&buf, "Pipeline").y;
    let mut build = Position::default();
    for (text, kind) in [
        ("clone", StepKind::Clone),
        ("build", StepKind::Build),
        ("start", StepKind::Start),
    ] {
        let area = label_in(&buf, pipe_y + 1..buf.area.bottom() - 1, text);
        let at = assert_hit(&app, area, Hit::Pipeline(kind));
        if kind == StepKind::Build {
            build = at;
        }
    }
    let tail = assert_hit(&app, label(&buf, "fixture line 59"), Hit::Logs);
    assert!((buf.area.top()..buf.area.bottom()).all(|y| label_on(&buf, y, "==> build").is_none()));
    assert!(
        event(&mut app, MouseEventKind::ScrollUp, tail),
        "wheel over build output"
    );
    assert_eq!(app.deploy.as_ref().expect("deploy view").logs.offset, 3);

    click(&mut app, build);
    let buf = draw(&app, 120, 24);
    assert_hit(&app, label(&buf, "==> build"), Hit::Logs);
    assert!(!app.deploy.as_ref().expect("deploy view").logs.follow);
    assert!(
        (buf.area.top()..buf.area.bottom()).all(|y| label_on(&buf, y, "fixture line 59").is_none())
    );
}

fn form() -> Form {
    Form::new(
        FormKind::NewProject,
        "Fixture form",
        vec![
            Field::text("name", "Project name"),
            Field::choice(
                "mode",
                "Launch mode",
                vec!["Preview".into(), "Production".into()],
            ),
        ],
    )
    .focus_on("name")
}

fn confirmation() -> Confirm {
    Confirm {
        title: "Fixture confirmation".into(),
        message: "Prune unused fixture images?".into(),
        danger: true,
        action: ConfirmAction::Prune,
        verb: "prune",
        busy: None,
        error: Vec::new(),
    }
}

fn assert_modal_targets(app: &App, buf: &Buffer, heading: &str, mut owns: impl FnMut(Hit) -> bool) {
    let title = label(buf, heading);
    let left = (0..title.x)
        .rev()
        .find(|&x| buf[(x, title.y)].symbol() == "╭")
        .expect("dialog left border");
    let right = (title.right()..buf.area.right())
        .find(|&x| buf[(x, title.y)].symbol() == "╮")
        .expect("dialog right border");
    let bottom = (title.y + 1..buf.area.bottom())
        .find(|&y| buf[(left, y)].symbol() == "╰")
        .expect("dialog bottom border");
    let inner = Rect::new(
        left + 1,
        title.y + 1,
        right - left - 1,
        bottom - title.y - 1,
    );
    assert_eq!(
        mouse::modal_bounds(&app.mouse),
        Some(Rect::new(
            left,
            title.y,
            right - left + 1,
            bottom - title.y + 1
        )),
        "{heading} bounds must follow the rendered border",
    );
    let targets = mouse::targets(&app.mouse);
    assert_eq!(
        targets.first(),
        Some(&(buf.area, Hit::Block)),
        "{heading} backdrop"
    );
    for &(area, hit) in &targets[1..] {
        assert!(
            owns(hit),
            "{heading} registered unexpected target {hit:?} at {area:?}"
        );
        assert_eq!(
            area.intersection(inner),
            area,
            "{heading} target {hit:?} escaped the dialog"
        );
    }
    assert_eq!(
        mouse::hit_at(&app.mouse, 0, 0),
        Some(Hit::Block),
        "{heading} outside the dialog"
    );
}

#[test]
fn form_dialog_only_registers_its_controls_and_choice_click_focuses_the_field() {
    let mut app = fixture();
    app.overlay = Some(Overlay::Form(form()));
    let buf = draw(&app, 120, 24);
    assert_modal_targets(&app, &buf, "Fixture form", |hit| {
        matches!(
            hit,
            Hit::FormField(0 | 1) | Hit::FormChoice(1, 0 | 1) | Hit::List(List::Form)
        ) || [
            key(KeyCode::Tab),
            Hit::Key(KeyCode::BackTab, KeyModifiers::SHIFT),
            key(KeyCode::Esc),
            key(KeyCode::Enter),
        ]
        .contains(&hit)
    });
    let pos = assert_hit(&app, label(&buf, "Production"), Hit::FormChoice(1, 1));
    let field = assert_hit(&app, label(&buf, "Project name"), Hit::FormField(0));
    assert_hit(&app, label(&buf, "⏎ save"), key(KeyCode::Enter));
    event(&mut app, MouseEventKind::ScrollDown, field);
    assert!(matches!(&app.overlay, Some(Overlay::Form(form)) if form.focus == 1));
    event(&mut app, MouseEventKind::ScrollUp, field);
    assert!(matches!(&app.overlay, Some(Overlay::Form(form)) if form.focus == 0));
    click(&mut app, pos);
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("choice click closed the form")
    };
    assert_eq!(form.choice("mode"), 1);
    assert_eq!(form.focus, 1);
}

#[tokio::test]
async fn confirm_click_starts_work_and_busy_dialog_only_keeps_the_backdrop() {
    let mut app = fixture();
    app.overlay = Some(Overlay::Confirm(confirmation()));
    let buf = draw(&app, 120, 24);
    assert_modal_targets(&app, &buf, "Fixture confirmation", |hit| {
        [char_key('y'), key(KeyCode::Esc)].contains(&hit)
    });
    let confirm = assert_hit(&app, label(&buf, "y prune"), char_key('y'));
    let keep = assert_hit(&app, label(&buf, "esc keep"), key(KeyCode::Esc));
    click(&mut app, confirm);
    assert!(
        matches!(&app.overlay, Some(Overlay::Confirm(c)) if c.busy.is_some()),
        "click should start pruning"
    );

    let buf = draw(&app, 120, 24);
    assert_modal_targets(&app, &buf, "Fixture confirmation", |_| false);
    label(&buf, "waiting for the daemon");
    assert_eq!(mouse::targets(&app.mouse), [(buf.area, Hit::Block)]);
    click(&mut app, keep);
    assert!(
        matches!(&app.overlay, Some(Overlay::Confirm(c)) if c.busy.is_some()),
        "busy dialog must not dismiss"
    );
}

#[test]
fn reveal_and_help_only_register_visible_dialog_actions() {
    let mut app = fixture();
    app.overlay = Some(Overlay::Reveal(Reveal {
        title: "Fixture reveal".into(),
        subtitle: "one-time result".into(),
        message: "Example value".into(),
        value: "sample".into(),
        note: "Keep it somewhere safe".into(),
        what: "the value",
    }));
    let buf = draw(&app, 120, 24);
    assert_modal_targets(&app, &buf, "Fixture reveal", |hit| {
        [char_key('c'), key(KeyCode::Enter)].contains(&hit)
    });
    assert_hit(&app, label(&buf, "c copy value"), char_key('c'));
    let done = assert_hit(&app, label(&buf, "⏎ done"), key(KeyCode::Enter));
    click(&mut app, done);
    assert!(app.overlay.is_none(), "done should dismiss reveal");

    app.overlay = Some(Overlay::Help);
    let buf = draw(&app, 120, 28);
    assert_modal_targets(&app, &buf, "Keys ·", |hit| matches!(hit, Hit::Key(..)));
    assert_hit(
        &app,
        label(&buf, "ctrl+r refresh"),
        Hit::Key(KeyCode::Char('r'), KeyModifiers::CONTROL),
    );
    let close = assert_hit(&app, label(&buf, "esc close help"), key(KeyCode::Esc));
    click(&mut app, close);
    assert!(
        app.overlay.is_none(),
        "close help should dismiss the overlay"
    );
}

#[test]
fn palette_only_registers_its_visible_controls_and_wheel_stays_inside_it() {
    let mut app = fixture();
    app.overlay = Some(Overlay::Palette(Palette {
        input: "service".into(),
        row: 0,
    }));
    let buf = draw(&app, 120, 24);
    assert_modal_targets(&app, &buf, "Command", |hit| {
        matches!(
            hit,
            Hit::PaletteInput | Hit::PaletteRow(0..=4) | Hit::List(List::Palette)
        ) || [key(KeyCode::Enter), key(KeyCode::Esc)].contains(&hit)
    });
    assert_hit(&app, label(&buf, ": service"), Hit::PaletteInput);
    assert_rows(
        &app,
        &buf,
        &[
            ("deploy service", Hit::PaletteRow(0)),
            ("restart service", Hit::PaletteRow(1)),
            ("stop service", Hit::PaletteRow(2)),
            (" start service", Hit::PaletteRow(3)),
            ("new service", Hit::PaletteRow(4)),
        ],
    );
    assert_hit(&app, label(&buf, "⏎ run"), key(KeyCode::Enter));
    assert_hit(&app, label(&buf, "esc close"), key(KeyCode::Esc));
    let pos = assert_hit(&app, label(&buf, "restart service"), Hit::PaletteRow(1));
    let background = assert_hit(&app, label(&buf, "fixture-api"), Hit::Block);
    assert!(
        event(&mut app, MouseEventKind::ScrollDown, background),
        "wheel on backdrop"
    );
    assert!(
        matches!(&app.overlay, Some(Overlay::Palette(p)) if p.row == 0),
        "outside wheel moved palette"
    );
    assert_eq!(
        app.overview.row, 0,
        "outside wheel moved the dimmed service list"
    );
    assert!(
        event(&mut app, MouseEventKind::ScrollDown, pos),
        "wheel on palette row"
    );
    assert!(
        matches!(&app.overlay, Some(Overlay::Palette(p)) if p.row == 1),
        "inside wheel did not move palette"
    );
}

#[tokio::test]
async fn double_click_opens_the_rendered_service_and_click_switches_tabs() {
    let mut app = fixture();
    let buf = draw(&app, 120, 24);
    let rect = label(&buf, "fixture-api");
    let row = assert_hit(&app, rect, Hit::Service(1));
    assert_eq!(buf[(row.x, row.y)].bg, theme::BG);
    assert!(
        event(&mut app, MouseEventKind::Moved, row),
        "entering a row should repaint"
    );
    assert!(
        !event(
            &mut app,
            MouseEventKind::Moved,
            Position::new(row.x + 1, row.y)
        ),
        "moving within a row should not repaint"
    );
    let buf = draw(&app, 120, 24);
    assert_eq!(
        buf[(row.x, row.y)].bg,
        theme::SURFACE,
        "hover should paint the row"
    );

    click(&mut app, row);
    assert_eq!(app.screen, Screen::Overview, "first click only selects");
    assert_eq!(app.overview.row, 1);
    click(&mut app, row);
    assert_eq!(app.screen, Screen::Service);
    let service = app.service.as_ref().expect("double-click opened a service");
    assert_eq!(service.id, "fixture-api");
    assert_eq!(service.tab, Tab::Overview);

    let buf = draw(&app, 120, 24);
    let tab = assert_hit(&app, label(&buf, "Volumes"), Hit::Tab(Tab::Volumes));
    click(&mut app, tab);
    assert_eq!(
        app.service.as_ref().expect("service view").tab,
        Tab::Volumes
    );
    let buf = draw(&app, 120, 24);
    assert!(
        buf[(tab.x, tab.y)].modifier.contains(Modifier::UNDERLINED),
        "selected tab should be underlined"
    );
}

fn secret(name: &str, kind: common::SecretKind, public: Option<&str>) -> common::Secret {
    common::Secret {
        id: name.into(),
        name: name.into(),
        kind,
        created_at: Timestamp::default(),
        updated_at: Timestamp::default(),
        public_part: public.map(Into::into),
        used_by: Vec::new(),
    }
}

fn secrets_fixture() -> App {
    let mut app = fixture();
    let mut deploy = secret(
        "fixture-key",
        common::SecretKind::SshKey,
        Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFixtureKeyBody fixture@host"),
    );
    deploy.used_by = vec![common::Usage {
        kind: common::UsageKind::Server,
        id: "fixture-node".into(),
        name: "fixture-node".into(),
    }];
    app.secrets.list = vec![
        deploy,
        secret(
            "fixture-creds",
            common::SecretKind::S3Credentials,
            Some("FIXTUREACCESSKEY"),
        ),
    ];
    app.storage.list = vec![common::StorageConnection {
        id: "fixture-store".into(),
        name: "fixture-store".into(),
        endpoint: "http://127.0.0.1:9000".into(),
        region: "us-east-1".into(),
        bucket: "fixture-bucket".into(),
        path_prefix: "prod".into(),
        path_style: true,
        credentials_secret_id: "fixture-creds".into(),
        created_at: Timestamp::default(),
        last_test: Some(common::StorageTest {
            ok: true,
            at: Timestamp::default(),
            error: None,
            latency_ms: Some(12),
        }),
    }];
    app
}

#[test]
fn secrets_rows_detail_actions_and_key_bar_have_targets() {
    let mut app = secrets_fixture();
    app.screen = Screen::Secrets;
    let buf = draw(&app, 160, 40);
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-key", Hit::Item(0)),
            ("fixture-creds", Hit::Item(1)),
        ],
    );
    assert_hit(&app, label(&buf, "PUBLIC PART"), Hit::List(List::Secrets));
    assert_hit(&app, label(&buf, "c copy"), char_key('c'));
    assert_hit(&app, label(&buf, "v reveal"), char_key('v'));
    assert_hit(&app, label(&buf, "••••••••"), char_key('v'));
    let y = buf.area.bottom() - 1;
    for (text, c) in [
        ("a add", 'a'),
        ("g generate key", 'g'),
        ("n rename", 'n'),
        ("e replace value", 'e'),
        ("x delete", 'x'),
    ] {
        let rect = label_on(&buf, y, text).unwrap_or_else(|| panic!("key bar {text:?}"));
        assert_hit(&app, rect, char_key(c));
    }
    let row = label(&buf, "fixture-creds");
    click(&mut app, Position::new(row.x, row.y));
    assert_eq!(app.secrets.row, 1, "click selects the secret");

    app.secrets.revealed = Some((
        common::RevealedSecret {
            secret: app.secrets.list[1].clone(),
            value: common::SecretValue::S3Credentials {
                access_key_id: "FIXTUREACCESSKEY".into(),
                secret_access_key: "fixture-secret-value".into(),
            },
        },
        Timestamp::default(),
    ));
    let buf = draw(&app, 160, 40);
    assert_hit(&app, label(&buf, "fixture-secret-value"), char_key('C'));
    assert_hit(&app, label(&buf, "C copy value"), char_key('C'));
    assert_hit(&app, label(&buf, "v hide"), char_key('v'));
}

#[test]
fn storage_rows_detail_and_settings_links_have_targets() {
    let mut app = secrets_fixture();
    app.screen = Screen::Storage;
    let buf = draw(&app, 160, 40);
    assert_rows(&app, &buf, &[("http://127.0.0.1:9000", Hit::Item(0))]);
    assert_hit(
        &app,
        label(&buf, "BUCKET / PREFIX"),
        Hit::List(List::Storage),
    );
    let test_row = label(&buf, "last test").y;
    let hint = (buf.area.left()..=buf.area.right() - 6)
        .rev()
        .map(|x| Rect::new(x, test_row, 6, 1))
        .find(|r| {
            r.positions()
                .zip("t test".chars())
                .all(|(p, c)| buf[p].symbol().chars().eq([c]))
        })
        .expect("detail test hint at the right edge");
    assert_hit(&app, hint, char_key('t'));
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("t test", char_key('t')),
        ("a add", char_key('a')),
        ("⏎ edit", key(KeyCode::Enter)),
        ("x delete", char_key('x')),
    ] {
        let rect = label_on(&buf, y, text).unwrap_or_else(|| panic!("key bar {text:?}"));
        assert_hit(&app, rect, hit);
    }

    app.screen = Screen::Settings;
    app.settings.settings = Some(InstanceSettings {
        version: "1.0.0".into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        public_ip_override: None,
        listen: "127.0.0.1:7700".into(),
        data_dir: "/fixture".into(),
        data_dir_size: 0,
        manage_proxy: true,
        proxy: ProxyStatus::Running,
        proxy_image: "example:1".into(),
        docker_version: None,
        ssh_public_key: None,
        reclaimable: None,
        supervised: false,
        domain: None,
        domain_verified: false,
    });
    let buf = draw(&app, 160, 40);
    assert_hit(&app, label(&buf, "s open secrets"), char_key('s'));
    assert_hit(&app, label(&buf, "o open storage"), char_key('o'));
}

#[tokio::test]
async fn key_picker_registers_rows_copy_and_choice_inside_the_dialog() {
    let mut app = secrets_fixture();
    app.screen = Screen::Servers;
    let mut server = server("fixture-edge");
    server.kind = ServerKind::Ssh;
    server.ssh_key_secret_id = Some("fixture-key".into());
    app.pick_server_key(&server);
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "SSH key", |hit| {
        matches!(
            hit,
            Hit::PickRow(0..=2) | Hit::List(List::Picker) | Hit::Key(..)
        )
    });
    assert_rows(
        &app,
        &buf,
        &[
            ("daemon's own key", Hit::PickRow(0)),
            ("fixture-key", Hit::PickRow(1)),
            ("+ generate new key", Hit::PickRow(2)),
        ],
    );
    assert_hit(&app, label(&buf, "c copy public key"), char_key('c'));
    assert_hit(&app, label(&buf, "esc cancel"), key(KeyCode::Esc));
    assert_hit(&app, label(&buf, "⏎ use fixture-key"), key(KeyCode::Enter));
    let row = label(&buf, "+ generate new key");
    click(&mut app, Position::new(row.x, row.y));
    assert!(matches!(&app.overlay, Some(Overlay::Picker(p)) if p.row == 2));
    let buf = draw(&app, 160, 40);
    assert_hit(&app, label(&buf, "⏎ generate"), key(KeyCode::Enter));
}

#[tokio::test]
async fn pick_field_click_opens_the_picker_and_a_refused_delete_only_offers_close() {
    let mut app = secrets_fixture();
    app.screen = Screen::Servers;
    app.form_server();
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Add server", |hit| {
        matches!(
            hit,
            Hit::FormField(_) | Hit::List(List::Form) | Hit::Key(..)
        )
    });
    let field = label(&buf, "daemon's own key");
    click(&mut app, Position::new(field.x, field.y));
    assert!(
        matches!(&app.overlay, Some(Overlay::Picker(_))),
        "a click on a pick field opens the picker"
    );
    app.picker_cancel();
    assert!(
        matches!(&app.overlay, Some(Overlay::Form(f)) if f.fields[f.focus].key == "ssh_key"),
        "cancel returns to the form on that field"
    );

    app.screen = Screen::Secrets;
    app.overlay = Some(Overlay::Confirm(Confirm {
        title: "Delete secret · fixture-key".into(),
        message: "Delete fixture-key?".into(),
        danger: true,
        action: ConfirmAction::DeleteSecret("fixture-key".into()),
        verb: "delete",
        busy: None,
        error: vec!["secret is used by server fixture-node".into()],
    }));
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Delete secret", |hit| hit == key(KeyCode::Esc));
    assert!((buf.area.top()..buf.area.bottom()).all(|y| label_on(&buf, y, "y delete").is_none()));
    let close = assert_hit(&app, label(&buf, "esc close"), key(KeyCode::Esc));
    click(&mut app, close);
    assert!(app.overlay.is_none());
}

fn instance_backup(
    id: &str,
    status: BackupStatus,
    storage_id: Option<&str>,
) -> common::InstanceBackup {
    common::InstanceBackup {
        id: id.into(),
        created_at: Timestamp::default(),
        status,
        trigger: BackupTrigger::Manual,
        file_name: format!("instance-{id}.tar.gz.enc"),
        size_bytes: if status == BackupStatus::Done {
            4096
        } else {
            0
        },
        storage: Default::default(),
        storage_id: storage_id.map(Into::into),
        error: (status == BackupStatus::Failed).then(|| "fixture upload failed".into()),
        finished_at: None,
    }
}

#[test]
fn instance_backups_panel_rows_schedule_and_focus_follow_clicks() {
    let mut app = secrets_fixture();
    app.screen = Screen::Settings;
    app.settings.settings = Some(InstanceSettings {
        version: "1.0.0".into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        public_ip_override: None,
        listen: "127.0.0.1:7700".into(),
        data_dir: "/fixture".into(),
        data_dir_size: 0,
        manage_proxy: true,
        proxy: ProxyStatus::Running,
        proxy_image: "example:1".into(),
        docker_version: None,
        ssh_public_key: None,
        reclaimable: None,
        supervised: false,
        domain: None,
        domain_verified: false,
    });
    app.settings.contexts.contexts.insert(
        "fixture-primary".into(),
        Context {
            url: "https://fixture-primary.test".into(),
            token: "test".into(),
        },
    );
    app.settings.backups = vec![
        instance_backup("one", BackupStatus::Done, Some("fixture-store")),
        instance_backup("two", BackupStatus::Failed, None),
    ];
    app.settings.schedule = Some(common::InstanceBackupSchedule {
        enabled: true,
        has_passphrase: true,
        storage_id: Some("fixture-store".into()),
        ..Default::default()
    });
    let buf = draw(&app, 160, 40);
    assert_rows(
        &app,
        &buf,
        &[
            ("instance-one.tar.gz.enc", Hit::Item(0)),
            ("fixture upload failed", Hit::Item(1)),
        ],
    );
    assert_hit(
        &app,
        label(&buf, "TRIGGER"),
        Hit::List(List::InstanceBackups),
    );
    assert_hit(&app, label(&buf, "daily 02:00 UTC"), char_key('e'));
    assert_hit(&app, label(&buf, "e edit"), char_key('e'));
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("tab contexts", key(KeyCode::Tab)),
        ("b back up now", char_key('b')),
        ("e schedule", char_key('e')),
        ("D download", char_key('D')),
        ("x delete", char_key('x')),
        ("s secrets", char_key('s')),
        ("o storage", char_key('o')),
    ] {
        let rect = label_on(&buf, y, text).unwrap_or_else(|| panic!("key bar {text:?}"));
        assert_hit(&app, rect, hit);
    }
    let row = label(&buf, "fixture upload failed");
    click(&mut app, Position::new(row.x, row.y));
    assert_eq!(app.settings.backup_row, 1);
    let context = label(&buf, "fixture-primary");
    click(&mut app, Position::new(context.x, context.y));
    assert_eq!(app.settings.focus, crate::app::SettingsFocus::Contexts);
    let buf = draw(&app, 160, 40);
    let y = buf.area.bottom() - 1;
    assert_hit(
        &app,
        label_on(&buf, y, "tab backups").expect("contexts key bar"),
        key(KeyCode::Tab),
    );
    click(&mut app, Position::new(row.x, row.y));
    assert_eq!(app.settings.focus, crate::app::SettingsFocus::Backups);
}

#[test]
fn instance_backup_dialog_masks_the_passphrase_and_registers_its_controls() {
    let mut app = secrets_fixture();
    app.screen = Screen::Settings;
    app.form_instance_backup();
    if let Some(Overlay::Form(form)) = &mut app.overlay {
        form.paste("correct horse battery");
    }
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Back up instance", |hit| {
        matches!(
            hit,
            Hit::FormField(_) | Hit::List(List::Form) | Hit::Key(..)
        )
    });
    assert!(
        (buf.area.top()..buf.area.bottom()).all(|y| label_on(&buf, y, "correct horse").is_none())
    );
    label(&buf, "•••••••••••••••••••••");
    label(&buf, "✓ 21 characters");
    assert_hit(&app, label(&buf, "⏎ back up"), key(KeyCode::Enter));
    label(
        &buf,
        &format!(
            "{} restore <file|s3://bucket/key> --passphrase-file <file>",
            common::brand::DAEMON_BIN
        ),
    );
    let store = label(&buf, "local");
    assert!(matches!(
        mouse::hit_at(&app.mouse, store.x, store.y),
        Some(Hit::FormField(0))
    ));
}

fn git_source(name: &str, status: common::GitSourceStatus) -> common::GitSource {
    common::GitSource {
        id: name.into(),
        name: name.into(),
        kind: common::GitSourceKind::GithubApp,
        html_url: "https://github.com".into(),
        api_url: "https://api.github.com".into(),
        owner: Some("fixture-org".into()),
        owner_kind: Some(common::GitOwnerKind::Organization),
        app_id: Some(4242),
        app_slug: Some(format!("{name}-app")),
        installation_id: (status == common::GitSourceStatus::Ready).then_some(77),
        status,
        status_detail: None,
        install_url: Some(format!(
            "https://github.com/apps/{name}-app/installations/new"
        )),
        webhook_url: Some(format!("https://deploy.example.test/github/{name}/webhook")),
        secret_id: None,
        last_checked_at: None,
        created_at: Timestamp::default(),
        used_by: Vec::new(),
    }
}

fn sources_fixture() -> App {
    let mut app = fixture();
    app.screen = Screen::Sources;
    app.sources.loaded = true;
    app.sources.list = vec![
        git_source("fixture-gh", common::GitSourceStatus::Ready),
        git_source("fixture-wait", common::GitSourceStatus::NeedsInstall),
    ];
    app
}

#[test]
fn sources_rows_detail_and_key_bar_have_targets() {
    let mut app = sources_fixture();
    let buf = draw(&app, 160, 40);
    assert_rows(
        &app,
        &buf,
        &[
            ("installation 77", Hit::Item(0)),
            ("not installed", Hit::Item(1)),
        ],
    );
    assert_hit(&app, label(&buf, "USED BY"), Hit::List(List::Sources));
    assert_hit(&app, label(&buf, "c check"), char_key('c'));
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("n new GitHub App", char_key('n')),
        ("a connect app", char_key('a')),
        ("t add token", char_key('t')),
        ("x delete", char_key('x')),
    ] {
        let rect = label_on(&buf, y, text).unwrap_or_else(|| panic!("key bar {text:?}"));
        assert_hit(&app, rect, hit);
    }
    assert!(label_on(&buf, y, "⏎ finish setup").is_none());

    let row = label(&buf, "not installed");
    click(&mut app, Position::new(row.x, row.y));
    assert_eq!(app.sources.row, 1);
    let buf = draw(&app, 160, 40);
    let finish = label_on(&buf, y, "⏎ finish setup").expect("finish setup hint");
    click(&mut app, Position::new(finish.x, finish.y));
    assert!(matches!(app.overlay, Some(Overlay::GithubSetup(_))));

    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "New GitHub App", |hit| {
        matches!(hit, Hit::Key(..))
    });
    label(
        &buf,
        "https://github.com/apps/fixture-wait-app/installations/new",
    );
    assert_hit(&app, label(&buf, "c copy url"), char_key('c'));
    assert_hit(&app, label(&buf, "o open browser"), char_key('o'));
    label(&buf, "waiting for GitHub");
    let close = assert_hit(&app, label(&buf, "esc close"), key(KeyCode::Esc));
    click(&mut app, close);
    assert!(app.overlay.is_none());
}

#[test]
fn empty_sources_offer_the_three_ways_in_and_an_unverified_domain_blocks_the_app() {
    let mut app = sources_fixture();
    app.sources.list.clear();
    app.settings.settings = Some(InstanceSettings {
        version: "1.0.0".into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        public_ip_override: None,
        listen: "127.0.0.1:7700".into(),
        data_dir: "/fixture".into(),
        data_dir_size: 0,
        manage_proxy: true,
        proxy: ProxyStatus::Running,
        proxy_image: "example:1".into(),
        docker_version: None,
        ssh_public_key: None,
        reclaimable: None,
        supervised: false,
        domain: Some("deploy.example.test".into()),
        domain_verified: false,
    });
    let buf = draw(&app, 160, 40);
    label(&buf, "No git sources yet.");
    for (text, c) in [
        ("n   create a GitHub App", 'n'),
        ("a   connect an existing GitHub App", 'a'),
        ("t   add a personal access token", 't'),
    ] {
        assert_hit(&app, label(&buf, text), char_key(c));
    }
    let create = label(&buf, "n   create a GitHub App");
    click(&mut app, Position::new(create.x, create.y));
    assert!(matches!(
        app.overlay,
        Some(Overlay::DomainNeeded("New GitHub App"))
    ));

    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "New GitHub App", |hit| {
        matches!(hit, Hit::Key(..))
    });
    label(&buf, "deploy.example.test");
    assert_hit(
        &app,
        label(&buf, "⏎ open Settings › Domain"),
        key(KeyCode::Enter),
    );
    let token = assert_hit(&app, label(&buf, "t add a token instead"), char_key('t'));
    click(&mut app, token);
    assert!(matches!(&app.overlay, Some(Overlay::Form(f)) if f.title == "Add token"));
}

fn domain_fixture(verified: bool) -> App {
    use common::{CheckStatus, DnsRecord, DomainCheck, DomainCheckKind, InstanceDomain};
    let mut app = fixture();
    app.screen = Screen::Domain;
    let record = |kind: &str, value: &str| DnsRecord {
        kind: kind.into(),
        name: "deploy.example.test".into(),
        value: value.into(),
    };
    let checks = DomainCheckKind::ALL
        .into_iter()
        .enumerate()
        .map(|(i, kind)| DomainCheck {
            kind,
            status: match (verified, i) {
                (true, _) => CheckStatus::Pass,
                (false, 0) => CheckStatus::Fail,
                (false, _) => CheckStatus::Skipped,
            },
            detail: if i == 0 && !verified {
                "resolves to 198.51.100.9".into()
            } else {
                "fixture detail".into()
            },
            hint: (i == 0 && !verified).then(|| "Point the A record at 203.0.113.10.".into()),
        })
        .collect();
    app.domain.data = Some(InstanceDomain {
        domain: Some("deploy.example.test".into()),
        url: verified.then(|| "https://deploy.example.test".into()),
        verified,
        verified_at: None,
        public_ipv4: Some("203.0.113.10".into()),
        public_ipv6: Some("2001:db8::10".into()),
        records: vec![record("A", "203.0.113.10"), record("AAAA", "2001:db8::10")],
        checks,
        checked_at: None,
        cloudflare: None,
    });
    app
}

#[test]
fn domain_guide_shows_cloudflare_and_the_flexible_mode() {
    use common::CloudflareProxy;
    for verified in [false, true] {
        let mut app = domain_fixture(verified);
        if let Some(d) = app.domain.data.as_mut() {
            d.cloudflare = Some(CloudflareProxy {
                records: vec!["AAAA".into()],
                direct: vec!["A".into()],
                flexible: !verified,
            });
        }
        let buf = draw(&app, 160, 40);
        label(&buf, "AAAA through Cloudflare");
        label(&buf, "A points straight here");
        label(&buf, "from CF-Connecting-IP");
        if verified {
            label(&buf, "Full (strict)");
        } else {
            label(&buf, "✕ Flexible");
            label(&buf, "Cloudflare proxies AAAA. Keep these values");
            assert_hit(&app, label(&buf, "AAAA"), Hit::Record(1));
        }
    }
}

#[test]
fn domain_guide_records_copy_and_keys_switch_to_the_input() {
    let mut app = domain_fixture(false);
    let buf = draw(&app, 160, 40);
    let header = label(&buf, "TYPE").y;
    let a = Rect::new(label(&buf, "TYPE").x, header + 1, 4, 1);
    assert_hit(&app, a, Hit::Record(0));
    let aaaa = assert_hit(&app, label(&buf, "AAAA"), Hit::Record(1));
    label(&buf, "Point the A record at 203.0.113.10.");
    label(&buf, "resolves to 198.51.100.9");
    click(&mut app, aaaa);
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|t| t.text == "copied the AAAA record value")
    );
    let y = buf.area.bottom() - 1;
    assert!(label_on(&buf, y, "c copy url").is_none());
    for (text, c) in [
        ("r check again", 'r'),
        ("e edit domain", 'e'),
        ("x remove domain", 'x'),
    ] {
        assert_hit(&app, label_on(&buf, y, text).expect(text), char_key(c));
    }
    let edit = label_on(&buf, y, "e edit domain").expect("edit hint");
    click(&mut app, Position::new(edit.x, edit.y));
    assert_eq!(app.domain.input.as_deref(), Some("deploy.example.test"));

    app.toast = None;
    let buf = draw(&app, 160, 40);
    label(&buf, "› domain");
    let save = assert_hit(
        &app,
        label_on(&buf, y, "⏎ save & check").expect("save hint"),
        key(KeyCode::Enter),
    );
    let back = assert_hit(
        &app,
        label_on(&buf, y, "esc back").expect("back hint"),
        key(KeyCode::Esc),
    );
    assert!(save.x < back.x);
    click(&mut app, back);
    assert_eq!(
        app.domain.input, None,
        "esc leaves the input, not the guide"
    );
    assert_eq!(app.screen, Screen::Domain);

    let app = domain_fixture(true);
    let buf = draw(&app, 160, 40);
    label(&buf, "https://deploy.example.test");
    assert_hit(&app, label(&buf, "c copy"), char_key('c'));
    assert_hit(
        &app,
        label_on(&buf, y, "c copy url").expect("copy url hint"),
        char_key('c'),
    );
}

#[tokio::test]
async fn settings_link_to_the_domain_guide_and_sources() {
    let mut app = sources_fixture();
    app.screen = Screen::Settings;
    app.settings.settings = Some(InstanceSettings {
        version: "1.0.0".into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        public_ip_override: None,
        listen: "127.0.0.1:7700".into(),
        data_dir: "/fixture".into(),
        data_dir_size: 0,
        manage_proxy: true,
        proxy: ProxyStatus::Running,
        proxy_image: "example:1".into(),
        docker_version: None,
        ssh_public_key: None,
        reclaimable: None,
        supervised: false,
        domain: Some("deploy.example.test".into()),
        domain_verified: true,
    });
    let buf = draw(&app, 160, 40);
    assert_hit(&app, label(&buf, "deploy.example.test"), char_key('d'));
    assert_hit(&app, label(&buf, "d guide"), char_key('d'));
    label(&buf, "1 ready · 1 need attention");
    let sources = assert_hit(&app, label(&buf, "g open sources"), char_key('g'));
    click(&mut app, sources);
    assert_eq!(app.screen, Screen::Sources);
}

fn repo(name: &str) -> common::GitRepo {
    common::GitRepo {
        full_name: format!("fixture-org/{name}"),
        private: name != "docs",
        default_branch: "main".into(),
        description: None,
        pushed_at: None,
    }
}

#[tokio::test]
async fn github_pickers_page_search_and_fill_the_new_service_form() {
    let mut app = sources_fixture();
    app.screen = Screen::Overview;
    app.form_new_service();
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Form(f)) if f.fields[f.focus].key == "gh_repo" && f.raw("git_source") == "fixture-gh"
    ));
    crate::keys::handle(&mut app, crossterm::event::KeyEvent::from(KeyCode::Enter));
    let names = ["api", "web", "ledger", "docs", "worker"];
    app.apply(crate::app::Msg::Repos {
        source: "fixture-gh".into(),
        query: String::new(),
        page: 1,
        result: Ok(common::RepoPage {
            repos: names.iter().map(|n| repo(n)).collect(),
            page: 1,
            per_page: 10,
            total: 23,
        }),
    });
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Repository", |hit| {
        matches!(
            hit,
            Hit::PickRow(0..=4) | Hit::List(List::Picker) | Hit::Key(..)
        )
    });
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-org/api", Hit::PickRow(0)),
            ("fixture-org/ledger", Hit::PickRow(2)),
        ],
    );
    label(&buf, "page 1 of 3 · 23 repositories · newest push first");
    let page = label(&buf, "←→ page");
    assert_hit(&app, Rect { width: 1, ..page }, key(KeyCode::Left));
    let next = assert_hit(
        &app,
        Rect {
            x: page.x + 1,
            width: page.width - 1,
            ..page
        },
        key(KeyCode::Right),
    );
    assert_hit(&app, label(&buf, "esc back"), key(KeyCode::Esc));
    assert_hit(
        &app,
        label(&buf, "⏎ use fixture-org/api"),
        key(KeyCode::Enter),
    );
    click(&mut app, next);
    assert!(
        matches!(&app.overlay, Some(Overlay::Picker(p)) if p.git.as_ref().is_some_and(|g| g.page == 2 && g.loading))
    );

    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::from(KeyCode::Char('l')),
    );
    assert!(
        matches!(&app.overlay, Some(Overlay::Picker(p)) if p.git.as_ref().is_some_and(|g| g.search == "l" && g.page == 1 && g.typed_at.is_some()))
    );

    let ledger = label(&buf, "fixture-org/ledger");
    click(&mut app, Position::new(ledger.x, ledger.y));
    click(&mut app, Position::new(ledger.x, ledger.y));
    assert!(matches!(&app.overlay, Some(Overlay::Picker(p)) if p.title == "Branch"));
    app.apply(crate::app::Msg::Branches {
        source: "fixture-gh".into(),
        repo: "fixture-org/ledger".into(),
        result: Ok(vec![
            common::GitBranch {
                name: "develop".into(),
                default: false,
            },
            common::GitBranch {
                name: "main".into(),
                default: true,
            },
        ]),
    });
    let buf = draw(&app, 160, 40);
    assert_rows(
        &app,
        &buf,
        &[("main", Hit::PickRow(0)), ("develop", Hit::PickRow(1))],
    );
    label(&buf, "default");
    label(&buf, "2 branches");
    let develop = label(&buf, "develop");
    click(&mut app, Position::new(develop.x, develop.y));
    let buf = draw(&app, 160, 40);
    let accept = assert_hit(&app, label(&buf, "⏎ use develop"), key(KeyCode::Enter));
    click(&mut app, accept);
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("the form comes back after the branch");
    };
    assert_eq!(form.raw("gh_repo"), "fixture-org/ledger");
    assert_eq!(form.raw("gh_branch"), "develop");
    assert_eq!(form.get("name"), "ledger");
    assert_eq!(form.fields[form.focus].key, "gh_root");
    assert!(form.on("auto_deploy"));
    assert_eq!(
        form.fields
            .iter()
            .find(|f| f.key == "auto_deploy")
            .map(|f| f.hint.as_str()),
        Some("every push to develop")
    );
}

fn compose_fixture() -> App {
    let mut app = fixture();
    let detail = app.details.get_mut("fixture-web").expect("fixture detail");
    detail.service.source = Source::Git {
        repo: "fixture-org/shop".into(),
        branch: "main".into(),
        dockerfile: "Dockerfile".into(),
        context: ".".into(),
        root: String::new(),
        watch_paths: true,
        source_id: None,
        auto_deploy: false,
        build: common::Build {
            method: common::BuildMethod::Compose,
            target: None,
            compose_file: Some("compose.yaml".into()),
        },
    };
    let container = |state: &str, exit: Option<i64>| common::ContainerInfo {
        id: "fixture-container".into(),
        name: "fixture-container".into(),
        image: "example:1".into(),
        state: state.into(),
        started_at: None,
        restart_count: 0,
        exit_code: exit,
        ports: Vec::new(),
        networks: Vec::new(),
    };
    detail.compose = vec![
        common::ComposeService {
            name: "frontend".into(),
            container: Some(container("running", None)),
        },
        common::ComposeService {
            name: "queue".into(),
            container: Some(container("running", None)),
        },
        common::ComposeService {
            name: "migrate".into(),
            container: Some(container("exited", Some(0))),
        },
    ];
    app
}

#[tokio::test]
async fn compose_containers_follow_logs_and_domains_ask_for_the_service() {
    let mut app = compose_fixture();
    app.open_service("fixture-web", Tab::Overview);
    let buf = draw(&app, 160, 40);
    label(&buf, "compose  compose.yaml · 3 services");
    assert_rows(
        &app,
        &buf,
        &[
            ("frontend", Hit::Container(0)),
            ("queue", Hit::Container(1)),
            ("migrate", Hit::Container(2)),
        ],
    );
    label(&buf, "exited 0");
    let queue = label(&buf, "queue");
    click(&mut app, Position::new(queue.x, queue.y));
    let view = app.service.as_ref().expect("service view");
    assert_eq!(view.tab, Tab::Logs);
    assert_eq!(view.log_container(), Some("queue"));

    let buf = draw(&app, 160, 40);
    label(&buf, "Logs · queue");
    let y = buf.area.bottom() - 1;
    let next = assert_hit(
        &app,
        label_on(&buf, y, "c container").expect("container hint"),
        char_key('c'),
    );
    click(&mut app, next);
    assert_eq!(
        app.service.as_ref().and_then(|v| v.log_container()),
        Some("migrate")
    );

    app.set_tab(Tab::Domains);
    app.form_domain();
    let buf = draw(&app, 160, 40);
    label(&buf, "fixture-web · compose");
    label(&buf, "in the compose file");
    let queue = label(&buf, "queue");
    assert!(matches!(
        mouse::hit_at(&app.mouse, queue.x + 1, queue.y),
        Some(Hit::FormChoice(1, 1))
    ));
    click(&mut app, Position::new(queue.x + 1, queue.y));
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("add domain form");
    };
    assert_eq!(
        form.choice_label("compose_service").as_deref(),
        Some("queue")
    );
}

#[tokio::test]
async fn detected_build_picks_the_method_and_fills_its_file() {
    let mut app = sources_fixture();
    app.screen = Screen::Overview;
    app.form_new_service();
    if let Some(Overlay::Form(form)) = &mut app.overlay {
        for (key, value) in [("gh_repo", "fixture-org/shop"), ("gh_branch", "main")] {
            form.field_mut(key).expect("pick field").value = value.into();
        }
    }
    app.apply(crate::app::Msg::Detected {
        source: "fixture-gh".into(),
        repo: "fixture-org/shop".into(),
        branch: "main".into(),
        root: String::new(),
        result: Ok(common::BuildDetection {
            method: common::BuildMethod::Compose,
            file: Some("docker-compose.yml".into()),
        }),
    });
    let buf = draw(&app, 160, 40);
    label(&buf, "✓ docker-compose.yml");
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("new service form");
    };
    assert_eq!(form.choice_label("build").as_deref(), Some("Compose"));
    assert_eq!(form.get("compose_file"), "docker-compose.yml");
    assert_eq!(form.get("dockerfile"), "", "hidden for compose");
    let chips = label(&buf, "Railpack");
    assert!(matches!(
        mouse::hit_at(&app.mouse, chips.x, chips.y),
        Some(Hit::FormChoice(_, 3))
    ));
}

fn notify_channel(name: &str, enabled: bool) -> common::notify::NotifyChannel {
    use common::notify::*;
    NotifyChannel {
        id: format!("{name}-id"),
        name: name.into(),
        kind: ChannelKind::Webhook,
        enabled,
        config: ChannelConfig::Webhook {
            url: "https://hooks.example.test/…".into(),
            secret: None,
            headers: Vec::new(),
        },
        target: "hooks.example.test/…".into(),
        created_at: Timestamp::default(),
        last_test: Some(ChannelTest {
            ok: true,
            at: Timestamp::default(),
            error: None,
            latency_ms: Some(42),
        }),
    }
}

fn notify_fixture() -> App {
    use common::notify::*;
    let mut app = fixture();
    app.screen = Screen::Notifications;
    app.notify.channels = vec![
        notify_channel("fixture-hook", true),
        notify_channel("fixture-quiet", false),
    ];
    app.notify.rules = vec![NotifyRule {
        id: "fixture-rule".into(),
        channel_id: "fixture-hook-id".into(),
        scope: RuleScope::Project {
            id: "fixture-project".into(),
        },
        scope_label: "fixture-project".into(),
        preset: Some("deploys".into()),
        events: preset("deploys").unwrap().events(),
        enabled: true,
        dedupe_secs: 300,
        max_per_hour: 30,
        created_at: Timestamp::default(),
    }];
    app.notify.deliveries = vec![
        NotifyDelivery {
            id: 2,
            at: Timestamp::default(),
            channel_id: "fixture-hook-id".into(),
            channel_name: "fixture-hook".into(),
            rule_id: Some("fixture-rule".into()),
            event: Some(NotifyEvent::DeployFailed),
            subject: "fixture-project/fixture-env/fixture-web #27".into(),
            title: "fixture-web failed to deploy".into(),
            status: DeliveryStatus::Failed,
            attempts: 3,
            error: Some("fixture refused".into()),
            latency_ms: None,
        },
        NotifyDelivery {
            id: 1,
            at: Timestamp::default(),
            channel_id: "fixture-hook-id".into(),
            channel_name: "fixture-hook".into(),
            rule_id: None,
            event: None,
            subject: "instance".into(),
            title: "Test message".into(),
            status: DeliveryStatus::Sent,
            attempts: 1,
            error: None,
            latency_ms: Some(42),
        },
    ];
    app
}

#[test]
fn notifications_tabs_rows_detail_and_key_bar_have_targets() {
    use crate::notify::Section;
    let mut app = notify_fixture();
    let buf = draw(&app, 160, 40);
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-hook", Hit::Item(0)),
            ("fixture-quiet", Hit::Item(1)),
        ],
    );
    assert_hit(&app, label(&buf, "LAST TEST"), Hit::List(List::Notify));
    let rules_tab = assert_hit(
        &app,
        label(&buf, "Rules"),
        Hit::NotifySection(Section::Rules),
    );
    assert_hit(
        &app,
        label(&buf, "Delivery log"),
        Hit::NotifySection(Section::Deliveries),
    );
    assert_hit(&app, label(&buf, "e disable"), char_key('e'));
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("tab section", key(KeyCode::Tab)),
        ("a add", char_key('a')),
        ("⏎ edit", key(KeyCode::Enter)),
        ("t test", char_key('t')),
        ("x delete", char_key('x')),
    ] {
        let rect = label_on(&buf, y, text).unwrap_or_else(|| panic!("key bar {text:?}"));
        assert_hit(&app, rect, hit);
    }
    let quiet = label(&buf, "fixture-quiet");
    click(&mut app, Position::new(quiet.x, quiet.y));
    assert_eq!(app.notify.channel_row, 1);
    let buf = draw(&app, 160, 40);
    assert_hit(&app, label(&buf, "e enable"), char_key('e'));

    click(&mut app, rules_tab);
    assert_eq!(app.notify.section, Section::Rules);
    let buf = draw(&app, 160, 40);
    assert_rows(&app, &buf, &[("dedupe 5m · 30/h", Hit::Item(0))]);
    assert_hit(&app, label(&buf, "LIMITS"), Hit::List(List::Notify));

    app.notify.section = Section::Deliveries;
    let buf = draw(&app, 160, 40);
    assert_rows(
        &app,
        &buf,
        &[
            ("deploy.failed", Hit::Item(0)),
            ("test message", Hit::Item(1)),
        ],
    );
    let failures = assert_hit(&app, label(&buf, "f failures only"), char_key('f'));
    assert_hit(&app, label(&buf, "r refresh"), char_key('r'));
    click(&mut app, failures);
    assert!(app.notify.failures_only);
    assert_eq!(app.notify.deliveries().len(), 1);
}

#[tokio::test]
async fn settings_key_bar_links_to_notifications() {
    let mut app = fixture();
    app.screen = Screen::Settings;
    app.settings.settings = Some(InstanceSettings {
        version: "1.0.0".into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        public_ip_override: None,
        listen: "127.0.0.1:7700".into(),
        data_dir: "/fixture".into(),
        data_dir_size: 0,
        manage_proxy: true,
        proxy: ProxyStatus::Running,
        proxy_image: "example:1".into(),
        docker_version: None,
        ssh_public_key: None,
        reclaimable: None,
        supervised: false,
        domain: None,
        domain_verified: false,
    });
    let buf = draw(&app, 160, 40);
    let link = assert_hit(&app, label(&buf, "n notifications"), char_key('n'));
    click(&mut app, link);
    assert_eq!(app.screen, Screen::Notifications);
}

#[test]
fn rule_dialog_registers_its_controls_and_clicks_pick_presets_events_and_scope() {
    use crate::notify::RuleFocus;
    use common::notify::{NotifyEvent, RuleScope};
    let mut app = notify_fixture();
    app.rule_dialog(false);
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Add rule", |hit| {
        matches!(
            hit,
            Hit::RuleField(_)
                | Hit::RuleChoice(..)
                | Hit::Preset(0..=4)
                | Hit::NotifyEvent(_)
                | Hit::List(List::Rule)
                | Hit::Key(..)
        )
    });
    assert_hit(&app, label(&buf, "⏎ add rule"), key(KeyCode::Enter));
    assert_hit(&app, label(&buf, "esc cancel"), key(KeyCode::Esc));
    let deploys = assert_hit(
        &app,
        label(&buf, "the whole deploy lifecycle"),
        Hit::Preset(1),
    );
    click(&mut app, deploys);
    let event = label(&buf, "Deploy succeeded");
    assert_hit(&app, event, Hit::NotifyEvent(NotifyEvent::DeploySucceeded));
    click(&mut app, Position::new(event.x, event.y));
    let chip = assert_hit(
        &app,
        label(&buf, "10/h"),
        Hit::RuleChoice(RuleFocus::Rate, 1),
    );
    click(&mut app, chip);
    {
        let Some(Overlay::Rule(d)) = &app.overlay else {
            panic!("rule dialog closed")
        };
        assert_eq!(d.preset, Some("deploys"));
        assert!(!d.events.contains(&NotifyEvent::DeploySucceeded));
        assert!(d.events.contains(&NotifyEvent::DeployFailed));
        assert_eq!(d.rate, 10);
        assert_eq!(d.focus, RuleFocus::Rate);
    }

    let scope = assert_hit(
        &app,
        label(&buf, "only events inside it"),
        Hit::RuleField(RuleFocus::Scope),
    );
    click(&mut app, scope);
    assert!(matches!(&app.overlay, Some(Overlay::Picker(_))));
    let buf = draw(&app, 160, 40);
    let project = assert_hit(
        &app,
        label(&buf, "fixture-project/fixture-env"),
        Hit::PickRow(2),
    );
    click(&mut app, project);
    click(&mut app, project);
    let Some(Overlay::Rule(d)) = &app.overlay else {
        panic!("picker did not return to the rule dialog")
    };
    assert_eq!(
        d.scope,
        RuleScope::Environment {
            id: "fixture-env".into()
        }
    );
    assert_eq!(d.scope_label, "fixture-project/fixture-env");
}

#[test]
fn channel_wizard_steps_register_only_their_controls() {
    use crate::form::FormKind;
    use common::notify::{ChannelKind, DeliveryStatus, NotifyDelivery};
    let mut app = notify_fixture();
    app.form_notify_kind();
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Add channel", |hit| {
        matches!(
            hit,
            Hit::FormField(0..=3) | Hit::FormChoice(0..=3, _) | Hit::List(List::Form)
        ) || [
            key(KeyCode::Tab),
            Hit::Key(KeyCode::BackTab, KeyModifiers::SHIFT),
            key(KeyCode::Esc),
            key(KeyCode::Enter),
        ]
        .contains(&hit)
    });
    let ntfy = assert_hit(&app, label(&buf, "ntfy"), Hit::FormChoice(2, 0));
    click(&mut app, ntfy);
    let next = assert_hit(&app, label(&buf, "⏎ next"), key(KeyCode::Enter));
    click(&mut app, next);
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("expected the settings step")
    };
    assert!(matches!(
        form.kind,
        FormKind::NotifyChannel {
            kind: ChannelKind::Ntfy,
            channel_id: None,
            ..
        }
    ));
    let buf = draw(&app, 160, 40);
    assert_hit(&app, label(&buf, "⏎ next: test"), key(KeyCode::Enter));
    label(&buf, "2 settings");

    let Some(Overlay::Form(form)) = app.overlay.take() else {
        unreachable!()
    };
    app.overlay = Some(Overlay::ChannelTest(crate::notify::ChannelTestStep {
        channel: notify_channel("fixture-hook", false),
        form: Box::new(form),
        created: true,
        result: Some(Ok(NotifyDelivery {
            id: 3,
            at: Timestamp::default(),
            channel_id: "fixture-hook-id".into(),
            channel_name: "fixture-hook".into(),
            rule_id: None,
            event: None,
            subject: "instance".into(),
            title: "Test message".into(),
            status: DeliveryStatus::Sent,
            attempts: 1,
            error: None,
            latency_ms: Some(42),
        })),
        saving: false,
    }));
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Add channel", |hit| matches!(hit, Hit::Key(..)));
    assert_hit(&app, label(&buf, "⏎ save"), key(KeyCode::Enter));
    assert_hit(&app, label(&buf, "t send again"), char_key('t'));
    assert_hit(&app, label(&buf, "esc cancel"), key(KeyCode::Esc));
    let back = assert_hit(
        &app,
        label(&buf, "⇧tab back"),
        Hit::Key(KeyCode::BackTab, KeyModifiers::SHIFT),
    );
    click(&mut app, back);
    assert!(matches!(&app.overlay, Some(Overlay::Form(_))));
}

fn remote(name: &str, status: ServerStatus) -> Server {
    Server {
        kind: ServerKind::Ssh,
        host: Some(format!("{name}.example.test")),
        user: Some("root".into()),
        status,
        ..server(name)
    }
}

#[tokio::test]
async fn server_field_picks_a_server_and_skips_offline_ones() {
    let mut app = fixture();
    app.screen = Screen::Overview;
    app.servers = vec![
        remote("fixture-down", ServerStatus::Offline),
        server("fixture-node"),
        remote("fixture-edge", ServerStatus::Online),
    ];
    app.form_new_service();
    let buf = draw(&app, 160, 40);
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("the new service form is open");
    };
    assert_eq!(
        form.raw("server"),
        "fixture-node",
        "defaults to the local server"
    );
    let server_field = form.fields.iter().position(|f| f.key == "server").unwrap();
    let pick = assert_hit(&app, label(&buf, "⏎ pick"), Hit::FormField(server_field));
    click(&mut app, pick);
    assert!(
        matches!(&app.overlay, Some(Overlay::Picker(p)) if p.title == "Server"),
        "a click on the server field opens the server picker"
    );

    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Server", |hit| {
        matches!(
            hit,
            Hit::PickRow(0..=2) | Hit::List(List::Picker) | Hit::Key(..)
        )
    });
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-node", Hit::PickRow(0)),
            ("fixture-edge", Hit::PickRow(1)),
            ("fixture-down", Hit::PickRow(2)),
        ],
    );
    label(&buf, "fixture-node  default");
    label(&buf, "ssh · root@fixture-edge.example.test");
    label(
        &buf,
        "fixture-down is offline, so it can't take new services.",
    );
    assert_hit(&app, label(&buf, "⏎ use fixture-node"), key(KeyCode::Enter));

    let down = label(&buf, "fixture-down");
    click(&mut app, Position::new(down.x, down.y));
    assert!(
        matches!(&app.overlay, Some(Overlay::Picker(p)) if p.row == 0),
        "an offline server can't be selected with the mouse"
    );
    crate::keys::handle(&mut app, crossterm::event::KeyEvent::from(KeyCode::Down));
    crate::keys::handle(&mut app, crossterm::event::KeyEvent::from(KeyCode::Down));
    assert!(
        matches!(&app.overlay, Some(Overlay::Picker(p)) if p.row == 1),
        "↓ stops before the offline server"
    );

    let buf = draw(&app, 160, 40);
    let accept = assert_hit(&app, label(&buf, "⏎ use fixture-edge"), key(KeyCode::Enter));
    click(&mut app, accept);
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("the form comes back after the pick");
    };
    let field = form.fields.iter().find(|f| f.key == "server").unwrap();
    assert_eq!(
        (
            field.value.as_str(),
            field.shown.as_str(),
            field.hint.as_str()
        ),
        ("fixture-edge", "fixture-edge", "✓ connected")
    );
    let buf = draw(&app, 160, 40);
    label(&buf, "fixture-edge  ssh · root@");
}

#[test]
fn one_server_hides_the_field_and_says_where_it_goes() {
    let mut app = fixture();
    app.screen = Screen::Overview;
    app.servers = vec![server("fixture-node")];
    app.form_new_service();
    let buf = draw(&app, 160, 40);
    label(&buf, "on fixture-node");
    assert!(
        (buf.area.top()..buf.area.bottom()).all(|y| label_on(&buf, y, "local · ").is_none()),
        "no server field with a single server"
    );
}

fn daemon_info(version: &str) -> common::DaemonInfo {
    common::DaemonInfo {
        version: version.into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        cpu_percent: 0.0,
        mem_used: 0,
        mem_total: 0,
        disk_used: 0,
        disk_total: 0,
        cpu_history: Vec::new(),
        proxy: ProxyStatus::Running,
        ssh_public_key: None,
        url: None,
    }
}

fn top_line(app: &App) -> String {
    let buf = draw(app, 200, 24);
    (0..buf.area.width).map(|x| buf[(x, 0)].symbol()).collect()
}

#[test]
fn top_bar_shows_the_version_skew_until_the_cli_is_replaced() {
    let mut app = fixture();
    let ours = env!("CARGO_PKG_VERSION");
    app.info = Some(daemon_info(ours));
    assert!(top_line(&app).contains(&format!("{} {ours}", common::brand::DAEMON_BIN)));
    assert!(!top_line(&app).contains('≠'));

    app.info = Some(daemon_info("99.0.0"));
    let hint = common::version_skew(ours, "99.0.0").expect("versions differ");
    assert!(top_line(&app).contains(&hint), "{}", top_line(&app));

    app.cli_updated = Some("99.0.0".into());
    assert!(!top_line(&app).contains('≠'));
}

#[test]
fn settings_update_covers_the_cli_and_the_daemon() {
    let mut app = fixture();
    app.screen = Screen::Settings;
    let ours = env!("CARGO_PKG_VERSION");
    app.settings.settings = Some(InstanceSettings {
        version: ours.into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        public_ip_override: None,
        listen: "127.0.0.1:7700".into(),
        data_dir: "/fixture".into(),
        data_dir_size: 0,
        manage_proxy: true,
        proxy: ProxyStatus::Running,
        proxy_image: "example:1".into(),
        docker_version: None,
        ssh_public_key: None,
        reclaimable: None,
        supervised: true,
        domain: None,
        domain_verified: false,
    });
    let update = |app: &mut App, daemon_behind: bool| {
        app.overlay = None;
        app.settings.release = Some(common::VersionCheck {
            current: ours.into(),
            latest: Some("v99.0.0".into()),
            update_available: daemon_behind,
        });
        crate::keys::handle(app, crossterm::event::KeyEvent::from(KeyCode::Char('u')));
        match &app.overlay {
            Some(Overlay::Confirm(Confirm {
                action: ConfirmAction::Update { cli, server, .. },
                message,
                ..
            })) => (*cli, *server, message.clone()),
            _ => panic!("u did not ask to update"),
        }
    };

    let (cli, server, message) = update(&mut app, true);
    assert!(cli && server, "{message}");
    assert!(
        message.contains(common::brand::CLI_BIN) && message.contains(common::brand::DAEMON_BIN)
    );

    let (cli, server, _) = update(&mut app, false);
    assert!(cli && !server);

    app.cli_updated = Some("99.0.0".into());
    let (cli, server, _) = update(&mut app, true);
    assert!(!cli && server);

    app.overlay = None;
    app.settings.release = Some(common::VersionCheck {
        current: ours.into(),
        latest: Some(format!("v{ours}")),
        update_available: false,
    });
    app.cli_updated = None;
    crate::keys::handle(
        &mut app,
        crossterm::event::KeyEvent::from(KeyCode::Char('u')),
    );
    assert!(app.overlay.is_none());
    assert!(
        app.toast
            .as_ref()
            .is_some_and(|t| t.text.contains("up to date"))
    );
}

fn onboarding(
    status: common::onboarding::OnboardingStatus,
    states: [common::onboarding::StepState; 5],
) -> common::onboarding::Onboarding {
    use common::onboarding::{Step as SetupStep, StepStatus as SetupStatus};
    common::onboarding::Onboarding {
        status,
        current: SetupStep::Welcome,
        steps: SetupStep::TASKS
            .into_iter()
            .zip(states)
            .map(|(step, state)| SetupStatus { step, state })
            .collect(),
        checklist_dismissed: false,
        automatic: false,
    }
}

fn press(app: &mut App, code: KeyCode) {
    crate::keys::handle(
        app,
        crossterm::event::KeyEvent::new(code, KeyModifiers::NONE),
    );
}

#[tokio::test]
async fn root_dir_picker_walks_folders_and_fills_the_form() {
    let mut app = sources_fixture();
    app.screen = Screen::Overview;
    app.form_new_service();
    if let Some(Overlay::Form(form)) = &mut app.overlay {
        for (key, value) in [("gh_repo", "fixture-org/mono"), ("gh_branch", "main")] {
            form.field_mut(key).expect("pick field").value = value.into();
        }
    }
    let buf = draw(&app, 160, 40);
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("new service form");
    };
    let root = form.fields.iter().position(|f| f.key == "gh_root").unwrap();
    assert!(
        !form
            .fields
            .iter()
            .any(|f| f.key == "watch_paths" && f.visible),
        "watch paths only show with a root"
    );
    let field = assert_hit(
        &app,
        label(&buf, "folder in the repo"),
        Hit::FormField(root),
    );
    click(&mut app, field);
    assert!(
        matches!(&app.overlay, Some(Overlay::Picker(p)) if p.title == "Root directory"),
        "a click on the root dir field opens the folder picker"
    );
    let dirs = |app: &mut App, path: &str, names: &[&str]| {
        app.apply(crate::app::Msg::Dirs {
            path: path.into(),
            result: Ok(names.iter().map(|n| n.to_string()).collect()),
        })
    };
    dirs(&mut app, "", &["apps", "docs"]);
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Root directory", |hit| {
        matches!(
            hit,
            Hit::PickRow(0..=2) | Hit::List(List::Picker) | Hit::Key(..)
        )
    });
    assert_rows(
        &app,
        &buf,
        &[
            ("use /", Hit::PickRow(0)),
            ("apps/", Hit::PickRow(1)),
            ("docs/", Hit::PickRow(2)),
        ],
    );
    label(&buf, "fixture-org/mono@main · /");
    let apps = label(&buf, "apps/");
    click(&mut app, Position::new(apps.x, apps.y));
    let buf = draw(&app, 160, 40);
    let open = assert_hit(&app, label(&buf, "⏎ open"), key(KeyCode::Enter));
    click(&mut app, open);
    dirs(&mut app, "", &["stale"]);
    dirs(&mut app, "apps", &["api", "web"]);
    let buf = draw(&app, 160, 40);
    assert_rows(
        &app,
        &buf,
        &[
            ("use apps/", Hit::PickRow(0)),
            ("../", Hit::PickRow(1)),
            ("api/", Hit::PickRow(2)),
            ("web/", Hit::PickRow(3)),
        ],
    );
    label(&buf, "up to the repository root");
    let api = label(&buf, "api/");
    click(&mut app, Position::new(api.x, api.y));
    click(&mut app, Position::new(api.x, api.y));
    dirs(&mut app, "apps/api", &[]);
    let buf = draw(&app, 160, 40);
    let accept = assert_hit(&app, label(&buf, "⏎ use apps/api/"), key(KeyCode::Enter));
    click(&mut app, accept);

    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("the form comes back with the folder");
    };
    assert_eq!(form.raw("gh_root"), "apps/api");
    let watch = form.fields.iter().find(|f| f.key == "watch_paths").unwrap();
    assert!(watch.visible && watch.on);
    assert_eq!(watch.hint, "pushes to apps/api");
    let buf = draw(&app, 160, 40);
    label(&buf, "apps/api");
}

#[tokio::test]
async fn setup_opens_once_on_connect_and_esc_hands_over_to_the_checklist() {
    use common::onboarding::{OnboardingStatus, Step as SetupStep, StepState};
    let mut app = fixture();
    app.onboarding_loaded(onboarding(OnboardingStatus::Pending, [StepState::Todo; 5]));
    assert_eq!(
        app.screen,
        Screen::Setup,
        "a pending instance opens the flow"
    );
    assert_eq!(app.setup.step, SetupStep::Welcome);

    press(&mut app, KeyCode::Enter);
    assert_eq!(app.setup.step, SetupStep::Domain, "⏎ starts");
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.screen, Screen::Overview, "esc leaves for later");

    app.onboarding_loaded(onboarding(OnboardingStatus::Pending, [StepState::Todo; 5]));
    assert_eq!(app.screen, Screen::Overview, "only once per connection");

    let mut states = [StepState::Todo; 5];
    states[1] = StepState::Skipped;
    states[4] = StepState::Done;
    app.onboarding_loaded(onboarding(OnboardingStatus::Dismissed, states));
    let buf = draw(&app, 160, 40);
    assert!(label_on(&buf, label(&buf, "Setup 1/5").y, "Setup 1/5").is_some());
    assert_rows(
        &app,
        &buf,
        &[
            ("Instance domain", Hit::SetupStep(SetupStep::Domain)),
            ("Backups", Hit::SetupStep(SetupStep::Backups)),
            ("First app", Hit::SetupStep(SetupStep::FirstApp)),
        ],
    );
    assert_hit(&app, label(&buf, "o open"), char_key('o'));
    assert_hit(&app, label(&buf, "X dismiss"), char_key('X'));

    let backups = label(&buf, "Backups");
    click(&mut app, Position::new(backups.x, backups.y));
    assert_eq!(app.screen, Screen::Setup, "a checklist row opens the flow");
    assert_eq!(app.setup.step, SetupStep::Backups);
}

#[tokio::test]
async fn setup_rail_choices_and_key_bar_have_targets() {
    use common::onboarding::{OnboardingStatus, Step as SetupStep, StepState};
    let mut app = fixture();
    app.onboarding_loaded(onboarding(OnboardingStatus::Pending, [StepState::Todo; 5]));
    app.go_to_step(SetupStep::Source);
    let buf = draw(&app, 160, 40);
    let rail: Vec<(&str, Hit)> = SetupStep::ALL
        .into_iter()
        .map(|s| (s.label(), Hit::SetupStep(s)))
        .collect();
    for (text, hit) in &rail {
        let rect = label_in(&buf, 3..20, text);
        assert_hit(&app, rect, *hit);
    }
    for (i, text) in ["GitHub App", "Existing GitHub App", "Personal access token"]
        .into_iter()
        .enumerate()
    {
        let rect = label_in(&buf, 4..12, text);
        assert_hit(&app, rect, Hit::SetupChoice(i));
    }
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("⏎ connect", key(KeyCode::Enter)),
        ("s skip", char_key('s')),
        ("esc later", key(KeyCode::Esc)),
        ("q quit", char_key('q')),
    ] {
        assert_hit(
            &app,
            label_on(&buf, y, text).unwrap_or_else(|| panic!("missing key hint {text:?}")),
            hit,
        );
    }

    let token = label_in(&buf, 4..12, "Personal access token");
    click(&mut app, Position::new(token.x, token.y));
    assert_eq!(app.setup.choice, 2, "a click selects the choice");
    press(&mut app, KeyCode::Up);
    assert_eq!(app.setup.choice, 1, "arrows move between choices");
    press(&mut app, KeyCode::Tab);
    assert_eq!(
        app.setup.step,
        SetupStep::Backups,
        "tab moves to the next step"
    );

    let buf = draw(&app, 160, 40);
    let rail_row = label_in(&buf, 3..20, "Git source");
    click(&mut app, Position::new(rail_row.x, rail_row.y));
    assert_eq!(
        app.setup.step,
        SetupStep::Source,
        "a rail click goes to the step"
    );
    let buf = draw(&app, 160, 40);
    let token = label_in(&buf, 4..12, "Personal access token");
    click(&mut app, Position::new(token.x, token.y));
    click(&mut app, Position::new(token.x, token.y));
    assert!(
        matches!(&app.overlay, Some(Overlay::Form(f)) if f.kind == FormKind::AddGithubToken),
        "a double-click connects with the existing token form"
    );
}

#[tokio::test]
async fn first_app_offers_the_sample_and_skipping_moves_on() {
    use common::onboarding::{OnboardingStatus, Step as SetupStep, StepState};
    let mut app = fixture();
    app.info = Some(common::DaemonInfo {
        version: "0.5.0".into(),
        hostname: "fixture-host".into(),
        public_ip: Some("5.75.12.4".into()),
        cpu_percent: 0.0,
        mem_used: 0,
        mem_total: 0,
        disk_used: 0,
        disk_total: 0,
        cpu_history: Vec::new(),
        proxy: ProxyStatus::Running,
        ssh_public_key: None,
        url: None,
    });
    app.onboarding_loaded(onboarding(OnboardingStatus::Pending, [StepState::Todo; 5]));
    app.go_to_step(SetupStep::FirstApp);
    let buf = draw(&app, 160, 40);
    assert_hit(&app, label(&buf, "Sample app"), Hit::SetupChoice(0));
    label(&buf, "whoami-5-75-12-4.sslip.io");
    assert!(label_on(&buf, buf.area.bottom() - 1, "⏎ create and deploy").is_some());
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(app.setup.step, SetupStep::Done);
    let buf = draw(&app, 160, 40);
    label(&buf, "You're set.");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.screen, Screen::Overview, "⏎ on Done finishes");
}

fn metrics_data(range: common::metrics::MetricsRange) -> common::metrics::ServiceMetrics {
    use common::metrics::{DeployMarker, HttpPoint, MetricPoint};
    let to = Timestamp::default() + chrono::TimeDelta::hours(1);
    let from = Timestamp::default();
    let points = (0..360)
        .map(|i| MetricPoint {
            at: from + chrono::TimeDelta::seconds(i * 10),
            cpu: 5.0 + (i % 7) as f64,
            cpu_max: 12.0 + (i % 7) as f64,
            mem: 200 << 20,
            mem_max: 260 << 20,
            mem_limit: 512 << 20,
            net_rx: 1_200_000.0,
            net_tx: 340_000.0,
            disk_read: 0.0,
            disk_write: 24_000.0,
            pids: 9,
            restarts: 0,
            uptime_secs: 600,
            http: Some(HttpPoint {
                rps: 42.0,
                s2xx: 400,
                s3xx: 0,
                s4xx: 15,
                s5xx: 5,
                p50_ms: Some(18.0),
                p95_ms: Some(120.0),
            }),
            replicas: 1,
        })
        .collect();
    common::metrics::ServiceMetrics {
        range,
        step_secs: range.step_secs(),
        from,
        to,
        containers: vec!["frontend".into(), "queue".into()],
        replicas: Vec::new(),
        replica: None,
        container: None,
        points,
        deploys: vec![DeployMarker {
            at: from + chrono::TimeDelta::minutes(18),
            number: 47,
            status: DeploymentStatus::Superseded,
        }],
        http: true,
    }
}

/// Braille cells below the toolbar, where only charts draw them.
fn braille_cells(buf: &Buffer) -> usize {
    let toolbar = label(buf, "1 1h").y;
    buf.content()
        .iter()
        .enumerate()
        .filter(|(i, _)| *i / buf.area.width as usize > toolbar as usize)
        .map(|(_, c)| c)
        .filter(|c| {
            c.symbol()
                .chars()
                .next()
                .is_some_and(|ch| ('\u{2801}'..='\u{28FF}').contains(&ch))
        })
        .count()
}

#[tokio::test]
async fn metrics_tab_switches_ranges_and_containers_by_click_and_key() {
    use common::metrics::MetricsRange;
    let mut app = compose_fixture();
    app.details
        .get_mut("fixture-web")
        .expect("fixture detail")
        .service
        .state = common::ServiceState::Running;
    app.open_service("fixture-web", Tab::Metrics);
    app.service.as_mut().expect("service view").metrics.data =
        Some(metrics_data(MetricsRange::Hour));
    let buf = draw(&app, 164, 40);
    for text in [
        "CPU  7% now · peak 18% · avg 8%",
        "Memory  200 MiB now · peak 260 MiB · limit 512 MiB",
        "Network  rx 1.2 MB/s · tx 340 kB/s",
        "Disk I/O  read 0 B/s · write 24 kB/s",
        "Requests  42 req/s · 2xx 95% · 4xx 4% · 5xx 1%",
        "Latency  p50 18 ms · p95 120 ms",
        "● live · every 10s",
        "-30m",
        "#47",
        "512M",
        "limit",
    ] {
        label(&buf, text);
    }
    assert!(braille_cells(&buf) > 200, "charts draw braille lines");
    assert!(
        buf.content().iter().any(|c| c.symbol() == "┊"),
        "deploys are marked"
    );

    let toolbar = label(&buf, "1 1h").y;
    for (text, range) in [
        ("1 1h", MetricsRange::Hour),
        ("2 24h", MetricsRange::Day),
        ("3 7d", MetricsRange::Week),
        ("4 30d", MetricsRange::Month),
    ] {
        assert_hit(
            &app,
            label_on(&buf, toolbar, text).expect("range"),
            Hit::MetricsRange(range),
        );
    }
    for (i, text) in ["all", "frontend", "queue", "migrate"]
        .into_iter()
        .enumerate()
    {
        assert_hit(
            &app,
            label_on(&buf, toolbar, text).expect("container"),
            Hit::MetricsContainer(i),
        );
    }
    assert_hit(
        &app,
        label_on(&buf, toolbar, "c").expect("c"),
        char_key('c'),
    );
    let y = buf.area.bottom() - 1;
    assert_hit(
        &app,
        label_on(&buf, y, "c container").expect("key bar"),
        char_key('c'),
    );
    label_on(&buf, y, "←→ range").expect("range hint");

    let week = label_on(&buf, toolbar, "3 7d").expect("range");
    click(&mut app, Position::new(week.x + 2, week.y));
    let metrics = |app: &App| {
        let m = &app.service.as_ref().expect("service view").metrics;
        (m.range, m.container.clone())
    };
    assert_eq!(metrics(&app), (MetricsRange::Week, None));
    let queue = label_on(&buf, toolbar, "queue").expect("queue");
    click(&mut app, Position::new(queue.x, queue.y));
    assert_eq!(metrics(&app), (MetricsRange::Week, Some("queue".into())));

    press(&mut app, KeyCode::Char('1'));
    assert_eq!(metrics(&app).0, MetricsRange::Hour);
    assert_eq!(
        app.service.as_ref().map(|v| v.tab),
        Some(Tab::Metrics),
        "digits pick ranges here"
    );
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    assert_eq!(metrics(&app).0, MetricsRange::Week);
    press(&mut app, KeyCode::Left);
    assert_eq!(metrics(&app).0, MetricsRange::Day);
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(metrics(&app).1.as_deref(), Some("migrate"));
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(metrics(&app).1, None, "c wraps around to all containers");

    let buf = draw(&app, 164, 40);
    let tab_y = label(&buf, "Overview    Deployments").y;
    let env = label_on(&buf, tab_y, "Env").expect("env tab");
    click(&mut app, Position::new(env.x, env.y));
    assert_eq!(app.service.as_ref().map(|v| v.tab), Some(Tab::Env));
}

#[tokio::test]
async fn metrics_tab_says_it_is_collecting_and_where_requests_come_from() {
    use common::metrics::MetricsRange;
    let mut app = fixture();
    app.details
        .get_mut("fixture-web")
        .expect("fixture detail")
        .service
        .state = common::ServiceState::Running;
    app.open_service("fixture-web", Tab::Metrics);
    let buf = draw(&app, 164, 40);
    let collecting = (0..buf.area.height)
        .filter(|&y| label_on(&buf, y, "collecting · first samples in ~10s").is_some())
        .count();
    assert_eq!(collecting, 3, "three rows of two panels each");
    assert_eq!(braille_cells(&buf), 0);
    assert!(label_on(&buf, label(&buf, "1 1h").y, "container").is_none());

    let mut empty = metrics_data(MetricsRange::Hour);
    empty.points.clear();
    empty.deploys.clear();
    empty.http = false;
    app.service.as_mut().expect("service view").metrics.data = Some(empty);
    let buf = draw(&app, 164, 40);
    label(
        &buf,
        "no domain · requests appear once a domain routes here",
    );
    label(&buf, "CPU");

    let mut quiet = metrics_data(MetricsRange::Hour);
    for p in &mut quiet.points {
        p.http = None;
    }
    app.service.as_mut().expect("service view").metrics.data = Some(quiet);
    let buf = draw(&app, 164, 40);
    label(&buf, "no requests in this range");
    label(&buf, "CPU  7% now");
}

fn failing(name: &str) -> Server {
    use common::{CheckStatus, PreflightCheck, ServerProblem};
    let check = |label: &str, status| PreflightCheck {
        label: label.into(),
        status,
        detail: None,
    };
    Server {
        checks: vec![
            check("ssh root@fixture:22", CheckStatus::Pass),
            check("docker installed", CheckStatus::Fail),
            check("docker running", CheckStatus::Skipped),
        ],
        problem: Some(ServerProblem {
            summary: "docker isn't installed on fixture".into(),
            fix: Some("install it with: curl -fsSL https://get.docker.com | sh".into()),
            detail: "docker isn't installed on fixture".into(),
            can_install_docker: true,
        }),
        ..remote(name, ServerStatus::Offline)
    }
}

#[tokio::test]
async fn a_failing_server_opens_its_checklist_and_offers_the_fix() {
    let mut app = fixture();
    app.screen = Screen::Servers;
    app.servers = vec![server("fixture-node"), failing("fixture-edge")];
    app.server_row = 1;
    let buf = draw(&app, 160, 40);
    let line = label(&buf, "docker isn't installed on fixture");
    let details = label_on(&buf, line.y, "⏎ details").expect("details hint beside the problem");
    let details = assert_hit(&app, details, key(KeyCode::Enter));
    click(&mut app, details);
    assert!(
        matches!(&app.overlay, Some(Overlay::ServerDetail(d)) if d.server.name == "fixture-edge")
    );

    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Server · fixture-edge", |hit| {
        matches!(hit, Hit::Key(..))
    });
    label(&buf, "✓ ssh root@fixture:22");
    label(&buf, "✕ docker installed");
    label(
        &buf,
        "fix  install it with: curl -fsSL https://get.docker.com | sh",
    );
    assert_hit(&app, label(&buf, "c check again"), char_key('c'));
    assert_hit(&app, label(&buf, "y copy fix"), char_key('y'));
    assert_hit(&app, label(&buf, "esc close"), key(KeyCode::Esc));
    let install = assert_hit(&app, label(&buf, "i install docker"), char_key('i'));
    click(&mut app, install);
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Confirm(c)) if matches!(&c.action, ConfirmAction::InstallDocker(id) if id == "fixture-edge")
    ));
}

#[tokio::test]
async fn a_cut_error_opens_in_full_with_e_or_a_click() {
    let mut app = fixture();
    let long = format!("deploy api failed: {}", "unauthorized ".repeat(20));
    app.error(long.clone());
    let buf = draw(&app, 120, 30);
    let hint = assert_hit(&app, label(&buf, "e details"), char_key('e'));
    click(&mut app, hint);
    assert!(matches!(&app.overlay, Some(Overlay::ErrorDetail(e)) if e.text == long));
    let buf = draw(&app, 120, 30);
    assert_modal_targets(&app, &buf, "Error", |hit| matches!(hit, Hit::Key(..)));
    label(&buf, "✕ deploy api failed");
    assert_hit(&app, label(&buf, "y copy"), char_key('y'));
    assert_hit(&app, label(&buf, "esc close"), key(KeyCode::Esc));

    app.overlay = None;
    app.error("short");
    draw(&app, 120, 30);
    assert!(!app.expand_error(), "an error that fits is not expandable");
}

#[tokio::test]
async fn settings_move_opens_the_form_picker_confirm_and_live_view() {
    let mut app = fixture();
    app.servers[1].kind = ServerKind::Ssh;
    app.servers[1].host = Some("203.0.113.7".into());
    app.open_service("fixture-web", Tab::Settings);
    let buf = draw(&app, 120, 30);
    let row = assert_hit(
        &app,
        label(&buf, "m  move to another server"),
        char_key('m'),
    );
    assert_hit(&app, label(&buf, "m move"), char_key('m'));

    click(&mut app, row);
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("move form");
    };
    assert!(matches!(&form.kind, FormKind::MoveService { from, .. } if from == "fixture-node"));
    assert_eq!(
        form.raw("server"),
        "fixture-worker",
        "the other server by default"
    );
    assert!(form.on("copy_volumes"), "the service has a volume");
    let buf = draw(&app, 120, 30);
    label(&buf, "from fixture-node");
    label(
        &buf,
        "fixture-node keeps serving until the new server passes its health check.",
    );
    label(
        &buf,
        "Point web.example.test at the new server once it is live.",
    );

    press(&mut app, KeyCode::Enter);
    let Some(Overlay::Picker(picker)) = &app.overlay else {
        panic!("server picker");
    };
    assert_eq!(picker.subtitle, "move fixture-web from fixture-node");
    let current = picker
        .items
        .iter()
        .find(|i| i.name == "fixture-node")
        .unwrap();
    assert!(current.disabled);
    assert_eq!(current.badge, "current");
    assert_eq!(picker.items[picker.row].name, "fixture-worker");
    let buf = draw(&app, 120, 30);
    label(&buf, "fixture-web runs here");

    press(&mut app, KeyCode::Enter);
    assert!(matches!(&app.overlay, Some(Overlay::Form(_))));
    press(&mut app, KeyCode::Char(' '));
    assert!(matches!(&app.overlay, Some(Overlay::Form(f)) if !f.on("copy_volumes")));
    app.submit_form(false);
    let Some(Overlay::Confirm(confirm)) = &app.overlay else {
        panic!("move confirmation");
    };
    assert_eq!(confirm.verb, "move");
    assert!(!confirm.danger);
    assert_eq!(
        confirm.message,
        "Move fixture-web to fixture-worker? fixture-node keeps serving until fixture-worker passes its health check."
    );
    assert!(matches!(
        &confirm.action,
        ConfirmAction::MoveService { service_id, server_id, to, copy_volumes: false }
            if service_id == "fixture-web" && server_id == "fixture-worker" && to == "fixture-worker"
    ));
    let buf = draw(&app, 120, 30);
    label(&buf, "y move");

    let mut moving = deployment();
    moving.trigger = "move".into();
    moving.steps = [StepKind::Transfer, StepKind::Start, StepKind::Health]
        .into_iter()
        .map(|kind| Step {
            kind,
            status: StepStatus::Pending,
            detail: None,
            started_at: None,
            finished_at: None,
        })
        .collect();
    moving.steps[0].status = StepStatus::Done;
    moving.steps[0].detail = Some("image + volumes".into());
    app.overlay = None;
    app.apply(crate::app::Msg::MoveStarted(
        Box::new(moving),
        "fixture-worker".into(),
    ));
    assert_eq!(app.screen, Screen::Deploy);
    let buf = draw(&app, 120, 30);
    for text in [
        "moving #27",
        "→ fixture-worker",
        "transfer",
        "image + volumes",
        "Move output",
        "fixture-node serves until fixture-wor…",
        "c cancel move",
    ] {
        label(&buf, text);
    }
}

#[tokio::test]
async fn move_needs_another_server() {
    let mut app = fixture();
    app.servers.truncate(1);
    app.open_service("fixture-web", Tab::Settings);
    press(&mut app, KeyCode::Char('m'));
    assert!(app.overlay.is_none());
}

fn replicas_fixture() -> App {
    let mut app = fixture();
    let scaling = common::Scaling {
        lb_policy: common::LbPolicy::LeastConn,
        max_surge: 1,
        shared_volume_ok: true,
        autoscale: common::Autoscale {
            enabled: true,
            min: 2,
            max: 6,
            cpu_percent: Some(70),
            ..Default::default()
        },
        placement: Default::default(),
    };
    let replica = |index: u16| common::Replica {
        index,
        server_id: "fixture-node".into(),
        deployment_id: None,
        container: Some(common::ContainerInfo {
            id: format!("fixture-r{index}"),
            name: format!("fixture-web-r{index}"),
            image: "example:1".into(),
            state: "running".into(),
            started_at: None,
            restart_count: i64::from(index),
            exit_code: None,
            ports: Vec::new(),
            networks: Vec::new(),
        }),
        health: common::ReplicaHealth::Healthy,
        routed: true,
        cpu_percent: Some(12.0),
        mem_used: Some(84 << 20),
    };
    let detail = app.details.get_mut("fixture-web").expect("fixture detail");
    detail.service.replicas = 3;
    detail.service.scaling = scaling.clone();
    detail.service.state = common::ServiceState::Running;
    detail.replicas = (0..3).map(replica).collect();
    for node in &mut app.tree.as_mut().expect("fixture tree").projects[0].environments {
        for s in node.services.iter_mut().filter(|s| s.id == "fixture-web") {
            s.replicas = 3;
            s.scaling = scaling.clone();
            s.state = common::ServiceState::Running;
        }
    }
    app
}

#[tokio::test]
async fn replica_rows_follow_logs_and_the_rail_picks_another_replica() {
    let mut app = replicas_fixture();
    app.open_service("fixture-web", Tab::Overview);
    let buf = draw(&app, 160, 40);
    label(&buf, "running · 3 replicas");
    label(&buf, "least_conn · autoscale 2–6 · cpu 70%");
    for heading in ["STATE", "HEALTH", "RESTARTS", "UPTIME", "CONTAINER"] {
        label(&buf, heading);
    }
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-web-r0", Hit::Replica(0)),
            ("fixture-web-r1", Hit::Replica(1)),
            ("fixture-web-r2", Hit::Replica(2)),
        ],
    );
    assert_hit(&app, label(&buf, "⏎ logs"), key(KeyCode::Enter));
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("↑", key(KeyCode::Up)),
        ("⏎ logs", key(KeyCode::Enter)),
        ("d deploy", char_key('d')),
    ] {
        assert_hit(&app, label_on(&buf, y, text).expect("key bar hint"), hit);
    }

    press(&mut app, KeyCode::Down);
    assert_eq!(app.service.as_ref().map(|v| v.row), Some(1));
    press(&mut app, KeyCode::Enter);
    let view = app.service.as_ref().expect("service view");
    assert_eq!(view.tab, Tab::Logs);
    assert_eq!(view.log_replica(), Some(1));

    app.set_tab(Tab::Overview);
    let buf = draw(&app, 160, 40);
    let r2 = label(&buf, "fixture-web-r2");
    click(&mut app, Position::new(r2.x, r2.y));
    assert_eq!(app.service.as_ref().and_then(|v| v.log_replica()), Some(2));

    let buf = draw(&app, 160, 40);
    label(&buf, "Logs · replica #2");
    let rail = label(&buf, "Replicas").y;
    for i in 0..3u16 {
        let row = label_on(&buf, rail + 1 + i, &format!("#{i}")).expect("rail row");
        assert_hit(&app, row, Hit::Replica(usize::from(i)));
    }
    let r0 = label_on(&buf, rail + 1, "#0").expect("rail row");
    click(&mut app, Position::new(r0.x, r0.y));
    assert_eq!(app.service.as_ref().and_then(|v| v.log_replica()), Some(0));
    let buf = draw(&app, 160, 40);
    let next = assert_hit(
        &app,
        label_on(&buf, buf.area.bottom() - 1, "c replica").expect("replica hint"),
        char_key('c'),
    );
    click(&mut app, next);
    assert_eq!(app.service.as_ref().and_then(|v| v.log_replica()), Some(1));
}

#[tokio::test]
async fn scaling_panel_rows_hints_and_the_scale_dialog_have_targets() {
    let mut app = replicas_fixture();
    app.open_service("fixture-web", Tab::Settings);
    let buf = draw(&app, 160, 40);
    label(&buf, "2–6 replicas · cpu 70%");
    label(&buf, "cooldown up 60s / down 5m");
    label(&buf, "round_robin, ip_hash for sticky sessions");
    assert_rows(
        &app,
        &buf,
        &[
            ("load balancing", Hit::Scaling(1)),
            ("rolling surge", Hit::Scaling(2)),
            ("shared volume", Hit::Scaling(4)),
        ],
    );
    let lb = label(&buf, "load balancing");
    assert_hit(&app, Rect::new(lb.x, lb.y - 1, 8, 1), Hit::Scaling(0));
    assert_hit(&app, Rect::new(lb.x, lb.y + 2, 9, 1), Hit::Scaling(3));
    assert_hit(&app, label(&buf, "s scale"), char_key('s'));
    assert_hit(&app, label(&buf, "a autoscale"), char_key('a'));
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("←", key(KeyCode::Left)),
        ("s scale", char_key('s')),
        ("a autoscale", char_key('a')),
    ] {
        assert_hit(&app, label_on(&buf, y, text).expect("key bar hint"), hit);
    }

    click(&mut app, Position::new(lb.x, lb.y));
    let view = app.service.as_ref().expect("service view");
    assert!(view.scaling_focus);
    assert_eq!(view.row, 1);
    let buf = draw(&app, 160, 40);
    assert_hit(&app, label(&buf, "⏎ to edit"), key(KeyCode::Enter));
    let image = label(&buf, "example:1");
    click(&mut app, Position::new(image.x, image.y));
    assert!(!app.service.as_ref().expect("service view").scaling_focus);

    let surge = label(&buf, "rolling surge");
    click(&mut app, Position::new(surge.x, surge.y));
    click(&mut app, Position::new(surge.x, surge.y));
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Form(f)) if matches!(f.kind, FormKind::RollingSurge { .. })
    ));
    app.overlay = None;

    let buf = draw(&app, 160, 40);
    let auto = label(&buf, "a autoscale");
    click(&mut app, Position::new(auto.x, auto.y));
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("autoscale form");
    };
    assert!(matches!(form.kind, FormKind::Autoscale { .. }));
    assert_eq!(form.choice_label("enabled").as_deref(), Some("on"));
    let buf = draw(&app, 160, 40);
    for (text, field) in [("req/s per replica", 5), ("scale-down cooldown", 7)] {
        let row = label(&buf, text);
        assert_eq!(
            mouse::hit_at(&app.mouse, row.x, row.y),
            Some(Hit::FormField(field))
        );
    }
    label(&buf, "Scales on the average over the last minute.");
    app.overlay = None;

    let buf = draw(&app, 160, 40);
    let scale = label(&buf, "s scale");
    click(&mut app, Position::new(scale.x, scale.y));
    let buf = draw(&app, 160, 40);
    label(&buf, "Autoscaling manages these replicas: 2–6 · cpu 70%.");
    let hints = label(&buf, "a autoscale   esc cancel");
    assert_hit(&app, Rect { width: 11, ..hints }, char_key('a'));

    let Some(Overlay::Scale(d)) = app.overlay.as_mut() else {
        panic!("scale dialog");
    };
    d.autoscale = None;
    app.apply(crate::app::Msg::Capacity(
        "fixture-web".into(),
        Ok(common::ScaleCapacity {
            server_id: "fixture-node".into(),
            mem_total: 8 << 30,
            mem_free: 800 << 20,
            per_replica: 85 << 20,
            fits_more: 9,
            servers: Vec::new(),
        }),
    ));
    let buf = draw(&app, 160, 40);
    label(&buf, "3 → 3");
    label(
        &buf,
        "Memory headroom on server fixture-node fits 9 more replicas (~85 MiB each)",
    );
    let adjust = label(&buf, "↑↓ adjust");
    let up = assert_hit(&app, Rect::new(adjust.x, adjust.y, 1, 1), key(KeyCode::Up));
    assert_hit(&app, label(&buf, "esc cancel"), key(KeyCode::Esc));
    assert_hit(&app, label(&buf, "⏎ scale"), key(KeyCode::Enter));
    click(&mut app, up);
    let buf = draw(&app, 160, 40);
    label(&buf, "3 → 4");
    label(&buf, "Starts 4 replicas on the first deploy.");
    if let Some(Overlay::Scale(d)) = app.overlay.as_mut() {
        d.live = true;
    }
    let buf = draw(&app, 160, 40);
    label(&buf, "Adds 1 container. No redeploy.");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    let buf = draw(&app, 160, 40);
    label(&buf, "Drains 1 container. No redeploy.");

    let Some(Overlay::Scale(d)) = app.overlay.as_mut() else {
        panic!("scale dialog");
    };
    d.shared_ok = false;
    let buf = draw(&app, 160, 40);
    label(&buf, "✕ Replicas > 1 refused");
    label(&buf, "fixture-data → /fixture/data");
    assert_hit(&app, label(&buf, "space toggle"), char_key(' '));
    let check = label(&buf, "[ ]  Allow a shared volume");
    click(&mut app, Position::new(check.x, check.y));
    let buf = draw(&app, 160, 40);
    label(&buf, "[x]  Allow a shared volume");
    let esc = label(&buf, "esc cancel");
    click(&mut app, Position::new(esc.x, esc.y));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn metrics_tab_picks_replicas_and_charts_their_count() {
    use common::metrics::MetricsRange;
    let mut app = replicas_fixture();
    app.open_service("fixture-web", Tab::Metrics);
    let mut data = metrics_data(MetricsRange::Hour);
    data.containers = Vec::new();
    data.replicas = vec![0, 1, 2];
    for (i, p) in data.points.iter_mut().enumerate() {
        p.replicas = if i < 180 { 2 } else { 3 };
    }
    app.service.as_mut().expect("service view").metrics.data = Some(data);
    let buf = draw(&app, 164, 40);
    label(&buf, "Replicas  3 now · min 2 · max 6");
    label(&buf, "autoscale on");
    let toolbar = label(&buf, "1 1h").y;
    for (i, text) in ["all replicas", "#0", "#1", "#2"].into_iter().enumerate() {
        assert_hit(
            &app,
            label_on(&buf, toolbar, text).expect("replica"),
            Hit::MetricsReplica(i),
        );
    }
    assert_hit(
        &app,
        label_on(&buf, toolbar, "c").expect("c"),
        char_key('c'),
    );
    assert_hit(
        &app,
        label_on(&buf, buf.area.bottom() - 1, "c replica").expect("key bar"),
        char_key('c'),
    );
    let picked = |app: &App| app.service.as_ref().expect("service view").metrics.replica;
    let one = label_on(&buf, toolbar, "#1").expect("#1");
    click(&mut app, Position::new(one.x, one.y));
    assert_eq!(picked(&app), Some(1));
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(picked(&app), Some(2));
    press(&mut app, KeyCode::Char('c'));
    assert_eq!(picked(&app), None, "c wraps around to all replicas");
    let all = label_on(&buf, toolbar, "all replicas").expect("all");
    press(&mut app, KeyCode::Char('c'));
    click(&mut app, Position::new(all.x, all.y));
    assert_eq!(picked(&app), None);
}

fn provider(id: &str, kind: common::lb::ProviderKind) -> common::lb::Provider {
    common::lb::Provider {
        id: id.into(),
        name: id.into(),
        kind,
        token_secret_id: format!("{id}-token"),
        status: common::lb::ProviderStatus::Connected,
        problem: None,
        account: Some(match kind {
            common::lb::ProviderKind::Hetzner => "3 servers".into(),
            common::lb::ProviderKind::Cloudflare => "Fixture account".into(),
        }),
        load_balancing: kind == common::lb::ProviderKind::Cloudflare,
        last_checked_at: Some(Timestamp::default()),
        created_at: Timestamp::default(),
    }
}

fn hetzner_discovery() -> common::lb::ProviderDiscovery {
    use common::lb::*;
    let matched = |server: &str, ip: &str, private: &str| ServerMatch {
        server_id: server.into(),
        address: Some(ip.into()),
        hetzner: Some(HetznerServer {
            id: 1,
            name: server.into(),
            location: "fsn1".into(),
            network_zone: "eu-central".into(),
            public_ipv4: Some(ip.into()),
            public_ipv6: None,
            private_networks: vec![PrivateAddress {
                network_id: 7,
                ip: private.into(),
            }],
        }),
    };
    ProviderDiscovery {
        servers: vec![
            matched("fixture-node", "198.51.100.2", "10.0.0.2"),
            matched("fixture-worker", "198.51.100.3", "10.0.0.3"),
            ServerMatch {
                server_id: "fixture-lab".into(),
                address: Some("192.0.2.40".into()),
                hetzner: None,
            },
        ],
        locations: vec![HetznerLocation {
            name: "fsn1".into(),
            city: "Falkenstein".into(),
            network_zone: "eu-central".into(),
        }],
        lb_types: vec![HetznerLbType {
            name: "lb11".into(),
            description: "LB11".into(),
            max_targets: 25,
            max_connections: 10_000,
            prices: vec![LocationPrice {
                location: "fsn1".into(),
                monthly: 5.39,
                currency: "EUR".into(),
            }],
        }],
        networks: vec![HetznerNetwork {
            id: 7,
            name: "fixture-net".into(),
            ip_range: "10.0.0.0/16".into(),
        }],
        load_balancers: vec![
            HetznerLbSummary {
                id: 4182733,
                name: "fixture-lb".into(),
                location: "fsn1".into(),
                lb_type: "lb11".into(),
                public_ipv4: Some("203.0.113.20".into()),
                managed: true,
            },
            HetznerLbSummary {
                id: 99,
                name: "hand-made-lb".into(),
                location: "nbg1".into(),
                lb_type: "lb11".into(),
                public_ipv4: Some("203.0.113.31".into()),
                managed: false,
            },
        ],
        zones: Vec::new(),
    }
}

fn service_lb() -> common::lb::ServiceLb {
    use common::lb::*;
    let target = |server: &str, address: &str| LbTarget {
        server_id: server.into(),
        address: address.into(),
        replicas: 1,
        provider_health: Some("healthy".into()),
    };
    ServiceLb {
        config: LbConfig {
            provider_id: "fixture-hetzner".into(),
            mode: LbMode::Hetzner,
            domains: Vec::new(),
            lb_type: Some("lb11".into()),
            location: Some("fsn1".into()),
            network_id: Some(7),
            adopt_lb_id: None,
            dns_provider_id: Some("fixture-cloudflare".into()),
            proxied: None,
            adopt_records: false,
        },
        state: LbSyncState::InSync,
        targets: vec![
            target("fixture-node", "10.0.0.2"),
            target("fixture-worker", "10.0.0.3"),
        ],
        resources: vec![
            LbResource {
                kind: LbResourceKind::HetznerLoadBalancer,
                id: "4182733".into(),
                name: "fixture-lb".into(),
                created: true,
            },
            LbResource {
                kind: LbResourceKind::CloudflareDnsRecord,
                id: "9f2c1d".into(),
                name: "web.example.test".into(),
                created: true,
            },
        ],
        addresses: vec!["203.0.113.20".into()],
        dns: Vec::new(),
        drift: Vec::new(),
        last_sync_at: Some(Timestamp::default()),
        last_error: None,
        next_attempt_at: None,
    }
}

/// Three servers (one offline), a Hetzner, a Cloudflare and a broken connection, and the web
/// service spread over two servers behind a Hetzner load balancer.
fn lb_fixture() -> App {
    use common::lb::ProviderKind;
    let mut app = replicas_fixture();
    app.servers[1].kind = ServerKind::Ssh;
    app.servers[1].host = Some("198.51.100.3".into());
    let mut lab = server("fixture-lab");
    lab.kind = ServerKind::Ssh;
    lab.host = Some("192.0.2.40".into());
    lab.status = ServerStatus::Offline;
    app.servers.push(lab);
    let mut broken = provider("fixture-stale", ProviderKind::Hetzner);
    broken.status = common::lb::ProviderStatus::Error;
    broken.problem = Some("token rejected · 401".into());
    broken.account = None;
    app.providers.list = vec![
        provider("fixture-hetzner", ProviderKind::Hetzner),
        provider("fixture-cloudflare", ProviderKind::Cloudflare),
        broken,
    ];
    app.providers.loaded = true;
    app.providers
        .discovery
        .insert("fixture-hetzner".into(), Ok(hetzner_discovery()));
    app.providers
        .users
        .insert("fixture-hetzner".into(), vec!["fixture-web".into()]);
    let mut token = secret("fixture-hetzner-token", common::SecretKind::Generic, None);
    token.id = "fixture-hetzner-token".into();
    app.secrets.list = vec![token];
    let placement = common::Placement {
        servers: vec!["fixture-node".into(), "fixture-worker".into()],
        strategy: common::PlacementStrategy::Spread,
        volumes_per_server: true,
    };
    let detail = app.details.get_mut("fixture-web").expect("fixture detail");
    detail.service.scaling.placement = placement.clone();
    detail.replicas.truncate(2);
    detail.replicas[1].server_id = "fixture-worker".into();
    detail.service.replicas = 2;
    detail.lb = Some(service_lb());
    for node in &mut app.tree.as_mut().expect("fixture tree").projects[0].environments {
        for s in node.services.iter_mut().filter(|s| s.id == "fixture-web") {
            s.scaling.placement = placement.clone();
            s.replicas = 2;
        }
    }
    app
}

fn lb_plan() -> common::lb::LbPlan {
    use common::lb::*;
    LbPlan {
        actions: vec![
            LbAction {
                verb: LbVerb::Create,
                resource: LbResourceKind::HetznerLoadBalancer,
                name: "fixture-lb".into(),
                detail: "lb11 in fsn1".into(),
            },
            LbAction {
                verb: LbVerb::Create,
                resource: LbResourceKind::CloudflareDnsRecord,
                name: "web.example.test".into(),
                detail: "A → load balancer, via fixture-cloudflare".into(),
            },
        ],
        targets: service_lb().targets,
        monthly_cost: Some(5.39),
        currency: "EUR".into(),
        cost_note: "Hetzner list price for lb11 in fsn1".into(),
        dns: Vec::new(),
        warnings: vec!["Hetzner bills it by the hour until you turn it off.".into()],
    }
}

#[tokio::test]
async fn providers_rows_detail_discovery_and_key_bar_have_targets() {
    let mut app = lb_fixture();
    app.screen = Screen::Settings;
    app.settings.settings = Some(InstanceSettings {
        version: "1.0.0".into(),
        hostname: "fixture-host".into(),
        public_ip: None,
        public_ip_override: None,
        listen: "127.0.0.1:7700".into(),
        data_dir: "/fixture".into(),
        data_dir_size: 0,
        manage_proxy: true,
        proxy: ProxyStatus::Running,
        proxy_image: "example:1".into(),
        docker_version: None,
        ssh_public_key: None,
        reclaimable: None,
        supervised: false,
        domain: None,
        domain_verified: false,
    });
    let buf = draw(&app, 160, 40);
    label(&buf, "3  2 connected · 1 need attention");
    let link = assert_hit(&app, label(&buf, "p open providers"), char_key('p'));
    click(&mut app, link);
    assert_eq!(app.screen, Screen::Providers);

    let buf = draw(&app, 160, 40);
    assert_rows(
        &app,
        &buf,
        &[
            ("fixture-hetzner ", Hit::Item(0)),
            ("fixture-cloudflare", Hit::Item(1)),
            ("fixture-stale", Hit::Item(2)),
        ],
    );
    assert_hit(
        &app,
        label(&buf, "LAST CHECKED"),
        Hit::List(List::Providers),
    );
    label(&buf, "error token rejected · 401");
    label(&buf, "Fixture account · load balancing");
    label(&buf, "fixture-hetzner-token  generic secret");
    label(
        &buf,
        "fixture-web load balancer  fixture-project / fixture-env",
    );
    let check_row = label(&buf, "reaches").y;
    let check = label_in(&buf, check_row..check_row + 1, "c check");
    assert_hit(&app, check, char_key('c'));
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("a add", char_key('a')),
        ("c check", char_key('c')),
        ("d remove", char_key('d')),
        ("⏎ details", key(KeyCode::Enter)),
        ("esc back", key(KeyCode::Esc)),
    ] {
        assert_hit(&app, label_on(&buf, y, text).expect("key bar hint"), hit);
    }

    let stale = label(&buf, "fixture-stale");
    click(&mut app, Position::new(stale.x, stale.y));
    assert_eq!(app.providers.row, 2);
    let first = label(&buf, "fixture-hetzner ");
    click(&mut app, Position::new(first.x, first.y));
    click(&mut app, Position::new(first.x, first.y));
    assert_eq!(app.providers.open.as_deref(), Some("fixture-hetzner"));

    let buf = draw(&app, 160, 40);
    label(&buf, "Providers / fixture-hetzner");
    label(&buf, "matched by public IP");
    label(&buf, "fixture-net · 10.0.0.2");
    label(&buf, "not in this project");
    label(&buf, "not managed by basse");
    let rows = label(&buf, "PRIVATE NETWORK").y;
    for (i, name) in ["fixture-node", "fixture-worker", "fixture-lab"]
        .into_iter()
        .enumerate()
    {
        let row = label_in(&buf, rows + 1..rows + 4, name);
        assert_hit(&app, row, Hit::Item(i));
    }
    let lab = label_in(&buf, rows + 1..rows + 4, "fixture-lab");
    click(&mut app, Position::new(lab.x, lab.y));
    assert_eq!(app.providers.detail_row, 2);
    assert_hit(
        &app,
        label_on(&buf, buf.area.bottom() - 1, "c check").expect("key bar"),
        char_key('c'),
    );
    let back = label_on(&buf, buf.area.bottom() - 1, "esc back").expect("key bar");
    click(&mut app, Position::new(back.x, back.y));
    assert!(app.providers.open.is_none());
    assert_eq!(app.screen, Screen::Providers);
}

#[tokio::test]
async fn add_provider_form_picks_a_secret_and_a_provider_in_use_is_refused_up_front() {
    let mut app = lb_fixture();
    app.screen = Screen::Providers;
    press(&mut app, KeyCode::Char('a'));
    let list = app.providers.list.clone();
    app.apply(crate::app::Msg::Providers(
        crate::providers::ProvidersMsg::List(list),
    ));
    assert!(
        matches!(&app.overlay, Some(Overlay::Form(_))),
        "a refreshed list keeps the form open"
    );
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Add provider", |hit| {
        matches!(
            hit,
            Hit::FormField(_) | Hit::FormChoice(..) | Hit::List(List::Form) | Hit::Key(..)
        )
    });
    label(&buf, "connect a cloud account");
    label(&buf, "saved as a secret");
    label(&buf, "Hetzner: a read & write project token.");
    let kind_row = label(&buf, "←→ choose").y;
    let cloudflare = label_in(&buf, kind_row..kind_row + 1, " Cloudflare ");
    click(&mut app, Position::new(cloudflare.x + 1, cloudflare.y));
    assert!(
        matches!(&app.overlay, Some(Overlay::Form(f)) if f.choice_label("kind").as_deref() == Some("Cloudflare"))
    );
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(&app.overlay, Some(Overlay::Form(f)) if f.error.as_deref() == Some("paste the API token, or pick a secret that holds it")),
        "nothing to connect with"
    );
    let buf = draw(&app, 160, 40);
    let secret = label(&buf, "— none");
    click(&mut app, Position::new(secret.x, secret.y));
    let Some(Overlay::Picker(p)) = &app.overlay else {
        panic!("secret picker");
    };
    assert_eq!(p.items.len(), 2);
    let buf = draw(&app, 160, 40);
    let token = label(&buf, "fixture-hetzner-token");
    click(&mut app, Position::new(token.x, token.y));
    assert_hit(&app, label(&buf, "esc cancel"), key(KeyCode::Esc));
    press(&mut app, KeyCode::Enter);
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("back on the form");
    };
    assert_eq!(form.raw("token_secret"), "fixture-hetzner-token");
    app.overlay = None;

    press(&mut app, KeyCode::Char('d'));
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Remove provider", |hit| {
        hit == key(KeyCode::Esc)
    });
    label(
        &buf,
        "fixture-web in fixture-project / fixture-env uses it for its",
    );
    label(
        &buf,
        "Turn that load balancer off first: fixture-web, Settings, l, x",
    );
    assert!((buf.area.top()..buf.area.bottom()).all(|y| label_on(&buf, y, "y remove").is_none()));
    let close = assert_hit(&app, label(&buf, "esc close"), key(KeyCode::Esc));
    click(&mut app, close);
    assert!(app.overlay.is_none());

    app.providers.row = 1;
    press(&mut app, KeyCode::Char('d'));
    let buf = draw(&app, 160, 40);
    label(&buf, "Remove fixture-cloudflare?");
    assert_hit(&app, label(&buf, "y remove"), char_key('y'));
}

#[tokio::test]
async fn scaling_panel_shows_placement_and_the_load_balancer() {
    let mut app = lb_fixture();
    app.open_service("fixture-web", Tab::Settings);
    let buf = draw(&app, 160, 40);
    label(&buf, "Scaling & placement");
    label(&buf, "fixture-node · fixture-worker");
    label(&buf, "spread  one replica per server before doubling up");
    label(&buf, "● Hetzner · lb11 · in sync · 2/2 targets healthy");
    assert_rows(
        &app,
        &buf,
        &[
            ("servers ", Hit::Scaling(5)),
            ("strategy", Hit::Scaling(6)),
            ("volumes per server", Hit::Scaling(7)),
            ("load balancer ", Hit::Scaling(8)),
        ],
    );
    let servers_row = label(&buf, "fixture-node · fixture-worker").y;
    assert_hit(
        &app,
        label_in(&buf, servers_row..servers_row + 1, "p placement"),
        char_key('p'),
    );
    assert_hit(&app, label(&buf, "l manage"), char_key('l'));
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("p placement", char_key('p')),
        ("l load balancer", char_key('l')),
        ("m move", char_key('m')),
    ] {
        assert_hit(&app, label_on(&buf, y, text).expect("key bar hint"), hit);
    }

    let lb = label(&buf, "load balancer ");
    click(&mut app, Position::new(lb.x, lb.y));
    click(&mut app, Position::new(lb.x, lb.y));
    let view = app.service.as_ref().expect("service view");
    assert!(view.lb_open, "a double-click on the row opens the status");
    app.close_lb();

    if let Some(d) = app.service.as_mut().and_then(|v| v.detail.as_mut()) {
        d.lb = None;
    }
    let buf = draw(&app, 160, 40);
    label(&buf, "○ off");
    let enable = assert_hit(&app, label(&buf, "l enable"), char_key('l'));
    click(&mut app, enable);
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Form(f)) if matches!(f.kind, FormKind::EnableLb { .. })
    ));
}

#[tokio::test]
async fn placement_dialog_toggles_servers_strategy_and_volumes_by_click() {
    use crate::lb::LbMsg;
    let mut app = lb_fixture();
    app.open_service("fixture-web", Tab::Settings);
    press(&mut app, KeyCode::Char('p'));
    app.apply(crate::app::Msg::Lb(LbMsg::Capacity(
        "fixture-web".into(),
        Ok(common::ScaleCapacity {
            server_id: "fixture-node".into(),
            mem_total: 8 << 30,
            mem_free: 5 << 30,
            per_replica: 85 << 20,
            fits_more: 9,
            servers: vec![
                common::ServerCapacity {
                    server_id: "fixture-node".into(),
                    mem_total: 8 << 30,
                    mem_free: 5 << 30,
                    fits_more: 9,
                    replicas: 1,
                    problem: None,
                },
                common::ServerCapacity {
                    server_id: "fixture-worker".into(),
                    mem_total: 4 << 30,
                    mem_free: 3 << 30,
                    fits_more: 6,
                    replicas: 1,
                    problem: None,
                },
            ],
        }),
    )));
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Placement", |hit| {
        matches!(
            hit,
            Hit::PlacementRow(0..=2) | Hit::PlacementStrategy(0..=1) | Hit::Key(..)
        )
    });
    label(&buf, "FREE MEMORY");
    label(&buf, crate::util::bytes(5 << 30).as_str());
    label(&buf, "offline  can't take replicas");
    label(&buf, "fixture-data → /fixture/data is copied per server");
    assert_hit(&app, label(&buf, "⏎ use 2 servers"), key(KeyCode::Enter));
    for (text, hit) in [
        ("space toggle", char_key(' ')),
        ("s strategy", char_key('s')),
        ("v volumes", char_key('v')),
        ("esc cancel", key(KeyCode::Esc)),
    ] {
        assert_hit(&app, label(&buf, text), hit);
    }
    let placed = |app: &App| match &app.overlay {
        Some(Overlay::Placement(d)) => d.chosen(),
        _ => panic!("placement dialog"),
    };
    let lab = label(&buf, "[ ] fixture-lab");
    click(&mut app, Position::new(lab.x, lab.y));
    assert_eq!(placed(&app).len(), 2, "an offline server can't be added");
    let worker = label(&buf, "[x] fixture-worker");
    click(&mut app, Position::new(worker.x, worker.y));
    assert_eq!(placed(&app), ["fixture-node"]);
    let buf = draw(&app, 160, 40);
    label(&buf, "⏎ use 1 server");
    let pack = label(&buf, " pack ");
    click(&mut app, Position::new(pack.x, pack.y));
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Placement(d)) if d.strategy == common::PlacementStrategy::Pack
    ));
    let buf = draw(&app, 160, 40);
    label(&buf, "fill servers in order before using the next");
    click(&mut app, Position::new(worker.x, worker.y));
    let check = label(&buf, "[x] on");
    click(&mut app, Position::new(check.x, check.y));
    let buf = draw(&app, 160, 40);
    label(
        &buf,
        "fixture-data → /fixture/data can't span servers: v keeps a copy per server.",
    );
    press(&mut app, KeyCode::Enter);
    assert!(
        matches!(&app.overlay, Some(Overlay::Placement(d)) if !d.busy),
        "volumes on two servers need a copy per server"
    );
    let esc = label(&buf, "esc cancel");
    click(&mut app, Position::new(esc.x, esc.y));
    assert!(app.overlay.is_none());
}

#[tokio::test]
async fn load_balancer_form_plan_status_drift_and_disable_have_targets() {
    use crate::lb::LbMsg;
    let mut app = lb_fixture();
    app.open_service("fixture-web", Tab::Settings);
    let lb = app
        .service
        .as_mut()
        .and_then(|v| v.detail.as_mut())
        .and_then(|d| d.lb.take());
    press(&mut app, KeyCode::Char('l'));
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Load balancer", |hit| {
        matches!(
            hit,
            Hit::FormField(_) | Hit::FormChoice(..) | Hit::List(List::Form) | Hit::Key(..)
        )
    });
    label(&buf, "fixture-web · enable");
    label(&buf, "fixture-hetzner  Hetzner Cloud");
    label(&buf, " Cloudflare DNS ");
    label(
        &buf,
        "Targets follow healthy replicas. Basse only ever deletes what it created itself.",
    );
    let dns = label(&buf, "records to add by hand");
    click(&mut app, Position::new(dns.x, dns.y));
    let Some(Overlay::Picker(p)) = &app.overlay else {
        panic!("dns picker");
    };
    assert_eq!(p.items.len(), 2, "none and the Cloudflare connection");
    let buf = draw(&app, 160, 40);
    let cf = label(&buf, "fixture-cloudflare");
    click(&mut app, Position::new(cf.x, cf.y));
    click(&mut app, Position::new(cf.x, cf.y));
    let Some(Overlay::Form(form)) = &app.overlay else {
        panic!("back on the form");
    };
    assert_eq!(form.raw("dns"), "fixture-cloudflare");
    let buf = draw(&app, 160, 40);
    let lb_type = label(&buf, "type ");
    click(&mut app, Position::new(lb_type.x, lb_type.y));
    let buf = draw(&app, 160, 40);
    label(&buf, "25 targets · 10k connections");
    label(&buf, "€5.39/month");
    press(&mut app, KeyCode::Enter);
    let buf = draw(&app, 160, 40);
    label(&buf, "25 targets · 10k connections");
    let network = label(&buf, "public addresses");
    click(&mut app, Position::new(network.x, network.y));
    let buf = draw(&app, 160, 40);
    label(&buf, "10.0.0.0/16");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Form(f)) if f.raw("network") == "7"
    ));

    let Some(Overlay::Form(form)) = app.overlay.as_mut() else {
        panic!("load balancer form");
    };
    form.busy = true;
    app.apply(crate::app::Msg::Lb(LbMsg::Plan(
        "fixture-web".into(),
        Ok(lb_plan()),
    )));
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Load balancer", |hit| {
        matches!(hit, Hit::Key(..))
    });
    label(&buf, "Basse will");
    label(&buf, "Hetzner load balancer");
    label(&buf, "10.0.0.2 · fixture-net");
    label(&buf, "≈ €5.39/month");
    label(
        &buf,
        "! Hetzner bills it by the hour until you turn it off.",
    );
    assert_hit(&app, label(&buf, "y enable"), char_key('y'));
    assert_hit(&app, label(&buf, "esc cancel"), key(KeyCode::Esc));
    let back = assert_hit(&app, label(&buf, "b back to edit"), char_key('b'));
    click(&mut app, back);
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Form(f)) if matches!(f.kind, FormKind::EnableLb { .. }) && !f.busy
    ));

    app.overlay = None;
    app.apply(crate::app::Msg::Lb(LbMsg::Enabled(
        "fixture-web".into(),
        Ok(lb.expect("fixture load balancer")),
    )));
    assert!(app.service.as_ref().is_some_and(|v| v.lb_open));
    assert!(
        app.toast
            .take()
            .is_some_and(|t| t.text == "Hetzner Load Balancer on · it syncs in the background")
    );
    let buf = draw(&app, 160, 40);
    label(&buf, "● in sync");
    label(
        &buf,
        "203.0.113.20   web.example.test → A record via fixture-cloudflare",
    );
    label(
        &buf,
        "lb11 · fsn1   fixture-net, TCP 80 and 443, health check on the proxy",
    );
    label(&buf, "fixture-hetzner · Hetzner Cloud");
    label(&buf, "2 of 2 healthy");
    label(&buf, "Created by basse");
    label(&buf, "deleted when you disable it");
    assert_hit(
        &app,
        label(&buf, "HETZNER HEALTH"),
        Hit::List(List::Targets),
    );
    let rows = label(&buf, "HETZNER HEALTH").y;
    for i in 0..2u16 {
        let row = label_in(&buf, rows + 1 + i..rows + 2 + i, "1 of 1");
        assert_hit(&app, row, Hit::Row(usize::from(i)));
    }
    let y = buf.area.bottom() - 1;
    for (text, hit) in [
        ("s sync", char_key('s')),
        ("f force sync", char_key('f')),
        ("x disable", char_key('x')),
        ("esc back", key(KeyCode::Esc)),
    ] {
        assert_hit(&app, label_on(&buf, y, text).expect("key bar hint"), hit);
    }
    let second = label_in(&buf, rows + 2..rows + 3, "fixture-worker");
    click(&mut app, Position::new(second.x, second.y));
    assert_eq!(app.service.as_ref().map(|v| v.row), Some(1));

    if let Some(lb) = app
        .service
        .as_mut()
        .and_then(|v| v.detail.as_mut())
        .and_then(|d| d.lb.as_mut())
    {
        lb.state = common::lb::LbSyncState::Drift;
        lb.drift = vec!["targets changed outside Basse: 10.0.0.9 added".into()];
    }
    let buf = draw(&app, 160, 40);
    label(&buf, "● drift");
    label(&buf, "waiting for you");
    label(
        &buf,
        "! targets changed outside Basse: 10.0.0.9 added · f force sync",
    );
    let drift = label(&buf, "! targets changed").y;
    assert_hit(
        &app,
        label_in(&buf, drift..drift + 1, "f force sync"),
        char_key('f'),
    );

    press(&mut app, KeyCode::Char('x'));
    let buf = draw(&app, 160, 40);
    assert_modal_targets(&app, &buf, "Turn off load balancer", |hit| {
        matches!(hit, Hit::Key(..))
    });
    label(&buf, "Delete the 2 resources Basse created?");
    label(&buf, "lb11 in fsn1 · billed until deleted");
    label(
        &buf,
        "Resources Basse adopted instead of creating are always kept.",
    );
    assert_hit(&app, label(&buf, "y delete 2 resources"), char_key('y'));
    assert_hit(&app, label(&buf, "k keep them"), char_key('k'));
    let cancel = assert_hit(&app, label(&buf, "esc cancel"), key(KeyCode::Esc));
    click(&mut app, cancel);
    assert!(app.overlay.is_none());
    press(&mut app, KeyCode::Esc);
    assert!(!app.service.as_ref().is_some_and(|v| v.lb_open));
    assert_eq!(app.screen, Screen::Service);
}
