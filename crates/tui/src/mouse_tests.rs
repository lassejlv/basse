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
                created_at: Timestamp::default(),
            }],
            volumes: vec![Volume {
                id: "fixture-volume".into(),
                service_id: web.id.clone(),
                name: "fixture-data".into(),
                mount_path: "/fixture/data".into(),
                created_at: Timestamp::default(),
            }],
            container: None,
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
        status: StepStatus::Running,
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
