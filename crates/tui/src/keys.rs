//! Key handling: overlays first, then the active screen.

use crate::app::*;
use crate::form::{FormAction, FormKind};
use common::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    if app.overlay.is_some() {
        return overlay(app, key);
    }
    if ctrl && key.code == KeyCode::Char('r') {
        app.refresh();
        if app
            .service
            .as_ref()
            .is_some_and(|v| v.tab == Tab::Logs && v.log_task.is_none())
        {
            app.restart_service_logs();
        }
        return app.toast("refreshed");
    }
    if text_input_active(app) {
        return text_input(app, key);
    }
    match key.code {
        KeyCode::Char('q') => return app.quit = true,
        KeyCode::Char('?') => return app.overlay = Some(Overlay::Help),
        KeyCode::Char(':') => return app.overlay = Some(Overlay::Palette(Palette::default())),
        KeyCode::Char('S') if app.screen != Screen::Servers => {
            app.screen = Screen::Servers;
            return app.refresh();
        }
        _ => {}
    }
    match app.screen {
        Screen::Overview => overview(app, key),
        Screen::Service => service(app, key),
        Screen::Deploy => deploy(app, key),
        Screen::Servers => servers(app, key),
    }
}

fn text_input_active(app: &App) -> bool {
    match app.screen {
        Screen::Overview => app.overview.filtering,
        Screen::Service => app.service.as_ref().is_some_and(|v| v.logs.searching),
        _ => false,
    }
}

/// The overview filter and the log search share one line editor.
fn text_input(app: &mut App, key: KeyEvent) {
    let viewport = app.log_viewport.get();
    let (text, done) = match app.screen {
        Screen::Overview => (&mut app.overview.filter, &mut app.overview.filtering),
        _ => {
            let Some(view) = app.service.as_mut() else {
                return;
            };
            (&mut view.logs.search, &mut view.logs.searching)
        }
    };
    match key.code {
        KeyCode::Esc => {
            text.clear();
            *done = false;
        }
        KeyCode::Enter => *done = false,
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char(c) => text.push(c),
        _ => {}
    }
    match app.screen {
        Screen::Overview => app.overview.row = 0,
        _ => {
            if let Some(view) = app.service.as_mut()
                && key.code == KeyCode::Enter
            {
                view.logs.match_idx = view.logs.matches().len().saturating_sub(1);
                view.logs.jump(0, viewport);
            }
        }
    }
}

fn overlay(app: &mut App, key: KeyEvent) {
    match app.overlay.as_mut() {
        Some(Overlay::Form(form)) => match form.handle_key(key) {
            FormAction::None => {}
            FormAction::Cancel => app.overlay = None,
            FormAction::Submit { alt } => {
                if matches!(form.kind, FormKind::EnvVar { .. }) {
                    app.submit_env_form(alt);
                } else {
                    app.submit_form(alt);
                }
            }
        },
        Some(Overlay::Confirm(_)) => match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                if let Some(Overlay::Confirm(c)) = app.overlay.take() {
                    app.confirm(c.action);
                }
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => app.overlay = None,
            _ => {}
        },
        Some(Overlay::Palette(p)) => match key.code {
            KeyCode::Esc => app.overlay = None,
            KeyCode::Up => p.row = p.row.saturating_sub(1),
            KeyCode::Down => p.row = (p.row + 1).min(p.commands().len().saturating_sub(1)),
            KeyCode::Backspace => {
                p.input.pop();
                p.row = 0;
            }
            KeyCode::Char(c) => {
                p.input.push(c);
                p.row = 0;
            }
            KeyCode::Enter => {
                let cmd = p.commands().get(p.row).copied();
                app.overlay = None;
                if let Some(cmd) = cmd {
                    run_command(app, cmd);
                }
            }
            _ => {}
        },
        Some(Overlay::Help) => app.overlay = None,
        None => {}
    }
}

pub fn run_command(app: &mut App, cmd: Command) {
    let selected = app.selected_service().cloned();
    match cmd {
        Command::Deploy | Command::Restart | Command::Stop | Command::Start | Command::Logs => {
            let Some(s) = selected else {
                return app.error("select a service first");
            };
            match cmd {
                Command::Deploy => app.deploy_service(&s.id),
                Command::Restart => app.restart_service(&s),
                Command::Stop => ask_stop(app, &s),
                Command::Start => app.start_service(&s),
                _ => app.open_service(&s.id, Tab::Logs),
            }
        }
        Command::NewService => app.form_new_service(),
        Command::NewProject => app.form_new_project(),
        Command::NewEnvironment => app.form_new_environment(),
        Command::Servers => {
            app.screen = Screen::Servers;
            app.refresh();
        }
        Command::AddServer => app.form_server(),
        Command::SetupProxy => match app.local_server().cloned() {
            Some(s) => app.setup_proxy(&s),
            None => app.error("no local server registered"),
        },
        Command::Refresh => app.refresh(),
        Command::Help => app.overlay = Some(Overlay::Help),
        Command::Quit => app.quit = true,
    }
}

fn ask(app: &mut App, title: &str, message: String, danger: bool, action: ConfirmAction) {
    app.overlay = Some(Overlay::Confirm(Confirm {
        title: title.into(),
        message,
        danger,
        action,
    }));
}

fn ask_stop(app: &mut App, s: &Service) {
    ask(
        app,
        "Stop service",
        format!(
            "Stop {}? Its domains will return 502 until you start it again.",
            s.name
        ),
        false,
        ConfirmAction::Stop(s.id.clone()),
    );
}

fn stop_or_start(app: &mut App, s: &Service) {
    match s.state {
        ServiceState::Stopped | ServiceState::Failed => app.start_service(s),
        ServiceState::New => app.deploy_service(&s.id),
        _ => ask_stop(app, s),
    }
}

fn ask_delete_service(app: &mut App, s: &Service) {
    ask(
        app,
        "Delete service",
        format!(
            "Delete {} and remove its containers? Volumes are kept.",
            s.name
        ),
        true,
        ConfirmAction::DeleteService(s.id.clone()),
    );
}

fn overview(app: &mut App, key: KeyEvent) {
    let pane = app.overview.pane;
    let selected = app.selected_service().cloned();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') if pane == Pane::Sidebar => app.move_side(-1),
        KeyCode::Down | KeyCode::Char('j') if pane == Pane::Sidebar => app.move_side(1),
        KeyCode::Up | KeyCode::Char('k') => app.move_row(-1),
        KeyCode::Down | KeyCode::Char('j') => app.move_row(1),
        KeyCode::Left | KeyCode::Char('h') => app.overview.pane = Pane::Sidebar,
        KeyCode::Right => app.overview.pane = Pane::Services,
        KeyCode::Tab | KeyCode::BackTab => {
            app.overview.pane = if pane == Pane::Sidebar {
                Pane::Services
            } else {
                Pane::Sidebar
            };
        }
        KeyCode::Enter if pane == Pane::Sidebar => match app.side_items().get(app.overview.side) {
            Some(SideItem::Project(_)) => app.toggle_project(),
            Some(SideItem::Environment(..)) => app.overview.pane = Pane::Services,
            None => {}
        },
        KeyCode::Enter => {
            if let Some(s) = selected {
                app.open_service(&s.id, Tab::Overview);
            }
        }
        KeyCode::Char(' ') if pane == Pane::Sidebar => app.toggle_project(),
        KeyCode::Char('/') => {
            app.overview.pane = Pane::Services;
            app.overview.filtering = true;
        }
        KeyCode::Esc if !app.overview.filter.is_empty() => app.overview.filter.clear(),
        KeyCode::Char('n') => app.form_new_service(),
        KeyCode::Char('N') => app.form_new_project(),
        KeyCode::Char('E') => app.form_new_environment(),
        KeyCode::Char('x') | KeyCode::Delete if pane == Pane::Sidebar => {
            let item = app.side_items().get(app.overview.side).copied();
            let Some(tree) = &app.tree else { return };
            match item {
                Some(SideItem::Project(pi)) => {
                    let p = &tree.projects[pi];
                    let n: usize = p.environments.iter().map(|e| e.services.len()).sum();
                    let msg = format!(
                        "Delete project {} with {} environments and {n} services? Containers are removed.",
                        p.project.name,
                        p.environments.len()
                    );
                    let id = p.project.id.clone();
                    ask(
                        app,
                        "Delete project",
                        msg,
                        true,
                        ConfirmAction::DeleteProject(id),
                    );
                }
                Some(SideItem::Environment(pi, ei)) => {
                    let e = &tree.projects[pi].environments[ei];
                    let msg = format!(
                        "Delete environment {} and its {} services?",
                        e.environment.name,
                        e.services.len()
                    );
                    let id = e.environment.id.clone();
                    ask(
                        app,
                        "Delete environment",
                        msg,
                        true,
                        ConfirmAction::DeleteEnvironment(id),
                    );
                }
                None => {}
            }
        }
        _ => {
            let Some(s) = selected else { return };
            match key.code {
                KeyCode::Char('d') => app.deploy_service(&s.id),
                KeyCode::Char('r') => app.restart_service(&s),
                KeyCode::Char('s') => stop_or_start(app, &s),
                KeyCode::Char('l') => app.open_service(&s.id, Tab::Logs),
                KeyCode::Char('x') | KeyCode::Delete => ask_delete_service(app, &s),
                _ => {}
            }
        }
    }
}

fn service(app: &mut App, key: KeyEvent) {
    let Some(view) = app.service.as_mut() else {
        return;
    };
    let tab = view.tab;
    let viewport = app.log_viewport.get();
    match key.code {
        KeyCode::Esc => return app.close_service(),
        KeyCode::Tab => return app.set_tab(Tab::ALL[(tab.index() + 1) % Tab::ALL.len()]),
        KeyCode::BackTab => {
            return app.set_tab(Tab::ALL[(tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len()]);
        }
        KeyCode::Char(c @ '1'..='7') => return app.set_tab(Tab::ALL[c as usize - '1' as usize]),
        _ => {}
    }
    let detail = view.detail.clone();
    let rows = match (tab, &detail) {
        (Tab::Deployments, _) => view.deployments.len(),
        (Tab::Env, Some(d)) => d.env.len(),
        (Tab::Domains, Some(d)) => d.domains.len(),
        (Tab::Volumes, Some(d)) => d.volumes.len(),
        _ => 0,
    };
    if tab == Tab::Logs {
        let logs = &mut view.logs;
        let handled = match key.code {
            KeyCode::Char('f') => {
                logs.follow = !logs.follow;
                if logs.follow {
                    logs.offset = 0;
                }
                true
            }
            KeyCode::Char('/') => {
                logs.searching = true;
                logs.search.clear();
                true
            }
            KeyCode::Char('n') => {
                logs.jump(1, viewport);
                true
            }
            KeyCode::Char('N') => {
                logs.jump(-1, viewport);
                true
            }
            KeyCode::Char('t') => {
                logs.timestamps = !logs.timestamps;
                true
            }
            KeyCode::Char('w') => {
                logs.wrap = !logs.wrap;
                true
            }
            KeyCode::Char('e') => {
                logs.errors_only = !logs.errors_only;
                logs.offset = 0;
                logs.follow = true;
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                logs.scroll(1, viewport);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                logs.scroll(-1, viewport);
                true
            }
            KeyCode::PageUp => {
                logs.scroll(viewport as isize - 2, viewport);
                true
            }
            KeyCode::PageDown => {
                logs.scroll(2 - viewport as isize, viewport);
                true
            }
            KeyCode::Char('g') | KeyCode::Home => {
                logs.scroll(isize::MAX / 2, viewport);
                true
            }
            KeyCode::Char('G') | KeyCode::End => {
                logs.scroll(isize::MIN / 2, viewport);
                true
            }
            _ => false,
        };
        if handled {
            return;
        }
    } else {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => return view.row = view.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                return view.row = (view.row + 1).min(rows.saturating_sub(1));
            }
            _ => {}
        }
    }
    let row = view.row;
    let Some(detail) = detail else { return };
    let s = detail.service.clone();
    match (tab, key.code) {
        (_, KeyCode::Char('d')) => app.deploy_service(&s.id),
        (_, KeyCode::Char('r')) => app.restart_service(&s),
        (_, KeyCode::Char('s')) => stop_or_start(app, &s),
        (_, KeyCode::Char('l')) => app.set_tab(Tab::Logs),
        (Tab::Deployments, KeyCode::Enter) => {
            if let Some(d) = app
                .service
                .as_ref()
                .and_then(|v| v.deployments.get(row))
                .cloned()
            {
                app.open_deploy(d);
            }
        }
        (Tab::Deployments, KeyCode::Char('R')) => {
            let Some(d) = app
                .service
                .as_ref()
                .and_then(|v| v.deployments.get(row))
                .cloned()
            else {
                return;
            };
            if d.image.is_none() {
                return app.error("that deployment never produced an image");
            }
            let label = d
                .commit
                .as_deref()
                .map_or(format!("#{}", d.number), util_sha);
            ask(
                app,
                "Roll back",
                format!(
                    "Deploy {} again from its image? This skips the build.",
                    label
                ),
                false,
                ConfirmAction::Rollback(d.id),
            );
        }
        (Tab::Deployments, KeyCode::Char('c')) => {
            let Some(d) = app
                .service
                .as_ref()
                .and_then(|v| v.deployments.get(row))
                .cloned()
            else {
                return;
            };
            if d.status.is_finished() {
                return app.error("that deployment already finished");
            }
            ask(
                app,
                "Cancel deployment",
                format!(
                    "Cancel deployment #{}? The live container keeps running.",
                    d.number
                ),
                true,
                ConfirmAction::CancelDeploy(d.id),
            );
        }
        (Tab::Env, KeyCode::Char('a')) => app.form_env(None),
        (Tab::Env, KeyCode::Enter) => {
            if let Some(v) = detail.env.get(row).cloned() {
                app.form_env(Some(v));
            }
        }
        (Tab::Env, KeyCode::Char('v')) => {
            if let Some(view) = app.service.as_mut() {
                view.reveal = !view.reveal;
            }
        }
        (Tab::Env, KeyCode::Char('x') | KeyCode::Delete) => {
            if let Some(v) = detail.env.get(row) {
                ask(
                    app,
                    "Remove variable",
                    format!("Remove {}? It disappears on the next deploy.", v.key),
                    true,
                    ConfirmAction::DeleteEnvVar(s.id.clone(), v.key.clone()),
                );
            }
        }
        (Tab::Domains, KeyCode::Char('a')) => app.form_domain(),
        (Tab::Domains, KeyCode::Char('x') | KeyCode::Delete) => {
            if let Some(d) = detail.domains.get(row) {
                ask(
                    app,
                    "Remove domain",
                    format!("Stop routing {} to {}?", d.host, s.name),
                    true,
                    ConfirmAction::DeleteDomain(d.id.clone()),
                );
            }
        }
        (Tab::Volumes, KeyCode::Char('a')) => app.form_volume(),
        (Tab::Volumes, KeyCode::Char('x') | KeyCode::Delete) => {
            if let Some(v) = detail.volumes.get(row) {
                ask(
                    app,
                    "Remove volume",
                    format!(
                        "Unmount {} from {} on the next deploy? The data is kept.",
                        v.name, s.name
                    ),
                    false,
                    ConfirmAction::DeleteVolume(v.id.clone(), false),
                );
            }
        }
        (Tab::Volumes, KeyCode::Char('X')) => {
            if let Some(v) = detail.volumes.get(row) {
                ask(
                    app,
                    "Remove volume and data",
                    format!("Remove {} and permanently delete its data?", v.name),
                    true,
                    ConfirmAction::DeleteVolume(v.id.clone(), true),
                );
            }
        }
        (Tab::Settings, KeyCode::Enter | KeyCode::Char('e')) => app.form_edit_service(),
        (Tab::Settings, KeyCode::Char('X')) => ask_delete_service(app, &s),
        _ => {}
    }
}

fn util_sha(c: &str) -> String {
    crate::util::short_sha(c).to_string()
}

fn deploy(app: &mut App, key: KeyEvent) {
    let viewport = app.log_viewport.get();
    let Some(view) = app.deploy.as_mut() else {
        return;
    };
    let logs = &mut view.logs;
    match key.code {
        KeyCode::Esc => app.close_deploy(),
        KeyCode::Char('f') => {
            logs.follow = !logs.follow;
            if logs.follow {
                logs.offset = 0;
            }
        }
        KeyCode::Char('w') => logs.wrap = !logs.wrap,
        KeyCode::Up | KeyCode::Char('k') => logs.scroll(1, viewport),
        KeyCode::Down | KeyCode::Char('j') => logs.scroll(-1, viewport),
        KeyCode::PageUp => logs.scroll(viewport as isize - 2, viewport),
        KeyCode::PageDown => logs.scroll(2 - viewport as isize, viewport),
        KeyCode::Char('g') => logs.scroll(isize::MAX / 2, viewport),
        KeyCode::Char('G') => logs.scroll(isize::MIN / 2, viewport),
        KeyCode::Char('c') => {
            let d = &view.deployment;
            if d.status.is_finished() {
                return app.error("this deployment already finished");
            }
            let (n, id) = (d.number, d.id.clone());
            ask(
                app,
                "Cancel deployment",
                format!("Cancel deployment #{n}? The live container keeps running."),
                true,
                ConfirmAction::CancelDeploy(id),
            );
        }
        KeyCode::Char('d') => {
            let id = view.deployment.service_id.clone();
            app.deploy_service(&id);
        }
        _ => {}
    }
}

fn servers(app: &mut App, key: KeyEvent) {
    let n = app.servers.len();
    let selected = app.servers.get(app.server_row).cloned();
    match key.code {
        KeyCode::Esc => {
            app.screen = if app.service.is_some() {
                Screen::Service
            } else {
                Screen::Overview
            };
        }
        KeyCode::Up | KeyCode::Char('k') => app.server_row = app.server_row.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            app.server_row = (app.server_row + 1).min(n.saturating_sub(1))
        }
        KeyCode::Char('a') => app.form_server(),
        KeyCode::Char('c') => {
            if let Some(s) = selected {
                app.check_server(&s);
            }
        }
        KeyCode::Char('p') => {
            if let Some(s) = selected {
                app.setup_proxy(&s);
            }
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            if let Some(s) = selected {
                if s.kind == ServerKind::Local {
                    return app.error("the local server cannot be removed");
                }
                ask(
                    app,
                    "Remove server",
                    format!(
                        "Remove {}? Services deployed there keep running but are no longer managed.",
                        s.name
                    ),
                    true,
                    ConfirmAction::RemoveServer(s.id),
                );
            }
        }
        _ => {}
    }
}
