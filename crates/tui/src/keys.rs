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
        KeyCode::Char('S') => return app.open_servers(),
        KeyCode::Char(',') if app.screen != Screen::Settings => return app.open_settings(),
        _ => {}
    }
    match app.screen {
        Screen::Overview => overview(app, key),
        Screen::Service => service(app, key),
        Screen::Deploy => deploy(app, key),
        Screen::Servers => servers(app, key),
        Screen::Settings => settings(app, key),
        Screen::Secrets => secrets(app, key),
        Screen::Storage => storage(app, key),
    }
}

fn storage(app: &mut App, key: KeyEvent) {
    let row = app.storage.row;
    match key.code {
        KeyCode::Esc => app.close_storage(),
        KeyCode::Up | KeyCode::Char('k') => app.select_storage(row.saturating_sub(1)),
        KeyCode::Down | KeyCode::Char('j') => app.select_storage(row + 1),
        KeyCode::Char('a') => app.form_storage(false),
        KeyCode::Enter | KeyCode::Char('e') => app.form_storage(true),
        KeyCode::Char('t') => app.test_storage(),
        KeyCode::Char('x') | KeyCode::Delete => {
            let Some(s) = app.storage.selected().cloned() else {
                return;
            };
            ask_to(
                app,
                &format!("Delete storage · {}", s.name),
                format!(
                    "Delete the connection {}? The daemon refuses while backups or schedules still use it.",
                    s.name
                ),
                true,
                ConfirmAction::DeleteStorage(s.id),
                "delete",
            );
        }
        _ => {}
    }
}

fn secrets(app: &mut App, key: KeyEvent) {
    let row = app.secrets.row;
    match key.code {
        KeyCode::Esc => app.close_secrets(),
        KeyCode::Up | KeyCode::Char('k') => app.select_secret(row.saturating_sub(1)),
        KeyCode::Down | KeyCode::Char('j') => app.select_secret(row + 1),
        KeyCode::Char('a') => app.form_add_secret(),
        KeyCode::Char('g') => app.form_generate_key(),
        KeyCode::Char('v') => app.toggle_reveal(),
        KeyCode::Char('c') => app.copy_public_part(),
        KeyCode::Char('C') => app.copy_secret_value(),
        KeyCode::Char('n') => app.form_rename_secret(),
        KeyCode::Char('e') => app.form_replace_secret(),
        KeyCode::Char('x') | KeyCode::Delete => {
            let Some(s) = app.secrets.selected().cloned() else {
                return;
            };
            ask_to(
                app,
                &format!("Delete secret · {}", s.name),
                format!(
                    "Delete {}? Its value is destroyed and cannot be recovered.",
                    s.name
                ),
                true,
                ConfirmAction::DeleteSecret(s.id),
                "delete",
            );
        }
        _ => {}
    }
}

fn text_input_active(app: &App) -> bool {
    match app.screen {
        Screen::Overview => app.overview.filtering,
        Screen::Service => app.service.as_ref().is_some_and(|v| v.logs.searching),
        _ => false,
    }
}

pub fn finish_text_input(app: &mut App) {
    if text_input_active(app) {
        text_input(app, KeyEvent::from(KeyCode::Enter));
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
            FormAction::Pick(field) => app.pick_for_form(field),
            FormAction::Submit { alt } => {
                if matches!(form.kind, FormKind::EnvVar { .. }) {
                    app.submit_env_form(alt);
                } else {
                    app.submit_form(alt);
                }
            }
        },
        Some(Overlay::Confirm(c)) => match key.code {
            _ if c.busy.is_some() => {}
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') if !c.error.is_empty() => {
                app.overlay = None
            }
            _ if !c.error.is_empty() => {}
            KeyCode::Char('y') | KeyCode::Enter => app.accept_confirm(),
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => app.overlay = None,
            _ => {}
        },
        Some(Overlay::Reveal(r)) => match key.code {
            KeyCode::Char('c') => {
                let (value, what) = (r.value.clone(), r.what);
                app.copy(&value, what);
            }
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => app.overlay = None,
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
        Some(Overlay::Picker(p)) => match key.code {
            KeyCode::Esc => app.picker_cancel(),
            KeyCode::Enter => app.picker_accept(),
            KeyCode::Char('c') => app.picker_copy(),
            KeyCode::Up | KeyCode::Char('k') => p.row = p.row.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                p.row = (p.row + 1).min(p.items.len().saturating_sub(1))
            }
            _ => {}
        },
        Some(Overlay::Help) => app.overlay = None,
        None => {}
    }
}

/// Bracketed paste: the whole text goes into the focused field at once.
pub fn paste(app: &mut App, text: &str) {
    match app.overlay.as_mut() {
        Some(Overlay::Form(form)) => form.paste(text),
        Some(Overlay::Palette(p)) => {
            p.input.push_str(text.lines().next().unwrap_or_default());
            p.row = 0;
        }
        _ => {}
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
        Command::Servers => app.open_servers(),
        Command::Settings => app.open_settings(),
        Command::Secrets => app.open_secrets(),
        Command::Storage => app.open_storage(),
        Command::BackUpInstance => {
            app.open_settings();
            app.form_instance_backup();
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
    ask_to(app, title, message, danger, action, "confirm");
}

fn ask_to(
    app: &mut App,
    title: &str,
    message: String,
    danger: bool,
    action: ConfirmAction,
    verb: &'static str,
) {
    app.overlay = Some(Overlay::Confirm(Confirm {
        title: title.into(),
        message,
        danger,
        action,
        verb,
        busy: None,
        error: Vec::new(),
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
    let tabs = view.tabs();
    let n = tabs.len();
    let at = tabs.iter().position(|t| *t == tab).unwrap_or(0);
    match key.code {
        KeyCode::Esc => return app.close_service(),
        KeyCode::Tab => return app.set_tab(tabs[(at + 1) % n]),
        KeyCode::BackTab => return app.set_tab(tabs[(at + n - 1) % n]),
        KeyCode::Char(c @ '1'..='9') => {
            if let Some(t) = tabs.get(c as usize - '1' as usize) {
                app.set_tab(*t);
            }
            return;
        }
        _ => {}
    }
    let backups = app.backups.get(&view.id).cloned().unwrap_or_default();
    let detail = view.detail.clone();
    let rows = match (tab, &detail) {
        (Tab::Backups, _) => backups.len(),
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
        (Tab::Domains, KeyCode::Enter) => {
            if let Some(domain) = detail.domains.get(row) {
                app.copy(&domain_url(domain), "domain URL");
            }
        }
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
        (Tab::Volumes, KeyCode::Enter) => {
            if let Some(volume) = detail.volumes.get(row) {
                app.copy(&volume.mount_path, "mount path");
            }
        }
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
        (Tab::Database, KeyCode::Char('v')) => {
            if let Some(view) = app.service.as_mut() {
                view.reveal = !view.reveal;
            }
        }
        (Tab::Database, KeyCode::Char('c')) => {
            if let Some(info) = &detail.database {
                app.copy(&info.internal_url, "internal URL");
            }
        }
        (Tab::Database, KeyCode::Char('C')) => match detail.database.as_ref() {
            Some(DatabaseInfo {
                public_url: Some(url),
                ..
            }) => app.copy(url, "public URL"),
            _ => app.error("not exposed · p to expose it first"),
        },
        (Tab::Database, KeyCode::Char('p')) => match detail.database.as_ref() {
            Some(info) if info.public_port.is_some() => ask(
                app,
                "Unexpose database",
                format!(
                    "Close port {} and make {} internal again? Connections from outside drop, and the container restarts once to apply it.",
                    info.public_port.unwrap_or_default(),
                    s.name
                ),
                false,
                ConfirmAction::PublicPort(s.id.clone(), None),
            ),
            Some(_) => app.form_expose(),
            None => {}
        },
        (Tab::Database, KeyCode::Char('R')) => {
            if let Some(info) = &detail.database {
                let user = info.username.as_deref().unwrap_or("the default user");
                ask(
                    app,
                    "Rotate password",
                    format!(
                        "Set a new password for {user} inside {} and update DATABASE_URL? The old password stops working at once. Services that connect to {} keep the old URL until you redeploy them.",
                        s.name, s.name
                    ),
                    true,
                    ConfirmAction::RotatePassword(s.id.clone()),
                );
            }
        }
        (Tab::Backups, KeyCode::Char('b')) => app.backup_now(&s.id, &s.name),
        (Tab::Backups, KeyCode::Char('e')) => app.form_schedule(),
        (Tab::Backups, KeyCode::Enter | KeyCode::Char('R' | 'D' | 'x')) => {
            let Some(b) = backups.get(row) else {
                return app.error("no backup selected · b to take one");
            };
            if key.code != KeyCode::Char('x') && b.status != BackupStatus::Done {
                return app.error(match b.status {
                    BackupStatus::Running => "that backup is still running",
                    _ => "that backup failed and has no file",
                });
            }
            let when = crate::database::created(b.created_at, chrono::Utc::now());
            match key.code {
                KeyCode::Char('R') => ask_to(
                    app,
                    "Restore backup",
                    format!(
                        "Restore {} ({when}) into {}? Everything written since then is replaced. Take a backup first if you may need today's data.",
                        b.file_name, s.name
                    ),
                    true,
                    ConfirmAction::RestoreBackup(b.id.clone(), b.file_name.clone()),
                    "restore",
                ),
                KeyCode::Enter | KeyCode::Char('D') => app.form_download(b),
                _ => ask_to(
                    app,
                    "Delete backup",
                    format!(
                        "Delete {} ({when})? The file is removed from storage.",
                        b.file_name
                    ),
                    true,
                    ConfirmAction::DeleteBackup(b.id.clone()),
                    "delete",
                ),
            }
        }
        (Tab::Settings, KeyCode::Enter | KeyCode::Char('e')) => app.form_edit_service(),
        (Tab::Settings, KeyCode::Char('X')) => ask_delete_service(app, &s),
        _ => {}
    }
}

pub fn domain_url(domain: &Domain) -> String {
    let scheme = if domain.tls { "https" } else { "http" };
    format!("{scheme}://{}", domain.host)
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
                logs.step_anchor = None;
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
        KeyCode::Esc => app.close_servers(),
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(public_key) = app
                .info
                .as_ref()
                .and_then(|info| info.ssh_public_key.clone())
            {
                app.copy(&public_key, "the ssh key");
            }
        }
        KeyCode::Up | KeyCode::Char('k') => app.server_row = app.server_row.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            app.server_row = (app.server_row + 1).min(n.saturating_sub(1))
        }
        KeyCode::Char('a') => app.form_server(),
        KeyCode::Char('K') => {
            if let Some(s) = selected {
                app.pick_server_key(&s);
            }
        }
        KeyCode::Enter | KeyCode::Char('c') => {
            if let Some(s) = selected {
                app.check_server(&s);
            }
        }
        KeyCode::Char('p') => {
            if let Some(s) = selected {
                app.setup_proxy(&s);
            }
        }
        KeyCode::Char('X') => {
            if let Some(s) = selected {
                if s.kind == ServerKind::Local {
                    return app.error("the local server cannot be removed");
                }
                app.form_purge_server(&s);
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

fn settings(app: &mut App, key: KeyEvent) {
    let names = app.settings.context_names();
    let selected = names.get(app.settings.row).cloned();
    if app.settings.focus == SettingsFocus::Backups {
        let row = app.settings.backup_row;
        match key.code {
            KeyCode::Up => return app.select_instance_backup(row.saturating_sub(1)),
            KeyCode::Down => return app.select_instance_backup(row + 1),
            KeyCode::Enter => return,
            KeyCode::Char('D') => return app.form_instance_download(),
            KeyCode::Char('x') | KeyCode::Delete => {
                let Some(b) = app.settings.selected_backup().cloned() else {
                    return;
                };
                let when = crate::database::created(b.created_at, chrono::Utc::now());
                return ask_to(
                    app,
                    "Delete instance backup",
                    format!(
                        "Delete {} ({when})? The archive is removed from its store.",
                        b.file_name
                    ),
                    true,
                    ConfirmAction::DeleteInstanceBackup(b.id),
                    "delete",
                );
            }
            _ => {}
        }
    }
    match key.code {
        KeyCode::Tab | KeyCode::BackTab => {
            app.settings.focus = match app.settings.focus {
                SettingsFocus::Backups => SettingsFocus::Contexts,
                SettingsFocus::Contexts => SettingsFocus::Backups,
            };
            return;
        }
        KeyCode::Char('b') => return app.form_instance_backup(),
        KeyCode::Char('e') => return app.form_instance_schedule(),
        KeyCode::Esc => return app.close_settings(),
        KeyCode::Up => {
            app.settings.row = app.settings.row.saturating_sub(1);
            return;
        }
        KeyCode::Down => {
            app.settings.row = (app.settings.row + 1).min(names.len().saturating_sub(1));
            return;
        }
        KeyCode::Enter => {
            if let Some(name) = selected {
                app.switch_context(&name);
            }
            return;
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            let Some(name) = selected else { return };
            if name == app.context {
                return app.error("switch to another context before removing this one");
            }
            ask(
                app,
                "Remove context",
                format!(
                    "Forget {name} and its token on this machine? The token keeps working until it is rotated on that daemon."
                ),
                true,
                ConfirmAction::RemoveContext(name),
            );
            return;
        }
        _ => {}
    }
    let Some(s) = app.settings.settings.clone() else {
        return;
    };
    match key.code {
        KeyCode::Char('u') => {
            let latest = app
                .settings
                .release
                .as_ref()
                .filter(|r| r.update_available)
                .and_then(|r| r.latest.clone());
            let Some(latest) = latest else {
                return app.toast(format!("{} {} is up to date", brand::DAEMON_BIN, s.version));
            };
            let restart = if s.supervised {
                "The daemon restarts and this screen reconnects."
            } else {
                "systemd does not supervise the daemon, so it restarts itself in place."
            };
            ask_to(
                app,
                &format!("Update · {} → {latest}", s.version),
                format!(
                    "Update {} on {} to {latest}? {restart} Running apps are not touched.",
                    brand::DAEMON_BIN,
                    app.context
                ),
                false,
                ConfirmAction::Update {
                    force: !s.supervised,
                },
                "update",
            );
        }
        KeyCode::Char('i') => app.form_public_ip(),
        KeyCode::Char('s') => app.open_secrets(),
        KeyCode::Char('o') => app.open_storage(),
        KeyCode::Char('m') => {
            let on = !s.manage_proxy;
            app.change_settings(
                common::api::UpdateSettings {
                    manage_proxy: Some(on),
                    ..Default::default()
                },
                format!("proxy auto-manage {}", if on { "on" } else { "off" }),
            );
        }
        KeyCode::Char('r') => app.restart_proxy(),
        KeyCode::Char('t') => ask(
            app,
            "Rotate API token",
            format!(
                "Issue a new token for {}? The old one stops working at once; this machine saves the new one.",
                app.context
            ),
            true,
            ConfirmAction::RotateToken,
        ),
        KeyCode::Char('k') => match &s.ssh_public_key {
            Some(k) => {
                let k = k.clone();
                app.copy(&k, "the ssh key");
            }
            None => app.error("the daemon has no ssh key"),
        },
        KeyCode::Char('c') => {
            let message = match s.reclaimable {
                Some(r) => format!(
                    "Remove {} old build image{} and the build cache to free about {}? The live and previous image of every service are kept for rollback.",
                    r.images,
                    if r.images == 1 { "" } else { "s" },
                    crate::util::bytes(r.total())
                ),
                None => "Remove old build images and the build cache? The live and previous image of every service are kept for rollback.".into(),
            };
            ask(app, "Prune images", message, false, ConfirmAction::Prune);
        }
        _ => {}
    }
}
