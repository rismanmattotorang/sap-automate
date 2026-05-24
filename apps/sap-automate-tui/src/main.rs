//! Phase 4: Ratatui terminal UI for the SAP-Automate server.
//!
//! Paper §X-F: five tabs (Sessions, Tools, KB, RAG, Logs) connected to an
//! "admin socket".  This Phase 4 build:
//!   - Implements all five tabs and the keyboard-driven navigation shape
//!     from paper §V-B (1–5 to switch tabs, q to quit, j/k to scroll).
//!   - Drives the layout with a synthetic traffic generator so operators
//!     see the latency budget moving in real time.  The synthetic feed is
//!     swappable for a real admin-endpoint feed in Phase 7.
//!   - Surfaces the `LatencyBreakdown` plumbing added in Phase 3 — the
//!     RAG tab shows dense / sparse / fusion / rerank μs costs live.

mod app;
mod tabs;
mod traffic;
mod ui;

use anyhow::Result;
use app::App;
use clap::Parser;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::Duration;
use tokio::time::interval;

extern crate futures;

#[derive(Parser)]
#[command(name = "sap-automate-tui", about = "Operator TUI for the SAP-Automate MCP server.")]
struct Cli {
    /// Future: connect to a running server's admin endpoint.  Phase 4
    /// ships with a synthetic feed so the UI is exercisable offline.
    #[arg(long)]
    admin_endpoint: Option<String>,

    /// Refresh interval in milliseconds.
    #[arg(long, default_value_t = 100)]
    refresh_ms: u64,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(cli.refresh_ms));

    // Synthetic traffic generator (Phase 7 will replace with a tokio mpsc
    // bound to the server's admin endpoint).
    let mut traffic = traffic::Synthetic::new();

    let result = loop {
        if let Err(e) = terminal.draw(|f| ui::draw(f, &app)) { break Err(e.into()); }

        tokio::select! {
            _ = tick.tick() => {
                // One synthetic event per tick — enough to move the UI
                // without flooding the user's eyes.
                if let Some(ev) = traffic.tick() {
                    app.observe(ev);
                }
            }
            maybe_evt = events.next() => {
                match maybe_evt {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                            KeyCode::Char('1') => app.set_tab(0),
                            KeyCode::Char('2') => app.set_tab(1),
                            KeyCode::Char('3') => app.set_tab(2),
                            KeyCode::Char('4') => app.set_tab(3),
                            KeyCode::Char('5') => app.set_tab(4),
                            KeyCode::Right | KeyCode::Tab => app.next_tab(),
                            KeyCode::Left | KeyCode::BackTab => app.prev_tab(),
                            KeyCode::Char('j') | KeyCode::Down => app.scroll_down(),
                            KeyCode::Char('k') | KeyCode::Up => app.scroll_up(),
                            _ => {}
                        }
                    }
                    Some(Err(e)) => break Err(e.into()),
                    None => break Ok(()),
                    _ => {}
                }
            }
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    result
}
