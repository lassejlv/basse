//! Full-screen terminal UI for the deploy daemon, built with ratatui.

mod app;
mod form;
mod keys;
mod mouse;
mod theme;
mod util;
mod view;

use anyhow::Result;
use app::{App, Msg};
use client::Client;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind};
use crossterm::execute;
use futures_util::StreamExt;
use std::time::Duration;

/// Run the TUI against `client` until the user quits. `context` is shown in the top bar.
pub async fn run(client: Client, context: String) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        hook(info);
    }));
    let result = event_loop(&mut terminal, client, context).await;
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    client: Client,
    context: String,
) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Msg)>();
    let mut app = App::new(client, context, tx);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(120));
    app.refresh();
    loop {
        let frame = terminal.draw(|frame| view::render(frame, &app))?;
        if let Some(sel) = app.mouse.selection.as_mut().filter(|s| s.copy_pending) {
            sel.copy_pending = false;
            let text = mouse::selected_text(frame.buffer, sel);
            if !text.trim().is_empty() {
                mouse::copy_to_clipboard(&text);
                let n = text.chars().count();
                app.toast(format!(
                    "copied {n} character{}",
                    if n == 1 { "" } else { "s" }
                ));
            }
            app.mouse.selection = None;
        }
        tokio::select! {
            _ = tick.tick() => app.on_tick(),
            Some(first) = rx.recv() => {
                let mut next = Some(first);
                while let Some((epoch, msg)) = next {
                    if epoch == app.epoch {
                        app.apply(msg);
                    }
                    next = rx.try_recv().ok();
                }
            }
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    app.mouse.selection = None;
                    keys::handle(&mut app, key);
                }
                Some(Ok(Event::Mouse(ev))) => mouse::handle(&mut app, ev),
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            },
        }
        if app.quit {
            return Ok(());
        }
    }
}
