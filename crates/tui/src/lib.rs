//! Full-screen terminal UI for the deploy daemon, built with ratatui.

mod app;
mod database;
mod form;
mod keys;
mod mouse;
#[cfg(test)]
mod mouse_tests;
mod notify;
mod picker;
mod setup;
mod theme;
mod util;
mod view;

use anyhow::Result;
use app::{App, Msg};
use client::Client;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyEventKind, MouseEventKind,
};
use crossterm::execute;
use futures_util::StreamExt;
use std::time::Duration;
use tokio::time::Instant;

const HOVER_FRAME_DELAY: Duration = Duration::from_millis(50);

/// Run the TUI against `client` until the user quits. `context` is shown in the top bar.
pub async fn run(client: Client, context: String) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            std::io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste
        );
        hook(info);
    }));
    let result = event_loop(&mut terminal, client, context).await;
    let _ = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
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
    let mut redraw = true;
    let mut hover_frame = None;
    loop {
        if redraw {
            hover_frame = None;
            let frame = terminal.draw(|frame| view::render(frame, &app))?;
            if let Some(sel) = app.mouse.selection.take_if(|s| s.copy_pending) {
                let text = mouse::selected_text(frame.buffer, &sel);
                if !text.trim().is_empty() {
                    mouse::copy_to_clipboard(&text);
                    let n = text.chars().count();
                    app.toast(format!(
                        "copied {n} character{}",
                        if n == 1 { "" } else { "s" }
                    ));
                }
            }
        }
        redraw = true;
        tokio::select! {
            _ = tick.tick() => app.on_tick(),
            _ = tokio::time::sleep_until(hover_frame.unwrap_or_else(Instant::now)), if hover_frame.is_some() => {}
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
                    app.mouse.cancel_gesture();
                    keys::handle(&mut app, key);
                }
                Some(Ok(Event::Mouse(ev))) if ev.kind == MouseEventKind::Moved => {
                    if mouse::handle(&mut app, ev) && hover_frame.is_none() {
                        hover_frame = Some(Instant::now() + HOVER_FRAME_DELAY);
                    }
                    redraw = false;
                }
                Some(Ok(Event::Mouse(ev))) => redraw = mouse::handle(&mut app, ev),
                Some(Ok(Event::Paste(text))) => keys::paste(&mut app, &text),
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
