//! Full-screen terminal UI for the deploy daemon, built with ratatui.

mod app;
mod form;
mod keys;
mod theme;
mod util;
mod view;

use anyhow::Result;
use app::{App, Msg};
use client::Client;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use std::time::Duration;

/// Run the TUI against `client` until the user quits. `context` is shown in the top bar.
pub async fn run(client: Client, context: String) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    let result = event_loop(&mut terminal, client, context).await;
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    client: Client,
    context: String,
) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Msg>();
    let mut app = App::new(client, context, tx);
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(120));
    app.refresh();
    loop {
        terminal.draw(|frame| view::render(frame, &app))?;
        tokio::select! {
            _ = tick.tick() => app.on_tick(),
            Some(msg) = rx.recv() => {
                app.apply(msg);
                while let Ok(msg) = rx.try_recv() {
                    app.apply(msg);
                }
            }
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => keys::handle(&mut app, key),
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
